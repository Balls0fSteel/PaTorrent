#!/bin/bash
# build.sh SLICE [--release] (on the Mac VM): under the VM lock, builds rookparcel-mac from
# ~/rp/SLICE/src on one core (target folder ~/rp/target, shared by the slices) and makes a dev
# Rookparcel.app in ~/rp/SLICE/out with macos_bundle.py native, then checks its signature.
set -euo pipefail

SLICE=${1:?usage: build.sh SLICE [--release]}
PROFILE=debug
[ "${2:-}" = "--release" ] && PROFILE=release
. "$(dirname "$0")/lib.sh"
SRC="$RP/$SLICE/src"
OUT="$RP/$SLICE/out"

lock "build $SLICE"
cd "$SRC"
flags=(-j 1 -p rookparcel-mac)
[ "$PROFILE" = release ] && flags+=(--release)
start=$(date +%s)
CARGO_TARGET_DIR="$RP/target" MACOSX_DEPLOYMENT_TARGET=11.0 nice -n 19 cargo build "${flags[@]}"
echo "build: $PROFILE in $(( $(date +%s) - start )) s"

family=$(tr -d '[:space:]' < assets/patina-icons/app-icon.txt)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
rm -rf "$OUT"
mkdir -p "$OUT"
python3 .github/scripts/macos_bundle.py native \
    --binary "$RP/target/$PROFILE/rookparcel-mac" \
    --icon-dir "assets/patina-icons/exports/app/$family" \
    --app-id io.github.balls0fsteel.Rookparcel \
    --version "$version-dev.$SLICE" \
    --extra LICENSE.md --extra THIRD-PARTY-NOTICES.md \
    --out "$OUT/Rookparcel.app"
codesign --verify --verbose=2 "$OUT/Rookparcel.app"
plutil -lint "$OUT/Rookparcel.app/Contents/Info.plist"
du -sh "$OUT/Rookparcel.app"
