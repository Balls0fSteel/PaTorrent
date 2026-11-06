# Shared by build.sh and run.sh on the Mac VM: paths and the VM lock (sourced, not run).
#
# The lock is a directory, ~/rp/vm.lock, made with mkdir (atomic); its `owner` file names the
# slice, the process and the time. A lock older than 30 minutes is stale and taken over.

RP=${RP_HOME:-$HOME/rp}
export PATH="$HOME/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
LOCK="$RP/vm.lock"
STALE=1800

lock() {
    local who="$1" waited=0
    mkdir -p "$RP"
    while ! mkdir "$LOCK" 2>/dev/null; do
        local since now
        since=$(stat -f %m "$LOCK" 2>/dev/null || echo 0)
        now=$(date +%s)
        if [ $((now - since)) -gt "$STALE" ]; then
            echo "vm.lock: stale ($(cat "$LOCK/owner" 2>/dev/null)), taking it over" >&2
            rm -rf "$LOCK"
            continue
        fi
        if [ $((waited % 60)) -eq 0 ]; then
            echo "vm.lock: held by $(cat "$LOCK/owner" 2>/dev/null); waiting" >&2
        fi
        sleep 5
        waited=$((waited + 5))
    done
    echo "$who pid $$ since $(date '+%Y-%m-%d %H:%M:%S')" > "$LOCK/owner"
    trap unlock EXIT INT TERM
}

unlock() {
    # only our own lock
    if grep -q "pid $$ " "$LOCK/owner" 2>/dev/null; then
        rm -rf "$LOCK"
    fi
}
