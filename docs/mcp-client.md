# MCP servers (Helmp as an MCP client)

[中文](mcp-client.zh-CN.md)

Helmp can connect to [Model Context Protocol](https://modelcontextprotocol.io) servers and turn every tool they offer into a launcher command. Add the filesystem, fetch, Git, Playwright or Context7 server once, and their tools show up when you type: fill in the generated form, press <kbd>↵</kbd>, and the result appears in the panel.

> Looking for the other direction (coding agents calling Helmp)? That's `helmp mcp`, a separate feature. The client commands in this document live under `helmp mcp-client`.

## Adding servers

Open **Settings › MCP** (or type "Add MCP Server" in the launcher). There are four ways:

| Way | What it does |
|---|---|
| **Import from Claude Code / Codex / Cursor** | Reads, without changing, `~/.claude.json` (also `$CLAUDE_CONFIG_DIR/.claude.json`), `$CODEX_HOME/config.toml` (default `~/.codex/config.toml`, the `[mcp_servers.*]` tables) and `~/.cursor/mcp.json`. Give a project folder to also read its `.mcp.json`, `.cursor/mcp.json` and that project's entry in `.claude.json`. You get a checklist; servers that already exist or use the legacy HTTP+SSE transport can't be picked. |
| **Featured** | A small built-in catalog: Filesystem, Fetch, Git, Memory, Sequential Thinking, Time (from [modelcontextprotocol/servers](https://github.com/modelcontextprotocol/servers)), [Playwright](https://github.com/microsoft/playwright-mcp) and [Context7](https://github.com/upstash/context7). Before adding, Helmp shows the exact command it will run (for example `npx -y @modelcontextprotocol/server-filesystem ~/Documents`), what has to be installed first (Node.js for `npx`, [uv](https://docs.astral.sh/uv/) for `uvx`), and the values you need to fill in. |
| **Paste JSON** | The `{"mcpServers": {...}}` block found in most server READMEs, or `{"name": {...}}`. |
| **Add Server** | A form: name, local command (stdio: command, arguments, working folder, environment) or remote URL (Streamable HTTP: URL, headers). |

Each server has an on/off switch, a **Keep running** switch, **Edit**, **Restart**, **View Log** and **Delete** (click twice). The list shows its state (Running / Stopped / Error with the last error), the number of tools, and the protocol version it negotiated.

### Secrets

Environment variables and headers whose names contain `TOKEN`, `KEY`, `SECRET`, `PASSWORD`, `PASS`, `AUTH`, `CREDENTIAL`, `COOKIE`, `SESSION`, `PAT` or `BEARER` (as a whole word: `GITHUB_TOKEN`, `X-Api-Key`, `Authorization`) are moved to the macOS Keychain when you save — including values found while importing. `config.json` keeps only the key names (`"secrets": ["env:GITHUB_TOKEN"]`). Keychain items use service `dev.helmp.mcp`, account `<server name>/env:<NAME>` or `<server name>/header:<Name>`. Settings never shows a stored value; type a new `NAME=value` line to replace it, or **Clear** to remove it. Deleting a server deletes its Keychain items.

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

## Using tools

- Every tool of an enabled server is a result in the **MCP** section, with the server name on the right. It matches the tool's title and name (Chinese titles by pinyin and initials too; `_` and `-` count as spaces) and, for queries of three or more characters, its description.
- Type the server name and a space (`github `) to list that server's tools, or `mcp ` to browse all tools.
- The tool's input schema becomes a form: strings are text fields (multi-line for long text), `enum` becomes a menu, `boolean` a Yes / No menu, `number` / `integer` are checked when you submit, required fields are marked and come first. Arrays, objects and `anyOf` fields get a JSON box; a schema that isn't a plain object gets one JSON box for all arguments.
- Tools annotated `destructiveHint: true`, or `readOnlyHint: false` without `openWorldHint: false` / `destructiveHint: false`, show a confirmation page with the arguments first. Tools without annotations run directly.
- Results: text is shown as selectable text, images are saved to `<data folder>/mcp/images/` (kept 7 days) and shown, `structuredContent` is shown as formatted JSON (unless the text already is that JSON), embedded resources as code, resource links as links. `isError` results are shown as errors with the server's message. <kbd>⌘C</kbd> copies the result; **Run Again** reruns it.

## Lifecycle

- **Lazy start.** A server starts the first time Helmp needs it: to call a tool, or once to read the tool list of a server that has none cached (right after you add it). Tool lists are cached in `<data folder>/mcp/tools.json` and reused until the server's command, arguments, URL or key names change, so searching never waits for a server.
- **Keep running** servers start with Helmp and when the panel opens, and never stop for idleness.
- **Idle shutdown.** Other servers stop after `idle_secs` (default 600) without use.
- **Crashes.** A server that fails to start or exits unexpectedly is marked Error and retried with a backoff of 2, 4, 8, 16, 32, then 60 seconds. Keep-running servers are restarted automatically; others on next use. **Restart** in Settings clears the backoff.
- **Timeouts.** The handshake has 90 seconds (the first `npx` / `uvx` run downloads packages), `tools/list` 30 seconds, `tools/call` `timeout_secs` (default 60). On a timeout Helmp sends `notifications/cancelled`.
- **Tool list changes.** `notifications/tools/list_changed` drops the cache and re-reads the tools. The list is also re-read whenever Helmp reconnects.
- **Logs.** Each server's stderr, start / stop and errors go to `<data folder>/mcp/logs/<server>.log` (trimmed to the last half when it passes 1 MB). **View Log** opens it.
- Stopping a stdio server closes its stdin, then sends SIGTERM and finally SIGKILL to its process group, so helpers such as the `node` started by `npx` stop too. Quitting Helmp stops all servers.
- GUI apps get a short `PATH`; Helmp adds `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`, `~/.cargo/bin`, `~/.bun/bin`, `~/.volta/bin` and `~/.deno/bin` when they exist, so `npx` and `uvx` are found. Use a full path otherwise.

## Protocol

Helmp speaks JSON-RPC 2.0 over **stdio** (newline-delimited) and **Streamable HTTP** (POST with `Accept: application/json, text/event-stream`; JSON and SSE replies, `Mcp-Session-Id`, `MCP-Protocol-Version` after the handshake, `DELETE` to end the session). It offers protocol version `2025-11-25` and accepts what the server answers (`2025-06-18`, `2025-03-26`, `2024-11-05`, or newer; the tool messages are compatible). It uses `initialize`, `tools/list` (with pagination), `tools/call`, answers `ping`, and handles `notifications/tools/list_changed`. Over HTTP, notifications only arrive inside a reply stream (Helmp doesn't keep a GET stream open). Not supported yet: the legacy HTTP+SSE transport, OAuth sign-in (use a token header instead), resources, prompts, sampling and elicitation.

This is a small hand-written client instead of the official Rust SDK (`rmcp`), because `rmcp` needs a tokio runtime and reqwest while Helmp is built on plain threads and `ureq`.

## Plugins can bundle servers

A [plugin](plugins.md) can declare `mcpServers`; its servers run only while the plugin is enabled, and their tools appear as `mcp:<plugin id>/<name>/<tool>`.

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

- Exactly one of `command` (a program name looked up on `PATH`, or `./path` inside the plugin folder; `args`, `env`, `cwd` relative to the plugin folder) or `url` (static `https://` / `http://` URL; `headers`).
- `env` and `headers` values may only use `{{pref.<id>}}`; put secrets in `secret` preferences, never in the manifest.
- A plugin with `mcpServers` doesn't need `commands`.
- Permissions shown before enabling: "Run a local program: `npx`" for each command, "Network access: `host`" for each URL.

## Command line

```bash
helmp mcp-client list                         # state, name, source, tool count, command or URL
helmp mcp-client add time -- uvx mcp-server-time
helmp mcp-client add docs --url https://mcp.example.com/mcp --header "Authorization: Bearer …"
helmp mcp-client add github --env GITHUB_TOKEN=… -- npx -y @modelcontextprotocol/server-github
helmp mcp-client add --json '{"mcpServers": {"mem": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-memory"]}}}'
helmp mcp-client tools time                   # start it and list tools (name, title, confirm, description)
helmp mcp-client import --list                # what Claude Code / Codex / Cursor have (read-only)
helmp mcp-client import [--from claude|codex|cursor] [--project <folder>] [name...]
helmp mcp-client remove time
helmp query "echo"                            # tools show up like any other result
helmp run mcp:time/get_current_time "current time" --form timezone=Asia/Shanghai
```

## Testing (dry run)

With `HELMP_DRY_RUN=1`, Helmp doesn't start real servers: `helmp run` on a tool prints `[dry-run] mcp-call <server> <tool> <arguments>`, and `helmp mcp-client tools` prints `[dry-run] mcp-start …`. Only in test mode (`HELMP_DRY_RUN=1` together with an isolated `HELMP_DATA_DIR`) do servers actually run when their command, or one of their arguments, is a file inside that data folder; HTTP servers run when their URL is `127.0.0.1`, `localhost` or `[::1]`. That's how `tests/e2e_mcp_client.rs` drives `examples/mock_mcp_server.rs`. In test mode the Keychain is replaced by `<data folder>/test-keychain.json`.
