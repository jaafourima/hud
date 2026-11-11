# ADR-0004: Compile-time plugins behind a `Plugin` trait

**Status:** accepted

Each domain (body, sleep, workouts, nutrition, screen time, habits, ...) is a crate implementing `hud_core::Plugin`: manifest, settings schema, entity definitions and migrations, Grafana views, optional server routes, optional UI contributions (pages, widgets, quick-log actions). Apps list the plugins they include in one explicit registry; users enable or disable them per account at runtime.

Plugins own their tables (prefixed `<plugin>_`) and talk to each other only through small read-only traits, optional when the other plugin is absent.

**Consequences:** type-safe and simple, but adding a plugin means rebuilding. Runtime loading (WASM) can be revisited once the plugin API settles.
