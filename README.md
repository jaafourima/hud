# hud

A self-hosted, plugin-based status window for your life: sleep, workouts, meals and calories, weight, phone and PC usage, habits, and whatever else you add. Local-first apps, one server you own, your data in Postgres where Grafana can read it.

> **Status: early development.** M0 (foundations) and M1 (core, local store, desktop shell, weight tracking) are done; M2 (server, accounts, sync, Grafana) is in review. See the [milestones](https://github.com/jaafourima/hud/milestones).

## Goals

- **One app, many domains.** Each domain is a compile-time plugin behind a shared trait, so new areas don't touch the core ([ADR-0004](docs/adr/0004-plugins.md)).
- **Local-first.** Every device works offline on SQLite and syncs to your server ([ADR-0003](docs/adr/0003-sync.md)).
- **Multi-user, self-hostable.** One `docker compose up` for server + TimescaleDB + Grafana.
- **Rust everywhere.** Axum server, Dioxus desktop and Android UI, with a thin Kotlin shim for Android-only APIs ([ADR-0002](docs/adr/0002-stack.md)).
- **Keeps what works.** Grafana, Health Connect, Open Food Facts/USDA, Tailscale/WireGuard are integrated, not rebuilt.

## Architecture

```
 Desktop app (Dioxus) ─┐                       ┌─> TimescaleDB/Postgres ─> Grafana
 Android app (Dioxus   ├─ SQLite + op log ─sync┤
   + Kotlin shim)      │                       └─ hud-server (Axum): auth, sync, food DB
 Linux agent ──────────┘
```

Workspace: `crates/` (core, store, sync, server, ui), `apps/` (desktop, android, agent), `plugins/` (body, sleep, workouts, nutrition, ...), `deploy/`. Crates appear as their milestones land.

## Roadmap

| | Milestone |
|---|---|
| M0 | Reset, license, CI, ADRs (done) |
| M1 | Core, local store, desktop shell, weight tracking (done) |
| M2 | Server, accounts, sync, Grafana connection (in review) |
| M3 | Sleep (toggleable sources) |
| M4 | Workouts |
| M5 | Nutrition (barcode, recipes, adaptive target) |
| M6 | Android app, Health Connect, **v1.0** |
| M7 | Screen time (Linux agent, Android) |
| M8 | Usage limits |
| M9 | Habits |
| M10 | AI meal photos, sensor-based sleep |

## Run your own server

```sh
cd deploy && cp .env.example .env     # set POSTGRES_PASSWORD and GRAFANA_ADMIN_PASSWORD (hex: openssl rand -hex 24)
docker compose up -d --build          # hud-server :8080, Postgres (internal only), Grafana :3000
```

The server listens on plain HTTP, bound to `HUD_BIND_ADDR` (default this machine). Reach it from your other devices over Tailscale/WireGuard, or put a TLS reverse proxy in front.

1. In the desktop app open **Sync**, enter the server address, tick *Create a new account* and connect. The first account can always be created; set `HUD_ALLOW_REGISTRATION=true` to let others sign up. Other devices connect with the same username and password. Changes sync every 30 seconds.
2. For dashboards: `python3 deploy/grafana_access.py http://localhost:8080 <username>` prints a read-only Postgres login that only sees your own data. Put the two lines in `deploy/.env` and run `docker compose up -d grafana`; the *hud: Body* dashboard is provisioned automatically.

Design notes: [sync](docs/adr/0003-sync.md), [isolation and access](docs/adr/0005-data-isolation-and-access.md).

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features   # server tests need Postgres: see below
cargo run -p hud-desktop   # needs webkit2gtk-4.1, gtk3 and libxdo (Arch: webkit2gtk-4.1 gtk3 xdotool)
```

Server and client integration tests create throwaway databases; point them at any Postgres where the user can create databases and roles:

```sh
HUD_TEST_DATABASE_URL=postgres://user:password@127.0.0.1:5432 cargo test --workspace --all-features
```

Without that variable those tests print a notice and pass, so plain `cargo test` still works.

Data lives in `$HUD_DATA_DIR`, else `$XDG_DATA_HOME/hud`, else `~/.local/share/hud`.

## License

[AGPL-3.0-or-later](LICENSE). Code ported from other projects is credited in [docs/THIRD_PARTY.md](docs/THIRD_PARTY.md). The original integration-based design is kept in [docs/history/brief.md](docs/history/brief.md).
