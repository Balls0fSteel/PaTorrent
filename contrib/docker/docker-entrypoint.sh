#!/bin/sh
# The image's entrypoint (Dockerfile, docs/wiki/Docker.md): runs rookparcel-nox as PID 1, which
# handles SIGTERM, SIGINT and SIGHUP itself.
#
# - ROOKPARCEL_WEBUI_PASS (and ROOKPARCEL_WEBUI_USER) set the Web UI login of a `run`: nox
#   stores the password hashed in /config/webui.json at every start. Unset, nox keeps the stored
#   one, or prints a temporary password when none is stored yet.
# - UMASK (default 0002): new downloads are group-writable and readable by others, so Sonarr
#   and Radarr in other containers can import and remove them.
# - Arguments that start with an option are added to the default `run` (the Dockerfile's CMD,
#   kept equal to the line below); a later option wins: `docker run IMAGE --verbose`.
#   A command that is not nox's (`sh`) runs instead of nox.
set -e

umask "${UMASK:-0002}"

if [ "$1" = rookparcel-nox ]; then
    shift
fi

case "$1" in
    "" | run | add | create | demo | help | -h | --help | -V | --version | --license) ;;
    -*) set -- run --state-dir /config --save /downloads --webui 0.0.0.0:8080 --port 6881 "$@" ;;
    *)
        if command -v "$1" >/dev/null 2>&1; then
            exec "$@"
        fi
        ;;
esac

if [ "$1" = run ] && [ -n "${ROOKPARCEL_WEBUI_PASS:-}" ]; then
    shift
    # before the given options, so a --webui-pass on the command line still wins
    if [ -n "${ROOKPARCEL_WEBUI_USER:-}" ]; then
        set -- --webui-user "$ROOKPARCEL_WEBUI_USER" "$@"
    fi
    set -- run --webui-pass "$ROOKPARCEL_WEBUI_PASS" "$@"
fi

exec rookparcel-nox "$@"
