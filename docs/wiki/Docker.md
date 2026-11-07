# Docker

`rookparcel-nox`, the daemon without a window, also comes as a container image for Docker and other container engines. This page covers starting it with `docker run` or Docker Compose, its folders and ports, the Web UI password, running it as another user, using it with Sonarr, Radarr and Prowlarr in containers, and stopping and updating it.

*New in 1.2.21; not in a published release yet.*

The image is `ghcr.io/balls0fsteel/rookparcel-nox`, for x86-64 (`amd64`) and 64-bit ARM (`arm64`) Linux. Each release has its own tag, such as `:1.2.21`, and `:latest` is the newest release (not a pre-release). Inside, nox runs the same way as on any Linux computer: everything on [Running rookparcel-nox](Running-rookparcel-nox) and [Web UI](Web-UI) applies.

## A first start

```sh
docker run -d --name rookparcel --restart unless-stopped --stop-timeout 60 \
  -p 8080:8080 -p 6881:6881 -p 6881:6881/udp \
  -v rookparcel-config:/config -v rookparcel-downloads:/downloads \
  ghcr.io/balls0fsteel/rookparcel-nox
```

1. Read the temporary password from the log:

   ```sh
   docker logs rookparcel
   ```

   ```text
   * Web UI: no password set; temporary password for this run: ... (set one with --webui-pass)
   ```

2. Open `http://127.0.0.1:8080/` (or `http://<the computer's address>:8080/` from another computer) and log in as `admin` with it.
3. In the page, open **Preferences > Web UI** and set your own **Password:**, with the temporary one as the **Current password:**.

The temporary password is printed on the first start only, and it stays valid until you set your own. If the log is gone before you read it (the container was removed), set a password with `ROOKPARCEL_WEBUI_PASS` ([below](#the-web-ui-password)).

Without a command, the image runs:

```sh
rookparcel-nox run --state-dir /config --save /downloads --webui 0.0.0.0:8080 --port 6881
```

Options after the image name are added to that command, and an option given again wins over the default: `... ghcr.io/balls0fsteel/rookparcel-nox --save /srv/media/downloads --verbose`. A command after the image name replaces it instead: `... ghcr.io/balls0fsteel/rookparcel-nox run --state-dir /config --save /srv/media/downloads --webui 0.0.0.0:8080 --port 6881`. Every option is listed on [Running rookparcel-nox](Running-rookparcel-nox#options-of-run-and-add).

## Folders

| Folder | What it holds |
|---|---|
| `/config` | Everything nox keeps: the torrent list and each torrent's progress, the DHT state, the Web UI's login and preferences (`webui.json`), labels and the RSS Downloader. Keep it on a volume, or a restart begins with an empty list. |
| `/downloads` | The default download folder. |

Both are volumes. For Sonarr, Radarr and Prowlarr, mount your media folder instead at the same path the apps use, and save into it ([below](#sonarr-radarr-and-prowlarr-in-containers)).

To back up, stop the container and copy the `/config` volume. To start over, remove the container and the volume.

## Ports

| Port | For |
|---|---|
| `8080/tcp` | The Web UI and the qBittorrent-compatible API. |
| `6881/tcp`, `6881/udp` | The listening port for peers (UDP carries uTP and the DHT). |

Map the listening port to the same number outside and inside (`-p 6881:6881`): nox tells trackers and peers the port it listens on, so `-p 6882:6881` would give them the wrong one. To use another port, change both, for example `-p 51413:51413 -p 51413:51413/udp` and `--port 51413`. A port changed later in the Web UI's **Preferences > Connection** needs the same change in the mapping.

**UPnP and NAT-PMP do not work from Docker's usual (bridge) network:** the router cannot be reached from inside it, so the port is not forwarded for you. Forward it in the router by hand, TCP and UDP, to the computer running Docker ([Forwarding the port in the router](Connection-ports-and-firewall#forwarding-the-port-in-the-router)), or, on Linux, run the container on the host's network (`--network host`), where nox sees the router and the local network as a program outside a container does, and `-p` is not needed. Local Peer Discovery also finds peers on your network only with host networking.

## The Web UI password

| Variable | Meaning |
|---|---|
| `ROOKPARCEL_WEBUI_PASS` | Sets the Web UI password at every start. It is stored hashed in `/config`, so it stays after you remove the variable. |
| `ROOKPARCEL_WEBUI_USER` | The login name that goes with it (default `admin`). Used only together with `ROOKPARCEL_WEBUI_PASS`. |

```sh
docker run -d --name rookparcel ... -e ROOKPARCEL_WEBUI_PASS='a long password' ghcr.io/balls0fsteel/rookparcel-nox
```

Without `ROOKPARCEL_WEBUI_PASS`, nox keeps the stored password, or prints a temporary one when none is stored yet.

Two things to know:

- While the variable is set, it wins at every start: a password you changed in the page goes back to the variable's at the next start. Remove the variable to keep the one set in the page.
- `docker inspect` shows the variable, and nox gets the password on its command line, which `docker top` and the host's process list show. On a computer others can log in to, use the temporary password and set your own in the page instead, or remove the variable after the first start.

The Web UI is plain HTTP. On a network you do not trust, serve it over HTTPS (`--webui-https`) or put it behind a reverse proxy that does: see [HTTPS](Web-UI#https) and [Web UI security](Web-UI#security).

## User and permissions

nox runs as the user `rookparcel`, user and group ID 1000, never as root. New files are readable by everyone and writable by their group (umask `0002`), so the apps in other containers can import and remove them.

| To | Do |
|---|---|
| Write into a host folder that another user owns | Start the container as that user: `--user 1001:1001` (`user: "1001:1001"` in Compose). Find the IDs with `id -u` and `id -g`. |
| Use another umask | `-e UMASK=0022` (files readable by everyone, writable only by their owner). |

With another user, `/config` must be writable by it too. A new named volume belongs to user 1000, so use a host folder you give to that user instead:

```sh
sudo mkdir -p /srv/rookparcel && sudo chown 1001:1001 /srv/rookparcel
docker run -d --name rookparcel --user 1001:1001 -v /srv/rookparcel:/config ...
```

## Docker Compose

The repository has a ready-made [compose.yaml](https://github.com/Balls0fSteel/Rookparcel/blob/main/contrib/docker/compose.yaml). It keeps `/config` in a volume, mounts `/srv/media` at the same path, saves into `/srv/media/downloads`, and gives nox 60 seconds to stop:

```yaml
services:
  rookparcel:
    image: ghcr.io/balls0fsteel/rookparcel-nox:latest
    container_name: rookparcel
    restart: unless-stopped
    user: "1000:1000"
    environment:
      ROOKPARCEL_WEBUI_PASS: ${ROOKPARCEL_WEBUI_PASS:-}
    command: [run, --state-dir, /config, --save, /srv/media/downloads, --webui, "0.0.0.0:8080", --port, "6881"]
    volumes:
      - rookparcel-config:/config
      - /srv/media:/srv/media
    ports:
      - "8080:8080"
      - "6881:6881/tcp"
      - "6881:6881/udp"
    stop_grace_period: 60s

volumes:
  rookparcel-config:
```

1. Put the password in a file `.env` beside `compose.yaml`: `ROOKPARCEL_WEBUI_PASS=a long password`. Without it, read the temporary one with `docker compose logs rookparcel`.
2. Change `/srv/media` and `user:` to your media folder and its owner.
3. Start it with `docker compose up -d`.

## Sonarr, Radarr and Prowlarr in containers

Set Rookparcel up in the apps as a **qBittorrent** client, as [Sonarr, Radarr and Prowlarr](Sonarr-Radarr-and-Prowlarr) describes. In containers, three things differ:

1. **The same path in every container.** Mount the media folder at the same path in nox's container and in the apps' (`/srv/media` in each, as in the Compose file above), and save the downloads inside it. The apps then find the files where nox reports them, and imports can use hardlinks.
2. **The host name.** On the same Compose or user-defined network, the apps reach nox as `http://rookparcel:8080` (the service or container name). nox answers only to IP addresses and `localhost` until you allow the name: add `rookparcel` to **Server domains (separate multiple entries with a semicolon):** in the Web UI's **Preferences > Web UI**, under **Connectivity**. Until then the apps' **Test** fails with "Bad Request: unexpected Host header". See [Host names and Docker](Sonarr-Radarr-and-Prowlarr#host-names-and-docker).
3. **The download folder the apps ask for** must be a path inside nox's container, such as `/srv/media/downloads`.

`--webui-save-root /srv/media` (added to the `run` command) limits the folders the Web UI and the apps can save into.

## Logs, stopping and reloading

nox writes its log to the container's output: `docker logs rookparcel` (`-f` to follow). `--verbose` adds tracker and peer events.

nox saves the state of every torrent when it stops. `docker stop` sends `SIGTERM` and waits 10 seconds by default before killing the container, which can be too short for a large list, so give it a minute:

```sh
docker stop -t 60 rookparcel
```

`--stop-timeout 60` on `docker run`, or `stop_grace_period: 60s` in Compose, makes that the default for the container. A clean stop ends with these lines in the log, and the container exits with code 0:

```text
* SIGTERM received (again to quit at once, without saving)
* shutting down
* stopped
```

`docker kill -s HUP rookparcel` reloads the IP filter, as `SIGHUP` does outside a container ([Stopping and reloading](Running-rookparcel-nox#stopping-and-reloading)).

## Updating

The image does not check for updates, and its Web UI shows no update link: a container is updated by pulling the new image and starting a new container on the same `/config`.

```sh
docker pull ghcr.io/balls0fsteel/rookparcel-nox
docker stop -t 60 rookparcel && docker rm rookparcel
docker run -d --name rookparcel ...    # the same command as before
```

With Compose: `docker compose pull`, then `docker compose up -d`.

For the same reason the [search helper](Searching-for-torrents#the-search-helper) is not offered in the image. Your own Prowlarr or Jackett can still be added as a search source ([Adding your own Prowlarr or Jackett](Searching-for-torrents#adding-your-own-prowlarr-or-jackett)).

## Building the image yourself

From a copy of the repository:

```sh
docker build -t rookparcel-nox .
```

The build compiles nox in a Rust container and copies it into a small Debian image. It takes a few minutes on all the computer's cores; `--build-arg CARGO_BUILD_JOBS=1` uses one. Then use `rookparcel-nox` in place of `ghcr.io/balls0fsteel/rookparcel-nox` in the commands above.

## See also

- [Running rookparcel-nox](Running-rookparcel-nox): every command and option.
- [Web UI](Web-UI): the page, HTTPS and security.
- [Sonarr, Radarr and Prowlarr](Sonarr-Radarr-and-Prowlarr): the apps' settings, categories and Remove Completed.
- [Connection, ports and firewall](Connection-ports-and-firewall): checking that peers can connect.
- The image's files: [Dockerfile](https://github.com/Balls0fSteel/Rookparcel/blob/main/Dockerfile) and [contrib/docker](https://github.com/Balls0fSteel/Rookparcel/tree/main/contrib/docker).
