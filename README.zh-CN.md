<p align="center">
  <img src="docs/images/icon-256.png" width="128" height="128" alt="Helmp 图标">
</p>

<h1 align="center">Helmp</h1>

<p align="center">
  macOS 原生启动器，顺便告诉你哪个 coding agent 在等你。<br>
  Rust + GPUI 编写，数据只在本机，不用登录，没有遥测。
</p>

<p align="center">
  <a href="README.md">English</a> · 简体中文
</p>

---

Helmp（读作 help me）是一个 Spotlight 式的 macOS 启动器：按 <kbd>⌥ Space</kbd>，输入，回车。除了打开应用，它还会列出你的 Claude Code 和 Codex 会话，一键跳到正在等你的那个；它有不限条数的本地剪贴板历史，能识别截图里的文字，用你自己接入的模型翻译，还能直接运行现成的 Raycast Script Commands 脚本。

> **当前状态：早期。** Helmp 还在积极开发中，暂时没有打包好的安装包，需要从源码构建。界面目前只有简体中文，英文界面正在做。

<!-- 截图：图片准备好后替换下面这些文件即可。 -->
<p align="center">
  <img src="docs/images/screenshot-home.png" width="720" alt="首页：等你处理的会话、最近使用与快捷命令">
</p>

| Agent 会话 | 剪贴板历史 |
|---|---|
| ![Agent 会话](docs/images/screenshot-agents.png) | ![剪贴板历史](docs/images/screenshot-clipboard.png) |
| **截图识别** | **翻译** |
| ![截图识别](docs/images/screenshot-ocr.png) | ![翻译](docs/images/screenshot-translate.png) |

## 功能

- **应用搜索，支持拼音。** 英文名、中文本地化名、全拼、首字母都能搜：`wx`、`weixin`、`微信` 都能找到微信，多音字的各个读音也都算数。详情里会写明这次是怎么匹配上的。<kbd>⌘K</kbd> 可以在访达中显示、复制路径，应用在运行时还能直接退出。索引覆盖 `/Applications`、`/System/Applications` 和 `~/Applications`，装卸应用后自动更新。
- **Agent 会话切换（Claude Code / Codex）。** 命令行和桌面 App 里的会话都会列出来，按「等你处理」「进行中」「最近完成」分组，详情里显示最后一条消息。回车直接跳过去：桌面 App 走深链，iTerm2 和终端 App 精确选中对应的标签页，其他终端则激活宿主 App。正在运行的会话绝不会被再恢复一次。Helmp 对 `~/.claude`、`~/.codex` 只读，不写入，也不装任何 hooks。
- **剪贴板历史。** 文本、图片、文件，存在本机的 SQLite 里，不限条数，按天分组，可按类型筛选。密码管理器标记为「隐藏」的内容不会记录，也可以排除指定的 App（默认排除「密码」）。回车粘贴回你刚才所在的 App，<kbd>⌥↵</kbd> 粘贴为纯文本。
- **截图识别文字。** 区域截图到剪贴板、截图并保存，或者用 Apple Vision 识别其中的文字（默认简体中文和英文）。也能识别剪贴板里的图片，或指定的图片文件（`ocr /图片/路径.png`）。<kbd>⌘T</kbd> 把识别结果带进翻译。
- **翻译，用你自己的模型。** 支持 Anthropic、任意 OpenAI 兼容接口（OpenAI、DeepSeek、各类网关）和本机 Ollama。API Key 存在 macOS 钥匙串里。唤起时自动带上前台 App 里选中的文字，<kbd>⌘↵</kbd> 用译文替换原来的选中内容。
- **兼容 Raycast 的脚本命令。** 带 `@raycast.*` 头注释的脚本放进脚本目录就会变成一条命令。语言不限，最多三个参数（自动生成表单），支持 `silent` / `compact` / `fullOutput` 三种输出方式，危险脚本可以要求确认，每次运行最多 30 秒。
- **菜单栏。** 没有 Dock 图标。有会话在等你时，菜单栏图标右上角会出现一个橙点；菜单里列出这些会话、常用入口、设置和退出。
- **全局快捷键。** 一个唤起面板，每个模块再各有一个，都能在设置里改或关掉。

一些让日常使用更顺手的细节：

- 面板打开时自动切到英文 ABC 键盘，收起时切回原来的输入法。搜拼音缩写不用再跟输入法较劲。
- 90 秒内再次唤起会保留上次的输入并全选；超过 90 秒，或者刚执行过一个结果，就从空白开始。
- 设置窗口（通用、扩展、模型、Agent、剪贴板、截图、外观、关于）和首次启动的权限引导。
- 无头命令行（`helmp query wx`），方便写脚本和排查问题。

## 环境要求

- macOS 15 Sequoia 或更新版本。
- 用 [rustup](https://rustup.rs) 管理的 Rust 工具链。具体版本锁在 [`rust-toolchain.toml`](rust-toolchain.toml) 里，第一次构建时会自动安装。
- Xcode。GPUI 用 `xcrun metal` 编译 Metal 着色器；如果提示找不到这个命令，运行 `xcodebuild -downloadComponent MetalToolchain`。

## 从源码构建

```bash
git clone https://github.com/PeterChen1997/helmp.git
cd helmp

# 直接运行（开发时）
cargo run

# 构建 release 版本
cargo build --release

# 打包成 target/release/Helmp.app 并签名
bash script/bundle.sh release
open target/release/Helmp.app
```

`script/bundle.sh` 的签名顺序：先用 `HELMP_SIGN_IDENTITY` 指定的证书，没有就找本机第一个「Developer ID Application」或「Apple Development」证书，都没有才退回 ad-hoc 签名。系统按签名记录权限，用 ad-hoc 签名的话，每次重新打包都要重新授权。

## 首次启动与权限

第一次启动会出现一个简短的引导。点某项权限旁边的按钮，再把浮窗里的 Helmp 图标拖进系统设置的列表即可。

| 权限 | 用途 | 不授权时 |
|---|---|---|
| 辅助功能 | 把剪贴板条目粘贴到前台 App、翻译时读取选中的文字、用译文替换选中内容 | 内容仍会写进剪贴板，需要你自己按 <kbd>⌘V</kbd> |
| 录屏 | 区域截图、截图识别文字 | 截图命令会提示你去授权；识别剪贴板图片和图片文件不受影响 |
| 自动化（首次使用时由系统询问） | 跳转 Agent 会话时选中 iTerm2 / 终端里对应的标签页 | 跳转到这两个终端里的会话会失败；桌面 App 里的会话走深链，不受影响 |

全局快捷键用的是 Carbon 的 `RegisterEventHotKey`，不需要任何权限。

## 快捷键

全局快捷键（默认值，都能在设置里修改）：

| 快捷键 | 作用 |
|---|---|
| <kbd>⌥ Space</kbd> | 唤起 / 收起面板 |
| <kbd>⇧⌘V</kbd> | 剪贴板历史 |
| <kbd>⇧⌘2</kbd> | 区域截图并识别文字（不打开面板，直接执行） |
| <kbd>⌥⇧T</kbd> | 翻译（自动带上选中的文字） |
| <kbd>⌃⌥A</kbd> | Agent 会话 |

翻译和 Agent 的默认快捷键特意避开了 <kbd>⇧⌘T</kbd>（浏览器的「重新打开标签页」）和 <kbd>⌥A</kbd>（会输入字符 `å`）。

面板内：

| 快捷键 | 作用 |
|---|---|
| <kbd>↑</kbd> <kbd>↓</kbd> | 选择 |
| <kbd>↵</kbd> | 执行主操作 |
| <kbd>⌘K</kbd> | 当前项的更多操作 |
| <kbd>⇥</kbd> | 在范围内切换筛选（如 全部 / 文本 / 图片 / 文件） |
| <kbd>esc</kbd> | 依次：关闭操作菜单 → 清空 → 退出范围 → 收起面板 |
| <kbd>⌘,</kbd> | 打开设置 |

输入前缀加空格可以直接进入某个模块：`ag ` / `agent ` 进 Agent 会话，`cb ` / `clip ` / `剪贴板 ` 进剪贴板，`fy ` / `tr ` / `翻译 ` 进翻译。

## 配置与数据位置

Helmp 保存的所有东西都在同一个目录里：

```
~/Library/Application Support/Helmp/
├── config.json          设置（不含任何密钥）
├── history.json         首页「最近使用」，最多 30 条
├── clipboard.sqlite     剪贴板历史
├── clipboard-images/    复制过的图片
├── scripts/             你的脚本命令（默认脚本目录）
├── screenshots/         「截图并保存」的输出位置（可改）
├── ocr/                 识别结果里展示的最近几张截图
└── icon-cache/          渲染好的应用与文件图标
```

`config.json` 由设置窗口读写；要手动改的话，请先退出 Helmp。API Key 存在钥匙串里，不在这个文件中。设置环境变量 `HELMP_DATA_DIR` 可以换一个目录（测试时很方便）。具体存了什么、读了什么，见 [docs/privacy.zh-CN.md](docs/privacy.zh-CN.md)。

## 脚本命令

Helmp 兼容 [Raycast Script Commands](https://github.com/raycast/script-commands) 的头注释格式。把可执行脚本放进 `~/Library/Application Support/Helmp/scripts/`（或者在「设置 › 扩展」里添加别的目录），几秒内就会出现在搜索结果里：

```bash
#!/bin/bash

# @raycast.schemaVersion 1
# @raycast.title 当前时间
# @raycast.mode compact
# @raycast.packageName 示例
# @raycast.description 显示当前日期与时间

date "+%Y-%m-%d %H:%M:%S %Z"
```

声明参数后会自动生成表单：

```bash
# @raycast.argument1 { "type": "text", "placeholder": "关键词" }
# @raycast.argument2 { "type": "dropdown", "placeholder": "引擎", "data": [{"title": "Google", "value": "g"}, {"title": "Bing", "value": "b"}] }
```

更多例子见 [`examples/scripts`](examples/scripts)，完整的字段说明见 [docs/scripts.zh-CN.md](docs/scripts.zh-CN.md)。

## 命令行

```bash
cargo run -- query wx            # 打印结果：分组、标题、副标题、id
cargo run -- run <id> [query]    # 执行某个结果
cargo run -- providers           # 列出已注册的插件
HELMP_DRY_RUN=1 cargo run -- run <id> agent   # 只打印 Agent 跳转命令，不执行
HELMP_TRACE=1 cargo run          # 把界面事件打到 stderr
```

## Roadmap

下面是想做但还没做的事，顺序可能调整。

- 英文界面（进行中）。
- 签名并公证的安装包、Homebrew cask、自动更新（菜单里的「检查更新」目前还没有可检查的发布渠道）。
- 以 MCP server 的形式把应用搜索、剪贴板、截图、OCR 开放给 coding agent。
- 内置开发者小工具：计算器、按端口结束进程、JSON / Base64 / URL / 时间戳转换。
- 文件搜索。
- 支持 Claude Code、Codex 之外的更多 agent。
- 脚本命令：Alfred Script Filter JSON 输出、自定义图标、带 `refreshTime` 的实时 `inline` 模式（这两个字段现在能解析，但还没用上）。
- 常驻插件（有自己的视图）和隔离的插件运行环境。
- 剪贴板固定条目、顺序粘贴。

## 参与贡献

欢迎提 issue 和 PR。动手之前请先看 [CONTRIBUTING.md](CONTRIBUTING.md)；[docs/architecture.md](docs/architecture.md)（英文）讲了各模块是怎么接进来的。

## 协议

[MIT](LICENSE) © 2026 Peter Chen
