//! hud-core: domain primitives and the plugin contract shared by every hud crate.

pub mod hlc;
pub mod ids;
pub mod op;
pub mod plugin;

pub use hlc::{Hlc, HlcClock};
pub use ids::{DeviceId, RecordId};
pub use op::Op;
pub use plugin::{
    EntityDef, Manifest, Migration, Plugin, Registry, RegistryError, SettingDef, SettingKind,
    SqlView,
};

/// Crate version, surfaced in the app's About screen and sync handshakes.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
