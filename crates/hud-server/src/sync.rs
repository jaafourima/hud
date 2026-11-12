//! `POST /api/sync`: accept a device's ops, then return what other devices have added.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::{Json, extract::State};
use hud_core::{DeviceId, EntityDef, Hlc, Op, RecordId, Registry};
use hud_sync::{MAX_OPS_PER_REQUEST, SyncRequest, SyncResponse};
use serde_json::{Map, Value};
use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::{
    AppState,
    auth::Authed,
    db::{ident, scope_to_user},
    error::{AppError, Result},
};

/// A timestamp this far ahead of the server clock is rejected: with last-writer-wins one
/// far-future edit would otherwise make every later edit to that record lose.
const MAX_CLOCK_AHEAD_MS: u64 = 24 * 60 * 60 * 1000;

pub async fn sync(
    State(s): State<AppState>,
    auth: Authed,
    Json(req): Json<SyncRequest>,
) -> Result<Json<SyncResponse>> {
    if req.ops.len() > MAX_OPS_PER_REQUEST {
        return Err(AppError::BadRequest(format!(
            "at most {MAX_OPS_PER_REQUEST} ops per request"
        )));
    }
    if req.cursor < 0 {
        return Err(AppError::BadRequest("cursor must not be negative".into()));
    }
    let defs: Vec<EntityDef> = req
        .ops
        .iter()
        .map(|op| validate(&s.registry, &auth, op))
        .collect::<Result<_>>()?;

    let mut client = s.pool.get().await?;
    let tx = client.transaction().await?;
    scope_to_user(&tx, auth.user_id).await?;
    for (op, def) in req.ops.iter().zip(&defs) {
        push(&tx, auth.user_id, def, op).await?;
    }
    let response = pull(&tx, auth.user_id, auth.device_id, req.cursor).await?;
    tx.commit().await?;
    Ok(Json(response))
}

fn validate(registry: &Registry, auth: &Authed, op: &Op) -> Result<EntityDef> {
    let (_, def) = registry
        .entity(&op.entity)
        .ok_or_else(|| AppError::BadRequest(format!("unknown entity `{}`", op.entity)))?;
    if op.device_id.as_uuid() != auth.device_id {
        return Err(AppError::BadRequest(
            "op device_id does not match the authenticated device".into(),
        ));
    }
    if let Some(bad) = op
        .fields
        .keys()
        .find(|k| !def.columns.contains(&k.as_str()))
    {
        return Err(AppError::BadRequest(format!(
            "entity `{}` has no field `{bad}`",
            op.entity
        )));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    if op.hlc.wall_ms() > now + MAX_CLOCK_AHEAD_MS {
        return Err(AppError::BadRequest(
            "op timestamp is too far in the future; check the device clock".into(),
        ));
    }
    Ok(*def)
}

/// Record the op, and apply it to the entity table if it is newer than anything already
/// known for that record (last-writer-wins by HLC; the same rule as `hud-store`).
async fn push(tx: &Transaction<'_>, user: Uuid, def: &EntityDef, op: &Op) -> Result<()> {
    let record = op.record_id.as_uuid();
    let newest: Option<String> = tx
        .query_one(
            "SELECT max(hlc COLLATE \"C\") FROM ops WHERE user_id = $1 AND entity = $2 AND record_id = $3",
            &[&user, &op.entity, &record],
        )
        .await?
        .get(0);
    let fields = Value::Object(op.fields.clone());
    let inserted = tx
        .execute(
            "INSERT INTO ops(user_id, record_id, entity, fields, hlc, device_id, deleted)
             VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT DO NOTHING",
            &[
                &user,
                &record,
                &op.entity,
                &fields,
                &op.hlc.to_string(),
                &op.device_id.as_uuid(),
                &op.deleted,
            ],
        )
        .await?;
    if inserted == 0 {
        return Ok(()); // already have this exact op
    }
    let is_newest = match newest {
        None => true,
        Some(prev) => {
            op.hlc
                > prev
                    .parse::<Hlc>()
                    .map_err(|e| AppError::Internal(e.to_string()))?
        }
    };
    if !is_newest {
        return Ok(());
    }

    let table = ident(def.table)?;
    if op.deleted {
        tx.execute(
            &format!("DELETE FROM {table} WHERE user_id = $1 AND id = $2"),
            &[&user, &record],
        )
        .await?;
        return Ok(());
    }
    // Build the whole row as JSON and let Postgres coerce it to the table's column types.
    let mut row: Map<String, Value> = op.fields.clone();
    row.insert("user_id".into(), user.to_string().into());
    row.insert("id".into(), record.to_string().into());
    row.insert("hlc".into(), op.hlc.to_string().into());
    row.insert("device_id".into(), op.device_id.to_string().into());
    let updates: Vec<String> = ["hlc", "device_id"]
        .iter()
        .chain(def.columns)
        .map(|c| ident(c).map(|c| format!("{c} = EXCLUDED.{c}")))
        .collect::<Result<_>>()?;
    tx.execute(
        &format!(
            "INSERT INTO {table} SELECT * FROM jsonb_populate_record(NULL::{table}, $1::jsonb) WHERE true
             ON CONFLICT (user_id, id) DO UPDATE SET {}",
            updates.join(", ")
        ),
        &[&Value::Object(row)],
    )
    .await?;
    Ok(())
}

async fn pull(tx: &Transaction<'_>, user: Uuid, device: Uuid, cursor: i64) -> Result<SyncResponse> {
    let rows = tx
        .query(
            "SELECT seq, record_id, entity, fields, hlc, device_id, deleted FROM ops
             WHERE user_id = $1 AND seq > $2 ORDER BY seq LIMIT $3",
            &[&user, &cursor, &(MAX_OPS_PER_REQUEST as i64 + 1)],
        )
        .await?;
    let more = rows.len() > MAX_OPS_PER_REQUEST;
    let rows = &rows[..rows.len().min(MAX_OPS_PER_REQUEST)];
    let cursor = rows.last().map_or(cursor, |r| r.get::<_, i64>(0));

    let mut ops = Vec::new();
    for r in rows.iter().filter(|r| r.get::<_, Uuid>(5) != device) {
        let Value::Object(fields) = r.get::<_, Value>(3) else {
            return Err(AppError::Internal("ops.fields is not an object".into()));
        };
        ops.push(Op {
            record_id: RecordId::from(r.get::<_, Uuid>(1)),
            entity: r.get(2),
            fields,
            hlc: r
                .get::<_, String>(4)
                .parse()
                .map_err(|e: hud_core::hlc::ParseHlcError| AppError::Internal(e.to_string()))?,
            device_id: DeviceId::from(r.get::<_, Uuid>(5)),
            deleted: r.get(6),
        });
    }
    Ok(SyncResponse { cursor, ops, more })
}
