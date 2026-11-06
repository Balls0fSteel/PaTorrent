#!/usr/bin/env bash
# pull.sh SLICE DIR (Git Bash on Windows): copies ~/rp/SLICE/shots from the Mac VM (the
# snapshots, windows.txt, the screen pictures, app.log, script.log) into DIR.
#
#   RP_VM   the VM (default x@192.168.1.128)
#   RP_KEY  the ssh key (default ~/.ssh/patorrent_vm)
set -euo pipefail

SLICE=${1:?usage: pull.sh SLICE DIR}
DIR=${2:?usage: pull.sh SLICE DIR}
case "$SLICE" in *[!A-Za-z0-9_-]*) echo "pull.sh: SLICE is letters, digits, - and _" >&2; exit 2;; esac
VM=${RP_VM:-x@192.168.1.128}
KEY=${RP_KEY:-$HOME/.ssh/patorrent_vm}

mkdir -p "$DIR"
scp -q -r -i "$KEY" -o BatchMode=yes "$VM:rp/$SLICE/shots/." "$DIR/"
ls -l "$DIR"
