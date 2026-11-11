//! The unit of sync: one change to one record.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    hlc::Hlc,
    ids::{DeviceId, RecordId},
};

/// A change to a record. `fields` holds the full new column values for `entity`;
/// a `deleted` op is a tombstone. Merging keeps the op with the greater `hlc` per record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Op {
    pub record_id: RecordId,
    /// Entity name as declared by a plugin, e.g. `body_weight`.
    pub entity: String,
    pub fields: Map<String, Value>,
    pub hlc: Hlc,
    pub device_id: DeviceId,
    pub deleted: bool,
}
