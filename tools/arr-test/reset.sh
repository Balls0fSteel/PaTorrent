#!/bin/sh
# Clears the *arr queues and nox's torrents, regenerates the payloads and restarts fakeznab.
cd ~/arrtest || exit 1
python3 - <<'EOF'
import sys
sys.argv = ["x"]
import arr
for app in ("sonarr", "radarr"):
    _, q = arr.call(app, "GET", "queue?includeUnknownSeriesItems=true&pageSize=50")
    for r in q.get("records", []):
        print(app, "queue delete", arr.call(app, "DELETE", f"queue/{r['id']}?removeFromClient=true&blocklist=false")[0])
EOF
pkill -f "^python3 fakeznab"
url=${PATORRENT_URL:-http://127.0.0.1:8080}
curl -s -c jar -d "username=${PATORRENT_USER:-admin}&password=${PATORRENT_PASS:-arr-test-pass}" "$url/api/v2/auth/login" >/dev/null
curl -s -b jar -d "hashes=all&deleteFiles=false" "$url/api/v2/torrents/delete" -w "client delete %{http_code}\n"
rm -f jar
rm -rf /srv/fakeznab/* /srv/media/downloads/*
find /srv/media/movies /srv/media/tv -mindepth 2 -type f -delete
setsid nohup python3 fakeznab.py --data /srv/media/downloads --store /srv/fakeznab --base http://172.17.0.1:9117 > fakeznab.log 2>&1 &
for i in $(seq 1 150); do grep -q "pc1:" fakeznab.log && break; sleep 2; done
cat fakeznab.log
