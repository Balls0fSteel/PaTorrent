#!/usr/bin/env python3
"""Prints the client's torrents as the shim reports them (nox, or the Windows GUI's Web UI)."""

import http.cookiejar
import json
import os
import urllib.parse
import urllib.request

URL = os.environ.get("PATORRENT_URL", "http://127.0.0.1:8080").rstrip("/")
USER = os.environ.get("PATORRENT_USER", "admin")
PASS = os.environ.get("PATORRENT_PASS", "arr-test-pass")

jar = http.cookiejar.CookieJar()
op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
op.open(f"{URL}/api/v2/auth/login", urllib.parse.urlencode({"username": USER, "password": PASS}).encode())
for t in json.load(op.open(f"{URL}/api/v2/torrents/info")):
    print(t["name"][:40], t["state"], t["category"], round(t["progress"], 2), "ratio_limit", t["ratio_limit"],
          "time_limit", t["seeding_time_limit"], "seeding_time", t["seeding_time"], t["content_path"])
