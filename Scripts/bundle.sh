#!/bin/zsh
# Builds ReviewBuddy.app into ./build. Usage: Scripts/bundle.sh [debug|release]
set -euo pipefail
cd "$(dirname "$0")/.."
CONFIG="${1:-release}"
if [ "$CONFIG" = "release" ]; then
  cargo build --release -p review-buddy
  BIN="target/release/review-buddy"
else
  cargo build -p review-buddy
  BIN="target/debug/review-buddy"
fi
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
APP="build/ReviewBuddy.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/ReviewBuddy"
cp Resources/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"   # regenerate: swift Scripts/make-icon.swift
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>dev.reviewbuddy.app</string>
  <key>CFBundleName</key><string>Review Buddy</string>
  <key>CFBundleDisplayName</key><string>Review Buddy</string>
  <key>CFBundleExecutable</key><string>ReviewBuddy</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>3</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict>
</plist>
PLIST
codesign --force --sign - "$APP" >/dev/null 2>&1 || true
echo "$APP"
