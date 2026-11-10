//! `helmp mcp`（给 coding agent 用的 MCP 服务）端到端测试：真实的 helmp 可执行文件 + 隔离环境（见 tests/support），
//! 在 stdio 上说 JSON-RPC。演练模式（HELMP_DRY_RUN=1）下不走图形界面的确认通道，也不打开任何窗口；
//! agent 配置（~/.claude.json、~/.codex、~/.cursor）都写在隔离的 HOME / CLAUDE_CONFIG_DIR / CODEX_HOME 里。

mod support;

use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Child, ChildStdin};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};
use support::{Helmp, MockServer};

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    next: u64,
}

impl Mcp {
    fn start(h: &Helmp) -> Self {
        let mut child = h.spawn(&["mcp"]);
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut mcp = Self {
            child,
            stdin,
            lines,
            next: 0,
        };
        let init = mcp.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "claude-code", "version": "test"}}),
        );
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18", "{init}");
        assert_eq!(init["result"]["serverInfo"]["name"], "helmp");
        mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        mcp
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").expect("write to helmp mcp");
        self.stdin.flush().expect("flush");
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(90))
                .unwrap_or_else(|_| panic!("helmp mcp didn't answer {method}"));
            let message: Value = serde_json::from_str(&line)
                .unwrap_or_else(|err| panic!("stdout 只能有协议消息：{line:?} ({err})"));
            if message["id"] == json!(id) {
                return message;
            }
        }
    }

    /// 调一个工具，返回 result（含 content / structuredContent / isError）。
    fn call(&mut self, tool: &str, args: Value) -> Value {
        let reply = self.request("tools/call", json!({"name": tool, "arguments": args}));
        assert!(reply.get("error").is_none(), "{tool}: {reply}");
        reply["result"].clone()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn speaks_mcp_and_lists_tools_resources_and_prompts() {
    let h = Helmp::new();
    let mut mcp = Mcp::start(&h);
    assert!(mcp.request("ping", json!({}))["result"].is_object());

    let tools = mcp.request("tools/list", json!({}));
    let tools = tools["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for name in [
        "helmp_plugin_guide",
        "helmp_plugin_scaffold",
        "helmp_plugin_validate",
        "helmp_plugin_install",
        "helmp_plugin_test",
        "helmp_plugin_list",
        "helmp_plugin_submit",
        "helmp_search",
        "helmp_run",
    ] {
        let tool = tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("missing {name} in {names:?}"));
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
    }
    // 发布工具的说明必须要求先问用户。
    let submit = tools
        .iter()
        .find(|t| t["name"] == "helmp_plugin_submit")
        .unwrap();
    assert!(
        submit["description"]
            .as_str()
            .unwrap()
            .contains("MUST ask the user")
    );
    assert_eq!(
        submit["inputSchema"]["required"],
        json!(["dir", "user_confirmed"])
    );

    let guide = mcp.call("helmp_plugin_guide", json!({}));
    let guide = text(&guide);
    assert!(
        guide.contains("## Workflow")
            && guide.contains("plugin.json")
            && guide.contains("dev.helmp.flomo"),
        "{}",
        &guide[..300]
    );

    let resources = mcp.request("resources/list", json!({}));
    assert!(
        resources["result"]["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["uri"] == "helmp://docs/plugins")
    );
    let doc = mcp.request(
        "resources/read",
        json!({"uri": "helmp://docs/plugins.zh-CN"}),
    );
    assert!(
        doc["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("清单字段参考")
    );
    let missing = mcp.request("resources/read", json!({"uri": "helmp://nope"}));
    assert!(missing["error"].is_object());

    let prompt = mcp.request("prompts/get", json!({"name": "create_helmp_plugin", "arguments": {"request": "把选中文字发到飞书机器人"}}));
    let prompt = prompt["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        prompt.contains("把选中文字发到飞书机器人") && prompt.contains("user_confirmed"),
        "{prompt}"
    );
}

#[test]
fn scaffold_validate_install_keeps_the_plugin_disabled() {
    // agent 装的插件不能自己生效：要用户在设置里看过权限、手动启用。
    let h = Helmp::new();
    let mut mcp = Mcp::start(&h);
    let dir = h.root.join("dev/feishu");
    let made = mcp.call(
        "helmp_plugin_scaffold",
        json!({"id": "com.example.feishu", "name": "Feishu Bot", "kind": "http", "dir": dir}),
    );
    assert_eq!(made["structuredContent"]["valid"], true, "{made}");
    assert!(dir.join("plugin.json").is_file());

    let checked = mcp.call("helmp_plugin_validate", json!({"dir": dir}));
    assert_eq!(checked["structuredContent"]["valid"], true, "{checked}");
    assert!(text(&checked).contains("example.com"), "{checked}");

    // 写坏后校验要给出带字段路径的错误。
    let broken = h.root.join("dev/broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(
        broken.join("plugin.json"),
        r#"{"schemaVersion":1,"id":"Bad","name":"B","version":"1","author":"a","commands":[]}"#,
    )
    .unwrap();
    let bad = mcp.call("helmp_plugin_validate", json!({"dir": broken}));
    assert_eq!(bad["structuredContent"]["valid"], false, "{bad}");
    assert!(
        bad["structuredContent"]["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e.as_str().unwrap().starts_with("id")),
        "{bad}"
    );
    assert_eq!(
        mcp.call("helmp_plugin_install", json!({"dir": broken}))["structuredContent"]["installed"],
        false
    );

    let installed = mcp.call("helmp_plugin_install", json!({"dir": dir}));
    assert_eq!(installed["isError"], false, "{installed}");
    assert_eq!(
        installed["structuredContent"]["state"], "disabled",
        "{installed}"
    );
    assert!(
        text(&installed).contains("Settings › Plugins"),
        "{installed}"
    );
    assert!(
        h.data
            .join("plugins/com.example.feishu/plugin.json")
            .is_file()
    );
    // 再装一次（更新）仍然停用，不会叠出第二份。
    mcp.call("helmp_plugin_install", json!({"dir": dir}));
    assert_eq!(
        std::fs::read_dir(h.data.join("plugins")).unwrap().count(),
        1
    );

    let list = mcp.call("helmp_plugin_list", json!({}));
    let plugins = list["structuredContent"]["plugins"].as_array().unwrap();
    let ours = plugins
        .iter()
        .find(|p| p["id"] == "com.example.feishu")
        .expect("installed plugin listed");
    assert_eq!(ours["state"], "disabled");
    assert_eq!(ours["source"], "user");
    assert!(
        plugins
            .iter()
            .any(|p| p["id"] == "dev.helmp.flomo" && p["state"] == "enabled")
    );
    // 没有替用户写下任何授权。
    let config = std::fs::read_to_string(h.data.join("config.json")).unwrap();
    assert!(!config.contains("com.example.feishu"), "{config}");
    assert!(h.cli(&["plugins"]).contains("disabled\tcom.example.feishu"));
}

#[test]
fn plugin_test_reports_dry_run_lines_and_hits_local_mocks() {
    let h = Helmp::new();
    let mut mcp = Mcp::start(&h);

    // 快捷链接：只回报要打开的地址。
    let ql = h.root.join("dev/ql");
    mcp.call(
        "helmp_plugin_scaffold",
        json!({"id": "com.example.ql", "name": "QL", "kind": "quicklink", "dir": ql}),
    );
    let out = mcp.call(
        "helmp_plugin_test",
        json!({"plugin": ql, "command": "search", "input": "rust & go"}),
    );
    assert_eq!(out["isError"], false, "{out}");
    assert!(
        text(&out).contains("[dry-run] open https://example.com/search?q=rust%20%26%20go"),
        "{out}"
    );

    // http：表单内容作为 JSON 发到本机假服务（secret 偏好设置只经 HELMP_TEST_SECRETS 传入沙箱）。
    let server = MockServer::start(|_| (200, json!({"ok": true}).to_string()));
    let http = h.root.join("dev/http");
    mcp.call(
        "helmp_plugin_scaffold",
        json!({"id": "com.example.hook", "name": "Hook", "kind": "http", "dir": http}),
    );
    let out = mcp.call(
        "helmp_plugin_test",
        json!({"plugin": http, "command": "send", "form": {"content": "hello #x\nline 2"}, "secrets": {"webhook": format!("{}/hook", server.url)}}),
    );
    assert_eq!(out["isError"], false, "{out}");
    assert!(text(&out).contains("Sent"), "{out}");
    let requests = server.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/hook");
    let body: Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(body, json!({"text": "hello #x\nline 2"}));

    // 没给 webhook：失败并说明原因，而不是假装成功。
    let out = mcp.call(
        "helmp_plugin_test",
        json!({"plugin": http, "command": "send", "form": {"content": "x"}}),
    );
    assert!(!text(&out).contains("Sent"), "{out}");

    // 列表命令：返回脚本输出的各项。
    let list = h.root.join("dev/list");
    mcp.call(
        "helmp_plugin_scaffold",
        json!({"id": "com.example.lst", "name": "Lst", "kind": "list", "dir": list}),
    );
    let out = mcp.call(
        "helmp_plugin_test",
        json!({"plugin": list, "command": "list", "input": "abc"}),
    );
    assert_eq!(out["structuredContent"]["items"][0]["uid"], "echo", "{out}");
    let out = mcp.call(
        "helmp_plugin_test",
        json!({"plugin": list, "command": "list", "input": "abc", "item": "echo"}),
    );
    assert!(text(&out).contains("[dry-run] copy abc"), "{out}");

    // 不存在的命令列出可选命令。
    let out = mcp.call(
        "helmp_plugin_test",
        json!({"plugin": ql, "command": "nope"}),
    );
    assert_eq!(out["isError"], true);
    assert!(text(&out).contains("search"), "{out}");
}

#[test]
fn search_and_run_respect_privacy_and_side_effects() {
    let h = Helmp::new();
    let mut mcp = Mcp::start(&h);

    let found = mcp.call("helmp_search", json!({"query": "12*7"}));
    let results = found["structuredContent"]["results"]
        .as_array()
        .unwrap()
        .clone();
    let calc = results
        .iter()
        .find(|r| r["id"] == "calculator:result")
        .unwrap_or_else(|| panic!("{found}"));
    assert_eq!(calc["title"], "84");
    assert_eq!(calc["needs_approval"], false);

    // 纯计算：直接拿到结果，什么也不复制。
    let ran = mcp.call(
        "helmp_run",
        json!({"query": "12*7", "id": "calculator:result"}),
    );
    assert_eq!(ran["structuredContent"]["decision"], "not_needed", "{ran}");
    assert!(text(&ran).contains("value: 84"), "{ran}");

    // 剪贴板相关结果默认不给 agent（命令行里能看到同一个查询的剪贴板结果）。
    assert!(
        h.query("clipboard history")
            .iter()
            .any(|r| r.id.starts_with("clipboard:"))
    );
    let found = mcp.call("helmp_search", json!({"query": "clipboard history"}));
    let results = found["structuredContent"]["results"].as_array().unwrap();
    assert!(
        results
            .iter()
            .all(|r| !r["id"].as_str().unwrap().starts_with("clipboard:")),
        "{found}"
    );
    assert!(
        found["structuredContent"]["hidden_private"]
            .as_u64()
            .unwrap()
            >= 1,
        "{found}"
    );
    let refused = mcp.call(
        "helmp_run",
        json!({"query": "clipboard history", "id": "clipboard:open"}),
    );
    assert_eq!(refused["isError"], true, "{refused}");

    // 有副作用的结果：演练模式下只回报，不走确认通道。
    let found = mcp.call("helmp_search", json!({"query": "npm react"}));
    let npm = found["structuredContent"]["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "plugins:dev.helmp.quicklinks/npm")
        .cloned()
        .unwrap_or_else(|| panic!("{found}"));
    assert_eq!(npm["needs_approval"], true);
    // 要输入内容的插件命令：用户在面板里要 Tab 进入范围才能输入，agent 也一样——搜索结果带 scope，
    // 直接按全局搜索的查询运行要明确报错并说清楚怎么做，而不是假装成功（那只是「进入范围」）。
    let scope = "plugins:dev.helmp.quicklinks/npm";
    assert_eq!(npm["scope"], scope, "{npm}");
    let entry = mcp.call("helmp_run", json!({"query": "npm react", "id": scope}));
    assert_eq!(entry["isError"], true, "{entry}");
    assert!(text(&entry).contains(&format!("scope={scope:?}")), "{entry}");
    // 在范围里搜、在范围里运行：query 就是要搜的内容。
    let inside = mcp.call("helmp_search", json!({"query": "react", "scope": scope}));
    let ready = &inside["structuredContent"]["results"][0];
    assert_eq!(ready["id"], scope, "{inside}");
    assert_eq!(ready["subtitle"], "https://www.npmjs.com/search?q=react", "{inside}");
    assert!(ready["scope"].is_null(), "范围里的结果不再是入口：{inside}");
    let ran = mcp.call(
        "helmp_run",
        json!({"query": "react", "scope": scope, "id": scope}),
    );
    assert!(
        text(&ran).contains("[dry-run] open https://www.npmjs.com/search?q=react"),
        "{ran}"
    );

    let unknown = mcp.call("helmp_run", json!({"query": "12*7", "id": "nope:x"}));
    assert_eq!(unknown["isError"], true);
}

#[test]
fn private_results_show_up_only_when_the_user_opts_in() {
    let h = Helmp::new().config(|c| c["mcp_server"] = json!({"expose_private": true}));
    let mut mcp = Mcp::start(&h);
    let found = mcp.call("helmp_search", json!({"query": "clipboard history"}));
    assert!(
        found["structuredContent"]["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"].as_str().unwrap().starts_with("clipboard:")),
        "{found}"
    );
}

#[test]
fn submit_is_refused_without_the_users_explicit_consent() {
    let h = Helmp::new();
    let mut mcp = Mcp::start(&h);
    let dir = h.root.join("dev/ql");
    mcp.call(
        "helmp_plugin_scaffold",
        json!({"id": "com.example.ql", "name": "QL", "kind": "quicklink", "dir": dir}),
    );
    let out = mcp.call(
        "helmp_plugin_submit",
        json!({"dir": dir, "user_confirmed": false}),
    );
    assert_eq!(out["isError"], true, "{out}");
    assert_eq!(out["structuredContent"]["submitted"], false);
    assert!(text(&out).contains("Ask the user"), "{out}");
}

fn helmp_exe() -> String {
    std::path::Path::new(env!("CARGO_BIN_EXE_helmp"))
        .canonicalize()
        .unwrap()
        .display()
        .to_string()
}

#[test]
fn mcp_install_writes_agent_configs_idempotently_with_backups() {
    // 只写隔离 HOME 里的配置；HELMP_CLAUDE_CLI 为空 = 不调用真实的 claude 命令，直接改文件。
    let h = Helmp::new().env("HELMP_CLAUDE_CLI", "");
    let claude = h.root.join("claude/.claude.json");
    std::fs::write(
        &claude,
        r#"{"numStartups": 7, "mcpServers": {"other": {"command": "x"}}}"#,
    )
    .unwrap();
    let codex = h.root.join("codex/config.toml");
    std::fs::write(&codex, "# mine\nmodel = \"o3\"\n").unwrap();
    let exe = helmp_exe();

    let out = h.cli(&["mcp", "install", "claude"]);
    assert!(out.contains("Connected Helmp to Claude Code"), "{out}");
    let config: Value = serde_json::from_str(&std::fs::read_to_string(&claude).unwrap()).unwrap();
    assert_eq!(config["numStartups"], 7);
    assert_eq!(config["mcpServers"]["other"]["command"], "x");
    let command = config["mcpServers"]["helmp"]["command"].as_str().unwrap();
    assert_eq!(
        std::path::Path::new(command)
            .canonicalize()
            .unwrap()
            .display()
            .to_string(),
        exe
    );
    assert_eq!(config["mcpServers"]["helmp"]["args"], json!(["mcp"]));
    let backup = h.root.join("claude/.claude.json.helmp-backup");
    assert!(
        std::fs::read_to_string(&backup)
            .unwrap()
            .contains("\"numStartups\": 7")
    );

    // 第二次：已连接，不改文件。
    let before = std::fs::read_to_string(&claude).unwrap();
    std::fs::remove_file(&backup).unwrap();
    assert!(
        h.cli(&["mcp", "install", "claude"])
            .contains("already connected")
    );
    assert_eq!(std::fs::read_to_string(&claude).unwrap(), before);
    assert!(!backup.exists(), "没有改动就不该再备份");

    h.cli(&["mcp", "install", "codex"]);
    let toml = std::fs::read_to_string(&codex).unwrap();
    assert!(toml.starts_with("# mine\nmodel = \"o3\"\n"), "{toml}");
    assert!(
        toml.contains("[mcp_servers.helmp]\ncommand = ") && toml.contains("args = [\"mcp\"]"),
        "{toml}"
    );
    assert!(h.root.join("codex/config.toml.helmp-backup").is_file());
    assert!(
        h.cli(&["mcp", "install", "codex"])
            .contains("already connected")
    );
    assert_eq!(std::fs::read_to_string(&codex).unwrap(), toml);

    h.cli(&["mcp", "install", "cursor"]);
    let cursor: Value =
        serde_json::from_str(&std::fs::read_to_string(h.home.join(".cursor/mcp.json")).unwrap())
            .unwrap();
    assert_eq!(cursor["mcpServers"]["helmp"]["args"], json!(["mcp"]));
    assert!(
        h.cli(&["mcp", "install", "cursor"])
            .contains("already connected")
    );

    let status = h.cli(&["mcp", "status"]);
    assert_eq!(status.matches("\tConnected\t").count(), 3, "{status}");
    assert!(h.cli_err(&["mcp", "install", "vim"]).contains("Usage"));
}

#[test]
fn mcp_install_uses_the_claude_cli_when_present() {
    // 有 claude 命令时交给它（`claude mcp add --scope user`），不直接改文件。这里用一个记录参数的假命令。
    let h = Helmp::new();
    let log = h.root.join("claude-args.log");
    let fake = h.write(
        "bin/claude",
        &format!("#!/bin/sh\necho \"$@\" >> '{}'\n", log.display()),
    );
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let h = h.env("HELMP_CLAUDE_CLI", fake.display().to_string());
    let out = h.cli(&["mcp", "install", "claude"]);
    assert!(out.contains("claude mcp add"), "{out}");
    let calls = std::fs::read_to_string(&log).unwrap();
    let add = calls
        .lines()
        .find(|l| l.starts_with("mcp add"))
        .unwrap_or_else(|| panic!("{calls}"));
    assert!(add.starts_with("mcp add --scope user helmp -- "), "{add}");
    assert!(add.ends_with(" mcp"), "{add}");
    assert!(
        !h.root.join("claude/.claude.json").exists(),
        "用命令登记时不直接改文件"
    );
}

#[test]
fn side_effects_are_sent_to_the_running_helmp_for_approval() {
    // 不在演练模式时，有副作用的操作交给图形界面经用户确认。这里用一个假的「Helmp 界面」监听数据目录里的套接字，
    // 检查请求内容和对拒绝 / 允许的处理；Helmp 没在运行时要明确告诉 agent。
    use std::os::unix::net::UnixListener;
    let h = Helmp::new().env("HELMP_DRY_RUN", "0");
    let mut mcp = Mcp::start(&h);
    let npm = "plugins:dev.helmp.quicklinks/npm";
    let args = json!({"query": "react", "scope": npm, "id": npm});
    let out = mcp.call("helmp_run", args.clone());
    assert_eq!(out["isError"], true, "{out}");
    assert!(text(&out).contains("isn't running"), "{out}");

    let listener = UnixListener::bind(h.data.join("agent.sock")).expect("bind fake helmp socket");
    let (tx, seen) = mpsc::channel();
    std::thread::spawn(move || {
        let replies = [
            json!({"ok": true, "decision": "denied"}),
            json!({"ok": true, "decision": "allowed", "failed": false, "output": "opened npm"}),
        ];
        for reply in replies {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            tx.send(serde_json::from_str::<Value>(&line).unwrap())
                .unwrap();
            writeln!(stream, "{reply}").unwrap();
        }
    });

    let out = mcp.call("helmp_run", args.clone());
    assert_eq!(out["isError"], true, "{out}");
    assert!(text(&out).contains("denied"), "{out}");
    let request = seen.recv().unwrap();
    assert_eq!(request["type"], "run");
    assert_eq!(request["agent"], "Claude Code");
    assert_eq!(request["id"], npm);
    // 界面要在同一个命令范围里重查（与 agent 看到的一致），否则「react」在全局搜索里找不到 npm。
    assert_eq!(request["scope"], npm);
    assert_eq!(request["query"], "react");

    let out = mcp.call("helmp_run", args);
    assert_eq!(out["isError"], false, "{out}");
    assert!(text(&out).contains("opened npm"), "{out}");
    assert_eq!(out["structuredContent"]["decision"], "allowed");
}
