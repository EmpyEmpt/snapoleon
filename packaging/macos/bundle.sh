#!/usr/bin/env bash
# Build Snapoleon.app from source on a Mac, sign it, and optionally run or install it.
#
#   packaging/macos/bundle.sh                      # release build, bundle in target/macos/Snapoleon.app
#   packaging/macos/bundle.sh --debug              # debug build (faster to compile)
#   packaging/macos/bundle.sh --run                # also open the built bundle where it is
#   packaging/macos/bundle.sh --install            # also replace /Applications/Snapoleon.app and launch it
#   packaging/macos/bundle.sh --reset-permissions  # also make macOS forget every privacy grant of app.snapoleon
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
run=0
reset=0
for arg in "$@"; do
  case "$arg" in
    --debug) profile=debug ;;
    --install) install=1 ;;
    --run) run=1 ;;
    --reset-permissions) reset=1 ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
installed=/Applications/Snapoleon.app

# The same floor as LSMinimumSystemVersion in Info.plist and the release build.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"

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
  # The hardened runtime as in the release build, so a local build behaves like a shipped one.
  codesign --force --options runtime --identifier app.snapoleon \
    --entitlements packaging/macos/entitlements.plist --sign - "$app"
fi
codesign --verify --strict "$app"
codesign -d -r- "$app" 2>&1 | sed -n 's/^designated => /designated requirement: /p'

# Process ids of the GUI app running from bundle $1. `snapoleon mcp` servers that agents keep open and CLI calls run
# the same executable with a subcommand; they are left alone.
gui_pids() {
  local exe="$1/Contents/MacOS/snapoleon"
  pgrep -l -f "^$exe( |\$)" 2>/dev/null | awk -v exe="$exe" '$2 == exe && ($3 == "" || $3 == "--background") { print $1 }' || true
}

# Ask the running app to quit over its own IPC, then end what is left of a GUI copy from bundle $1.
stop_app() {
  "$app/Contents/MacOS/snapoleon" quit >/dev/null 2>&1 || true
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    [[ -z "$(gui_pids "$1")" ]] && return 0
    sleep 0.5
  done
  gui_pids "$1" | xargs kill 2>/dev/null || true
  sleep 0.5
}

if [[ "$reset" == 1 ]]; then
  for service in ScreenCapture Accessibility ListenEvent Microphone; do
    tccutil reset "$service" app.snapoleon >/dev/null 2>&1 || true
  done
  # The app remembers that it asked (TCC cannot say so); without this it offers System Settings instead of Allow.
  for key in SnapoleonAskedScreenRecording SnapoleonAskedAccessibility; do
    defaults delete app.snapoleon "$key" >/dev/null 2>&1 || true
  done
  echo "reset Screen Recording, Accessibility, Input Monitoring and Microphone for app.snapoleon"
fi

if [[ "$install" == 1 ]]; then
  stop_app "$installed"
  rm -rf "$installed"
  ditto "$app" "$installed"
  # One registered app.snapoleon only: `open -b`, snapoleon:// links, notifications and the rows in System Settings
  # otherwise pick whichever copy LaunchServices saw last (this build folder, a mounted DMG).
  "$lsregister" -u "$app" >/dev/null 2>&1 || true
  "$lsregister" -u /Volumes/Snapoleon/Snapoleon.app >/dev/null 2>&1 || true
  rm -rf "$app"
  "$lsregister" -f "$installed"
  open "$installed"
  echo "installed $installed"
elif [[ "$run" == 1 ]]; then
  stop_app "$app"
  open "$app"
  echo "running $app"
else
  echo "built $app"
fi
