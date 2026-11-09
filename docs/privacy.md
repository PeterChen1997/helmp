# Privacy

English · [简体中文](privacy.zh-CN.md)

Short version: Helmp has no account, no telemetry, no crash reporting and no update check. Everything it stores stays in one folder on your Mac. The only network requests it makes are the model calls you configure for Translate.

This page describes what the code does today. If you find a mismatch, please open an issue.

## What Helmp stores

All files live in `~/Library/Application Support/Helmp/` (or `HELMP_DATA_DIR` if set):

| Path | Contents | How to remove it |
|---|---|---|
| `config.json` | Your settings: hotkeys, enabled modules, model names / endpoints / model ids, clipboard and screenshot options, extra script folders. **No API keys.** | Delete the file to reset settings |
| `history.json` | The "Recent" row on the home view: up to 30 entries, each with the item's title, subtitle, icon and **the query you typed** to find it | Delete the file |
| `clipboard.sqlite` | Clipboard history: text, file paths, the name and bundle id of the app you copied from, and the time. No item limit unless you set a retention period | Settings › Clipboard, or delete the file |
| `clipboard-images/` | Images you copied, named by content hash | Removed with the clipboard history |
| `screenshots/` | Output of "Capture and save" (you can choose another folder) | Delete the files |
| `ocr/` | The last few screenshots shown next to OCR results; older ones are deleted automatically | Delete the folder |
| `scripts/` | Your script commands | Yours to manage |
| `icon-cache/` | PNG renderings of app and file icons | Safe to delete at any time |

If you used an earlier build called "huan", Helmp moves `~/Library/Application Support/huan/` to the new folder once on first launch.

### Clipboard rules

- Content that password managers mark as concealed (`org.nspasteboard.ConcealedType`) is skipped. This is on by default and can be turned off in Settings.
- Content marked as transient or auto-generated is always skipped, including items Helmp itself writes back.
- Apps on the ignore list are never recorded. The Passwords app is on it by default.
- Image recording can be turned off.

## Keychain

API keys for your models are stored in the macOS login Keychain as generic passwords:

- Service: `dev.helmp.launcher`
- Account: `model:<model id>`

They are written when you leave the API Key field, press Return, or test the connection, and never appear in `config.json`. Remove them with Keychain Access (search for `dev.helmp.launcher`) or by deleting the model in Settings.

## What Helmp reads but doesn't store

- **Applications.** `Info.plist` and localization files of apps in `/Applications`, `/System/Applications` and `~/Applications`, to build the search index in memory.
- **Agent sessions.** Read-only, polled once a second while Helmp runs:
  - Claude Code: `~/.claude/sessions/*.json`, the last 64 KB of `~/.claude/projects/*/<session>.jsonl` when a session has no title, and the Claude desktop app's session files in `~/Library/Application Support/Claude/claude-code-sessions/`. `CLAUDE_CONFIG_DIR` is respected.
  - Codex: `~/.codex/thread-writer-locks/` (to tell whether a session is alive), `state_5.sqlite` opened read-only, and the last 64 KB of session logs. `CODEX_HOME` is respected.
  - Process information (`proc_pidinfo`) of those sessions, and the Git branch of their working folders.

  Helmp never writes to these folders, never deletes anything and installs no hooks. You can turn each source off in Settings › Agents.
- **Selected text.** Only when you open Translate with an empty input and a model is configured, Helmp asks the front app for its selected text through the Accessibility API. It's kept in memory for about a second and a half.
- **Clipboard.** Polled every 300 ms to build the history (see the rules above). Turn the history off in Settings › Clipboard.

## Network

- **Translate** sends the text you translate, plus a short instruction, to the endpoint you configured: Anthropic, an OpenAI-compatible URL, or Ollama (`http://localhost:11434` by default). "Test connection" in Settings sends a small request to the same endpoint. Each request has a 30-second timeout. Helmp doesn't route these requests through any server of its own; there isn't one.
- Nothing else. There is no analytics, telemetry, crash reporting or update check. "Check for Updates" in the menu currently just shows the installed version.

Things that may reach the network because *you* asked for them: opening a link from the clipboard history, deep links to the Claude / ChatGPT desktop apps, and any script command you run.

## Permissions

| Permission | Why |
|---|---|
| Accessibility | Simulate <kbd>⌘V</kbd> to paste into the front app; read and replace selected text for Translate |
| Screen Recording | Region screenshots (`screencapture -i`) |
| Automation (iTerm2, Terminal) | Select the tab of an agent session when you jump to it |

Helmp checks permissions without triggering system prompts. Every feature that needs one degrades with a message instead of failing silently.

## Uninstalling

1. Quit Helmp and delete `Helmp.app`.
2. Delete `~/Library/Application Support/Helmp/`.
3. In Keychain Access, delete items whose name is `dev.helmp.launcher`.
4. Optionally remove Helmp from System Settings › Privacy & Security (Accessibility, Screen Recording, Automation) and from Login Items.
