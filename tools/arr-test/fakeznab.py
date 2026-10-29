#!/usr/bin/env python3
"""A tiny Torznab indexer for testing PaTorrent with Sonarr, Radarr and Prowlarr.

Serves synthetic releases: test-pattern videos made with ffmpeg (no real content), each as a
single-file v1 .torrent. The payload is also placed in the download folder, so the client
finds it complete after its check and seeds it.

    fakeznab.py --data /srv/media/downloads --store /srv/fakeznab --base http://172.17.0.1:9117
"""

import argparse
import hashlib
import html
import os
import subprocess
import sys
import time
import urllib.parse
from email.utils import formatdate
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# (id, title, Newznab category, kind, seconds of video)
RELEASES = [
    ("tv1", "Fawlty.Towers.S01E01.720p.WEB-DL.x264-PATEST", 5040, "tv", 1500),
    ("mv1", "Night.of.the.Living.Dead.1968.720p.WEB-DL.x264-PATEST", 2040, "movie", 1800),
    ("pc1", "PaTorrent.Prowlarr.Grab.Test.x264-PATEST", 5040, "other", 120),
]
PIECE = 256 * 1024


def bencode(v):
    if isinstance(v, int):
        return b"i%de" % v
    if isinstance(v, str):
        v = v.encode()
    if isinstance(v, bytes):
        return b"%d:%s" % (len(v), v)
    if isinstance(v, list):
        return b"l" + b"".join(bencode(x) for x in v) + b"e"
    if isinstance(v, dict):
        items = sorted((k.encode() if isinstance(k, str) else k, x) for k, x in v.items())
        return b"d" + b"".join(bencode(k) + bencode(x) for k, x in items) + b"e"
    raise TypeError(type(v))


def make_payload(path, seconds):
    if os.path.exists(path):
        return
    # about 640 kbit/s, so the files pass the *arr apps' minimum sizes (90 MB for 30 min of TV);
    # the noise keeps x264 from compressing the pattern below the bitrate
    subprocess.run(
        ["ffmpeg", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=640x360:rate=10",
         "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=22050",
         "-vf", "noise=alls=40:allf=t", "-t", str(seconds), "-c:v", "libx264", "-preset", "ultrafast",
         "-b:v", "640k", "-minrate", "640k", "-maxrate", "640k", "-bufsize", "1280k", "-pix_fmt", "yuv420p",
         "-c:a", "aac", "-b:a", "32k", "-shortest", path],
        check=True,
    )


def make_torrent(path, name):
    pieces = bytearray()
    with open(path, "rb") as f:
        while chunk := f.read(PIECE):
            pieces += hashlib.sha1(chunk).digest()
    info = {"name": name, "length": os.path.getsize(path), "piece length": PIECE, "pieces": bytes(pieces)}
    meta = {"info": info, "created by": "fakeznab", "creation date": int(time.time())}
    return bencode(meta), hashlib.sha1(bencode(info)).hexdigest()


class Index:
    def __init__(self, data, store, base):
        self.base = base.rstrip("/")
        self.items = []
        os.makedirs(data, exist_ok=True)
        os.makedirs(store, exist_ok=True)
        for rid, title, cat, kind, secs in RELEASES:
            name = title + ".mkv"
            src = os.path.join(store, name)
            make_payload(src, secs)
            dst = os.path.join(data, name)
            if not os.path.exists(dst):
                os.link(src, dst) if os.stat(store).st_dev == os.stat(data).st_dev else subprocess.run(
                    ["cp", src, dst], check=True)
            torrent, ih = make_torrent(src, name)
            self.items.append(dict(id=rid, title=title, cat=cat, kind=kind, torrent=torrent, infohash=ih,
                                   size=os.path.getsize(src)))
            print(f"{rid}: {title} {ih} {os.path.getsize(src)} bytes", flush=True)

    def caps(self):
        return (
            '<?xml version="1.0" encoding="UTF-8"?><caps><server title="fakeznab"/>'
            '<limits default="100" max="100"/><searching>'
            '<search available="yes" supportedParams="q"/>'
            '<tv-search available="yes" supportedParams="q,season,ep"/>'
            '<movie-search available="yes" supportedParams="q"/></searching><categories>'
            '<category id="2000" name="Movies"><subcat id="2040" name="Movies/HD"/></category>'
            '<category id="5000" name="TV"><subcat id="5040" name="TV/HD"/></category>'
            "</categories></caps>"
        )

    def results(self, t):
        kinds = {"tvsearch": {"tv", "other"}, "movie": {"movie"}}.get(t, {"tv", "movie", "other"})
        now = formatdate(time.time() - 3600, usegmt=True)
        out = ['<?xml version="1.0" encoding="UTF-8"?><rss version="2.0" '
               'xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel><title>fakeznab</title>']
        for it in self.items:
            if it["kind"] not in kinds:
                continue
            url = f"{self.base}/dl/{it['id']}.torrent"
            parent = it["cat"] // 1000 * 1000
            out.append(
                f"<item><title>{html.escape(it['title'])}</title><guid>{it['id']}</guid>"
                f"<pubDate>{now}</pubDate><size>{it['size']}</size><link>{url}</link>"
                f'<enclosure url="{url}" length="{it["size"]}" type="application/x-bittorrent"/>'
                f'<torznab:attr name="category" value="{parent}"/>'
                f'<torznab:attr name="category" value="{it["cat"]}"/>'
                f'<torznab:attr name="seeders" value="25"/><torznab:attr name="peers" value="30"/>'
                f'<torznab:attr name="infohash" value="{it["infohash"]}"/>'
                f'<torznab:attr name="downloadvolumefactor" value="1"/>'
                f'<torznab:attr name="uploadvolumefactor" value="1"/></item>'
            )
        out.append("</channel></rss>")
        return "".join(out)


def handler(index):
    class H(BaseHTTPRequestHandler):
        def send(self, code, ctype, body):
            body = body.encode() if isinstance(body, str) else body
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            u = urllib.parse.urlparse(self.path)
            q = dict(urllib.parse.parse_qsl(u.query))
            if u.path.rstrip("/") == "/api":
                t = q.get("t", "")
                if t == "caps":
                    return self.send(200, "application/xml", index.caps())
                if t in ("search", "tvsearch", "movie"):
                    return self.send(200, "application/rss+xml", index.results(t))
                return self.send(200, "application/xml", '<error code="202" description="No such function"/>')
            if u.path.startswith("/dl/"):
                rid = u.path[4:].removesuffix(".torrent")
                for it in index.items:
                    if it["id"] == rid:
                        return self.send(200, "application/x-bittorrent", it["torrent"])
            self.send(404, "text/plain", "not found")

        def log_message(self, fmt, *args):
            sys.stdout.write("%s %s\n" % (self.address_string(), fmt % args))
            sys.stdout.flush()

    return H


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True)
    ap.add_argument("--store", required=True)
    ap.add_argument("--base", required=True)
    ap.add_argument("--port", type=int, default=9117)
    a = ap.parse_args()
    index = Index(a.data, a.store, a.base)
    ThreadingHTTPServer(("0.0.0.0", a.port), handler(index)).serve_forever()


if __name__ == "__main__":
    main()
