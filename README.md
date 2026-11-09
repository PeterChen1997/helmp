<p align="center">
  <img src="docs/images/icon-256.png" width="128" height="128" alt="Helmp icon">
</p>

<h1 align="center">Helmp</h1>

<p align="center">
  A native macOS launcher that also tells you which coding agent is waiting for you.<br>
  Written in Rust with GPUI. Local-first, no account, no telemetry.
</p>

<p align="center">
  English · <a href="README.zh-CN.md">简体中文</a>
</p>

---

Helmp (read it as "help me") is a Spotlight-style launcher for macOS. Press <kbd>⌥ Space</kbd>, type, hit <kbd>↵</kbd>. Besides launching apps, it lists your Claude Code and Codex sessions and jumps straight to the one that needs you, keeps an unlimited local clipboard history, reads text and QR codes out of screenshots, translates with a model you bring yourself, does quick math, runs system commands and your existing Raycast Script Commands. The interface is available in English and Simplified Chinese.

> **Status: early.** Helmp is under active development and there are no prebuilt releases yet. You build it from source.

<!-- Screenshots: the files below are placeholders. See docs/images/README.md for the list. -->
<p align="center">
  <img src="docs/images/screenshot-home.png" width="720" alt="Home: waiting agent sessions, recent items and quick commands">
</p>

| Agent sessions | Clipboard history |
|---|---|
| ![Agent sessions](docs/images/screenshot-agents.png) | ![Clipboard history](docs/images/screenshot-clipboard.png) |
| **Screenshot OCR** | **Translate** |
| ![Screenshot OCR](docs/images/screenshot-ocr.png) | ![Translate](docs/images/screenshot-translate.png) |

## Features

- **App search, pinyin included.** Matches English names, localized Chinese names, full pinyin and initials: `wx`, `weixin` and `微信` all find WeChat, and polyphonic characters are handled. The detail pane tells you why something matched. <kbd>⌘K</kbd> reveals the app in Finder, copies its path, or quits it if it is running. The index watches `/Applications`, `/System/Applications` and `~/Applications` and updates itself.
- **Agent session switcher for Claude Code and Codex.** Sessions from the CLI and the desktop apps, grouped into *waiting for you*, *running* and *recently finished*, with the last message in the detail pane. <kbd>↵</kbd> jumps to the session: a deep link for the desktop apps, the exact tab in iTerm2 or Terminal.app, or the host app for other terminals. A live session is never resumed a second time. Helmp only reads `~/.claude` and `~/.codex`; it never writes there and installs no hooks.
- **Clipboard history.** Text, images and files, stored locally in SQLite with no item limit, grouped by day and filterable by type. Content marked as concealed by password managers is skipped, and you can exclude apps (Passwords is excluded by default). <kbd>↵</kbd> pastes into the app you came from, <kbd>⌥↵</kbd> pastes as plain text.
- **Screenshot OCR and QR codes.** Capture a region to the clipboard, save it, or recognize its text with Apple Vision. Line breaks and paragraphs are kept; <kbd>↵</kbd> copies everything, <kbd>⇧⌘C</kbd> copies it as one line, <kbd>⌥⌘C</kbd> copies it without line breaks, and <kbd>⌘T</kbd> sends it to Translate. QR codes and barcodes in the image are listed too, with an action to open a link. Recognition languages default to Simplified Chinese and English; Traditional Chinese, Japanese, Korean, French, German, Spanish, Italian, Portuguese and Russian can be added in Settings › Screenshot. Text can also be read from an image on the clipboard or from a file (`ocr /path/to/image.png`).
- **Translate with your own model.** Anthropic, any OpenAI-compatible endpoint (OpenAI, DeepSeek, gateways) or a local Ollama; API keys live in the macOS Keychain. Translation starts as you type (after a short pause), the source language is detected, and the target defaults to "Chinese → English, everything else → Chinese"; you can pick another target or model for a single run. When you open it, Translate picks up the text selected in the front app. <kbd>↵</kbd> copies the translation, <kbd>⌘↵</kbd> replaces the selection with it, <kbd>⇧⌘C</kbd> copies the source text and <kbd>⌘R</kbd> reads the translation aloud with the system voice. <kbd>↑</kbd> in an empty input brings back recent source texts; repeating one shows the earlier result instead of calling the model again. This history is kept in memory only.
- **Calculator.** Type an expression and the answer is the first result: `+ - * / % ^ **`, parentheses, `sqrt`, `sin`, `ln`, `log`, `pow`, `min`, `max` and more, `pi` and `e`. Full-width symbols, Chinese brackets and `×` `÷` work too. <kbd>↵</kbd> copies the result. Plain search terms such as `wx` or `2026` don't produce a result.
- **Web search fallback.** For any non-empty query, the bottom of the list offers "Search Google / Baidu / GitHub for …". <kbd>↵</kbd> opens the search in your default browser. Bing and Bing Translator are built in but off by default; turn engines on or off, reorder them and add your own URL templates (`{query}` is replaced) in Settings › Extensions.
- **System commands.** Lock Screen, Sleep, Empty Trash, Show Desktop, Hide Other Apps, Toggle Dark Mode, Restart, Shut Down, Log Out, Open System Settings and Quit All Apps, found by English or Chinese name and pinyin. Commands that can't be undone (Empty Trash, Restart, Shut Down, Log Out, Quit All Apps) show a confirmation page first: <kbd>↵</kbd> goes ahead, <kbd>esc</kbd> cancels. The confirmation can be turned off in Settings › Extensions.
- **Raycast-compatible script commands.** Drop a script with `@raycast.*` header comments into the scripts folder and it shows up as a command. Any language, up to three arguments rendered as a form, `silent` / `compact` / `fullOutput` output modes, a confirmation page for scripts marked `needsConfirmation`, and a 30-second hard timeout.
- **`helmp://` deep links.** Open the panel with a query, jump into an extension or open a settings page from scripts, Shortcuts or other launchers. See [docs/deeplinks.md](docs/deeplinks.md).
- **Menu bar.** No Dock icon. The menu bar icon gets an orange dot when a session is waiting; its menu lists those sessions, the main commands, settings and quit.
- **Global hotkeys with conflict checks.** One to open the panel, plus one per module, all changeable or removable in Settings. While you record a new combination, Helmp checks it against its own commands, the macOS keyboard shortcuts you have enabled, combinations macOS reserves (such as <kbd>⌘Tab</kbd>), common menu shortcuts (<kbd>⌘Q</kbd>, <kbd>⌘C</kbd>…) and other apps that already registered it. A conflicting combination isn't saved, and Helmp tells you what is using it. If a saved hotkey fails to register later, Settings shows why next to it.
- **English and Simplified Chinese.** The interface follows your system language by default; switch it in Settings › General › Language, no restart needed. Search works the same in both: Chinese names and pinyin always match.

Smaller things that make it pleasant to use daily:

- The keyboard switches to the ABC layout while the panel is open and back to your input method when it hides, so pinyin abbreviations work without fighting the IME.
- Reopen within 90 seconds and your last query is still there, selected; after that, or after you ran something, you start fresh.
- A settings window (General, Extensions, Models, Agents, Clipboard, Screenshot, Appearance, About) and a first-run guide for permissions.
- A headless CLI (`helmp query wx`) for scripting and debugging.

## Requirements

- macOS 15 Sequoia or later.
- Rust toolchain via [rustup](https://rustup.rs). The exact version is pinned in [`rust-toolchain.toml`](rust-toolchain.toml) and installed automatically.
- Xcode. GPUI compiles its Metal shaders with `xcrun metal`; if that command is missing, run `xcodebuild -downloadComponent MetalToolchain`.

## Build from source

```bash
git clone https://github.com/PeterChen1997/helmp.git
cd helmp

# Run directly (development)
cargo run

# Release binary
cargo build --release

# Package as target/release/Helmp.app and sign it
bash script/bundle.sh release
open target/release/Helmp.app
```

`script/bundle.sh` signs the app with `HELMP_SIGN_IDENTITY` if set, otherwise with the first "Developer ID Application" or "Apple Development" certificate it finds, and falls back to ad-hoc signing. macOS remembers permissions per signature, so with ad-hoc signing you will have to grant permissions again after every rebuild. The `helmp://` URL scheme is registered by the packaged app only, so deep links need `Helmp.app`, not `cargo run`.

## First launch and permissions

On first launch Helmp opens a short guide. Click the button next to a permission, then drag the Helmp icon from the floating card into the list in System Settings.

| Permission | Used for | Without it |
|---|---|---|
| Accessibility | Pasting a clipboard item into the front app, reading the selected text for Translate, replacing the selection, and Lock Screen (which presses <kbd>⌃⌘Q</kbd> for you) | The item is copied and you paste with <kbd>⌘V</kbd> yourself; Translate starts with an empty input |
| Screen Recording | Region screenshots and screenshot OCR | Screenshot commands stop and offer to open the right settings page; OCR of clipboard images and files still works |
| Automation (asked by macOS on first use) | Selecting the right iTerm2 / Terminal.app tab when jumping to an agent session; System Events and Finder for some system commands (Lock Screen, Toggle Dark Mode, Restart, Shut Down, Log Out, Empty Trash) | Those actions fail with an error message; desktop-app sessions open through deep links and are unaffected |

The global hotkeys use Carbon `RegisterEventHotKey` and need no permission.

## Keyboard shortcuts

Global (defaults, all configurable in Settings):

| Shortcut | Action |
|---|---|
| <kbd>⌥ Space</kbd> | Show / hide the panel |
| <kbd>⇧⌘V</kbd> | Clipboard history |
| <kbd>⇧⌘2</kbd> | Capture a region and recognize text (runs without opening the panel) |
| <kbd>⌥⇧T</kbd> | Translate (picks up the selected text) |
| <kbd>⌃⌥A</kbd> | Agent sessions |

The translate and agent defaults deliberately avoid <kbd>⇧⌘T</kbd> (reopen tab in browsers) and <kbd>⌥A</kbd> (types `å`).

In the panel:

| Shortcut | Action |
|---|---|
| <kbd>↑</kbd> <kbd>↓</kbd> | Move selection (in Translate, <kbd>↑</kbd> in an empty input recalls recent source texts) |
| <kbd>↵</kbd> | Run the primary action |
| <kbd>⌘K</kbd> | More actions for the selected item |
| <kbd>⇥</kbd> | On the home view or in search results: enter the selected item's extension. Inside an extension: next filter (e.g. All / Text / Images / Files) |
| <kbd>esc</kbd> | Close the action menu → close the result or confirmation page → leave the extension → hide |
| <kbd>⌫</kbd> | With an empty input inside an extension: leave it |
| <kbd>⌘,</kbd> | Settings |

Type a prefix followed by a space to jump into a module: `ag ` / `agent ` for agent sessions, `cb ` / `clip ` / `剪贴板 ` / `剪切板 ` for the clipboard, `fy ` / `tr ` / `翻译 ` for Translate. You can add your own alias for each extension in Settings › Extensions.

## Deep links

Helmp registers the `helmp://` URL scheme:

```bash
open "helmp://show?q=wx"                      # open the panel with a query
open "helmp://scope/translate?q=hello%20world" # enter an extension: translate, clipboard, agents
open "helmp://settings/extensions"            # open a settings page
open "helmp://hide"                           # hide the panel
```

The full reference is in [docs/deeplinks.md](docs/deeplinks.md).

## Configuration

Everything Helmp stores lives in one folder:

```
~/Library/Application Support/Helmp/
├── config.json          settings, including web search engines (no secrets)
├── history.json         "Recent" on the home view, up to 30 entries
├── clipboard.sqlite     clipboard history
├── clipboard-images/    images copied to the clipboard
├── scripts/             your script commands (default script folder)
├── screenshots/         "Capture and save" output (configurable)
├── ocr/                 the last few screenshots shown in OCR results
└── icon-cache/          rendered app and file icons
```

`config.json` is written by the Settings window; edit it by hand only while Helmp is not running. API keys are stored in the Keychain, not in this file. Set `HELMP_DATA_DIR` to use a different folder (handy for testing). See [docs/privacy.md](docs/privacy.md) for exactly what is stored and read.

## Script commands

Helmp understands the [Raycast Script Commands](https://github.com/raycast/script-commands) header format. Put an executable script in `~/Library/Application Support/Helmp/scripts/` (or add another folder in Settings › Extensions) and it appears within a few seconds:

```bash
#!/bin/bash

# @raycast.schemaVersion 1
# @raycast.title Current Time
# @raycast.mode compact
# @raycast.packageName Examples
# @raycast.description Show the current date and time

date "+%Y-%m-%d %H:%M:%S %Z"
```

Arguments become a form:

```bash
# @raycast.argument1 { "type": "text", "placeholder": "query" }
# @raycast.argument2 { "type": "dropdown", "placeholder": "engine", "data": [{"title": "Google", "value": "g"}, {"title": "Bing", "value": "b"}] }
```

More examples are in [`examples/scripts`](examples/scripts). The full field reference is in [docs/scripts.md](docs/scripts.md).

## Command line

```bash
cargo run -- query wx            # print results: section, title, subtitle, id
cargo run -- run <id> [query]    # run one result
cargo run -- providers           # list registered providers
HELMP_DRY_RUN=1 cargo run -- run <id> agent   # print the agent jump instead of doing it
HELMP_TRACE=1 cargo run          # log UI events to stderr
```

## Roadmap

Ideas we want to get to. None of these exist yet, and the order may change.

- Signed and notarized releases, a Homebrew cask, and automatic updates (the "Check for Updates" menu item and the update toggle in Settings have nothing to check against yet).
- Expose Helmp's capabilities (app search, clipboard, screenshot, OCR) to coding agents as an MCP server.
- More built-in developer utilities: kill the process on a port, JSON / Base64 / URL / timestamp conversion.
- File search.
- More agents beyond Claude Code and Codex.
- Script commands: Alfred Script Filter JSON output, custom icons, and live `inline` mode with `refreshTime` (both fields are parsed today but not used).
- Long-running plugins with their own views, and an isolated plugin runtime.
- Clipboard pinning and sequential paste.

## Contributing

Issues and pull requests are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) first; the architecture overview in [docs/architecture.md](docs/architecture.md) explains how modules plug in.

## License

[MIT](LICENSE) © 2026 Peter Chen
