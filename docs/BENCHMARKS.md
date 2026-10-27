# Benchmarks against other clients

PaTorrent's daemon (`patorrent-nox`) measured beside the daemons of qBittorrent, Transmission,
Deluge and rTorrent, on the same machine with the same torrents. The website shows the results
([benchmarks.html](../site/benchmarks.html)); this file is the method, so anyone can run it again.
The harness is `tools/bench/bench.py` (Python 3, standard library; Deluge's RPC needs
`pip install deluge-client`) and `tools/bench/summarize.py` turns its JSON lines into tables.

## Results, 2026-10-27

Run on the VMware VM described under Limits; raw results in `docs/benchmarks/2026-10-27-ubuntu-vm.jsonl`. Transfer: 2 GiB, 5 runs each, medians with the range; throughput and CPU between 10% and 80% of the payload; *first byte to done* includes the end of the download (rTorrent slows to about 32 MB/s once some 1.7 GiB of memory-mapped pieces wait to be flushed; Transmission holds 62.4 MiB/s from one peer). Many torrents: one run per count; memory and idle CPU after the restart. Right after adding 10,000, PaTorrent used 319 MB (qBittorrent 636, Transmission 328, Deluge 905, rTorrent 2,101): it keeps metadata only for active torrents, which a restart leaves out.

### Transfer

| Client | Version | Runs | Throughput (MiB/s) | range | CPU s per GiB | range | First byte to done (s) |
|---|---|---|---|---|---|---|---|
| PaTorrent | patorrent-nox 1.0.1 | 5 of 5 | 3,592 | 2,124–3,605 | 1.04 | 0.94–1.16 | 1.6 |
| qBittorrent | v5.1.4 | 5 of 5 | 1,471 | 1,380–1,662 | 2.14 | 2.06–2.22 | 2.5 |
| Transmission | Transmission 4.1.1 (56442e2929) | 5 of 5 | 62.4 | 62.4–62.5 | 2.66 | 2.66–2.72 | 41.2 |
| Deluge | Deluge 2.2.1.dev0+20250824 (libtorrent 2.0.12.0) | 5 of 5 | 1,566 | 1,217–1,670 | 2.11 | 1.97–2.22 | 2.0 |
| rTorrent | rTorrent 0.9.8 (libtorrent 0.13.8) | 5 of 5 | 1,924 | 1,721–2,056 | 0.85 | 0.81–0.94 | 126 |

### 1,000 torrents

| Client | Add (s) | Start with all listed (s) | Memory, RSS (MB) | PSS (MB) | Idle CPU (%) | Empty RSS (MB) |
|---|---|---|---|---|---|---|
| PaTorrent | 0.1 | 0.07 | 15.1 | 10.9 | 0.05 | 9.5 |
| qBittorrent | 0.1 | 0.18 | 105 | 90.1 | 0.07 | 53.6 |
| Transmission | 0.5 | 0.09 | 52.4 | 40.4 | 0.20 | 20.9 |
| Deluge | 51.6 | 0.91 | 135 | 120 | 0.22 | 68.5 |
| rTorrent | 1.1 | 0.55 | 230 | 219 | 0.10 | 16.5 |

### 10,000 torrents

| Client | Add (s) | Start with all listed (s) | Memory, RSS (MB) | PSS (MB) | Idle CPU (%) | Empty RSS (MB) |
|---|---|---|---|---|---|---|
| PaTorrent | 1.3 | 0.19 | 60.2 | 56.1 | 0.07 | 9.5 |
| qBittorrent | 1.2 | 0.86 | 612 | 597 | 0.07 | 53.5 |
| Transmission | 4.9 | 1.00 | 327 | 315 | 0.77 | 20.8 |
| Deluge | 517 | 5.76 | 701 | 686 | 0.28 | 68.4 |
| rTorrent | 24.7 | 22.62 | 2,142 | 2,132 | 1.25 | 16.4 |

## What is measured

**Many torrents** (`bench.py many --count N`). Every client gets the same N generated torrents,
the shape of `tools/swarm`'s corpus: 10 MB to 20 GB (log-uniform), about 1,500 pieces each, 40%
single-file and 60% with 2 to 41 files, one tracker that refuses connections, and no data on
disk. They are added started, through each client's own API, in batches where it has them.

- *Add*: from the first add request until the client's API lists all N.
- *Start with all listed*: the client is stopped (SIGTERM, letting it save its state) and
  started again; the time from starting the process until its API lists all N.
- *Memory* after the restart and 60 s of settling: RSS (resident memory) and PSS (RSS with
  shared libraries divided among the processes using them), summed over the client's processes.
- *Idle CPU*: the client's CPU time over the next 60 s, untouched (no API calls), as a percentage
  of one core.

**Transfer** (`bench.py transfer --mib 2048 --runs 5`). One 2 GiB file of random data in one
torrent (1 MiB pieces), on a tmpfs so the disk is not measured. A reference qBittorrent seeds it,
taking TCP only so every client uses the same transport; a tracker in the harness gives each
side the other's address. Each client downloads it with a fresh profile, five times, a
different client first each round.

- The bytes the downloader received from the seed are read from the kernel's per-connection
  counters (`ss -ti`) every 50 ms: the same for every client, where the clients' own APIs lag by
  up to a second.
- *Throughput* and *CPU per GiB* are taken between 10% and 80% of the payload, leaving out the
  announce, the connection and the end of the download.
- Each download ends when the client says every piece is verified; the harness then stops the
  client and compares the file's SHA-1 with the original.

## Fair-play rules

- Every client runs from a throwaway profile with DHT, local peer discovery, peer exchange and
  port mapping off (no outside peers), everything else at its defaults: queueing, caches,
  connection limits and encryption are each client's own. Nothing is tuned for PaTorrent.
- PaTorrent is the published 1.0.1 release (`patorrent-1.0.1-linux-x86_64.tar.gz`, checked
  against `SHA256SUMS`), not a local build. The other clients are the distribution's packages.
- Ubuntu confines `transmission-daemon` with an AppArmor profile to its service user's folders,
  so the harness runs it as that user (`debian-transmission`), as the packaged service does,
  with its downloads on a tmpfs of its own. The others run as the logged-in user.
- CPU time is user plus system time from `/proc`, summed over the client's processes and
  threads; a `sudo runuser` that only starts a client is left out.
- Results are medians; the range across runs is shown with them.

## Limits

- One machine, a virtual one (VMware Workstation, 8 vCPUs and 8 GB on an Intel i9-13900K host,
  Ubuntu 26.04.1). VMware gives the guest no steal time, so a busy host shows up as extra CPU
  time in the guest; runs are repeated and interleaved for that reason, and a single run should
  not be read on its own.
- Transfers between two processes on one machine show what a client's code costs, not how it
  behaves on the internet: there is no latency, no packet loss and no swarm.
- Because the seed takes TCP only, clients that try uTP first (PaTorrent, qBittorrent, Deluge,
  Transmission) wait for that attempt to fail before connecting; the time to the first byte is
  recorded but not compared.
- The Windows GUI and the GTK GUI are not measured here; `docs/ARCHITECTURE.md` and PLAN.md
  have the 100,000-torrent measurements of the Windows GUI.

## Running it again

On a Linux machine with the clients installed (Ubuntu: `qbittorrent-nox transmission-daemon
deluged deluge-console rtorrent`) and PaTorrent's Linux tarball unpacked where `Patorrent.binary`
in `bench.py` points:

```bash
sudo mount -t tmpfs -o size=6g,uid=$(id -u),gid=$(id -g) tmpfs /mnt/benchram
sudo mount -t tmpfs -o size=6g,uid=debian-transmission,gid=debian-transmission,mode=0755 \
    tmpfs /var/lib/transmission-daemon/downloads
python3 -m venv ~/.venv-linux && ~/.venv-linux/bin/pip install deluge-client
~/.venv-linux/bin/python tools/bench/bench.py transfer --mib 2048 --runs 5 --out results.jsonl
~/.venv-linux/bin/python tools/bench/bench.py many --count 1000 --out results.jsonl
~/.venv-linux/bin/python tools/bench/bench.py many --count 10000 --out results.jsonl
python3 tools/bench/summarize.py results.jsonl
```

`--clients patorrent,qbittorrent` picks clients; `--keep` leaves the profiles in `~/bench/work`.
Transmission needs root through `sudo -n` (the AppArmor note above). The raw results of each
published run are in `docs/benchmarks/`.
