#!/usr/bin/env bash
# Build Iskra as a universal macOS app, for Apple silicon and Intel Macs alike:
#
#   scripts/mac-app.sh
#
# Builds the app for both (release, with Cargo.lock as it is), joins the two into one binary
# with lipo, and makes target/mac/Iskra.app of it, with the app's icon, and the line macOS
# shows when the app first asks for the microphone, which recording sound needs. The app is
# signed ad hoc, so it runs on this Mac; another Mac's Gatekeeper wants it opened with a
# right-click ▸ Open the first time, until it is signed and notarized with a Developer ID.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
# The oldest macOS it runs on, the same for both.
export MACOSX_DEPLOYMENT_TARGET=11.0
# The paths of the sources are kept in the binary, for its panic messages: the home folder
# (where cargo keeps the crates) and this checkout are written as `~` and `.` instead, so
# that the app handed out says nothing of who built it, or where.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=~ --remap-path-prefix=$ROOT=."

VERSION="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="iskra"))')"

for target in "${TARGETS[@]}"; do
  rustup target add "$target" >/dev/null
  cargo build --release --locked -p iskra --target "$target"
done

OUT="$ROOT/target/mac"
APP="$OUT/Iskra.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

lipo -create -output "$APP/Contents/MacOS/iskra" "target/${TARGETS[0]}/release/iskra" "target/${TARGETS[1]}/release/iskra"

# The icon, at every size macOS asks for, from the app's 1024 px PNG.
ICONSET="$OUT/iskra.iconset"
rm -rf "$ICONSET" && mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z $size $size apps/iskra/assets/iskra.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  sips -z $((size * 2)) $((size * 2)) apps/iskra/assets/iskra.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/iskra.icns"
rm -rf "$ICONSET"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Iskra</string>
  <key>CFBundleDisplayName</key><string>Iskra</string>
  <key>CFBundleIdentifier</key><string>org.iskraanimation.iskra</string>
  <key>CFBundleExecutable</key><string>iskra</string>
  <key>CFBundleIconFile</key><string>iskra</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>$MACOSX_DEPLOYMENT_TARGET</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.graphics-design</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Iskra records sound onto the timeline from the microphone.</string>
</dict>
</plist>
EOF

codesign --force --deep --sign - "$APP"

if strings -a "$APP/Contents/MacOS/iskra" | grep -q -F "$HOME"; then
  echo "mac-app: the binary still holds $HOME" >&2
  exit 1
fi
echo "built $APP ($(lipo -archs "$APP/Contents/MacOS/iskra"), version $VERSION)"
