#!/bin/zsh
# Builds ReviewBuddy.app into ./build. Usage: Scripts/bundle.sh [debug|release]
set -euo pipefail
cd "$(dirname "$0")/.."
CONFIG="${1:-release}"
swift build -c "$CONFIG" --product ReviewBuddy
BIN="$(swift build -c "$CONFIG" --show-bin-path)/ReviewBuddy"
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
  <key>CFBundleShortVersionString</key><string>0.2.0</string>
  <key>CFBundleVersion</key><string>2</string>
  <key>LSMinimumSystemVersion</key><string>26.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict>
</plist>
PLIST
codesign --force --sign - "$APP" >/dev/null 2>&1 || true
echo "$APP"
