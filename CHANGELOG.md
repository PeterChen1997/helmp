# Changelog

All notable changes to Helmp are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/). Each release is written in English first, then in Simplified Chinese.

所有值得注意的改动都记在这里，格式参照 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循[语义化版本](https://semver.org/lang/zh-CN/)。每个版本先写英文，再写简体中文。

## [Unreleased]

### Fixed

- The launcher panel no longer has a title bar, so macOS can't show close, minimize, and zoom buttons in its top-left corner. Chinese input methods, the glass background, and rounded corners work as before.
- Pasting from clipboard history no longer fails silently. Without Accessibility permission, Helmp still copies the item, explains why it couldn't paste, and offers a button that opens the Accessibility pane in System Settings. Helmp also waits for the app you were using to return to the front before it presses <kbd>⌘V</kbd>. If that app doesn't come back within 1.5 seconds, Helmp reports the problem instead of sending the keystroke to the wrong app or to itself.

### 简体中文

#### 修复

- 启动器面板去掉了标题栏，左上角不会再出现关闭、最小化、缩放按钮。中文输入法、毛玻璃背景和圆角都不受影响。
- 从剪贴板历史粘贴不再悄悄失败。没有辅助功能权限时仍会复制，并说明没能粘贴的原因，提供直达「系统设置 › 辅助功能」的按钮。按 <kbd>⌘V</kbd> 前会等原来的 App 回到前台；1.5 秒内没回来就报错，不会把按键发给别的 App 或 Helmp 自己。

## [0.4.4] - 2026-11-11

### Added

- Screenshot selections support rectangle, ellipse, line, arrow, pen, highlighter, and rectangular mosaic annotations. Choose a preset or enter a hex color, adjust stroke and mosaic block sizes, and use <kbd>⌘Z</kbd> / <kbd>⇧⌘Z</kbd> to undo or redo. Copy is the rightmost action.
- Completed agent replies preserve Markdown formatting and remember which completion you have read.

### Changed

- Clipboard history targets 50 MB of saved content. When it exceeds that size, Helmp removes the oldest unpinned items older than 72 hours first. Copies over 50 MB keep a local record of their time, type, and size.

### Fixed

- Global shortcuts respond to system events immediately. Screenshot overlays open without a popup animation or an extra wait when the launcher is hidden. On macOS 26 and later, ScreenCaptureKit provides full-resolution frames directly in memory. The system screenshot utility remains available as a fallback.

### 简体中文

#### 新增

- 截图选区支持矩形、椭圆、直线、箭头、画笔、荧光笔和框选马赛克。可用预设色或输入十六进制颜色，调整笔宽和马赛克块大小，并用 <kbd>⌘Z</kbd> / <kbd>⇧⌘Z</kbd> 撤销或重做。复制按钮位于最右侧。
- 已完成的 Agent 回复保留 Markdown 格式，并记住已经查看过的完成结果。

#### 变更

- 剪贴板历史以 50 MB 为清理目标。超过时优先清理 72 小时前最旧的未置顶条目。单次复制超过 50 MB 时，本地只记录时间、类型和大小。

#### 修复

- 全局快捷键收到系统事件后立即响应。启动器已收起时，截图直接取屏，遮罩关闭弹出动画。macOS 26 及以上通过 ScreenCaptureKit 直接获取内存里的全分辨率画面，必要时回退到系统截图工具。

## [0.4.3] - 2026-11-11

### Changed

- "Helmp Settings" no longer appears in search results, where it was easy to mistake for macOS System Settings. The bottom-right of the panel now shows <kbd>⌘,</kbd> to open Helmp's settings, and the shortcut works from the panel. The menu bar still has "Settings…".

### 简体中文

#### 变更

- 「Helmp 设置」不再出现在搜索结果里，免得和 macOS 的「系统设置」混在一起。面板右下角常驻提示 <kbd>⌘,</kbd>，在面板里按它打开 Helmp 设置；菜单栏的「设置…」仍然可用。

## [0.4.2] - 2026-11-11

### Added

- Settings › About has a "Contact the Author" section with the author's name and a link to the blog.

### Changed

- "Screenshot" (<kbd>⇧⌘1</kbd>) and "Recognize Text in Screenshot" (<kbd>⇧⌘2</kbd>) are shown as two separate shortcuts everywhere. Settings › Extensions used to show a single shortcut on the "Screenshot & OCR" row (the OCR one), and Settings › Screenshot only the Screenshot one, so it looked like one shortcut. Both now appear side by side, each with its own recorder, and recording one with the other's combination is refused by name. Search results and the home view show each command's own shortcut. Existing settings are kept.
- Clipboard history opens with the most recent item selected instead of a pinned item at the top (pinned items stay listed where they are).
- Double-clicking a clipboard item pastes it into the app you came from, like <kbd>↵</kbd> (it used to only copy). With auto-paste off or without the Accessibility permission it copies, as <kbd>↵</kbd> does.
- <kbd>Esc</kbd> in clipboard history closes the panel directly instead of going back to the main search; the next time the panel opens it starts from the main search.

### Fixed

- Much lower CPU use while idle. Codex threads whose details can't be read are retried every 10 seconds instead of every second, agent folders are scanned every 5 seconds while the panel is closed (every second while it's open), and the menu bar only wakes up when there is something to do.
- Release notes in Settings › About are shown as headings and bullet points instead of raw Markdown (`### Added`, `<kbd>⇧⌘1</kbd>`).

### 简体中文

#### 新增

- 设置 › 关于新增「联系作者」：作者名字和博客链接。

#### 变更

- 「截图」（<kbd>⇧⌘1</kbd>）和「截图识别文字」（<kbd>⇧⌘2</kbd>）在各处都显示成两条独立的快捷键。以前「设置 › 扩展」里「截图与文字识别」一行只有一个快捷键（其实是识别文字的那条），「设置 › 截图」里只有截图的那条，看起来像只有一个。现在两条并排列出、各自录制，把一条录成另一条正在用的组合会被指名拦下。搜索结果和首页上各自显示自己的快捷键。已有设置保持不变。
- 打开剪贴板历史时默认选中最近的一条，而不是排在最前的置顶记录（置顶的仍列在原处）。
- 双击剪贴板条目和回车一样，粘贴到你刚才所在的 App（以前只复制）。关了自动粘贴或没有辅助功能权限时和回车一样只复制。
- 在剪贴板历史里按 <kbd>Esc</kbd> 直接收起面板，不再退回主搜索；下次唤起从主搜索开始。

#### 修复

- 空闲时 CPU 占用大幅降低。读不到详情的 Codex 线程改为每 10 秒重试（原来每秒），面板收起时每 5 秒扫描一次 agent 目录（打开时仍是每秒），菜单栏只在有事时才唤醒。
- 设置 › 关于里的更新说明按小标题和列表显示，不再露出 Markdown 原文（`### Added`、`<kbd>⇧⌘1</kbd>`）。

## [0.4.1] - 2026-11-11

### Added

- Snipaste-style screenshots. "Screenshot" (<kbd>⇧⌘1</kbd>, also in the menu bar) freezes every display first, then you select on the still image: drag in any direction, or click to pick the window under the mouse. The selection stays put; drag its handles to resize it or drag inside to move it. A toolbar at its bottom-right offers Save, Copy, Pin, Recognize Text and Close. <kbd>↵</kbd> or a double-click inside copies, <kbd>⌘C</kbd> copies, <kbd>⌘S</kbd> saves (a save dialog opening in the screenshot folder from Settings), <kbd>Esc</kbd> quits, and a right-click clears the selection (a second one quits). Copy and Save confirm with a short "Copied" / "Saved" notice. Images keep the display's full pixel resolution and color profile. "Pin" turns the selection into an always-on-top floating image at the same spot: drag it anywhere, hover it for Copy / Save / Recognize Text / Close, press <kbd>Esc</kbd> or double-click to close it; pin as many as you like. Change the shortcut in Settings › Screenshot. The old "Screenshot to Clipboard" and "Screenshot and Save" commands open the same screen (Return saves in the latter), so recent items and shortcuts that point at them keep working.
- "Recognize Text in Screenshot" (<kbd>⇧⌘2</kbd>) uses the same frozen screen and recognizes the moment you release the mouse. The text appears in a floating window next to the selection ("Recognizing…" first), titled like "2 lines · English", with Copy (<kbd>↵</kbd>), Translate (<kbd>⌘T</kbd>), Open Link for QR codes (<kbd>⌘O</kbd>) and Close (<kbd>Esc</kbd>). Non-empty text is still copied automatically, and the window says so; when nothing is found it says that too.

### Fixed

- Results shown after an action that ran with the panel hidden (a missing-permission notice, an error) no longer vanish right away. When another process hands focus back a moment after the panel reappears, the panel now takes focus back instead of hiding. This is why "Screenshot" and "Recognize Text in Screenshot" seemed to do nothing.
- Searching "截图", "jt" or "screenshot" lists "Screenshot" first again, even if you use "Recognize Text in Screenshot" more often.

### 简体中文

#### 新增

- Snipaste 式截图。「截图」（<kbd>⇧⌘1</kbd>，菜单栏里也有）先冻结所有屏幕，再在静止的画面上框选：往任意方向拖，或者单击选中鼠标下的窗口。松手后选区留在原地，拖控制点改大小，在里面拖动可以平移；选区右下角外侧是工具栏「保存、复制、钉图、识别文字、关闭」。<kbd>↵</kbd> 或在选区里双击复制，<kbd>⌘C</kbd> 复制，<kbd>⌘S</kbd> 保存（另存为面板，默认在设置里的截图目录），<kbd>Esc</kbd> 退出，右键取消选区（再按一次退出）。复制、保存后有「已复制」「已保存」提示。图片保持屏幕的原始像素分辨率和色彩配置。「钉图」把选区原地变成一张置顶的浮动图片：可以拖到任意位置，鼠标移上去出现复制 / 保存 / 识别文字 / 关闭，按 <kbd>Esc</kbd> 或双击关闭，可以钉很多张。快捷键在「设置 › 截图」里改。原来的「截图到剪贴板」「截图并保存」命令打开同一个界面（后者回车是保存），指向它们的最近使用和快捷键照常可用。
- 「截图识别文字」（<kbd>⇧⌘2</kbd>）用同样的冻结画面，松开鼠标立即识别。结果显示在选区旁边的浮窗里（先显示「识别中…」），标题如「识别到 2 行 · 中文」，可以复制（<kbd>↵</kbd>）、翻译（<kbd>⌘T</kbd>）、打开二维码链接（<kbd>⌘O</kbd>）、关闭（<kbd>Esc</kbd>）。非空的文字仍会自动复制，窗口里写明；什么都没识别到时也明确说出来。

#### 修复

- 面板收起期间执行的动作，结果（没有权限的提示、出错说明）弹出后不再一闪而过。面板重新出现后，别的进程才把焦点还回来时，面板会把焦点要回来，而不是收起。「截图」「截图识别文字」看起来没反应就是这个原因。
- 搜「截图」「jt」「screenshot」时「截图」重新排第一，即使你更常用「截图识别文字」。

## [0.4.0] - 2026-11-10

### Added

- Automatic updates. With "Automatically check for updates" on (Settings › General), Helmp checks once a day for a new version, first at Qiniu (`blog.peterchen97.cn`, reachable from mainland China) and then GitHub Releases. "Check for Updates…" in the menu bar, the panel command "Check for Helmp Updates" and the button in Settings open Settings › About and check right away; the result or the reason it failed is shown there. When a version is found the menu item reads "Update to x.y.z…", and About shows the release notes and a "Download and Install" button. Nothing is downloaded before you click it. Helmp then checks the download's size and SHA-256, its code signature, that it comes from the same Developer ID team and that Gatekeeper accepts it, quits, swaps itself and reopens. When it can't replace itself (not in Applications, or no write permission) it downloads the dmg for you instead. `helmp update check` / `helmp update install [--yes]` do the same from the command line. See [docs/updates.md](docs/updates.md).

### Changed

- Plugin commands that take input (flomo, Quick Search's `gh` / `npm`…, list commands) and MCP tools with arguments no longer take what you type in the main search, so a search can't trigger them by accident: typing `flomo hello` or `gh rust` only lists the command. Press Tab (or Return) on it to enter the command's own scope: a tag with the plugin's name appears in the search bar, the flomo form gets the cursor, `gh` takes the search terms; Esc, or Backspace when empty, goes back. `helmp://plugin/…?q=` opens the panel inside that scope. `helmp query` / `helmp run` take `--scope <id>`, and agents get a `scope` from `helmp_search` to pass to `helmp_search` / `helmp_run`.

### Fixed

- Chinese and other input methods compose again in the panel, and ABC keeps working. The panel was a non-activating panel; macOS doesn't start an input-method session for those, so Doubao, Apple Pinyin and others typed raw letters. The panel now activates Helmp when it opens (like Alfred and Raycast) and hands focus back to the previous app when it closes; paste-style actions wait for that before sending ⌘V. Verified with real key events for Apple Pinyin, Doubao and ABC, in the search box and the flomo form; `script/ime-check.sh` repeats the check.
- Opening Helmp yourself (or opening it again while it runs) now shows the panel directly instead of a "Helmp is running" card; first launch still shows onboarding.
- Opening the panel from another Helmp window no longer makes it flash and disappear: a momentary focus hop between windows is ignored (the panel hides only if it is still in the background 150 ms later).
- Apps bundled inside Xcode (Device Hub, Instruments, Accessibility Inspector, FileMerge, Simulator…) are found, like in Spotlight.
- Spaces and hyphens no longer matter in names: `device hub` finds DeviceHub, `devicehub` finds "Device Hub".
- Long bookmark titles no longer match unrelated queries through loose letter-order matching.

### 简体中文

#### 新增

- 自动更新。打开「设置 › 通用 › 自动检查更新」后，Helmp 每天检查一次新版本，先查七牛（`blog.peterchen97.cn`，国内可直连），再查 GitHub Releases。菜单栏「检查更新…」、面板命令「检查 Helmp 更新」和设置里的按钮会打开「设置 › 关于」并立即检查，结果或失败原因显示在那里。发现新版本时菜单项变成「更新到 x.y.z…」，关于页显示更新说明和「下载并安装」按钮，点之前不下载任何东西。之后 Helmp 校验下载内容的大小和 SHA-256、代码签名、是否出自同一个 Developer ID 团队、Gatekeeper 是否放行，再退出、替换并重新打开。无法原地替换时（不在「应用程序」里、或没有写权限）改为替你下载 dmg。命令行用 `helmp update check` / `helmp update install [--yes]`。见 [docs/updates.zh-CN.md](docs/updates.zh-CN.md)。

#### 变更

- 接受输入的插件命令（flomo、快捷搜索的 `gh` / `npm`…、列表命令）和带参数的 MCP 工具，不再接收全局搜索里打的字，日常搜索不会误触发：输入 `flomo 你好`、`gh rust` 只列出这个命令。在它上面按 Tab（或回车）进入命令自己的范围：搜索栏出现插件名标签，flomo 光标直接进表单，`gh` 接收搜索词；Esc、或输入为空时按 ⌫ 返回。`helmp://plugin/…?q=` 打开面板直接进入这个范围。`helmp query` / `helmp run` 支持 `--scope <id>`，agent 从 `helmp_search` 拿到 `scope`，再交给 `helmp_search` / `helmp_run`。

#### 修复

- 面板里中文等输入法恢复组字，ABC 也照常可用。原因是面板是「非激活面板」，macOS 不为它建立输入法会话，豆包、系统拼音等只会原样上屏字母。现在唤起时激活 Helmp（与 Alfred、Raycast 一致），收起时把焦点还给原来的 App，粘贴类动作发 ⌘V 前会等焦点回去。已用真实按键验证系统拼音、豆包、ABC，覆盖搜索框和 flomo 表单；`script/ime-check.sh` 可重复这项检查。
- 手动打开 Helmp（或在运行中再次打开）直接唤起面板，不再弹「Helmp 已在运行」卡片；首次启动仍走引导。
- 从 Helmp 的其他窗口唤起面板不再一闪就消失：忽略窗口之间瞬时的焦点跳动（150 ms 后仍在后台才收起）。
- 能搜到 Xcode 里自带的工具（Device Hub、Instruments、Accessibility Inspector、FileMerge、Simulator 等），和 Spotlight 一致。
- 名字里的空格、连字符不再影响匹配：`device hub` 能找到 DeviceHub，`devicehub` 也能找到「Device Hub」。
- 很长的书签标题不再靠「字母按顺序出现」的模糊匹配命中无关查询。

## [0.3.0] - 2026-11-10

### Added

- **Plugin Center**: install reviewed plugins from [PeterChen1997/helmp-plugins](https://github.com/PeterChen1997/helmp-plugins) in Settings › Plugins, with `helmp plugins install <id>`, or from the landing page (`helmp://install-plugin?id=…`). Every file's SHA-256 is checked, installs are atomic, and installed plugins stay off until you approve their permissions. Settings shows when an update is available. See [docs/plugin-center.md](docs/plugin-center.md).
- `helmp plugins submit <folder>` opens a pull request to the Plugin Center with your `gh` account. It only explains what it will do unless you add `--yes`. `helmp plugins validate --json` prints the result as JSON.
- Manifests can declare `license`.
- **Coding agents can build and operate Helmp** (`helmp mcp`, an MCP server): Claude Code, Codex or Cursor can scaffold, validate, install and dry-run plugins from a plain request, ask you before submitting them to the Plugin Center, and search or run Helmp commands. Anything with side effects asks you in Helmp first (Return allows, Esc denies, 60 s timeout); clipboard history and session transcripts stay private by default. Connect an agent in Settings › Agents or with `helmp mcp install claude|codex|cursor` (configs are backed up first). See [docs/mcp.md](docs/mcp.md).
- **MCP servers in the launcher** (Settings › MCP, `helmp mcp-client`): add servers from a curated list, by pasting JSON, or by importing (read-only) from Claude Code, Codex and Cursor; their tools become launcher commands with forms generated from the input schema, destructive tools ask first, secrets stay in the Keychain. Plugins can bundle MCP servers with `mcpServers`. See [docs/mcp-client.md](docs/mcp-client.md).

### Changed

- The panel is smaller and better proportioned: 760 × 480 instead of 880 × 540, with a 330 pt list column.
- Opening Helmp yourself shows a welcome page with the shortcut to open it (also when you open it again while it runs; not when it starts at login).
- Results: a single click selects and shows details, a double click runs; a double click on a clipboard item copies it instead of pasting.

### Fixed

- Typing in the panel with ABC and other keyboard layouts: the panel's first responder was GPUI's container view, which gets no text input context, so keys arrived but no text was inserted.
- ⌘Space (or any shortcut) held by another launcher such as HapiGo can now be saved: Helmp names the app that probably holds it and takes the shortcut over automatically once that app lets go.
- Vision OCR requests run one at a time, which avoids sporadic recognition errors under load.

### 简体中文

#### 新增

- **插件中心**：在「设置 › 插件」里、用 `helmp plugins install <id>`，或者在落地页上（`helmp://install-plugin?id=…`）安装经过审核的插件（来自 [PeterChen1997/helmp-plugins](https://github.com/PeterChen1997/helmp-plugins)）。每个文件都核对 SHA-256，安装是原子的，装好的插件在你确认权限前保持停用。有新版本时设置页会提示。见 [docs/plugin-center.zh-CN.md](docs/plugin-center.zh-CN.md)。
- `helmp plugins submit <文件夹>` 用你的 `gh` 账号向插件中心开 PR；不加 `--yes` 只说明将要做什么。`helmp plugins validate --json` 以 JSON 输出结果。
- 清单可以声明 `license`。
- **Coding agent 能开发和操作 Helmp**（`helmp mcp`，MCP 服务）：Claude Code、Codex、Cursor 可以按一句需求新建、校验、安装、演练插件，提交到插件中心前先问你，也能搜索和执行 Helmp 的命令。有副作用的操作先在 Helmp 里问你（回车允许、Esc 拒绝、60 秒超时）；剪贴板历史和会话记录默认不给。在「设置 › Agent」或用 `helmp mcp install claude|codex|cursor` 连接（改配置前先备份）。见 [docs/mcp.zh-CN.md](docs/mcp.zh-CN.md)。
- **启动器里用 MCP 服务器**（「设置 › MCP」、`helmp mcp-client`）：从精选列表、粘贴 JSON，或只读导入 Claude Code / Codex / Cursor 里的配置添加；它们的工具变成启动器命令，按参数自动生成表单，破坏性工具先确认，密钥只存钥匙串。插件可以用 `mcpServers` 自带 MCP 服务器。见 [docs/mcp-client.zh-CN.md](docs/mcp-client.zh-CN.md)。

#### 变更

- 面板缩小、比例更协调：880 × 540 改为 760 × 480，列表栏 330。
- 手动打开 Helmp 时显示欢迎页，写明唤起快捷键（运行中再次打开也会显示；登录时自动启动不弹）。
- 结果列表：单击选中看详情，双击执行；剪贴板条目双击是复制，不是粘贴。

#### 修复

- 用 ABC 等键盘布局时面板里打不了字：第一响应者是 GPUI 外层的容器视图，没有文字输入上下文，按键收到了却插不进字。
- 被 HapiGo 等其他启动器占着的 ⌘Space（或任何快捷键）现在可以保存：提示里点名可能占用的应用，对方让出后自动接管。
- Vision 识别串行执行，避免负载高时偶发识别失败。

## [0.2.0] - 2026-11-10

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

[Unreleased]: https://github.com/PeterChen1997/helmp/compare/v0.4.3...HEAD
[0.4.3]: https://github.com/PeterChen1997/helmp/compare/v0.4.2...v0.4.3
[0.4.2]: https://github.com/PeterChen1997/helmp/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/PeterChen1997/helmp/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/PeterChen1997/helmp/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/PeterChen1997/helmp/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/PeterChen1997/helmp/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/PeterChen1997/helmp/releases/tag/v0.1.0
