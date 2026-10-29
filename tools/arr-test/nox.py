#!/usr/bin/env python3
"""Prints nox's torrents as the shim reports them."""

import http.cookiejar
import json
import urllib.parse
import urllib.request

jar = http.cookiejar.CookieJar()
op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
op.open("http://127.0.0.1:8080/api/v2/auth/login",
        urllib.parse.urlencode({"username": "admin", "password": "arr-test-pass"}).encode())
for t in json.load(op.open("http://127.0.0.1:8080/api/v2/torrents/info")):
    print(t["name"][:40], t["state"], t["category"], round(t["progress"], 2), "ratio_limit", t["ratio_limit"],
          "time_limit", t["seeding_time_limit"], "seeding_time", t["seeding_time"], t["content_path"])
