#!/bin/sh
# Build the iOS app as an App Store Connect .ipa (Xcode automatic signing with the team in
# crates/desktop/gen/apple/project.yml) and, with "upload", send it to App Store Connect using the
# API key in ~/.private_keys (APPLE_API_KEY and APPLE_API_ISSUER from the signing env).
# Usage: tools/ios.sh [upload]
set -e
cd "$(dirname "$0")/.."
export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
# Homebrew's rust formula has no iOS std; the Xcode build phase must find rustup's cargo first.
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
(cd crates/desktop && ../../web/node_modules/.bin/tauri ios build --export-method app-store-connect)
ipa=$(ls crates/desktop/gen/apple/build/arm64/*.ipa)
if [ "$1" = "upload" ]; then
  xcrun altool --upload-app -f "$ipa" -t ios --apiKey "$APPLE_API_KEY" --apiIssuer "$APPLE_API_ISSUER"
fi
echo "Built $ipa"
