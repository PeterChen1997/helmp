# Changelog

All notable changes to Helmp are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/). Each release is written in English first, then in Simplified Chinese.

所有值得注意的改动都记在这里，格式参照 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循[语义化版本](https://semver.org/lang/zh-CN/)。每个版本先写英文，再写简体中文。

## [Unreleased]

Planned as 0.2.0.

### Added

- **Files and recent files** (`f ` / `file ` / `文件 `): Spotlight file search plus files used in the last 30 days, matched by pinyin. A few results also appear in the main search; folders to search and exclude are set in Settings › Extensions › File Search.
- **System Settings panes**: open any pane of System Settings by name or pinyin, including the individual Privacy & Security pages.
- **System toggles**: Keep Awake (with a duration such as `2h`, stopped when Helmp quits), volume, mute / unmute, show or hide hidden files, eject all disks. Hidden files and eject confirm first.
- **Browser bookmarks** (`bm ` / `书签 `): Chrome, Edge, Brave, Arc (every profile) and Safari. Safari needs Full Disk Access, and Helmp says so when it's missing.
- **Dictionary** (`define ` / `dict ` / `d ` / `词典 ` / `查 `): offline lookups in the macOS dictionaries.
- **Units and currency** in the calculator: `10 km to mi`, `3斤`, `100 usd to cny`. Exchange rates are the ECB's daily reference rates, cached in `fx.json` so conversion works offline.
- **Time, time zones and timestamps**: `now`, `time tokyo`, Unix timestamps, `3pm 北京 in 纽约`, `in 3 days`, `next friday`. Time zone rules are built in.
- **Developer tools** (`dev ` / `开发 `): UUID, password, hashes, Base64, URL encoding, JSON, naming styles, number bases, JWT decoding, placeholder text, local and public IP.
- **Emoji and symbols** (`emoji ` / `表情 ` / `符号 `, or `:xiao` in the main search), with pinyin search, skin tones and recently used first.
- **Color picker and conversion**: pick a color on screen, or type HEX / `rgb()` / `hsl()`, and copy it as HEX, RGB, HSL, OKLCH, SwiftUI or NSColor.
- **Window management**: 18 layouts (halves, quarters, thirds, two-thirds, maximize, almost maximize, center, restore, next display) for the window you were using. Needs Accessibility.
- **Processes and ports** (`kill ` / `进程 ` / `ps `, `port 3000`): quit or force quit (after confirming) a process, and find what is listening on a port.
- **Snippets** (`sn ` / `片段 `): placeholders (`{date}`, `{time}`, `{clipboard}`, `{uuid}`, `{cursor}`, `{argument}`), a new Settings › Snippets page, and import from Raycast and Alfred.
- **Clipboard pinning and sequential paste**: pinned items stay on top and are never removed by retention; paste everything from an item to the latest in copy order.
- **Plugins**: declarative `plugin.json` manifests with open, http, script, copy and paste actions, forms, preferences with secrets in the Keychain, and script-backed lists in Alfred Script Filter JSON. A plugin's permissions are shown before you turn it on, and a plugin that asks for more after an update turns itself off until you confirm again.
- **Settings › Plugins**: turn plugins on and off, edit preferences, open the plugins folder, reload, create a new plugin from a template, open the developer docs.
- **Bundled plugins**: flomo, Quick Search (`gh`, `npm`, `crates`, `mdn`, `gt`, `zhihu`, `bili`, `xhs`) and Obsidian quick capture.
- `helmp plugins` and `helmp plugins validate <folder>` on the command line, and the `helmp://plugin/<plugin id>/<command id>` deep link.
- Plugin developer docs: [docs/plugins.md](docs/plugins.md), [docs/plugins.zh-CN.md](docs/plugins.zh-CN.md) and the JSON Schema [docs/plugin-schema.json](docs/plugin-schema.json).
- An end-to-end test suite (`tests/e2e_*.rs`) that runs the real binary in an isolated environment with `HELMP_DRY_RUN=1`. Every action that changes the user's environment now goes through one dry-run switch (`src/effects.rs`), so tests never touch the real clipboard, Keychain, windows, processes or network.

### Fixed

- Screenshot OCR results no longer squeeze the recognized text into two lines in the detail pane.
- For polyphonic characters, a match on a rare reading now ranks below a match on the common reading in the same tier.

### 简体中文

#### 新增

- **文件与最近文件**（`f ` / `file ` / `文件 `）：用 Spotlight 搜文件，加上最近 30 天用过的文件（支持拼音）。全局搜索里也会混入少量结果；搜索范围和排除的目录在「设置 › 扩展 › 文件搜索」里设置。
- **系统设置面板**：按名字或拼音直接打开系统设置的任意一页，「隐私与安全性」下的各个子页也能直达。
- **系统开关**：防休眠（可带时长，比如 `2h`，Helmp 退出时一起停止）、音量、静音 / 取消静音、显示或隐藏隐藏文件、弹出所有磁盘。显示隐藏文件和弹出磁盘会先确认。
- **浏览器书签**（`bm ` / `书签 `）：Chrome、Edge、Brave、Arc（每个 profile）和 Safari。Safari 需要「完全磁盘访问」权限，没有时会明确说明。
- **词典**（`define ` / `dict ` / `d ` / `词典 ` / `查 `）：用 macOS 自带词典离线查词。
- 计算器支持**单位与汇率换算**：`10 km to mi`、`3斤`、`100 usd to cny`。汇率用欧洲央行（ECB）每日参考汇率，缓存在 `fx.json`，离线也能换算。
- **时间、时区与时间戳**：`now`、`time tokyo`、Unix 时间戳、`3pm 北京 in 纽约`、`in 3 days`、`next friday`。时区规则内置。
- **开发工具**（`dev ` / `开发 `）：UUID、密码、哈希、Base64、URL 编解码、JSON、命名风格、进制转换、JWT 解码、假文、本机与公网 IP。
- **Emoji 与符号**（`emoji ` / `表情 ` / `符号 `，或在全局搜索里输入 `:xiao`）：支持拼音搜索、选肤色，最近用过的排在前面。
- **取色器与颜色转换**：从屏幕上取色，或输入 HEX / `rgb()` / `hsl()`，以 HEX、RGB、HSL、OKLCH、SwiftUI、NSColor 写法复制。
- **窗口管理**：18 种布局（半屏、四角、三分之一、三分之二、最大化、几乎最大化、居中、恢复、移到下一个显示器），作用于你刚才在用的窗口。需要辅助功能权限。
- **进程与端口**（`kill ` / `进程 ` / `ps `，`port 3000`）：退出或（确认后）强制退出进程，查出谁在监听某个端口。
- **文本片段**（`sn ` / `片段 `）：支持占位符（`{date}`、`{time}`、`{clipboard}`、`{uuid}`、`{cursor}`、`{argument}`），新增「设置 › 片段」，可从 Raycast 和 Alfred 导入。
- **剪贴板置顶与合并粘贴**：置顶的条目排在最前、不受保留天数清理；可以把从某条到最新的几条按复制顺序合并粘贴。
- **插件**：声明式的 `plugin.json` 清单，支持 open、http、script、copy、paste 动作，表单，偏好设置（密钥只存在钥匙串里），以及由脚本输出 Alfred Script Filter JSON 的列表。启用插件前会先列出它的权限；插件更新后要更多权限时会自动停用，等你再次确认。
- **设置 › 插件**：启用 / 停用插件、填写偏好设置、打开插件文件夹、重新加载、从模板新建插件、打开开发者文档。
- **自带插件**：flomo、快捷搜索（`gh`、`npm`、`crates`、`mdn`、`gt`、`zhihu`、`bili`、`xhs`）和 Obsidian 快速记录。
- 命令行新增 `helmp plugins` 和 `helmp plugins validate <目录>`，深链接新增 `helmp://plugin/<插件 id>/<命令 id>`。
- 插件开发文档：[docs/plugins.zh-CN.md](docs/plugins.zh-CN.md)、[docs/plugins.md](docs/plugins.md)，以及 JSON Schema [docs/plugin-schema.json](docs/plugin-schema.json)。
- 端到端测试（`tests/e2e_*.rs`）：在隔离环境里以 `HELMP_DRY_RUN=1` 运行真实的可执行文件。所有会改动用户环境的动作都统一经过一个演练开关（`src/effects.rs`），测试不会碰真实的剪贴板、钥匙串、窗口、进程和网络。

#### 修复

- 截图识别的详情里，识别出的文字不再被挤成两行。
- 多音字按生僻读音命中时，排在同一档按常用读音命中的结果之后。

## [0.1.0] - 2026-11-10

The first public build, signed with a Developer ID and notarized by Apple. Requires macOS 15 Sequoia or later.

### Added

- App, command and file search with pinyin and pinyin-initial matching.
- Clipboard history, opened directly with <kbd>⌘⇧V</kbd>.
- Screenshot OCR with <kbd>⌘⇧2</kbd>: keeps paragraphs and reads QR codes.
- Translate with <kbd>⌥⇧T</kbd>: prefills the selection, translates as you type, and uses your own model.
- Claude Code / Codex sessions waiting on you, with <kbd>⌃⌥A</kbd>, and a jump back to them.
- Calculator, web search fallback, and system commands that confirm before doing anything destructive.
- `helmp://` deep links and a bilingual interface (Chinese and English).
- All shortcuts can be changed in Settings › Extensions, with conflict warnings while you record.

### Changed

- The panel's search field is a plain text field and no longer switches the input method.
- Opening Translate puts the focus in the text box below.

### Fixed

- API keys saved by an older dev build may not be readable by the signed build. Helmp now explains why, and saving the key again in Settings › Models deletes the old Keychain item before creating a new one.

### 简体中文

Helmp 的第一个公开构建：已用 Developer ID 签名并通过 Apple 公证。需要 macOS 15 Sequoia 或更高版本。

#### 新增

- 搜索应用、命令和文件，支持拼音和拼音首字母。
- 剪贴板历史，<kbd>⌘⇧V</kbd> 直接打开。
- 截图识别文字（OCR），<kbd>⌘⇧2</kbd>：保留分段，能识别二维码。
- 翻译，<kbd>⌥⇧T</kbd>：选中的文字会自动带入；输入时实时翻译，翻译用的是你自己配置的模型。
- 查看正在等你回复的 Claude Code / Codex 会话，<kbd>⌃⌥A</kbd>，可以一键跳回对应会话。
- 计算器、网页搜索、系统命令（危险操作会先让你确认）。
- 支持 `helmp://` 深链接；中英双语界面。
- 所有快捷键都可以在「设置 › 扩展」里修改，录制时会提示冲突。

#### 变更

- 面板搜索框是普通输入框，不再自动切换输入法。
- 进入翻译后，焦点直接落在下方的文本框里。

#### 修复

- 旧的开发版存在钥匙串里的 API Key，正式签名的版本可能读不出来。现在会说明原因；到「设置 › 模型」重新保存一次 Key，会先删掉旧的钥匙串条目再新建。

[Unreleased]: https://github.com/PeterChen1997/helmp/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/PeterChen1997/helmp/releases/tag/v0.1.0
