# rookparcel-nox, the headless daemon: Web UI, the native push API and the qBittorrent WebAPI
# v2-compatible shim that Sonarr, Radarr and Prowlarr use (docs/wiki/Docker.md).
#
#   docker build -t rookparcel-nox .
#   docker run -d --name rookparcel -p 8080:8080 -p 6881:6881 -p 6881:6881/udp \
#     -v rookparcel-config:/config -v /srv/media:/srv/media rookparcel-nox
#
# The build stage compiles the whole workspace's manifests but only the nox package; the
# runtime stage is Debian slim with OpenSSL 3 (linked by hand: crates/core/src/net/tls/openssl.rs)
# and the CA certificates. Pass --build-arg CARGO_BUILD_JOBS=1 to build on one core.

FROM rust:1-bookworm AS build
RUN apt-get update \
 && apt-get install -y --no-install-recommends libssl-dev \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# Cargo needs every workspace member's manifest and sources (crates/*, tools/swarm); nox's
# build.rs includes crates/win32/res/resource.rs and the frontend's embeds assets/
COPY . .
# empty: Cargo's default (every core)
ARG CARGO_BUILD_JOBS=
# a container is updated by pulling a new image, so no update check and no search helper
# download (crates/frontend/src/update.rs)
ENV ROOKPARCEL_NO_UPDATE_CHECK=1
RUN if [ -z "$CARGO_BUILD_JOBS" ]; then unset CARGO_BUILD_JOBS; fi \
 && cargo build --release --locked -p rookparcel-nox \
 && install -D -m 755 target/release/rookparcel-nox /out/rookparcel-nox

FROM debian:bookworm-slim
ARG VERSION=dev
LABEL org.opencontainers.image.title="rookparcel-nox" \
      org.opencontainers.image.description="Rookparcel's headless BitTorrent daemon with its Web UI and a qBittorrent-compatible WebAPI" \
      org.opencontainers.image.source="https://github.com/Balls0fSteel/Rookparcel" \
      org.opencontainers.image.url="https://github.com/Balls0fSteel/Rookparcel/wiki/Docker" \
      org.opencontainers.image.documentation="https://github.com/Balls0fSteel/Rookparcel/wiki/Docker" \
      org.opencontainers.image.licenses="PolyForm-Noncommercial-1.0.0" \
      org.opencontainers.image.version="${VERSION}"
RUN apt-get update \
 && apt-get install -y --no-install-recommends libssl3 ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --gid 1000 rookparcel \
 && useradd --uid 1000 --gid 1000 --no-create-home --home-dir /config --shell /usr/sbin/nologin rookparcel \
 && mkdir -p /config /downloads \
 && chown rookparcel:rookparcel /config /downloads
COPY --from=build /out/rookparcel-nox /usr/local/bin/rookparcel-nox
COPY contrib/docker/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
# a rookparcel.ini (or PaTorrent's patorrent.ini) beside the binary would mean portable mode,
# whose folders win over --state-dir's defaults; the binary starts once as a check
RUN chmod 755 /usr/local/bin/docker-entrypoint.sh \
 && test ! -e /usr/local/bin/rookparcel.ini && test ! -e /usr/local/bin/patorrent.ini \
 && rookparcel-nox --version
# HOME too: a command line without --state-dir still keeps its files in the volume
ENV HOME=/config
USER rookparcel
WORKDIR /downloads
VOLUME ["/config", "/downloads"]
# the Web UI and WebAPI; the listening port for peers (TCP, and UDP for uTP and the DHT)
EXPOSE 8080/tcp 6881/tcp 6881/udp
# nox saves its state on SIGTERM; give it time (docker stop -t 60, compose stop_grace_period)
STOPSIGNAL SIGTERM
ENTRYPOINT ["docker-entrypoint.sh"]
# docker-entrypoint.sh adds options given alone to the same line
CMD ["run", "--state-dir", "/config", "--save", "/downloads", "--webui", "0.0.0.0:8080", "--port", "6881"]
