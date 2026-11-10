//! 插件开发工具：指南、脚手架、校验、安装、试运行、列表、发布。
//!
//! - 安装只复制文件，从不替用户启用或授权：新插件保持停用，等用户在设置 › 插件里看过权限后打开。
//! - 试运行（演练）在临时的隔离数据目录里起一个 `helmp run` 子进程：插件在那里是启用的，
//!   但演练模式下打开、复制、粘贴、发往真实主机的请求只回报；发往本机的请求真的发（对着本地假服务测）。
//! - 发布只在 agent 声明用户明确同意（`user_confirmed: true`）后才调用 `helmp plugins submit`。

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::providers::plugins::manifest::{ActionSpec, Command as PluginCommand};
use crate::providers::plugins::registry::{self, Entry, Plugin, Source, State};
use crate::providers::plugins::scaffold;

use super::server::{Server, ToolOutput};
use super::{guide, ipc, operate};

fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    arg(args, key).with_context(|| format!("{key} is required"))
}

fn absolute(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        bail!("{} must be an absolute path", path.display());
    }
    Ok(path)
}

fn state_name(state: State) -> &'static str {
    match state {
        State::Enabled => "enabled",
        State::Disabled => "disabled",
        State::NeedsConsent => "needs-consent",
        State::Broken => "error",
    }
}

/// 命令的触发词：第一个关键词，没有就用标题（与插件 provider 的匹配规则一致）。
fn trigger(command: &PluginCommand) -> String {
    command
        .keywords
        .first()
        .cloned()
        .unwrap_or_else(|| command.title.get().to_string())
}

fn command_kind(command: &PluginCommand) -> &'static str {
    match (&command.list, &command.action) {
        (Some(_), _) => "list",
        (None, Some(ActionSpec::Open(_))) => "open",
        (None, Some(ActionSpec::Http(_))) => "http",
        (None, Some(ActionSpec::Script(_))) => "script",
        (None, Some(ActionSpec::Copy(_))) => "copy",
        (None, Some(ActionSpec::Paste(_))) => "paste",
        (None, None) => "none",
    }
}

fn describe_plugin(plugin: &Plugin) -> Value {
    let commands: Vec<Value> = plugin
        .manifest
        .commands
        .iter()
        .map(|command| {
            json!({
                "id": command.id,
                "title": command.title.get(),
                "keywords": command.keywords,
                "kind": command_kind(command),
                "result_id": format!("plugins:{}/{}", plugin.id(), command.id),
                "form": command.form.iter().map(|f| f.id.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "id": plugin.id(),
        "name": plugin.manifest.name.get(),
        "version": plugin.manifest.version,
        "commands": commands,
        "permissions": plugin.permissions.describe(),
        "permission_keys": plugin.permissions.keys(),
        "preferences": plugin.manifest.preferences.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
    })
}

// ---------------------------------------------------------------- guide

pub fn guide(args: &Value) -> ToolOutput {
    let zh = arg(args, "language") == Some("zh");
    let text = guide::text(zh);
    ToolOutput::ok(
        text,
        json!({"resource": if zh { guide::RESOURCE_ZH } else { guide::RESOURCE_EN }}),
    )
}

// ---------------------------------------------------------------- scaffold

const RUN_SCRIPT: &str = r#"#!/bin/sh
# $1 is the text typed after the keyword. Whatever this prints is shown in Helmp.
echo "You typed: ${1:-nothing}"
"#;

const LIST_SCRIPT: &str = r#"#!/bin/sh
# Print Alfred Script Filter JSON. $1 is the text typed after the keyword.
q=$(printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g')
cat <<EOF
{"items": [
  {"uid": "echo", "title": "Echo: $q", "subtitle": "Return copies it", "arg": "$q"}
]}
EOF
"#;

fn keyword_for(id: &str) -> String {
    let last = id.rsplit('.').next().unwrap_or(id);
    let keyword: String = last
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if keyword.is_empty() {
        "myplugin".into()
    } else {
        keyword
    }
}

/// 某种模板的清单与脚本（相对路径, 内容）。
fn template(
    kind: &str,
    id: &str,
    name: &str,
) -> Result<(Value, Vec<(&'static str, &'static str)>)> {
    let keyword = keyword_for(id);
    let base = |commands: Value, preferences: Value| {
        json!({
            "$schema": "https://raw.githubusercontent.com/PeterChen1997/helmp/HEAD/docs/plugin-schema.json",
            "schemaVersion": 1,
            "id": id,
            "name": name,
            "description": format!("{name} for Helmp"),
            "version": "0.1.0",
            "author": "You",
            "icon": "puzzle",
            "tint": "indigo",
            "preferences": preferences,
            "commands": commands,
        })
    };
    Ok(match kind {
        "quicklink" => (
            base(
                json!([{
                    "id": "search", "title": name, "keywords": [keyword],
                    "argument": {"placeholder": "Search terms", "required": true},
                    "action": {"type": "open", "url": "https://example.com/search?q={{query|url}}"}
                }]),
                json!([]),
            ),
            Vec::new(),
        ),
        "http" => (
            base(
                json!([{
                    "id": "send", "title": name, "keywords": [keyword],
                    "form": [{"id": "content", "type": "textarea", "label": "Content", "required": true, "default": "{{query}}"}],
                    "action": {
                        "type": "http", "method": "POST", "url": "{{pref.webhook}}",
                        "json": {"text": "{{form.content}}"},
                        "timeoutMs": 10000,
                        "message": "Sent"
                    }
                }]),
                json!([{
                    "id": "webhook", "type": "secret", "title": "Webhook URL", "required": true,
                    "placeholder": "https://example.com/hooks/…", "allowedHosts": ["example.com"]
                }]),
            ),
            Vec::new(),
        ),
        "script" => (
            base(
                json!([{
                    "id": "run", "title": name, "keywords": [keyword],
                    "argument": {"placeholder": "Input"},
                    "action": {"type": "script", "path": "bin/run.sh", "args": ["{{query}}"], "output": "message"}
                }]),
                json!([]),
            ),
            vec![("bin/run.sh", RUN_SCRIPT)],
        ),
        "list" => (
            base(
                json!([{
                    "id": "list", "title": name, "keywords": [keyword],
                    "list": {
                        "script": {"path": "bin/list.sh", "args": ["{{query}}"]},
                        "action": {"type": "copy", "text": "{{item.arg}}"}
                    }
                }]),
                json!([]),
            ),
            vec![("bin/list.sh", LIST_SCRIPT)],
        ),
        other => bail!("Unknown kind {other:?} (use starter, quicklink, http, script or list)"),
    })
}

fn list_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path.display().to_string());
            }
        }
    }
    walk(dir, &mut out);
    out
}

pub fn scaffold(args: &Value) -> Result<ToolOutput> {
    let id = required(args, "id")?;
    let name = required(args, "name")?;
    let kind = arg(args, "kind").unwrap_or("starter");
    if !crate::providers::plugins::manifest::valid_id(id) {
        bail!(
            "Invalid id {id:?}: use reverse-DNS, lowercase letters, digits and '-', e.g. com.yourname.my-plugin"
        );
    }
    if registry::BUNDLED.iter().any(|(bundled, _)| *bundled == id) {
        bail!("{id} is a bundled plugin id; pick another one");
    }
    let dir = match arg(args, "dir") {
        Some(dir) => absolute(dir)?,
        None => crate::paths::data_dir()?.join("plugins-dev").join(id),
    };
    if dir
        .read_dir()
        .is_ok_and(|mut entries| entries.next().is_some())
    {
        bail!(
            "{} already exists and isn't empty; pick another dir or edit the existing plugin",
            dir.display()
        );
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let manifest_path = dir.join("plugin.json");
    if kind == "starter" {
        scaffold::write_starter(&dir, id)?;
        let mut manifest: Value = serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)?;
        manifest["name"] = json!(name);
        std::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest)? + "\n",
        )?;
    } else {
        let (manifest, scripts) = template(kind, id, name)?;
        for (relative, body) in scripts {
            let path = dir.join(relative);
            std::fs::create_dir_all(path.parent().context("script parent")?)?;
            std::fs::write(&path, body)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest)? + "\n",
        )?;
    }
    let entry = registry::load_dir(&dir, Source::User);
    let errors = entry.loaded.as_ref().err().cloned().unwrap_or_default();
    let files = list_files(&dir);
    let text = format!(
        "Created a {kind} plugin {id} in {}\nFiles:\n  {}\nNext: edit plugin.json (and scripts), then helmp_plugin_validate with dir={}.{}",
        dir.display(),
        files.join("\n  "),
        dir.display(),
        if errors.is_empty() {
            String::new()
        } else {
            format!("\nTemplate problems (please report): {}", errors.join("; "))
        }
    );
    Ok(ToolOutput::ok(
        text,
        json!({"id": id, "dir": dir, "manifest": manifest_path, "files": files, "valid": errors.is_empty()}),
    ))
}

// ---------------------------------------------------------------- validate

pub fn validate(args: &Value) -> Result<ToolOutput> {
    let dir = absolute(required(args, "dir")?)?;
    let entry = registry::load_dir(&dir, Source::User);
    Ok(match &entry.loaded {
        Ok(plugin) => {
            if registry::BUNDLED
                .iter()
                .any(|(bundled, _)| *bundled == plugin.id())
            {
                return Ok(ToolOutput::error(
                    format!(
                        "INVALID: {} is a bundled plugin id; user plugins can't reuse it",
                        plugin.id()
                    ),
                    json!({"valid": false, "errors": ["id: reuses a bundled plugin id"]}),
                ));
            }
            let info = describe_plugin(plugin);
            let permissions = plugin.permissions.describe();
            let text = format!(
                "VALID: {} {} ({} command(s))\nPermissions the user will approve:\n{}",
                plugin.id(),
                plugin.manifest.version,
                plugin.manifest.commands.len(),
                if permissions.is_empty() {
                    "  - none".to_string()
                } else {
                    permissions
                        .iter()
                        .map(|p| format!("  - {p}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            );
            let mut data = info;
            data["valid"] = json!(true);
            ToolOutput::ok(text, data)
        }
        Err(errors) => ToolOutput::error(
            format!(
                "INVALID: {} problem(s)\n{}",
                errors.len(),
                errors
                    .iter()
                    .map(|e| format!("  - {e}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            json!({"valid": false, "errors": errors}),
        ),
    })
}

// ---------------------------------------------------------------- install

/// 复制插件目录。符号链接一律拒绝（脚本路径不能借它跳出插件目录），版本库与系统杂项跳过。
fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("create {}", to.display()))?;
    for entry in std::fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if matches!(name_text.as_ref(), ".git" | ".DS_Store" | "node_modules") {
            continue;
        }
        let kind = entry.file_type()?;
        let (src, dst) = (entry.path(), to.join(&name));
        if kind.is_symlink() {
            bail!(
                "{} is a symbolic link; plugins can't contain links",
                src.display()
            );
        } else if kind.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            // fs::copy 连权限位（可执行）一起复制。
            std::fs::copy(&src, &dst).with_context(|| format!("copy {}", src.display()))?;
        }
    }
    Ok(())
}

fn find_entry(id: &str) -> Option<Entry> {
    registry::load_all()
        .into_iter()
        .find(|entry| entry.id == id)
}

pub fn install(_server: &Server, args: &Value) -> Result<ToolOutput> {
    let src = absolute(required(args, "dir")?)?;
    let entry = registry::load_dir(&src, Source::User);
    let plugin = match &entry.loaded {
        Ok(plugin) => plugin.clone(),
        Err(errors) => {
            return Ok(ToolOutput::error(
                format!(
                    "Not installed: the plugin is invalid.\n{}",
                    errors
                        .iter()
                        .map(|e| format!("  - {e}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
                json!({"installed": false, "errors": errors}),
            ));
        }
    };
    let id = plugin.id().to_string();
    let root = registry::user_dir()?;
    let dest = root.join(&id);
    if let Some(existing) = find_entry(&id) {
        match existing.source {
            Source::Bundled => bail!("{id} is a bundled plugin id; pick another id"),
            Source::User if existing.dir.as_deref() != Some(dest.as_path()) => bail!(
                "A plugin with id {id} is already installed in {}; remove that folder or install from it",
                existing
                    .dir
                    .map(|d| d.display().to_string())
                    .unwrap_or_default()
            ),
            Source::User => {}
        }
    }
    let same = src
        .canonicalize()
        .ok()
        .zip(dest.canonicalize().ok())
        .is_some_and(|(a, b)| a == b);
    if !same {
        // 先复制到隐藏的临时目录（轮询会跳过点开头的文件夹），再整体换上，免得热重载读到半截插件。
        let staging = root.join(format!(".{id}.installing"));
        let old = root.join(format!(".{id}.old"));
        let _ = std::fs::remove_dir_all(&staging);
        let _ = std::fs::remove_dir_all(&old);
        copy_dir(&src, &staging)?;
        if dest.exists() {
            std::fs::rename(&dest, &old)
                .with_context(|| format!("move {} aside", dest.display()))?;
        }
        std::fs::rename(&staging, &dest)
            .with_context(|| format!("install into {}", dest.display()))?;
        let _ = std::fs::remove_dir_all(&old);
    }

    let installed = registry::load_dir(&dest, Source::User);
    let state = installed.state(&super::fresh_config());
    // Helmp 在运行就把设置 › 插件翻到这个插件，让用户看权限、打开它。测试（演练模式）不碰界面。
    let settings = if std::env::var("HELMP_DRY_RUN").as_deref() == Ok("1") {
        "skipped (dry-run)"
    } else {
        match ipc::request(
            &json!({"v": 1, "type": "open_plugin", "plugin": id}),
            Duration::from_secs(5),
        ) {
            Ok(reply) if reply.get("ok") == Some(&Value::Bool(true)) => "opened",
            Ok(_) | Err(ipc::ClientError::Other(_)) => "failed",
            Err(ipc::ClientError::NotRunning) => "helmp-not-running",
        }
    };
    let next = match state {
        State::Enabled => "The user had already enabled this plugin with the same permissions, so the new version is active now.".to_string(),
        State::NeedsConsent => "Its permissions changed since the user enabled it, so it is paused until the user reviews and confirms them in Helmp › Settings › Plugins. Ask the user to do that.".to_string(),
        _ => format!(
            "It is DISABLED until the user reviews its permissions and turns it on in Helmp › Settings › Plugins{}. Ask the user to do that now, and don't claim it is enabled.",
            match settings {
                "opened" => " (opened for them)",
                "helmp-not-running" => " (Helmp isn't running; ask them to start it first)",
                _ => "",
            }
        ),
    };
    let permissions = plugin.permissions.describe();
    let text = format!(
        "Installed {id} {} into {}.\nState: {}.\nPermissions: {}\n{next}",
        plugin.manifest.version,
        dest.display(),
        state_name(state),
        if permissions.is_empty() {
            "none".to_string()
        } else {
            permissions.join("; ")
        },
    );
    let mut data = describe_plugin(&plugin);
    data["installed"] = json!(true);
    data["dir"] = json!(dest);
    data["state"] = json!(state_name(state));
    data["settings"] = json!(settings);
    Ok(ToolOutput::ok(text, data))
}

// ---------------------------------------------------------------- list

pub fn list() -> Result<ToolOutput> {
    let config = super::fresh_config();
    let mut text = String::new();
    let plugins: Vec<Value> = registry::load_all()
        .iter()
        .map(|entry| {
            let state = state_name(entry.state(&config));
            let source = match entry.source {
                Source::Bundled => "bundled",
                Source::User => "user",
            };
            let mut value = match &entry.loaded {
                Ok(plugin) => describe_plugin(plugin),
                Err(errors) => json!({"id": entry.id, "errors": errors}),
            };
            value["state"] = json!(state);
            value["source"] = json!(source);
            value["dir"] = json!(entry.dir);
            text.push_str(&format!(
                "{state}\t{}\t{source}\t{}\n",
                entry.id,
                value["name"].as_str().unwrap_or("-")
            ));
            if let Err(errors) = &entry.loaded {
                for error in errors {
                    text.push_str(&format!("  - {error}\n"));
                }
            }
            value
        })
        .collect();
    Ok(ToolOutput::ok(text, json!({"plugins": plugins})))
}

// ---------------------------------------------------------------- test

static NEXT_SANDBOX: AtomicUsize = AtomicUsize::new(0);

/// 试运行用的临时数据目录，结束时删掉。
struct Sandbox(PathBuf);

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn test(server: &Server, args: &Value) -> Result<ToolOutput> {
    let target = required(args, "plugin")?;
    let command_id = required(args, "command")?;
    let input = args
        .get("input")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let form = operate::string_map(args.get("form"));
    let item_uid = arg(args, "item");
    let dry_run = args.get("dry_run").and_then(Value::as_bool).unwrap_or(true);

    // 插件目录，或已安装插件的 id。
    let entry = if target.starts_with('/') {
        registry::load_dir(Path::new(target), Source::User)
    } else {
        find_entry(target).with_context(|| {
            format!("No installed plugin {target}; pass the plugin folder instead")
        })?
    };
    let plugin = match &entry.loaded {
        Ok(plugin) => plugin.clone(),
        Err(errors) => bail!(
            "The plugin is invalid; fix it first:\n{}",
            errors.join("\n")
        ),
    };
    let id = plugin.id().to_string();
    let command = plugin.manifest.command(command_id).with_context(|| {
        let ids: Vec<_> = plugin
            .manifest
            .commands
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        format!(
            "Plugin {id} has no command {command_id:?} (commands: {})",
            ids.join(", ")
        )
    })?;
    let query = if input.is_empty() {
        trigger(command)
    } else {
        format!("{} {input}", trigger(command))
    };
    let base_id = format!("plugins:{id}/{command_id}");
    let result_id = match item_uid {
        Some(uid) => format!("{base_id}#{uid}"),
        None => base_id.clone(),
    };

    if !dry_run && std::env::var("HELMP_DRY_RUN").as_deref() != Ok("1") {
        // 真跑：交给用户的 Helmp，经面板确认。插件必须已安装并启用。
        let request = json!({
            "v": 1, "type": "run", "agent": server.agent_name(),
            "query": query, "id": result_id, "action": null, "form": form,
        });
        let reply = match ipc::request(&request, ipc::CONFIRM_TIMEOUT + Duration::from_secs(160)) {
            Ok(reply) => reply,
            Err(ipc::ClientError::NotRunning) => {
                bail!(
                    "Helmp isn't running, so the user can't approve a real run. Ask them to start Helmp."
                )
            }
            Err(ipc::ClientError::Other(err)) => return Err(err),
        };
        return operate::confirmation_output(&result_id, "Run", &reply);
    }

    // 演练：隔离的数据目录里只启用这一个插件。
    let root = std::env::temp_dir().join(format!(
        "helmp-mcp-test-{}-{}",
        std::process::id(),
        NEXT_SANDBOX.fetch_add(1, Ordering::SeqCst)
    ));
    let sandbox = Sandbox(root.clone());
    let data = sandbox.0.join("data");
    std::fs::create_dir_all(&data)?;
    let bundled = entry.source == Source::Bundled;
    if !bundled {
        let dir = entry.dir.as_deref().context("plugin folder")?;
        copy_dir(dir, &data.join("plugins").join(&id))?;
    }
    let config = super::fresh_config();
    let mut prefs: serde_json::Map<String, Value> = config
        .plugins
        .get(&id)
        .map(|settings| {
            settings
                .prefs
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect()
        })
        .unwrap_or_default();
    for (key, value) in operate::string_map(args.get("prefs")) {
        prefs.insert(key, json!(value));
    }
    let extensions: serde_json::Map<String, Value> = server
        .engine
        .provider_ids()
        .into_iter()
        .filter(|pid| *pid != crate::providers::plugins::PROVIDER_ID)
        .map(|pid| (pid.to_string(), json!({"enabled": false})))
        .collect();
    let sandbox_config = json!({
        "general": {"language": config.general.language, "launch_at_login": false, "auto_check_updates": false},
        "clipboard": {"enabled": false, "record_images": false},
        "onboarding_done": true,
        "extensions": extensions,
        "plugins": {id.clone(): {"enabled": true, "consent": plugin.permissions.keys(), "prefs": prefs}},
    });
    std::fs::write(
        data.join("config.json"),
        serde_json::to_string_pretty(&sandbox_config)?,
    )?;
    let secrets: serde_json::Map<String, Value> = operate::string_map(args.get("secrets"))
        .into_iter()
        .map(|(key, value)| (format!("{id}/{key}"), json!(value)))
        .collect();

    let exe = std::env::current_exe().context("locate the helmp executable")?;
    let mut child = Command::new(exe);
    child
        .env("HELMP_DATA_DIR", &data)
        .env("HELMP_DRY_RUN", "1")
        .env("HELMP_TEST_SECRETS", Value::Object(secrets).to_string())
        .env("HELMP_SCRIPT_DIRS", sandbox.0.join("scripts"))
        .env_remove("HELMP_TRACE");
    let list_mode = command.list.is_some() && item_uid.is_none();
    if list_mode {
        child.args(["query", &query]);
    } else {
        child.args(["run", &result_id, &query]);
        for (key, value) in &form {
            child.args(["--form", &format!("{key}={value}")]);
        }
    }
    let out = child.output().context("run helmp")?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    // 子进程的 stderr 有加载日志；失败时只留有用的部分。
    let problems: Vec<&str> = stderr
        .lines()
        .filter(|line| !line.contains("plugins 已加载") && !line.trim().is_empty())
        .collect();

    if list_mode {
        let prefix = format!("{base_id}#");
        let items: Vec<Value> = stdout
            .lines()
            .filter_map(|line| {
                let cols: Vec<&str> = line.split('\t').collect();
                let row_id = *cols.get(3)?;
                let uid = row_id.strip_prefix(&prefix)?;
                Some(json!({"uid": uid, "title": cols.get(1), "subtitle": cols.get(2), "id": row_id}))
            })
            .collect();
        let text = format!(
            "[dry-run] list {base_id} for {query:?}: {} item(s)\n{}{}",
            items.len(),
            items
                .iter()
                .map(|i| format!(
                    "  {}\t{}\t{}\n",
                    i["uid"].as_str().unwrap_or_default(),
                    i["title"].as_str().unwrap_or_default(),
                    i["subtitle"].as_str().unwrap_or_default()
                ))
                .collect::<String>(),
            if items.is_empty() && !problems.is_empty() {
                format!("stderr:\n{}", problems.join("\n"))
            } else {
                String::new()
            }
        );
        return Ok(ToolOutput::ok(
            text,
            json!({"query": query, "items": items, "dry_run": true}),
        ));
    }

    let data_out = json!({
        "query": query, "id": result_id, "dry_run": true, "success": out.status.success(),
        "output": stdout.trim_end(), "errors": problems,
    });
    Ok(if out.status.success() {
        ToolOutput::ok(
            format!(
                "Ran {result_id} with query {query:?} (dry run):\n{}",
                stdout.trim_end()
            ),
            data_out,
        )
    } else {
        ToolOutput::error(
            format!(
                "{result_id} failed (dry run):\n{}\n{}",
                stdout.trim_end(),
                problems.join("\n")
            )
            .trim_end()
            .to_string(),
            data_out,
        )
    })
}

// ---------------------------------------------------------------- submit

pub fn submit(args: &Value) -> Result<ToolOutput> {
    let dir = absolute(required(args, "dir")?)?;
    if args.get("user_confirmed") != Some(&Value::Bool(true)) {
        return Ok(ToolOutput::error(
            "Not submitted. Publishing makes the plugin public in the Helmp plugin center. Ask the user explicitly \
             whether they want to upload it, and call again with user_confirmed: true only after they clearly say yes.",
            json!({"submitted": false, "reason": "user_confirmed must be true"}),
        ));
    }
    let entry = registry::load_dir(&dir, Source::User);
    if let Err(errors) = &entry.loaded {
        bail!(
            "The plugin is invalid; fix it before publishing:\n{}",
            errors.join("\n")
        );
    }
    let exe = std::env::current_exe().context("locate the helmp executable")?;
    let out = Command::new(exe)
        .args(["plugins", "submit"])
        .arg(&dir)
        .arg("--yes")
        .output()
        .context("run helmp plugins submit")?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        if stderr.contains("\"submit\"")
            && (stderr.contains("Unknown subcommand") || stderr.contains("不认识的子命令"))
        {
            bail!(
                "This version of Helmp can't publish plugins yet (`helmp plugins submit` is missing). Tell the user to update Helmp."
            );
        }
        bail!("Publishing failed:\n{}{}", stdout, stderr);
    }
    let url = stdout
        .split_whitespace()
        .find(|word| word.starts_with("https://"))
        .map(str::to_string);
    Ok(ToolOutput::ok(
        format!("Submitted.\n{}", stdout.trim_end()),
        json!({"submitted": true, "url": url, "output": stdout.trim_end()}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_kind_is_a_valid_plugin() {
        // 脚手架的产物必须直接通过校验，agent 才能从「能跑」的状态开始改。
        let root = std::env::temp_dir().join(format!("helmp-mcp-scaffold-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for kind in ["starter", "quicklink", "http", "script", "list"] {
            let dir = root.join(kind);
            let out = scaffold(&json!({"id": format!("com.example.t-{kind}"), "name": "Thing", "kind": kind, "dir": dir})).unwrap();
            assert_eq!(out.data["valid"], true, "{kind}: {}", out.text);
            let entry = registry::load_dir(&dir, Source::User);
            assert_eq!(
                entry.plugin().unwrap().manifest.name.get(),
                "Thing",
                "{kind}"
            );
            // 不能覆盖已有的插件目录。
            assert!(
                scaffold(&json!({"id": "com.example.x", "name": "X", "kind": kind, "dir": dir}))
                    .is_err()
            );
        }
        assert!(scaffold(&json!({"id": "Bad Id", "name": "X"})).is_err());
        assert!(
            scaffold(&json!({"id": "dev.helmp.flomo", "name": "X", "dir": root.join("f")}))
                .is_err()
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn submit_requires_explicit_user_consent() {
        // 发布是公开行为：没有用户明确同意（user_confirmed 不是 true）绝不调用发布命令。
        for args in [
            json!({"dir": "/tmp/x"}),
            json!({"dir": "/tmp/x", "user_confirmed": false}),
            json!({"dir": "/tmp/x", "user_confirmed": "yes"}),
        ] {
            let out = submit(&args).unwrap();
            assert!(
                out.is_error && out.text.contains("Ask the user"),
                "{}",
                out.text
            );
            assert_eq!(out.data["submitted"], false);
        }
    }

    #[test]
    fn copy_dir_refuses_symlinks_and_keeps_executable_bits() {
        let root = std::env::temp_dir().join(format!("helmp-mcp-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/bin")).unwrap();
        std::fs::create_dir_all(root.join("src/.git")).unwrap();
        std::fs::write(root.join("src/bin/run.sh"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(
            root.join("src/bin/run.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        copy_dir(&root.join("src"), &root.join("dst")).unwrap();
        let mode = std::fs::metadata(root.join("dst/bin/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "脚本复制后要仍可执行");
        assert!(!root.join("dst/.git").exists());
        std::os::unix::fs::symlink("/etc/passwd", root.join("src/bin/link")).unwrap();
        assert!(copy_dir(&root.join("src"), &root.join("dst2")).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
