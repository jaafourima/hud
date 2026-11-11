# hud

A self-hosted, plugin-based status window for your life: sleep, workouts, meals and calories, weight, phone and PC usage, habits, and whatever else you add. Local-first apps, one server you own, your data in Postgres where Grafana can read it.

> **Status: early development.** Milestone M0 (foundations) is done; M1 (core, local store, desktop shell) is in progress. See the [milestones](https://github.com/jaafourima/hud/milestones).

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
| M1 | Core, local store, desktop shell, weight tracking |
| M2 | Server, auth, sync, Grafana connection |
| M3 | Sleep (toggleable sources) |
| M4 | Workouts |
| M5 | Nutrition (barcode, recipes, adaptive target) |
| M6 | Android app, Health Connect, **v1.0** |
| M7 | Screen time (Linux agent, Android) |
| M8 | Usage limits |
| M9 | Habits |
| M10 | AI meal photos, sensor-based sleep |

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## License

[AGPL-3.0-or-later](LICENSE). Code ported from other projects is credited in [docs/THIRD_PARTY.md](docs/THIRD_PARTY.md). The original integration-based design is kept in [docs/history/brief.md](docs/history/brief.md).
