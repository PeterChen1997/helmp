#!/usr/bin/env bash
# 打正式发布包：Developer ID 签名（hardened runtime）→ 公证 + staple → dmg（签名、公证、staple）+ zip。
# 产物在 target/dist/：Helmp-<版本>.dmg、Helmp-<版本>.zip、SHA256SUMS.txt。
#
# 用法：script/release.sh [--dry-run]
# 公证用 App Store Connect API 密钥（与本机其他项目共用）：
#   HELMP_NOTARY_KEY_ID / HELMP_NOTARY_ISSUER / HELMP_NOTARY_KEY_PATH 可覆盖默认值。
set -euo pipefail

DRY=0
[[ "${1:-}" == "--dry-run" ]] && DRY=1

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

KEY_ID="${HELMP_NOTARY_KEY_ID:-68W68734N8}"
ISSUER="${HELMP_NOTARY_ISSUER:-20201e41-a5f0-43d7-a531-827f80cc98ee}"
KEY_PATH="${HELMP_NOTARY_KEY_PATH:-$HOME/.appstoreconnect/private_keys/AuthKey_${KEY_ID}.p8}"
NOTARY=(--key "$KEY_PATH" --key-id "$KEY_ID" --issuer "$ISSUER")

step() { printf '\n==> %s\n' "$*"; }
fail() { printf '\nRELEASE_FAILED: %s\n' "$*" >&2; exit 1; }
run() {
  if [[ $DRY == 1 ]]; then printf '   [dry-run]'; printf ' %q' "$@"; printf '\n'; else "$@"; fi
}

# notarytool --wait 在 Invalid 时退出码也可能是 0，必须看 status；不通过就打印公证日志。
notarize() {
  local file="$1" out id status
  if [[ $DRY == 1 ]]; then
    printf '   [dry-run] xcrun notarytool submit %q --wait\n' "$file"
    return
  fi
  out="$(xcrun notarytool submit "$file" "${NOTARY[@]}" --wait --output-format json 2>&1)" \
    || fail "notarytool submit 失败：${out}"
  id="$(printf '%s' "$out" | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("id",""))')"
  status="$(printf '%s' "$out" | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("status",""))')"
  if [[ "$status" != "Accepted" ]]; then
    [[ -n "$id" ]] && xcrun notarytool log "$id" "${NOTARY[@]}" >&2 || true
    fail "公证未通过（status=${status:-未知}）：$file"
  fi
  echo "   公证通过：$id"
}

for tool in cargo codesign xcrun hdiutil ditto shasum security; do
  command -v "$tool" >/dev/null || fail "缺少 $tool"
done
[[ -f "$KEY_PATH" ]] || fail "缺少 App Store Connect API 密钥：$KEY_PATH"
identity="${HELMP_SIGN_IDENTITY:-$(security find-identity -v -p codesigning | awk 'index($0, "Developer ID Application") { print $2; exit }')}"
[[ -n "$identity" ]] || fail "没有 Developer ID Application 证书，无法公证"

version="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
app="target/release/Helmp.app"
dist="target/dist"
dmg="$dist/Helmp-$version.dmg"
zip="$dist/Helmp-$version.zip"

step "构建并打包 Helmp $version"
run bash script/bundle.sh release >/dev/null

step "Developer ID 签名（hardened runtime + 时间戳）"
run codesign --force --options runtime --timestamp --entitlements resources/Helmp.entitlements \
  --sign "$identity" "$app"
run codesign --verify --strict --deep "$app"

step "公证 App 并 staple"
run mkdir -p "$dist"
run rm -f "$dmg" "$zip" "$dist/SHA256SUMS.txt"
run ditto -c -k --keepParent "$app" "$dist/notarize.zip"
notarize "$dist/notarize.zip"
run rm -f "$dist/notarize.zip"
run xcrun stapler staple "$app"

step "打 zip（staple 之后的 App）"
run ditto -c -k --keepParent "$app" "$zip"

step "打 dmg（带「应用程序」快捷方式），签名、公证、staple"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
run ditto "$app" "$stage/Helmp.app"
run ln -s /Applications "$stage/Applications"
run hdiutil create -volname "Helmp" -srcfolder "$stage" -ov -format UDZO "$dmg" >/dev/null
run codesign --force --timestamp --sign "$identity" "$dmg"
notarize "$dmg"
run xcrun stapler staple "$dmg"

step "校验"
run spctl --assess --type execute --verbose "$app"
run spctl --assess --type open --context context:primary-signature --verbose "$dmg"
if [[ $DRY == 0 ]]; then
  (cd "$dist" && shasum -a 256 "$(basename "$dmg")" "$(basename "$zip")" > SHA256SUMS.txt)
  cat "$dist/SHA256SUMS.txt"
fi
echo "完成：$dist"
