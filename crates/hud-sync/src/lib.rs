//! Wire protocol between hud clients and `hud-server`. JSON over HTTPS.
//!
//! * `POST /api/register`  [`Credentials`]   -> 201
//! * `POST /api/devices`   [`PairRequest`]    -> [`PairResponse`]   (pairs this device; returns its token)
//! * `POST /api/sync`      [`SyncRequest`]    -> [`SyncResponse`]   (`Authorization: Bearer <token>`)
//!
//! A sync request pushes the device's unacknowledged ops and pulls everything newer than
//! `cursor` that came from other devices, in one round trip.

use hud_core::{DeviceId, Op};
use serde::{Deserialize, Serialize};

/// Most ops accepted in one `/api/sync` push, and most returned in one response.
pub const MAX_OPS_PER_REQUEST: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairRequest {
    pub username: String,
    pub password: String,
    /// Human-readable, e.g. "Laptop" or "Pixel 8".
    pub device_name: String,
    /// The client's own id (from its local database), so ops it already made stay attributed to it.
    pub device_id: DeviceId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairResponse {
    pub device_id: DeviceId,
    /// Shown once; the server stores only a hash.
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRequest {
    /// Highest server sequence number this device has already received (0 for a new device).
    pub cursor: i64,
    pub ops: Vec<Op>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResponse {
    /// Store this and send it next time.
    pub cursor: i64,
    /// Ops from this user's other devices, oldest first.
    pub ops: Vec<Op>,
    /// More ops are waiting; sync again right away.
    pub more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
}
