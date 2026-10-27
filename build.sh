#!/bin/sh
# Build the interface, then the release CLI that embeds it. `./build.sh app` also bundles
# Recursa.app and a DMG into target/release/bundle/, signed and notarized when the Developer ID
# identity and App Store Connect key are in the environment (APPLE_SIGNING_IDENTITY, APPLE_API_KEY,
# APPLE_API_ISSUER, APPLE_API_KEY_PATH; read from $RECURSA_SIGNING_ENV, default
# ~/.private_keys/recursa-signing.env, when that file exists), ad-hoc signed otherwise.
set -e
cd "$(dirname "$0")"
(cd web && { [ -d node_modules ] || npm ci; })
# Third-party notices ship inside both binaries; refresh them in case dependencies changed.
python3 -I tools/notices.py
(cd web && npm run build)
cargo build --release
echo "Built ./target/release/recursa"
if [ "$1" = "app" ]; then
  signing="${RECURSA_SIGNING_ENV:-$HOME/.private_keys/recursa-signing.env}"
  [ -f "$signing" ] && . "$signing"
  export APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:--}"
  # Tauri signs, notarizes and staples the app; tools/dmg.sh lays out and signs the disk image.
  (cd crates/desktop && ../../web/node_modules/.bin/tauri build --bundles app)
  version=$(node -p "require('./web/package.json').version")
  case "$(uname -m)" in arm64) arch=aarch64 ;; *) arch=x64 ;; esac
  tools/dmg.sh target/release/bundle/macos/Recursa.app "target/release/bundle/dmg/Recursa_${version}_${arch}.dmg"
  echo "Built ./target/release/bundle/macos/Recursa.app"
fi
