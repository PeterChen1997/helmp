# Script commands

English · [简体中文](scripts.zh-CN.md)

A script command is any executable file with a few special comments at the top. Helmp reads the [Raycast Script Commands](https://github.com/raycast/script-commands) format, so most scripts written for Raycast work without changes. Scripts can be written in any language your Mac can run: shell, Python, Ruby, Swift, JavaScript, AppleScript…

## Where scripts live

Helmp scans these folders, recursively, skipping hidden files and folders:

1. `~/Library/Application Support/Helmp/scripts/` (always included)
2. Folders you add under **Settings › Extensions › Script folders**
3. Folders in the `HELMP_SCRIPT_DIRS` environment variable, separated by `:`

Folders are checked every 5 seconds. New, edited and deleted scripts show up without restarting; only files whose modification time, size or executable bit changed are parsed again. Only the first 64 KB of each file is read when looking for the header.

A file is treated as a script command only if it contains at least one `@raycast.` or `@helmp.` field. Files with fields that can't be used (missing title, unknown mode, bad argument JSON…) are skipped, and the reason is printed to stderr (run Helmp from a terminal to see it).

## Minimal example

```bash
#!/bin/bash

# @raycast.schemaVersion 1
# @raycast.title Current Time
# @raycast.mode compact

date "+%Y-%m-%d %H:%M:%S %Z"
```

Save it in the scripts folder, make it executable (`chmod +x current-time.sh`), then type `current time` in Helmp.

## Header fields

Fields are written as `<comment> @raycast.<field> <value>`. The comment marker can be `#`, `//` or `--`. Field names are case-insensitive.

Every field also has a `@helmp.<field>` spelling. If a script has both, the `@helmp.` value wins, so you can tweak a shared Raycast script for Helmp without breaking it for Raycast.

| Field | Required | Meaning |
|---|---|---|
| `title` | yes | Name shown in the list and matched by search (pinyin works for Chinese titles) |
| `mode` | yes | `silent`, `compact`, `fullOutput` or `inline`, see below |
| `schemaVersion` | no | Must be `1` if present |
| `packageName` | no | Group name, shown in the subtitle. Also searchable, ranked below the title |
| `description` | no | Shown in the row's subtitle and the detail pane |
| `author` | no | Parsed but not displayed |
| `argument1` … `argument3` | no | Arguments, see below |
| `needsConfirmation` | no | `true` or `false`. When `true`, the script only runs if the query ends with ` !` |
| `currentDirectoryPath` | no | Working directory. `~` and `~/…` are expanded, relative paths are resolved against the script's folder. Default: the script's folder |
| `platform` | no | If present, it must include `macOS` |
| `icon` | no | Parsed but not displayed yet; every script uses the same terminal icon for now |
| `refreshTime` | no | Parsed but not used yet |

Other Raycast fields (`authorURL`, …) are ignored.

## Output modes

| Mode | What you see |
|---|---|
| `silent` | Nothing. The panel stays hidden. Good for scripts that copy, open or toggle something |
| `compact` | The last line of stdout |
| `fullOutput` | All of stdout and stderr, in a monospaced detail pane |
| `inline` | Currently the same as `compact` (Raycast keeps it on the row and refreshes it; Helmp doesn't yet) |

Output is shown in the panel with a **Copy Output** action (<kbd>⌘C</kbd>). Raycast shows `silent` output in a HUD; Helmp has no HUD, so `silent` really is silent.

## Arguments

Declare up to three arguments as JSON objects:

```bash
# @raycast.argument1 { "type": "text", "placeholder": "city", "percentEncoded": true }
# @raycast.argument2 { "type": "password", "placeholder": "token", "optional": true }
# @raycast.argument3 { "type": "dropdown", "placeholder": "class", "data": [{"title": "Economy", "value": "e"}, {"title": "Business", "value": "b"}] }
```

| Key | Meaning |
|---|---|
| `type` | `text` (default), `password` (masked input) or `dropdown` |
| `placeholder` | Field label and hint |
| `optional` | `true` if the argument may be empty |
| `percentEncoded` | `true` to URL-encode the value before passing it |
| `data` | For `dropdown`: an array of `{ "title", "value" }` |

The older `"secure": true` flag is treated as `"type": "password"`.

A script with arguments opens as a form. You can also type them straight after the title: with the script above, `flight berlin` fills `berlin` into the first field. Multiple arguments are split on whitespace, and the last one takes the rest of the text. Values are passed to the script as `$1`, `$2`, `$3`.

## How a script runs

- If the file is executable, Helmp runs it directly. Otherwise it reads the shebang (`#!/usr/bin/env python3`) and runs the interpreter. A file with neither is skipped. Helmp never changes file permissions.
- Each run has a **30-second** limit. On timeout the whole process group is killed, including anything the script started.
- Helmp keeps at most the last 1 MiB of stdout and of stderr.
- A non-zero exit code is an error: the panel shows it together with the end of stderr.
- The script inherits Helmp's environment. When Helmp is started from the Dock or at login, that environment is minimal, so prefer absolute paths (`/opt/homebrew/bin/jq`) or set `PATH` at the top of the script.

## Examples

The [`examples/scripts`](../examples/scripts) folder has three small scripts, one per common mode:

| Script | Mode | What it shows |
|---|---|---|
| [`current-time.sh`](../examples/scripts/current-time.sh) | `compact` | The simplest possible command |
| [`generate-uuid.sh`](../examples/scripts/generate-uuid.sh) | `silent` | Do something (copy a UUID) and stay out of the way |
| [`local-ip.sh`](../examples/scripts/local-ip.sh) | `fullOutput` | Multi-line output |

To try them:

```bash
mkdir -p ~/Library/Application\ Support/Helmp/scripts
cp examples/scripts/*.sh ~/Library/Application\ Support/Helmp/scripts/
```

Or point Helmp at the folder without copying: `HELMP_SCRIPT_DIRS="$PWD/examples/scripts" cargo run`.

The [Raycast script-commands repository](https://github.com/raycast/script-commands) has hundreds more. Check what a script does before you run it: scripts run with your user's full permissions.
