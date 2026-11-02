#!/usr/bin/env python3
"""patorrent-nox against a current qBittorrent (libtorrent 2.0.15 / 2.1.x) over both WebAPIs.

Release checks for the libtorrent 2.0.12-2.1.2 ports (docs/BACKLOG.md "libtorrent 2.0.12-2.1.2
follow-ups"): v2-only and hybrid torrents with odd file sizes in both directions, MSE with RC4
forced in both directions, and the slow-torrent queue (`dont_count_slow_torrents`).

qBittorrent runs in a linuxserver.io container on the host network; patorrent-nox runs here.
Everything listens on or connects to 127.0.0.1 only; DHT, LSD, PeX and port mapping are off on
both sides and no torrent has a tracker. See README.md beside this file.

    python3 run.py --nox PATH --image IMAGE [--work DIR] [--keep] CHECK...

CHECK: v2, hybrid, mse, queue (or all).
"""

import argparse
import filecmp
import hashlib
import http.cookiejar
import json
import os
import random
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

QBT_WEB, QBT_BT = 18180, 16981
NOX_WEB, NOX_BT = 18190, 16991
NOX_PASS = "interop-pass"

# Odd sizes: smaller than a block, one byte, around block and piece boundaries (16 KiB, 4 MiB,
# 16 MiB), and a few primes. About 41 MB in all.
ODD_FILES = [
    ("tiny.txt", 1000),
    ("one.bin", 1),
    ("block-1.bin", 16383),
    ("block+1.bin", 16385),
    ("three-blocks+1.bin", 3 * 16384 + 1),
    ("a/piece4m-1.bin", 4 * 2**20 - 1),
    ("a/piece4m+16k+1.bin", 4 * 2**20 + 16385),
    ("a/b/prime.bin", 9_999_991),
    ("a/b/twenty.bin", 20_000_003),
    ("z-last.bin", 32769),
]


def log(*a):
    print(time.strftime("%H:%M:%S"), *a, flush=True)


# --- bencode (just enough to find the info dictionary) ---------------------------------------


def info_span(b):
    """Byte range of the top-level `info` value in a .torrent."""

    def skip(i):
        c = b[i : i + 1]
        if c == b"i":
            return b.index(b"e", i) + 1
        if c in (b"l", b"d"):
            i += 1
            while b[i : i + 1] != b"e":
                i = skip(i)
            return i + 1
        colon = b.index(b":", i)
        return colon + 1 + int(b[i:colon])

    assert b[:1] == b"d"
    i = 1
    while b[i : i + 1] != b"e":
        kend = skip(i)
        colon = b.index(b":", i)
        key = b[colon + 1 : kend]
        vend = skip(kend)
        if key == b"info":
            return kend, vend
        i = vend
    raise ValueError("no info")


def torrent_ids(data):
    s, e = info_span(data)
    info = data[s:e]
    return hashlib.sha1(info).hexdigest(), hashlib.sha256(info).hexdigest()


# --- HTTP --------------------------------------------------------------------------------------


class Api:
    def __init__(self, base, user=None, password=None):
        self.base = base.rstrip("/")
        self.op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
        self.user, self.password = user, password

    def req(self, path, data=None, files=None, raw=False, ctype=None, timeout=30):
        url = f"{self.base}/{path}"
        # nox's native endpoints (/pa/) want this header
        headers = {"X-Requested-With": "PaTorrent"} if path.startswith("pa/") else {}
        body = None
        if files is not None:
            boundary = "----interop%016x" % random.getrandbits(64)
            parts = []
            for k, v in (data or {}).items():
                parts.append(
                    f'--{boundary}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode()
                )
            for k, (fname, content) in files.items():
                parts.append(
                    f'--{boundary}\r\nContent-Disposition: form-data; name="{k}"; filename="{fname}"\r\n'
                    f"Content-Type: application/x-bittorrent\r\n\r\n".encode()
                    + content
                    + b"\r\n"
                )
            parts.append(f"--{boundary}--\r\n".encode())
            body = b"".join(parts)
            headers["Content-Type"] = f"multipart/form-data; boundary={boundary}"
        elif ctype == "json":
            body = json.dumps(data).encode()
            headers["Content-Type"] = "application/json"
        elif data is not None:
            body = urllib.parse.urlencode(data).encode()
            headers["Content-Type"] = "application/x-www-form-urlencoded"
        r = self.op.open(urllib.request.Request(url, body, headers), timeout=timeout)
        out = r.read()
        if raw:
            return out
        try:
            return json.loads(out)
        except ValueError:
            return out.decode(errors="replace")

    def login(self):
        if self.user:
            r = self.req("api/v2/auth/login", {"username": self.user, "password": self.password})
            if r != "Ok.":
                raise RuntimeError(f"login refused: {r}")

    def info(self):
        return self.req("api/v2/torrents/info")

    def by_name(self, name):
        for t in self.info():
            if t["name"] == name:
                return t
        return None

    def add(self, torrent, savepath, extra=None):
        data = {"savepath": savepath, "skip_checking": "false", "paused": "false", "stopped": "false"}
        data.update(extra or {})
        r = self.req("api/v2/torrents/add", data, files={"torrents": ("t.torrent", torrent)})
        if isinstance(r, str) and not r.startswith("Ok"):
            raise RuntimeError(f"add refused: {r}")

    def delete_all(self, files):
        self.req("api/v2/torrents/delete", {"hashes": "all", "deleteFiles": "true" if files else "false"})

    def props(self, h):
        return self.req(f"api/v2/torrents/properties?hash={h}")

    def peers(self, h):
        r = self.req(f"api/v2/sync/torrentPeers?hash={h}&rid=0")
        return list((r.get("peers") or {}).values()) if isinstance(r, dict) else []

    def add_peer(self, h, port):
        self.req("api/v2/torrents/addPeers", {"hashes": h, "peers": f"127.0.0.1:{port}"})


def wait_for(what, pred, limit, step=0.5):
    deadline = time.time() + limit
    while time.time() < deadline:
        v = pred()
        if v:
            return v
        time.sleep(step)
    raise TimeoutError(f"{what}: not within {limit} s")


# --- clients -----------------------------------------------------------------------------------


class Qbt:
    def __init__(self, image, work, encryption=0):
        self.image = image
        self.name = "pa-interop-qbt"
        self.cfg = os.path.join(work, "qbt-config")
        self.work = work
        self.encryption = encryption
        self.api = Api(f"http://127.0.0.1:{QBT_WEB}")

    def start(self):
        subprocess.run(["docker", "rm", "-f", self.name], capture_output=True)
        shutil.rmtree(self.cfg, ignore_errors=True)
        conf_dir = os.path.join(self.cfg, "qBittorrent")
        os.makedirs(conf_dir)
        # 0 prefer, 1 require (qBittorrent sets RC4 only and forced both ways), 2 disable
        conf = f"""[LegalNotice]
Accepted=true

[BitTorrent]
Session\\Port={QBT_BT}
Session\\DHTEnabled=false
Session\\LSDEnabled=false
Session\\PeXEnabled=false
Session\\Encryption={self.encryption}
Session\\QueueingSystemEnabled=false
Session\\AddTorrentStopped=false
Session\\GlobalMaxRatio=-1
Session\\DefaultSavePath={self.work}/qbt-dl
Session\\TempPathEnabled=false

[Network]
PortForwardingEnabled=false

[Preferences]
WebUI\\Port={QBT_WEB}
WebUI\\LocalHostAuth=false
WebUI\\HostHeaderValidation=false
WebUI\\CSRFProtection=false
Connection\\ResolvePeerCountries=false
General\\Locale=en
"""
        with open(os.path.join(conf_dir, "qBittorrent.conf"), "w") as f:
            f.write(conf)
        subprocess.run(
            [
                "docker", "run", "-d", "--name", self.name, "--network", "host",
                "-e", f"PUID={os.getuid()}", "-e", f"PGID={os.getgid()}", "-e", "TZ=Etc/UTC",
                "-e", f"WEBUI_PORT={QBT_WEB}", "-e", f"TORRENTING_PORT={QBT_BT}",
                "-v", f"{self.cfg}:/config", "-v", f"{self.work}:{self.work}",
                self.image,
            ],
            check=True,
            capture_output=True,
        )

        def up():
            try:
                return self.api.req("api/v2/app/version", timeout=3)
            except (urllib.error.URLError, OSError):
                return None

        version = wait_for("qBittorrent Web API", up, 90)
        build = self.api.req("api/v2/app/buildInfo")
        prefs = self.api.req("api/v2/app/preferences")
        log(f"qBittorrent {version}, libtorrent {build.get('libtorrent')}, {build.get('platform')}"
            f" (dht {prefs.get('dht')}, lsd {prefs.get('lsd')}, pex {prefs.get('pex')}, upnp {prefs.get('upnp')},"
            f" encryption {prefs.get('encryption')}, listen {prefs.get('listen_port')})")
        if prefs.get("dht") or prefs.get("lsd") or prefs.get("pex") or prefs.get("listen_port") != QBT_BT:
            raise RuntimeError("qBittorrent did not take the test configuration")
        return version, build.get("libtorrent")

    def stop(self):
        subprocess.run(["docker", "logs", self.name], stdout=open(os.path.join(self.work, "qbt-docker.log"), "w"),
                       stderr=subprocess.STDOUT)
        subprocess.run(["docker", "rm", "-f", self.name], capture_output=True)

    def create_torrent(self, src, fmt, piece_size):
        """A torrent of `src` made by qBittorrent's (libtorrent's) creator, or None."""
        try:
            r = self.api.req("api/v2/torrentcreator/addTask", {
                "sourcePath": src, "format": fmt, "pieceSize": str(piece_size), "private": "false",
                "startSeeding": "false", "optimizeAlignment": "true",
            })
        except urllib.error.HTTPError as e:
            log(f"qBittorrent's torrent creator refused: {e}")
            return None
        task = r["taskID"]

        def done():
            st = self.api.req(f"api/v2/torrentcreator/status?taskID={task}")
            st = st[0] if isinstance(st, list) else st
            return st if st.get("status") in ("Finished", "Failed") else None

        st = wait_for("torrent creator", done, 300, 1)
        if st["status"] != "Finished":
            log(f"qBittorrent's torrent creator failed: {st}")
            return None
        data = self.api.req(f"api/v2/torrentcreator/torrentFile?taskID={task}", raw=True)
        self.api.req("api/v2/torrentcreator/deleteTask", {"taskID": task})
        return data


class Nox:
    def __init__(self, exe, work, tag):
        self.exe = exe
        self.dir = os.path.join(work, f"nox-{tag}")
        self.log = os.path.join(work, f"nox-{tag}.log")
        self.api = Api(f"http://127.0.0.1:{NOX_WEB}", "admin", NOX_PASS)
        self.proc = None

    def start(self, prefs):
        shutil.rmtree(self.dir, ignore_errors=True)
        os.makedirs(os.path.join(self.dir, "dl"))
        self.proc = subprocess.Popen(
            [
                self.exe, "run", "--state-dir", os.path.join(self.dir, "state"),
                "--save", os.path.join(self.dir, "dl"), "--listen", f"127.0.0.1:{NOX_BT}",
                "--webui", f"127.0.0.1:{NOX_WEB}", "--webui-pass", NOX_PASS, "--no-update-check", "--verbose",
            ],
            stdout=open(self.log, "w"), stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL,
        )

        def up():
            try:
                self.api.login()
                return True
            except (urllib.error.URLError, OSError, RuntimeError):
                return None

        wait_for("nox Web UI", up, 30)
        base = {"enable_dht": False, "enable_lsd": False, "enable_upnp": False, "enable_natpmp": False}
        base.update(prefs)
        self.api.req("pa/prefs", base, ctype="json")
        model = self.api.req("pa/prefs")
        got = {k: model["fields"][k]["value"] for k in base}
        log(f"nox {self.api.req('api/v2/app/buildInfo').get('app_version', '')} prefs {got}")

    def stop(self):
        if self.proc:
            self.proc.terminate()
            try:
                self.proc.wait(20)
            except subprocess.TimeoutExpired:
                self.proc.kill()
            self.proc = None

    def log_lines(self, start=0):
        with open(self.log, errors="replace") as f:
            return f.read().splitlines()[start:]


# --- data --------------------------------------------------------------------------------------


def make_files(root, files, seed):
    rnd = random.Random(seed)
    for rel, size in files:
        p = os.path.join(root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        if os.path.exists(p) and os.path.getsize(p) == size:
            continue
        with open(p, "wb") as f:
            left = size
            while left:
                n = min(left, 1 << 20)
                f.write(rnd.randbytes(n))
                left -= n


def nox_create(nox_exe, src, out, kind, piece_size):
    flag = {"v1": "--v1", "v2": "--v2", "hybrid": "--hybrid"}[kind]
    subprocess.run(
        [nox_exe, "create", flag, "--piece-size", str(piece_size // 1024), "--no-date", "-o", out, src],
        check=True, capture_output=True,
    )
    with open(out, "rb") as f:
        return f.read()


def same_tree(a, b, files):
    bad = []
    for rel, _ in files:
        pa, pb = os.path.join(a, rel), os.path.join(b, rel)
        if not os.path.exists(pb) or not filecmp.cmp(pa, pb, shallow=False):
            bad.append(rel)
    return bad


DONE_STATES = {"uploading", "stalledUP", "queuedUP", "pausedUP", "stoppedUP", "forcedUP"}


def complete(api, name):
    t = api.by_name(name)
    return t if t and t["progress"] >= 1 and t["state"] in DONE_STATES else None


def transfer(direction, torrent, name, src_parent, files, qbt, nox, limit=600):
    """direction `down`: qBittorrent seeds, nox downloads and connects; `up`: nox seeds,
    qBittorrent downloads and connects. Returns a result dict."""
    qbt.api.delete_all(files=False)
    nox.api.delete_all(files=False)
    time.sleep(1)
    leech_dir = os.path.join(qbt.work, f"leech-{direction}")
    shutil.rmtree(leech_dir, ignore_errors=True)
    os.makedirs(leech_dir)
    seed_api, leech_api = (qbt.api, nox.api) if direction == "down" else (nox.api, qbt.api)
    seed_port = QBT_BT if direction == "down" else NOX_BT
    log_start = len(nox.log_lines())
    seed_api.add(torrent, src_parent)
    leech_api.add(torrent, leech_dir)
    seed = wait_for("seed checked", lambda: complete(seed_api, name), 120)
    leech = wait_for("leech added", lambda: leech_api.by_name(name), 30)
    # qBittorrent keeps a just-checked torrent paused in libtorrent for a few seconds while its
    # API already says stalledUP; with no other torrent active, libtorrent drops every incoming
    # connection meanwhile ("no active torrents"), a uTP SYN with an immediate FIN that we, like
    # libtorrent, wait out for 15 s before trying TCP. Measured 2026-11-02: 5 s is enough.
    if direction == "down":
        time.sleep(5)
    h_seed, h_leech = seed["hash"], leech["hash"]
    t0 = time.time()
    leech_api.add_peer(h_leech, seed_port)
    flags = {"leech": set(), "seed": set()}
    last_peer = time.time()
    while True:
        if complete(leech_api, name):
            break
        if time.time() - t0 > limit:
            break
        for side, api, h in (("leech", leech_api, h_leech), ("seed", seed_api, h_seed)):
            try:
                for p in api.peers(h):
                    flags[side].add(p.get("flags", ""))
            except (urllib.error.URLError, OSError):
                pass
        if time.time() - last_peer > 5 and not leech_api.peers(h_leech):
            leech_api.add_peer(h_leech, seed_port)
            last_peer = time.time()
        time.sleep(0.5)
    elapsed = time.time() - t0
    ok = complete(leech_api, name) is not None
    time.sleep(2)
    bad = same_tree(os.path.join(src_parent, name), os.path.join(leech_dir, name), files) if ok else ["(incomplete)"]
    ps, pl = seed_api.props(h_seed), leech_api.props(h_leech)
    # the leech rechecks what it got: everything must pass again
    leech_api.req("api/v2/torrents/recheck", {"hashes": h_leech})
    time.sleep(2)
    rechecked = None
    try:
        rechecked = wait_for("recheck", lambda: complete(leech_api, name), 120)
    except TimeoutError:
        pass
    lines = nox.log_lines(log_start)
    closed = [ln for ln in lines if "127.0.0.1" in ln and ("closed" in ln.lower() or "disconnect" in ln.lower())]
    errors = [ln for ln in lines if ln.startswith("!")]
    return {
        "direction": direction,
        "ok": ok and not bad and rechecked is not None,
        "elapsed_s": round(elapsed, 1),
        "mismatch": bad,
        "recheck_progress": rechecked["progress"] if rechecked else leech_api.by_name(name)["progress"],
        "seed_wasted": ps.get("total_wasted"),
        "leech_wasted": pl.get("total_wasted"),
        "leech_downloaded": pl.get("total_downloaded_session", pl.get("total_downloaded")),
        "seed_uploaded": ps.get("total_uploaded_session", ps.get("total_uploaded")),
        "total_size": pl.get("total_size"),
        "piece_size": pl.get("piece_size"),
        "pieces": pl.get("pieces_num"),
        "leech_peer_flags": sorted(flags["leech"]),
        "seed_peer_flags": sorted(flags["seed"]),
        "nox_disconnects": closed[:20],
        "nox_errors": errors[:20],
    }


# --- checks ------------------------------------------------------------------------------------


def check_versions(kind, args, qbt, nox, piece_sizes, results, creators=("patorrent", "qbittorrent")):
    src_parent = os.path.join(args.work, "src")
    name = "oddsizes"
    make_files(os.path.join(src_parent, name), ODD_FILES, 52)
    for ps in piece_sizes:
        for creator in creators:
            out = os.path.join(args.work, f"{name}-{kind}-{ps // 1024}k-{creator}.torrent")
            if creator == "patorrent":
                t = nox_create(args.nox, os.path.join(src_parent, name), out, kind, ps)
            else:
                t = qbt.create_torrent(os.path.join(src_parent, name), kind, ps)
                if t is None:
                    continue
                with open(out, "wb") as f:
                    f.write(t)
            v1, v2 = torrent_ids(t)
            for direction in ("down", "up"):
                label = f"{kind} piece {ps // 1024} KiB, made by {creator}, {direction}"
                log(f"--- {label} (v1 {v1[:12]} v2 {v2[:12]})")
                try:
                    r = transfer(direction, t, name, src_parent, ODD_FILES, qbt, nox)
                except Exception as e:  # noqa: BLE001 - recorded as a failed run
                    r = {"direction": direction, "ok": False, "error": repr(e)}
                r["label"] = label
                log(json.dumps(r))
                results.append(r)


def check_mse(args, qbt, nox, results):
    src_parent = os.path.join(args.work, "src")
    name = "oddsizes"
    make_files(os.path.join(src_parent, name), ODD_FILES, 52)
    for kind in ("v1", "hybrid"):
        t = nox_create(args.nox, os.path.join(src_parent, name), os.path.join(args.work, f"mse-{kind}.torrent"),
                       kind, 256 * 1024)
        for direction in ("down", "up"):
            label = f"mse rc4 forced, {kind}, {direction}"
            log(f"--- {label}")
            try:
                r = transfer(direction, t, name, src_parent, ODD_FILES, qbt, nox)
                # rows that moved data (interested / interesting); a uTP connect still waiting
                # for its SYN-ACK has no flags yet
                moved = [f for f in r["leech_peer_flags"] + r["seed_peer_flags"] if set(f) & set("DdUu")]
                r["all_rc4"] = bool(moved) and all("E" in f for f in moved)
                r["ok"] = r["ok"] and r["all_rc4"]
            except Exception as e:  # noqa: BLE001
                r = {"direction": direction, "ok": False, "error": repr(e)}
            r["label"] = label
            log(json.dumps(r))
            results.append(r)


def check_queue(args, qbt, nox, results):
    """N = 2 active downloads. Q1 slow (`SLOW` from qBittorrent), Q2 stalled (no source), Q3-Q5
    fast (300 KiB/s). Expect Q1/Q2 to hold both slots for `auto_manage_startup`, then Q3/Q4 to
    start while Q1/Q2 keep running, Q5 to stay queued behind the fast ones; Q3 throttled to
    `SLOW` lets Q5 start after another `auto_manage_startup`; Q1 fast again counts at once and
    pushes the last one back. Seeds: active_seeds 1, S1 and S2 idle: S2 starts after the startup.

    The payload rate is a 5 s average and arrives in 16 KiB blocks, so one block alone reads
    3276 B/s: a source slower than that still crosses the default `inactive_down_rate` (2048)
    with every block and counts again at once. The check raises the threshold to
    `--inactive-down-rate` (20 KiB/s) and throttles the slow source to 5 KiB/s.
    """
    startup = args.startup
    slow = 5 * 1024
    src_parent = os.path.join(args.work, "qsrc")
    size = 64 * 2**20
    torrents = {}
    for n in ("Q1", "Q2", "Q3", "Q4", "Q5", "S1", "S2"):
        make_files(os.path.join(src_parent, n), [("data.bin", size if n[0] == "Q" else 3 * 2**20)], sum(n.encode()))
        torrents[n] = nox_create(args.nox, os.path.join(src_parent, n), os.path.join(args.work, f"{n}.torrent"),
                                 "v1", 256 * 1024)
    qbt.api.delete_all(files=False)
    nox.api.delete_all(files=False)
    for n in ("Q1", "Q3", "Q4", "Q5"):
        qbt.api.add(torrents[n], src_parent)
    qh = {n: wait_for(f"qbt seeds {n}", lambda n=n: complete(qbt.api, n), 120)["hash"] for n in ("Q1", "Q3", "Q4", "Q5")}

    def limit(n, bps):
        qbt.api.req("api/v2/torrents/setUploadLimit", {"hashes": qh[n], "limit": str(bps)})

    limit("Q1", slow)
    for n in ("Q3", "Q4", "Q5"):
        limit(n, 300 * 1024)
    leech = os.path.join(args.work, "qleech")
    shutil.rmtree(leech, ignore_errors=True)
    os.makedirs(leech)
    # the seeds first: unchecked, they are downloads (and queued as such) until their check
    for n in ("S1", "S2"):
        nox.api.add(torrents[n], src_parent)
    wait_for("nox checked S1 and S2", lambda: all(
        t["progress"] >= 1 and t["state"] not in ("checkingUP", "checkingResumeData") for t in nox.api.info()), 60)
    t0 = time.time()
    for n in ("Q1", "Q2", "Q3", "Q4", "Q5"):
        nox.api.add(torrents[n], leech)
    wait_for("nox added all", lambda: len(nox.api.info()) == 7, 30)
    timeline = []
    rates = []  # (t, {name: download rate as the shim reports it})
    last = {}

    def poll():
        now = time.time() - t0
        cur = {}
        for t in nox.api.info():
            cur[t["name"]] = (t["state"], t["dlspeed"], t["upspeed"], t.get("priority"))
            if t["name"] in qh and t["state"] in ("downloading", "stalledDL", "metaDL") and t["num_seeds"] == 0:
                try:
                    nox.api.add_peer(t["hash"], QBT_BT)
                except (urllib.error.URLError, OSError):
                    pass
        for n, v in sorted(cur.items()):
            if last.get(n, (None,))[0] != v[0]:
                timeline.append((round(now, 1), n, v[0]))
                log(f"  t={now:6.1f}s {n}: {last.get(n, ('-',))[0]} -> {v[0]} (down {v[1]} B/s, up {v[2]} B/s)")
        last.update(cur)
        rates.append((round(now, 1), {n: v[1] for n, v in sorted(cur.items()) if n[0] == "Q"}))
        return now, cur

    def running(cur, n):
        # a queued torrent that was never started stays in checkingResumeData here
        return cur[n][0] not in ("queuedDL", "queuedUP", "pausedDL", "pausedUP", "stoppedDL", "stoppedUP",
                                 "checkingResumeData")

    def until(cond, secs):
        end = time.time() + secs
        while time.time() < end:
            now, cur = poll()
            if cond(cur):
                return now
            time.sleep(1)
        return None

    def seeds(c):
        return sum(running(c, n) for n in ("S1", "S2"))

    findings = {}
    _, cur = poll()
    findings["seeds_running_at_start"] = seeds(cur)
    # phase 1: within the startup Q1/Q2 count; Q3 must wait (and one idle seed holds the slot)
    early = until(lambda c: running(c, "Q3") or running(c, "Q4") or seeds(c) > 1, startup - 5)
    findings["q3_or_second_seed_started_within_startup"] = early
    # phase 2: Q1/Q2 slow for `startup` s: Q3, Q4 start, Q1/Q2 still run, Q5 queued
    started = until(lambda c: running(c, "Q3") and running(c, "Q4"), 40)
    findings["q3_q4_started_at"] = started
    s2 = until(lambda c: seeds(c) == 2, 20)
    findings["both_seeds_running_at"] = s2
    _, cur = poll()
    findings["q1_q2_still_running"] = running(cur, "Q1") and running(cur, "Q2")
    held = until(lambda c: running(c, "Q5"), 20)
    findings["q5_started_while_q3_q4_fast"] = held
    # phase 3: Q3 slow -> after `startup` Q5 starts
    limit("Q3", slow)
    tq3 = time.time() - t0
    findings["q3_throttled_at"] = round(tq3, 1)
    early5 = until(lambda c: running(c, "Q5"), startup - 5)
    findings["q5_started_within_startup_of_q3_slow"] = early5
    q5 = until(lambda c: running(c, "Q5"), 45)
    findings["q5_started_at"] = q5
    # phase 4: Q1 fast again: counts at once, the last running download (Q5) is queued again
    limit("Q1", 300 * 1024)
    tq1 = time.time() - t0
    findings["q1_unthrottled_at"] = round(tq1, 1)
    back = until(lambda c: not running(c, "Q5"), 30) if q5 is not None else None
    findings["q5_queued_again_at"] = back
    _, cur = poll()
    findings["final_states"] = {n: v[0] for n, v in sorted(cur.items())}
    ok = (
        early is None
        and findings["seeds_running_at_start"] == 1
        and started is not None
        and started >= startup - 2
        and findings["q1_q2_still_running"]
        and s2 is not None
        and held is None
        and early5 is None
        and q5 is not None
        and back is not None
    )
    r = {"label": f"queue: active_downloads 2, active_seeds 1, auto_manage_startup {startup},"
                  f" inactive_down_rate {args.inactive_down_rate}", "ok": ok,
         "findings": findings, "timeline": timeline}
    log(json.dumps(r))
    r["rates"] = rates
    results.append(r)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("checks", nargs="+", choices=["v2", "hybrid", "mse", "queue", "all"])
    ap.add_argument("--nox", required=True)
    ap.add_argument("--image", required=True, help="qBittorrent image, e.g. lscr.io/linuxserver/qbittorrent:5.3.0_v2.1.2-ls486")
    ap.add_argument("--work", default=os.path.expanduser("~/qbt-interop/work"))
    ap.add_argument("--startup", type=int, default=30, help="auto_manage_startup for the queue check")
    ap.add_argument("--inactive-down-rate", type=int, default=20480, help="B/s, for the queue check")
    ap.add_argument("--piece-sizes", default="16,4096,16384", help="KiB")
    ap.add_argument("--creators", default="patorrent,qbittorrent")
    ap.add_argument("--out", help="write the results as JSON here")
    args = ap.parse_args()
    checks = ["v2", "hybrid", "mse", "queue"] if "all" in args.checks else args.checks
    args.work = os.path.abspath(args.work)
    os.makedirs(args.work, exist_ok=True)
    piece_sizes = [int(k) * 1024 for k in args.piece_sizes.split(",")]
    creators = tuple(args.creators.split(","))
    results = []
    meta = {}
    for check in checks:
        enc = 1 if check == "mse" else 0
        qbt = Qbt(args.image, args.work, enc)
        nox = Nox(args.nox, args.work, check)
        try:
            meta["qbittorrent"], meta["libtorrent"] = qbt.start()
            prefs = {}
            if check == "mse":
                # libtorrent numbers: policy 0 forced; level 2 RC4
                prefs = {"out_enc_policy": 0, "in_enc_policy": 0, "allowed_enc_level": 2}
            if check == "queue":
                prefs = {"active_downloads": 2, "active_seeds": 1, "dont_count_slow_torrents": True,
                         "inactive_down_rate": args.inactive_down_rate, "inactive_up_rate": 2048,
                         "auto_manage_startup": args.startup}
            nox.start(prefs)
            if check in ("v2", "hybrid"):
                check_versions(check, args, qbt, nox, piece_sizes, results, creators)
            elif check == "mse":
                check_mse(args, qbt, nox, results)
            else:
                check_queue(args, qbt, nox, results)
        except Exception as e:  # noqa: BLE001
            log(f"{check}: {e!r}")
            results.append({"label": check, "ok": False, "error": repr(e)})
        finally:
            nox.stop()
            try:
                qbt.api.delete_all(files=False)
            except Exception:  # noqa: BLE001
                pass
            qbt.stop()
    print()
    print(f"qBittorrent {meta.get('qbittorrent')} / libtorrent {meta.get('libtorrent')}")
    for r in results:
        extra = ""
        if "elapsed_s" in r:
            extra = (f" {r['elapsed_s']} s, wasted seed/leech {r.get('seed_wasted')}/{r.get('leech_wasted')},"
                     f" disconnects {len(r.get('nox_disconnects', []))}, flags {r.get('leech_peer_flags')}")
        print(f"  {'PASS' if r['ok'] else 'FAIL'}  {r['label']}{extra}{' ' + r['error'] if 'error' in r else ''}")
    if args.out:
        with open(args.out, "w") as f:
            json.dump({"meta": meta, "results": results}, f, indent=1)
    sys.exit(0 if all(r["ok"] for r in results) else 1)


if __name__ == "__main__":
    main()
