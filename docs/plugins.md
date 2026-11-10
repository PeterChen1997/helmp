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

- `default: "{{query}}"`: typing `flomo read a great book #reading` pre-fills the form with `read a great book #reading`.
- `json` is real JSON; every string inside is a template, and Helmp escapes quotes and line breaks for you. A `Content-Type: application/json` header is added automatically.
- `success`: the request counts as successful when the status is 2xx and, if the response has a `code` field, it equals `0`. Anything else is shown as an error with the server's `message`, never as success.

**4. Check it.**

```bash
helmp plugins validate ~/Library/Application\ Support/Helmp/plugins/com.example.flomo
# OK com.example.flomo 0.1.0 (1 commands)
# Permissions:
#   - Network access: flomoapp.com (URL comes from preference webhook)
```

**5. Turn it on.** Settings › Plugins › My Plugins: click the switch, review the permissions, click **Allow and Turn On**. Click the row to open it and paste your webhook URL. Now type `flomo` in Helmp.

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
| `permissions` | | Only `["clipboard.read"]` can be declared (needed for `{{clipboard}}`); everything else is derived |
| `preferences` | | See below |
| `commands` | yes | At least one |

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
| `argument` | `{ "placeholder": text, "required": bool }`. Typing a keyword (or the title), a space and some text passes the text as `{{query}}` |
| `form` | Fields shown in the panel's form view (`text`, `textarea`, `select`), each with `id`, `label`, `placeholder`, `required`, `default` (template: `{{query}}`, dates, non-secret prefs), `options` |
| `action` | What Return does. Exactly one of `action` / `list` |
| `list` | A script-driven list, see [List commands](#list-commands) |

A command with `argument`, `form` or `list` accepts input: `flomo hello` passes `hello` as `{{query}}`.

### Actions

| `type` | Fields | Notes |
|---|---|---|
| `open` | `url` | Opens a URL (any scheme: `https:`, `obsidian:`, `things:`…) with macOS `open` |
| `copy` | `text` | Copies to the clipboard |
| `paste` | `text` | Copies, then sends ⌘V to the front app (needs Accessibility permission; otherwise just copies) |
| `http` | `method` (`GET` `POST` `PUT` `PATCH` `DELETE`), `url`, `headers`, `json` or `body`, `timeoutMs` (≤ 30000, default 10000), `success`, `message`, `errorPath` | See below |
| `script` | `path`, `args`, `env`, `stdin`, `timeoutMs` (≤ 120000, default 30000), `output` (`message` default, `silent`, `copy`, `paste`) | Runs a script from the plugin folder |

**http.** The URL must start with `https://` or `http://` and a literal host, or with `{{pref.<id>}}` for a preference that declares `allowedHosts`. A template in the host part is rejected: network access must be known before you enable a plugin. At run time Helmp checks the rendered URL's host against the permissions again, doesn't follow redirects, and rejects header values containing line breaks. `success.status` lists accepted status codes (default: any 2xx); `success.jsonPath` + `success.equals` additionally check a response field (`optional: true` = only when the field exists). `message` is the success text and may use `{{response.<path>}}`; `errorPath` picks the error text from a JSON error response. Failures are shown as errors with the status code and the server's reason.

**script.** `path` is relative to the plugin folder and may not leave it (symlinks are resolved first). Each `args` entry is one argv argument, rendered from a template; nothing goes through a shell, so `; rm -rf` in what you type is just text. `env` adds environment variables (also `HELMP_PLUGIN_DIR` and `HELMP_PLUGIN_ID`); `stdin` is written to the script's standard input. The working directory is the plugin folder. Scripts are run directly if executable, otherwise via their `#!` line. A non-zero exit is shown as an error with the end of stderr.

## Templates

Every string marked as a template can use `{{ … }}`:

| Variable | Value |
|---|---|
| `{{query}}` | Text typed after the keyword |
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

Bundled plugins are on by default. Your plugins are off until you turn them on; Helmp stores the permissions you confirmed and asks again if they change.

## List commands

A list command runs a script while you type (after `keyword` + space) and shows the items it prints. The output format is [Alfred's Script Filter JSON](https://www.alfredapp.com/help/workflows/inputs/script-filter/json/), so many existing scripts work as they are:

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

## Testing your plugin

```bash
# Validate: schema, template variables, hosts, script paths. Lists every problem; exits 1 on failure.
helmp plugins validate path/to/plugin
# Same, as JSON (ok, id, version, commands, permissions, permissions_text, errors) for scripts and agents.
helmp plugins validate path/to/plugin --json

# List plugins and their state (enabled / disabled / needs-consent / error).
helmp plugins

# Run a command without side effects: opens, copies, pastes and requests to real hosts are only printed.
HELMP_DRY_RUN=1 helmp query "npm react"
HELMP_DRY_RUN=1 helmp run plugins:dev.helmp.quicklinks/npm "npm react"
HELMP_DRY_RUN=1 helmp run plugins:com.example.flomo/memo "flomo hi" --form content="hello #test"
```

In dry-run mode `{{clipboard}}` becomes `[clipboard]` instead of reading your clipboard, and an http request to a real host prints `[dry-run] http POST <url> <body>` (secrets masked) instead of being sent. Scripts still run, like script commands do.

For automated tests, run `helmp` with an isolated data folder: `HELMP_DATA_DIR=<temp dir> HELMP_DRY_RUN=1`. In that mode only (both set):

- Secrets come from `HELMP_TEST_SECRETS='{"<plugin id>/<preference id>": "value"}'` and the Keychain is never touched. Outside this mode the variable is ignored.
- Requests to `127.0.0.1` / `localhost` are allowed and actually sent, so a plugin locked to `flomoapp.com` can be pointed at a local mock server.

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

`helmp://plugin/<plugin id>/<command id>?q=<text>` invokes a command, as if you typed its keyword, a space and `<text>`. Commands that only open a link run immediately; commands with a form, an http request, a script or clipboard access are shown in the panel and wait for you to press Return, so a link on a web page can't submit anything for you. See [deeplinks.md](deeplinks.md).

```bash
open "helmp://plugin/dev.helmp.quicklinks/npm?q=react"
open "helmp://plugin/dev.helmp.flomo/memo?q=%23idea%20write%20docs"
```
