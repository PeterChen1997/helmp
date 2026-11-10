#!/usr/bin/env bash
# 输入法回归检查：用真实 HID 按键在面板里打字，核对中英文输入法都能用。
#
# 依次切到系统拼音、豆包（若已启用）、ABC，在面板搜索框里打 nihao+空格 / wx，看搜索框的结果；
# 再在 flomo 表单里用系统拼音打一次。结束后恢复原来的输入法。
#
# 前提：先 `bash script/bundle.sh release`，且 Helmp.app 已有辅助功能权限（发按键要用）。
# 会短暂唤起面板抢焦点约数秒；面板不是 key window 时绝不发键（不会打进别的 App）。
# 数据目录、HOME 都是临时的，剪贴板采集关闭。
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
app="$root/target/release/Helmp.app"
[[ -d "$app" ]] || { echo "先运行 bash script/bundle.sh release" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cat > "$work/tis.swift" <<'SWIFT'
import Carbon
let a = CommandLine.arguments
func sid(_ s: TISInputSource) -> String {
    Unmanaged<CFString>.fromOpaque(TISGetInputSourceProperty(s, kTISPropertyInputSourceID)!).takeUnretainedValue() as String
}
if a[1] == "current" { print(sid(TISCopyCurrentKeyboardInputSource().takeRetainedValue())) }
else {
    let f = [kTISPropertyInputSourceID as String: a[2]] as CFDictionary
    guard let l = TISCreateInputSourceList(f, false)?.takeRetainedValue() as? [TISInputSource], let s = l.first else { exit(2) }
    exit(TISSelectInputSource(s) == noErr ? 0 : 3)
}
SWIFT
swiftc -O "$work/tis.swift" -o "$work/tis" 2>/dev/null
original="$("$work/tis" current)"
trap '"$work/tis" select "$original" >/dev/null 2>&1 || true; rm -rf "$work"' EXIT

failed=0
run() { # 输入源 查询 按键 期望（正则）
  local source="$1" query="$2" keys="$3" expect="$4"
  if ! "$work/tis" select "${source}" 2>/dev/null; then echo "跳过 ${source}（未启用）"; return; fi
  rm -rf "$work/d" "$work/h"; mkdir -p "$work/d" "$work/h"
  echo '{"clipboard":{"enabled":false},"onboarding_done":true}' > "$work/d/config.json"
  open -n -W --stdout "$work/out.log" --stderr "$work/out.log" \
    --env HELMP_SELF_TEST_KEY_WINDOW=1 --env HELMP_DATA_DIR="$work/d" --env HOME="$work/h" \
    --env "HELMP_SELF_TEST=$query" --env "HELMP_SELF_TEST_KEYS=$keys" "$app"
  local last
  last="$(grep -a "self-test key" "$work/out.log" | tail -1 || true)"
  if grep -aq "不是 key" "$work/out.log"; then
    echo "失败 ${source}：面板没拿到焦点（测试期间请别点别处）"; failed=1
  elif [[ "${last}" =~ $expect ]]; then
    echo "通过 ${source}：${last}"
  else
    echo "失败 ${source}：${last}"; failed=1
  fi
}

run com.apple.inputmethod.SCIM.ITABC 1 "native:nihao native:_" 'query="你好"'
run com.bytedance.inputmethod.doubaoime.pinyin 1 "native:nihao native:_" 'query="你'
run com.apple.keylayout.ABC 1 "native:wx" 'query="wx"'
run com.apple.inputmethod.SCIM.ITABC flomo "tab native:nihao native:_" '"content", "你好"'
exit "$failed"
