#!/usr/bin/env bash
# 发版后更新落地页 https://blog.peterchen97.cn/helmp/ ：部署 site/ → 回读线上，一个入口做完。
# 每日发版巡检在 script/publish-release.sh --publish 之后直接调它；任何一步不对都非零退出。
#
#   script/update-landing.sh <版本> [--dry-run]
#
# 页面上不写版本号：下载按钮固定指向 GitHub 的 releases/latest，发版本身（安装包、latest.json 更新源、GitHub Release）
# 由 publish-release.sh 负责。这里做两件事：
#   1. 在 origin/main 最新代码的临时 worktree 里跑 script/deploy-site.sh，把 site/ 传到七牛 helmp/ 并刷 CDN
#      （发版一律基于远端主分支，本地工作树不动；--dry-run 时只列出 key，不碰七牛）
#   2. 回读：GitHub 最新 Release 是 v<版本> 且带 Helmp-<版本>.dmg、releases/latest/download/Helmp-<版本>.dmg 能下、
#      七牛更新源 helmp/releases/latest.json 是 <版本>、线上页面的下载按钮指向 releases/latest
set -euo pipefail

REPO="PeterChen1997/helmp"
BRANCH="main"
SITE_URL="https://blog.peterchen97.cn/helmp/"

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
fail() { echo "✗ $*" >&2; exit 1; }

version="${1:-}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "用法：$0 <版本，如 0.4.5> [--dry-run]" >&2; exit 2; }
dry_run=""
case "${2:-}" in
  "") ;;
  --dry-run) dry_run=--dry-run ;;
  *) echo "用法：$0 <版本> [--dry-run]" >&2; exit 2 ;;
esac

echo "==> 基于 origin/$BRANCH 最新代码部署 site/ ${dry_run}"
work="$(mktemp -d)"
trap 'git -C "$root" worktree remove --force "$work/wt" 2>/dev/null; rm -rf "$work"' EXIT
git fetch -q origin "$BRANCH"
git worktree add -q --detach "$work/wt" "origin/$BRANCH"
"$work/wt/script/deploy-site.sh" $dry_run

echo "==> 回读"
latest="$(gh api "repos/$REPO/releases/latest" -q '.tag_name, .assets[].name')"
tag="$(head -1 <<<"$latest")"
[[ "$tag" == "v$version" ]] || fail "GitHub 最新 Release 是 ${tag}，不是 v${version}（还没 publish-release.sh --publish？）"
grep -qx "Helmp-$version.dmg" <<<"$latest" || fail "v$version 的 Release 里没有 Helmp-$version.dmg"
echo "✓ GitHub 最新 Release v${version}，带 Helmp-$version.dmg"
code="$(curl -sL -o /dev/null -r 0-0 -w '%{http_code}' "https://github.com/$REPO/releases/latest/download/Helmp-$version.dmg")"
[[ "$code" == 200 || "$code" == 206 ]] || fail "releases/latest/download/Helmp-$version.dmg 下载不到（HTTP ${code}）"
echo "✓ releases/latest/download/Helmp-$version.dmg 可下载（HTTP ${code}）"
feed="$(curl -fsS "${SITE_URL}releases/latest.json" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["version"])')"
[[ "$feed" == "$version" ]] || fail "七牛更新源 latest.json 是 ${feed}，不是 $version"
echo "✓ 更新源 ${SITE_URL}releases/latest.json 是 $version"
page="$(curl -fsS "$SITE_URL")"
grep -q "github.com/$REPO/releases/latest" <<<"$page" || fail "线上页面里没有指向 releases/latest 的下载按钮"
echo "✓ 线上页面下载按钮 → https://github.com/$REPO/releases/latest"
echo "完成：$SITE_URL 的下载入口已是 $version"
