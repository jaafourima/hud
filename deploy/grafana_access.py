#!/usr/bin/env python3
"""Create your read-only Grafana login for a hud server and print the .env lines.

    python3 deploy/grafana_access.py http://localhost:8080 alice

Asks for your hud password (or reads $HUD_PASSWORD), pairs a throwaway device ("grafana-setup") and calls
POST /api/grafana. Running it again rotates the Grafana password.
"""
import getpass, json, os, sys, urllib.error, urllib.request, uuid


def post(url, body, token=None):
    req = urllib.request.Request(url, json.dumps(body).encode(), {"Content-Type": "application/json"})
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.load(r)
    except urllib.error.HTTPError as e:
        sys.exit(f"server said {e.code}: {e.read().decode()}")
    except urllib.error.URLError as e:
        sys.exit(f"cannot reach {url}: {e.reason}")


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    base, user = sys.argv[1].rstrip("/"), sys.argv[2]
    pw = os.environ.get("HUD_PASSWORD") or getpass.getpass("hud password: ")
    paired = post(f"{base}/api/devices", {"username": user, "password": pw,
                                          "device_name": "grafana-setup", "device_id": str(uuid.uuid4())})
    access = post(f"{base}/api/grafana", {}, paired["token"])
    print("\n# add to deploy/.env, then: docker compose up -d grafana")
    print(f"HUD_GRAFANA_DB_USER={access['user']}")
    print(f"HUD_GRAFANA_DB_PASSWORD={access['password']}")


main()
