# Helmp MCP 服务：让 coding agent 开发和使用 Helmp

[English](mcp.md)

`helmp mcp` 是内置在 Helmp 可执行文件里的 [MCP](https://modelcontextprotocol.io) 服务。Claude Code、Codex、Cursor 等 coding agent 通过 stdio 启动它之后，可以：

- **替你开发 Helmp 功能**：你说想要什么（「把选中文字发到飞书机器人」），agent 把它做成一个[插件](plugins.zh-CN.md)：设计、生成模板、校验、演练测试、安装、给你看，最后问你要不要上传到插件中心；
- **使用 Helmp**：像在面板里输入一样搜索，并执行结果。凡是有副作用的操作，都先在 Helmp 面板里问你。

- [连接 agent](#连接-agent)
- [工具、资源与提示词](#工具资源与提示词)
- [哪些操作要你确认](#哪些操作要你确认)
- [隐私](#隐私)
- [对话示例](#对话示例)
- [细节](#细节)

## 连接 agent

在 **设置 › Agent › 连接到 coding agent** 里，点 Claude Code、Codex 或 Cursor 旁边的「连接」，然后重启那个 agent。每一行会显示是否已连接、配置写在哪个文件。也可以在终端里：

```bash
helmp mcp install claude    # 或 codex、cursor
helmp mcp status            # 各 agent 是否已连接
```

| Agent | Helmp 写什么 |
|---|---|
| Claude Code | 装了 `claude` 命令时运行 `claude mcp add --scope user helmp -- <helmp> mcp`；否则在 `~/.claude.json`（设了 `CLAUDE_CONFIG_DIR` 时是其下的 `.claude.json`）里加 `mcpServers.helmp` |
| Codex | `~/.codex/config.toml`（或 `$CODEX_HOME/config.toml`）里的 `[mcp_servers.helmp]` |
| Cursor | `~/.cursor/mcp.json` 里的 `mcpServers.helmp` |

`<helmp>` 是正在运行的 Helmp 可执行文件路径，如 `/Applications/Helmp.app/Contents/MacOS/helmp`。改文件之前先复制一份 `<文件>.helmp-backup`；只动 `helmp` 这一项，别的服务、设置和 TOML 注释原样保留。已经连接且没有变化时再点一次什么也不改；Helmp 挪了位置时这一行会提示，点「重新连接」更新路径。只有你点「连接」或运行 `helmp mcp install` 时 Helmp 才会写这些文件。

其他 MCP 客户端也能用：把 `<helmp> mcp` 配成一个 stdio 服务即可。

## 工具、资源与提示词

| 工具 | 作用 |
|---|---|
| `helmp_plugin_guide` | 插件开发指南：agent 的工作流、[插件文档](plugins.zh-CN.md)、测试方法，以及作为完整示例的自带插件 |
| `helmp_plugin_scaffold` | 用模板（`quicklink`、`http`、`script`、`list`、`starter`）生成一个能通过校验的插件目录，默认在 `~/Library/Application Support/Helmp/plugins-dev/<id>`，也可以指定 |
| `helmp_plugin_validate` | 与 Helmp 加载时完全相同的校验：列出每个问题及其字段路径，或者列出命令和你将要确认的权限 |
| `helmp_plugin_install` | 把目录复制进 Helmp 的插件文件夹。**插件保持停用**，直到你在「设置 › 插件」里看过权限并打开它；Helmp 在运行时会直接翻到这个插件 |
| `helmp_plugin_test` | 无界面运行一个命令。演练（默认）：在隔离的沙箱里运行，打开、复制、粘贴和发往真实主机的请求只以 `[dry-run] …` 回报；发往 `127.0.0.1` / `localhost` 的请求真的发，agent 可以对着本地假服务测；密钥只在这一次运行里使用。`dry_run: false` 真的运行已安装且启用的插件，要你在面板里确认 |
| `helmp_plugin_list` | 自带与用户插件的状态（已启用、已停用、待确认权限、出错）、错误、权限和命令 |
| `helmp_plugin_submit` | 把插件上传到公开的 Helmp 插件中心（`helmp plugins submit <目录> --yes`），返回 PR 链接。agent 必须先问你，你明确同意后才能带 `user_confirmed: true` 调用 |
| `helmp_search` | 像面板一样搜索：应用、计算器与换算、时区、开发者工具、词典、系统命令、窗口、插件命令……每个结果都标明执行它要不要你确认 |
| `helmp_run` | 执行一个结果。纯计算直接返回文字，其余都先问你（见下） |

资源：`helmp://docs/plugins` 与 `helmp://docs/plugins.zh-CN`（同一份指南，附英文或中文插件文档）。提示词：`create_helmp_plugin`，参数 `request`，带 agent 走完 设计 → 生成模板 → 写文件 → 校验 → 演练测试 → 安装 → 给你看 → 问你是否上传。

## 哪些操作要你确认

| 操作 | 确认 |
|---|---|
| 纯计算：计算器、单位与汇率换算、时间、开发者工具、查词典、翻译、对 agent 指定的图片文件做 OCR | 不用。agent 拿到文字，不会复制或粘贴任何东西 |
| 其余一切：打开应用 / 文件 / 链接、复制、粘贴、结束进程、移动窗口、开关、系统命令、插件的 HTTP 请求与脚本、任何次级操作 | Helmp 面板显示 **「Claude Code 想执行：打开「npm react」」**，附查询词和表单内容。**回车**允许，**Esc** 拒绝；60 秒内不处理视为拒绝，之后再按回车也不会执行 |
| 启用 agent 安装的插件 | 只有你能在「设置 › 插件」里看过权限后打开。MCP 服务从不写授权 |
| 上传插件 | agent 必须在对话里问你；没有 `user_confirmed: true` 工具直接拒绝 |

规则是保守的：只有上面这些计算不用确认，点名执行次级操作（比如翻译结果的「替换选中文字」）一律要确认。MCP 服务进程本身执行任何动作都处于演练模式，即使某个结果被误判，也只会回报、不会真的执行；真正的副作用只会在你按下回车之后、由 Helmp 主程序执行。Helmp 没在运行时，agent 会被告知请你先打开 Helmp。

确认请求经 Helmp 数据目录里的 Unix 套接字传递（`agent.sock`，权限 0600），只接受你自己这个用户的连接。

## 隐私

- 剪贴板历史、Agent 会话记录和首页的「最近使用」默认不给 agent：`helmp_search` 不返回它们（结果里会说明隐藏了几条），`helmp_run` 拒绝执行。要放开，在 `~/Library/Application Support/Helmp/config.json` 里设 `"mcp_server": {"expose_private": true}`。
- 插件里的 `{{clipboard}}` 在 MCP 服务和演练中是 `[clipboard]`，服务从不读取你的剪贴板。
- 插件演练用一次性的数据目录：不读你的钥匙串，agent 为测试传入的密钥不会被保存。
- 只在 App 里才联网查询的内容（汇率、公网 IP）MCP 服务查不到。

## 对话示例

> **你：** 帮我做一个把选中文字发到飞书机器人的插件
>
> **Agent：** *（读 `helmp_plugin_guide`）* 方案：插件 `com.you.feishu-bot`，命令「发到飞书」（关键词 `fs`），带一个用输入内容预填的文本框；POST `{"msg_type":"text","content":{"text":…}}` 到你机器人的 webhook，地址存在钥匙串里，只允许 `open.feishu.cn`。
> *（`helmp_plugin_scaffold` kind `http` → 改 `plugin.json` → `helmp_plugin_validate`：通过，权限「访问网络：open.feishu.cn」）*
> *（起一个本地假服务，`helmp_plugin_test` 带 `secrets: {"webhook": "http://127.0.0.1:8765/hook"}` → 假服务收到了 JSON，Helmp 显示「已发送到飞书」）*
> *（`helmp_plugin_install` → 状态：已停用）*
> 装好了。Helmp 已经打开「设置 › 插件」里的 **飞书机器人**：确认它只需要访问 open.feishu.cn，打开开关，把 webhook 地址粘进去。之后选中文字、唤出 Helmp、输入 `fs` 就能发。
> 要把它上传到公开的 Helmp 插件中心给别人用吗？
>
> **你：** 不用了
>
> **Agent：** 好的，只留在本地。

## 细节

- 传输：stdio 上按行分隔的 JSON-RPC 2.0。协议版本 `2025-11-25`、`2025-06-18`、`2025-03-26`、`2024-11-05`；客户端要别的版本时回最新版。工具结果包含文字和 `structuredContent`；工具层面的失败（插件无效、被拒绝、超时）是 `isError: true` 的结果。
- 列插件、查状态时每次都从磁盘读 Helmp 的 `config.json`，agent 运行期间你启用的插件会立刻显示为已启用。
- 搜索与 `helmp query` 一样快；第一次搜索时建一次索引（应用、文件……）。
- 测试：[`tests/e2e_mcp_server.rs`](../tests/e2e_mcp_server.rs) 在隔离环境里（临时数据目录与 `HOME`、演练模式、假的 `claude` 命令）通过 stdio 驱动 `helmp mcp`，不会碰你的 agent 配置、钥匙串和剪贴板。
