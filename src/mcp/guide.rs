//! 写给 agent 的插件开发指南：工作流（本文件）+ 插件文档（docs/plugins*.md，编译进来）+ 自带插件清单作示例。
//! 同一份内容以工具（`helmp_plugin_guide`）、资源（`helmp://docs/plugins`）和提示词（`create_helmp_plugin`）三种形式提供。

use crate::providers::plugins::registry;

const PLUGINS_EN: &str = include_str!("../../docs/plugins.md");
const PLUGINS_ZH: &str = include_str!("../../docs/plugins.zh-CN.md");

pub const RESOURCE_EN: &str = "helmp://docs/plugins";
pub const RESOURCE_ZH: &str = "helmp://docs/plugins.zh-CN";

/// agent 做插件的流程。英文写给模型看（中文文档另有一份完整的人类版）。
const WORKFLOW: &str = r#"# Building Helmp plugins as an agent

Helmp is a macOS launcher (like Raycast/Alfred). A Helmp plugin is a folder with a declarative
`plugin.json` (no code needed for "keyword → URL" or "form → HTTP request"); scripts are available when
logic is needed. You build plugins with the `helmp_plugin_*` tools of this MCP server.

## Workflow (follow it in order)

1. **Design.** Restate the user's feature as one or more commands: keyword, input (argument / form / list),
   action (`open`, `copy`, `paste`, `http`, `script`). Prefer declarative actions; use a script only for logic.
   Secrets (tokens, webhook URLs) are `"type": "secret"` preferences: Helmp keeps them in the Keychain.
   Plugin ids are reverse-DNS, lowercase (`com.<user>.<name>`); never reuse `dev.helmp.*`.
2. **Scaffold.** `helmp_plugin_scaffold` with an id, a name and the closest `kind`
   (`quicklink`, `http`, `script`, `list`, or `starter` for all three). It returns the folder and files.
3. **Write files.** Edit `plugin.json` (and scripts under `bin/`, make them executable) with your own file tools.
   Unknown fields are errors; text fields can be `"…"` or `{"en": "…", "zh-Hans": "…"}`.
4. **Validate.** `helmp_plugin_validate` until it reports `valid: true`. Read the derived permissions: they are
   what the user will be asked to approve. Keep them minimal (one host, no `open any link` unless needed).
5. **Test (dry run).** `helmp_plugin_test` runs a command headless in an isolated sandbox: opens, copies,
   pastes and requests to real hosts are only reported as `[dry-run] …` lines; requests to
   `127.0.0.1`/`localhost` are really sent, so you can point a URL preference at a local mock server
   (pass it through `prefs`, or `secrets` for secret preferences). Scripts really run. Fix and repeat.
6. **Install.** `helmp_plugin_install` copies the folder into Helmp. The plugin stays **disabled** until the
   user reviews its permissions and turns it on in Settings › Plugins: tell the user exactly that and wait.
   Never claim it is enabled, and never try to enable it yourself.
7. **Show the user.** Summarize what the plugin does, its keyword(s), its permissions and the dry-run results.
   Once they enabled it, `helmp_plugin_test` with `dry_run: false` (or `helmp_run`) asks the user to approve
   a real run in the Helmp panel.
8. **Ask before publishing.** Ask the user explicitly whether they want to upload the plugin to the public
   Helmp plugin center. Only after a clear yes call `helmp_plugin_submit` with `user_confirmed: true`.
   If they say no, you are done: the plugin works locally.

## Operating Helmp

`helmp_search` finds launcher results (apps, calculator, conversions, time zones, developer tools, plugin
commands…). `helmp_run` runs one: pure computations return their text directly; anything with a side effect
(opening, copying, pasting, killing processes, window moves, toggles, plugin HTTP requests, scripts, system
commands) is shown to the user in the Helmp panel and runs only if they press Return. Commands that take
input (plugin commands with an argument, form or list; MCP tools with arguments) only show up as an entry in
the main search, with a `scope`: the user presses Tab on them and types inside that scope. Do the same: pass
that `scope` to `helmp_search` / `helmp_run` with the input as `query` (`flomo` → scope
`plugins:dev.helmp.flomo/memo`, query = the memo). Clipboard history and agent session transcripts are hidden
from you unless the user opted in.

## Example: "send the selected text to a Feishu (Lark) bot"

- id `com.you.feishu-bot`, kind `http`; a secret preference `webhook` with
  `"allowedHosts": ["open.feishu.cn"]`; a command `send` with keyword `fs` and a textarea form field
  `content` (default `{{query}}`); action `http` POST `{{pref.webhook}}` with
  `"json": {"msg_type": "text", "content": {"text": "{{form.content}}"}}`,
  `"success": {"jsonPath": "code", "equals": 0, "optional": true}`, `"errorPath": "msg"`.
- Test it against a mock: `helmp_plugin_test` with `secrets: {"webhook": "http://127.0.0.1:<port>/hook"}`.
"#;

/// 指南全文。`zh` 为真时附中文文档，否则英文文档。
pub fn text(zh: bool) -> String {
    let docs = if zh { PLUGINS_ZH } else { PLUGINS_EN };
    let mut out = String::from(WORKFLOW);
    out.push_str("\n---\n\n");
    out.push_str(docs);
    out.push_str("\n---\n\n# Bundled plugins (complete, working examples)\n");
    for (id, manifest) in registry::BUNDLED {
        out.push_str(&format!("\n## {id}\n\n```json\n{}\n```\n", manifest.trim()));
    }
    out
}

/// `create_helmp_plugin` 提示词：把用户的需求和工作流交给 agent。
pub fn prompt(request: &str) -> String {
    format!(
        "The user wants a new Helmp feature:\n\n> {}\n\nBuild it as a Helmp plugin with the tools of the \
         `helmp` MCP server. First call `helmp_plugin_guide` (or read the resource `{RESOURCE_EN}`) for the \
         full manifest reference, then follow this workflow strictly: design → `helmp_plugin_scaffold` → \
         write the files → `helmp_plugin_validate` (until valid) → `helmp_plugin_test` (dry run, against a \
         local mock server for HTTP) → `helmp_plugin_install` → tell the user to review the permissions and \
         enable it in Helmp › Settings › Plugins, and show them what it does → ask the user explicitly whether \
         they want to upload it to the public Helmp plugin center; call `helmp_plugin_submit` with \
         `user_confirmed: true` only after they say yes.",
        request.trim().replace('\n', "\n> ")
    )
}
