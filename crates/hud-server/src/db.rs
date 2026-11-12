//! Connection pool, migrations, and the per-request scoped transaction.

use deadpool_postgres::{Client, Config, ManagerConfig, Pool, RecyclingMethod, Runtime};
use hud_core::{EntityDef, Registry};
use tokio_postgres::{NoTls, Transaction};
use uuid::Uuid;

use crate::error::{AppError, Result};

pub const APP_ROLE: &str = "hud_app";

pub fn connect(url: &str) -> Result<Pool> {
    let mut cfg = Config::new();
    cfg.url = Some(url.to_owned());
    // Without this, a login role named like an existing schema (e.g. `hud`) would shadow `public`.
    cfg.options = Some("-c search_path=public".to_owned());
    cfg.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });
    cfg.create_pool(Some(Runtime::Tokio1), NoTls)
        .map_err(|e| AppError::Internal(e.to_string()))
}

const CORE: [(i32, &str); 1] = [(1, include_str!("../migrations/0001_init.sql"))];

/// Bring the database up to date: core schema, each plugin's tables (with row-level
/// security added here), and the Grafana views. Safe to run on every start.
pub async fn migrate(pool: &Pool, registry: &Registry) -> Result<()> {
    let mut client = pool.get().await?;
    // One migrator at a time (several servers, or parallel tests, may start together).
    client
        .execute("SELECT pg_advisory_lock(7447001)", &[])
        .await?;
    let result = run(&mut client, registry).await;
    let _ = client
        .execute("SELECT pg_advisory_unlock(7447001)", &[])
        .await;
    result
}

async fn run(c: &mut Client, registry: &Registry) -> Result<()> {
    c.batch_execute("CREATE TABLE IF NOT EXISTS schema_migrations(scope text NOT NULL, version int NOT NULL, PRIMARY KEY(scope, version))")
        .await?;
    apply(c, "core", &CORE).await?;
    for plugin in registry.plugins() {
        let id = plugin.manifest().id;
        let mut migrations: Vec<(i32, &str)> = plugin
            .server_migrations()
            .iter()
            .map(|m| (m.version as i32, m.sql))
            .collect();
        migrations.sort_by_key(|m| m.0);
        apply(c, id, &migrations).await?;
        for entity in plugin.entities() {
            secure_table(c, entity).await?;
        }
    }
    refresh_grafana_views(c, registry).await
}

async fn apply(c: &mut Client, scope: &str, migrations: &[(i32, &str)]) -> Result<()> {
    for &(version, sql) in migrations {
        let done = c
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE scope = $1 AND version = $2)",
                &[&scope, &version],
            )
            .await?
            .get::<_, bool>(0);
        if !done {
            let tx = c.transaction().await?;
            tx.batch_execute(sql).await?;
            tx.execute(
                "INSERT INTO schema_migrations(scope, version) VALUES ($1, $2)",
                &[&scope, &version],
            )
            .await?;
            tx.commit().await?;
        }
    }
    Ok(())
}

/// Identifiers come from plugin definitions, not users, but are still checked before being
/// spliced into SQL.
pub(crate) fn ident(name: &str) -> Result<&str> {
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        Ok(name)
    } else {
        Err(AppError::Internal(format!(
            "invalid identifier `{name}` in plugin definition"
        )))
    }
}

/// Plugin tables only declare their columns; row-level security and grants are added here
/// so a plugin author cannot forget them.
async fn secure_table(c: &Client, entity: &EntityDef) -> Result<()> {
    let t = ident(entity.table)?;
    let have: Vec<String> = c
        // `to_regclass` resolves the name through the search_path, like the table's own queries do.
        .query(
            "SELECT attname::text FROM pg_attribute WHERE attrelid = to_regclass($1) AND attnum > 0 AND NOT attisdropped",
            &[&t],
        )
        .await?
        .iter()
        .map(|r| r.get(0))
        .collect();
    for col in ["user_id", "id", "hlc", "device_id"]
        .iter()
        .copied()
        .chain(entity.columns.iter().copied())
    {
        if !have.iter().any(|h| h == col) {
            return Err(AppError::Internal(format!(
                "server table `{t}` is missing column `{col}`"
            )));
        }
    }
    c.batch_execute(&format!(
        "ALTER TABLE {t} ENABLE ROW LEVEL SECURITY;
         DROP POLICY IF EXISTS user_isolation ON {t};
         CREATE POLICY user_isolation ON {t}
           USING (user_id = hud_sys.current_user_id()) WITH CHECK (user_id = hud_sys.current_user_id());
         GRANT SELECT, INSERT, UPDATE, DELETE ON {t} TO {APP_ROLE};"
    ))
    .await?;
    Ok(())
}

async fn refresh_grafana_views(c: &Client, registry: &Registry) -> Result<()> {
    for plugin in registry.plugins() {
        for view in plugin.grafana_views() {
            let name = ident(view.name)?;
            c.batch_execute(&format!(
                "DROP VIEW IF EXISTS grafana.{name};
                 CREATE VIEW grafana.{name} AS
                   SELECT * FROM ({inner}) v WHERE v.user_id = hud_sys.grafana_user();
                 GRANT SELECT ON grafana.{name} TO hud_grafana;",
                inner = view.sql
            ))
            .await?;
        }
    }
    Ok(())
}

/// Start of every data request: drop to the restricted role, set who we act for so
/// row-level security applies, and serialize this user's writes (which keeps each user's
/// `ops.seq` values committing in order, so pull cursors never skip rows).
pub async fn scope_to_user(tx: &Transaction<'_>, user_id: Uuid) -> Result<()> {
    tx.batch_execute(&format!("SET LOCAL ROLE {APP_ROLE}"))
        .await?;
    let id = user_id.to_string();
    tx.execute("SELECT set_config('app.user_id', $1, true)", &[&id])
        .await?;
    tx.execute(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        &[&id],
    )
    .await?;
    Ok(())
}
