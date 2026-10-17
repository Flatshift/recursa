#!/bin/sh
# Build the interface, then the release CLI that embeds it. `./build.sh app` also bundles
# Recursa.app and a DMG (ad-hoc signed) into target/release/bundle/.
set -e
cd "$(dirname "$0")"
(cd web && { [ -d node_modules ] || npm ci; })
# Third-party notices ship inside both binaries; refresh them in case dependencies changed.
python3 -I tools/notices.py
(cd web && npm run build)
cargo build --release
echo "Built ./target/release/recursa"
if [ "$1" = "app" ]; then
  (cd crates/desktop && ../../web/node_modules/.bin/tauri build)
  echo "Built ./target/release/bundle/macos/Recursa.app"
fi
