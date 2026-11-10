//! 从别的客户端导入服务器配置（只读，从不改动那些文件）：
//! - Claude Code：`~/.claude.json`（设置了 `CLAUDE_CONFIG_DIR` 时也看 `$CLAUDE_CONFIG_DIR/.claude.json`）
//!   顶层的 `mcpServers`；给了项目目录时再加 `<项目>/.mcp.json` 和 `.claude.json` 里这个项目的 `mcpServers`。
//! - Codex：`$CODEX_HOME/config.toml`（缺省 `~/.codex/config.toml`）的 `[mcp_servers.<名字>]`。
//! - Cursor：`~/.cursor/mcp.json`；给了项目目录时再加 `<项目>/.cursor/mcp.json`。
//! - 粘贴：常见的 `{"mcpServers": {...}}`，或直接 `{"名字": {...}}`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::servers::{ServerConfig, Transport, valid_name};
use crate::i18n::{tr, trf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Claude,
    Codex,
    Cursor,
    Pasted,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Claude => "Claude Code",
            Source::Codex => "Codex",
            Source::Cursor => "Cursor",
            Source::Pasted => tr("粘贴", "Pasted"),
        }
    }

    pub fn from_word(word: &str) -> Option<Source> {
        Some(match word.to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Source::Claude,
            "codex" => Source::Codex,
            "cursor" => Source::Cursor,
            _ => return None,
        })
    }
}

/// 找到的一个服务器。`server` 为 Err 时是不能导入的原因（如旧版 SSE 传输）。
#[derive(Debug, Clone)]
pub struct Found {
    pub source: Source,
    pub file: PathBuf,
    pub name: String,
    pub server: Result<ServerConfig, String>,
    /// 导入后还要用户补的东西（如 Codex 的 bearer_token_env_var 读不到）。
    pub note: Option<String>,
}

/// 把别的客户端里的名字变成 Helmp 允许的名字（`/` 之类换成 `-`）。
pub fn clean_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || "-_. ".contains(c) { c } else { '-' })
        .take(64)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if valid_name(&cleaned) {
        cleaned
    } else {
        "server".into()
    }
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| match item {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string_map(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    let value = match value {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    };
                    (key.clone(), value)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Claude / Cursor / 粘贴 共用的 JSON 服务器对象。
pub fn from_json(name: &str, value: &Value) -> Result<ServerConfig, String> {
    let kind = value
        .get("type")
        .or_else(|| value.get("transport"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let enabled = value.get("disabled").and_then(Value::as_bool) != Some(true)
        && value.get("enabled").and_then(Value::as_bool) != Some(false);
    let transport = if let Some(command) = value.get("command").and_then(Value::as_str) {
        Transport::Stdio {
            command: command.to_string(),
            args: strings(value.get("args")),
            env: string_map(value.get("env")),
            cwd: value.get("cwd").and_then(Value::as_str).map(str::to_string),
        }
    } else if let Some(url) = value
        .get("url")
        .or_else(|| value.get("serverUrl"))
        .and_then(Value::as_str)
    {
        if kind == "sse" {
            return Err(tr(
                "旧版 HTTP+SSE 传输暂不支持（只支持 stdio 和 Streamable HTTP）",
                "The legacy HTTP+SSE transport isn't supported yet (stdio and Streamable HTTP only)",
            )
            .into());
        }
        Transport::Http {
            url: url.to_string(),
            headers: string_map(value.get("headers")),
        }
    } else {
        return Err(tr(
            "既没有 command 也没有 url",
            "Has neither a command nor a url",
        )
        .into());
    };
    Ok(ServerConfig {
        name: clean_name(name),
        enabled,
        resident: false,
        transport,
        secrets: Vec::new(),
        timeout_secs: None,
    })
}

fn servers_in(object: Option<&Value>, source: Source, file: &Path, out: &mut Vec<Found>) {
    let Some(map) = object.and_then(Value::as_object) else {
        return;
    };
    for (name, value) in map {
        out.push(Found {
            source,
            file: file.to_path_buf(),
            name: clean_name(name),
            server: from_json(name, value),
            note: None,
        });
    }
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(value) => Some(value),
        Err(err) => {
            eprintln!("helmp: 读取 {} 失败：{err}", path.display());
            None
        }
    }
}

/// Claude Code 的 `.claude.json`：顶层 mcpServers，加上指定项目的。
pub fn parse_claude(value: &Value, file: &Path, project: Option<&Path>) -> Vec<Found> {
    let mut out = Vec::new();
    servers_in(value.get("mcpServers"), Source::Claude, file, &mut out);
    if let Some(project) = project
        && let Some(projects) = value.get("projects").and_then(Value::as_object)
    {
        let wanted = project.display().to_string();
        for (path, entry) in projects {
            if path.trim_end_matches('/') == wanted.trim_end_matches('/') {
                servers_in(entry.get("mcpServers"), Source::Claude, file, &mut out);
            }
        }
    }
    out
}

/// Cursor 的 mcp.json、项目的 .mcp.json 与粘贴的 JSON：`{"mcpServers": {...}}`。
pub fn parse_mcp_json(value: &Value, source: Source, file: &Path) -> Vec<Found> {
    let mut out = Vec::new();
    servers_in(value.get("mcpServers"), source, file, &mut out);
    out
}

fn toml_to_json(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(text) => Value::String(text.clone()),
        toml::Value::Integer(n) => Value::from(*n),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(items) => Value::Array(items.iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => Value::Object(
            table
                .iter()
                .map(|(key, value)| (key.clone(), toml_to_json(value)))
                .collect(),
        ),
    }
}

/// Codex 的 config.toml：`[mcp_servers.<名字>]`，http 用 `url` + `http_headers` /
/// `bearer_token_env_var`（从当前环境读得到就转成 Authorization 请求头，读不到就提示补填）。
pub fn parse_codex(text: &str, file: &Path) -> Result<Vec<Found>, String> {
    let table: toml::Table = text.parse().map_err(|err| format!("{err}"))?;
    let mut out = Vec::new();
    let Some(servers) = table.get("mcp_servers").and_then(toml::Value::as_table) else {
        return Ok(out);
    };
    for (name, value) in servers {
        let json = toml_to_json(value);
        let mut note = None;
        let mut fixed = json.clone();
        if let Some(headers) = json.get("http_headers") {
            fixed["headers"] = headers.clone();
        }
        let server = from_json(name, &fixed).map(|mut server| {
            if let (Transport::Http { headers, .. }, Some(var)) = (
                &mut server.transport,
                json.get("bearer_token_env_var").and_then(Value::as_str),
            ) {
                match std::env::var(var) {
                    Ok(token) if !token.is_empty() => {
                        headers.insert("Authorization".into(), format!("Bearer {token}"));
                    }
                    _ => {
                        note = Some(trf!(
                            "Codex 从环境变量 {var} 读令牌；导入后在编辑里补上 Authorization 请求头",
                            "Codex reads the token from ${var}; add an Authorization header after importing"
                        ))
                    }
                }
            }
            if json.get("env_http_headers").is_some() {
                note = Some(
                    tr(
                        "有从环境变量取值的请求头（env_http_headers），导入后请在编辑里补上",
                        "Some headers come from environment variables (env_http_headers); add them after importing",
                    )
                    .into(),
                );
            }
            server
        });
        out.push(Found {
            source: Source::Codex,
            file: file.to_path_buf(),
            name: clean_name(name),
            server,
            note,
        });
    }
    Ok(out)
}

/// 粘贴的 JSON：`{"mcpServers": {...}}` 或 `{"名字": {...}}`。
pub fn parse_pasted(text: &str) -> Result<Vec<Found>, String> {
    let value: Value = serde_json::from_str(text.trim()).map_err(|err| {
        trf!("不是合法的 JSON：{err}", "Not valid JSON: {err}")
    })?;
    let file = PathBuf::new();
    if value.get("mcpServers").is_some() {
        return Ok(parse_mcp_json(&value, Source::Pasted, &file));
    }
    if value.get("command").is_some() || value.get("url").is_some() {
        return Err(tr(
            "缺服务器名字：写成 {\"名字\": {\"command\": …}} 或 {\"mcpServers\": {…}}",
            "Missing a server name: use {\"name\": {\"command\": …}} or {\"mcpServers\": {…}}",
        )
        .into());
    }
    let is_servers = value.as_object().is_some_and(|map| {
        !map.is_empty()
            && map
                .values()
                .all(|v| v.get("command").is_some() || v.get("url").is_some())
    });
    if !is_servers {
        return Err(tr("没有找到服务器", "No servers found").into());
    }
    let mut out = Vec::new();
    servers_in(Some(&value), Source::Pasted, &file, &mut out);
    Ok(out)
}

fn claude_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        files.push(PathBuf::from(dir).join(".claude.json"));
    }
    if let Ok(home) = crate::paths::home_dir() {
        files.push(home.join(".claude.json"));
    }
    files.dedup();
    files
}

fn codex_file() -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME") {
        Some(dir) => Some(PathBuf::from(dir).join("config.toml")),
        None => crate::paths::home_dir()
            .ok()
            .map(|home| home.join(".codex/config.toml")),
    }
}

/// 扫描一个来源（只读）。`project`：项目目录，额外看项目级配置。
pub fn scan(source: Source, project: Option<&Path>) -> Vec<Found> {
    let mut out = Vec::new();
    match source {
        Source::Claude => {
            for file in claude_files() {
                if let Some(value) = read_json(&file) {
                    out.extend(parse_claude(&value, &file, project));
                }
            }
            if let Some(project) = project {
                let file = project.join(".mcp.json");
                if let Some(value) = read_json(&file) {
                    out.extend(parse_mcp_json(&value, Source::Claude, &file));
                }
            }
        }
        Source::Codex => {
            if let Some(file) = codex_file()
                && let Ok(text) = std::fs::read_to_string(&file)
            {
                match parse_codex(&text, &file) {
                    Ok(found) => out.extend(found),
                    Err(err) => eprintln!("helmp: 读取 {} 失败：{err}", file.display()),
                }
            }
        }
        Source::Cursor => {
            let mut files = Vec::new();
            if let Ok(home) = crate::paths::home_dir() {
                files.push(home.join(".cursor/mcp.json"));
            }
            if let Some(project) = project {
                files.push(project.join(".cursor/mcp.json"));
            }
            for file in files {
                if let Some(value) = read_json(&file) {
                    out.extend(parse_mcp_json(&value, Source::Cursor, &file));
                }
            }
        }
        Source::Pasted => {}
    }
    // 同一来源同名的只留第一个（项目级与用户级重名时以用户级为准）。
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|found| seen.insert(found.name.clone()));
    out
}

pub fn scan_all(project: Option<&Path>) -> Vec<Found> {
    [Source::Claude, Source::Codex, Source::Cursor]
        .into_iter()
        .flat_map(|source| scan(source, project))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_json_servers_are_read_with_project_scope() {
        // Claude Code 的三种写法（stdio、http、旧 SSE）都要认出来；SSE 列出来但标明不支持。
        let value = json!({
            "numStartups": 3,
            "mcpServers": {
                "github": {"type": "stdio", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"],
                           "env": {"GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_x"}},
                "context7": {"type": "http", "url": "https://mcp.context7.com/mcp", "headers": {"Authorization": "Bearer k"}},
                "old": {"type": "sse", "url": "https://x.example/sse"}
            },
            "projects": {
                "/Users/me/app": {"mcpServers": {"local db": {"command": "uvx", "args": ["db-mcp"]}}},
                "/Users/me/other": {"mcpServers": {"nope": {"command": "x"}}}
            }
        });
        let file = Path::new("/h/.claude.json");
        let found = parse_claude(&value, file, Some(Path::new("/Users/me/app/")));
        let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["github", "context7", "old", "local db"], "保持文件里的顺序");
        let github = found[0].server.as_ref().unwrap();
        assert!(matches!(&github.transport, Transport::Stdio { command, env, .. }
            if command == "npx" && env["GITHUB_PERSONAL_ACCESS_TOKEN"] == "ghp_x"));
        assert!(matches!(&found[1].server.as_ref().unwrap().transport, Transport::Http { headers, .. }
            if headers["Authorization"] == "Bearer k"));
        assert!(found[2].server.is_err(), "SSE 不支持");
        assert_eq!(parse_claude(&value, file, None).len(), 3);
    }

    #[test]
    fn codex_toml_servers_are_read() {
        let text = r#"
model = "gpt-5"
[mcp_servers.fetch]
command = "uvx"
args = ["mcp-server-fetch"]

[mcp_servers.docs]
url = "https://mcp.example.com/mcp"
http_headers = { "X-Region" = "eu" }
bearer_token_env_var = "HELMP_TEST_SURELY_UNSET_TOKEN"

[mcp_servers.off]
command = "node"
args = ["server.js"]
env = { "API_KEY" = "k", "PORT" = 3000 }
enabled = false
"#;
        let found = parse_codex(text, Path::new("/h/.codex/config.toml")).unwrap();
        let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["docs", "fetch", "off"], "toml 表按名字排序");
        let docs = &found[0];
        assert!(matches!(&docs.server.as_ref().unwrap().transport, Transport::Http { headers, .. }
            if headers["X-Region"] == "eu" && !headers.contains_key("Authorization")));
        assert!(docs.note.is_some(), "读不到令牌要提示补填");
        let off = found[2].server.as_ref().unwrap();
        assert!(!off.enabled);
        assert!(matches!(&off.transport, Transport::Stdio { env, .. } if env["PORT"] == "3000"));
        assert!(parse_codex("[[broken", Path::new("x")).is_err());
    }

    #[test]
    fn cursor_and_pasted_json_are_read() {
        let value = json!({"mcpServers": {"playwright": {"command": "npx", "args": ["@playwright/mcp@latest"]},
                                          "remote": {"url": "https://x.example/mcp", "disabled": true}}});
        let found = parse_mcp_json(&value, Source::Cursor, Path::new("/h/.cursor/mcp.json"));
        assert_eq!(found.len(), 2);
        assert!(!found[1].server.as_ref().unwrap().enabled);

        assert_eq!(parse_pasted(&value.to_string()).unwrap().len(), 2);
        let bare = parse_pasted(r#"{"time": {"command": "uvx", "args": ["mcp-server-time"]}}"#).unwrap();
        assert_eq!(bare[0].name, "time");
        assert!(parse_pasted(r#"{"command": "uvx"}"#).is_err(), "没有名字");
        assert!(parse_pasted("{").is_err());
        assert!(parse_pasted(r#"{"a": 1}"#).is_err());
    }

    #[test]
    fn foreign_names_are_cleaned() {
        assert_eq!(clean_name("@scope/server"), "-scope-server");
        assert_eq!(clean_name("  "), "server");
        assert_eq!(clean_name("中文名"), "中文名");
    }
}
