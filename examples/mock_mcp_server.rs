//! 端到端测试用的假 MCP 服务器（stdio，换行分隔的 JSON-RPC）。tests/e2e_mcp_client.rs 把编译好的
//! 二进制复制进隔离的数据目录再运行它（演练模式只运行数据目录里的服务器）。
//!
//! 工具：
//! - `echo`：text（必填）、mood（enum）、loud（boolean）、times（integer）→ 文字 + structuredContent
//!   （里面带上环境变量 `MOCK_TOKEN`，验证钥匙串里的密钥确实传给了服务器）。
//! - `delete_note`：destructiveHint=true，name（必填）。
//! - `fail`：返回 isError。
//! - `picture`：返回一张 1×1 的 PNG。
//! - `add_tool`：加一个 `bonus` 工具（状态写进可执行文件旁的 mock-state），并发 `tools/list_changed`。
//!
//! 握手回的协议版本取 `MOCK_PROTOCOL`，缺省 `2025-06-18`（验证旧版本服务器也能用）。

use std::io::{BufRead as _, Write as _};

use serde_json::{Value, json};

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

fn state_file() -> std::path::PathBuf {
    let exe = std::env::current_exe().expect("exe");
    exe.with_file_name("mock-state")
}

fn tools() -> Vec<Value> {
    let mut tools = vec![
        json!({
            "name": "echo",
            "title": "Echo Text",
            "description": "Repeat the given text back",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "What to say"},
                    "mood": {"type": "string", "enum": ["happy", "sad"]},
                    "loud": {"type": "boolean"},
                    "times": {"type": "integer"}
                },
                "required": ["text"]
            },
            "annotations": {"readOnlyHint": true}
        }),
        json!({
            "name": "delete_note",
            "title": "Delete Note",
            "description": "Delete a note forever",
            "inputSchema": {
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"]
            },
            "annotations": {"destructiveHint": true}
        }),
        json!({
            "name": "fail",
            "description": "Always fails",
            "inputSchema": {"type": "object"}
        }),
        json!({
            "name": "picture",
            "description": "Draw a tiny picture",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "add_tool",
            "description": "Register the bonus tool",
            "inputSchema": {"type": "object"}
        }),
    ];
    if state_file().exists() {
        tools.push(json!({
            "name": "bonus",
            "title": "Bonus Tool",
            "description": "Appears after add_tool",
            "inputSchema": {"type": "object"}
        }));
    }
    tools
}

fn text(text: impl Into<String>) -> Value {
    json!({"type": "text", "text": text.into()})
}

fn call(name: &str, args: &Value, out: &mut impl std::io::Write) -> Value {
    match name {
        "echo" => {
            let said = args["text"].as_str().unwrap_or_default();
            let mood = args.get("mood").and_then(Value::as_str).unwrap_or("plain");
            let loud = args.get("loud").and_then(Value::as_bool).unwrap_or(false);
            let times = args.get("times").and_then(Value::as_i64).unwrap_or(1);
            let mut line = format!("{mood}: {said}");
            if loud {
                line = line.to_uppercase();
            }
            let line = vec![line; usize::try_from(times.max(1)).unwrap_or(1)].join(" / ");
            json!({
                "content": [text(line)],
                "structuredContent": {"said": said, "mood": mood, "loud": loud, "times": times,
                                      "token": std::env::var("MOCK_TOKEN").unwrap_or_default()}
            })
        }
        "delete_note" => json!({"content": [text(format!("deleted {}", args["name"].as_str().unwrap_or_default()))]}),
        "fail" => json!({"isError": true, "content": [text("disk is full")]}),
        "picture" => json!({"content": [{"type": "image", "data": PNG, "mimeType": "image/png"}, text("a dot")]}),
        "add_tool" => {
            std::fs::write(state_file(), "bonus").expect("state");
            let note = json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"});
            writeln!(out, "{note}").expect("write");
            json!({"content": [text("bonus tool added")]})
        }
        "bonus" => json!({"content": [text("bonus!")]}),
        _ => json!({"isError": true, "content": [text(format!("unknown tool {name}"))]}),
    }
}

fn main() {
    let protocol = std::env::var("MOCK_PROTOCOL").unwrap_or_else(|_| "2025-06-18".into());
    eprintln!("mock server ready");
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = message.get("id").cloned() else {
            continue; // 通知
        };
        let method = message["method"].as_str().unwrap_or_default();
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": protocol,
                "capabilities": {"tools": {"listChanged": true}},
                "serverInfo": {"name": "mock", "version": "1.0.0"}
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => {
                let name = message["params"]["name"].as_str().unwrap_or_default().to_string();
                eprintln!("call {name}");
                Ok(call(&name, &message["params"]["arguments"], &mut stdout))
            }
            other => Err(json!({"code": -32601, "message": format!("Method not found: {other}")})),
        };
        let reply = match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
        };
        writeln!(stdout, "{reply}").expect("write");
        stdout.flush().expect("flush");
    }
}
