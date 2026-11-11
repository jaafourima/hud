# Personal Tracking Stack — Project Brief

Goal: track health, fitness, nutrition, sleep, and phone + PC usage, sync between an Android phone and a Linux PC, and visualize everything in one self-hosted dashboard on a NAS. All data stays on my own devices (no cloud).

## Devices & apps

### Android phone — Health Connect is the hub
| Domain | App | Open source | How data leaves the app |
|---|---|---|---|
| Nutrition (macros, photo logging) | **Chompass** (MIT, IzzyOnDroid) | Yes | Writes meals (+ weight if logged) to Health Connect; manual CSV/JSON export as fallback |
| Workouts + weight | **openGym** (AGPL-3.0, standalone APK) | Yes | "Write to Health Connect" toggle (finished workouts + weigh-ins); "Auto-backup on changes" → dated JSON in `Documents/openGym` (keeps newest 14) |
| Sleep | **Plees Tracker** (MIT, F-Droid) | Yes | Does NOT write Health Connect. Backup/export to a device folder → Syncthing |
| Phone usage tracking | **ActivityWatch** (Android) | Yes | Local REST API + aw-sync via shared folder |
| Phone usage limits | **Curbox** (GPL-3.0+, F-Droid) | Yes | Enforcement only, no network permission, nothing to sync |
| Health Connect → NAS bridge | **HC Webhook** (github.com/mcnaveen/health-connect-webhook) | Yes | POSTs Health Connect data as JSON to a configured webhook (WorkManager background sync) |

Decisions already made:
- **Weight is logged only in openGym.** openGym writes it to Health Connect; Chompass reads it from there. Never also log weight in Chompass (creates duplicate Health Connect entries).
- Chompass calorie goal stays on **Auto/Adaptive** (not Locked) so it follows activity (Health Connect burn / 14-day active average).
- Keep openGym as the **APK**, not self-hosted — Health Connect writing works from the phone app only.

### Linux PC
| Domain | App | Notes |
|---|---|---|
| Usage tracking | **ActivityWatch** | aw-server REST API at `localhost:5600` |
| Per-app limits | **ScreenGuard** (github.com/adityakrishnan005-a11y/ScreenGuard) | v0.1.0 (Aug 2026), GNOME Wayland + X11 only, SQLite local. Disable diagnostics in Settings → Diagnostics; optionally cut network entirely |
| Alternative limiter | **Timekpr-nExT** | Account-level hard limits; PlayTime limits individual apps |
| Website limits | **LeechBlock** (browser extension) | Optional |

Cutting network access for a tool on Linux (any of):
- `firejail --net=none <app>`
- Flatpak: `flatpak override --user --unshare=network <app-id>`
- systemd unit: `IPAddressDeny=any`
- nftables rule matching a dedicated user's UID

### NAS
- **Syncthing** — shared folder receiving Plees backups, openGym JSON backups, ActivityWatch exports
- **n8n** — ingestion
- **Postgres** — single database, one table per domain
- **Grafana** — the dashboard (Postgres data source)
- Optional: **Tailscale/WireGuard** so the phone can sync while away from home

## Architecture

```
PHONE                                   NAS
Chompass ─┐                             ┌─ n8n Webhook workflow ─┐
openGym ──┼─> Health Connect ─> HC Webhook ──POST──┘                        │
Plees ────────> folder ─┐                                          ├─> Postgres ─> Grafana
openGym backup JSON ────┼──> Syncthing ──> NAS sync folder ──> n8n File workflow ─┘
ActivityWatch (aw-sync)─┘        ▲
PC                               │
ActivityWatch ─> systemd timer ─> daily JSON export ─┘
ScreenGuard (limits only, local)
```

- Health data path: phone → Health Connect → HC Webhook → n8n → Postgres.
- File path: Syncthing folder on NAS → scheduled n8n workflow → Postgres.
- ActivityWatch's own aw-sync can also run through Syncthing so phone + PC usage appear together in the ActivityWatch UI (Android sync is new — test before relying on it).

## Postgres schema (to design)
Tables, each keyed by date/timestamp + source so re-sent data **upserts** instead of duplicating:
- `nutrition` — date, meal, calories, protein, carbs, fat, (micros where available)
- `body` — timestamp, weight, body_fat
- `workouts` — session start/end, type, volume/sets, calories if provided
- `activity` — date, steps, active calories
- `sleep` — start, end, duration
- `screen_time` — date, device (phone/pc), app, duration

## Grafana panels (target)
- Calories vs. target, protein trend
- Weight line
- Training volume over time
- Sleep duration + bedtime/wake consistency
- Phone vs. PC hours, top apps per day

## Tasks for Claude Code
1. Postgres: create the schema above (SQL migration) with unique constraints for upserts.
2. n8n workflow A — **HC Webhook ingest**: Webhook node → map each Health Connect record type → Postgres upsert. Need one real HC Webhook payload sample to map fields exactly.
3. n8n workflow B — **Syncthing file ingest**: Schedule trigger → read new files in the sync folder (Plees, openGym JSON, ActivityWatch JSON) → parse → Postgres upsert.
4. PC: systemd service + timer that exports the day's ActivityWatch data via its REST API into the Syncthing folder as JSON.
5. Grafana: provision the Postgres data source and a dashboard JSON with the panels above.
6. Docs: Syncthing folder layout and setup steps for each phone app.

## n8n requirements
- Deliver **complete, importable workflow JSON**, not partial node snippets.
- Prefer clean **node-based** solutions over code workarounds.
- **Never use `this.helpers.*`** (e.g. `httpRequest`, `getBinaryDataBuffer`) inside Code nodes — use real nodes instead.

## Known caveats
- HC Webhook has **no retry** for failed requests. Mitigate: upsert by date/timestamp and have it resend overlapping date ranges.
- Fallback if HC Webhook is unreliable: Chompass CSV export + openGym JSON backups via Syncthing.
- Whether openGym includes calories with workouts in Health Connect is unverified — check what arrives.
- Unverified: whether Chompass imports weight from Health Connect into its own history or only uses it for goals. Test by logging a weigh-in in openGym.
- n8n webhook must be reachable only on the LAN (or over Tailscale/WireGuard).
