"""Writes a single-file v1 .torrent for PATH (1 MiB pieces, no trackers): mktorrent.py PATH OUT"""
import hashlib
import os
import sys


def ben(x):
    if isinstance(x, int):
        return b"i%de" % x
    if isinstance(x, str):
        x = x.encode()
    if isinstance(x, bytes):
        return b"%d:%s" % (len(x), x)
    if isinstance(x, dict):
        return b"d" + b"".join(ben(k) + ben(v) for k, v in sorted(x.items())) + b"e"
    raise TypeError(x)


path, out = sys.argv[1], sys.argv[2]
plen = 1 << 20
pieces = b""
with open(path, "rb") as f:
    while True:
        chunk = f.read(plen)
        if not chunk:
            break
        pieces += hashlib.sha1(chunk).digest()
info = {"name": os.path.basename(path), "length": os.path.getsize(path), "piece length": plen, "pieces": pieces}
with open(out, "wb") as f:
    f.write(ben({"info": info, "created by": "closing-test"}))
