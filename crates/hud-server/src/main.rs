use std::{env, net::SocketAddr, process::ExitCode};

use hud_core::Registry;
use hud_server::{AppState, db, router};

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("hud-server: {msg}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let url = env::var("HUD_DATABASE_URL").map_err(|_| "HUD_DATABASE_URL is not set")?;
    let listen: SocketAddr = env::var("HUD_LISTEN")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()
        .map_err(|e| format!("HUD_LISTEN: {e}"))?;
    let allow_registration =
        env::var("HUD_ALLOW_REGISTRATION").is_ok_and(|v| v == "true" || v == "1");

    let registry =
        Registry::new(vec![Box::new(hud_plugin_body::Body)]).map_err(|e| e.to_string())?;
    let pool = db::connect(&url).map_err(|e| format!("{e:?}"))?;
    db::migrate(&pool, &registry)
        .await
        .map_err(|e| format!("migration failed: {e:?}"))?;

    let mut state =
        AppState::new(pool, registry, allow_registration).map_err(|e| format!("{e:?}"))?;
    if let Ok(host) = env::var("HUD_GRAFANA_DB_HOST") {
        state.grafana_db_host = host.into();
    }
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| format!("cannot bind {listen}: {e}"))?;
    tracing::info!("hud-server listening on {listen}");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|e| e.to_string())
}
