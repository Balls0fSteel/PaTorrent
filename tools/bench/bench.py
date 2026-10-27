#!/usr/bin/env python3
"""bench.py: PaTorrent's daemon against other BitTorrent clients on one Linux machine.

Method and published results: docs/BENCHMARKS.md. Standard library only, except Deluge's RPC
(`pip install deluge-client`). Linux only: it reads /proc for CPU time and memory.

    bench.py many --count 10000 [--clients ...] [--out results.jsonl]
    bench.py transfer --mib 2048 [--runs 3] [--clients ...] [--out results.jsonl]

`many`: every client gets the same N generated torrents (10 MB-20 GB, about 1,500 pieces each,
no data on disk, one tracker that refuses connections), added started. Measured: time to add,
then after a restart the time until the client lists all N, its memory and its idle CPU.

`transfer`: one torrent of random data on a tmpfs, seeded by a reference qBittorrent (TCP only,
found through a tracker in this process); each client downloads it. Measured: wall time from
adding to complete, and the downloader's CPU time.

Every client runs from a throwaway profile with DHT, local peer discovery, PeX and port mapping
off, everything else at its defaults. Results are JSON lines.
"""

import argparse
import base64
import hashlib
import http.client
import http.cookiejar
import http.server
import json
import os
import random
import shutil
import signal
import socket
import struct
import subprocess
import sys
import threading
import time
import urllib.parse
import urllib.request
import uuid
import xmlrpc.client

TICK = os.sysconf("SC_CLK_TCK")
PAGE = os.sysconf("SC_PAGE_SIZE")
HOME = os.path.expanduser("~")
ALL = ["patorrent", "qbittorrent", "transmission", "deluge", "rtorrent"]

# ------------------------------------------------------------------------------------ bencode


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


def info_hash(torrent):
    """SHA-1 of the info dictionary of a torrent this module wrote (its keys sort last)."""
    start = torrent.index(b"4:infod") + 6
    return hashlib.sha1(torrent[start:-1]).hexdigest()


# ------------------------------------------------------------------------------------ corpus

WORDS = ["Open", "Public", "Libre", "Archive", "Field", "Studio", "Night", "Analog", "Recordings",
         "Dataset", "Footage", "Lectures", "Sampler", "Atlas", "Sessions", "Scans"]


def synthetic_torrent(rng, i, tracker):
    """The shape of tools/swarm/src/corpus.rs: log-uniform 10 MB-20 GB, ~1,500 pieces."""
    import math
    total = int(math.exp(math.log(10e6) + (math.log(20e9) - math.log(10e6)) * rng.random()))
    piece = 1 << max(14, min(24, (total // 1500 - 1).bit_length()))
    pieces = rng.randbytes(-(-total // piece) * 20)
    name = f"{rng.choice(WORDS)} {rng.choice(WORDS)} {1990 + rng.randrange(36)} #{i:06}"
    info = {"name": name, "piece length": piece, "pieces": pieces}
    if rng.random() < 0.4:
        info["length"] = total
    else:
        n = 2 + rng.randrange(40)
        weights = [1 + rng.randrange(100) for _ in range(n)]
        left, files = total, []
        for k, w in enumerate(weights):
            size = left if k + 1 == n else total * w // sum(weights)
            left -= size
            path = [f"Disc {1 + k // 10}"] if rng.random() < 0.3 else []
            files.append({"length": size, "path": path + [f"{k + 1:02} {rng.choice(WORDS)}.bin"]})
        info["files"] = files
    return bencode({"announce": tracker, "created by": "bench.py",
                    "creation date": 1_500_000_000 + rng.randrange(250_000_000), "info": info})


def corpus(count, tracker, seed=1):
    rng = random.Random(seed)
    return [synthetic_torrent(rng, i, tracker) for i in range(count)]


def make_payload(path, mib, piece=1 << 20):
    """Random data of `mib` MiB in one file and its single-file v1 torrent (no announce yet)."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    hashes = []
    with open(path, "wb") as f:
        rnd = random.Random(42)
        for _ in range(mib * (1 << 20) // piece):
            block = rnd.randbytes(piece)
            hashes.append(hashlib.sha1(block).digest())
            f.write(block)
    return {"name": os.path.basename(path), "piece length": piece, "length": mib << 20,
            "pieces": b"".join(hashes)}


# ------------------------------------------------------------------------------------ tracker


def local_ip():
    """This machine's own non-loopback address: Transmission does not connect to 127.0.0.1."""
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
        s.connect(("192.0.2.1", 9))
        return s.getsockname()[0]


class Tracker:
    """A minimal HTTP tracker: every peer of a torrent gets the others, compact, all at this
    machine's address, so the traffic stays inside the kernel."""

    def __init__(self, port):
        self.swarms = swarms = {}  # info hash -> the ports peers announced (they listen on)
        lock = threading.Lock()
        ip = socket.inet_aton(local_ip())

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                q = urllib.parse.urlsplit(self.path).query
                args = urllib.parse.parse_qs(q, encoding="latin-1")
                ih = args.get("info_hash", [""])[0].encode("latin-1")
                port = int(args.get("port", ["0"])[0])
                with lock:
                    peers = swarms.setdefault(ih, set())
                    if args.get("event", [""])[0] == "stopped":
                        peers.discard(port)
                    else:
                        peers.add(port)
                    others = [p for p in peers if p != port]
                body = bencode({"interval": 30, "peers": b"".join(
                    ip + struct.pack(">H", p) for p in others)})
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *a):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
        self.url = f"http://127.0.0.1:{port}/announce"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()


# ------------------------------------------------------------------------------------ /proc


def tree(pid):
    """`pid` and its descendants, without the `sudo runuser` that starts a client as another user."""
    children = {}
    for d in os.listdir("/proc"):
        if d.isdigit():
            try:
                with open(f"/proc/{d}/stat") as f:
                    ppid = int(f.read().rsplit(")", 1)[1].split()[1])
                children.setdefault(ppid, []).append(int(d))
            except OSError:
                pass
    out, todo = [], [pid]
    while todo:
        p = todo.pop()
        try:
            with open(f"/proc/{p}/comm") as f:
                if f.read().strip() not in ("sudo", "runuser"):
                    out.append(p)
        except OSError:
            pass
        todo.extend(children.get(p, []))
    return out


def smaps_rollup(p):
    try:
        with open(f"/proc/{p}/smaps_rollup") as f:
            return f.read()
    except PermissionError:  # another user's process (Transmission)
        return subprocess.run(["sudo", "-n", "cat", f"/proc/{p}/smaps_rollup"],
                              capture_output=True, text=True).stdout


def cpu_seconds(pid):
    total = 0
    for p in tree(pid):
        try:
            with open(f"/proc/{p}/stat") as f:
                fields = f.read().rsplit(")", 1)[1].split()
            total += int(fields[11]) + int(fields[12])
        except OSError:
            pass
    return total / TICK


def threads(pid, top=6):
    """The busiest threads of the process tree since they started: [name, user s, system s]."""
    out = []
    for p in tree(pid):
        try:
            tasks = os.listdir(f"/proc/{p}/task")
        except OSError:
            continue
        for t in tasks:
            try:
                with open(f"/proc/{p}/task/{t}/stat") as f:
                    s = f.read()
                fields = s.rsplit(")", 1)[1].split()
                out.append([s[s.index("(") + 1:s.rindex(")")], int(fields[11]) / TICK, int(fields[12]) / TICK])
            except OSError:
                pass
    out.sort(key=lambda r: r[1] + r[2], reverse=True)
    return [[n, round(u, 2), round(k, 2)] for n, u, k in out[:top]]


def memory(pid):
    """(RSS, peak RSS, PSS) in bytes over the process tree."""
    rss = hwm = pss = 0
    for p in tree(pid):
        try:
            with open(f"/proc/{p}/status") as f:
                for line in f:
                    if line.startswith("VmRSS:"):
                        rss += int(line.split()[1]) * 1024
                    elif line.startswith("VmHWM:"):
                        hwm += int(line.split()[1]) * 1024
            for line in smaps_rollup(p).splitlines():
                if line.startswith("Pss:"):
                    pss += int(line.split()[1]) * 1024
        except OSError:
            pass
    return rss, hwm, pss


def sha1_file(path):
    h = hashlib.sha1()
    with open(path, "rb") as f:
        while chunk := f.read(1 << 22):
            h.update(chunk)
    return h.hexdigest()


def received(seed_port, own_port, seen):
    """Bytes the downloader listening on `own_port` received from the seed listening on
    `seed_port`, whoever connected, from the kernel's counters (`ss`). `seen` keeps each
    connection's count, so one that closed (two seeds part once the download is done) still counts."""
    out = subprocess.run(["ss", "-tinH", "state", "established"], capture_output=True, text=True).stdout
    conn = None
    for line in out.splitlines():
        if not line[:1].isspace():
            local, peer = line.split()[2:4]
            lp, pp = local.rsplit(":", 1)[1], peer.rsplit(":", 1)[1]
            take = (pp == str(seed_port) or lp == str(own_port)) and lp != str(seed_port)
            conn = (local, peer) if take else None
        elif conn and "bytes_received:" in line:
            seen[conn] = int(line.split("bytes_received:")[1].split()[0])
    return sum(seen.values())


def wait_for(what, check, timeout, every=0.1):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if check():
                return
        except Exception:
            pass
        time.sleep(every)
    raise TimeoutError(f"{what}: not after {timeout} s")


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


# ------------------------------------------------------------------------------------ clients


class Client:
    name = "?"

    def __init__(self, work, save):
        self.work = os.path.join(work, self.name)
        self.save = save
        self.port = free_port()
        self.api = free_port()
        self.proc = None
        shutil.rmtree(self.work, ignore_errors=True)
        os.makedirs(self.work)
        self.make_save()

    def make_save(self):
        os.makedirs(self.save, exist_ok=True)

    def clean(self):
        """Removes the downloaded data (the tmpfs is RAM)."""
        shutil.rmtree(self.save, ignore_errors=True)

    # lifecycle ------------------------------------------------------------------------
    def command(self):
        raise NotImplementedError

    def start(self):
        log = open(os.path.join(self.work, "stdout.log"), "ab")
        env = dict(os.environ, TERM="xterm", HOME=self.work if self.isolate_home else HOME)
        self.proc = subprocess.Popen(self.command(), stdout=log, stderr=subprocess.STDOUT,
                                     stdin=subprocess.DEVNULL, env=env, cwd=self.work)
        self.t_start = time.monotonic()
        wait_for(f"{self.name} API", lambda: self._alive() and self.ping(), 120, 0.05)

    isolate_home = False

    def _alive(self):
        if self.proc.poll() is not None:
            raise RuntimeError(f"{self.name} exited with {self.proc.returncode}; see {self.work}")
        return True

    def stop(self, timeout=300):
        if self.proc is None or self.proc.poll() is not None:
            return 0.0
        t0 = time.monotonic()
        self.proc.send_signal(signal.SIGTERM)
        try:
            self.proc.wait(timeout)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
            return float("inf")
        return time.monotonic() - t0

    def configure(self):
        """Settings that need the client running; it is restarted afterwards when True."""
        return False

    # measurements ----------------------------------------------------------------------
    @property
    def pid(self):
        return self.proc.pid

    def ping(self):
        raise NotImplementedError

    def add(self, torrents, seeding=False):
        raise NotImplementedError

    def count(self):
        raise NotImplementedError

    def has(self, n):
        return self.count() >= n

    def done(self, ih):
        """(verified bytes, wanted bytes) of torrent `ih`."""
        raise NotImplementedError

    def complete(self, ih):
        have, want = self.done(ih)
        return want > 0 and have == want

    def diagnose(self, ih):
        """What the client says about a torrent that does not finish."""
        return {}

    def version(self):
        raise NotImplementedError


class QbtApi(Client):
    """qBittorrent's WebAPI, which qBittorrent and PaTorrent's compatibility layer both speak."""
    user, password = "admin", "benchpass123"

    def _opener(self):
        if not hasattr(self, "_op"):
            self._op = urllib.request.build_opener(
                urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
            self.logged_in = False
        return self._op

    def call(self, path, data=None, files=None, timeout=120):
        op = self._opener()
        url = f"http://127.0.0.1:{self.api}/api/v2/{path}"
        headers = {"Referer": f"http://127.0.0.1:{self.api}"}
        body = None
        if files is not None:
            boundary = uuid.uuid4().hex
            parts = []
            for k, v in (data or {}).items():
                parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode())
            for i, blob in enumerate(files):
                parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="torrents"; '
                             f'filename="{i}.torrent"\r\nContent-Type: application/x-bittorrent\r\n\r\n'.encode()
                             + blob + b"\r\n")
            body = b"".join(parts) + f"--{boundary}--\r\n".encode()
            headers["Content-Type"] = f"multipart/form-data; boundary={boundary}"
        elif data is not None:
            body = urllib.parse.urlencode(data).encode()
        req = urllib.request.Request(url, body, headers)
        with op.open(req, timeout=timeout) as r:
            return r.read()

    def login(self):
        self._opener()
        r = self.call("auth/login", {"username": self.user, "password": self.password})
        if r.strip() != b"Ok.":
            raise RuntimeError(f"{self.name} login: {r!r}")

    def ping(self):
        if hasattr(self, "_op"):
            del self._op
        self.login()
        return bool(self.call("app/version"))

    def set_prefs(self, prefs):
        self.call("app/setPreferences", {"json": json.dumps(prefs)})

    def add(self, torrents, seeding=False):
        for i in range(0, len(torrents), 100):
            data = {"savepath": self.save}
            if seeding:
                data["skip_checking"] = "true"
            r = self.call("torrents/add", data, torrents[i:i + 100])
            if r.strip() != b"Ok.":
                raise RuntimeError(f"{self.name} add: {r!r}")

    def count(self):
        return len(json.loads(self.call("torrents/info")))

    def has(self, n):
        return len(json.loads(self.call(f"torrents/info?offset={n - 1}&limit=1"))) == 1

    def done(self, ih):
        info = json.loads(self.call(f"torrents/info?hashes={ih}"))
        return (info[0]["completed"], info[0]["size"]) if info else (0, 0)

    def seeding(self, ih):
        info = json.loads(self.call(f"torrents/info?hashes={ih}"))
        return bool(info) and info[0]["state"] in ("uploading", "stalledUP", "forcedUP", "queuedUP")

    def version(self):
        return self.call("app/version").decode().strip()


class Qbittorrent(QbtApi):
    name = "qbittorrent"

    def __init__(self, work, save, tcp_only=False, label=None):
        if label:
            self.name = label
        super().__init__(work, save)
        conf = os.path.join(self.work, "qBittorrent", "config")
        os.makedirs(conf)
        with open(os.path.join(conf, "qBittorrent.conf"), "w") as f:
            f.write(f"""[LegalNotice]
Accepted=true

[BitTorrent]
Session\\DHTEnabled=false
Session\\LSDEnabled=false
Session\\PeXEnabled=false
Session\\Port={self.port}
Session\\DefaultSavePath={save}
Session\\BTProtocol={"TCP" if tcp_only else "Both"}

[Preferences]
Connection\\UPnP=false
General\\Locale=en
WebUI\\Address=127.0.0.1
WebUI\\Port={self.api}
WebUI\\LocalHostAuth=false
WebUI\\CSRFProtection=false
WebUI\\HostHeaderValidation=false
""")

    def command(self):
        return ["qbittorrent-nox", f"--profile={self.work}", f"--webui-port={self.api}"]

    def login(self):
        self._opener()  # localhost needs no login (WebUI\LocalHostAuth=false)


class Patorrent(QbtApi):
    name = "patorrent"
    binary = os.path.join(HOME, "bench/pa/patorrent-1.0.1-linux-x86_64/patorrent-nox")

    def command(self):
        return [self.binary, "run", "--state-dir", os.path.join(self.work, "state"), "--save", self.save,
                "--webui", f"127.0.0.1:{self.api}", "--webui-pass", self.password,
                "--port", str(self.port)]

    def configure(self):
        self.set_prefs({"dht": False, "lsd": False, "upnp": False})
        # PeX and uTP have no qBittorrent key: write them where nox keeps its settings
        self.stop()
        path = os.path.join(self.work, "state", "webui.json")
        with open(path) as f:
            cfg = json.load(f)
        cfg["options"].update({"bt.pex": "0"})
        with open(path, "w") as f:
            json.dump(cfg, f)
        return True

    def version(self):
        return subprocess.run([self.binary, "--version"], capture_output=True, text=True).stdout.strip()


class Transmission(Client):
    """Ubuntu's AppArmor profile lets transmission-daemon touch only its service user's folders,
    so it runs as that user, as the packaged service does, with its files in those folders (the
    downloads folder is a tmpfs of its own). Other clients' `save` is ignored."""
    name = "transmission"
    user = "debian-transmission"
    root = "/var/lib/transmission-daemon"

    def __init__(self, work, save):
        self.conf = os.path.join(self.root, ".config/transmission-daemon/bench")
        super().__init__(work, os.path.join(self.root, "downloads/bench"))
        self.sid = ""
        self.as_user(["rm", "-rf", self.conf])
        self.as_user(["mkdir", "-p", self.conf])
        self.as_user(["tee", os.path.join(self.conf, "settings.json")], json.dumps({
                "dht-enabled": False, "lpd-enabled": False, "pex-enabled": False,
                "port-forwarding-enabled": False, "peer-port": self.port, "peer-port-random-on-start": False,
                "rpc-port": self.api, "rpc-bind-address": "127.0.0.1", "rpc-authentication-required": False,
                "rpc-whitelist-enabled": False, "rpc-host-whitelist-enabled": False,
                "download-dir": save, "incomplete-dir-enabled": False,
                "message-level": 2,
            }))

    def as_user(self, cmd, stdin=None):
        subprocess.run(self.run_as + cmd, input=stdin, text=True, stdout=subprocess.DEVNULL, check=True)

    @property
    def run_as(self):
        return ["sudo", "-n", "runuser", "-u", self.user, "--"]

    def make_save(self):
        self.as_user(["rm", "-rf", self.save])
        self.as_user(["mkdir", "-p", self.save])

    def clean(self):
        self.as_user(["rm", "-rf", self.save])

    def command(self):
        debug = ["--log-level=trace"] if os.environ.get("BENCH_DEBUG") else []
        return self.run_as + ["transmission-daemon", "-f", "-g", self.conf] + debug

    def rpc(self, method, args=None):
        body = json.dumps({"method": method, "arguments": args or {}}).encode()
        for _ in range(2):
            c = http.client.HTTPConnection("127.0.0.1", self.api, timeout=120)
            c.request("POST", "/transmission/rpc", body, {"X-Transmission-Session-Id": self.sid})
            r = c.getresponse()
            data = r.read()
            if r.status == 409:
                self.sid = r.getheader("X-Transmission-Session-Id")
                continue
            out = json.loads(data)
            if out.get("result") != "success":
                raise RuntimeError(f"transmission {method}: {out.get('result')}")
            return out.get("arguments", {})
        raise RuntimeError("transmission: no session id")

    def ping(self):
        return "version" in self.rpc("session-get", {"fields": ["version"]})

    def add(self, torrents, seeding=False):
        for t in torrents:
            self.rpc("torrent-add", {"metainfo": base64.b64encode(t).decode(), "download-dir": self.save,
                                     "paused": False})

    def count(self):
        return len(self.rpc("torrent-get", {"fields": ["id"]})["torrents"])

    def done(self, ih):
        t = self.rpc("torrent-get", {"ids": [ih], "fields": ["leftUntilDone", "sizeWhenDone"]})["torrents"]
        return (t[0]["sizeWhenDone"] - t[0]["leftUntilDone"], t[0]["sizeWhenDone"]) if t else (0, 0)

    def seeding(self, ih):
        t = self.rpc("torrent-get", {"ids": [ih], "fields": ["status"]})["torrents"]
        return bool(t) and t[0]["status"] == 6

    def version(self):
        return "Transmission " + self.rpc("session-get", {"fields": ["version"]})["version"]


class Deluge(Client):
    name = "deluge"

    def command(self):
        return ["deluged", "-d", "-c", self.work, "-p", str(self.api), "-u", "127.0.0.1",
                "-L", "warning", "-l", os.path.join(self.work, "deluged.log")]

    def _client(self):
        if getattr(self, "_c", None) is None:
            from deluge_client import DelugeRPCClient
            with open(os.path.join(self.work, "auth")) as f:
                user, pw = f.readline().strip().split(":")[:2]
            self._c = DelugeRPCClient("127.0.0.1", self.api, user, pw, decode_utf8=True)
            self._c.connect()
        return self._c

    def ping(self):
        self._c = None
        return bool(self._client().call("daemon.info"))

    def configure(self):
        self._client().call("core.set_config", {
            "dht": False, "lsd": False, "utpex": False, "upnp": False, "natpmp": False,
            "random_port": False, "listen_ports": [self.port, self.port], "download_location": self.save,
            "new_release_check": False,
        })
        return True

    def stop(self, timeout=300):
        self._c = None
        return super().stop(timeout)

    def add(self, torrents, seeding=False):
        c = self._client()
        opts = {"download_location": self.save, "add_paused": False, "seed_mode": seeding}
        for i in range(0, len(torrents), 100):
            c.call("core.add_torrent_files",
                   [(f"{i + k}.torrent", base64.b64encode(t).decode(), opts)
                    for k, t in enumerate(torrents[i:i + 100])])

    def count(self):
        return len(self._client().call("core.get_session_state"))

    def done(self, ih):
        s = self._client().call("core.get_torrent_status", ih, ["total_done", "total_wanted"])
        return (s["total_done"], s["total_wanted"]) if s else (0, 0)

    def seeding(self, ih):
        return self._client().call("core.get_torrent_status", ih, ["state"]).get("state") == "Seeding"

    def version(self):
        c = self._client()
        return f"Deluge {c.call('daemon.info')} (libtorrent {c.call('core.get_libtorrent_version')})"


class ScgiTransport(xmlrpc.client.Transport):
    def __init__(self, sock):
        super().__init__()
        self.sock = sock

    def request(self, host, handler, body, verbose=False):
        headers = b"CONTENT_LENGTH\0%d\0SCGI\x001\0" % len(body)
        s = socket.socket(socket.AF_UNIX)
        s.settimeout(120)
        s.connect(self.sock)
        s.sendall(b"%d:%s," % (len(headers), headers) + body)
        chunks = []
        while True:
            c = s.recv(1 << 16)
            if not c:
                break
            chunks.append(c)
        s.close()
        resp = b"".join(chunks)
        payload = resp.split(b"\r\n\r\n", 1)[1]
        p, u = self.getparser()
        p.feed(payload)
        p.close()
        return u.close()


class Rtorrent(Client):
    name = "rtorrent"

    def __init__(self, work, save):
        super().__init__(work, save)
        self.sock = os.path.join(self.work, "rpc.sock")
        os.makedirs(os.path.join(self.work, "session"))
        with open(os.path.join(self.work, "rtorrent.rc"), "w") as f:
            f.write(f"""directory.default.set = {save}
session.path.set = {self.work}/session
network.port_range.set = {self.port}-{self.port}
network.port_random.set = no
dht.mode.set = disable
protocol.pex.set = no
network.scgi.open_local = {self.sock}
system.daemon.set = true
""")

    def command(self):
        return ["rtorrent", "-n", "-o", f"import={self.work}/rtorrent.rc"]

    @property
    def x(self):
        return xmlrpc.client.ServerProxy("http://localhost/", transport=ScgiTransport(self.sock))

    def ping(self):
        return os.path.exists(self.sock) and bool(self.x.system.client_version())

    def add(self, torrents, seeding=False):
        x = self.x
        for t in torrents:
            x.load.raw_start("", xmlrpc.client.Binary(t), f"d.directory.set={self.save}")

    def count(self):
        return len(self.x.download_list(""))

    def done(self, ih):
        x, h = self.x, ih.upper()
        return x.d.completed_bytes(h), x.d.size_bytes(h)

    def seeding(self, ih):
        return self.x.d.complete(ih.upper()) == 1

    def diagnose(self, ih):
        x, h = self.x, ih.upper()
        out = {}
        for m in ("message", "peers_connected", "chunks_hashed", "completed_chunks", "size_chunks",
                  "left_bytes", "is_hash_checking", "state", "down.rate"):
            try:
                out[m] = getattr(x.d, m)(h)
            except Exception as e:
                out[m] = str(e)[:60]
        for m in ("pieces.memory.current", "pieces.memory.max", "pieces.sync.queue_size",
                  "pieces.stats_not_preloaded", "throttle.global_down.max_rate"):
            try:
                out[m] = x.__getattr__(m)("")
            except Exception as e:
                out[m] = str(e)[:60]
        return out

    def stop(self, timeout=300):
        if self.proc is None or self.proc.poll() is not None:
            return 0.0
        t0 = time.monotonic()
        try:
            self.x.system.shutdown.normal()
        except Exception:
            self.proc.send_signal(signal.SIGTERM)
        try:
            self.proc.wait(timeout)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
            return float("inf")
        return time.monotonic() - t0

    def version(self):
        return f"rTorrent {self.x.system.client_version()} (libtorrent {self.x.system.library_version()})"


CLIENTS = {"patorrent": Patorrent, "qbittorrent": Qbittorrent, "transmission": Transmission,
           "deluge": Deluge, "rtorrent": Rtorrent}


def launch(cls, work, save, **kw):
    c = cls(work, save, **kw)
    c.start()
    if c.configure():
        c.stop()
        c.start()
    return c


def record(out, row):
    row["time"] = time.strftime("%Y-%m-%dT%H:%M:%S%z")
    line = json.dumps(row)
    print(line, flush=True)
    with open(out, "a") as f:
        f.write(line + "\n")


def idle(c, settle, window):
    """Lets the client settle, then measures CPU over `window` seconds without touching it."""
    time.sleep(settle)
    c0, t0 = cpu_seconds(c.pid), time.monotonic()
    time.sleep(window)
    c1, t1 = cpu_seconds(c.pid), time.monotonic()
    rss, hwm, pss = memory(c.pid)
    return {"idle_cpu_pct": round(100 * (c1 - c0) / (t1 - t0), 3), "rss_mb": round(rss / 2**20, 1),
            "peak_rss_mb": round(hwm / 2**20, 1), "pss_mb": round(pss / 2**20, 1)}


# ------------------------------------------------------------------------------------ scenarios


def many(a):
    torrents = corpus(a.count, "http://127.0.0.1:9/announce")
    meta_mb = sum(map(len, torrents)) / 2**20
    print(f"corpus: {a.count} torrents, {meta_mb:.0f} MiB of metadata", flush=True)
    for name in a.clients:
        work = os.path.join(a.work, f"many-{a.count}")
        save = os.path.join(work, "data", name)
        c = None
        row = {"scenario": "many", "client": name, "count": a.count, "metadata_mib": round(meta_mb, 1)}
        try:
            c = launch(CLIENTS[name], work, save)
            row["version"] = c.version()
            row.update({f"empty_{k}": v for k, v in idle(c, 5, 10).items()})
            cpu0, t0 = cpu_seconds(c.pid), time.monotonic()
            c.add(torrents)
            row["add_request_s"] = round(time.monotonic() - t0, 2)
            wait_for("all listed", lambda: c.has(a.count), a.timeout, 0.25)
            row["add_s"] = round(time.monotonic() - t0, 2)
            row["add_cpu_s"] = round(cpu_seconds(c.pid) - cpu0, 2)
            row.update({f"added_{k}": v for k, v in idle(c, a.settle, a.window).items()})
            row["stop_s"] = round(c.stop(), 2)
            c.start()
            wait_for("all listed after restart", lambda: c.has(a.count), a.timeout, 0.1)
            row["restart_s"] = round(time.monotonic() - c.t_start, 2)
            row["restart_cpu_s"] = round(cpu_seconds(c.pid), 2)
            row.update({f"restarted_{k}": v for k, v in idle(c, a.settle, a.window).items()})
            row["count_seen"] = c.count()
        except Exception as e:
            row["error"] = f"{type(e).__name__}: {e}"
        finally:
            if c:
                c.stop()
                if not a.keep:
                    c.clean()
                    shutil.rmtree(c.work, ignore_errors=True)
            record(a.out, row)


def transfer(a):
    tracker = Tracker(free_port())
    ram = a.ram
    seed_dir = os.path.join(ram, "seed")
    payload = os.path.join(seed_dir, f"payload-{a.mib}MiB.bin")
    print(f"writing {a.mib} MiB of random data", flush=True)
    shutil.rmtree(ram + "/seed", ignore_errors=True)
    info = make_payload(payload, a.mib)
    torrent = bencode({"announce": tracker.url, "info": info})
    ih = info_hash(torrent)
    want = sha1_file(payload)
    seed = launch(Qbittorrent, a.work, seed_dir, tcp_only=True, label="seed-qbittorrent")
    seed.add([torrent], seeding=True)
    wait_for("seed ready", lambda: seed.seeding(ih), 600, 0.2)
    # the ports the clients really listen on are the ones they announce
    swarm = tracker.swarms.setdefault(bytes.fromhex(ih), set())
    wait_for("seed announced", lambda: len(swarm) == 1, 120, 0.2)
    seed_port = next(iter(swarm))
    print(f"seed listens on {seed_port} (configured {seed.port})", flush=True)
    try:
        for run in range(a.runs):
            # a different client goes first each run: a slow spell of the host is spread out
            order = a.clients[run % len(a.clients):] + a.clients[:run % len(a.clients)]
            for name in order:
                dl = os.path.join(ram, "dl")
                shutil.rmtree(dl, ignore_errors=True)
                row = {"scenario": "transfer", "client": name, "mib": a.mib, "run": run + 1,
                       "seed": seed.version()}
                c = None
                try:
                    c = launch(CLIENTS[name], a.work, dl)
                    row["version"] = c.version()
                    time.sleep(2)
                    cpu0, t0 = cpu_seconds(c.pid), time.monotonic()
                    seed_cpu0 = cpu_seconds(seed.pid)
                    before = set(swarm)
                    c.add([torrent])
                    # Samples of (time, CPU, bytes received from the seed). The kernel's byte
                    # counts are exact and the same for every client (their APIs lag by up to a
                    # second); the steady-state figures are taken between 10% and 90% of the
                    # payload, leaving out the announce, the connect and the endgame.
                    # Completion is the client's own word: every piece verified.
                    size = a.mib << 20
                    samples, next_poll, own_port, seen = [], t0, None, {}
                    while True:
                        now = time.monotonic()
                        if now - t0 > a.timeout:
                            raise TimeoutError(f"download: {c.done(ih)} after {a.timeout} s; "
                                               f"{c.diagnose(ih)}")
                        if own_port is None:
                            own_port = next(iter(set(swarm) - before), None)
                        got_bytes = received(seed_port, own_port, seen) if own_port else 0
                        samples.append((now - t0, cpu_seconds(c.pid) - cpu0, got_bytes))
                        if now >= next_poll:
                            if samples[-1][2] > 0 and c.complete(ih):
                                break
                            next_poll += 0.25
                        time.sleep(0.05)

                    def at(frac):
                        return next(s for s in samples if s[2] >= frac * size)
                    # 80%, not 90%: a connection's last bytes before it closes can fall between
                    # two samples (up to 50 ms of transfer)
                    first, lo, hi, last = at(1e-9), at(0.1), at(0.8), samples[-1]
                    window = 0.7 * a.mib
                    row["seconds"] = round(last[0], 2)
                    row["first_byte_s"] = round(first[0], 2)
                    row["cpu_s"] = round(last[1], 2)
                    row["cpu_s_per_gib_total"] = round(last[1] / (a.mib / 1024), 2)
                    row["mib_per_s"] = round(window / (hi[0] - lo[0]), 1)
                    row["cpu_s_per_gib"] = round((hi[1] - lo[1]) / (window / 1024), 2)
                    row["received_mib"] = round(last[2] / 2**20, 1)
                    row["seed_cpu_s"] = round(cpu_seconds(seed.pid) - seed_cpu0, 2)
                    row["threads"] = threads(c.pid)
                    row["trace"] = [[round(t, 2), round(u, 2), round(b / 2**20)] for t, u, b in samples[::10]]
                    got = os.path.join(c.save, os.path.basename(payload))
                    c.stop()  # the files are flushed and renamed now
                    row["verified"] = sha1_file(got) == want
                except Exception as e:
                    row["error"] = f"{type(e).__name__}: {e}"
                finally:
                    if c:
                        c.stop()
                        c.clean()
                    record(a.out, row)
    finally:
        seed.stop()


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("scenario", choices=["many", "transfer"])
    p.add_argument("--clients", default=",".join(ALL))
    p.add_argument("--count", type=int, default=1000)
    p.add_argument("--mib", type=int, default=2048)
    p.add_argument("--runs", type=int, default=3)
    p.add_argument("--settle", type=float, default=60, help="seconds before an idle measurement")
    p.add_argument("--window", type=float, default=60, help="seconds of idle measurement")
    p.add_argument("--timeout", type=float, default=1800)
    p.add_argument("--work", default=os.path.join(HOME, "bench/work"))
    p.add_argument("--ram", default="/mnt/benchram", help="tmpfs for the transfer data")
    p.add_argument("--out", default=os.path.join(HOME, "bench/results.jsonl"))
    p.add_argument("--keep", action="store_true")
    a = p.parse_args()
    a.clients = [c for c in a.clients.split(",") if c]
    for c in a.clients:
        if c not in CLIENTS:
            p.error(f"unknown client {c}")
    os.makedirs(a.work, exist_ok=True)
    {"many": many, "transfer": transfer}[a.scenario](a)


if __name__ == "__main__":
    main()
