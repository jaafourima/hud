# hud

A self-hosted "status window" for your real life: health, fitness, nutrition, sleep, and phone + PC screen time in one Grafana dashboard. Everything stays on your own devices, no cloud.

```
PHONE                                   NAS
Chompass ─┐                             ┌─ n8n webhook workflow ─┐
openGym ──┼─> Health Connect ─> HC Webhook ──POST──┘            │
Plees ────────> folder ─┐                                       ├─> Postgres ─> Grafana
openGym backup JSON ────┼──> Syncthing ──> NAS sync folder ──> n8n file workflow ─┘
PC                      │
ActivityWatch ─> systemd timer ─> daily JSON export ─┘
```

Full design, app choices and known caveats: [docs/brief.md](docs/brief.md).

## Status

- [x] Postgres schema with upsert keys (`db/migrations/001_init.sql`)
- [x] NAS stack: Postgres + n8n + Grafana (`nas/docker-compose.yml`)
- [x] PC ActivityWatch exporter + systemd timer (`pc/`)
- [ ] n8n workflow A: Health Connect webhook ingest
- [ ] n8n workflow B: Syncthing file ingest
- [ ] Grafana provisioning + dashboard
- [ ] Syncthing / phone app setup docs

See the open issues for the backlog.

## Layout

| Path | What |
|---|---|
| `nas/` | `docker-compose.yml` and `.env.example` for the NAS services |
| `db/migrations/` | SQL run on first Postgres start; every table upserts on a natural key |
| `pc/` | `aw_export.py` (stdlib only) and the systemd user units that run it daily |
| `docs/` | Project brief |

## Quickstart

```sh
# NAS
cd nas && cp .env.example .env   # fill in passwords
docker compose up -d             # Postgres :5432, n8n :5678, Grafana :3000 (LAN only)

# PC: export yesterday's ActivityWatch data every day
mkdir -p ~/.config/systemd/user
cp pc/hud-aw-export.* ~/.config/systemd/user/
# edit the ExecStart path and SYNC_DIR in the .service file, then:
systemctl --user enable --now hud-aw-export.timer

# backfill or resend a day
python3 pc/aw_export.py --date 2026-11-10 --out ~/Sync/hud/activitywatch
```

## Design notes

- **Idempotent ingest.** Every table has a unique key (`date`/`ts` + `source`), so resending overlapping ranges upserts instead of duplicating. This matters because HC Webhook never retries failed requests.
- **LAN only.** Service ports bind to `HUD_BIND_ADDR`; reach the NAS from outside over Tailscale/WireGuard.
- **No secrets in git.** `.env` is ignored; only `.env.example` is committed.
