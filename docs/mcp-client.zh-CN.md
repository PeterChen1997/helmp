# MCP 服务器（Helmp 作为 MCP 客户端）

[English](mcp-client.md)

Helmp 可以连接 [Model Context Protocol](https://modelcontextprotocol.io) 服务器，把它们提供的每个工具变成启动器命令。文件系统、抓网页、Git、Playwright、Context7 这些服务器加一次，之后输入工具名就能搜到：填好自动生成的表单，回车，结果显示在面板里。

> 要的是反方向（让编程 Agent 调用 Helmp）？那是 `helmp mcp`，另一个功能。本文的命令行都在 `helmp mcp-client` 下。

## 添加服务器

打开 **设置 › MCP**（或在启动器里输入「添加 MCP 服务器」）。四种方式：

| 方式 | 说明 |
|---|---|
| **从 Claude Code / Codex / Cursor 导入** | 只读、不改动：`~/.claude.json`（也看 `$CLAUDE_CONFIG_DIR/.claude.json`）、`$CODEX_HOME/config.toml`（缺省 `~/.codex/config.toml` 的 `[mcp_servers.*]`）、`~/.cursor/mcp.json`。填了项目目录时再读它的 `.mcp.json`、`.cursor/mcp.json` 和 `.claude.json` 里这个项目的配置。结果是一个勾选清单；已存在的、用旧版 HTTP+SSE 传输的不能勾。 |
| **精选** | 编译进来的一小份目录：Filesystem、Fetch、Git、Memory、Sequential Thinking、Time（来自 [modelcontextprotocol/servers](https://github.com/modelcontextprotocol/servers)），以及 [Playwright](https://github.com/microsoft/playwright-mcp)、[Context7](https://github.com/upstash/context7)。添加前会列出要运行的完整命令（如 `npx -y @modelcontextprotocol/server-filesystem ~/Documents`）、要先装好的东西（`npx` 要 Node.js，`uvx` 要 [uv](https://docs.astral.sh/uv/)）和要填的值。 |
| **粘贴 JSON** | 各家 README 里常见的 `{"mcpServers": {...}}`，或 `{"名字": {...}}`。 |
| **添加服务器** | 手动填：名字，本地命令（stdio：命令、参数、工作目录、环境变量）或远程地址（Streamable HTTP：地址、请求头）。 |

每个服务器有启用开关、**常驻** 开关、**编辑**、**重启**、**查看日志**、**删除**（点两次）。列表里显示状态（运行中 / 已停止 / 出错 + 最近的错误）、工具数和协商出的协议版本。

### 密钥

名字里含 `TOKEN`、`KEY`、`SECRET`、`PASSWORD`、`PASS`、`AUTH`、`CREDENTIAL`、`COOKIE`、`SESSION`、`PAT`、`BEARER`（整词：`GITHUB_TOKEN`、`X-Api-Key`、`Authorization`）的环境变量和请求头，保存时自动存进 macOS 钥匙串，导入时发现的也一样。`config.json` 里只记键名（`"secrets": ["env:GITHUB_TOKEN"]`）。钥匙串条目：服务 `dev.helmp.mcp`，账户 `<服务器名>/env:<变量名>` 或 `<服务器名>/header:<请求头名>`。设置页从不显示已存的值；重新写一行 `名字=值` 可以替换，点「清除」删除。删除服务器会一并删除它的钥匙串条目。

### config.json

```json
{
  "mcp": {
    "idle_secs": 600,
    "servers": [
      {"name": "fs", "transport": {"type": "stdio", "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me/Documents"]}},
      {"name": "github", "transport": {"type": "stdio", "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-github"], "env": {}},
        "secrets": ["env:GITHUB_PERSONAL_ACCESS_TOKEN"]},
      {"name": "context7", "enabled": true, "resident": false, "timeout_secs": 60,
        "transport": {"type": "http", "url": "https://mcp.context7.com/mcp", "headers": {}}}
    ]
  }
}
```

## 使用工具

- 启用的服务器的每个工具都是「MCP」分组里的一个结果，右侧写服务器名。按工具的标题和名字匹配（中文标题支持拼音和首字母，`_`、`-` 当空格），三个字以上的查询也匹配描述。
- 输入服务器名加空格（`github `）列出这个服务器的全部工具；输入 `mcp ` 浏览全部工具。
- 工具的输入 schema 变成表单：字符串是文本框（长文本多行），`enum` 是下拉，`boolean` 是「是 / 否」下拉，`number` / `integer` 提交时检查，必填项标出来并排在前面。数组、对象、`anyOf` 字段用 JSON 文本框；整个 schema 不是普通 object 时，所有参数用一个 JSON 文本框。
- 标了 `destructiveHint: true`，或 `readOnlyHint: false` 且没标 `openWorldHint: false` / `destructiveHint: false` 的工具，先出确认页（列出参数）。没有标注的直接运行。
- 结果：文字可选中复制；图片存到 `<数据目录>/mcp/images/`（保留 7 天）并显示；`structuredContent` 显示为格式化的 JSON（文字已经是同一份 JSON 时不重复）；内嵌资源显示为代码，资源链接显示为链接。`isError` 显示成错误，带上服务器给的原因。<kbd>⌘C</kbd> 复制结果，「重新运行」再跑一次。

## 生命周期

- **懒启动**：第一次用到才启动——调用工具，或者服务器还没有工具缓存（刚添加）时启动一次读工具列表。工具列表缓存在 `<数据目录>/mcp/tools.json`，命令、参数、地址、密钥键名不变就一直用，所以搜索从不等服务器。
- **常驻**：随 Helmp 启动、打开面板时确保在运行，不因空闲停止。
- **空闲关闭**：其余服务器 `idle_secs`（默认 600）秒没用就停掉。
- **崩溃**：启动失败或意外退出标为「出错」，按 2、4、8、16、32、60 秒退避重试。常驻的自动重启，其余的等下次用到。设置里的「重启」清零退避。
- **超时**：握手 90 秒（`npx` / `uvx` 第一次运行要下载），`tools/list` 30 秒，`tools/call` 用 `timeout_secs`（默认 60）。超时后发 `notifications/cancelled`。
- **工具列表变化**：收到 `notifications/tools/list_changed` 就作废缓存、重新读取；每次重新连上也会重新读。
- **日志**：每个服务器的 stderr、启动 / 停止和错误写进 `<数据目录>/mcp/logs/<服务器>.log`（超过 1 MB 只留后一半）。「查看日志」打开它。
- 停止 stdio 服务器：先关 stdin，再向它的进程组发 SIGTERM，最后 SIGKILL，`npx` 拉起的 `node` 也一起结束。退出 Helmp 会停掉所有服务器。
- 图形界面启动的 App 拿到的 `PATH` 很短；Helmp 会补上存在的 `/opt/homebrew/bin`、`/usr/local/bin`、`~/.local/bin`、`~/.cargo/bin`、`~/.bun/bin`、`~/.volta/bin`、`~/.deno/bin`，`npx`、`uvx` 才找得到。其他位置请写完整路径。

## 协议

JSON-RPC 2.0，走 **stdio**（换行分隔）和 **Streamable HTTP**（POST，`Accept: application/json, text/event-stream`；支持 JSON 与 SSE 回复、`Mcp-Session-Id`、握手后带 `MCP-Protocol-Version`、结束时 `DELETE` 会话）。发起协议版本 `2025-11-25`，接受服务器回的版本（`2025-06-18`、`2025-03-26`、`2024-11-05` 或更新的，工具相关消息兼容）。用到 `initialize`、`tools/list`（分页）、`tools/call`，回应 `ping`，处理 `notifications/tools/list_changed`。HTTP 下通知只在回复的事件流里收到（不常开 GET 流）。暂不支持：旧版 HTTP+SSE 传输、OAuth 登录（请用令牌请求头）、resources、prompts、sampling、elicitation。

没有用官方 Rust SDK（`rmcp`）而是手写了一个小客户端：`rmcp` 需要 tokio 运行时和 reqwest，而 Helmp 全程是普通线程加 `ureq`。

## 插件可以带服务器

[插件](plugins.zh-CN.md)可以声明 `mcpServers`；插件启用时这些服务器才会运行，工具 id 为 `mcp:<插件 id>/<名字>/<工具>`。

```json
{
  "schemaVersion": 1,
  "id": "com.example.github-tools",
  "name": "GitHub Tools",
  "version": "1.0.0",
  "author": "You",
  "preferences": [{"id": "token", "type": "secret", "title": "GitHub token", "required": true}],
  "mcpServers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": {"GITHUB_PERSONAL_ACCESS_TOKEN": "{{pref.token}}"}
    }
  }
}
```

- `command`（在 `PATH` 里找的程序名，或插件目录里的 `./路径`；可带 `args`、`env`、相对插件目录的 `cwd`）与 `url`（写死的 `https://` / `http://` 地址；可带 `headers`）二选一。
- `env`、`headers` 的值只能用 `{{pref.<id>}}`；密钥写成 `secret` 偏好设置，不要写进清单。
- 带 `mcpServers` 的插件可以没有 `commands`。
- 启用前列出的权限：每个命令一条「运行本地程序：`npx`」，每个地址一条「访问网络：`主机`」。

## 命令行

```bash
helmp mcp-client list                         # 状态、名字、来源、工具数、命令或地址
helmp mcp-client add time -- uvx mcp-server-time
helmp mcp-client add docs --url https://mcp.example.com/mcp --header "Authorization: Bearer …"
helmp mcp-client add github --env GITHUB_TOKEN=… -- npx -y @modelcontextprotocol/server-github
helmp mcp-client add --json '{"mcpServers": {"mem": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-memory"]}}}'
helmp mcp-client tools time                   # 启动并列出工具（名字、标题、是否要确认、描述）
helmp mcp-client import --list                # 看看 Claude Code / Codex / Cursor 里有哪些（只读）
helmp mcp-client import [--from claude|codex|cursor] [--project <目录>] [名字...]
helmp mcp-client remove time
helmp query "echo"                            # 工具和其他结果一样能查到
helmp run mcp:time/get_current_time "current time" --form timezone=Asia/Shanghai
```

## 测试（演练模式）

设了 `HELMP_DRY_RUN=1` 时不启动真实服务器：对工具 `helmp run` 打印 `[dry-run] mcp-call <服务器> <工具> <参数>`，`helmp mcp-client tools` 打印 `[dry-run] mcp-start …`。只有测试模式（`HELMP_DRY_RUN=1` 加上隔离的 `HELMP_DATA_DIR`）下，命令或某个参数是这个数据目录里的文件的服务器才会真的运行；HTTP 服务器的地址是 `127.0.0.1`、`localhost`、`[::1]` 时才会真的连。`tests/e2e_mcp_client.rs` 就是这样驱动 `examples/mock_mcp_server.rs` 的。测试模式下钥匙串换成 `<数据目录>/test-keychain.json`。
