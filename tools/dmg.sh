#!/bin/sh
# Pack Recursa.app into a disk image laid out by crates/desktop/dmg/settings.py (dmgbuild, installed
# into target/dmgbuild on first use), sign it with APPLE_SIGNING_IDENTITY (ad-hoc when unset) and,
# when the App Store Connect key is in the environment, notarize and staple it.
# Usage: tools/dmg.sh <Recursa.app> <out.dmg>
set -e
cd "$(dirname "$0")/.."
app="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
case "$2" in /*) out="$2" ;; *) out="$PWD/$2" ;; esac
identity="${APPLE_SIGNING_IDENTITY:--}"

venv=target/dmgbuild
[ -x "$venv/bin/dmgbuild" ] || { python3 -m venv "$venv" && "$venv/bin/pip" install -q dmgbuild; }
mkdir -p "$(dirname "$out")"
rm -f "$out"
(cd crates/desktop/dmg && "$OLDPWD/$venv/bin/dmgbuild" -s settings.py -D app="$app" Recursa "$out")

if [ "$identity" = "-" ]; then
  codesign --force --sign - "$out"
else
  codesign --force --timestamp --sign "$identity" "$out"
  if [ -n "$APPLE_API_KEY_PATH" ]; then
    xcrun notarytool submit "$out" --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" \
      --issuer "$APPLE_API_ISSUER" --wait
    xcrun stapler staple "$out"
  fi
fi
echo "Built $out"
