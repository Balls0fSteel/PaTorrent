#!/usr/bin/env bash
# push.sh SLICE (Git Bash on Windows, or any shell with git, tar, ssh and scp): copies this
# checkout's files (tracked ones and new ones not ignored, uncommitted changes included; not
# docs/, site/, third_party/) to the Mac VM into ~/rp/SLICE/src, replacing what was there.
#
#   RP_VM   the VM (default x@192.168.1.128)
#   RP_KEY  the ssh key (default ~/.ssh/patorrent_vm)
set -euo pipefail

SLICE=${1:?usage: push.sh SLICE}
case "$SLICE" in *[!A-Za-z0-9_-]*) echo "push.sh: SLICE is letters, digits, - and _" >&2; exit 2;; esac
VM=${RP_VM:-x@192.168.1.128}
KEY=${RP_KEY:-$HOME/.ssh/patorrent_vm}
SSH=(ssh -i "$KEY" -o BatchMode=yes)

cd "$(git rev-parse --show-toplevel)"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# files deleted in the working tree are still in the index: only those that exist
git ls-files --cached --others --exclude-standard \
    | grep -v -E '^(docs|site|third_party)/' \
    | while IFS= read -r f; do [ -f "$f" ] && printf '%s\n' "$f"; done > "$tmp/files"
tar -czf "$tmp/src.tgz" -T "$tmp/files"
echo "push: $(wc -l < "$tmp/files") files, $(du -h "$tmp/src.tgz" | cut -f1)"
scp -q -i "$KEY" -o BatchMode=yes "$tmp/src.tgz" "$VM:rp-push-$SLICE.tgz"
# Windows checkouts have CRLF line ends: the scripts the VM runs get LF back
"${SSH[@]}" "$VM" "set -e; mkdir -p ~/rp/$SLICE; rm -rf ~/rp/$SLICE/src; mkdir ~/rp/$SLICE/src; \
    tar -xzf ~/rp-push-$SLICE.tgz -C ~/rp/$SLICE/src; rm ~/rp-push-$SLICE.tgz; \
    find ~/rp/$SLICE/src/tools/mac-vm ~/rp/$SLICE/src/.github/scripts -type f \\( -name '*.sh' -o -name '*.py' \\) \
        -exec perl -pi -e 's/\\r\$//' {} +; chmod +x ~/rp/$SLICE/src/tools/mac-vm/*.sh; \
    echo \"push: ~/rp/$SLICE/src ready\""
