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
  (cd crates/desktop && ../../web/node_modules/.bin/tauri build)
  # Tauri notarizes and staples the app; the disk image needs its own ticket.
  if [ "$APPLE_SIGNING_IDENTITY" != "-" ] && [ -n "$APPLE_API_KEY_PATH" ]; then
    for dmg in target/release/bundle/dmg/*.dmg; do
      xcrun notarytool submit "$dmg" --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" \
        --issuer "$APPLE_API_ISSUER" --wait
      xcrun stapler staple "$dmg"
    done
  fi
  echo "Built ./target/release/bundle/macos/Recursa.app"
fi
