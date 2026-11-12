# ADR-0005: Per-user data isolation, device tokens, Grafana access

**Status:** accepted

**Isolation.** Every data request runs in a transaction that does `SET LOCAL ROLE hud_app` and sets `app.user_id`. Plugin tables and `ops` carry `user_id` and a row-level-security policy `user_id = current_user_id()`. The server also filters by user in its own queries, but RLS means a forgotten `WHERE` cannot leak another user's rows. The server adds RLS and grants to every plugin table itself (plugins only declare columns), and refuses to start if a plugin table lacks `user_id`.

**Accounts and devices.** Passwords are hashed with argon2 (length-limited so a request cannot demand unbounded work); unknown usernames are verified against a dummy hash so timing does not reveal who exists. A client pairs once with username and password and receives a random 256-bit device token; only its SHA-256 is stored. Re-pairing a device replaces its token. The first account can always be created; others only if `HUD_ALLOW_REGISTRATION` is on.

**Grafana.** Grafana stays unmodified and connects to Postgres directly. `POST /api/grafana` creates a login role per user that can only `SELECT` from `grafana.*` views. Each view is the plugin's `SqlView` filtered by `hud_sys.grafana_user()`, which maps `session_user` (the login role, which a client cannot change) to a user id. Rotating by calling the endpoint again.

**Known limits (tracked):** no rate limiting on login; the device token sits in the client's SQLite file (OS keyring later); the server speaks plain HTTP and expects a VPN or TLS proxy in front; the Grafana role has no expiry.
