#!/usr/bin/env python3
"""Adds fakeznab to Prowlarr as a Generic Torznab indexer, searches it and grabs one release."""

import sys
import urllib.parse

sys.argv = ["arr.py"]
import arr  # noqa: E402

_, existing = arr.call("prowlarr", "GET", "indexer")
for e in existing:
    arr.call("prowlarr", "DELETE", f"indexer/{e['id']}")
_, schemas = arr.call("prowlarr", "GET", "indexer/schema")
s = next(x for x in schemas if x["implementation"] == "Torznab")
s = arr.fill(s, {"baseUrl": arr.FAKEZNAB, "apiPath": "/api", "apiKey": "x"})
s.update(name="fakeznab", enable=True, appProfileId=1, priority=25)
st, idx = arr.call("prowlarr", "POST", "indexer", s)
print("indexer", st, "" if st < 400 else idx)
if st >= 400:
    sys.exit(1)

st, results = arr.call("prowlarr", "GET", "search?type=search&query=" + urllib.parse.quote("Prowlarr Grab Test"))
print("search", st, [r["title"] for r in results])
pick = next(r for r in results if "Prowlarr.Grab" in r["title"])
st, res = arr.call("prowlarr", "POST", "search", {"guid": pick["guid"], "indexerId": pick["indexerId"]})
print("grab", st, res if st >= 400 else "")
