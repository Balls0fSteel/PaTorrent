#!/usr/bin/env python3
"""Drives Sonarr, Radarr and Prowlarr through their APIs for the PaTorrent shim test."""

import json
import os
import re
import sys
import urllib.error
import urllib.request

APPS = {"sonarr": (8989, "v3"), "radarr": (7878, "v3"), "prowlarr": (9696, "v1")}
CATEGORY_FIELD = {"sonarr": "tvCategory", "radarr": "movieCategory", "prowlarr": "category"}
# the client's Web UI as the apps reach it (the host is `client`'s argument); the defaults are nox's
CLIENT_PORT = int(os.environ.get("PATORRENT_PORT", "8080"))
CLIENT_USER = os.environ.get("PATORRENT_USER", "admin")
NOX_PASS = os.environ.get("PATORRENT_PASS", "arr-test-pass")
FAKEZNAB = os.environ.get("FAKEZNAB_URL", "http://172.17.0.1:9117")


def key(app):
    with open(f"/srv/arr/{app}/config.xml") as f:
        return re.search(r"<ApiKey>([^<]+)", f.read()).group(1)


def call(app, method, path, body=None):
    port, ver = APPS[app]
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{port}/api/{ver}/{path}", data=data, method=method)
    req.add_header("X-Api-Key", key(app))
    req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=120) as r:
            text = r.read().decode()
            return r.status, json.loads(text) if text else None
    except urllib.error.HTTPError as e:
        text = e.read().decode()
        try:
            return e.code, json.loads(text)
        except ValueError:
            return e.code, text


def fill(schema, values):
    for f in schema["fields"]:
        if f["name"] in values:
            f["value"] = values[f["name"]]
    return schema


def client(app, host):
    _, schemas = call(app, "GET", "downloadclient/schema")
    s = next(x for x in schemas if x["implementation"] == "QBittorrent")
    values = {"host": host, "port": CLIENT_PORT, "useSsl": False, "username": CLIENT_USER, "password": NOX_PASS,
              CATEGORY_FIELD[app]: {"sonarr": "tv-sonarr", "radarr": "radarr", "prowlarr": "prowlarr"}[app]}
    s = fill(s, values)
    s.update(name="PaTorrent", enable=True, priority=1)
    if app != "prowlarr":
        s.update(removeCompletedDownloads=True, removeFailedDownloads=True)
    st, res = call(app, "POST", "downloadclient/test", s)
    print(f"{app} test against {host}: HTTP {st}")
    if st >= 400:
        print(json.dumps(res, indent=1)[:2000])
        return
    _, existing = call(app, "GET", "downloadclient")
    for e in existing:
        call(app, "DELETE", f"downloadclient/{e['id']}")
    st, res = call(app, "POST", "downloadclient", s)
    print(f"{app} client saved: HTTP {st}")


def indexer(app, seed_time=None, seed_ratio=None):
    _, schemas = call(app, "GET", "indexer/schema")
    s = next(x for x in schemas if x["implementation"] == "Torznab")
    cats = {"sonarr": [5000, 5040], "radarr": [2000, 2040]}[app]
    values = {"baseUrl": FAKEZNAB, "apiPath": "/api", "apiKey": "x", "categories": cats}
    if app == "sonarr":
        values["animeCategories"] = []
    if seed_time is not None:
        values["seedCriteria.seedTime"] = seed_time
    if seed_ratio is not None:
        values["seedCriteria.seedRatio"] = seed_ratio
    s = fill(s, values)
    s.update(name="fakeznab", enableRss=False, enableAutomaticSearch=True, enableInteractiveSearch=True,
             priority=1)
    _, existing = call(app, "GET", "indexer")
    for e in existing:
        call(app, "DELETE", f"indexer/{e['id']}")
    st, res = call(app, "POST", "indexer", s)
    print(f"{app} indexer: HTTP {st}", "" if st < 400 else json.dumps(res)[:1500])


def pathmap(app, host, remote, local):
    """A Remote Path Mapping: the client's folder `remote` on `host` is `local` in the app."""
    _, existing = call(app, "GET", "remotepathmapping")
    for e in existing:
        if e["host"] == host:
            call(app, "DELETE", f"remotepathmapping/{e['id']}")
    st, res = call(app, "POST", "remotepathmapping", {"host": host, "remotePath": remote, "localPath": local})
    print(f"{app} path mapping {remote} -> {local}: HTTP {st}", "" if st < 400 else json.dumps(res)[:1500])


def get(app, path):
    st, res = call(app, "GET", path)
    print(json.dumps(res, indent=1)[:6000])


def main():
    cmd, app, *rest = sys.argv[1:]
    if cmd == "client":
        client(app, rest[0])
    elif cmd == "indexer":
        seed_time, seed_ratio = rest
        indexer(app, None if seed_time == "-" else int(seed_time), None if seed_ratio == "-" else float(seed_ratio))
    elif cmd == "pathmap":
        pathmap(app, *rest)
    elif cmd == "get":
        get(app, rest[0])
    elif cmd in ("post", "put", "delete"):
        st, res = call(app, cmd.upper(), rest[0], json.loads(rest[1]) if len(rest) > 1 else None)
        print(st, json.dumps(res, indent=1)[:6000])
    else:
        sys.exit(f"unknown command {cmd}")


if __name__ == "__main__":
    main()
