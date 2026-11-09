# Architecture

Helmp is a single Rust binary. The UI is drawn with [GPUI](https://www.gpui.rs) (through `gpui-kit`), and everything macOS-specific goes through `objc2` bindings. Features are **providers**: modules that return plain data. The launcher renders that data and owns all key handling, so a provider never draws UI.

```
                    ┌──────────────── main.rs ────────────────┐
                    │ Engine::new(providers::all())           │
                    │ args? → cli.rs (headless)               │
                    │ else  → GPUI app (Accessory, no Dock)   │
                    └───────┬───────────────┬─────────────────┘
                            │               │
        hotkey.rs ──────────┤               ├────────── menubar.rs
   (Carbon global hotkeys)  │               │       (NSStatusItem + NSMenu)
                            ▼               ▼
                    launcher.rs  ◀──── panel.rs (NSVisualEffectView glass,
              (search bar, list,          window placement per screen)
               detail, form, ⌘K)
                            │ query / query_scoped / suggestions / Action
                            ▼
                       engine.rs ──── history.rs ("Recent")
                            │
     ┌──────────┬───────────┼───────────┬────────────┬────────────┬───────────┐
     ▼          ▼           ▼           ▼            ▼            ▼           ▼
  builtin     apps      screenshot  clipboard     scripts      agents     translate
                                                                              │
                                                                       models/ (Anthropic,
                                                                       OpenAI-compatible,
                                                                       Ollama; Keychain)

  config.rs ── read by everyone, written by settings/ ── onboarding/ (first-run permissions)
```

## Modules

| Path | What it does |
|---|---|
| `src/main.rs` | Entry point. Builds the engine, dispatches CLI subcommands, otherwise opens the panel window, registers hotkeys, installs the menu bar item and shows onboarding if needed. Also hosts the `HELMP_SELF_TEST` GUI self-test. |
| `src/provider.rs` | The provider interface: `Provider`, `Item`, `Action`, `Outcome`, `Scope` and the view types. Described below. |
| `src/providers/` | One folder or file per provider; `providers/mod.rs` is the registry. |
| `src/engine.rs` | Starts providers, routes queries, resolves scope prefixes (`cb `), builds the home view and sorts / groups results. |
| `src/launcher.rs` | The 880 × 540 panel: search bar, list / list + detail / form + detail views, status bar, ⌘K menu and all key handling. Runs actions and interprets their `Outcome`. |
| `src/panel.rs` | Platform glue for the panel window: frosted-glass background and per-screen placement. |
| `src/hotkey.rs` | Global hotkeys via `global-hotkey` (Carbon, no permission needed). Parses `alt-space` style strings, re-registers incrementally when settings change. |
| `src/menubar.rs` | Menu bar icon (template image + orange dot when a session is waiting) and its menu. |
| `src/config.rs` | `<data_dir>/config.json`: `get()` snapshot, `update()` write-and-notify, `subscribe()`. |
| `src/history.rs` | `<data_dir>/history.json`: the "Recent" row on the home view; replays an entry by querying its id again. |
| `src/matcher.rs` | Shared scoring: literal rules plus pinyin rules (full pinyin, initials, polyphonic characters). |
| `src/models/` | Bring-your-own model clients, Keychain storage for API keys, and reading the selected text via Accessibility. |
| `src/settings/` | The settings window. Pure mapping logic lives in `settings/model.rs` so it can be unit tested. |
| `src/onboarding/` | First-run permission window and the drag-to-authorize card shown on top of System Settings. |
| `src/permissions.rs` | Read-only checks for Accessibility and Screen Recording. Never triggers a system prompt. |
| `src/icons.rs` | Renders app / file icons to PNG in `<data_dir>/icon-cache/` on a background thread. |
| `src/input_source.rs` | Switches to the ABC layout while the panel is visible and restores the previous input method on hide. |
| `src/paths.rs` | Data folder: `~/Library/Application Support/Helmp/`, overridable with `HELMP_DATA_DIR`. |
| `src/style.rs` | Light and dark palettes shared by the panel, settings and onboarding. |
| `src/dock.rs` | Temporarily shows a Dock icon while a regular window (settings, onboarding) is open. |
| `src/cli.rs` | `helmp query`, `helmp run`, `helmp providers`. |
| `src/trace.rs` | `HELMP_TRACE=1` logs UI events to stderr. |

### Built-in providers

| id | Scope prefixes | Background work |
|---|---|---|
| `builtin` | — | none (Settings, Quit) |
| `apps` | — | scans `/Applications`, `/System/Applications`, `~/Applications`; FSEvents watcher, rebuild debounced by 1 s |
| `screenshot` | — | none; runs `screencapture -i` and Apple Vision on demand |
| `clipboard` | `cb `, `clip `, `剪贴板 `, `剪切板 ` | polls `NSPasteboard.changeCount` every 300 ms, stores in SQLite |
| `scripts` | — (matched by title or `packageName`) | rescans script folders every 5 s, re-parses only changed files |
| `agents` | `ag `, `agent ` | polls Claude Code / Codex state every 1 s, reads only the last 64 KB of any log |
| `translate` | `fy `, `tr `, `翻译 ` | caches the selected text while the panel is visible |

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
}
```

Rules every provider follows:

- `query`, `query_scoped` and `suggestions` run on every keystroke. They must only read memory and return within milliseconds. Indexing, watching and polling happen in `start()` or on threads it spawns.
- `Item.id` is `<provider id>:<domain id>` and must be stable: history replays an item by querying its id again.
- An empty global query should usually return nothing; put home-view content in `suggestions()`.

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

Use `..Item::blank()` to fill in the fields you don't need.

### `Detail` and `Form`

A `Detail` has an icon, title, optional tag, a list of `Block`s (`Text`, `Code`, `Quote { label, text }`, `Image(path)`), a key–value `metadata` table, and `actions` used when the detail is the *result* of running something (the first one becomes <kbd>↵</kbd>). There are no buttons in the detail pane; actions always go through <kbd>↵</kbd> and <kbd>⌘K</kbd>.

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

### `Scope`

A provider can declare one scope. Inside it, the search bar shows a colored label and only that provider is queried.

| Field | Meaning |
|---|---|
| `id` | Usually the provider id |
| `label`, `tint` | The label shown in the search bar |
| `prefixes` | Typing one of these enters the scope. Include the trailing space (`"cb "`), otherwise `cbr` or `clipboard` would trigger it |
| `placeholder` | Search bar placeholder inside the scope |
| `filters` | Segmented filter on the right, cycled with <kbd>⇥</kbd>. The first one is the default and is passed to `query_scoped` |

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
4. Settings that the user can change go into `Config` in `src/config.rs`. Every field needs a default (`#[serde(default)]`) so older `config.json` files keep loading.
5. Add tests next to the code. Use `HELMP_DATA_DIR` or a temp folder for anything that touches disk.

For features that don't need Rust, a [script command](scripts.md) is usually enough.

## Threads

- GPUI main thread: rendering, key handling, menu bar callbacks (which only push commands into a channel that a foreground task polls).
- One thread per provider for `start()`, plus whatever watchers or pollers it spawns.
- Actions run on GPUI's background executor.
- Hotkey events arrive on the Carbon callback thread and are forwarded to the main thread through a channel polled every 50 ms.
