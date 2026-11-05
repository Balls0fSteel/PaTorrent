#!/bin/sh
# D47 on Linux: rookparcel-gtk closing while torrents move to another (slow) filesystem.
#   closing-test.sh setup     data, torrents, the slow target filesystem, settings
#   closing-test.sh start     Xvfb :78 and the app in a throttled scope; torrents dropped in the
#                             watch folder; returns once the moves have started
#   closing-test.sh shot NAME screenshot of the display to run/NAME.png
#   closing-test.sh term      SIGTERM to the app (as a logout or `kill` would)
#   closing-test.sh cleanup   stops everything, unmounts and deletes the test files
# The app runs as this user in a transient systemd scope: disk reads from /dev/sda throttled
# and memory capped, so the copies cannot come from the page cache and take a while.
set -eu
B=$HOME/closing-test
APP=${APP:-$HOME/rookparcel-sec/target/debug/rookparcel-gtk}
MNT=/mnt/ptest
N=${N:-5}
SIZE_MB=${SIZE_MB:-120}

case "$1" in
setup)
    rm -rf "$B/run"
    mkdir -p "$B/run/cfg/rookparcel/gtk" "$B/run/data" "$B/run/dl" "$B/run/watch" "$B/run/torrents"
    i=1
    while [ "$i" -le "$N" ]; do
        head -c "${SIZE_MB}M" /dev/urandom > "$B/run/dl/disk-image-$i.bin"
        python3 "$B/mktorrent.py" "$B/run/dl/disk-image-$i.bin" "$B/run/torrents/disk-image-$i.torrent"
        i=$((i + 1))
    done
    if ! mountpoint -q "$MNT"; then
        truncate -s 2G "$B/slow.img"
        mkfs.ext4 -q -F "$B/slow.img"
        sudo mkdir -p "$MNT"
        sudo mount -o loop "$B/slow.img" "$MNT"
        sudo chown "$(id -u):$(id -g)" "$MNT"
    fi
    rm -rf "$MNT/done"
    cat > "$B/run/cfg/rookparcel/gtk/rookparcel.ini" <<EOF
lt.enable_dht=false
lt.enable_lsd=false
lt.enable_upnp=false
lt.enable_natpmp=false
lt.listen_interfaces=127.0.0.1:0
dir.download=$B/run/dl
dir.move_completed=1
dir.completed=$MNT/done
dir.autoload=1
dir.autoload_dir=$B/run/watch
ui.close_to_tray=0
EOF
    sync
    echo "setup: $N x ${SIZE_MB} MB in $B/run/dl, target $MNT/done"
    ;;
start)
    # out of the page cache, so the checks and copies read the disk
    for f in "$B"/run/dl/*.bin; do dd if="$f" iflag=nocache count=0 status=none; done
    Xvfb :78 -screen 0 1280x800x24 -nolisten tcp > "$B/run/xvfb.log" 2>&1 &
    echo $! > "$B/run/xvfb.pid"
    sleep 1
    sudo systemd-run --scope --quiet --uid="$(id -u)" --gid="$(id -g)" \
        -p MemoryMax=400M -p "IOReadBandwidthMax=/dev/sda 25M" -- \
        env HOME="$HOME" PATH="$PATH" DISPLAY=:78 GDK_BACKEND=x11 NO_AT_BRIDGE=1 \
        XDG_CONFIG_HOME="$B/run/cfg" XDG_DATA_HOME="$B/run/data" \
        dbus-run-session -- "$APP" > "$B/run/app.log" 2>&1 &
    sleep 4
    cp "$B"/run/torrents/*.torrent "$B/run/watch/"
    # the moves start once the checks are done
    i=0
    while [ ! -d "$MNT/done" ] && [ "$i" -lt 240 ]; do sleep 0.5; i=$((i + 1)); done
    echo "moves started after $((i / 2)) s"
    ;;
shot)
    DISPLAY=:78 import -window root "$B/run/$2.png"
    ;;
term)
    pkill -TERM -x rookparcel-gtk || true
    ;;
cleanup)
    pkill -TERM -x rookparcel-gtk || true
    sleep 2
    pkill -KILL -x rookparcel-gtk || true
    [ -f "$B/run/xvfb.pid" ] && kill "$(cat "$B/run/xvfb.pid")" 2>/dev/null || true
    if mountpoint -q "$MNT"; then sudo umount "$MNT"; fi
    sudo rmdir "$MNT" 2>/dev/null || true
    rm -rf "$B/run" "$B/slow.img"
    echo cleaned
    ;;
esac
