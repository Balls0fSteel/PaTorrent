#!/bin/sh
# D49 keep ping low against a real bloated line, in Linux network namespaces (needs sudo).
#   line.sh up                       builds the line (below)
#   line.sh run DIR [PROTO] [PHASES] DIR upload or download; PROTO tcp (default), utp or both;
#                                    PHASES the client's run, default off:60,on:180
#   line.sh down                     removes the namespaces
#
#   lt-home 192.168.77.2 --- lt-cpe (the modem) ===upload=== lt-isp 84.1.1.1 --- lt-net 203.0.113.10
#                            192.168.77.1/84.1.1.2 <==download==            (the swarm; 1.1.1.1 on lo)
#
# The modem's upload (UP, default 4mbit) and the ISP's download to it (DOWN, 8mbit) are token
# buckets with deep buffers (UPBUF, DOWNBUF: half a second each), and the home side has 4 ms each
# way. The first router past the line is lt-isp's 84.1.1.1, at hop 2. The test binary is
# crates/core/tests/bloated_line.rs, built in release first; it runs as this user.
set -eu
UP=${UP:-4mbit}
DOWN=${DOWN:-8mbit}
UPBUF=${UPBUF:-250kb}
DOWNBUF=${DOWNBUF:-500kb}
SRC=${SRC:-$HOME/rookparcel-ping}
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$HOME/rookparcel-sec/target}
WORK=${WORK:-$HOME/bloated-line}
NS="lt-home lt-cpe lt-isp lt-net"

ns() { n=$1; shift; sudo ip netns exec "$n" "$@"; }

up() {
    for n in $NS; do sudo ip netns add "$n"; ns "$n" ip link set lo up; done
    sudo ip link add h0 netns lt-home type veth peer name c0 netns lt-cpe
    sudo ip link add c1 netns lt-cpe type veth peer name i0 netns lt-isp
    sudo ip link add i1 netns lt-isp type veth peer name n0 netns lt-net
    ns lt-home ip addr add 192.168.77.2/24 dev h0
    ns lt-cpe ip addr add 192.168.77.1/24 dev c0
    ns lt-cpe ip addr add 84.1.1.2/30 dev c1
    ns lt-isp ip addr add 84.1.1.1/30 dev i0
    ns lt-isp ip addr add 203.0.113.1/24 dev i1
    ns lt-net ip addr add 203.0.113.10/24 dev n0
    ns lt-net ip addr add 1.1.1.1/32 dev lo
    ns lt-home ip link set h0 up
    ns lt-cpe ip link set c0 up
    ns lt-cpe ip link set c1 up
    ns lt-isp ip link set i0 up
    ns lt-isp ip link set i1 up
    ns lt-net ip link set n0 up
    ns lt-home ip route add default via 192.168.77.1
    ns lt-cpe ip route add default via 84.1.1.1
    ns lt-isp ip route add 192.168.77.0/24 via 84.1.1.2
    ns lt-isp ip route add default via 203.0.113.10
    ns lt-net ip route add default via 203.0.113.1
    ns lt-cpe sysctl -qw net.ipv4.ip_forward=1
    ns lt-isp sysctl -qw net.ipv4.ip_forward=1
    # Rookparcel pings with ICMP datagram sockets; a new namespace allows them to no group
    ns lt-home sysctl -qw net.ipv4.ping_group_range="0 2147483647"
    ns lt-home tc qdisc add dev h0 root netem delay 4ms
    ns lt-cpe tc qdisc add dev c0 root netem delay 4ms
    ns lt-cpe tc qdisc add dev c1 root tbf rate "$UP" burst 16kb limit "$UPBUF"
    ns lt-isp tc qdisc add dev i0 root tbf rate "$DOWN" burst 16kb limit "$DOWNBUF"
    ns lt-home ping -c 3 -i 0.2 -q 1.1.1.1
}

down() {
    for n in $NS; do sudo ip netns del "$n" 2>/dev/null || true; done
}

run() {
    dir=$1
    proto=${2:-tcp}
    phases=${3:-off:60,on:180}
    (cd "$SRC" && ~/.cargo/bin/cargo test --release -p rookparcel-core --test bloated_line --no-run 2>&1 | tail -1)
    bin=$(ls -t "$CARGO_TARGET_DIR"/release/deps/bloated_line-* | grep -v '\.d$' | head -1)
    mkdir -p "$WORK"
    tag="$dir-$proto-$(date +%H%M%S)"
    : > "$WORK/$tag.swarm.log"
    vars="TMPDIR=$WORK LINE_DIR=$dir LINE_PROTO=$proto LINE_PHASES=$phases LINE_PEERS=${PEERS:-4} LINE_MB=${MB:-320}"
    # shellcheck disable=SC2086
    ns lt-net sudo -u "$(id -un)" env $vars LINE_ROLE=swarm "$bin" --ignored --exact bloated_line --nocapture \
        > "$WORK/$tag.swarm.log" 2>&1 &
    i=0
    until grep -q "swarm ready" "$WORK/$tag.swarm.log"; do
        i=$((i + 1))
        [ "$i" -lt 300 ] || { cat "$WORK/$tag.swarm.log"; exit 1; }
        sleep 1
    done
    # shellcheck disable=SC2086
    ns lt-home sudo -u "$(id -un)" env $vars LINE_ROLE=client "$bin" --ignored --exact bloated_line --nocapture \
        2>&1 | tee "$WORK/$tag.client.log"
    for p in $(sudo ip netns pids lt-net) $(sudo ip netns pids lt-home); do sudo kill "$p" 2>/dev/null || true; done
    echo "logs: $WORK/$tag.*.log"
}

case "${1:-}" in
up) up ;;
down) down ;;
run) shift; run "$@" ;;
*) sed -n '2,7p' "$0"; exit 2 ;;
esac
