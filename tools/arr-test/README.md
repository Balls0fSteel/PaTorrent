# Sonarr, Radarr and Prowlarr against patorrent-nox

A manual end-to-end check of the qBittorrent shim ([docs/INTEGRATIONS.md](../../docs/INTEGRATIONS.md)) on a Linux machine with Docker: the *arr apps' connection Test, a grab, the import and Remove Completed. First run 2026-10-29 on the Xubuntu VM; results in CHANGELOG.md.

No real releases are used. `fakeznab.py` is a minimal Torznab indexer that serves three synthetic releases, test-pattern videos with a tone made by ffmpeg (about 640 kbit/s, so they pass the apps' minimum sizes and audio check), each as a single-file v1 `.torrent`. It also places the payload in the download folder, so nox finds it complete after its check and seeds it without peers.

## Setup

Python 3, ffmpeg and Docker on the host; paths are fixed in the scripts.

```sh
sudo mkdir -p /srv/media/{downloads,tv,movies} /srv/fakeznab /srv/arr/{sonarr,radarr,prowlarr} /srv/noxcfg
sudo chown -R "$USER" /srv/media /srv/fakeznab /srv/arr /srv/noxcfg
python3 fakeznab.py --data /srv/media/downloads --store /srv/fakeznab --base http://172.17.0.1:9117 &
patorrent-nox run --state-dir /srv/noxcfg --save /srv/media/downloads --webui 0.0.0.0:8080 --webui-pass arr-test-pass &
for a in sonarr:8989 radarr:7878 prowlarr:9696; do n=${a%%:*}; p=${a##*:}
  docker run -d --name $n --add-host patorrent:host-gateway -e PUID=$(id -u) -e PGID=$(id -g) \
    -p $p:$p -v /srv/arr/$n:/config -v /srv/media:/srv/media lscr.io/linuxserver/$n:latest
done
```

The containers reach nox as `patorrent`, a host name, so its Host check refuses them until the name is allowed (`web_ui_domain_list` through `app/setPreferences`, or Preferences > Web UI > Server domains). For a time-only seeding goal, set nox's global ratio target to 0 (`queue.seed_ratio` in `/pa/prefs`).

## Steps

1. `python3 arr.py client <app> patorrent` for each app: the download client's Test, then saved.
2. `python3 arr.py indexer sonarr 1 -` and `... radarr 1 -`: fakeznab with a seed time of 1 minute and no ratio.
3. Root folders `/srv/media/tv` and `/srv/media/movies` (`arr.py post <app> rootfolder '{"path":...}'`).
4. `python3 add.py`: adds Fawlty Towers and Night of the Living Dead (1968) and searches for S01E01 and the movie.
5. `python3 nox.py` lists nox's torrents: expect `ratio_limit -1`, `seeding_time_limit 1`, then `pausedUP` about a minute after the grab, then the torrent gone (Remove Completed) and the file imported by hardlink.
6. `python3 prowlarr.py`: fakeznab as a Generic Torznab indexer in Prowlarr, a search and a grab into category `prowlarr`.

`sh reset.sh` clears the queues and nox's torrents and regenerates the payloads; rescan the series and movie (`RescanSeries`, `RescanMovie` commands) before searching again, or the apps still think they have the files.
