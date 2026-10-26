#!/usr/bin/env bash
# Build Snapoleon.app from source on a Mac, sign it, and optionally install it.
#
#   packaging/macos/bundle.sh                  # release build, bundle in target/macos/Snapoleon.app
#   packaging/macos/bundle.sh --debug          # debug build (faster to compile)
#   packaging/macos/bundle.sh --install        # also replace /Applications/Snapoleon.app and launch it
#
# Signing decides whether macOS remembers the privacy permissions (Screen Recording, Accessibility) across
# rebuilds. TCC keys a grant to the app's designated requirement:
#   - a real certificate (Developer ID, or the local one from make-signing-identity.sh) gives
#     `identifier "app.snapoleon" and certificate leaf = ...`, which stays the same for every build;
#   - ad-hoc signing gives a requirement on the code hash, so every rebuild is a new app to TCC and the
#     switch in System Settings stops working until it is removed and granted again.
# The identity is $SNAPOLEON_SIGN_IDENTITY, else "Snapoleon Local Signing" when it exists, else ad-hoc.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
profile=release
install=0
for arg in "$@"; do
  case "$arg" in
    --debug) profile=debug ;;
    --install) install=1 ;;
    -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

cd "$root"
if [[ "$profile" == release ]]; then
  cargo build --release -p snapoleon
else
  cargo build -p snapoleon
fi

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
app="$root/target/macos/Snapoleon.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/$profile/snapoleon" "$app/Contents/MacOS/snapoleon"
cp packaging/macos/snapoleon.icns "$app/Contents/Resources/snapoleon.icns"

plist="$app/Contents/Info.plist"
cp packaging/macos/Info.plist "$plist"
pb() { /usr/libexec/PlistBuddy -c "$1" "$plist" >/dev/null 2>&1 || true; }
set_key() { pb "Delete :$1"; pb "Add :$1 string $2"; }
set_key CFBundleIdentifier app.snapoleon
set_key CFBundleName Snapoleon
set_key CFBundleDisplayName Snapoleon
set_key CFBundleExecutable snapoleon
set_key CFBundleIconFile snapoleon.icns
set_key CFBundlePackageType APPL
set_key CFBundleInfoDictionaryVersion 6.0
set_key CFBundleShortVersionString "$version"
set_key CFBundleVersion "$version"

identity="${SNAPOLEON_SIGN_IDENTITY:-}"
if [[ -z "$identity" ]] && security find-identity -v -p codesigning 2>/dev/null | grep -q '"Snapoleon Local Signing"'; then
  identity="Snapoleon Local Signing"
fi
if [[ -n "$identity" ]]; then
  echo "signing with \"$identity\""
  codesign --force --options runtime --timestamp=none --identifier app.snapoleon \
    --entitlements packaging/macos/entitlements.plist --sign "$identity" "$app"
else
  echo "signing ad-hoc: permissions will have to be granted again after every rebuild" >&2
  echo "(run packaging/macos/make-signing-identity.sh once to avoid that)" >&2
  codesign --force --identifier app.snapoleon --entitlements packaging/macos/entitlements.plist --sign - "$app"
fi
codesign --verify --strict "$app"
codesign -d -r- "$app" 2>&1 | sed -n 's/^designated => /designated requirement: /p'

if [[ "$install" == 1 ]]; then
  osascript -e 'tell application id "app.snapoleon" to quit' >/dev/null 2>&1 || true
  pkill -x snapoleon >/dev/null 2>&1 || true
  sleep 0.5
  rm -rf /Applications/Snapoleon.app
  ditto "$app" /Applications/Snapoleon.app
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f /Applications/Snapoleon.app
  open /Applications/Snapoleon.app
  echo "installed /Applications/Snapoleon.app"
else
  echo "built $app"
fi
