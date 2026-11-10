# Helmp MCP server: let coding agents build and use Helmp

[简体中文](mcp.zh-CN.md)

`helmp mcp` is an [MCP](https://modelcontextprotocol.io) server built into the Helmp binary. Coding agents such as Claude Code, Codex and Cursor launch it over stdio and can then:

- **build Helmp features for you**: ask for a feature ("send the selected text to my Feishu bot") and the agent designs it as a [plugin](plugins.md), scaffolds, validates and dry-run tests it, installs it, shows it to you, and asks whether you want to publish it;
- **use Helmp**: search like typing in the panel and run results. Anything with a side effect asks you in the Helmp panel first.

- [Connect an agent](#connect-an-agent)
- [Tools, resources and prompts](#tools-resources-and-prompts)
- [What needs your approval](#what-needs-your-approval)
- [Privacy](#privacy)
- [Example conversation](#example-conversation)
- [Details](#details)

## Connect an agent

In **Settings › Agents › Connect Coding Agents**, click **Connect** next to Claude Code, Codex or Cursor, then restart that agent. The row shows whether it is connected and which file holds the configuration. From a terminal:

```bash
helmp mcp install claude    # or: codex, cursor
helmp mcp status            # connected / not connected, per agent
```

| Agent | What Helmp writes |
|---|---|
| Claude Code | Runs `claude mcp add --scope user helmp -- <helmp> mcp` when the `claude` command is installed; otherwise adds `mcpServers.helmp` to `~/.claude.json` (or `$CLAUDE_CONFIG_DIR/.claude.json`) |
| Codex | A `[mcp_servers.helmp]` table in `~/.codex/config.toml` (or `$CODEX_HOME/config.toml`) |
| Cursor | `mcpServers.helmp` in `~/.cursor/mcp.json` |

`<helmp>` is the path of the running Helmp binary, e.g. `/Applications/Helmp.app/Contents/MacOS/helmp`. Before changing a file Helmp copies it to `<file>.helmp-backup`. Only the `helmp` entry is touched; other servers, settings and TOML comments stay as they are. Connecting again when nothing changed does nothing; if Helmp moved, the row says so and **Reconnect** updates the path. Helmp only writes these files when you click Connect or run `helmp mcp install`.

Any other MCP client works too: run `<helmp> mcp` as a stdio server.

## Tools, resources and prompts

| Tool | What it does |
|---|---|
| `helmp_plugin_guide` | The plugin authoring guide: the agent workflow, the [plugin reference](plugins.md), testing, and the bundled plugins as complete examples |
| `helmp_plugin_scaffold` | Creates a valid plugin folder from a template (`quicklink`, `http`, `script`, `list` or `starter`) in `~/Library/Application Support/Helmp/plugins-dev/<id>` or a folder you choose |
| `helmp_plugin_validate` | Validates a folder exactly as Helmp loads it: every problem with its field path, or the commands and the permissions you will be asked to approve |
| `helmp_plugin_install` | Copies the folder into Helmp's plugins folder. **The plugin stays off** until you review its permissions and turn it on in Settings › Plugins, which Helmp opens on that plugin if it is running |
| `helmp_plugin_test` | Runs one command headless. Dry run (default): an isolated sandbox where opens, copies, pastes and requests to real hosts are only reported as `[dry-run] …`; requests to `127.0.0.1` / `localhost` are sent, so the agent can test against a local mock server; secrets are passed for that run only. `dry_run: false` runs the installed, enabled plugin for real, with your approval in the panel |
| `helmp_plugin_list` | Bundled and user plugins with state (enabled, disabled, needs-consent, error), errors, permissions and commands |
| `helmp_plugin_submit` | Publishes a plugin to the public Helmp plugin center (`helmp plugins submit <folder> --yes`) and returns the pull request URL. The agent must ask you first and pass `user_confirmed: true` only after you said yes |
| `helmp_search` | Searches Helmp like the panel: apps, calculator and conversions, time zones, developer tools, dictionary, system commands, windows, plugin commands… Each result says whether running it needs your approval |
| `helmp_run` | Runs a result. Pure computations return text directly; everything else asks you first (see below) |

Resources: `helmp://docs/plugins` and `helmp://docs/plugins.zh-CN` (the same guide, English or Chinese plugin docs). Prompt: `create_helmp_plugin` with a `request` argument, which walks the agent through design → scaffold → write files → validate → dry-run test → install → show you → ask whether to publish.

## What needs your approval

| Action | Approval |
|---|---|
| Pure computations: calculator, unit and currency conversion, time, developer tools, dictionary lookup, translation, OCR of an image file the agent names | None. The agent gets the text; nothing is copied or pasted |
| Everything else: opening apps, files or links, copying, pasting, killing processes, moving windows, toggles, system commands, plugin HTTP requests and scripts, any secondary action | The Helmp panel shows **"Claude Code wants to run: Open “npm react”"** with the query and form values. **Return** allows, **Esc** denies. No answer within 60 seconds counts as denied, and a late Return does nothing |
| Turning on a plugin the agent installed | Only you can, in Settings › Plugins, after seeing its permissions. The MCP server never writes consent |
| Publishing a plugin | The agent must ask you in the conversation; the tool refuses without `user_confirmed: true` |

The rule is conservative: only the computations above skip approval, and naming a secondary action (such as "Replace Selection" on a translation) always asks. The MCP server process itself runs every action in dry-run mode, so even a misclassified result could only be reported, not performed; real side effects only happen inside the Helmp app after you press Return. If Helmp isn't running, the agent is told to ask you to start it.

The approval request travels over a Unix socket in Helmp's data folder (`agent.sock`, mode 0600), accepted only from your own user account.

## Privacy

- Clipboard history, agent session transcripts and your Recent items are never returned to agents by default: they are left out of `helmp_search` (the result count says how many were hidden) and `helmp_run` refuses them. To allow it, set `"mcp_server": {"expose_private": true}` in `~/Library/Application Support/Helmp/config.json`.
- `{{clipboard}}` in plugins is `[clipboard]` in the MCP server and dry runs; the server never reads your clipboard.
- Plugin dry runs use a throwaway data folder: your Keychain is not read, and secrets the agent passes for a test are not stored.
- Network lookups that only run in the live app (currency rates, public IP) are unavailable to the MCP server.

## Example conversation

> **You:** 帮我做一个把选中文字发到飞书机器人的插件 (make a plugin that sends the selected text to my Feishu bot)
>
> **Agent:** *(reads `helmp_plugin_guide`)* Plan: plugin `com.you.feishu-bot`, command **Send to Feishu** (keyword `fs`) with a text field prefilled from what you type; it POSTs `{"msg_type":"text","content":{"text":…}}` to your bot's webhook, stored in the Keychain and limited to `open.feishu.cn`.
> *(`helmp_plugin_scaffold` kind `http` → edits `plugin.json` → `helmp_plugin_validate`: valid, permission "Network access: open.feishu.cn")*
> *(starts a local mock, `helmp_plugin_test` with `secrets: {"webhook": "http://127.0.0.1:8765/hook"}` → the mock received the JSON, Helmp showed "Sent to Feishu")*
> *(`helmp_plugin_install` → state: disabled)*
> Installed. Helmp opened Settings › Plugins on **Feishu Bot**: check that it only needs network access to open.feishu.cn, turn it on and paste your webhook URL. Then select text, open Helmp and type `fs`.
> Do you want to publish it to the public Helmp plugin center so others can use it?
>
> **You:** 不用了 (no thanks)
>
> **Agent:** OK, it stays local.

## Details

- Transport: newline-delimited JSON-RPC 2.0 over stdio. Protocol versions `2025-11-25`, `2025-06-18`, `2025-03-26` and `2024-11-05`; a client asking for another version gets the newest. Tool results carry text plus `structuredContent`; tool failures (invalid plugin, denied, timed out) are results with `isError: true`.
- The server reads Helmp's `config.json` from disk for every plugin list and state check, so plugins you enable while the agent is running show up as enabled.
- Searches are as fast as `helmp query`; the first one builds the indexes (apps, files…) once.
- Tests: [`tests/e2e_mcp_server.rs`](../tests/e2e_mcp_server.rs) drives `helmp mcp` over stdio in an isolated environment (temporary data folder and `HOME`, dry-run mode, a fake `claude` command), so they never touch your agent configs, Keychain or clipboard.
