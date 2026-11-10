//! 把 Helmp 的 MCP 服务登记到 coding agent 的配置里（只在用户点「连接」或运行 `helmp mcp install` 时）：
//!
//! | agent | 配置 | 写法 |
//! |---|---|---|
//! | Claude Code | `claude mcp add --scope user helmp -- <helmp> mcp`；没有 `claude` 命令时改 `~/.claude.json`（设了 `CLAUDE_CONFIG_DIR` 则是其下的 `.claude.json`）的 `mcpServers.helmp` | JSON |
//! | Codex | `$CODEX_HOME/config.toml`（默认 `~/.codex`）的 `[mcp_servers.helmp]` | TOML |
//! | Cursor | `~/.cursor/mcp.json` 的 `mcpServers.helmp` | JSON |
//!
//! 改文件前先备份成 `<文件>.helmp-backup`；已经是同样的配置就什么也不写（幂等）。
//! 只动 `helmp` 这一项，其余内容（包括 TOML 里的注释）原样保留。

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

pub const SERVER_NAME: &str = "helmp";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Cursor,
}

impl Agent {
    pub const ALL: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::Cursor];

    pub fn parse(name: &str) -> Option<Agent> {
        match name.to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claudecode" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            "cursor" => Some(Agent::Cursor),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Cursor => "Cursor",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Cursor => "cursor",
        }
    }

    /// 这个 agent 的配置文件。
    pub fn config_path(self) -> Result<PathBuf> {
        let home = crate::paths::home_dir()?;
        Ok(match self {
            Agent::Claude => match std::env::var_os("CLAUDE_CONFIG_DIR") {
                Some(dir) => PathBuf::from(dir).join(".claude.json"),
                None => home.join(".claude.json"),
            },
            Agent::Codex => match std::env::var_os("CODEX_HOME") {
                Some(dir) => PathBuf::from(dir).join("config.toml"),
                None => home.join(".codex/config.toml"),
            },
            Agent::Cursor => home.join(".cursor/mcp.json"),
        })
    }
}

/// 登记的启动命令：Helmp 自己的可执行文件 + `mcp`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub command: String,
    pub args: Vec<String>,
}

impl Launch {
    pub fn current() -> Result<Launch> {
        let exe = std::env::current_exe().context("locate the helmp executable")?;
        Ok(Launch {
            command: exe.display().to_string(),
            args: vec!["mcp".into()],
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    NotConnected,
    /// 已登记，指向当前这个 Helmp。
    Connected,
    /// 已登记，但命令不同（比如 Helmp 挪了位置），重新连接会改过来。
    Outdated(String),
}

impl Status {
    pub fn label(&self) -> String {
        use crate::i18n::{tr, trf};
        match self {
            Status::NotConnected => tr("未连接", "Not connected").into(),
            Status::Connected => tr("已连接", "Connected").into(),
            Status::Outdated(command) => trf!(
                "已连接（指向 {command}）",
                "Connected (points to {command})"
            ),
        }
    }
}

// ---------------------------------------------------------------- JSON（Claude Code、Cursor）

fn read_json(path: &Path) -> Result<Value> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("{} isn't valid JSON; not touching it", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(err) => Err(err).with_context(|| format!("read {}", path.display())),
    }
}

fn json_status(config: &Value, launch: &Launch) -> Status {
    let Some(entry) = config.pointer(&format!("/mcpServers/{SERVER_NAME}")) else {
        return Status::NotConnected;
    };
    let command = entry
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let args: Vec<&str> = entry
        .get("args")
        .and_then(Value::as_array)
        .map(|args| args.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if command == launch.command
        && args == launch.args.iter().map(String::as_str).collect::<Vec<_>>()
    {
        Status::Connected
    } else {
        Status::Outdated(command.to_string())
    }
}

/// 设置 `mcpServers.helmp`，其余键保持原样（serde_json 保留键的顺序）。
fn json_with_server(mut config: Value, launch: &Launch, with_type: bool) -> Result<Value> {
    if !config.is_object() {
        bail!("the config file isn't a JSON object");
    }
    let servers = config
        .as_object_mut()
        .context("config object")?
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    let servers = servers
        .as_object_mut()
        .context("mcpServers isn't an object")?;
    let mut entry = json!({"command": launch.command, "args": launch.args, "env": {}});
    if with_type {
        entry = json!({"type": "stdio", "command": launch.command, "args": launch.args, "env": {}});
    }
    servers.insert(SERVER_NAME.into(), entry);
    Ok(config)
}

// ---------------------------------------------------------------- TOML（Codex）

/// 表头的表名：`[mcp_servers.helmp]` → `mcp_servers.helmp`；不是表头返回 None。
fn table_name(line: &str) -> Option<String> {
    let line = line.trim();
    let inner = line
        .strip_prefix("[[")
        .and_then(|l| l.split("]]").next())
        .or_else(|| line.strip_prefix('[').and_then(|l| l.split(']').next()))?;
    Some(
        inner
            .split('.')
            .map(|part| part.trim().trim_matches('"').trim_matches('\''))
            .collect::<Vec<_>>()
            .join("."),
    )
}

fn is_ours(table: &str) -> bool {
    let ours = format!("mcp_servers.{SERVER_NAME}");
    table == ours || table.starts_with(&format!("{ours}."))
}

fn toml_string(text: &str) -> String {
    // TOML 基本字符串的转义与 JSON 相同（反斜杠、引号、控制字符）。
    serde_json::to_string(text).unwrap_or_else(|_| format!("\"{text}\""))
}

fn toml_block(launch: &Launch) -> String {
    let args: Vec<String> = launch.args.iter().map(|a| toml_string(a)).collect();
    format!(
        "[mcp_servers.{SERVER_NAME}]\ncommand = {}\nargs = [{}]\n",
        toml_string(&launch.command),
        args.join(", ")
    )
}

/// 去掉已有的 `[mcp_servers.helmp]`（及其子表），在末尾追加新的一段。
fn toml_with_server(text: &str, launch: &Launch) -> String {
    let mut kept = Vec::new();
    let mut skipping = false;
    for line in text.lines() {
        if let Some(table) = table_name(line) {
            skipping = is_ours(&table);
        }
        if !skipping {
            kept.push(line);
        }
    }
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(&toml_block(launch));
    out
}

/// 读 `[mcp_servers.helmp]` 的 command / args（只认本函数写出的单行写法）。
fn toml_status(text: &str, launch: &Launch) -> Status {
    let mut inside = false;
    let mut found = false;
    let (mut command, mut args) = (String::new(), String::new());
    for line in text.lines() {
        if let Some(table) = table_name(line) {
            inside = table == format!("mcp_servers.{SERVER_NAME}");
            found |= inside;
            continue;
        }
        if !inside {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            match key.trim() {
                "command" => {
                    command = serde_json::from_str(value.trim())
                        .unwrap_or_else(|_| value.trim().to_string())
                }
                "args" => args = value.trim().to_string(),
                _ => {}
            }
        }
    }
    if !found {
        return Status::NotConnected;
    }
    let expected: Vec<String> = launch.args.iter().map(|a| toml_string(a)).collect();
    if command == launch.command && args == format!("[{}]", expected.join(", ")) {
        Status::Connected
    } else {
        Status::Outdated(command)
    }
}

// ---------------------------------------------------------------- 状态与写入

pub fn status(agent: Agent) -> Result<Status> {
    status_for(agent, &Launch::current()?)
}

fn status_for(agent: Agent, launch: &Launch) -> Result<Status> {
    let path = agent.config_path()?;
    Ok(match agent {
        Agent::Claude | Agent::Cursor => json_status(&read_json(&path)?, launch),
        Agent::Codex => toml_status(&std::fs::read_to_string(&path).unwrap_or_default(), launch),
    })
}

/// 写之前备份：`<文件>.helmp-backup`（文件不存在就不用备份）。
fn back_up(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let mut name = path.file_name().context("file name")?.to_os_string();
    name.push(".helmp-backup");
    let backup = path.with_file_name(name);
    std::fs::copy(path, &backup).with_context(|| format!("back up {}", path.display()))?;
    Ok(Some(backup))
}

/// 先写临时文件再改名，并保留原文件的权限（~/.claude.json 是 0600）。
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let tmp = path.with_extension("helmp-tmp");
    std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    if let Ok(metadata) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, metadata.permissions());
    }
    std::fs::rename(&tmp, path).with_context(|| format!("save {}", path.display()))
}

/// `claude` 命令：`HELMP_CLAUDE_CLI` 可以指定（空字符串 = 不用命令、直接改文件，测试用），
/// 否则在 PATH 和常见安装位置里找。图形界面从访达启动时 PATH 很短，所以要看常见位置。
fn claude_cli() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("HELMP_CLAUDE_CLI") {
        return (!value.is_empty()).then(|| PathBuf::from(value));
    }
    let home = crate::paths::home_dir().ok();
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join("claude"))
                .collect()
        })
        .unwrap_or_default();
    if let Some(home) = home {
        candidates.push(home.join(".claude/local/claude"));
        candidates.push(home.join(".local/bin/claude"));
    }
    candidates.push("/opt/homebrew/bin/claude".into());
    candidates.push("/usr/local/bin/claude".into());
    candidates.into_iter().find(|path| path.is_file())
}

/// 连接的结果，给界面和命令行显示。
#[derive(Debug)]
pub struct Installed {
    pub path: PathBuf,
    pub backup: Option<PathBuf>,
    /// false = 原本就是这个配置，什么也没改。
    pub changed: bool,
    pub via_cli: bool,
}

pub fn install(agent: Agent) -> Result<Installed> {
    install_with(agent, &Launch::current()?)
}

fn install_with(agent: Agent, launch: &Launch) -> Result<Installed> {
    let path = agent.config_path()?;
    if status_for(agent, launch)? == Status::Connected {
        return Ok(Installed {
            path,
            backup: None,
            changed: false,
            via_cli: false,
        });
    }
    match agent {
        Agent::Claude => {
            if let Some(cli) = claude_cli() {
                let backup = back_up(&path)?;
                // 已有（指向别处）的先删掉，`mcp add` 遇到同名会失败。
                let _ = Command::new(&cli)
                    .args(["mcp", "remove", "--scope", "user", SERVER_NAME])
                    .output();
                let out = Command::new(&cli)
                    .args(["mcp", "add", "--scope", "user", SERVER_NAME, "--"])
                    .arg(&launch.command)
                    .args(&launch.args)
                    .output()
                    .with_context(|| format!("run {}", cli.display()))?;
                if !out.status.success() {
                    bail!(
                        "`claude mcp add` failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    );
                }
                return Ok(Installed {
                    path,
                    backup,
                    changed: true,
                    via_cli: true,
                });
            }
            let config = json_with_server(read_json(&path)?, launch, true)?;
            let backup = back_up(&path)?;
            write_atomic(&path, &(serde_json::to_string_pretty(&config)? + "\n"))?;
            Ok(Installed {
                path,
                backup,
                changed: true,
                via_cli: false,
            })
        }
        Agent::Cursor => {
            let config = json_with_server(read_json(&path)?, launch, false)?;
            let backup = back_up(&path)?;
            write_atomic(&path, &(serde_json::to_string_pretty(&config)? + "\n"))?;
            Ok(Installed {
                path,
                backup,
                changed: true,
                via_cli: false,
            })
        }
        Agent::Codex => {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
            };
            let next = toml_with_server(&text, launch);
            let backup = back_up(&path)?;
            write_atomic(&path, &next)?;
            Ok(Installed {
                path,
                backup,
                changed: true,
                via_cli: false,
            })
        }
    }
}

/// 给人看的一句话。
pub fn describe(agent: Agent, installed: &Installed) -> String {
    use crate::i18n::trf;
    let name = agent.name();
    if !installed.changed {
        return trf!(
            "{name} 已经连接到 Helmp（{}），没有改动",
            "{name} is already connected to Helmp ({}); nothing changed",
            installed.path.display()
        );
    }
    let backup = installed
        .backup
        .as_ref()
        .map(|b| trf!("，原文件备份在 {}", "; backup at {}", b.display()))
        .unwrap_or_default();
    let how = if installed.via_cli {
        " (claude mcp add)"
    } else {
        ""
    };
    trf!(
        "已把 Helmp 连接到 {name}{how}：{}{backup}。重启 {name} 后生效",
        "Connected Helmp to {name}{how}: {}{backup}. Restart {name} to use it",
        installed.path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> Launch {
        Launch {
            command: "/Applications/Helmp.app/Contents/MacOS/helmp".into(),
            args: vec!["mcp".into()],
        }
    }

    #[test]
    fn json_configs_keep_other_servers_and_settings() {
        // ~/.claude.json 里有大量别的设置，Cursor 的 mcp.json 里有别的服务：只能加 helmp 这一项。
        let before =
            json!({"numStartups": 3, "mcpServers": {"other": {"command": "x"}}, "projects": {}});
        let after = json_with_server(before.clone(), &launch(), true).unwrap();
        assert_eq!(after["numStartups"], 3);
        assert_eq!(after["mcpServers"]["other"]["command"], "x");
        assert_eq!(after["mcpServers"]["helmp"]["type"], "stdio");
        assert_eq!(after["mcpServers"]["helmp"]["args"], json!(["mcp"]));
        let keys: Vec<_> = after.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            ["numStartups", "mcpServers", "projects"],
            "键的顺序不能被打乱"
        );
        assert_eq!(json_status(&after, &launch()), Status::Connected);
        assert_eq!(json_status(&before, &launch()), Status::NotConnected);
        let moved = Launch {
            command: "/tmp/helmp".into(),
            ..launch()
        };
        assert_eq!(
            json_status(&after, &moved),
            Status::Outdated(launch().command)
        );
        let cursor = json_with_server(json!({}), &launch(), false).unwrap();
        assert!(cursor["mcpServers"]["helmp"].get("type").is_none());
        assert!(json_with_server(json!([1]), &launch(), false).is_err());
    }

    #[test]
    fn codex_toml_replaces_only_our_table_and_keeps_comments() {
        let before = "# my codex\nmodel = \"o3\"\n\n[mcp_servers.helmp]\ncommand = \"/old/helmp\"\nargs = [\"mcp\"]\n\n[mcp_servers.helmp.env]\nA = \"1\"\n\n[mcp_servers.other]\ncommand = \"x\" # keep\n";
        let after = toml_with_server(before, &launch());
        assert!(after.starts_with("# my codex\nmodel = \"o3\"\n"), "{after}");
        assert!(
            after.contains("[mcp_servers.other]\ncommand = \"x\" # keep"),
            "{after}"
        );
        assert!(
            !after.contains("/old/helmp") && !after.contains("[mcp_servers.helmp.env]"),
            "{after}"
        );
        assert_eq!(after.matches("[mcp_servers.helmp]").count(), 1, "{after}");
        assert_eq!(toml_status(&after, &launch()), Status::Connected);
        assert_eq!(
            toml_status(before, &launch()),
            Status::Outdated("/old/helmp".into())
        );
        assert_eq!(toml_status("model = 1\n", &launch()), Status::NotConnected);
        // 再写一次结果不变（幂等）。
        assert_eq!(toml_with_server(&after, &launch()), after);
        assert_eq!(toml_with_server("", &launch()), toml_block(&launch()));
        // 带空格和引号的路径要正确转义。
        let odd = Launch {
            command: "/Users/a b/\"x\"/helmp".into(),
            args: vec!["mcp".into()],
        };
        let text = toml_with_server("", &odd);
        assert!(
            text.contains(r#"command = "/Users/a b/\"x\"/helmp""#),
            "{text}"
        );
        assert_eq!(toml_status(&text, &odd), Status::Connected);
    }

    #[test]
    fn table_headers_are_recognized_with_quotes_and_spaces() {
        assert_eq!(
            table_name("[mcp_servers.helmp]").as_deref(),
            Some("mcp_servers.helmp")
        );
        assert_eq!(
            table_name(" [ mcp_servers . \"helmp\" ] # c").as_deref(),
            Some("mcp_servers.helmp")
        );
        assert_eq!(table_name("[[profiles]]").as_deref(), Some("profiles"));
        assert_eq!(table_name("args = [\"mcp\"]"), None);
        assert!(is_ours("mcp_servers.helmp.env"));
        assert!(!is_ours("mcp_servers.helmpx"));
    }
}
