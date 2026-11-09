#!/usr/bin/env bash
# 把 helmp 打成 target/<profile>/Helmp.app（菜单栏工具型，LSUIElement），并做 ad-hoc 签名。
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
app="target/$profile/Helmp.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/$profile/helmp" "$app/Contents/MacOS/helmp"
cp resources/AppIcon.icns resources/menubar-template.png resources/menubar-template@2x.png "$app/Contents/Resources/"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>dev.helmp.launcher</string>
  <key>CFBundleName</key><string>Helmp</string>
  <key>CFBundleDisplayName</key><string>Helmp</string>
  <key>CFBundleExecutable</key><string>helmp</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSAppleEventsUsageDescription</key><string>Helmp 需要控制终端 App，才能跳转到对应的 Agent 会话。</string>
</dict>
</plist>
PLIST

# 签名身份要稳定：系统按签名识别 App 来记录辅助功能 / 录屏授权。ad-hoc 签名每次打包都不同，
# 授过的权限会对不上新版本。优先 HELMP_SIGN_IDENTITY，其次本机的 Developer ID、Apple Development，
# 都没有才退回 ad-hoc（并提醒）。
identity="${HELMP_SIGN_IDENTITY:-}"
if [[ -z "$identity" ]]; then
  for kind in "Developer ID Application" "Apple Development"; do
    identity="$(security find-identity -v -p codesigning | awk -v k="$kind" 'index($0, k) { print $2; exit }')"
    [[ -n "$identity" ]] && break
  done
fi
if [[ -n "$identity" ]]; then
  codesign --force --timestamp=none --sign "$identity" "$app" >/dev/null
else
  echo "警告：没有找到代码签名证书，改用 ad-hoc 签名；每次重新打包后需要重新授权。" >&2
  codesign --force --sign - "$app" >/dev/null
fi
echo "$root/$app"
