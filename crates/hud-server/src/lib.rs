//! hud-server: accounts, device tokens, sync and Grafana access on top of Postgres.
//!
//! Every data request runs in a transaction that first switches to the restricted
//! `hud_app` role and sets `app.user_id`, so Postgres row-level security enforces that a
//! user only ever sees their own rows, even if a query forgot its `WHERE user_id = ...`.

mod auth;
pub mod db;
mod error;
mod grafana;
mod sync;

use std::sync::Arc;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};
use deadpool_postgres::Pool;
use hud_core::Registry;

pub use error::AppError;

#[derive(Clone)]
pub struct AppState {
    pub pool: Pool,
    pub registry: Arc<Registry>,
    /// Let additional users sign up (the first account can always be created).
    pub allow_registration: bool,
    /// Hash verified for unknown usernames so login timing doesn't reveal who exists.
    pub(crate) dummy_hash: Arc<String>,
    /// Host and port Grafana should use to reach Postgres, reported by `/api/grafana`.
    pub grafana_db_host: Arc<String>,
    pub grafana_db_port: u16,
}

impl AppState {
    pub fn new(pool: Pool, registry: Registry, allow_registration: bool) -> Result<Self, AppError> {
        Ok(Self {
            pool,
            registry: Arc::new(registry),
            allow_registration,
            dummy_hash: Arc::new(auth::hash_password("not-a-real-password")?),
            grafana_db_host: Arc::new("postgres".to_owned()),
            grafana_db_port: 5432,
        })
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/register", post(auth::register))
        .route("/api/devices", post(auth::pair))
        .route("/api/sync", post(sync::sync))
        .route("/api/grafana", post(grafana::create))
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
        .with_state(state)
}
