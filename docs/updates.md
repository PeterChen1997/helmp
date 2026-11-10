# Updates

English · [简体中文](updates.zh-CN.md)

Helmp checks a small JSON file for new versions and installs one only after you click "Download and Install". The design follows Sparkle: a version manifest, a signature check, and your consent before anything is replaced.

## For users

- **Automatic checks.** With Settings › General › "Automatically check for updates" on, Helmp checks 20 seconds after launch and then once a day. A failed check (offline, say) is retried an hour later and never pops anything up. Turn the switch off and Helmp never checks on its own.
- **Manual checks.** The menu bar's "Check for Updates…", the panel command "Check for Helmp Updates", and the button in Settings › General all open Settings › About and check right away. The result, or the reason it failed, is shown there.
- **When an update is found,** the menu bar item reads "Update to x.y.z…" and Settings › About shows the release notes and a "Download and Install" button. Nothing is downloaded before you click it.
- **Installing.** Helmp downloads the zip, checks its size and SHA-256 against the manifest, unpacks it next to the current app, and checks that the new app is the next version of Helmp (bundle id `dev.helmp.launcher`, the manifest's version, never older), that its signature is valid (`codesign --verify --deep --strict`), that it is signed by the same Developer ID team as the running Helmp, and that Gatekeeper accepts it (`spctl --assess`). Only then does Helmp quit; a small helper waits for it to exit, swaps the apps (keeping the old one until the new one is in place, and putting it back if anything fails), and reopens Helmp.
- **When Helmp can't replace itself** (it isn't running from a Helmp.app in `/Applications` or `~/Applications`, or that folder isn't writable), the button reads "Download dmg": Helmp downloads and verifies the dmg, saves it to Downloads and opens it so you can drag Helmp to Applications. A Helmp without a Developer ID signature (a self-built copy) refuses to install in place, because it can't tell whether the update comes from the same developer; download the release manually instead.
- **What is sent.** A plain `GET` of the manifest (and of the installer when you install), with the User-Agent `Helmp/<version> (macOS updater)`. No identifier, no usage data.

## Feeds

Helmp tries these in order and uses the first that answers with a valid manifest:

1. `https://blog.peterchen97.cn/helmp/releases/latest.json` (Qiniu CDN, reachable from mainland China)
2. `https://github.com/PeterChen1997/helmp/releases/latest/download/latest.json` (the `latest.json` attached to the latest GitHub release; a direct download, so the anonymous API rate limit doesn't apply)

If an earlier feed fails and a later one works, the failure is printed to stderr; if all fail, each address is listed with its reason. A GitHub Releases API response (`api.github.com/repos/…/releases/latest`) is also understood: Helmp fetches the `latest.json` attached to that release.

Override the list with `HELMP_UPDATE_FEED` (takes precedence) or `updates.feed_url` in `config.json`; separate several addresses with commas.

## Manifest

```json
{
  "version": "0.4.0",
  "published_at": "2026-11-20T08:00:00Z",
  "notes": {"en": "…", "zh-Hans": "…"},
  "minimum_macos": "15.0",
  "assets": [
    {"name": "Helmp-0.4.0.zip", "kind": "zip", "size": 12345678, "sha256": "<64 hex>",
     "urls": ["https://blog.peterchen97.cn/helmp/releases/0.4.0/Helmp-0.4.0.zip",
              "https://github.com/PeterChen1997/helmp/releases/download/v0.4.0/Helmp-0.4.0.zip"]},
    {"name": "Helmp-0.4.0.dmg", "kind": "dmg", "…": "…"}
  ]
}
```

- `version` is compared as semver: numeric fields by value, a pre-release sorts before its release, build metadata is ignored.
- All problems are reported together: unsafe asset names, a `sha256` that isn't 64 hex digits, an implausible size (over 512 MB), non-http(s) URLs, no zip.
- Download URLs are tried in order. A file that downloads but doesn't match `size` and `sha256` is refused outright rather than retried from another URL: the manifest and the file disagree.
- An unknown `kind` is kept but never chosen, so a future format doesn't break older versions.
- If `minimum_macos` is newer than this Mac, Settings › About says so and offers no install button.

## Command line

```sh
helmp update check            # prints current / latest / status (update-available | up-to-date) / feed
helmp update install          # prints the plan and exits with an error: nothing is installed
helmp update install --yes    # downloads, verifies and installs
```

`install` without `--yes` follows the same rule as `helmp plugins submit`: an agent acting for you must ask first. While the Helmp app is running, `install` refuses to replace it from the command line; use the menu bar instead.

With `HELMP_DRY_RUN=1` Helmp only contacts local addresses (`127.0.0.1`, `localhost`, `[::1]`), only downloads and verifies, and never unpacks or replaces anything. `HELMP_UPDATE_TARGET` (the app to replace) is honoured only in this mode. The end-to-end tests in `tests/e2e_updates.rs` run this way against a local mock feed.

## Publishing a release

1. Add the version's section to `CHANGELOG.md` (English, then `### 简体中文`). Its two halves become the release notes.
2. `script/release.sh`: signs, notarizes and staples `target/dist/Helmp-<version>.zip` and `.dmg`.
3. `script/publish-release.sh --dry-run`: writes `target/dist/latest.json` and `<version>.json` and lists what would be uploaded.
4. `script/publish-release.sh --publish`:
   - uploads to Qiniu with `qshell`: the installers under `helmp/releases/<version>/`, then the `<version>.json` snapshot, then `helmp/releases/latest.json` last, so a client never sees a manifest that points at missing files. Every key must start with `helmp/` (the bucket is also the blog's root);
   - refreshes `latest.json` on the CDN;
   - creates the GitHub release `v<version>` if it doesn't exist and attaches the zip, the dmg and, last, `latest.json`.

Without a flag, the script only writes the manifest.
