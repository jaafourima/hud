//! `POST /api/grafana`: a read-only Postgres login for pointing Grafana at your own data.

use axum::{Json, extract::State};
use rand::{RngExt, distr::Alphanumeric};
use serde::Serialize;

use crate::{AppState, auth::Authed, db::ident, error::Result};

#[derive(Serialize)]
pub struct GrafanaAccess {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: String,
    /// Grafana's PostgreSQL data source reads views from this schema.
    pub schema: &'static str,
}

/// Creates (or rotates) the caller's Grafana login. The role can only read the `grafana.*`
/// views, and those views return only the owner's rows.
pub async fn create(State(s): State<AppState>, auth: Authed) -> Result<Json<GrafanaAccess>> {
    let mut client = s.pool.get().await?;
    let tx = client.transaction().await?;
    tx.execute("SELECT pg_advisory_xact_lock(7447003)", &[])
        .await?;

    for old in tx
        .query(
            "SELECT role_name FROM hud_sys.grafana_access WHERE user_id = $1",
            &[&auth.user_id],
        )
        .await?
    {
        let name: String = old.get(0);
        let name = ident(&name)?; // we wrote it, but it is spliced into SQL below
        tx.batch_execute(&format!("DROP OWNED BY {name}; DROP ROLE {name};"))
            .await?;
    }
    tx.execute(
        "DELETE FROM hud_sys.grafana_access WHERE user_id = $1",
        &[&auth.user_id],
    )
    .await?;

    let user = format!("hud_grafana_{}", &auth.user_id.simple().to_string()[..16]);
    let password: String = rand::rng()
        .sample_iter(Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    // The password is server-generated alphanumerics, so quoting it into the statement is safe.
    tx.batch_execute(&format!(
        "CREATE ROLE {} LOGIN PASSWORD '{password}' IN ROLE hud_grafana",
        ident(&user)?
    ))
    .await?;
    tx.execute(
        "INSERT INTO hud_sys.grafana_access(role_name, user_id) VALUES ($1, $2)",
        &[&user, &auth.user_id],
    )
    .await?;
    let database: String = tx.query_one("SELECT current_database()", &[]).await?.get(0);
    tx.commit().await?;

    Ok(Json(GrafanaAccess {
        host: s.grafana_db_host.to_string(),
        port: s.grafana_db_port,
        database,
        user,
        password,
        schema: "grafana",
    }))
}
