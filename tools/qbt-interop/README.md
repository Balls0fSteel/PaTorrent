# patorrent-nox against a current qBittorrent

`run.py` drives a release `patorrent-nox` and a qBittorrent 5.x container over their WebAPIs
(nox's qBittorrent shim and its `/pa/prefs`) on one Linux machine. It covers the release checks
for the libtorrent 2.0.12-2.1.2 ports ([BACKLOG](../../docs/BACKLOG.md) "libtorrent
2.0.12-2.1.2 follow-ups"). First run 2026-11-02 on the Xubuntu VM.

- `v2`, `hybrid`: a folder of ten files with odd sizes (1 byte, 1000 bytes, block and piece
  boundaries plus or minus one, primes; about 38.5 MB), as a torrent made by our creator and
  by qBittorrent's (`torrentcreator/addTask`), with 16 KiB, 4 MiB and 16 MiB pieces. Each is
  downloaded by nox from qBittorrent (`down`, nox connects) and by qBittorrent from nox
  (`up`, qBittorrent connects). Pass: complete, files equal byte for byte, a recheck on the
  downloader passes, no wasted bytes (failed + redundant) on either side.
- `mse`: v1 and hybrid with RC4 forced on both sides (nox `out_enc_policy`/`in_enc_policy`
  forced, `allowed_enc_level` RC4; qBittorrent "Require encryption"), both directions; every
  peer row on both sides must carry the `E` flag.
- `queue`: nox with `active_downloads` 2, `active_seeds` 1 and `dont_count_slow_torrents`,
  five 64 MiB downloads (one at 5 KiB/s from qBittorrent, one with no source, three at
  300 KiB/s) and two idle seeds. Expects the slow ones to hold their slots for
  `auto_manage_startup` (`--startup`, default 30 s) and then stop counting, fast ones to count,
  a torrent turning slow to free its slot once its rate has fallen below the threshold and
  the startup has passed again, and one turning fast to count again at once. The threshold is
  raised to `--inactive-down-rate` 20480: data arrives in 16 KiB blocks, so a trickle source
  reads above the default 2048 B/s whenever a block lands and counts again at once (upstream
  behaves the same).

A `down` run waits 5 s after qBittorrent reports the seed checked: qBittorrent keeps a new
torrent paused in libtorrent for a moment after its check, and libtorrent then drops incoming
connections when no torrent is active (a uTP connect then waits 15 s; BACKLOG has the details).

Everything is on 127.0.0.1: nox listens on loopback, the container uses the host network with
DHT, LSD, PeX and port forwarding off, and the torrents have no trackers.

```sh
docker pull lscr.io/linuxserver/qbittorrent:5.3.0_v2.1.2-ls486    # libtorrent 2.1.2
docker pull lscr.io/linuxserver/qbittorrent:5.3.0_v2.0.15-ls482    # libtorrent 2.0.15
python3 run.py all --nox ~/qbt-interop/bin/patorrent-nox \
    --image lscr.io/linuxserver/qbittorrent:5.3.0_v2.1.2-ls486 --work ~/qbt-interop/work --out results.json
```

`--piece-sizes 16,4096` (KiB) and `--creators patorrent` narrow a run. The work directory keeps
the logs (`nox-<check>.log` with `--verbose` disconnect reasons, `qbt-docker.log`) and the
generated data; remove it afterwards. Ports 18180/16981 (qBittorrent) and 18190/16991 (nox)
must be free.
