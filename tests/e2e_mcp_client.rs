//! MCP 客户端端到端测试：真实的 helmp 可执行文件 + 隔离环境（见 tests/support）+ 假 MCP 服务器。
//!
//! - stdio：examples/mock_mcp_server.rs 编译出的二进制，复制进隔离的数据目录再运行
//!   （演练模式只运行数据目录里的服务器，见 src/providers/mcp/manager.rs）。
//! - http：tests/support 的本地 MockServer 扮演 Streamable HTTP 服务器（只回 JSON）。
//! - 密钥走数据目录里的 `test-keychain.json`（测试模式不碰真实钥匙串）；导入只读隔离 HOME 里的夹具。

mod support;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use support::{Helmp, MockServer};

/// 编译好的假服务器（`cargo test` 会先编译 examples）。
fn mock_binary() -> PathBuf {
    let helmp = Path::new(env!("CARGO_BIN_EXE_helmp"));
    let path = helmp
        .parent()
        .expect("target dir")
        .join("examples")
        .join("mock_mcp_server");
    assert!(
        path.is_file(),
        "找不到 {}：先 cargo build --examples",
        path.display()
    );
    path
}

/// 把假服务器复制到 `<root>/<relative>`，返回绝对路径。
fn install_mock(h: &Helmp, relative: &str) -> PathBuf {
    let target = h.root.join(relative);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::copy(mock_binary(), &target).unwrap();
    target
}

/// 带一个 stdio 假服务器「demo」的环境。
fn with_demo() -> (Helmp, PathBuf) {
    let h = Helmp::new();
    let bin = install_mock(&h, "data/mock/mock_mcp_server");
    let command = bin.display().to_string();
    let h = h.config(move |c| {
        c["mcp"] = json!({"servers": [{
            "name": "demo",
            "transport": {"type": "stdio", "command": command, "env": {"MOCK_LEVEL": "1"}},
            "secrets": ["env:MOCK_TOKEN"]
        }]});
    });
    h.write(
        "data/test-keychain.json",
        &json!({"demo/env:MOCK_TOKEN": "tok-from-keychain"}).to_string(),
    );
    (h, bin)
}

#[test]
fn tools_show_up_in_search_and_run_with_the_form() {
    // 加了服务器，它的工具就是启动器命令：能搜到，表单参数按 schema 转成正确的类型传过去。
    let (h, _) = with_demo();
    let row = h.find("echo", |r| r.id == "mcp:demo/echo");
    assert_eq!(row.badge, "MCP");
    assert_eq!(row.title, "Echo Text");
    assert_eq!(row.subtitle, "Repeat the given text back");

    let out = h.run(
        &row.id,
        "echo",
        &["--form", "text=hi", "--form", "mood=sad", "--form", "loud=true", "--form", "times=2"],
    );
    assert!(out.contains("SAD: HI / SAD: HI"), "{out}");
    assert!(out.contains("\"times\": 2"), "整数要按整数传：{out}");
    assert!(out.contains("\"loud\": true"), "布尔要按布尔传：{out}");
    assert!(
        out.contains("\"token\": \"tok-from-keychain\""),
        "钥匙串里的密钥要作为环境变量传给服务器：{out}"
    );
    assert!(out.contains("Actions: Copy Result / Run Again"), "{out}");

    // 表单填错：不调用，直接说哪里不对。
    let bad = h.run(&row.id, "echo", &["--form", "text=hi", "--form", "times=two"]);
    assert!(bad.contains("times: must be an integer"), "{bad}");

    // 服务器名 + 空格：列出这个服务器的全部工具。
    let all = h.query("demo ");
    let ids: Vec<&str> = all
        .iter()
        .filter(|r| r.id.starts_with("mcp:demo/"))
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(ids.len(), 5, "{all:#?}");

    let list = h.cli(&["mcp-client", "list"]);
    assert!(list.contains("\tdemo\tuser\t5\t"), "{list}");
    let log = std::fs::read_to_string(h.data.join("mcp/logs/demo.log")).unwrap();
    assert!(log.contains("stderr mock server ready"), "stderr 要进日志：{log}");
    assert!(log.contains("protocol=2025-06-18"), "旧版本服务器也能握手：{log}");
    let config = std::fs::read_to_string(h.data.join("config.json")).unwrap();
    assert!(!config.contains("tok-from-keychain"));
}

#[test]
fn destructive_tools_confirm_before_running() {
    // 标了 destructiveHint 的工具：先出确认页，确认后才真的调用。
    let (h, _) = with_demo();
    let row = h.find("delete note", |r| r.id == "mcp:demo/delete_note");
    let first = h.run(&row.id, "delete note", &["--form", "name=groceries"]);
    assert!(first.contains("Run “Delete Note”?"), "{first}");
    assert!(first.contains("\"name\": \"groceries\""), "确认页要列出参数：{first}");
    assert!(first.contains("Actions: Run / Cancel"), "{first}");
    assert!(!first.contains("deleted groceries"), "确认前不能调用：{first}");

    let confirmed = h.run(
        &row.id,
        "delete note",
        &["--form", "name=groceries", "--then", "Run"],
    );
    assert!(confirmed.contains("deleted groceries"), "{confirmed}");
}

#[test]
fn errors_images_and_list_changed() {
    let (h, _) = with_demo();
    // isError：显示成错误，带上服务器给的原因。
    let fail = h.run("mcp:demo/fail", "fail", &[]);
    assert!(fail.contains("“fail” failed"), "{fail}");
    assert!(fail.contains("disk is full"), "{fail}");
    assert!(fail.contains("Copy Error"), "{fail}");

    // 图片：存进数据目录并显示。
    let picture = h.run("mcp:demo/picture", "picture", &[]);
    let path = picture
        .lines()
        .find_map(|line| line.strip_prefix("[Image ").and_then(|l| l.strip_suffix(']')))
        .unwrap_or_else(|| panic!("{picture}"));
    assert!(path.starts_with(&h.data.join("mcp/images").display().to_string()));
    assert!(std::fs::read(path).unwrap().starts_with(b"\x89PNG"));
    assert!(picture.contains("a dot"));

    // tools/list_changed：缓存作废，下次就能搜到新工具。
    assert!(h.query("bonus").iter().all(|r| r.id != "mcp:demo/bonus"));
    let added = h.run("mcp:demo/add_tool", "add tool", &[]);
    assert!(added.contains("bonus tool added"), "{added}");
    let row = h.find("bonus", |r| r.id == "mcp:demo/bonus");
    assert_eq!(row.title, "Bonus Tool");
}

#[test]
fn newest_protocol_and_cli_tools() {
    // 回 2025-11-25 的服务器照常工作；`helmp mcp-client tools` 列出工具并标出要确认的。
    let (h, _) = with_demo();
    let h = h.env("MOCK_PROTOCOL", "2025-11-25");
    let tools = h.cli(&["mcp-client", "tools", "demo"]);
    assert!(tools.contains("echo\tEcho Text\t-\tRepeat the given text back"), "{tools}");
    assert!(tools.contains("delete_note\tDelete Note\tconfirm\t"), "{tools}");
    let log = std::fs::read_to_string(h.data.join("mcp/logs/demo.log")).unwrap();
    assert!(log.contains("protocol=2025-11-25"), "{log}");
}

#[test]
fn real_servers_are_not_started_in_dry_run() {
    // 演练模式下不能真的去跑 npx、连外网：只回报。
    let h = Helmp::new().config(|c| {
        c["mcp"] = json!({"servers": [
            {"name": "npm", "transport": {"type": "stdio", "command": "npx", "args": ["-y", "@x/never"]}},
            {"name": "web", "transport": {"type": "http", "url": "https://mcp.example.com/mcp"}}
        ]});
    });
    assert!(h.query("echo").iter().all(|r| !r.id.starts_with("mcp:")));
    assert_eq!(
        h.cli(&["mcp-client", "tools", "npm"]).trim(),
        "[dry-run] mcp-start npm npx -y @x/never"
    );
    assert!(h.cli(&["mcp-client", "tools", "web"]).starts_with("[dry-run] mcp-start web"));
    assert!(!h.data.join("mcp/logs/npm.log").exists(), "根本没有启动");
}

#[test]
fn http_servers_get_headers_secrets_and_the_protocol_version() {
    // Streamable HTTP：密钥请求头从钥匙串取，握手后的请求带 MCP-Protocol-Version 和会话 id。
    let server = MockServer::start(|request| {
        let body: Value = serde_json::from_str(&request.body).unwrap_or(Value::Null);
        let id = body.get("id").cloned();
        let result = match body["method"].as_str().unwrap_or_default() {
            "initialize" => json!({"protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
                                   "serverInfo": {"name": "web-mock", "version": "1"}}),
            "tools/list" => json!({"tools": [{"name": "lookup", "title": "Look Up",
                                   "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}}]}),
            "tools/call" => json!({"content": [{"type": "text", "text": format!("found {}", body["params"]["arguments"]["q"].as_str().unwrap_or_default())}]}),
            _ => return (202, String::new()),
        };
        (200, json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string())
    });
    let url = format!("{}/mcp", server.url);
    let h = Helmp::new().config(move |c| {
        c["mcp"] = json!({"servers": [{
            "name": "web",
            "transport": {"type": "http", "url": url, "headers": {"X-Team": "core"}},
            "secrets": ["header:Authorization"]
        }]});
    });
    h.write(
        "data/test-keychain.json",
        &json!({"web/header:Authorization": "Bearer sekrit"}).to_string(),
    );
    let out = h.run("mcp:web/lookup", "look up", &["--form", "q=rust"]);
    assert!(out.contains("found rust"), "{out}");
    let requests = server.requests();
    let methods: Vec<String> = requests
        .iter()
        .map(|r| {
            serde_json::from_str::<Value>(&r.body).unwrap_or(Value::Null)["method"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert!(methods.contains(&"notifications/initialized".to_string()), "{methods:?}");
    let call = requests
        .iter()
        .find(|r| r.body.contains("tools/call"))
        .expect("tools/call sent");
    let header = |name: &str| {
        call.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(header("authorization").as_deref(), Some("Bearer sekrit"));
    assert_eq!(header("x-team").as_deref(), Some("core"));
    assert_eq!(header("mcp-protocol-version").as_deref(), Some("2025-11-25"));
    assert!(header("accept").unwrap_or_default().contains("text/event-stream"));
}

#[test]
fn import_reads_other_clients_without_touching_them() {
    // 从 Claude Code / Codex / Cursor 一键导入：只读那几个文件，密钥进钥匙串而不是 config.json。
    let mut h = Helmp::new();
    let claude = h.write(
        "home/.claude.json",
        &json!({"numStartups": 1, "mcpServers": {
            "github": {"type": "stdio", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"],
                       "env": {"GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_secret123"}},
            "legacy": {"type": "sse", "url": "https://old.example.com/sse"}
        }})
        .to_string(),
    );
    let codex = h.write(
        "codex/config.toml",
        "[mcp_servers.fetch]\ncommand = \"uvx\"\nargs = [\"mcp-server-fetch\"]\n",
    );
    let cursor = h.write(
        "home/.cursor/mcp.json",
        &json!({"mcpServers": {"context7": {"url": "https://mcp.context7.com/mcp",
                                            "headers": {"Authorization": "Bearer ctx-key"}}}})
        .to_string(),
    );
    let before: Vec<(Vec<u8>, std::time::SystemTime)> = [&claude, &codex, &cursor]
        .iter()
        .map(|p| {
            (
                std::fs::read(p).unwrap(),
                std::fs::metadata(p).unwrap().modified().unwrap(),
            )
        })
        .collect();

    let listed = h.cli(&["mcp-client", "import", "--list"]);
    assert!(listed.contains("Claude Code\tgithub\tnew\tnpx -y @modelcontextprotocol/server-github"), "{listed}");
    assert!(listed.contains("Claude Code\tlegacy\tunsupported:"), "{listed}");
    assert!(listed.contains("Codex\tfetch\tnew\tuvx mcp-server-fetch"), "{listed}");
    assert!(listed.contains("Cursor\tcontext7\tnew\thttps://mcp.context7.com/mcp"), "{listed}");

    let imported = h.cli(&["mcp-client", "import"]);
    h.keep_config();
    for name in ["github", "fetch", "context7"] {
        assert!(imported.contains(&format!("Added {name}")), "{imported}");
    }
    let config = std::fs::read_to_string(h.data.join("config.json")).unwrap();
    assert!(!config.contains("ghp_secret123") && !config.contains("ctx-key"), "{config}");
    assert!(config.contains("env:GITHUB_PERSONAL_ACCESS_TOKEN"), "{config}");
    let keychain: Value =
        serde_json::from_str(&std::fs::read_to_string(h.data.join("test-keychain.json")).unwrap())
            .unwrap();
    assert_eq!(keychain["github/env:GITHUB_PERSONAL_ACCESS_TOKEN"], "ghp_secret123");
    assert_eq!(keychain["context7/header:Authorization"], "Bearer ctx-key");

    // 再导入一次：都已存在，什么也不做。
    assert!(h.cli(&["mcp-client", "import"]).contains("No new servers"));
    let after: Vec<(Vec<u8>, std::time::SystemTime)> = [&claude, &codex, &cursor]
        .iter()
        .map(|p| {
            (
                std::fs::read(p).unwrap(),
                std::fs::metadata(p).unwrap().modified().unwrap(),
            )
        })
        .collect();
    assert_eq!(before, after, "别的客户端的配置文件一个字节都不能改");

    // 删除：配置和钥匙串里的密钥一起清掉。
    h.cli(&["mcp-client", "remove", "github"]);
    h.keep_config();
    let keychain = std::fs::read_to_string(h.data.join("test-keychain.json")).unwrap();
    assert!(!keychain.contains("ghp_secret123"), "{keychain}");
    assert!(!h.cli(&["mcp-client", "list"]).contains("\tgithub\t"));
}

#[test]
fn cli_add_accepts_commands_urls_and_pasted_json() {
    let mut h = Helmp::new();
    h.cli(&["mcp-client", "add", "time", "--", "uvx", "mcp-server-time"]);
    h.keep_config();
    h.cli(&["mcp-client", "add", "docs", "--url", "https://mcp.example.com/mcp", "--header", "X-Api-Key: k123"]);
    h.keep_config();
    h.cli(&["mcp-client", "add", "--json", r#"{"mcpServers": {"mem": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-memory"]}}}"#]);
    h.keep_config();
    let list = h.cli(&["mcp-client", "list"]);
    assert!(list.contains("\ttime\tuser\t-\tuvx mcp-server-time"), "{list}");
    assert!(list.contains("\tdocs\tuser\t-\thttps://mcp.example.com/mcp"), "{list}");
    assert!(list.contains("\tmem\tuser\t"), "{list}");
    let err = h.cli_err(&["mcp-client", "add", "time", "--", "uvx", "x"]);
    assert!(err.contains("already exists"), "{err}");
    let config = std::fs::read_to_string(h.data.join("config.json")).unwrap();
    assert!(!config.contains("k123"));
}

#[test]
fn plugin_bundled_servers_only_appear_after_enabling() {
    // 插件带的 MCP 服务器：插件没启用（没确认权限）时不启动、工具不出现；启用后才有。
    let manifest = json!({
        "schemaVersion": 1,
        "id": "com.example.mcp",
        "name": "Notes MCP",
        "version": "1.0.0",
        "author": "Tests",
        "mcpServers": {"notes": {"command": "./mock_mcp_server"}}
    })
    .to_string();
    let setup = |h: &Helmp| {
        h.write("data/plugins/com.example.mcp/plugin.json", &manifest);
        install_mock(h, "data/plugins/com.example.mcp/mock_mcp_server");
    };
    let off = Helmp::new();
    setup(&off);
    assert!(off.query("echo").iter().all(|r| !r.id.starts_with("mcp:")));
    let validated = off.cli(&["plugins", "validate", &off.data.join("plugins/com.example.mcp").display().to_string()]);
    assert!(validated.contains("Run a local program: ./mock_mcp_server"), "{validated}");

    let on = Helmp::new().config(|c| {
        c["plugins"]["com.example.mcp"] = json!({"enabled": true, "consent": ["process:./mock_mcp_server"]});
    });
    setup(&on);
    let row = on.find("echo", |r| r.id == "mcp:com.example.mcp/notes/echo");
    assert_eq!(row.title, "Echo Text");
    let out = on.run(&row.id, "echo", &["--form", "text=from plugin"]);
    assert!(out.contains("plain: from plugin"), "{out}");
    assert!(on.cli(&["mcp-client", "list"]).contains("\tcom.example.mcp/notes\tplugin:com.example.mcp\t"));
}
