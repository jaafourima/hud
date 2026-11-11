#!/usr/bin/env python3
"""Export one local day of ActivityWatch events to JSON for the Syncthing folder.

Usage: aw_export.py [--date YYYY-MM-DD] [--out DIR] [--server URL]
Defaults: yesterday, $HUD_SYNC_DIR/activitywatch, http://localhost:5600.
Re-running a date overwrites its file, so resending is safe (the ingest upserts).
"""
import argparse, json, os, sys, tempfile
from datetime import date, datetime, time, timedelta
from urllib.parse import quote, urlencode
from urllib.request import urlopen


def get(server, path):
    with urlopen(f"{server}/api/0/{path}", timeout=30) as r:
        return json.load(r)


def export(server, day):
    # local-day bounds, tz-aware so aw-server gets unambiguous instants
    start = datetime.combine(day, time.min).astimezone()
    end = datetime.combine(day + timedelta(days=1), time.min).astimezone()
    q = urlencode({"start": start.isoformat(), "end": end.isoformat(), "limit": -1})
    buckets = {}
    for bid, meta in get(server, "buckets/").items():
        events = get(server, f"buckets/{quote(bid)}/events?{q}")
        buckets[bid] = {"type": meta["type"], "hostname": meta["hostname"], "events": events}
    return {"date": day.isoformat(), "buckets": buckets}


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--date", type=date.fromisoformat, default=date.today() - timedelta(days=1))
    ap.add_argument("--server", default="http://localhost:5600")
    ap.add_argument("--out", default=os.path.join(os.environ.get("HUD_SYNC_DIR", "."), "activitywatch"))
    a = ap.parse_args()

    data = export(a.server.rstrip("/"), a.date)
    os.makedirs(a.out, exist_ok=True)
    dest = os.path.join(a.out, f"pc-{a.date}.json")
    # atomic: Syncthing must never pick up a half-written file
    fd, tmp = tempfile.mkstemp(dir=a.out, suffix=".tmp")
    with os.fdopen(fd, "w") as f:
        json.dump(data, f)
    os.replace(tmp, dest)
    n = sum(len(b["events"]) for b in data["buckets"].values())
    print(f"{dest}: {len(data['buckets'])} buckets, {n} events")


if __name__ == "__main__":
    sys.exit(main())
