# Plugins

[简体中文](plugins.zh-CN.md)

A Helmp plugin is a folder with a `plugin.json` manifest. Most integrations are just "a form → an HTTP request" or "a keyword → a URL", so the manifest describes them declaratively and you don't write code. When you need logic, a command can run a script from the plugin folder, and a list command can show the items a script prints.

Raycast-style [script commands](scripts.md) keep working alongside plugins. Use a script command for a one-off script; use a plugin when you want preferences, secrets, forms, several commands, or something you can share as a folder.

- [Where plugins live](#where-plugins-live)
- [Tutorial: build the flomo plugin](#tutorial-build-the-flomo-plugin)
- [Manifest reference](#manifest-reference)
- [Templates](#templates)
- [Permissions](#permissions)
- [List commands (script output protocol)](#list-commands)
- [Testing your plugin](#testing-your-plugin)
- [Deep links](#deep-links)

## Where plugins live

| Kind | Location | Default |
|---|---|---|
| Bundled | Built into Helmp (flomo, Obsidian, Quick Search). Read-only; the source is in [`resources/plugins/`](../resources/plugins) and doubles as examples | On, can be turned off |
| Yours | `~/Library/Application Support/Helmp/plugins/<folder>/plugin.json` | Off until you turn it on |

Manage them in **Settings › Plugins** (or type "Manage Plugins" in Helmp). The page lists built-in features, bundled plugins and your plugins with version, author, permissions and errors, and has buttons for **Open Plugins Folder**, **Reload**, **New Plugin** (creates a working template folder and shows it in Finder) and **Developer Docs**.

Helmp checks the plugins folder every 3 seconds and reloads when a file changes. A plugin that fails validation stays in the list, marked as an error with the reason; it is never silently skipped.

Turning on one of your plugins first shows what it can do (its [permissions](#permissions)). If you later change the manifest so that it needs more (another host, running scripts…), the plugin turns itself off ("needs permission review") until you confirm again.

### Plugin data

Don't store anything in the plugin folder: an update from the Plugin Center or a plugin source replaces the whole folder. Scripts, list scripts and the plugin's local MCP servers get `HELMP_PLUGIN_DATA_DIR`, which points at `~/Library/Application Support/Helmp/plugin-data/<id>/`. Helmp creates it on first use, and updates and reinstalls never touch it. When you uninstall a plugin in Settings, Helmp asks whether to delete its data too (`helmp plugins uninstall <id> [--delete-data]` from a terminal).

## Tutorial: build the flomo plugin

[flomo](https://flomoapp.com) gives PRO members a personal webhook: `POST https://flomoapp.com/iwh/<token>/<secret>/` with the JSON body `{"content": "…"}` creates a memo, and `#tags` in the text become tags. The whole URL is the secret. Helmp ships this plugin already; building it yourself takes five minutes and covers most of the manifest.

**1. Create the folder.** In Settings › Plugins click **Open Plugins Folder**, then make a folder `com.example.flomo` with a `plugin.json`:

```json
{
  "schemaVersion": 1,
  "id": "com.example.flomo",
  "name": "My flomo",
  "version": "0.1.0",
  "author": "You",
  "icon": "feather",
  "tint": "green",
  "commands": []
}
```

`id` is reverse-DNS (`com.yourname.something`), lowercase. `icon` is any [Lucide](https://lucide.dev/icons) icon name, or a `.png` in the folder.

**2. Add the secret.** The webhook URL is a credential, so declare it as a `secret` preference. Secrets are stored in the macOS Keychain only (service `dev.helmp.plugins`, account `<plugin id>/<preference id>`), never in `config.json`, and are masked as `••••` in messages. Because the whole URL comes from a preference, Helmp can't know the host in advance, so you must say which hosts it may point to:

```json
"preferences": [
  {
    "id": "webhook",
    "type": "secret",
    "title": { "en": "Webhook URL", "zh-Hans": "记录 API 地址" },
    "placeholder": "https://flomoapp.com/iwh/xxxx/yyyy/",
    "required": true,
    "allowedHosts": ["flomoapp.com"]
  }
]
```

**3. Add the command.** A form with one text area, and an `http` action:

```json
"commands": [
  {
    "id": "memo",
    "title": { "en": "Save to flomo", "zh-Hans": "记到 flomo" },
    "keywords": ["flomo", "memo"],
    "form": [
      { "id": "content", "type": "textarea", "label": "Memo", "required": true, "default": "{{query}}" }
    ],
    "action": {
      "type": "http",
      "method": "POST",
      "url": "{{pref.webhook}}",
      "json": { "content": "{{form.content}}" },
      "success": { "jsonPath": "code", "equals": 0, "optional": true },
      "message": { "en": "Saved to flomo", "zh-Hans": "已记录" },
      "errorPath": "message"
    }
  }
]
```

- `default: "{{query}}"`: the text the command's scope starts with (the `q` of a [deep link](#deep-links), or what you type in the search bar inside the scope) pre-fills the form.
- `json` is real JSON; every string inside is a template, and Helmp escapes quotes and line breaks for you. A `Content-Type: application/json` header is added automatically.
- `success`: the request counts as successful when the status is 2xx and, if the response has a `code` field, it equals `0`. Anything else is shown as an error with the server's `message`, never as success.

**4. Check it.**

```bash
helmp plugins validate ~/Library/Application\ Support/Helmp/plugins/com.example.flomo
# OK com.example.flomo 0.1.0 (1 commands)
# Permissions:
#   - Network access: flomoapp.com (URL comes from preference webhook)
```

**5. Turn it on.** Settings › Plugins › My Plugins: click the switch, review the permissions, click **Allow and Turn On**. Click the row to open it and paste your webhook URL. Now type `flomo` in Helmp and press <kbd>Tab</kbd>: the search bar shows a **My flomo** tag, the cursor is in the form, and <kbd>↵</kbd> sends the memo.

## Manifest reference

`docs/plugin-schema.json` is a JSON Schema for editor completion (`"$schema": "https://raw.githubusercontent.com/PeterChen1997/helmp/HEAD/docs/plugin-schema.json"` or a local path). `helmp plugins validate` is the source of truth. Unknown fields are errors, so typos are caught. `"$comment"` is allowed at the top level and in preferences, commands and form fields for notes.

Text fields (`name`, `title`…) take a plain string or `{"en": "…", "zh-Hans": "…"}`; `en` is required and is the fallback.

### Top level

| Field | Required | Description |
|---|---|---|
| `schemaVersion` | yes | `1` |
| `id` | yes | Reverse-DNS, lowercase letters, digits, `-`; unique. Bundled ids can't be reused |
| `name`, `description` | `name` | Text |
| `version` | yes | `x.y.z` |
| `author` | yes | |
| `homepage` | | Shown as a button in Settings |
| `license` | | SPDX license identifier such as `MIT`. Required to publish in the [Plugin Center](plugin-center.md) |
| `icon`, `tint` | | Lucide icon name or `.png` in the folder; tint: `blue` `indigo` `purple` `orange` `teal` `green` `red` `gray` `graphite` |
| `permissions` | | Only `"clipboard.read"` (needed for `{{clipboard}}`) and `"notifications"` (lets scheduled tasks send notifications) can be declared; everything else is derived |
| `preferences` | | See below |
| `commands` | yes* | At least one (*optional when the plugin only bundles `mcpServers`, or only provides `greetings`, `schedules` or `themes`) |
| `mcpServers` | | MCP servers the plugin brings; their tools become commands while the plugin is enabled. See [MCP servers](mcp-client.md#plugins-can-bundle-servers) |
| `greetings` | | Text for the empty search box. See [Search-box greetings](#search-box-greetings) |
| `schedules` | | Scripts that run on a schedule and may send notifications. See [Scheduled tasks and notifications](#scheduled-tasks-and-notifications) |
| `themes` | | Color themes, optionally with a banner image and dates. See [Themes](#themes) |

### Preferences

Shown in Settings › Plugins when you open the plugin.

| Field | Description |
|---|---|
| `id` | Letters, digits, `_`, `-`; referenced as `{{pref.<id>}}` |
| `type` | `text`, `secret` (Keychain only, masked input), `select`, `checkbox` (`"true"`/`"false"`) |
| `title`, `description`, `placeholder` | Text |
| `required` | Commands refuse to run (and point to Settings) until it's filled |
| `default` | Not for secrets. Select: one of the option values |
| `options` | Select only: `[{"title": …, "value": "…"}]` |
| `allowedHosts` | When an http URL starts with `{{pref.<id>}}`: the hosts it may point to |

### Commands

| Field | Description |
|---|---|
| `id` | Unique in the plugin; the result id is `plugins:<plugin id>/<command id>` |
| `title`, `subtitle` | Text. The title is searchable in every language, including pinyin and initials (`ggfy` finds 谷歌翻译) |
| `keywords` | Extra search words. The first one is the "trigger" shown in hints |
| `icon`, `tint` | Override the plugin's |
| `argument` | `{ "placeholder": text, "required": bool }`. Inside the command's scope, the text in the search bar is `{{query}}`; `placeholder` is shown there as the hint |
| `form` | Fields shown in the panel's form view (`text`, `textarea`, `select`), each with `id`, `label`, `placeholder`, `required`, `default` (template: `{{query}}`, dates, non-secret prefs), `options` |
| `action` | What Return does. Exactly one of `action` / `list` |
| `list` | A script-driven list, see [List commands](#list-commands) |

A command with `argument`, `form` or `list` accepts input. So that everyday searching can't trigger a plugin by accident, the main search never takes input for it: typing `flomo` (or `flomo hello`, or `gh rust`) only lists the command, with a <kbd>Tab</kbd> hint. Press <kbd>Tab</kbd> (or <kbd>↵</kbd>) on it to enter the command's own scope: a tag with the plugin's name (or the command's title, for a plugin with several commands) appears left of the search bar, in the command's color, and then

- **form** commands (flomo): the cursor goes straight into the form; <kbd>↵</kbd> submits, <kbd>⇧↵</kbd> adds a line;
- **argument** commands (`gh`, `npm`): what you type in the search bar is `{{query}}`; the result shows the URL it will open, <kbd>↵</kbd> runs it;
- **list** commands: the list script runs with what you type.

<kbd>Esc</kbd>, or <kbd>⌫</kbd> when the search bar (and the form) is empty, goes back to the main search. Commands without input (open a fixed URL, copy something) still run directly with <kbd>↵</kbd> from the main search.

### Actions

| `type` | Fields | Notes |
|---|---|---|
| `open` | `url` | Opens a URL (any scheme: `https:`, `obsidian:`, `things:`…) with macOS `open` |
| `copy` | `text` | Copies to the clipboard |
| `paste` | `text` | Copies, then sends ⌘V to the front app (needs Accessibility permission; otherwise just copies) |
| `http` | `method` (`GET` `POST` `PUT` `PATCH` `DELETE`), `url`, `headers`, `json` or `body`, `timeoutMs` (≤ 30000, default 10000), `success`, `message`, `errorPath` | See below |
| `script` | `path`, `args`, `env`, `stdin`, `timeoutMs` (≤ 120000, default 30000), `output` (`message` default, `silent`, `copy`, `paste`) | Runs a script from the plugin folder |

**http.** The URL must start with `https://` or `http://` and a literal host, or with `{{pref.<id>}}` for a preference that declares `allowedHosts`. A template in the host part is rejected: network access must be known before you enable a plugin. At run time Helmp checks the rendered URL's host against the permissions again, doesn't follow redirects, and rejects header values containing line breaks. `success.status` lists accepted status codes (default: any 2xx); `success.jsonPath` + `success.equals` additionally check a response field (`optional: true` = only when the field exists). `message` is the success text and may use `{{response.<path>}}`; `errorPath` picks the error text from a JSON error response. Failures are shown as errors with the status code and the server's reason.

**script.** `path` is relative to the plugin folder and may not leave it (symlinks are resolved first). Each `args` entry is one argv argument, rendered from a template; nothing goes through a shell, so `; rm -rf` in what you type is just text. `env` adds environment variables (also `HELMP_PLUGIN_DIR`, `HELMP_PLUGIN_ID` and `HELMP_PLUGIN_DATA_DIR`, see [Plugin data](#plugin-data)); `stdin` is written to the script's standard input. The working directory is the plugin folder. Scripts are run directly if executable, otherwise via their `#!` line. A non-zero exit is shown as an error with the end of stderr.

## Templates

Every string marked as a template can use `{{ … }}`:

| Variable | Value |
|---|---|
| `{{query}}` | Text typed in the search bar inside the command's scope (or the deep link's `q`) |
| `{{form.<id>}}` | A form field |
| `{{pref.<id>}}` | A preference (secrets are read from the Keychain only when used) |
| `{{item.title}}` `{{item.subtitle}}` `{{item.arg}}` `{{item.uid}}` `{{item.url}}` | The chosen list item (`list.action` only) |
| `{{response.<path>}}` | A field of the JSON response (`http.message` only), e.g. `data.id`, `items.0.title` |
| `{{clipboard}}` | Clipboard text. Requires `"permissions": ["clipboard.read"]`; read only when used; not available in form defaults or list scripts |
| `{{date}}` `{{time}}` `{{date:%Y/%m/%d %H%M}}` | Local date `2026-11-10`, time `14:05`, or any `strftime` format |

Filters, applied left to right: `|url` (percent-encode; use it for anything inside a URL), `|json` (escape for a JSON string, without the quotes; only needed in a text `body`), `|trim`, `|upper`, `|lower`. Example: `https://www.npmjs.com/search?q={{query|trim|url}}`.

There are no conditions or expressions: if you need logic, use a script. Unknown variables and filters are validation errors, not empty strings.

## Permissions

Permissions are derived from the manifest, shown in Settings, and confirmed when you turn a plugin on:

| Permission | Comes from |
|---|---|
| Network access: `host` | Hosts of `http` URLs, or `allowedHosts` of the preference a URL comes from |
| Open web pages in your browser / Open `obsidian://` links (launches the matching app) | Schemes of `open` URLs |
| Open any link or file | An `open` URL that is entirely a template, or a list without `action` (opens `item.arg`) |
| Read the clipboard | `"permissions": ["clipboard.read"]` |
| Write to the clipboard | `copy` / `paste` actions, `script` output `copy` / `paste` |
| Send ⌘V to the front app | `paste` |
| Run scripts from the plugin folder | `script` actions and `list` commands. Scripts run as you, not sandboxed: only enable plugins you trust |
| Run a local program: `npx` | Each `command` in `mcpServers` (also as you, not sandboxed) |
| Network access: `host` | Also each `url` in `mcpServers` |
| Show messages in the search box | `greetings` |
| Run scripts from the plugin folder in the background on a schedule | `schedules` |
| Send notifications | `"permissions": ["notifications"]` |
| Provide color themes (and switch to them on set dates) | `themes` (with `dates`) |

Bundled plugins are on by default. Your plugins are off until you turn them on; Helmp stores the permissions you confirmed and asks again if they change.

## List commands

A list command runs a script while you type inside its scope (select the command and press <kbd>Tab</kbd>) and shows the items it prints. The output format is [Alfred's Script Filter JSON](https://www.alfredapp.com/help/workflows/inputs/script-filter/json/), so many existing scripts work as they are:

```json
{"items": [
  {"uid": "helmp", "title": "helmp", "subtitle": "~/code/helmp", "arg": "/Users/me/code/helmp",
   "icon": {"path": "icons/repo.png"}, "text": {"copy": "helmp"}},
  {"title": "No more results", "valid": false}
]}
```

| Field | Use |
|---|---|
| `title` | Required |
| `subtitle`, `arg`, `uid`, `quicklookurl` | Shown / available as `{{item.subtitle}}`, `{{item.arg}}`, `{{item.uid}}`, `{{item.url}}`. `arg` may be an array (joined with line breaks) |
| `icon.path` | A png/jpg (relative to the plugin folder or absolute); `"type": "fileicon"` uses the file's Finder icon |
| `valid: false` | A hint row; Return does nothing |
| `text.copy` | What ⌘K › Copy copies (default: `arg`) |

```json
{
  "id": "recent",
  "title": "Recent Projects",
  "keywords": ["proj"],
  "list": {
    "script": { "path": "bin/recent.sh", "args": ["{{query}}"], "timeoutMs": 1000 },
    "action": { "type": "open", "url": "{{item.arg}}" },
    "minQueryLength": 0,
    "cacheSeconds": 5
  }
}
```

The script gets `{{query}}` and preferences (no form, no clipboard), must finish within `timeoutMs` (default 1000, max 3000), and its result for the same input is cached for `cacheSeconds` (at least 1). Up to 200 items are shown. If the script fails or prints something that isn't the JSON above, the list shows one error row with the reason. Without `action`, choosing an item opens `item.arg` as a URL or path.

## Search-box greetings

A plugin can put a line of text in the search box: a quote of the day, a tip, a note for a special date. It's shown as the search box's placeholder, so it's only visible in the global search while the box is empty and disappears as soon as you type. Inside a command's scope, the scope's own hint is shown instead, and "Indexing apps…" still comes first right after launch.

```json
{
  "schemaVersion": 1,
  "id": "com.example.daily-quote",
  "name": "Daily Quote",
  "version": "1.0.0",
  "author": "Example",
  "greetings": {
    "rotation": "daily",
    "items": [
      { "en": "Well begun is half done.", "zh-Hans": "良好的开端是成功的一半。" },
      "Done is better than perfect."
    ],
    "dates": [
      { "date": "01-01", "text": "Happy New Year!" },
      { "date": "2027-06-01", "text": "Only on June 1, 2027" }
    ]
  }
}
```

A complete example is in [`examples/plugins/com.example.daily-quote`](../examples/plugins/com.example.daily-quote/plugin.json). A plugin may consist of `greetings` alone, with no commands.

| Field | Description |
|---|---|
| `rotation` | `open` (default): the next item every time the panel opens. `daily`: one item per day, in order, continuing across months and years |
| `items` | Text entries to rotate through |
| `dates` | `{ "date", "text" }` entries for specific days. `MM-DD` repeats every year; `YYYY-MM-DD` is that day only |

Rules:

- **Dates win.** On a day with a matching `dates` entry, it replaces the rotation. A `YYYY-MM-DD` entry beats an `MM-DD` one for the same day; several entries for the same day take turns by `rotation`. A yearly `02-29` shows on February 28 in common years, unless that day has its own `02-28` entry.
- **Local time.** "Today" is the date in your Mac's time zone.
- **One plugin at a time.** When several enabled plugins provide greetings, a plugin with a dated entry for today wins; otherwise the plugin whose id sorts first is used. Turn a plugin's greetings off in Settings › Plugins (open the plugin, "Search Box Messages") to let the next one show.
- **Limits.** At least one entry in `items` or `dates`, at most 1000 in total; each text is one line of up to 120 characters. Text is plain, localized like any other text field.
- **Cheap by design.** Greetings are declarative: nothing is fetched, no script runs, and picking one is an in-memory lookup when the panel opens. For text that changes on its own (weather, a feed), use a list command or update the plugin.

Greetings are a permission ("Show messages in the search box"): you confirm it when you turn the plugin on, and a plugin that adds greetings in an update needs your review again.

## Themes

A plugin can provide color themes. You pick one in Settings › Appearance, under the Light / Dark / Auto choice: a theme only changes colors, and light or dark still follows that setting, so every theme has a light set and a dark set. It applies to the launcher panel, Settings and the welcome window.

```json
{
  "schemaVersion": 1,
  "id": "com.example.seasonal-themes",
  "name": "Seasonal Themes",
  "version": "1.0.0",
  "author": "Example",
  "themes": [
    {
      "id": "winter",
      "name": { "en": "Winter Holidays", "zh-Hans": "冬日节庆" },
      "light": { "background": "#fdf8f2", "foreground": "#2b1d17", "accent": "#b3261e", "selection": "#b3261e" },
      "dark": { "background": "#182030", "foreground": "#f3efe6", "accent": "#e0b64a", "selection": "#9a2a22" },
      "image": { "light": "images/winter-light.png", "dark": "images/winter-dark.png" },
      "dates": [{ "from": "12-20", "to": "12-26" }]
    }
  ]
}
```

A complete example with two themes and their images is in [`examples/plugins/com.example.seasonal-themes`](../examples/plugins/com.example.seasonal-themes/plugin.json). A plugin may consist of `themes` alone, with no commands; it can provide up to 10.

| Field | Description |
|---|---|
| `id` | Unique within the plugin (letters, digits, `_`, `-`) |
| `name` | Text shown on the theme's card in Settings |
| `light`, `dark` | Colors, each `#rrggbb`: `background` (panel and windows; Helmp adds its own translucency), `foreground` (text), `accent` (switches, selected labels, primary buttons), `selection` (the selected row) |
| `image` | Optional `{ "light", "dark" }` images in the plugin folder (at least one): png, jpg, webp or gif, up to 4 MiB. Shown behind the search bar only, under a layer of the background color, so result rows always sit on plain color |
| `dates` | Optional `{ "from", "to" }` ranges, both ends included (`to` defaults to `from`). Both `MM-DD` (every year; a `from` later than `to` wraps over New Year) or both `YYYY-MM-DD` (that range only) |

Only these four colors are open; everything else (secondary text, sidebar, text on the selection and on buttons) is derived from them. Validation checks that the result is readable, and reports the field and the ratio when it isn't:

- `foreground` on `background`: at least 4.5:1; secondary text (mixed from the two): at least 3:1.
- `accent` on `background`: at least 3:1.
- The light set's background must be lighter than its text, and the dark set's darker; system controls such as text fields follow light or dark.
- With an image: the same text ratios against the image's average color, mixed half-and-half with the background (the least cover the search bar gets). Use a light image for the light set and a dark one for the dark set. A busy image can still look noisy even when its average passes, so check it in the panel.
- Text on the selection and on the accent is white or black, whichever reads better.

Dates:

- **Dated themes switch on by themselves.** On a day inside one of its ranges, the theme replaces your choice and hands it back the day after the range ends. Your choice itself is never changed. Settings › Appearance says which theme is on and until when, with a **Not today** button (and **Use it today** to undo). The **Switch themes by date** switch turns this off.
- **Local time.** "Today" is the date in your Mac's time zone. A yearly `02-29` (as either end) falls on February 28 in common years.
- **One at a time.** When several themes are inside their dates, the first by plugin id (then manifest order) wins.
- **Turning the plugin off** (or uninstalling it) returns to the default colors right away. Your choice is kept, so turning the plugin back on restores it.

Themes are a permission ("Provide color themes", or "…and switch to them on set dates" when any theme has `dates`): you confirm it when you turn the plugin on, and a plugin that adds themes or dates in an update needs your review again.

## Scheduled tasks and notifications

A plugin can run a script from its folder on a schedule — every so many minutes, every day at a set time, or on a date — and the script can ask Helmp to post a macOS notification. Use it for a morning weather note, a reminder on a date, or syncing data now and then.

```json
{
  "schemaVersion": 1,
  "id": "com.example.weather-reminder",
  "name": "Weather Reminder",
  "version": "1.0.0",
  "author": "Example",
  "permissions": ["notifications"],
  "preferences": [
    { "id": "city", "type": "text", "title": "City", "default": "Berlin" }
  ],
  "schedules": [
    { "id": "morning", "title": "Morning forecast", "at": "08:00",
      "script": { "path": "forecast.sh", "env": { "CITY": "{{pref.city}}" } } },
    { "id": "refresh", "everyMinutes": 60, "script": { "path": "refresh.sh" } },
    { "id": "new-year", "date": "01-01", "at": "09:00", "script": { "path": "new-year.sh" } }
  ]
}
```

A complete example is in [`examples/plugins/com.example.weather-reminder`](../examples/plugins/com.example.weather-reminder/plugin.json). A plugin may consist of `schedules` alone, with no commands.

| Field | Description |
|---|---|
| `id` | Letters, digits, `_`, `-`; unique within the plugin |
| `title` | Text shown in Settings (default: `id`) |
| `everyMinutes` | Run every N minutes, 5 to 10080 (a week) |
| `at` | Run every day at this local time, 24-hour `HH:MM` |
| `date` | Run on this date: `MM-DD` every year, or `YYYY-MM-DD` that day only (same as greetings). With `at`, at that time; without, as soon as Helmp runs that day |
| `script.path` | Script in the plugin folder (run like `script` actions: executable or with a shebang, no shell) |
| `script.args`, `script.env` | Templates; only `{{pref.<id>}}`, `{{date}}` and `{{time}}` are available (no input, form or clipboard in the background) |
| `script.timeoutMs` | Default 30000, max 120000 |

Use exactly one of `everyMinutes`, `at`, or `date` (optionally with `at`).

**When tasks run.**

- Only while Helmp is running. There is no background service when Helmp is quit.
- A daily or dated task runs once per day. If its time passed while Helmp wasn't running or the Mac was asleep, it runs once when Helmp starts or the Mac wakes — on the same day only; missed earlier days are skipped.
- An interval task runs when its interval has passed since the last run. After a long absence it runs once, not once per missed interval.
- Times and dates follow your Mac's time zone. On a daylight-saving change a task still runs once: a time skipped by the clock runs right after the jump, and a repeated hour doesn't run it twice. A yearly `02-29` runs on February 28 in common years.
- At most 2 scheduled scripts run at a time across all plugins, and a task never overlaps with itself. While a plugin's script is running, automatic updates skip that plugin until next time.
- The last run of each task is kept in Helmp's own data (`plugin-schedules.json`), not in the plugin's data folder, so updates and reinstalls don't reset it.

**Output and notifications.** Print nothing, or one JSON object on stdout:

```json
{"notify": {"title": "Rain this afternoon", "body": "Take an umbrella", "url": "https://example.com/forecast"}}
```

`title` is required (up to 100 characters), `body` is optional (up to 500), and `url` (http or https) is opened when the notification is clicked. Anything else on stdout counts as a failure, so write logs to stderr. Sending a notification needs `"permissions": ["notifications"]`. The first notification asks macOS for permission; if it's denied, Helmp doesn't ask again — the task's result says notifications are off and how to turn them on in System Settings › Notifications.

**Stopping.** Turning the plugin off, uninstalling it, a pending permission review, or turning a single task off in Settings stops its running script immediately, child processes included. Quitting Helmp does the same. A script that runs past its timeout is killed the same way.

**Settings.** Settings › Plugins lists each task under the plugin with its schedule, the last run's time and result, and a switch to turn that task off.

**Trying it from a terminal.**

```bash
helmp plugins schedules                                  # tasks of enabled plugins: id, schedule, last run, result
helmp plugins schedules run com.example.weather-reminder/morning   # run once now and print the result
helmp plugins schedules watch --for 120                  # run the scheduler in the foreground for two minutes
```

Running scripts in the background and sending notifications are permissions ("Run scripts from the plugin folder in the background on a schedule", "Send notifications"): you confirm them when you turn the plugin on, and a plugin that adds them in an update needs your review again.

## Testing your plugin

```bash
# Validate: schema, template variables, hosts, script paths. Lists every problem; exits 1 on failure.
helmp plugins validate path/to/plugin
# Same, as JSON (ok, id, version, commands, permissions, permissions_text, errors) for scripts and agents.
helmp plugins validate path/to/plugin --json

# List plugins and their state (enabled / disabled / needs-consent / error).
helmp plugins

# Run a command without side effects: opens, copies, pastes and requests to real hosts are only printed.
# The main search only lists a command that takes input; Return on it just enters its scope.
HELMP_DRY_RUN=1 helmp query "npm"
HELMP_DRY_RUN=1 helmp run plugins:dev.helmp.quicklinks/npm npm        # enter scope plugins:dev.helmp.quicklinks/npm
# --scope <command result id> is the same as selecting the command and pressing Tab.
HELMP_DRY_RUN=1 helmp query --scope plugins:dev.helmp.quicklinks/npm react
HELMP_DRY_RUN=1 helmp run plugins:dev.helmp.quicklinks/npm react --scope plugins:dev.helmp.quicklinks/npm
HELMP_DRY_RUN=1 helmp run plugins:com.example.flomo/memo hi --scope plugins:com.example.flomo/memo --form content="hello #test"
```

When the main search doesn't list the result and its id is a command scope, `helmp run` looks inside that scope, so `helmp run plugins:dev.helmp.quicklinks/npm react` also works.

In dry-run mode `{{clipboard}}` becomes `[clipboard]` instead of reading your clipboard, and an http request to a real host prints `[dry-run] http POST <url> <body>` (secrets masked) instead of being sent. Scripts still run, like script commands do.

For automated tests, run `helmp` with an isolated data folder: `HELMP_DATA_DIR=<temp dir> HELMP_DRY_RUN=1`. In that mode only (both set):

- Secrets come from `HELMP_TEST_SECRETS='{"<plugin id>/<preference id>": "value"}'` and the Keychain is never touched. Outside this mode the variable is ignored.
- Requests to `127.0.0.1` / `localhost` are allowed and actually sent, so a plugin locked to `flomoapp.com` can be pointed at a local mock server.
- `HELMP_SCHEDULE_MINUTE_MS` shortens a "minute" for scheduled tasks and `HELMP_SCHEDULE_TICK_MS` sets how often the scheduler checks, so `helmp plugins schedules watch --for 3` can run a five-minute task several times. Notifications are printed as `[dry-run] notify <title> | <body> | <url>` instead of being posted.

Helmp's own end-to-end tests ([`tests/e2e_plugins.rs`](../tests/e2e_plugins.rs)) use exactly this setup.

## Publish to the Plugin Center

The [Plugin Center](plugin-center.md) lists reviewed plugins that anyone can install with one click. To publish yours:

1. Add `"license"` (an SPDX id such as `"MIT"`) to `plugin.json`, and a `README.md` that explains what the plugin does, its commands and how to set it up. A 256×256 `icon.png` is optional.
2. Keep the folder clean: UTF-8 text files and PNG images only, no symlinks or hidden files, at most 64 files, 512 KiB per file and 2 MiB in total. Pick an id under a domain you own (not `dev.helmp.*`).
3. Run `helmp plugins validate <folder>`.
4. Run `helmp plugins submit <folder>`. It checks the same rules as the registry's CI, prints the permissions, files and what it will do, then stops without changing anything. Run `helmp plugins submit <folder> --yes` to go ahead. It needs the [GitHub CLI](https://cli.github.com/) signed in (`gh auth login`). It forks [PeterChen1997/helmp-plugins](https://github.com/PeterChen1997/helmp-plugins) with your account, pushes a branch named `plugin/<id>-<version>` with your files in `plugins/<id>/`, and opens a pull request whose description lists the permissions, every file's SHA-256 and the reviewer checklist. It prints the pull request's URL.
5. A maintainer reviews the pull request. Once it's merged, the registry rebuilds `index.json` and the plugin shows up in the Plugin Center.

To publish an update, bump `version` and submit again. Users who installed it see **Update available**, and if the permissions changed they approve them again.

Coding agents (Claude Code, Codex) can build, validate and submit a plugin for you through Helmp's MCP server (`helmp mcp`). They must show you the plan and get your OK before running the submit step.

## Deep links

`helmp://plugin/<plugin id>/<command id>?q=<text>` invokes a command. Commands that only open a link (and have what they need) run immediately. A command that takes input opens the panel inside its scope with `<text>` as the input (the flomo form pre-filled) and waits for you to press Return; anything else is shown in the panel and waits too, so a link on a web page can't submit anything for you. See [deeplinks.md](deeplinks.md).

```bash
open "helmp://plugin/dev.helmp.quicklinks/npm?q=react"
open "helmp://plugin/dev.helmp.flomo/memo?q=%23idea%20write%20docs"
```
