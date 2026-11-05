#!/usr/bin/env bash
# 把 huan 打成 target/<profile>/Huan.app（菜单栏工具型，LSUIElement），并做 ad-hoc 签名。
# 用法：script/bundle.sh [debug|release]，默认 debug。
set -euo pipefail

profile="${1:-debug}"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

if [[ "$profile" == "release" ]]; then
  cargo build --release
else
  cargo build
fi

version="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
app="target/$profile/Huan.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "target/$profile/huan" "$app/Contents/MacOS/huan"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>dev.huan.launcher</string>
  <key>CFBundleName</key><string>Huan</string>
  <key>CFBundleDisplayName</key><string>唤</string>
  <key>CFBundleExecutable</key><string>huan</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSAppleEventsUsageDescription</key><string>唤需要控制终端 App，才能跳转到对应的 agent 会话。</string>
</dict>
</plist>
PLIST

# ad-hoc 签名：TCC 权限在每次重签后会失效，正式分发需换成 Developer ID。
codesign --force --sign - "$app" >/dev/null
echo "$root/$app"
