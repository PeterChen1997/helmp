#!/usr/bin/env bash
# 发布更新：为 script/release.sh 打好的包生成更新清单 latest.json（格式见 docs/updates.md），
# 上传到七牛和 GitHub Releases，让已安装的 Helmp 能检查到新版本。
#
# 用法：
#   script/publish-release.sh             只在 target/dist/ 生成 latest.json 和 <版本>.json，不碰网络
#   script/publish-release.sh --dry-run   生成，并列出将要上传的 key、要刷新的 URL 和 gh 命令
#   script/publish-release.sh --publish   生成并上传：七牛 → 刷新 CDN → GitHub Releases
#
# 输入：$HELMP_DIST（默认 target/dist）里的 Helmp-<版本>.zip 和 Helmp-<版本>.dmg（版本取自 Cargo.toml）。
# 更新说明取 CHANGELOG.md 里 `## [<版本>]` 一节：`### 简体中文` 之前是英文，之后是中文。
#
# 七牛的 key：
#   helmp/releases/<版本>/Helmp-<版本>.zip|dmg   安装包（按版本分目录，不覆盖旧版本）
#   helmp/releases/<版本>.json                   这一版的清单快照
#   helmp/releases/latest.json                   更新源（Helmp 读的就是它）
# 先传安装包、最后传 latest.json：客户端不会读到指向还不存在的文件的清单。
#
# 安全约束同 deploy-site.sh：这个 bucket 也是博客根目录，所有 key 必须以 helmp/ 开头，否则整体中止。
set -euo pipefail

QSHELL="${QSHELL:-$HOME/Downloads/qshell}"
BUCKET="peter-blog"
KEY_PREFIX="helmp/"
CDN="https://blog.peterchen97.cn/"
REPO="PeterChen1997/helmp"

mode=generate
case "${1:-}" in
  "") ;;
  --dry-run) mode=dry-run ;;
  --publish) mode=publish ;;
  *) echo "用法：$0 [--dry-run | --publish]" >&2; exit 2 ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
dist="${HELMP_DIST:-$root/target/dist}"

fail() { echo "中止：$*" >&2; exit 1; }

[[ "$KEY_PREFIX" == "helmp/" ]] || fail "KEY_PREFIX 必须是 helmp/，当前是 '$KEY_PREFIX'"

version="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]] || fail "Cargo.toml 里的版本号不对：'$version'"
tag="v$version"
zip="$dist/Helmp-$version.zip"
dmg="$dist/Helmp-$version.dmg"
for file in "$zip" "$dmg"; do
  [[ -s "$file" ]] || fail "找不到 ${file}（先运行 script/release.sh）"
done
# 最低系统版本与 bundle.sh 写进 Info.plist 的一致。
minimum_macos="$(grep -o 'LSMinimumSystemVersion</key><string>[^<]*' script/bundle.sh | sed 's/.*<string>//')"
[[ -n "$minimum_macos" ]] || fail "没能从 script/bundle.sh 读出 LSMinimumSystemVersion"

release_dir="releases/$version"
qiniu_base="${CDN}${KEY_PREFIX}${release_dir}/"
github_base="https://github.com/$REPO/releases/download/$tag/"
latest="$dist/latest.json"
snapshot="$dist/$version.json"

echo "==> 生成更新清单 $version"
python3 -I - "$version" "$minimum_macos" "$qiniu_base" "$github_base" "$latest" "$snapshot" "$zip" "$dmg" <<'PY'
import datetime, hashlib, json, os, re, sys

version, minimum, qiniu, github, latest, snapshot, *files = sys.argv[1:]

text = open("CHANGELOG.md", encoding="utf-8").read()
match = re.search(r"^## \[" + re.escape(version) + r"\][^\n]*\n(.*?)(?=^## \[|\Z)", text, re.S | re.M)
if not match:
    sys.exit(f"中止：CHANGELOG.md 里没有 ## [{version}] 一节，更新说明不能为空")
section = match.group(1)
if "### 简体中文" not in section:
    sys.exit(f"中止：CHANGELOG.md 的 {version} 一节缺少 ### 简体中文")
en, zh = (part.strip() for part in section.split("### 简体中文", 1))
if not en or not zh:
    sys.exit(f"中止：CHANGELOG.md 的 {version} 一节英文或中文为空")
# 中文部分的小标题是 ####，去掉一级与英文一致。
zh = re.sub(r"^####", "###", zh, flags=re.M)

assets = []
for path in files:
    name = os.path.basename(path)
    data = open(path, "rb").read()
    assets.append({
        "name": name,
        "kind": name.rsplit(".", 1)[1],
        "size": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
        # 顺序就是客户端尝试的顺序：七牛（国内直连）在前，GitHub 在后。
        "urls": [qiniu + name, github + name],
    })

manifest = {
    "version": version,
    "published_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "notes": {"en": en, "zh-Hans": zh},
    "minimum_macos": minimum,
    "assets": assets,
}
for out in (latest, snapshot):
    with open(out, "w", encoding="utf-8") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=2)
        f.write("\n")
for asset in assets:
    print(f"    {asset['name']}  {asset['size']} 字节  sha256 {asset['sha256']}")
PY
echo "    $latest"
echo "    $snapshot"

# 上传顺序：安装包 → 快照 → latest.json。
uploads=(
  "${KEY_PREFIX}${release_dir}/Helmp-$version.zip=$zip"
  "${KEY_PREFIX}${release_dir}/Helmp-$version.dmg=$dmg"
  "${KEY_PREFIX}releases/$version.json=$snapshot"
  "${KEY_PREFIX}releases/latest.json=$latest"
)
for entry in "${uploads[@]}"; do
  key="${entry%%=*}"
  [[ "$key" == helmp/?* && "$key" != *..* ]] || fail "不安全的 key：$key"
done
# 安装包和快照按版本分 key、从不覆盖，只有 latest.json 需要刷新 CDN。
refresh_url="${CDN}${KEY_PREFIX}releases/latest.json"
gh_assets=("$zip" "$dmg" "$latest")

if [[ $mode == generate ]]; then
  echo "==> 只生成了清单；--dry-run 查看上传计划，--publish 上传。"
  exit 0
fi

echo "==> 将上传到七牛 bucket=${BUCKET}（按顺序）："
for entry in "${uploads[@]}"; do
  echo "    ${entry%%=*}  ←  ${entry#*=}"
done
echo "==> 将刷新的 URL："
echo "    $refresh_url"
echo "==> 将附加到 GitHub Release ${tag}（${REPO}）："
for file in "${gh_assets[@]}"; do
  echo "    $(basename "$file")"
done

if [[ $mode == dry-run ]]; then
  echo "==> --dry-run：没有上传，也没有刷新。"
  exit 0
fi

[[ -x "$QSHELL" ]] || fail "找不到可执行的 qshell：${QSHELL}（可用环境变量 QSHELL 指定）"
command -v gh >/dev/null || fail "缺少 gh"

echo "==> 上传到七牛..."
for entry in "${uploads[@]}"; do
  key="${entry%%=*}"
  file="${entry#*=}"
  "$QSHELL" rput --overwrite "$BUCKET" "$key" "$file"
done

echo "==> 刷新 CDN..."
refresh_list="$(mktemp -t helmp-refresh)"
trap 'rm -f "$refresh_list"' EXIT
echo "$refresh_url" > "$refresh_list"
"$QSHELL" cdnrefresh -i "$refresh_list"

echo "==> GitHub Release $tag..."
if ! gh release view "$tag" --repo "$REPO" >/dev/null 2>&1; then
  notes_file="$(mktemp -t helmp-notes)"
  python3 -I -c 'import json,sys; n=json.load(open(sys.argv[1]))["notes"]; print(n["en"] + "\n\n---\n\n" + n["zh-Hans"])' "$latest" > "$notes_file"
  gh release create "$tag" --repo "$REPO" --title "Helmp $version" --notes-file "$notes_file"
  rm -f "$notes_file"
fi
# latest.json 最后传：releases/latest/download/latest.json 就是 Helmp 的第二个更新源。
gh release upload "$tag" "$zip" "$dmg" --repo "$REPO" --clobber
gh release upload "$tag" "$latest" --repo "$REPO" --clobber

echo "==> 完成：$refresh_url"
