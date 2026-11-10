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

Helmp doesn't check for updates in the background. When you open **Browse Plugin Center** in Settings (or run `helmp plugins available`), plugins installed from the Plugin Center that have a newer version show **Update available**, and the button changes to **Update to x.y.z**. `helmp plugins install <id>` also updates.

An update downloads and verifies the new version, then replaces the folder. If the new version needs the same permissions you already allowed, it stays on. If its permissions changed (a new host, or it now runs scripts), it's paused and marked **needs permission review** until you approve it again.

## Security model

- **Human review.** Every plugin reaches the Plugin Center through a pull request that a maintainer reviews: permissions match what the plugin does, hosts are expected, scripts aren't obfuscated and don't fetch and run remote code, and the plugin works as its README says. A CI check enforces the format and file rules and flags scripts for manual review. It never runs plugin code.
- **Verified downloads.** Helmp downloads only the files listed in the index and checks each file's size and SHA-256 before writing anything. If any file doesn't match (a tampered file or a CDN glitch), the whole install is refused and nothing is written. Paths that would leave the plugin folder (`../`, absolute paths, hidden files) make Helmp reject the whole index. On jsDelivr, files are fetched from the exact registry commit the index was built from, so a stale CDN cache can't mix versions.
- **Atomic install.** Files are written to a hidden temporary folder and validated with the same rules as `helmp plugins validate`. The manifest's id and version must match the index. Only then is the folder renamed into place. A failed update leaves the previous version untouched.
- **Consent.** Installed plugins are off by default. You approve the derived permissions before a plugin runs, and approve again whenever they change. Secrets stay in the Keychain.
- **Limits.** Up to 64 files per plugin, 512 KiB per file and 2 MiB in total. Only UTF-8 text files and PNG icons are allowed: no symlinks and no other binaries.

What's left to trust: the maintainer's review, GitHub, jsDelivr's TLS, and the scripts a plugin runs with your user's permissions once you turn it on. Only turn on plugins whose permissions make sense to you.

## Settings

| Setting | What it does |
|---|---|
| `plugins_registry_url` in `config.json` | Registry base URL (the folder that holds `index.json`). Default: `https://cdn.jsdelivr.net/gh/PeterChen1997/helmp-plugins@main/` |
| `HELMP_REGISTRY_URL` environment variable | Overrides the above, for example to point at a local test server |
| `HELMP_REGISTRY_REPO` environment variable | The GitHub repository `helmp plugins submit` opens pull requests against. Default: `PeterChen1997/helmp-plugins` |

## Publish your plugin

See [plugins.md › Publish to the Plugin Center](plugins.md#publish-to-the-plugin-center). In short:

1. Add `"license"` (for example `"MIT"`) and a `README.md` to your plugin folder.
2. Run `helmp plugins validate <folder>`.
3. Run `helmp plugins submit <folder>`. It prints what it's going to do and stops. Run it again with `--yes` to fork the registry with your `gh` account, push a branch named `plugin/<id>-<version>`, and open a pull request. It prints the pull request's URL.

Coding agents such as Claude Code or Codex can do this for you through Helmp's MCP server (`helmp mcp`). They must ask you before they submit anything. A maintainer then reviews the pull request. Once it's merged, the plugin appears in the Plugin Center.
