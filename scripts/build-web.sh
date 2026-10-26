#!/usr/bin/env bash
# Builds the browser demo into dist/web (index.html + wasm + assets/maps, assets/sounds).
#
#   scripts/build-web.sh            # build
#   python3 -m http.server -d dist/web 8080   # serve, then open http://localhost:8080
#
# Tooling: plain `cargo` + `wasm-bindgen-cli` (no trunk). The CLI version must equal the
# `wasm-bindgen` crate in Cargo.lock; the script checks and tells you how to install it.
# No wasm-opt step: `-Oz` shrinks the raw file ~18% but made the gzipped download larger.
set -euo pipefail
cd "$(dirname "$0")/.."

want=$(awk '/^name = "wasm-bindgen"$/ {getline; gsub(/[^0-9.]/, "", $0); print; exit}' Cargo.lock)
have=$(wasm-bindgen --version 2>/dev/null | awk '{print $2}' || true)
if [ "$have" != "$want" ]; then
  echo "need wasm-bindgen-cli $want (found: ${have:-none})" >&2
  echo "  cargo install wasm-bindgen-cli --version $want --locked" >&2
  exit 1
fi
rustup target add wasm32-unknown-unknown >/dev/null

PROFILE=${PROFILE:-web-release}
cargo build --locked --profile "$PROFILE" --target wasm32-unknown-unknown -p cp_web
target_dir=$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
wasm="$target_dir/wasm32-unknown-unknown/$PROFILE/cp_web.wasm"

out=${OUT:-dist/web}
rm -rf "$out"
mkdir -p "$out/assets"
wasm-bindgen --target web --no-typescript --out-dir "$out" --out-name cp_web "$wasm"
cp apps/web/index.html "$out/"
cp -R assets/maps "$out/assets/maps"
cp -R assets/sounds "$out/assets/sounds"

echo "built $out"
size() { wc -c < "$1" | awk '{printf "%.2f MB", $1/1048576}'; }
echo "  wasm $(size "$out/cp_web_bg.wasm"), gzipped $(gzip -9 -c "$out/cp_web_bg.wasm" | wc -c | awk '{printf "%.2f MB", $1/1048576}'), js $(size "$out/cp_web.js")"
