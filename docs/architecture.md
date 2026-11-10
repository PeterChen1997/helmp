# Architecture

Helmp is a single Rust binary. The UI is drawn with [GPUI](https://www.gpui.rs) (through `gpui-kit`), and everything macOS-specific goes through `objc2` bindings. Features are **providers**: modules that return plain data. The launcher renders that data and owns all key handling, so a provider never draws UI.

```
                    ┌──────────────── main.rs ────────────────┐
                    │ Engine::new(providers::all())           │
                    │ args? → cli.rs (headless)               │
                    │ else  → GPUI app (Accessory, no Dock)   │
                    └───┬───────────┬───────────┬─────────────┘
                        │           │           │
     hotkey.rs ─────────┤           │           ├────────── menubar.rs
 (Carbon global hotkeys;│           │           │       (NSStatusItem + NSMenu)
  hotkey/conflict.rs)   │      deeplink.rs      │
                        │     (helmp:// URLs)   │
                        ▼           ▼           ▼
                    launcher.rs  ◀──── panel.rs (NSVisualEffectView glass,
              (search bar, list,          window placement per screen)
               detail, form, ⌘K)
                            │ query / query_scoped / suggestions / will_enter / recall / Action
                            ▼
                       engine.rs ──── history.rs ("Recent")
                            │
                            ▼  providers/ (one thread each for start())
   builtin · calculator · system · apps · screenshot · clipboard
   scripts · agents · translate · websearch

   translate ──▶ models/ (Anthropic, OpenAI-compatible,
                 Ollama; Keychain; reading the selection)

  config.rs ── read by everyone, written by settings/ ── onboarding/ (first-run permissions)
  i18n.rs  ── tr("中文", "English") for every UI string; follows config.general.language
```

## Modules

| Path | What it does |
|---|---|
| `src/main.rs` | Entry point. Builds the engine, dispatches CLI subcommands, otherwise opens the panel window, registers hotkeys and the `helmp://` handler, installs the menu bar item and shows onboarding if needed. Also hosts the `HELMP_SELF_TEST` GUI self-test. |
| `src/provider.rs` | The provider interface: `Provider`, `Item`, `Action`, `Outcome`, `Scope` and the view types. Described below. |
| `src/providers/` | One folder or file per provider; `providers/mod.rs` is the registry. |
| `src/engine.rs` | Starts providers, routes queries, resolves scope prefixes (`cb `), builds the home view and sorts / groups results. |
| `src/launcher.rs` | The 880 × 540 panel: search bar, list / list + detail / form + detail views, status bar, ⌘K menu and all key handling. Runs actions and interprets their `Outcome`. |
| `src/panel.rs` | Platform glue for the panel window: frosted-glass background and per-screen placement. |
| `src/hotkey.rs` | Global hotkeys via `global-hotkey` (Carbon, no permission needed). Parses `alt-space` style strings, re-registers incrementally when settings change, and records why a binding failed so Settings can show it. Default bindings live in `EXTENSION_SHORTCUTS`. |
| `src/hotkey/conflict.rs` | Conflict check used while recording a hotkey: Helmp's own commands, enabled system shortcuts (read from `com.apple.symbolichotkeys.plist`), combinations macOS reserves, common menu shortcuts, and a register-and-release probe for other apps. `check` is a pure function and is unit tested. |
| `src/deeplink.rs` | `helmp://` URLs: `show`, `scope/<id>`, `settings/<page>`, `hide`. Registered before the app starts, so a link can cold-launch Helmp. See [deeplinks.md](deeplinks.md). |
| `src/i18n.rs` | Interface language (Simplified Chinese / English). `tr("中文", "English")` and `trf!(…)` pick the current language; `config.general.language` drives it, defaulting to the system's preferred languages. Search keywords don't go through it. |
| `src/menubar.rs` | Menu bar icon (template image + orange dot when a session is waiting) and its menu. |
| `src/config.rs` | `<data_dir>/config.json`: `get()` snapshot, `update()` write-and-notify, `subscribe()`. |
| `src/history.rs` | `<data_dir>/history.json`: the "Recent" row on the home view; replays an entry by querying its id again. |
| `src/matcher.rs` | Shared scoring: literal rules plus pinyin rules (full pinyin, initials, polyphonic characters). |
| `src/models/` | Bring-your-own model clients, Keychain storage for API keys, and reading the selected text (`selection.rs`: Accessibility first, then a simulated ⌘C that restores the clipboard). |
| `src/settings/` | The settings window. Pure mapping logic lives in `settings/model.rs` so it can be unit tested; `settings/hotkey.rs` is the hotkey recorder. |
| `src/onboarding/` | First-run permission window and the drag-to-authorize card shown on top of System Settings. |
| `src/permissions.rs` | Read-only checks for Accessibility and Screen Recording. Never triggers a system prompt. |
| `src/icons.rs` | Renders app / file icons to PNG in `<data_dir>/icon-cache/` on a background thread. |
| `src/paths.rs` | Data folder: `~/Library/Application Support/Helmp/`, overridable with `HELMP_DATA_DIR`. |
| `src/style.rs` | Light and dark palettes shared by the panel, settings and onboarding. |
| `src/dock.rs` | Temporarily shows a Dock icon while a regular window (settings, onboarding) is open. |
| `src/cli.rs` | `helmp query`, `helmp run`, `helmp providers`. |
| `src/mcp/` | `helmp mcp`: stdio MCP server for coding agents (plugin authoring tools, search / run), the `agent.sock` approval channel served by the GUI, and agent config writers. See [mcp.md](mcp.md). |
| `src/trace.rs` | `HELMP_TRACE=1` logs UI events to stderr. |

### Built-in providers

| id | Scope prefixes | Background work |
|---|---|---|
| `builtin` | — | none (Settings, Quit) |
| `calculator` | — | none; parses the query as an expression (`calculator/parser.rs`) and returns at most one item, scored above every text match |
| `system` | — | none; Lock Screen, Sleep, Empty Trash, Restart… Irreversible ones return a `Detail::confirm` page first |
| `apps` | — | scans `/Applications`, `/System/Applications`, `~/Applications`; FSEvents watcher, rebuild debounced by 1 s |
| `screenshot` | — | none; runs `screencapture -i` and Apple Vision on demand |
| `clipboard` | `cb `, `clip `, `剪贴板 `, `剪切板 ` | polls `NSPasteboard.changeCount` every 300 ms, stores in SQLite |
| `scripts` | — (matched by title or `packageName`) | rescans script folders every 5 s, re-parses only changed files |
| `agents` | `ag `, `agent ` | polls Claude Code / Codex state every 1 s, reads only the last 64 KB of any log |
| `translate` | `fy `, `tr `, `翻译 ` | reads the selected text in `will_enter`; keeps the last 20 translations in memory (`recall` and cache) |
| `websearch` | — | none; one item per enabled engine for any non-empty query, scored below every local result |

## Lifecycle of a query

1. On launch, `Engine::start_in_background` calls every provider's `start()` on its own thread. A failing provider prints to stderr and does not take the others down.
2. Each keystroke goes to `Engine::resolve_prefix`. If the text starts with a scope prefix, the launcher enters that scope and calls `query_scoped(rest, filter)` on that provider only. Otherwise `query(text)` is called on every enabled provider (Settings › Extensions can disable one).
3. An empty query builds the home view from each provider's `suggestions()` plus "Recent" from `history.rs`.
4. The engine sorts by score, then groups by `Item.badge`: a section's position is decided by its best item, and items inside a section are ordered by score, then title. At most 200 results are returned.
5. While the panel is visible it re-queries every second, so agent status, new clipboard items and freshly rendered icons show up without typing.

## Lifecycle of an action

1. <kbd>↵</kbd> runs `item.action`; <kbd>⌘K</kbd> lists `item.actions`, each of which may carry its own shortcut (`⌘↵`, `⇧⌘C`, …) that also works directly from the list.
2. The item is recorded in history (so it appears under "Recent").
3. The panel hides first, so that a paste or keystroke lands in the app the user came from. Two exceptions keep it open: submitting a form (the result appears in the detail pane) and actions marked `keep_open()` (the list refreshes in place, e.g. deleting a clipboard entry).
4. The action runs on GPUI's background executor. Its `Outcome` is handled on the UI thread.

## The provider interface

Defined in [`src/provider.rs`](../src/provider.rs).

```rust
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;                         // "apps", "agents"; also the Item.id prefix
    fn start(&self) -> Result<()> { Ok(()) }              // background thread, called once, may block
    fn query(&self, query: &str) -> Vec<Item>;            // global search
    fn scope(&self) -> Option<Scope> { None }             // optional dedicated scope
    fn query_scoped(&self, query: &str, _filter: &str) -> Vec<Item> { self.query(query) }
    fn suggestions(&self) -> Vec<Item> { Vec::new() }     // home view (empty query)
    fn will_enter(&self, _app_in_front: bool) {}          // just before the scope opens; fast
    fn recall(&self) -> Vec<String> { Vec::new() }        // ↑ history inside the scope, newest first
}
```

Rules every provider follows:

- `query`, `query_scoped` and `suggestions` run on every keystroke. They must only read memory and return within milliseconds. Indexing, watching and polling happen in `start()` or on threads it spawns.
- `Item.id` is `<provider id>:<domain id>` and must be stable: history replays an item by querying its id again.
- An empty global query should usually return nothing; put home-view content in `suggestions()`.
- `will_enter` runs on the UI thread before the panel shows. `app_in_front` is true when the user's app is still in front (global hotkey, menu bar, deep link), which is the only moment a simulated keystroke reaches that app. Return within a few hundred milliseconds.
- Every user-visible string goes through `tr("中文", "English")` or `trf!`. `Item.badge` is also the grouping key, and the engine pins the language for the duration of one query so a language switch can't split a section.

### `Item`

| Field | Meaning |
|---|---|
| `id` | `<provider>:<domain id>`, stable and unique within the provider |
| `title`, `subtitle` | The two lines of the row |
| `badge` | Section name; rows with the same badge are grouped under one heading |
| `score` | Higher sorts first. `matcher::score` gives 1000 (exact) down to 300 (pinyin subsequence). Home sections use conventions: "waiting" items ≥ 900, quick commands < 800 |
| `action`, `action_title` | Primary action (<kbd>↵</kbd>) and its label in the status bar, e.g. "Open", "Paste to WeChat" |
| `icon` | `Icon::None`, `Icon::Image(path)`, `Icon::File(path)` (system icon of a file or `.app`), or `Icon::Symbol { name, tint }` (a [Lucide](https://lucide.dev) icon on a colored tile) |
| `accessories` | Right side of the row: `Accessory::Text`, `Accessory::Tag { text, tone }`, `Accessory::Keys` (`Accessory::keys("⇧⌘V")`) |
| `actions` | Secondary actions for the ⌘K menu (`NamedAction { title, shortcut, action }`) |
| `detail` | Turns the view into list + detail |
| `form` | Turns the view into form + detail; <kbd>↵</kbd> passes the field values to the action |
| `auto_run` | For form items: run the primary action automatically, debounced, whenever the input or a field changes, and show the result in place (Translate's type-to-translate). The action must be safe to repeat |

Use `..Item::blank()` to fill in the fields you don't need.

### `Detail` and `Form`

A `Detail` has an icon, title, optional tag, a list of `Block`s (`Text`, `Result { text, large }` for selectable main output such as a translation, `Caption`, `Code`, `Quote { label, text }`, `Image(path)`), a key–value `metadata` table, and `actions` used when the detail is the *result* of running something (the first one becomes <kbd>↵</kbd>). Actions go through <kbd>↵</kbd>, <kbd>⌘K</kbd> and their own shortcuts.

`Detail::confirm(icon, question, consequence, confirm_title, action)` builds the confirmation page used by irreversible system commands and `needsConfirmation` scripts: <kbd>↵</kbd> runs `action`, "Cancel" or <kbd>esc</kbd> goes back to the list. The action that returns it should be `keep_open()` so the page appears in place.

A `Form` is a list of `Field { key, label, kind, value, required }` where `kind` is `Text { placeholder, multiline }`, `Secret { placeholder }` or `Select { options }`. Field values arrive in `ActionInput.form`, keyed by `key`.

### `Action`

An `Action` wraps a closure that runs on a background thread and returns `Result<Outcome>`.

| Constructor | Use |
|---|---|
| `Action::new(|| …)` | Action without input |
| `Action::with_input(|input| …)` | Action that reads form values |
| `Action::open(target)` | `open` a file, app or URL (including `claude://` and `codex://` deep links) |
| `Action::copy(text)` | Write text to the clipboard (forces a UTF-8 locale for `pbcopy`) |
| `Action::reveal(path)` | Reveal in Finder |
| `Action::enter_scope(id)` | Enter another provider's scope |
| `Action::rerun()` | Run the item's primary action again ("Try Again" on an error page); interpreted by the launcher |
| `Action::dismiss()` | Close the shown detail and go back to the list ("Cancel" on a confirmation page) |
| `.keep_open()` | Don't hide the panel; refresh the list in place when done |

An error returned by an action is shown in the panel; it is never swallowed.

### `Outcome`

| Variant | What the launcher does |
|---|---|
| `Done` | Nothing more; the panel stays hidden |
| `Message(String)` | Reopens the panel and shows the text (script output, error explanation) |
| `Show(Detail)` | Shows a detail, e.g. a translation or OCR result. In a form view it appears on the right without closing the panel |
| `EnterScope(id)` | Enters a provider's scope |
| `EnterScopeWith(id, text)` | Enters a scope with initial text, e.g. OCR text into Translate |
| `OpenSettings` | Opens the settings window (actions run off the UI thread and can't open windows themselves) |
| `Dismiss` | Closes the shown detail and returns to the list, like <kbd>esc</kbd> |

### `Scope`

A provider can declare one scope. Inside it, the search bar shows a colored label and only that provider is queried.

| Field | Meaning |
|---|---|
| `id` | Usually the provider id |
| `label`, `tint` | The label shown in the search bar |
| `prefixes` | Typing one of these enters the scope. Include the trailing space (`"cb "`), otherwise `cbr` or `clipboard` would trigger it |
| `placeholder` | Search bar placeholder inside the scope |
| `filters` | Segmented filter on the right, cycled with <kbd>⇥</kbd>. The first one is the default and is passed to `query_scoped` |

Users can add an extra prefix per provider in Settings › Extensions (`ExtensionConfig.alias`). Outside a scope, <kbd>⇥</kbd> enters the scope of the selected item's provider. A scope can also be opened from outside with `helmp://scope/<id>`.

## Adding a provider

1. Create `src/providers/<name>.rs` (or a folder) and implement `Provider`. [`src/providers/builtin.rs`](../src/providers/builtin.rs) is the smallest complete example:

   ```rust
   use crate::matcher;
   use crate::provider::{Action, Icon, Item, Provider, Tint};

   pub struct Hello;

   impl Provider for Hello {
       fn id(&self) -> &'static str {
           "hello"
       }

       fn query(&self, query: &str) -> Vec<Item> {
           let Some(score) = matcher::score(query, "hello world") else {
               return Vec::new();
           };
           vec![Item {
               id: "hello:world".into(),
               title: "Hello, world".into(),
               badge: "Examples".into(),
               score,
               icon: Icon::Symbol { name: "hand", tint: Tint::Teal },
               action: Action::copy("Hello, world"),
               action_title: "Copy".into(),
               ..Item::blank()
           }]
       }
   }
   ```

2. Register it in [`src/providers/mod.rs`](../src/providers/mod.rs): add `mod hello;` and a line in `all()`.
3. If it needs a default global hotkey, add an entry to `EXTENSION_SHORTCUTS` in `src/hotkey.rs`. A hotkey either enters the provider's scope (`Command::Scope`) or runs one item directly (`Command::RunItem`).
4. Wrap user-visible text in `tr("中文", "English")` (or `trf!` with arguments) from `src/i18n.rs`. Keep matching keywords separate from display text so both languages find the item.
5. Settings that the user can change go into `Config` in `src/config.rs`. Every field needs a default (`#[serde(default)]`) so older `config.json` files keep loading.
6. Add tests next to the code. Use `HELMP_DATA_DIR` or a temp folder for anything that touches disk.

For features that don't need Rust, a [script command](scripts.md) is usually enough.

## Threads

- GPUI main thread: rendering, key handling, menu bar callbacks (which only push commands into a channel that a foreground task polls).
- One thread per provider for `start()`, plus whatever watchers or pollers it spawns.
- Actions run on GPUI's background executor.
- Hotkey events arrive on the Carbon callback thread and are forwarded to the main thread through a channel polled every 50 ms.
- `helmp://` URLs arrive through GPUI's `on_open_urls` callback and are forwarded through a channel polled every 100 ms.
