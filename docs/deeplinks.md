# Deep links / 深链接

[English](#english) · [简体中文](#简体中文)

## English

Helmp registers the `helmp://` URL scheme, so scripts, Shortcuts, Alfred or Raycast workflows and other launchers can drive it. Open a link with `open` in a terminal, the "Open URLs" action in Shortcuts, or anything else that opens URLs.

```bash
open "helmp://show?q=wx"
```

The scheme is declared in the `Info.plist` that `script/bundle.sh` writes, so links work with the packaged `Helmp.app` only, not with `cargo run`. If Helmp isn't running, macOS launches it and the link is handled as soon as it starts.

### Links

| Link | What it does |
|---|---|
| `helmp://show` | Show the panel. `helmp://` on its own does the same |
| `helmp://show?q=<text>` | Show the panel with `<text>` in the search bar, as if you typed it |
| `helmp://scope/<id>` | Show the panel inside an extension |
| `helmp://scope/<id>?q=<text>` | Same, with `<text>` as the initial input |
| `helmp://settings` | Open the settings window |
| `helmp://settings/<page>` | Open the settings window on one page |
| `helmp://hide` | Hide the panel |

Extension ids for `scope/`:

| id | Extension |
|---|---|
| `translate` | Translate |
| `clipboard` | Clipboard history |
| `agents` | Agent sessions |

Settings pages (case-insensitive): `general`, `extensions`, `models`, `agents`, `clipboard`, `screenshot`, `appearance`, `about`.

### Details

- `q` is URL-decoded: `%20` and `+` both become a space, and non-ASCII text should be percent-encoded (`helmp://scope/translate?q=%E4%BD%A0%E5%A5%BD`). An empty `q` is ignored. Other query parameters are ignored.
- `helmp://scope/translate` without `q` behaves like the Translate hotkey: it picks up the text selected in the front app (see [privacy.md](privacy.md#what-helmp-reads-but-doesnt-store) for how). With `q`, that text is translated instead and nothing is read from the front app.
- If the extension is turned off in Settings › Extensions, or the id doesn't exist, the panel still opens and Helmp prints `helmp: 没有名为 <id> 的范围（扩展可能被停用）` to stderr.
- A link Helmp doesn't understand (unknown path, unknown settings page) is ignored and reported on stderr as `helmp: 不认识的链接 <url>`. Run Helmp from a terminal (`target/release/Helmp.app/Contents/MacOS/helmp`) to see these messages.
- Deep links never send anything over the network.

### Examples

```bash
# Translate the clipboard
open "helmp://scope/translate?q=$(pbpaste | python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.stdin.read()))')"

# Search the clipboard history for "invoice"
open "helmp://scope/clipboard?q=invoice"

# Jump to the model settings
open "helmp://settings/models"
```

## 简体中文

Helmp 注册了 `helmp://` 链接协议，脚本、快捷指令、Alfred / Raycast 工作流或别的启动器都能用它来调出 Helmp。在终端里用 `open`、在快捷指令里用「打开 URL」，或者任何能打开链接的方式都行。

```bash
open "helmp://show?q=wx"
```

这个协议写在 `script/bundle.sh` 生成的 `Info.plist` 里，所以只有打包后的 `Helmp.app` 能收到链接，`cargo run` 起来的进程收不到。Helmp 没在运行时，系统会先启动它，启动完成后立即处理这个链接。

### 链接一览

| 链接 | 作用 |
|---|---|
| `helmp://show` | 唤起面板。只写 `helmp://` 效果相同 |
| `helmp://show?q=<文字>` | 唤起面板，并把 `<文字>` 填进搜索框，和手动输入一样 |
| `helmp://scope/<id>` | 唤起面板并进入某个扩展 |
| `helmp://scope/<id>?q=<文字>` | 同上，`<文字>` 作为扩展里的初始输入 |
| `helmp://settings` | 打开设置窗口 |
| `helmp://settings/<页面>` | 打开设置窗口的某一页 |
| `helmp://hide` | 收起面板 |

`scope/` 可用的扩展 id：

| id | 扩展 |
|---|---|
| `translate` | 翻译 |
| `clipboard` | 剪贴板历史 |
| `agents` | Agent 会话 |

设置页面（不区分大小写）：`general`（通用）、`extensions`（扩展）、`models`（模型）、`agents`（Agent）、`clipboard`（剪贴板）、`screenshot`（截图）、`appearance`（外观）、`about`（关于）。

### 细节

- `q` 会做 URL 解码：`%20` 和 `+` 都变成空格；中文等非 ASCII 文字请先百分号编码（`helmp://scope/translate?q=%E4%BD%A0%E5%A5%BD`）。`q` 为空时忽略，其他参数也一律忽略。
- 不带 `q` 的 `helmp://scope/translate` 和翻译快捷键一样，会读取前台 App 里选中的文字（怎么读的见 [privacy.zh-CN.md](privacy.zh-CN.md#会读取但不保存的内容)）。带了 `q` 就翻译 `q` 的内容，不再读前台 App。
- 扩展在「设置 › 扩展」里被停用、或者 id 不存在时，面板照常打开，Helmp 在 stderr 打印 `helmp: 没有名为 <id> 的范围（扩展可能被停用）`。
- 不认识的链接（路径不对、设置页面名不对）会被忽略，并在 stderr 打印 `helmp: 不认识的链接 <url>`。想看这些信息，从终端启动 Helmp（`target/release/Helmp.app/Contents/MacOS/helmp`）。
- 深链接不会发出任何网络请求。

### 示例

```bash
# 翻译剪贴板里的文字
open "helmp://scope/translate?q=$(pbpaste | python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.stdin.read()))')"

# 在剪贴板历史里搜「发票」
open "helmp://scope/clipboard?q=%E5%8F%91%E7%A5%A8"

# 直接打开模型设置
open "helmp://settings/models"
```
