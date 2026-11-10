//! MCP 协议层：stdio 上按行收发 JSON-RPC 2.0（MCP 的 stdio 传输），手写而不用 rmcp——
//! rmcp 要求 tokio 运行时，而 Helmp 全是同步代码；这里要的只是 initialize / tools / resources / prompts。
//!
//! - 协议版本：客户端要的版本在 [`PROTOCOL_VERSIONS`] 里就照用，否则回最新版。
//! - `tools/call` 各开一个线程：`helmp_run` 可能要等用户在面板里确认（最多约一分钟），不能堵住 ping 等请求。
//! - stdout 只写协议消息；Helmp 自己的日志都走 stderr。

use std::io::{BufRead as _, Write as _};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Result;
use serde_json::{Value, json};

use crate::engine::Engine;

use super::{guide, operate, plugin_dev};

/// 支持的协议版本，新的在前。
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Helmp is the user's macOS launcher. Use the helmp_plugin_* tools to build Helmp \
plugins for features the user asks for (start with helmp_plugin_guide), and helmp_search / helmp_run to operate \
Helmp. Side effects always need the user's approval in the Helmp panel; installed plugins stay disabled until the \
user enables them; never upload a plugin unless the user explicitly agreed.";

/// 一次工具调用的结果：给模型看的文字 + 结构化数据。
pub struct ToolOutput {
    pub text: String,
    pub data: Value,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(text: impl Into<String>, data: Value) -> Self {
        Self {
            text: text.into(),
            data,
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>, data: Value) -> Self {
        Self {
            text: text.into(),
            data,
            is_error: true,
        }
    }

    fn into_json(self) -> Value {
        let mut result = json!({
            "content": [{"type": "text", "text": self.text}],
            "isError": self.is_error,
        });
        if self.data.is_object() {
            result["structuredContent"] = self.data;
        }
        result
    }
}

pub struct Server {
    pub engine: Engine,
    /// 客户端在 initialize 里报的名字（如 `claude-code`），确认页上显示成「Claude Code 想执行…」。
    client: Mutex<String>,
    started: OnceLock<()>,
}

impl Server {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            client: Mutex::new(String::new()),
            started: OnceLock::new(),
        }
    }

    /// 给用户看的 agent 名字。
    pub fn agent_name(&self) -> String {
        let raw = self
            .client
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        agent_display_name(&raw)
    }

    /// 第一次搜索 / 执行前建好各 provider 的索引（与 `helmp query` 一样，剪贴板采集关闭）。
    pub fn ensure_started(&self) {
        self.started.get_or_init(|| crate::cli::start(&self.engine));
    }

    /// 处理一条消息；通知和客户端发来的响应返回 None。
    pub fn handle(&self, message: &Value) -> Option<Value> {
        let method = message.get("method").and_then(Value::as_str)?;
        let id = message.get("id")?.clone();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => self.call_tool(&params),
            "resources/list" => Ok(json!({"resources": resources()})),
            "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
            "resources/read" => read_resource(&params),
            "prompts/list" => Ok(json!({"prompts": prompts()})),
            "prompts/get" => get_prompt(&params),
            _ => Err((-32601, format!("Method not found: {method}"))),
        };
        Some(match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        if let Some(name) = params.pointer("/clientInfo/name").and_then(Value::as_str) {
            *self
                .client
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = name.to_string();
        }
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let version = PROTOCOL_VERSIONS
            .iter()
            .find(|v| **v == requested)
            .copied()
            .unwrap_or(PROTOCOL_VERSIONS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": {
                "tools": {"listChanged": false},
                "resources": {"listChanged": false},
                "prompts": {"listChanged": false},
            },
            "serverInfo": {"name": "helmp", "title": "Helmp", "version": env!("CARGO_PKG_VERSION")},
            "instructions": INSTRUCTIONS,
        })
    }

    fn call_tool(&self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let empty = json!({});
        let args = params
            .get("arguments")
            .filter(|a| a.is_object())
            .unwrap_or(&empty);
        let output = match name {
            "helmp_plugin_guide" => Ok(plugin_dev::guide(args)),
            "helmp_plugin_scaffold" => plugin_dev::scaffold(args),
            "helmp_plugin_validate" => plugin_dev::validate(args),
            "helmp_plugin_install" => plugin_dev::install(self, args),
            "helmp_plugin_test" => plugin_dev::test(self, args),
            "helmp_plugin_list" => plugin_dev::list(),
            "helmp_plugin_submit" => plugin_dev::submit(args),
            "helmp_search" => operate::search(self, args),
            "helmp_run" => operate::run(self, args),
            _ => return Err((-32602, format!("Unknown tool: {name}"))),
        };
        // 工具自己的失败（参数不对、校验失败、用户拒绝）放在结果里，让模型看到原因。
        let output = output.unwrap_or_else(|err| {
            ToolOutput::error(format!("{err:#}"), json!({"error": format!("{err:#}")}))
        });
        Ok(output.into_json())
    }
}

/// `claude-code` → `Claude Code`；认不出的原样显示。
pub fn agent_display_name(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("claude") {
        "Claude Code".into()
    } else if lower.contains("codex") {
        "Codex".into()
    } else if lower.contains("cursor") {
        "Cursor".into()
    } else if raw.trim().is_empty() {
        "Agent".into()
    } else {
        raw.trim().chars().take(40).collect()
    }
}

fn resources() -> Value {
    json!([
        {
            "uri": guide::RESOURCE_EN, "name": "helmp-plugin-guide", "title": "Helmp plugin guide",
            "description": "Workflow, manifest reference, templates, permissions, list protocol, testing and examples for Helmp plugins.",
            "mimeType": "text/markdown"
        },
        {
            "uri": guide::RESOURCE_ZH, "name": "helmp-plugin-guide-zh", "title": "Helmp 插件开发指南（中文）",
            "description": "Same guide with the Chinese plugin documentation.",
            "mimeType": "text/markdown"
        }
    ])
}

fn read_resource(params: &Value) -> Result<Value, (i64, String)> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let zh = match uri {
        guide::RESOURCE_EN => false,
        guide::RESOURCE_ZH => true,
        _ => return Err((-32002, format!("Resource not found: {uri}"))),
    };
    Ok(json!({"contents": [{"uri": uri, "mimeType": "text/markdown", "text": guide::text(zh)}]}))
}

fn prompts() -> Value {
    json!([{
        "name": "create_helmp_plugin",
        "title": "Create a Helmp plugin",
        "description": "Turn a feature request into a working Helmp plugin: design, scaffold, validate, dry-run test, install, show the user, then ask whether to publish it.",
        "arguments": [{"name": "request", "description": "What the user wants Helmp to do, in their own words", "required": true}]
    }])
}

fn get_prompt(params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if name != "create_helmp_plugin" {
        return Err((-32602, format!("Unknown prompt: {name}")));
    }
    let request = params
        .pointer("/arguments/request")
        .and_then(Value::as_str)
        .filter(|r| !r.trim().is_empty())
        .ok_or((-32602, "Missing required argument: request".to_string()))?;
    Ok(json!({
        "description": "Build a Helmp plugin for this request",
        "messages": [{"role": "user", "content": {"type": "text", "text": guide::prompt(request)}}]
    }))
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

fn annotations(
    title: &str,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
    open_world: bool,
) -> Value {
    json!({
        "title": title,
        "readOnlyHint": read_only,
        "destructiveHint": destructive,
        "idempotentHint": idempotent,
        "openWorldHint": open_world,
    })
}

/// 工具清单。描述写给模型看：何时用、会发生什么、要先问用户什么。
pub fn tools() -> Value {
    let string_map = json!({"type": "object", "additionalProperties": {"type": "string"}});
    json!([
        {
            "name": "helmp_plugin_guide",
            "title": "Helmp plugin guide",
            "description": "Returns the authoritative guide for building Helmp plugins: the agent workflow, the full plugin.json reference (commands, actions, templates, preferences, permissions), the script list protocol, how to test, and complete bundled examples (flomo, quick links, Obsidian). Read it before writing a plugin.",
            "inputSchema": schema(json!({"language": {"type": "string", "enum": ["en", "zh"], "description": "Documentation language (default en)"}}), &[]),
            "annotations": annotations("Helmp plugin guide", true, false, true, false),
        },
        {
            "name": "helmp_plugin_scaffold",
            "title": "Scaffold a Helmp plugin",
            "description": "Creates a new, valid plugin folder from a template, outside Helmp's live plugins folder (default <Helmp data>/plugins-dev/<id>). Edit the returned files, then validate, test and install. Fails if the folder already exists and isn't empty.",
            "inputSchema": schema(json!({
                "id": {"type": "string", "description": "Reverse-DNS plugin id, lowercase: com.<you>.<name>"},
                "name": {"type": "string", "description": "Display name"},
                "kind": {"type": "string", "enum": ["starter", "quicklink", "http", "script", "list"], "description": "Template: quicklink = keyword → URL; http = form → HTTP request with a secret webhook/token preference; script = run a script; list = script-driven list; starter = one of each (default)"},
                "dir": {"type": "string", "description": "Absolute folder to create (optional)"}
            }), &["id", "name"]),
            "annotations": annotations("Scaffold a Helmp plugin", false, false, false, false),
        },
        {
            "name": "helmp_plugin_validate",
            "title": "Validate a Helmp plugin",
            "description": "Validates a plugin folder exactly like Helmp does when loading it. Returns every problem (prefixed with the field path, e.g. `commands[0].action.url: …`) or, when valid, the commands and the permissions the user will be asked to approve.",
            "inputSchema": schema(json!({"dir": {"type": "string", "description": "Absolute path of the plugin folder (contains plugin.json)"}}), &["dir"]),
            "annotations": annotations("Validate a Helmp plugin", true, false, true, false),
        },
        {
            "name": "helmp_plugin_install",
            "title": "Install a plugin into Helmp",
            "description": "Validates the folder and copies it into Helmp's plugins folder (replacing an earlier copy with the same id); Helmp reloads it within seconds. A newly installed plugin stays DISABLED: the user must review its permissions and turn it on in Helmp › Settings › Plugins (opened for them if Helmp is running). Tell the user to do that; you cannot enable it. If permissions changed since the user enabled it, it needs their review again.",
            "inputSchema": schema(json!({"dir": {"type": "string", "description": "Absolute path of the plugin folder"}}), &["dir"]),
            "annotations": annotations("Install a plugin into Helmp", false, false, true, false),
        },
        {
            "name": "helmp_plugin_test",
            "title": "Test-run a plugin command",
            "description": "Runs one command of a plugin headless and returns what Helmp would show. With dry_run (default true) it runs in an isolated sandbox with the plugin enabled: opens, copies, pastes and HTTP requests to real hosts are only reported as `[dry-run] …` lines, while requests to 127.0.0.1/localhost are really sent (use a local mock server) and scripts really run. For a list command without `item`, returns the list items. dry_run=false runs it for real through the user's Helmp: the plugin must be installed and enabled, and the user approves the run in the Helmp panel.",
            "inputSchema": schema(json!({
                "plugin": {"type": "string", "description": "Plugin folder (absolute path) or the id of an installed plugin"},
                "command": {"type": "string", "description": "Command id from plugin.json"},
                "input": {"type": "string", "description": "Text typed after the command's keyword; becomes {{query}}"},
                "form": {"type": "object", "additionalProperties": {"type": "string"}, "description": "Form field values by field id"},
                "prefs": {"type": "object", "additionalProperties": {"type": "string"}, "description": "Dry run only: non-secret preference values by preference id"},
                "secrets": {"type": "object", "additionalProperties": {"type": "string"}, "description": "Dry run only: secret preference values by preference id (never stored)"},
                "item": {"type": "string", "description": "List commands: uid of the item to run"},
                "dry_run": {"type": "boolean", "default": true}
            }), &["plugin", "command"]),
            "annotations": annotations("Test-run a plugin command", false, false, false, true),
        },
        {
            "name": "helmp_plugin_list",
            "title": "List Helmp plugins",
            "description": "Lists bundled and user plugins with their state (enabled, disabled, needs-consent, error), errors, folder, permissions and commands.",
            "inputSchema": schema(json!({}), &[]),
            "annotations": annotations("List Helmp plugins", true, false, true, false),
        },
        {
            "name": "helmp_plugin_submit",
            "title": "Publish a plugin to the Helmp plugin center",
            "description": "Uploads a plugin folder to the PUBLIC Helmp plugin center (opens a pull request) and returns its URL. Before calling, you MUST ask the user explicitly whether they want to publish this plugin publicly, and only call with user_confirmed=true after they clearly said yes. Never call it on your own initiative.",
            "inputSchema": schema(json!({
                "dir": {"type": "string", "description": "Absolute path of the plugin folder"},
                "user_confirmed": {"type": "boolean", "description": "true only if the user explicitly agreed to publish this plugin publicly in this conversation"}
            }), &["dir", "user_confirmed"]),
            "annotations": annotations("Publish a plugin", false, false, false, true),
        },
        {
            "name": "helmp_search",
            "title": "Search Helmp",
            "description": "Searches Helmp like typing in its panel: apps, calculator and unit/currency conversions, time zones, developer tools (base64, hashes, UUIDs, JSON…), dictionary, system commands, windows, processes, files, plugin commands. Returns result ids, titles, subtitles, actions, form fields and whether running the result needs the user's approval. Clipboard history and agent session transcripts are excluded unless the user enabled sharing them.",
            "inputSchema": schema(json!({
                "query": {"type": "string", "description": "What you would type in the Helmp panel, e.g. `12*7`, `100 usd to cny`, `time tokyo`, `base64 hello`, `flomo idea`"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 50, "default": 20}
            }), &["query"]),
            "annotations": annotations("Search Helmp", true, false, true, false),
        },
        {
            "name": "helmp_run",
            "title": "Run a Helmp result",
            "description": "Runs a result found with helmp_search (same query). Pure computations (calculator, conversions, time, developer tools, dictionary, translation, OCR of an image path) return their text directly and nothing is copied. Anything with a side effect (open, copy, paste, kill, window moves, toggles, plugin HTTP requests, scripts, system commands) is shown to the user in the Helmp panel as \"<agent> wants to run …\" and runs only if they press Return within 60 seconds; Esc or no answer means denied. Requires the Helmp app to be running for side effects.",
            "inputSchema": schema(json!({
                "query": {"type": "string", "description": "The same query you passed to helmp_search"},
                "id": {"type": "string", "description": "Result id from helmp_search"},
                "action": {"type": "string", "description": "Title of a secondary action from the result's `actions` (default: the primary action)"},
                "form": string_map
            }), &["query", "id"]),
            "annotations": annotations("Run a Helmp result", false, true, false, true),
        }
    ])
}

/// `helmp mcp`：读 stdin、写 stdout，直到客户端关闭 stdin。
pub fn serve(engine: Engine) -> Result<()> {
    // 本进程里执行的动作一律只回报；真正有副作用的操作交给图形界面经用户确认后执行。
    crate::effects::force_dry_run();
    crate::providers::clipboard::disable_capture();
    let server = Arc::new(Server::new(engine));
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let write = |out: &Mutex<std::io::Stdout>, value: &Value| {
        let mut out = out
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = writeln!(out, "{value}");
        let _ = out.flush();
    };
    let mut workers = Vec::new();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(err) => {
                let reply = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("Parse error: {err}")}});
                write(&out, &reply);
                continue;
            }
        };
        if let Value::Array(batch) = &message {
            let replies: Vec<Value> = batch.iter().filter_map(|m| server.handle(m)).collect();
            if !replies.is_empty() {
                write(&out, &Value::Array(replies));
            }
            continue;
        }
        if message.get("method").and_then(Value::as_str) == Some("tools/call") {
            let (server, out) = (Arc::clone(&server), Arc::clone(&out));
            workers.push(std::thread::spawn(move || {
                if let Some(reply) = server.handle(&message) {
                    write(&out, &reply);
                }
            }));
            workers.retain(|worker| !worker.is_finished());
            continue;
        }
        if let Some(reply) = server.handle(&message) {
            write(&out, &reply);
        }
    }
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server::new(Engine::new(Vec::new()))
    }

    #[test]
    fn negotiates_known_protocol_versions_and_falls_back_to_the_latest() {
        // 客户端要 2025-06-18 就回 2025-06-18；要一个不认识的（更新或更旧）回我们最新的，由客户端决定是否断开。
        let s = server();
        for version in ["2025-06-18", "2025-11-25"] {
            let reply = s
                .handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "claude-code", "version": "1"}}}))
                .unwrap();
            assert_eq!(reply["result"]["protocolVersion"], version);
        }
        let reply = s
            .handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {"protocolVersion": "2099-01-01"}}))
            .unwrap();
        assert_eq!(reply["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        assert_eq!(s.agent_name(), "Claude Code");
    }

    #[test]
    fn notifications_get_no_reply_and_unknown_methods_an_error() {
        let s = server();
        assert!(
            s.handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                .is_none()
        );
        let reply = s
            .handle(&json!({"jsonrpc": "2.0", "id": "x", "method": "nope"}))
            .unwrap();
        assert_eq!(reply["error"]["code"], -32601);
        assert_eq!(reply["id"], "x");
    }

    #[test]
    fn every_tool_has_a_schema_and_annotations() {
        // 客户端按 inputSchema 生成调用；缺了模型就不知道怎么传参。
        for tool in tools().as_array().unwrap() {
            assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
            assert!(tool["description"].as_str().unwrap().len() > 40, "{tool}");
            assert!(tool["annotations"]["readOnlyHint"].is_boolean(), "{tool}");
        }
    }

    #[test]
    fn agent_names_are_human_readable() {
        assert_eq!(agent_display_name("codex-mcp-client"), "Codex");
        assert_eq!(agent_display_name("cursor-vscode"), "Cursor");
        assert_eq!(agent_display_name(""), "Agent");
        assert_eq!(agent_display_name("zed"), "zed");
    }
}
