#!/usr/bin/env bash
# 自动粘贴回归检查：真的走一遍「面板里回车粘贴一条剪贴板记录 → 面板收起、焦点还回去 → 替用户按 ⌘V」。
#
# 不碰用户的系统剪贴板：Helmp 用 HELMP_PASTEBOARD 换成一个私有的命名 pasteboard；收 ⌘V 的是这里临时编译的
# 一个小 App（PasteTarget），它只记下收到的按键和私有 pasteboard 里的内容，不读系统剪贴板、不真的粘贴。
#
# 前提：先 `bash script/bundle.sh release`，且 Helmp.app 已有辅助功能权限（发按键要用）。
# 会短暂把 PasteTarget 和面板摆到前台约数秒；测试期间请别用电脑：点了别的 App，面板会因失去焦点收起、
# 检查失败（Helmp 只把 ⌘V 发给面板还焦点的那个 App，等不到就报错，不会发给你点的 App）。
# 数据目录、HOME 都是临时的。
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
app="$root/target/release/Helmp.app"
[[ -d "$app" ]] || { echo "先运行 bash script/bundle.sh release" >&2; exit 1; }

work="$(mktemp -d)"
board="helmp-paste-check-$$"
target_app="$work/PasteTarget.app"
cleanup() {
  osascript -e 'tell application id "dev.helmp.paste-target" to quit' >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

mkdir -p "$target_app/Contents/MacOS"
cat > "$work/target.swift" <<'SWIFT'
import AppKit
// 只记录：收到的按键（带不带 ⌘）、⌘V 那一刻私有 pasteboard 里的文字、自己何时成为前台。⌘V 吞掉，不读系统剪贴板。
let log = CommandLine.arguments[1]
let board = NSPasteboard(name: NSPasteboard.Name(CommandLine.arguments[2]))
func write(_ line: String) {
    let handle = FileHandle(forWritingAtPath: log)!
    handle.seekToEndOfFile()
    handle.write((line + "\n").data(using: .utf8)!)
    handle.closeFile()
}
FileManager.default.createFile(atPath: log, contents: nil)
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let window = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 360, height: 120),
                      styleMask: [.titled], backing: .buffered, defer: false)
window.title = "Helmp paste-check target"
NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: nil) { _ in
    write("active pid=\(getpid())")
}
NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
    let cmd = event.modifierFlags.contains(.command)
    write("key code=\(event.keyCode) cmd=\(cmd) key_window=\(NSApp.keyWindow != nil)")
    if cmd && event.keyCode == 9 {
        write("pasted \(board.string(forType: .string) ?? "<none>")")
        return nil
    }
    return event
}
window.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)
app.run()
SWIFT
swiftc -O "$work/target.swift" -o "$target_app/Contents/MacOS/PasteTarget" 2>/dev/null
cat > "$target_app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>dev.helmp.paste-target</string>
  <key>CFBundleExecutable</key><string>PasteTarget</string>
  <key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
PLIST

mkdir -p "$work/d" "$work/h"
echo '{"clipboard":{"enabled":true,"record_images":false,"auto_paste":true},"onboarding_done":true,"general":{"language":"en"}}' > "$work/d/config.json"
now="$(($(date +%s) * 1000))"
sqlite3 "$work/d/clipboard.sqlite" <<SQL
CREATE TABLE clips (id INTEGER PRIMARY KEY, kind TEXT NOT NULL, content TEXT NOT NULL,
  hash TEXT NOT NULL UNIQUE, width INTEGER NOT NULL DEFAULT 0, height INTEGER NOT NULL DEFAULT 0,
  source_name TEXT, source_bundle TEXT, copied_at INTEGER NOT NULL, seq INTEGER NOT NULL);
INSERT INTO clips (id, kind, content, hash, copied_at, seq) VALUES (1, 'text', 'older clip', 'h1', $((now - 2000)), 1);
INSERT INTO clips (id, kind, content, hash, copied_at, seq) VALUES (2, 'text', 'paste-check $$', 'h2', $((now - 1000)), 2);
SQL

failed=0
check() { # 名字 期望粘贴的文字 额外的 --env 参数……
  local name="$1" expect="$2"; shift 2
  # 每次都重新启动 PasteTarget，等它真的成为前台再启动 Helmp（重新激活一个已在运行的 App 可能迟到，
  # 迟到的激活会在面板开着时抢走焦点，面板随之收起）。
  osascript -e 'tell application id "dev.helmp.paste-target" to quit' >/dev/null 2>&1 || true
  sleep 0.5
  rm -f "$work/target.log" "$work/out.log"
  open -n "$target_app" --args "$work/target.log" "$board"
  for _ in $(seq 50); do grep -q "^active" "$work/target.log" 2>/dev/null && break; sleep 0.1; done
  grep -q "^active" "$work/target.log" || { echo "失败 ${name}：PasteTarget 没能成为前台 App"; failed=1; return; }
  sleep 0.5
  # -g：Helmp 在后台启动，唤起面板前的前台 App 就是 PasteTarget；面板收起后焦点要还给它。
  open -n -g -W --stdout "$work/out.log" --stderr "$work/out.log" \
    --env HELMP_SELF_TEST_KEY_WINDOW=1 --env HELMP_DATA_DIR="$work/d" --env HOME="$work/h" \
    --env HELMP_NO_WELCOME=1 --env HELMP_TRACE=1 --env HELMP_PASTEBOARD="$board" \
    --env HELMP_SELF_TEST=1 "$@" "$app"
  if grep -q "^pasted ${expect}\$" "$work/target.log"; then
    echo "通过 ${name}：⌘V 落到了唤起面板前的 App，粘贴的是「${expect}」"
  else
    echo "失败 ${name}：PasteTarget 没收到带 ⌘ 的 V（或内容不对）"; failed=1
    echo "--- PasteTarget"; cat "$work/target.log"
    echo "--- Helmp"; grep -aE "self-test|trace|失败|error" "$work/out.log" || true
  fi
}

# 回车粘贴默认选中的最近一条（双击走的是同一个 execute_selected，见 gui_double_click_on_a_clip_pastes_it）。
check "回车" "paste-check $$" --env "HELMP_SELF_TEST_EXEC=cb "
exit "$failed"
