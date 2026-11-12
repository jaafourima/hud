//! hud-store: the client-side SQLite database.
//!
//! * Runs each plugin's migrations (tracked per plugin in `_migrations`).
//! * Every local write becomes an [`Op`], is applied to the plugin's table and queued in
//!   `_outbox` until the server acknowledges it.
//! * Remote ops are applied with last-writer-wins per record, ordered by [`Hlc`]. Deletes
//!   remove the row and leave a tombstone, so a late-arriving older edit can't resurrect it.
//!   The result depends only on the *set* of ops, not their arrival order or duplication.
//!
//! Entity tables declared by plugins must have `id TEXT PRIMARY KEY`, `hlc TEXT NOT NULL`
//! and `device_id TEXT NOT NULL`, plus the columns listed in their `EntityDef`.

use std::path::Path;

use hud_core::{DeviceId, EntityDef, Hlc, HlcClock, Op, RecordId, Registry};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value as Sql};
use serde_json::{Map, Value};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("unknown entity `{0}`")]
    UnknownEntity(String),
    #[error("entity `{entity}` has no column `{field}`")]
    UnknownField { entity: String, field: String },
    #[error("invalid SQL identifier `{0}` in a plugin definition")]
    BadIdentifier(String),
    #[error("table `{table}` is missing required column `{column}`")]
    MissingColumn { table: String, column: &'static str },
    #[error("corrupt stored value: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, Error>;

const STANDARD_COLUMNS: [&str; 3] = ["id", "hlc", "device_id"];

pub struct Store {
    conn: Connection,
    registry: Registry,
    device: DeviceId,
    clock: HlcClock,
}

impl Store {
    pub fn open(path: impl AsRef<Path>, registry: Registry) -> Result<Self> {
        Self::init(Connection::open(path)?, registry)
    }

    pub fn open_in_memory(registry: Registry) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, registry)
    }

    fn init(mut conn: Connection, registry: Registry) -> Result<Self> {
        // journal_mode returns a row, so read it instead of using pragma_update.
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS _migrations(plugin TEXT NOT NULL, version INTEGER NOT NULL, PRIMARY KEY(plugin, version));
             CREATE TABLE IF NOT EXISTS _outbox(seq INTEGER PRIMARY KEY AUTOINCREMENT, op TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS _tombstones(entity TEXT NOT NULL, id TEXT NOT NULL, hlc TEXT NOT NULL, PRIMARY KEY(entity, id));",
        )?;

        let device = match meta(&conn, "device_id")? {
            Some(s) => s.parse().map_err(|_| Error::Corrupt("device_id".into()))?,
            None => {
                let d = DeviceId::new();
                set_meta(&conn, "device_id", &d.to_string())?;
                d
            }
        };
        let last = meta(&conn, "last_hlc")?
            .map(|s| s.parse::<Hlc>().map_err(|e| Error::Corrupt(e.to_string())))
            .transpose()?;
        let clock = HlcClock::resume(device, last);

        for plugin in registry.plugins() {
            let id = plugin.manifest().id;
            let mut migrations = plugin.migrations().to_vec();
            migrations.sort_by_key(|m| m.version);
            for m in migrations {
                let done: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM _migrations WHERE plugin=?1 AND version=?2)",
                    params![id, m.version],
                    |r| r.get(0),
                )?;
                if !done {
                    let tx = conn.transaction()?;
                    tx.execute_batch(m.sql)?;
                    tx.execute(
                        "INSERT INTO _migrations(plugin, version) VALUES (?1, ?2)",
                        params![id, m.version],
                    )?;
                    tx.commit()?;
                }
            }
            for e in plugin.entities() {
                check_entity(&conn, e)?;
            }
        }
        Ok(Self {
            conn,
            registry,
            device,
            clock,
        })
    }

    pub fn device_id(&self) -> DeviceId {
        self.device
    }

    /// Direct read access for plugin queries. Writes must go through [`Store::upsert`]
    /// and [`Store::delete`] so they are synced.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Create or replace a record with the given column values. Columns not in `fields`
    /// become NULL, so pass the full record.
    pub fn upsert(&mut self, entity: &str, id: RecordId, fields: Map<String, Value>) -> Result<Op> {
        self.write(entity, id, fields, false)
    }

    pub fn delete(&mut self, entity: &str, id: RecordId) -> Result<Op> {
        self.write(entity, id, Map::new(), true)
    }

    fn write(
        &mut self,
        entity: &str,
        id: RecordId,
        fields: Map<String, Value>,
        deleted: bool,
    ) -> Result<Op> {
        let def = *entity_def(&self.registry, entity)?;
        let op = Op {
            record_id: id,
            entity: entity.to_owned(),
            fields,
            hlc: self.clock.now(),
            device_id: self.device,
            deleted,
        };
        let tx = self.conn.transaction()?;
        apply_op(&tx, &def, &op)?;
        tx.execute(
            "INSERT INTO _outbox(op) VALUES (?1)",
            params![serde_json::to_string(&op)?],
        )?;
        set_meta(&tx, "last_hlc", &op.hlc.to_string())?;
        tx.commit()?;
        Ok(op)
    }

    /// A small key-value store for app settings (server URL, sync cursor, ...). Not synced.
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        meta(&self.conn, &format!("setting:{key}"))
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        set_meta(&self.conn, &format!("setting:{key}"), value)
    }

    pub fn clear_setting(&self, key: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM _meta WHERE key = ?1",
            params![format!("setting:{key}")],
        )?;
        Ok(())
    }

    /// Apply ops received from the server. Returns how many changed local state.
    /// Safe to call with ops in any order, with duplicates, or ones already applied.
    /// Ops for entities this build has no plugin for are skipped, not errors: another
    /// device may run plugins this one lacks.
    // ponytail: skipped ops are not replayed if the plugin is added later; reset the sync cursor to 0 then.
    pub fn apply_remote(&mut self, ops: &[Op]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut applied = 0;
        for op in ops {
            let Ok(def) = entity_def(&self.registry, &op.entity).copied() else {
                continue;
            };
            self.clock.observe_now(&op.hlc);
            applied += usize::from(apply_op(&tx, &def, op)?);
        }
        if let Some(h) = self.clock.last() {
            set_meta(&tx, "last_hlc", &h.to_string())?;
        }
        tx.commit()?;
        Ok(applied)
    }

    /// Oldest-first ops not yet acknowledged by the server, with their outbox sequence numbers.
    pub fn pending_ops(&self, limit: usize) -> Result<Vec<(i64, Op)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT seq, op FROM _outbox ORDER BY seq LIMIT ?1")?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.map(|r| {
            let (seq, json) = r?;
            Ok((seq, serde_json::from_str(&json)?))
        })
        .collect()
    }

    /// Drop outbox entries up to and including `seq` once the server has acknowledged them.
    pub fn mark_synced(&self, seq: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM _outbox WHERE seq <= ?1", params![seq])?;
        Ok(())
    }

    /// One record as a column map (including `id`, `hlc`, `device_id`), if it exists.
    pub fn get(&self, entity: &str, id: RecordId) -> Result<Option<Map<String, Value>>> {
        let def = entity_def(&self.registry, entity)?;
        let names: Vec<&str> = STANDARD_COLUMNS
            .iter()
            .chain(def.columns)
            .copied()
            .collect();
        let cols = names
            .iter()
            .map(|c| quote(c))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("SELECT {cols} FROM {} WHERE id = ?1", quote(def.table));
        Ok(self
            .conn
            .query_row(&sql, params![id.to_string()], |r| {
                names
                    .iter()
                    .enumerate()
                    .map(|(i, n)| Ok(((*n).to_owned(), to_json(r.get::<_, Sql>(i)?))))
                    .collect()
            })
            .optional()?)
    }
}

fn entity_def<'a>(registry: &'a Registry, name: &str) -> Result<&'a EntityDef> {
    registry
        .entity(name)
        .map(|(_, e)| e)
        .ok_or_else(|| Error::UnknownEntity(name.to_owned()))
}

/// Apply one op with LWW semantics. Returns whether local state changed.
fn apply_op(conn: &Connection, def: &EntityDef, op: &Op) -> Result<bool> {
    let table = quote(def.table);
    let id = op.record_id.to_string();

    // The newest op we already hold for this record: its row's hlc, or its tombstone's.
    let row_hlc = stored_hlc(
        conn,
        &format!("SELECT hlc FROM {table} WHERE id = ?1"),
        params![id],
    )?;
    let tomb_hlc = stored_hlc(
        conn,
        "SELECT hlc FROM _tombstones WHERE entity = ?1 AND id = ?2",
        params![def.name, id],
    )?;
    if row_hlc.into_iter().chain(tomb_hlc).any(|h| h >= op.hlc) {
        return Ok(false);
    }

    if op.deleted {
        conn.execute(&format!("DELETE FROM {table} WHERE id = ?1"), params![id])?;
        conn.execute(
            "INSERT INTO _tombstones(entity, id, hlc) VALUES (?1, ?2, ?3)
             ON CONFLICT(entity, id) DO UPDATE SET hlc = excluded.hlc",
            params![def.name, id, op.hlc.to_string()],
        )?;
        return Ok(true);
    }

    if let Some(bad) = op
        .fields
        .keys()
        .find(|k| !def.columns.contains(&k.as_str()))
    {
        return Err(Error::UnknownField {
            entity: def.name.to_owned(),
            field: bad.clone(),
        });
    }
    let names: Vec<&str> = STANDARD_COLUMNS
        .iter()
        .chain(def.columns)
        .copied()
        .collect();
    let cols = names
        .iter()
        .map(|c| quote(c))
        .collect::<Vec<_>>()
        .join(", ");
    let marks = (1..=names.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let updates = names[1..]
        .iter()
        .map(|c| format!("{0} = excluded.{0}", quote(c)))
        .collect::<Vec<_>>()
        .join(", ");
    let mut values = vec![
        Sql::Text(id.clone()),
        Sql::Text(op.hlc.to_string()),
        Sql::Text(op.device_id.to_string()),
    ];
    values.extend(
        def.columns
            .iter()
            .map(|c| to_sql(op.fields.get(*c).unwrap_or(&Value::Null))),
    );
    conn.execute(
        &format!(
            "INSERT INTO {table}({cols}) VALUES ({marks}) ON CONFLICT(id) DO UPDATE SET {updates}"
        ),
        params_from_iter(values),
    )?;
    conn.execute(
        "DELETE FROM _tombstones WHERE entity = ?1 AND id = ?2",
        params![def.name, id],
    )?;
    Ok(true)
}

fn stored_hlc(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<Option<Hlc>> {
    conn.query_row(sql, p, |r| r.get::<_, String>(0))
        .optional()?
        .map(|s| {
            s.parse()
                .map_err(|e: hud_core::hlc::ParseHlcError| Error::Corrupt(e.to_string()))
        })
        .transpose()
}

fn check_entity(conn: &Connection, def: &EntityDef) -> Result<()> {
    for name in std::iter::once(def.table).chain(def.columns.iter().copied()) {
        if name.is_empty()
            || name.starts_with('_')
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(Error::BadIdentifier(name.to_owned()));
        }
    }
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", quote(def.table)))?;
    let have: Vec<String> = stmt
        .query_map([], |r| r.get(1))?
        .collect::<std::result::Result<_, _>>()?;
    for col in STANDARD_COLUMNS.iter().chain(def.columns) {
        if !have.iter().any(|h| h == col) {
            return Err(Error::MissingColumn {
                table: def.table.to_owned(),
                column: col,
            });
        }
    }
    Ok(())
}

fn quote(ident: &str) -> String {
    format!("\"{ident}\"")
}

fn to_sql(v: &Value) -> Sql {
    match v {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(i64::from(*b)),
        Value::Number(n) => n
            .as_i64()
            .map(Sql::Integer)
            .unwrap_or_else(|| Sql::Real(n.as_f64().unwrap_or(f64::NAN))),
        Value::String(s) => Sql::Text(s.clone()),
        other => Sql::Text(other.to_string()),
    }
}

fn to_json(v: Sql) -> Value {
    match v {
        Sql::Null | Sql::Blob(_) => Value::Null,
        Sql::Integer(i) => Value::from(i),
        Sql::Real(f) => Value::from(f),
        Sql::Text(s) => Value::String(s),
    }
}

fn meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM _meta WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()?)
}

fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO _meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
