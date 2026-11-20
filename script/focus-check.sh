#!/usr/bin/env bash
# 面板焦点回归检查（CARM-340）：反复叫出面板 → 用真实 HID 按键打一个字 → 收起，核对每次字都进了面板。
#
# 每轮打印焦点状态（key / 激活 / 输入上下文 / 是否退回了非激活面板），并数一下这段时间里 WindowServer
# 拒绝 Helmp 到前台（`Denying Helmp the right to be in front`）的次数。屏幕锁着时一定全失败，先解锁。
#
# 用法：bash script/focus-check.sh [轮数=30] [clipboard|toggle]
# 前提：先 `bash script/bundle.sh release`，且 Helmp.app 已有辅助功能权限（发按键要用）。
# 会反复抢键盘约 轮数×1.5 秒，期间别动键盘鼠标。数据目录、HOME 都是临时的，剪贴板采集关闭。
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
app="$root/target/release/Helmp.app"
[[ -d "$app" ]] || { echo "先运行 bash script/bundle.sh release" >&2; exit 1; }
rounds="${1:-30}" command="${2:-clipboard}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/d" "$work/h"
echo '{"clipboard":{"enabled":false},"onboarding_done":true}' > "$work/d/config.json"

started="$(date '+%Y-%m-%d %H:%M:%S')"
open -n -W --stdout "$work/out.log" --stderr "$work/out.log" \
  --env HELMP_SELF_TEST=1 --env HELMP_SELF_TEST_KEY_WINDOW=1 --env HELMP_TRACE=1 \
  --env HELMP_DATA_DIR="$work/d" --env HOME="$work/h" \
  --env HELMP_SELF_TEST_FOCUS_LOOP="$rounds" --env HELMP_SELF_TEST_FOCUS_COMMAND="$command" "$app"
grep -a "self-test focus" "$work/out.log"
denied="$(/usr/bin/log show --start "$started" --style compact \
  --predicate 'process == "WindowServer" AND eventMessage CONTAINS "Denying Helmp"' 2>/dev/null | grep -c Denying || true)"
echo "这段时间系统拒绝 Helmp 到前台 ${denied} 次"
if [[ -n "${FOCUS_CHECK_LOG:-}" ]]; then cp "$work/out.log" "$FOCUS_CHECK_LOG"; fi
summary="$(grep -a 'self-test focus summary' "$work/out.log" || true)"
[[ "$summary" =~ ([0-9]+)/([0-9]+)\ ok ]] && [[ "${BASH_REMATCH[1]}" == "${BASH_REMATCH[2]}" ]]
