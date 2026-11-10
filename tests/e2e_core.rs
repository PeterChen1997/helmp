//! 核心功能端到端测试：真实的 helmp 可执行文件 + 隔离环境（见 tests/support）。
//! 每个用例写明「为什么这条链路重要」，业务行为变了测试就该失败。

mod support;

use serde_json::json;
use support::{Helmp, MockServer, repo_file};

#[test]
fn pinyin_initials_find_and_open_an_app() {
    // 中文用户最常用的路径：打拼音首字母找到 App 并打开。找不到或打开错，启动器就没法用。
    let h = Helmp::new();
    let app = h.find("jsq", |r| r.id.ends_with("/Calculator.app"));
    assert_eq!(app.badge, "Applications");
    assert_eq!(
        h.run(&app.id, "jsq", &[]).trim(),
        "[dry-run] open /System/Applications/Calculator.app"
    );
}

#[test]
fn calculator_answers_inline_and_copies() {
    // 算式要直接出结果，回车复制；运算优先级错了就是给用户错误答案。
    let h = Helmp::new();
    let row = h.find("2+3*4", |r| r.id == "calculator:result");
    assert_eq!(row.title, "14");
    let out = h.run("calculator:result", "2+3*4", &["--action", "Copy Expression and Result"]);
    assert!(out.contains("[dry-run] copy 2+3*4 = 14"), "{out}");
}

#[test]
fn unknown_text_falls_back_to_web_search() {
    // 什么都没匹配时要能一键搜网页，链接必须正确转义。
    let h = Helmp::new();
    h.find("rust lang", |r| r.id == "websearch:google");
    let out = h.run("websearch:google", "rust lang", &[]);
    assert_eq!(out.trim(), "[dry-run] open https://www.google.com/search?q=rust%20lang");
}

#[test]
fn dangerous_system_commands_confirm_first() {
    // 重启这类不可撤销的操作：第一次回车只能出确认页，确认后才执行。
    let h = Helmp::new();
    h.find("restart", |r| r.id == "system:restart");
    let first = h.run("system:restart", "restart", &[]);
    assert!(first.contains("Restart now?"), "{first}");
    assert!(!first.contains("Would run"), "确认前不能执行：{first}");
    let confirmed = h.run("system:restart", "restart", &["--then", "Restart"]);
    assert!(confirmed.contains("Would run: osascript"), "{confirmed}");
}

#[test]
fn translate_without_model_points_to_settings() {
    // 没配模型时不能报一串错，要引导去设置。
    let h = Helmp::new();
    let row = h.find("tr hello", |r| r.id == "translate:setup");
    assert!(row.subtitle.contains("Settings › Models"), "{row:?}");
}

#[test]
fn translate_calls_the_configured_model_and_offers_actions() {
    // 用户自己的模型：请求要发到配置的地址，带上原文；译文能复制、能替换选中文字。
    let server = MockServer::start(|req| {
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
        let text = body["messages"]
            .as_array()
            .and_then(|m| m.last())
            .and_then(|m| m["content"].as_str())
            .unwrap_or_default()
            .to_string();
        let reply = if text == "good morning" { "早上好" } else { "?" };
        (200, json!({"message": {"role": "assistant", "content": reply}, "done": true}).to_string())
    });
    let url = server.url.clone();
    let h = Helmp::new().config(move |c| {
        c["models"] = json!([{"id": "m1", "name": "Mock", "kind": "ollama", "base_url": url, "model": "mock"}]);
    });
    let row = h.find("tr good morning", |r| r.id.starts_with("translate:"));
    let out = h.run(&row.id, "tr good morning", &["--form", "text=good morning"]);
    assert!(out.contains("早上好"), "{out}");
    assert!(out.contains("Mock · English → Simplified Chinese"), "{out}");
    let requests = server.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(requests[0].path.starts_with("/api/chat"), "{:?}", requests[0]);
}

#[test]
fn ocr_reads_text_and_qr_from_an_image_file() {
    // 截图识别的核心是 Vision 识别：中英混排要按行读出，图里的二维码链接要读出来。
    let h = Helmp::new();
    let image = repo_file("tests/fixtures/ocr-qr.png");
    let query = format!("ocr {}", image.display());
    let row = h.find(&query, |r| r.id.starts_with("screenshot:ocr-file:"));
    let out = h.run(&row.id, &query, &[]);
    assert!(out.contains("QR Code"), "{out}");
    assert!(out.contains("https://"), "{out}");
}

#[test]
fn script_commands_run_with_arguments() {
    // Raycast 兼容脚本：放进脚本目录就出现在搜索里，参数作为表单传给脚本，输出展示出来。
    let h = Helmp::new();
    let script = h.write(
        "scripts/greet.sh",
        "#!/bin/bash\n# @raycast.schemaVersion 1\n# @raycast.title Greet Someone\n# @raycast.mode fullOutput\n# @raycast.argument1 { \"type\": \"text\", \"placeholder\": \"name\" }\necho \"hello, $1\"\n",
    );
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let row = h.find("greet", |r| r.title == "Greet Someone");
    let out = h.run(&row.id, "greet", &["--form", "argument1=Ada"]);
    assert!(out.contains("hello, Ada"), "{out}");
}

#[test]
fn unknown_cli_action_is_reported_not_guessed() {
    // 端到端测试依赖 --action 精确命中；拼错时要列出可选项而不是悄悄执行主操作。
    let h = Helmp::new();
    let err = h.cli_err(&["run", "calculator:result", "1+2", "--action", "Nope"]);
    assert!(err.contains("Copy Expression and Result"), "{err}");
}

#[test]
fn many_plugins_do_not_push_the_obvious_app_down() {
    // 插件越来越多，最常见的「拼音首字母找 App」不能被别的插件挤掉：
    // jsq 时「取色」也能靠「拾色器」的冷僻读音命中，但第一条必须是计算器。
    let h = Helmp::new();
    let rows = h.query("jsq");
    let first = rows.first().expect("有结果");
    assert!(first.id.ends_with("/Calculator.app"), "{rows:#?}");
}

#[test]
fn plugin_commands_do_not_fuzzy_match_unrelated_queries() {
    // 插件命令名较长，字母按顺序出现就算命中的话，uuid 会冒出「Obsidian 快速记录」这种无关结果。
    let h = Helmp::new();
    let rows = h.query("uuid");
    assert!(rows.first().is_some_and(|r| r.title.len() == 36), "第一条应是生成的 UUID：{rows:#?}");
    assert!(!rows.iter().any(|r| r.title.contains("Obsidian")), "{rows:#?}");
}
