# Plugin Center

English · [简体中文](plugin-center.zh-CN.md)

The Plugin Center is a list of [plugins](plugins.md) that a maintainer has reviewed. Browse it on the web at <https://blog.peterchen97.cn/helmp/plugins/> or in **Settings › Plugins › Install from Plugin Center**.

Plugins live in a public GitHub repository, [PeterChen1997/helmp-plugins](https://github.com/PeterChen1997/helmp-plugins), one folder per plugin (`plugins/<id>/`). A script in that repository generates `index.json`, which lists every plugin's version, permissions, and the SHA-256 and size of every file. Helmp downloads the index and files through jsDelivr.

## Install

Pick whichever is handy:

- On the web page, click **Install in Helmp**. It opens `helmp://install-plugin?id=<id>`.
- In **Settings › Plugins**, click **Browse Plugin Center**, then **Install** next to a plugin.
- In a terminal, run `helmp plugins install <id>`. Run `helmp plugins available` to list what's in the Plugin Center.

Installed plugins go to `~/Library/Application Support/Helmp/plugins/<id>/` and start **turned off**. Settings opens on the plugin and shows what it can do: which hosts it reaches, whether it runs scripts, and so on. Nothing runs until you click **Allow and Turn On**. A link on a web page can install a plugin, but it can never turn one on.

Some ids are already bundled with Helmp, such as `dev.helmp.flomo`, `dev.helmp.quicklinks` and `dev.helmp.obsidian`. Helmp won't install a second copy over a bundled plugin. It explains that the plugin is already there, and you turn it on in Settings. Helmp also won't overwrite a plugin you put in the plugins folder yourself under the same id. Move that one away first.

## Updates

Plugins installed from the Plugin Center or a plugin source update themselves. With **Settings › Plugins › Automatically update plugins** on (the default), Helmp checks once a day, 20 seconds after launch and then every 24 hours, on the same schedule as Helmp's own update check. Each source's index is fetched once, and each plugin is compared only with the source it was installed from. The same section shows when the last check ran and what happened, and has a **Check Now** button.

- If the new version needs the same permissions you already allowed and no file of a plugin that runs code changed, it's installed quietly and stays on.
- If its permissions changed (a new host, a different MCP server command line, or it now runs scripts), it's installed but paused and marked **needs permission review** until you approve it again. The same happens when an update changes any file of a plugin that runs code (scripts, schedules, MCP servers): the scripts you approved are the ones that were installed then, so new files wait for you. A version bump alone doesn't count.
- If anything fails (offline, HTTP 401, a file that doesn't match its SHA-256), nothing changes: the old version stays as it was, you aren't interrupted, and Helmp tries again in an hour.
- A plugin whose script or MCP server is running at that moment is skipped and updated on the next try.

You can also update by hand: `helmp plugins update` (or `--all`) checks every plugin, `helmp plugins update <id>` checks one, and `helmp plugins install <id>` still works. When you open **Browse Plugin Center**, plugins with a newer version show **Update to x.y.z**. Turn the switch off and Helmp stops checking on its own; `helmp plugins update --scheduled` behaves like the daily check, so it does nothing while the switch is off.

An update downloads and verifies the new version, then replaces the plugin folder. Data a plugin keeps in its data folder (`HELMP_PLUGIN_DATA_DIR`, see [Plugin data](plugins.md#plugin-data)) is kept.

## Uninstall

Expand a plugin in **Settings › Plugins** and click **Uninstall…**. If the plugin has stored data, Helmp asks whether to delete it too or keep it for a reinstall. Uninstalling also forgets that you turned the plugin on, so a reinstall starts turned off. From a terminal: `helmp plugins uninstall <id>` keeps the data, `--delete-data` deletes it. Plugins you put in the plugins folder by hand aren't removed this way; delete their folder yourself.

## Security model

- **Human review.** Every plugin reaches the Plugin Center through a pull request that a maintainer reviews: permissions match what the plugin does, hosts are expected, scripts aren't obfuscated and don't fetch and run remote code, and the plugin works as its README says. A CI check enforces the format and file rules and flags scripts for manual review. It never runs plugin code.
- **Verified downloads.** Helmp downloads only the files listed in the index and checks each file's size and SHA-256 before writing anything. If any file doesn't match (a tampered file or a CDN glitch), the whole install is refused and nothing is written. Paths that would leave the plugin folder (`../`, absolute paths, hidden files) make Helmp reject the whole index. On jsDelivr, files are fetched from the exact registry commit the index was built from, so a stale CDN cache can't mix versions.
- **Atomic install.** Files are written to a hidden temporary folder and validated with the same rules as `helmp plugins validate`. The manifest's id and version must match the index. Only then is the folder renamed into place. A failed update leaves the previous version untouched.
- **Consent.** Installed plugins are off by default. You approve the derived permissions before a plugin runs, and approve again whenever they change. Secrets stay in the Keychain.
- **Limits.** Up to 64 files per plugin, 512 KiB per file and 2 MiB in total. Only UTF-8 text files and PNG icons are allowed: no symlinks and no other binaries. Helmp checks each downloaded file again: an image must really be the format its extension says (by its header), and everything else must be UTF-8 text.

What's left to trust: the maintainer's review, GitHub, jsDelivr's TLS, and the scripts a plugin runs with your user's permissions once you turn it on. Only turn on plugins whose permissions make sense to you.

## Custom plugin sources

Besides the public Plugin Center, Helmp can browse and install from **custom plugin sources**: registries with the same layout that you host yourself, like a Homebrew tap or a team's private store. The Plugin Center is always the first source; custom sources are added after it.

### Add a source

- **Settings › Plugins › Plugin Sources › Add Source**: a name, the URL, and optionally an auth header name and a token.
- **Terminal**: `helmp plugins source add <name> <url> [--header <name>] [--token-stdin]`, for example `printf 'Bearer …' | helmp plugins source add Team https://plugins.example.com/team/ --token-stdin`. `helmp plugins source list` shows the sources (never the token); `helmp plugins source remove <name>` removes one and its token.
- **Link**: `helmp://add-plugin-source?url=<url>&name=<name>` opens Settings with the form filled in. Nothing is added until you click **Add**, and links can't carry a token: a link with any other parameter is ignored.

The URL must use `https` (plain `http` is only accepted for `localhost` / `127.0.0.1`, for testing) and can't contain a user name, password, query or fragment.

### Layout

A source is a folder served over HTTPS, laid out exactly like the Plugin Center:

```text
<url>/index.json
<url>/plugins/<id>/plugin.json
<url>/plugins/<id>/<other files listed in index.json>
```

`index.json` (`"schema": 1`) lists each plugin's `id`, `version`, `name`, `author`, optional `description`, `permissions` and `permissions_text`, and `files`: every file's `path`, `sha256`, `size` and `executable`. One way to generate it, for a folder `plugins/<id>/…`:

```sh
python3 - <<'PY'
import hashlib, json, os, subprocess
plugins = []
for pid in sorted(os.listdir("plugins")):
    root = os.path.join("plugins", pid)
    info = json.loads(subprocess.run(["helmp", "plugins", "validate", root, "--json"],
                                     capture_output=True, text=True, check=True).stdout)
    manifest = json.load(open(os.path.join(root, "plugin.json")))
    files = []
    for dirpath, _, names in os.walk(root):
        for name in sorted(names):
            path = os.path.join(dirpath, name)
            data = open(path, "rb").read()
            files.append({"path": os.path.relpath(path, root), "sha256": hashlib.sha256(data).hexdigest(),
                          "size": len(data), "executable": os.access(path, os.X_OK)})
    plugins.append({"id": pid, "version": info["version"], "name": manifest["name"],
                    "author": manifest["author"], "description": manifest.get("description"),
                    "permissions": info["permissions"], "permissions_text": {"en": info["permissions_text"]},
                    "files": sorted(files, key=lambda f: f["path"])})
json.dump({"schema": 1, "commit": None, "plugins": plugins}, open("index.json", "w"), indent=2)
PY
```

Custom sources get roomier limits than the Plugin Center, since you chose to trust them: up to 64 files, 4 MiB per file and 16 MiB per plugin. Besides UTF-8 text and PNG they may include JPEG (`.jpg` / `.jpeg`), WebP and GIF images, for illustrations or animations. Every image is checked by its file header, not just its extension, so a renamed binary is refused. Hidden files and symlinks aren't allowed, and `helmp plugins validate` must pass. (16 MiB keeps a daily update to about the size of a photo; `helmp plugins submit` and the Plugin Center keep their own, stricter rules.)

### Authentication

If the source needs a token, set the header name (default `Authorization`) and the token value, which Helmp sends verbatim (for example `Bearer <token>`, or a bare key with `--header X-Api-Key`). For a private GitHub repository, serve the raw files through any HTTPS host that accepts a header token; plain `raw.githubusercontent.com` URLs work with `Authorization: token <PAT>`.

- The token is stored in the macOS Keychain (service `dev.helmp.plugin-sources`), never in `config.json`, logs, error messages or crash reports.
- It's only sent to the source's own host. Redirects are followed only within that host; a redirect to another host stops the download instead of forwarding the token.
- HTTP 401 / 403 and unreachable sources are reported per source (which source, what went wrong). One failing source never hides the others.

### Browsing, installing and updating

The plugin list in Settings (and `helmp plugins available`) merges all sources and shows where each plugin comes from. If the same id appears in several sources, the Plugin Center wins, then the first source in the list, and Helmp says so next to the plugin. To install from a specific source, use its row in Settings or `helmp plugins install <id> --source <name>`.

A plugin is only ever updated from the source it was installed from, so another source can't replace it by publishing the same id. If you remove that source, the plugin stays installed but gets no more updates.

Everything else is the same as for the Plugin Center: SHA-256 checks, atomic install, plugins start turned off, and permission changes need your approval again.

**Only add sources you trust.** Custom sources aren't reviewed by the Plugin Center maintainers. Treat a source's token like a password: give each person their own read-only token if your host supports it, and rotate it if it leaks.

## Settings

| Setting | What it does |
|---|---|
| `plugins_registry_url` in `config.json` | Registry base URL (the folder that holds `index.json`). Default: `https://cdn.jsdelivr.net/gh/PeterChen1997/helmp-plugins@main/` |
| `HELMP_REGISTRY_URL` environment variable | Overrides the above, for example to point at a local test server |
| `general.auto_update_plugins` in `config.json` | Check plugins installed from the Plugin Center or a source once a day and update them. Default: `true`. The **Automatically update plugins** switch in Settings |
| `plugin_sources` in `config.json` | Custom plugin sources (`name`, `url`, optional `auth_header`). Tokens live in the Keychain, not here. Manage them in Settings or with `helmp plugins source` |
| `HELMP_REGISTRY_REPO` environment variable | The GitHub repository `helmp plugins submit` opens pull requests against. Default: `PeterChen1997/helmp-plugins` |

## Publish your plugin

See [plugins.md › Publish to the Plugin Center](plugins.md#publish-to-the-plugin-center). In short:

1. Add `"license"` (for example `"MIT"`) and a `README.md` to your plugin folder.
2. Run `helmp plugins validate <folder>`.
3. Run `helmp plugins submit <folder>`. It prints what it's going to do and stops. Run it again with `--yes` to fork the registry with your `gh` account, push a branch named `plugin/<id>-<version>`, and open a pull request. It prints the pull request's URL.

Coding agents such as Claude Code or Codex can do this for you through Helmp's MCP server (`helmp mcp`). They must ask you before they submit anything. A maintainer then reviews the pull request. Once it's merged, the plugin appears in the Plugin Center.
