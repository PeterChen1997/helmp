# 隐私说明

[English](privacy.md) · 简体中文

一句话：Helmp 没有账号、没有遥测、没有崩溃上报，也不检查更新。它保存的东西都在你 Mac 上的同一个目录里。唯一会发出的网络请求，是你为翻译配置的模型调用。

本页描述的是当前代码的实际行为。如果发现对不上，请开 issue 告诉我们。

## Helmp 保存了什么

所有文件都在 `~/Library/Application Support/Helmp/`（设置了 `HELMP_DATA_DIR` 时用它）：

| 路径 | 内容 | 怎么删 |
|---|---|---|
| `config.json` | 你的设置：快捷键、启用的模块、模型的名称 / 地址 / 模型名、剪贴板与截图选项、额外的脚本目录。**不含任何 API Key。** | 删掉文件即恢复默认设置 |
| `history.json` | 首页「最近使用」：最多 30 条，每条包括标题、副标题、图标，以及**当时你输入的查询词** | 删掉文件 |
| `clipboard.sqlite` | 剪贴板历史：文本、文件路径、复制来源 App 的名称与 bundle id、复制时间。默认不限条数，可以设置保留天数 | 「设置 › 剪贴板」里清空，或删掉文件 |
| `clipboard-images/` | 复制过的图片，按内容哈希命名 | 随剪贴板历史一起清除 |
| `screenshots/` | 「截图并保存」的输出（可以改到别的目录） | 直接删 |
| `ocr/` | 识别结果旁边展示的最近几张截图，更早的会自动删除 | 删掉目录 |
| `scripts/` | 你的脚本命令 | 由你自己管理 |
| `icon-cache/` | 应用与文件图标渲染成的 PNG | 随时可删 |

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

## 会读取但不保存的内容

- **应用。** `/Applications`、`/System/Applications`、`~/Applications` 里各个 App 的 `Info.plist` 和本地化文件，用来在内存里建搜索索引。
- **Agent 会话。** 只读，Helmp 运行期间每秒轮询一次：
  - Claude Code：`~/.claude/sessions/*.json`；会话没有标题时读 `~/.claude/projects/*/<会话>.jsonl` 的最后 64 KB；Claude 桌面 App 在 `~/Library/Application Support/Claude/claude-code-sessions/` 下的会话文件。支持 `CLAUDE_CONFIG_DIR`。
  - Codex：`~/.codex/thread-writer-locks/`（判断会话是否还活着）、以只读方式打开的 `state_5.sqlite`，以及会话日志的最后 64 KB。支持 `CODEX_HOME`。
  - 这些会话的进程信息（`proc_pidinfo`）和工作目录的 Git 分支。

  Helmp 从不往这些目录写东西，不删除任何文件，也不安装 hooks。每个来源都可以在「设置 › Agent」里单独关闭。
- **选中的文字。** 只有在你打开翻译、输入框为空、并且已经接入模型时，Helmp 才会通过辅助功能接口向前台 App 要选中的文字，在内存里缓存约 1.5 秒。
- **剪贴板。** 每 300 毫秒检查一次，用来记录历史（规则见上）。可以在「设置 › 剪贴板」里整个关掉。

## 网络

- **翻译**会把你要翻译的文字和一段简短的指令，发到你配置的接口：Anthropic、OpenAI 兼容地址，或者 Ollama（默认 `http://localhost:11434`）。设置里的「测试连接」也会向同一个接口发一个很小的请求。每个请求 30 秒超时。Helmp 没有自己的服务器，这些请求不经过任何中转。
- 除此之外没有任何网络请求：没有统计、遥测、崩溃上报，也不检查更新。菜单里的「检查更新…」目前只会显示当前版本。

由*你主动触发*、可能访问网络的操作：从剪贴板历史里打开链接、跳转 Claude / ChatGPT 桌面 App 的深链，以及你运行的脚本命令。

## 权限

| 权限 | 用途 |
|---|---|
| 辅助功能 | 模拟 <kbd>⌘V</kbd> 粘贴到前台 App；翻译时读取、替换选中的文字 |
| 录屏 | 区域截图（`screencapture -i`） |
| 自动化（iTerm2、终端） | 跳转 Agent 会话时选中对应的标签页 |

Helmp 检查权限时不会触发系统弹框。缺少权限的功能会给出提示，而不是悄悄失败。

## 卸载

1. 退出 Helmp，删除 `Helmp.app`。
2. 删除 `~/Library/Application Support/Helmp/`。
3. 在「钥匙串访问」里删除名称为 `dev.helmp.launcher` 的条目。
4. 可选：在「系统设置 › 隐私与安全性」（辅助功能、录屏、自动化）和「登录项」里移除 Helmp。
