//! hud-core: domain primitives and the plugin contract shared by every hud crate.
//! Filled in by milestone M1.

/// Crate version, surfaced in the app's About screen and sync handshakes.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_set() {
        assert!(!super::VERSION.is_empty());
    }
}
