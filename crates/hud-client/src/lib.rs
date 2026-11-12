//! hud-client: pair a device with a `hud-server` and keep its local [`Store`] in sync.
//!
//! Settings kept in the store: `sync.url`, `sync.token`, `sync.cursor`.
// ponytail: the token sits in the local SQLite file; move it to the OS keyring when packaging.

use std::sync::{Mutex, MutexGuard};

use hud_store::Store;
use hud_sync::{
    Credentials, ErrorBody, MAX_OPS_PER_REQUEST, PairRequest, PairResponse, SyncRequest,
    SyncResponse,
};
use reqwest::StatusCode;
use serde::{Serialize, de::DeserializeOwned};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not connected to a server")]
    NotConfigured,
    #[error("cannot reach the server: {0}")]
    Network(#[from] reqwest::Error),
    /// The server answered with an error; `message` is its explanation.
    #[error("{message}")]
    Server { status: StatusCode, message: String },
    #[error(transparent)]
    Store(#[from] hud_store::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub pushed: usize,
    pub pulled: usize,
}

fn lock(store: &Mutex<Store>) -> MutexGuard<'_, Store> {
    store.lock().unwrap_or_else(|p| p.into_inner())
}

fn http() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
}

async fn post<B: Serialize, R: DeserializeOwned>(
    base: &str,
    path: &str,
    token: Option<&str>,
    body: &B,
) -> Result<R> {
    let mut req = http()?.post(format!("{base}{path}")).json(body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await?;
    let status = resp.status();
    if status.is_success() {
        // Empty bodies (201 on register) deserialize into `()`-like targets via null.
        let bytes = resp.bytes().await?;
        let bytes = if bytes.is_empty() {
            b"null".as_slice().into()
        } else {
            bytes
        };
        return serde_json::from_slice(&bytes).map_err(|e| Error::Server {
            status,
            message: format!("bad response: {e}"),
        });
    }
    let message = resp
        .json::<ErrorBody>()
        .await
        .map(|b| b.error)
        .unwrap_or_else(|_| status.to_string());
    Err(Error::Server { status, message })
}

fn normalize(url: &str) -> String {
    url.trim().trim_end_matches('/').to_owned()
}

/// Pair this device with the server (optionally creating the account first) and remember
/// the token. The next [`sync`] downloads everything from scratch.
pub async fn connect(
    store: &Mutex<Store>,
    url: &str,
    username: &str,
    password: &str,
    device_name: &str,
    create_account: bool,
) -> Result<()> {
    let base = normalize(url);
    if create_account {
        let creds = Credentials {
            username: username.into(),
            password: password.into(),
        };
        post::<_, ()>(&base, "/api/register", None, &creds).await?;
    }
    let device_id = lock(store).device_id();
    let req = PairRequest {
        username: username.into(),
        password: password.into(),
        device_name: device_name.into(),
        device_id,
    };
    let paired: PairResponse = post(&base, "/api/devices", None, &req).await?;

    let s = lock(store);
    s.set_setting("sync.url", &base)?;
    s.set_setting("sync.token", &paired.token)?;
    s.set_setting("sync.cursor", "0")?;
    Ok(())
}

pub fn is_connected(store: &Mutex<Store>) -> bool {
    let s = lock(store);
    matches!(
        (s.setting("sync.url"), s.setting("sync.token")),
        (Ok(Some(_)), Ok(Some(_)))
    )
}

pub fn server_url(store: &Mutex<Store>) -> Option<String> {
    lock(store).setting("sync.url").ok().flatten()
}

/// Forget the server. Local data stays; sync stops.
pub fn disconnect(store: &Mutex<Store>) -> Result<()> {
    let s = lock(store);
    for key in ["sync.url", "sync.token", "sync.cursor"] {
        s.clear_setting(key)?;
    }
    Ok(())
}

/// Push unsent changes and pull new ones until both sides are drained. The store lock is
/// never held across a network call. If it fails midway, whatever was already exchanged
/// stays applied, and the rest is retried on the next call (every step is idempotent).
pub async fn sync(store: &Mutex<Store>) -> Result<Report> {
    let (base, token, mut cursor) = {
        let s = lock(store);
        match (s.setting("sync.url")?, s.setting("sync.token")?) {
            (Some(url), Some(token)) => (
                url,
                token,
                s.setting("sync.cursor")?
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0i64),
            ),
            _ => return Err(Error::NotConfigured),
        }
    };
    let mut report = Report::default();
    loop {
        let pending = lock(store).pending_ops(MAX_OPS_PER_REQUEST)?;
        let req = SyncRequest {
            cursor,
            ops: pending.iter().map(|(_, op)| op.clone()).collect(),
        };
        let resp: SyncResponse = post(&base, "/api/sync", Some(&token), &req).await?;

        {
            let mut s = lock(store);
            report.pulled += s.apply_remote(&resp.ops)?;
            if let Some((seq, _)) = pending.last() {
                s.mark_synced(*seq)?;
            }
            s.set_setting("sync.cursor", &resp.cursor.to_string())?;
        }
        report.pushed += pending.len();
        cursor = resp.cursor;
        if !resp.more && pending.len() < MAX_OPS_PER_REQUEST {
            return Ok(report);
        }
    }
}
