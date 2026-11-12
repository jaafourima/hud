# ADR-0002: Rust everywhere, Dioxus UI, thin Kotlin shim on Android

**Status:** accepted

- Server, sync, storage, plugins and UI are Rust. UI is Dioxus (MIT/Apache-2.0): one codebase for Linux desktop and Android.
- Android-only APIs (Health Connect, UsageStats, accessibility service, foreground service, camera) have no Rust bindings, so a small Kotlin layer wraps them and is called over JNI. It holds no business logic.
- Server: Axum + sqlx on Postgres (TimescaleDB when available). Clients: SQLite via rusqlite.
- Grafana, Health Connect, Open Food Facts/USDA data and Tailscale/WireGuard are integrated, not rebuilt.

**Consequences:** desktop is built first (fast loop); Android follows in M6.

**Amendment (M2):** the server uses `tokio-postgres` + `deadpool-postgres`, not `sqlx`. Every `sqlx` release resolves its SQLite driver, which links `libsqlite3-sys` at a version that conflicts with the patched one `rusqlite` needs (RUSTSEC-2026-0459). Cargo allows one `links = "sqlite3"` crate per build, and downgrading `rusqlite` to fit was not acceptable. Server TLS is left to a reverse proxy or a VPN (Tailscale/WireGuard); Postgres is only reachable inside the compose network.
