//! Accounts and device tokens.

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::{
    Json,
    extract::{FromRequestParts, State},
    http::{StatusCode, header, request::Parts},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hud_sync::{Credentials, PairRequest, PairResponse};
use rand::Rng;
use sha2::{Digest, Sha256};
use tokio_postgres::error::SqlState;
use uuid::Uuid;

use crate::{
    AppState,
    error::{AppError, Result},
};

const MIN_PASSWORD: usize = 10;
const MAX_PASSWORD: usize = 128; // bounds the work one request can ask argon2 to do

fn validate(c: &Credentials) -> Result<()> {
    let ok_name = (3..=32).contains(&c.username.len())
        && c.username
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'));
    if !ok_name {
        return Err(AppError::BadRequest(
            "username must be 3-32 characters: letters, digits, _ . -".into(),
        ));
    }
    if !(MIN_PASSWORD..=MAX_PASSWORD).contains(&c.password.len()) {
        return Err(AppError::BadRequest(format!(
            "password must be {MIN_PASSWORD}-{MAX_PASSWORD} characters"
        )));
    }
    Ok(())
}

pub(crate) fn hash_password(password: &str) -> Result<String> {
    // The salt is generated from the OS RNG inside `hash_password`.
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| AppError::Internal(e.to_string()))
}

fn verify_password(password: &str, hash: &str) -> bool {
    hash.parse::<PasswordHash>().is_ok_and(|h| {
        Argon2::default()
            .verify_password(password.as_bytes(), &h)
            .is_ok()
    })
}

/// `POST /api/register`. Open only while there are no users, unless the operator enabled
/// `HUD_ALLOW_REGISTRATION`.
pub async fn register(State(s): State<AppState>, Json(c): Json<Credentials>) -> Result<StatusCode> {
    validate(&c)?;
    let password = c.password.clone();
    let hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))??;

    let mut client = s.pool.get().await?;
    let tx = client.transaction().await?;
    tx.execute("SELECT pg_advisory_xact_lock(7447002)", &[])
        .await?; // no two "first users"
    let users: i64 = tx
        .query_one("SELECT count(*) FROM users", &[])
        .await?
        .get(0);
    if users > 0 && !s.allow_registration {
        return Err(AppError::Forbidden(
            "registration is closed on this server".into(),
        ));
    }
    tx.execute(
        "INSERT INTO users(username, password_hash) VALUES ($1, $2)",
        &[&c.username, &hash],
    )
    .await
    .map_err(|e| {
        if e.code() == Some(&SqlState::UNIQUE_VIOLATION) {
            AppError::Conflict("that username is taken".into())
        } else {
            e.into()
        }
    })?;
    tx.commit().await?;
    Ok(StatusCode::CREATED)
}

/// Returns the user id if the credentials are right. Takes the same time for unknown users
/// as for wrong passwords, so response timing doesn't reveal which usernames exist.
async fn check_credentials(s: &AppState, username: &str, password: &str) -> Result<Uuid> {
    let client = s.pool.get().await?;
    let row = client
        .query_opt(
            "SELECT id, password_hash FROM users WHERE lower(username) = lower($1)",
            &[&username],
        )
        .await?;
    let (id, hash) = match row {
        Some(r) => (Some(r.get::<_, Uuid>(0)), r.get::<_, String>(1)),
        None => (None, s.dummy_hash.to_string()),
    };
    let password = password.to_owned();
    let ok = tokio::task::spawn_blocking(move || verify_password(&password, &hash))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    match (id, ok) {
        (Some(id), true) => Ok(id),
        _ => Err(AppError::Unauthorized),
    }
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// `POST /api/devices`: check the password, then issue a token for this device.
pub async fn pair(
    State(s): State<AppState>,
    Json(r): Json<PairRequest>,
) -> Result<Json<PairResponse>> {
    if r.password.len() > MAX_PASSWORD {
        return Err(AppError::Unauthorized);
    }
    let name = r.device_name.trim();
    if name.is_empty() || name.len() > 64 {
        return Err(AppError::BadRequest(
            "device name must be 1-64 characters".into(),
        ));
    }
    let user_id = check_credentials(&s, &r.username, &r.password).await?;

    let mut raw = [0u8; 32];
    rand::rng().fill_bytes(&mut raw);
    let token = URL_SAFE_NO_PAD.encode(raw);

    let client = s.pool.get().await?;
    // Re-pairing the same device (lost token) replaces its token; another user's device id is refused.
    let row = client
        .query_opt(
            "INSERT INTO devices(id, user_id, name, token_hash) VALUES ($1, $2, $3, $4)
             ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, token_hash = EXCLUDED.token_hash
             WHERE devices.user_id = EXCLUDED.user_id
             RETURNING id",
            &[&r.device_id.as_uuid(), &user_id, &name, &token_hash(&token)],
        )
        .await?;
    if row.is_none() {
        return Err(AppError::Conflict(
            "device id belongs to another account".into(),
        ));
    }
    Ok(Json(PairResponse {
        device_id: r.device_id,
        token,
    }))
}

/// Extractor: the user and device behind a `Authorization: Bearer <token>` header.
pub struct Authed {
    pub user_id: Uuid,
    pub device_id: Uuid,
}

impl FromRequestParts<AppState> for Authed {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, s: &AppState) -> Result<Self> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(AppError::Unauthorized)?;
        let client = s.pool.get().await?;
        let row = client
            .query_opt(
                "UPDATE devices SET last_seen = now() WHERE token_hash = $1 RETURNING id, user_id",
                &[&token_hash(token)],
            )
            .await?
            .ok_or(AppError::Unauthorized)?;
        Ok(Authed {
            device_id: row.get(0),
            user_id: row.get(1),
        })
    }
}
