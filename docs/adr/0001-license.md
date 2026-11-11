# ADR-0001: AGPL-3.0-or-later

**Status:** accepted

hud reuses source from several open-source projects (openGym AGPL-3.0, Curbox GPL-3.0+, ActivityWatch MPL-2.0, Chompass and Plees Tracker MIT). AGPL-3.0-or-later is compatible with all of them, so we can port code from any. It also keeps hosted forks open, which suits a self-hosted tool.

**Consequences:** every dependency must be AGPL-compatible (enforced by `cargo-deny`, see `deny.toml`). Ported code keeps its upstream copyright notice and is listed in `docs/THIRD_PARTY.md`. Upstream licenses are re-checked at port time.
