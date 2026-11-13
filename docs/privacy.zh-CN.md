# 隐私说明

[English](privacy.md) · 简体中文

一句话：Helmp 没有账号、没有遥测、没有崩溃上报。它保存的东西都在你 Mac 上的同一个目录里。会发出的网络请求只有这几类：发往你为翻译配置的模型接口；你运行已启用插件的命令时，发往该插件声明的主机；以及（除非你关掉）每天一次读取 Helmp 的更新清单（只是下载一个版本文件，不带任何关于你的信息）。你添加的 MCP 服务器是你自己选的程序或服务：Helmp 启动或连接它们，并把你运行的工具的参数发给它们；它们还会访问什么，由它们自己决定。

本页描述的是当前代码的实际行为。如果发现对不上，请开 issue 告诉我们。

## Helmp 保存了什么

所有文件都在 `~/Library/Application Support/Helmp/`（设置了 `HELMP_DATA_DIR` 时用它）：

| 路径 | 内容 | 怎么删 |
|---|---|---|
| `config.json` | 你的设置：界面语言、快捷键、启用的模块和别名、模型的名称 / 地址 / 模型名、剪贴板与截图选项、网页搜索引擎（含自定义的网址模板）、额外的脚本目录。**不含任何 API Key。** | 删掉文件即恢复默认设置 |
| `history.json` | 首页「最近使用」：最多 30 条，每条包括标题、副标题、图标，以及**当时你输入的查询词** | 删掉文件 |
| `agent-completions.json` | Agent 会话的已读记录，只保存会话 ID 和看过的完成轮次，不保存消息内容 | 退出 Helmp 后删除文件可重置未读状态 |
| `clipboard.sqlite` | 剪贴板历史：文本、文件路径、复制来源 App 的名称与 bundle id、复制时间。内容清理目标为 50 MB，保护最近 72 小时的历史和置顶条目。单次复制超过 50 MB 只记录时间、类型和大小到日志 | 「设置 › 剪贴板」里清空，或删掉文件 |
| `clipboard-images/` | 复制过的图片，按内容哈希命名 | 随剪贴板历史一起清除 |
| `screenshots/` | 「截图并保存」的输出（可以改到别的目录） | 直接删 |
| `ocr/` | 面板里识别结果旁边展示的最近几张图，更早的会自动删除 | 删掉目录 |
| `capture-history/` | 截图历史：**最近 10 张**复制、保存、钉图或识别过文字的截图的最终图（含标注），每张一个 `<时间>.png` 加一个小的 `<时间>.json`（时间、像素尺寸、怎么用掉的、在屏幕上的位置）。新的进来时更早的自动删除；取消的截图不记 | 在「截图历史」里删除，或关掉「设置 › 截图 › 保留截图历史」（同时删掉整个目录） |
| `scripts/` | 你的脚本命令 | 由你自己管理 |
| `icon-cache/` | 应用与文件图标渲染成的 PNG | 随时可删 |
| `fx.json` | 最近一次取到的欧洲央行汇率（日期和汇率表），货币换算用 | 随时可删，联网后会重新下载 |
| `snippets.json` | 你的文本片段（名称、关键词、内容、标签） | 「设置 › 片段」里删除，或删掉文件 |
| `plugins/` | 你安装或新建的插件（每个文件夹里一个 `plugin.json`）。插件的密钥不在这里，存在钥匙串 | 「设置 › 插件」里管理，或删掉文件夹 |
| `mcp/` | MCP 客户端的数据：`tools.json`（服务器工具列表的缓存）、`logs/<服务器>.log`（每个服务器的 stderr 和启动 / 停止 / 出错记录，超过 1 MB 自动截短）、`images/`（工具返回的图片，7 天后删除）。服务器设置在 `config.json`，令牌在钥匙串 | 随时可删，工具列表会重新读取 |
| `emoji-recent.json`、`colors/` | 最近用过的 emoji，以及换算或取过的颜色的色块图 | 随时可删 |
| `emoji-recent.json` | 最近粘贴或复制过的 emoji，最多 24 个 | 删除文件 |
| `colors/` | 转换或取到的颜色的小色块 PNG | 随时可删 |

如果你用过叫「唤（huan）」的早期版本，Helmp 首次启动时会把 `~/Library/Application Support/huan/` 整体移到新目录，只做一次。

### 剪贴板规则

- 密码管理器标记为「隐藏」的内容（`org.nspasteboard.ConcealedType`）不记录。默认开启，可在设置里关闭。
- 标记为「临时 / 自动生成」的内容一律不记录，包括 Helmp 自己写回剪贴板的条目。
- 忽略列表里的 App 复制的内容不记录，默认包含「密码」App。
- 可以关闭图片记录。

## 钥匙串

模型的 API Key 以「通用密码」的形式存在 macOS 登录钥匙串里：

- 服务名：`dev.helmp.launcher`
- 账户名：`model:<模型 id>`

在 API Key 输入框失焦、按回车或测试连接时写入，永远不会出现在 `config.json` 里。可以在「钥匙串访问」里搜 `dev.helmp.launcher` 删除，或者在设置里删掉对应的模型。

[插件](plugins.zh-CN.md)的密钥类设置（比如 flomo 的 webhook 地址）也这样保存，服务名 `dev.helmp.plugins`，账户 `<插件 id>/<设置 id>`。在输入框失焦或按回车时写入，设置里不会再显示，不写进 `config.json`，在提示信息里打码成 `••••`。

钥匙串条目和保存它的 App 的签名绑定。自己构建的（ad-hoc 签名）Helmp 存下的 Key，签名发布版可能读不到；遇到这种情况 Helmp 会直接说明，在「设置 › 模型」里重新保存一次 Key，就会替换掉旧条目。

[MCP 服务器](mcp-client.zh-CN.md)的密钥类环境变量和请求头（名字含 TOKEN、KEY、SECRET、PASSWORD、AUTH…，以及 `Authorization`）存在服务名 `dev.helmp.mcp` 下，账户 `<服务器名>/env:<变量名>` 或 `<服务器名>/header:<请求头名>`，从别的客户端导入时发现的也一样。`config.json` 里只记它们的名字。删除服务器时一并删除。

## 会读取但不保存的内容

- **应用。** `/Applications`、`/System/Applications`、`~/Applications` 里各个 App 的 `Info.plist` 和本地化文件，用来在内存里建搜索索引。
- **其他 MCP 客户端的配置。** 只在你点「设置 › MCP › 从 Claude Code / Codex / Cursor 导入」（或运行 `helmp mcp-client import`）时读取：`~/.claude.json`（以及 `$CLAUDE_CONFIG_DIR/.claude.json`）、`$CODEX_HOME/config.toml` 或 `~/.codex/config.toml`、`~/.cursor/mcp.json`，以及你指定的项目目录里的 `.mcp.json` / `.cursor/mcp.json`。只读，只复制你勾选的服务器。
- **Agent 会话。** 只读，Helmp 运行期间每秒轮询一次：
  - Claude Code：`~/.claude/sessions/*.json`；会话没有标题时读 `~/.claude/projects/*/<会话>.jsonl` 的最后 64 KB；Claude 桌面 App 在 `~/Library/Application Support/Claude/claude-code-sessions/` 下的会话文件。支持 `CLAUDE_CONFIG_DIR`。
  - Codex：`~/.codex/thread-writer-locks/`（判断会话是否还活着）、以只读方式打开的 `state_5.sqlite`，以及会话日志的最后 64 KB。支持 `CODEX_HOME`。
  - 这些会话的进程信息（`proc_pidinfo`）和工作目录的 Git 分支。

  Helmp 从不往这些目录写东西，不删除任何文件，也不安装 hooks。每个来源都可以在「设置 › Agent」里单独关闭。
- **选中的文字。** 每次打开翻译、且没有带着文字进入时（快捷键、菜单栏、前缀，或不带 `q` 的 `helmp://scope/translate` 链接），Helmp 会通过辅助功能接口向前台 App 要选中的文字。有些 App（Chrome、Electron 应用等）不通过这个接口提供选中内容；对这类 App，并且只在打开 Helmp 时它还在前台的情况下，Helmp 会替你按一下 <kbd>⌘C</kbd>，读出复制到的文字，再把剪贴板原来的内容放回去。这个过程中剪贴板历史暂停记录，复制的选中内容不会进历史。系统无法读回的数据（比如文件承诺）没法还原。App 明确表示没有选中内容时，Helmp 不会按 <kbd>⌘C</kbd>。没有辅助功能权限时以上都不会发生。读到的文字只放在内存里，下次打开翻译时被替换。
- **翻译历史。** 最近 20 次翻译（原文、译文、语言、模型）留在内存里，用于 <kbd>↑</kbd> 翻出历史，以及同样的请求不再重复调用模型。从不写盘，退出 Helmp 就没了。
- **快捷键设置。** 在设置里录制快捷键时，Helmp 会只读地读取 `~/Library/Preferences/com.apple.symbolichotkeys.plist`，找出会冲突的系统快捷键。
- **用于识别的图片。** 截图、剪贴板图片和传给 `ocr` 的文件，都在本机用 Apple Vision 识别，二维码和条码检测也一样。
- **冻结的屏幕画面。** 截图框选期间，各块屏的画面暂存在系统临时目录（`$TMPDIR/helmp-capture-<pid>/`），框选界面关闭时删除。钉图只在内存里；完成的截图的最终图另外记在截图历史里（`capture-history/`，见上表），可以关掉。
- **剪贴板。** 每 300 毫秒检查一次，用来记录历史（规则见上）。可以在「设置 › 剪贴板」里整个关掉。

## 网络

- **翻译**会把你要翻译的文字和一段简短的指令，发到你配置的接口：Anthropic、OpenAI 兼容地址，或者 Ollama（默认 `http://localhost:11434`）。翻译在你停止输入片刻后自动开始，所以每次停顿都可能发出一个请求，而不只是按回车时；完全相同的请求直接用内存里的历史结果。设置里的「测试连接」也会向同一个接口发一个很小的请求。每个请求 30 秒超时。Helmp 没有自己的服务器，这些请求不经过任何中转。
- **「设置 › 模型」**会检查你添加的每个 Ollama 接口是否在运行（`GET <接口地址>/api/tags`，2 秒超时）。
- **你启用的插件**只在你运行它的命令时发请求，而且只发往它权限里列出的主机（启用前在「设置 › 插件」里能看到）。比如自带的 flomo 插件在你回车时把内容发到你的 flomo webhook。发出前 Helmp 会再核对一次主机名，不跟随重定向。插件里的脚本以你的身份运行，能做的事和脚本命令一样多。
- **MCP 服务器**：你添加的（或已启用插件带的）服务器，是 Helmp 以你的身份启动、不受沙盒限制的程序（stdio），或它连接的网址（HTTP）。你运行某个工具时，Helmp 把你填的参数发给这个服务器；也会向它要工具列表（刚添加时、列表变化时、重新连上时）。stdio 服务器拿到你配置的环境变量（含钥匙串里的密钥），HTTP 服务器拿到你配置的请求头。服务器拿这些做什么、还会访问哪些主机，由服务器自己决定，所以只添加你信任的服务器。服务器第一次用到才启动，空闲 10 分钟后停止（标了「常驻」的除外）。精选里用 `npx` / `uvx` 启动的服务器，第一次运行会从 npm / PyPI 下载包。
- **货币换算**在启动时如果本地汇率超过一天，会下载欧洲央行每日参考汇率（`GET https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml`，8 秒超时），之后每小时检查一次、一天最多下载一次。请求里不带任何关于你或你输入内容的信息。
- **开发工具 › 公网 IP** 只在你对这一行按回车时，向 `https://api.ipify.org`（或设置 `devtools.public_ip_url` 里的地址）查询你的公网 IP。
- **检查更新**会下载版本清单 `https://blog.peterchen97.cn/helmp/releases/latest.json`，取不到时改为 `https://github.com/PeterChen1997/helmp/releases/latest/download/latest.json`（15 秒超时）。打开「设置 › 通用 › 自动检查更新」时，启动 20 秒后检查一次，之后每天一次；关掉后只在你点「检查更新」时检查。请求是普通的 `GET`，User-Agent 为 `Helmp/<版本> (macOS updater)`，不带任何标识符，也不带你的使用情况。安装包只在你点「下载并安装」后才下载，地址来自清单（同样是这两个主机）。安装时新版本解压到 App 旁边的隐藏目录 `.helmp-update-…`，助手脚本放在临时目录（`$TMPDIR/helmp-update-<pid>/`），解压目录和脚本用完即删，助手的 `update.log` 留在那里，直到 macOS 清理临时文件；下载的 dmg 存在「下载」文件夹。详见 [updates.zh-CN.md](updates.zh-CN.md)。
- 除此之外没有任何网络请求：没有统计、遥测、崩溃上报。

插件中心（「设置 › 插件 › 浏览插件中心」、`helmp plugins available` / `install`、`install-plugin` 链接）只在你使用时才从 `cdn.jsdelivr.net` 下载插件索引和插件文件，Helmp 不会在后台检查。见 [plugin-center.zh-CN.md](plugin-center.zh-CN.md)。

由*你主动触发*、可能访问网络的操作：网页搜索（回车后由默认浏览器打开搜索网址，Helmp 自己不发请求，副标题里就是完整网址）、从剪贴板历史或二维码里打开链接、跳转 Claude / ChatGPT 桌面 App 的深链，以及你运行的脚本命令。

翻译里的「朗读译文」用的是 macOS 自带的 `say` 命令。`helmp://` 深链接在本机处理。

## 权限

| 权限 | 用途 |
|---|---|
| 辅助功能 | 模拟 <kbd>⌘V</kbd> 粘贴到前台 App；翻译时读取选中的文字（必要时模拟 <kbd>⌘C</kbd>）、替换选中内容；「锁屏」代按 <kbd>⌃⌘Q</kbd> |
| 录屏 | 截图和截图识别文字时通过 ScreenCaptureKit 冻结屏幕，必要时回退到系统截图工具 |
| 自动化（iTerm2、终端） | 跳转 Agent 会话时选中对应的标签页 |
| 自动化（System Events、访达） | 系统命令：锁屏、切换深色模式、重启、关机、注销通过 System Events 执行，清空废纸篓通过访达执行 |

Helmp 检查权限时不会触发系统弹框。缺少权限的功能会给出提示，而不是悄悄失败。

## 卸载

1. 退出 Helmp，删除 `Helmp.app`。
2. 删除 `~/Library/Application Support/Helmp/`。
3. 在「钥匙串访问」里删除名称为 `dev.helmp.launcher` 或 `dev.helmp.plugins` 的条目。
4. 可选：在「系统设置 › 隐私与安全性」（辅助功能、录屏、自动化）和「登录项」里移除 Helmp。
