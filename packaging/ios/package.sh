#!/usr/bin/env bash
# Build CADCraft for iPadOS 16+ and package it as a TrollStore .tipa (an IPA, ad-hoc signed;
# TrollStore applies its own signature on install).
#
# Needs: Xcode (iPhoneOS SDK), rustup with the aarch64-apple-ios target.
#
#   packaging/ios/package.sh               -> dist/CADCraft-<version>-ios16.tipa
#   packaging/ios/package.sh --simulator   -> dist/simulator/CADCraft.app (Apple silicon Macs)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"
export IPHONEOS_DEPLOYMENT_TARGET=16.0
TARGET=aarch64-apple-ios
SIMULATOR=0
if [ "${1:-}" = "--simulator" ]; then
  TARGET=aarch64-apple-ios-sim
  SIMULATOR=1
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
SHORT_VERSION="${VERSION%%-*}"
BUILD_SHA="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"

rustup target add "$TARGET" >/dev/null
cargo build --release --locked --target "$TARGET" -p cadcraft

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
APP="$STAGE/Payload/CADCraft.app"
mkdir -p "$APP"
cp "target/$TARGET/release/cadcraft" "$APP/CADCraft"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/$SHORT_VERSION/g" -e "s/@BUILD_SHA@/$BUILD_SHA/g" \
  packaging/ios/Info.plist.in >"$APP/Info.plist"
plutil -lint "$APP/Info.plist" >/dev/null
plutil -convert binary1 "$APP/Info.plist"
printf 'APPL????' >"$APP/PkgInfo"

# Loose iPad icons (no asset catalog): 76 pt and 83.5 pt at 2x.
ICON=assets/app-icon/cadcraft-1024.png
sips -z 152 152 "$ICON" --out "$APP/AppIcon76x76@2x~ipad.png" >/dev/null
sips -z 167 167 "$ICON" --out "$APP/AppIcon83.5x83.5@2x~ipad.png" >/dev/null
sips -z 76 76 "$ICON" --out "$APP/AppIcon76x76~ipad.png" >/dev/null

# Licences ship with the app.
cp LICENSE-MIT LICENSE-APACHE NOTICE ATTRIBUTION.md "$APP/"

strip -x "$APP/CADCraft" 2>/dev/null || true

if [ "$SIMULATOR" = 1 ]; then
  # Simulators of iOS 27 and later stop apps linked against their SDK that don't adopt the UIScene
  # life cycle, which winit 0.30 predates: the binary says it was linked against iOS 18 instead.
  # Only the simulator build: iOS 16 on a device doesn't make that check.
  xcrun vtool -set-build-version iossim 16.0 18.0 -replace -output "$APP/CADCraft.tmp" "$APP/CADCraft"
  mv -f "$APP/CADCraft.tmp" "$APP/CADCraft"
  codesign --force --sign - --timestamp=none "$APP"
  rm -rf dist/simulator && mkdir -p dist/simulator
  cp -R "$APP" dist/simulator/
  echo "$ROOT/dist/simulator/CADCraft.app"
  exit 0
fi
codesign --force --sign - --timestamp=none --entitlements packaging/ios/entitlements.plist "$APP"

mkdir -p dist
OUT="$ROOT/dist/CADCraft-$VERSION-ios16.tipa"
rm -f "$OUT"
(cd "$STAGE" && zip -qry "$OUT" Payload)
echo "$OUT"
