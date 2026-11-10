//! 清单插件端到端测试：真实的 helmp 可执行文件 + 隔离环境（见 tests/support）。
//!
//! 密钥一律走 `HELMP_TEST_SECRETS`（只在隔离数据目录 + HELMP_DRY_RUN=1 时生效），不碰真实钥匙串；
//! http 只发往本机假服务（MockServer），发往真实主机的请求在演练模式下只回报不发送。

mod support;

use std::os::unix::fs::PermissionsExt as _;

use serde_json::{Value, json};
use support::{Helmp, MockServer, repo_file};

/// 写一个用户插件（清单 + 可选的脚本），返回插件目录。
fn user_plugin(
    h: &Helmp,
    id: &str,
    manifest: &Value,
    scripts: &[(&str, &str)],
) -> std::path::PathBuf {
    let dir = format!("data/plugins/{id}");
    h.write(&format!("{dir}/plugin.json"), &manifest.to_string());
    for (path, body) in scripts {
        let path = h.write(&format!("{dir}/{path}"), body);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    h.root.join(dir)
}

fn enable(id: &str, consent: &[&str]) -> impl FnOnce(&mut Value) {
    let (id, consent) = (id.to_string(), json!(consent));
    move |c: &mut Value| {
        c["plugins"][id] = json!({"enabled": true, "consent": consent});
    }
}

fn quicklink_manifest() -> Value {
    json!({
        "schemaVersion": 1,
        "id": "com.example.hello",
        "name": "Hello",
        "version": "0.1.0",
        "author": "Tests",
        "commands": [{
            "id": "search", "title": "Hello Search", "keywords": ["hellox"],
            "argument": {"required": true},
            "action": {"type": "open", "url": "https://example.com/?q={{query|url}}"}
        }]
    })
}

#[test]
fn user_plugin_shows_up_only_after_the_user_enables_it() {
    // 放进插件目录的东西不能自己就生效：用户要先看过权限、点了启用。
    let h = Helmp::new();
    user_plugin(&h, "com.example.hello", &quicklink_manifest(), &[]);
    assert!(
        !h.query("hellox rust")
            .iter()
            .any(|r| r.id.starts_with("plugins:com.example.hello")),
        "未启用的用户插件不能出现在搜索里"
    );
    let list = h.cli(&["plugins"]);
    assert!(
        list.contains("disabled\tcom.example.hello\t0.1.0\tuser\tHello"),
        "{list}"
    );

    // 启用但确认的权限和清单对不上（比如插件更新后多要了权限）：仍不生效，状态是待确认。
    let stale = Helmp::new().config(enable("com.example.hello", &["open:obsidian"]));
    user_plugin(&stale, "com.example.hello", &quicklink_manifest(), &[]);
    assert!(
        !stale
            .query("hellox rust")
            .iter()
            .any(|r| r.id.starts_with("plugins:com.example.hello"))
    );
    assert!(
        stale
            .cli(&["plugins"])
            .contains("needs-consent\tcom.example.hello")
    );

    let on = Helmp::new().config(enable("com.example.hello", &["open:https"]));
    user_plugin(&on, "com.example.hello", &quicklink_manifest(), &[]);
    let row = on.find("hellox rust & go", |r| {
        r.id == "plugins:com.example.hello/search"
    });
    assert_eq!(row.title, "Hello Search");
    let out = on.run(&row.id, "hellox rust & go", &[]);
    assert_eq!(
        out.trim(),
        "[dry-run] open https://example.com/?q=rust%20%26%20go"
    );
}

#[test]
fn broken_user_plugins_are_listed_with_the_reason() {
    // 写坏的插件不能悄悄消失：`helmp plugins` 要列出它和出错原因。
    let h = Helmp::new();
    h.write(
        "data/plugins/com.example.broken/plugin.json",
        r#"{"schemaVersion": 1, "id": "com.example.broken""#,
    );
    let list = h.cli(&["plugins"]);
    assert!(list.contains("error\tcom.example.broken"), "{list}");
    assert!(list.contains("plugin.json line"), "{list}");
    // 自带插件默认启用。
    assert!(
        list.contains("enabled\tdev.helmp.flomo\t1.0.0\tbundled\tflomo"),
        "{list}"
    );
}

fn flomo(server: &MockServer) -> Helmp {
    let secrets = json!({"dev.helmp.flomo/webhook": format!("{}/iwh/tok/sec/", server.url)});
    Helmp::new().env("HELMP_TEST_SECRETS", secrets.to_string())
}

#[test]
fn flomo_posts_the_memo_as_json_and_reports_success() {
    // flomo 记录 API：POST JSON {"content": ...} 到用户的 webhook；#标签就是正文的一部分。成功要说「已记录」。
    let server = MockServer::start(|_| (200, json!({"code": 0, "message": "已记录"}).to_string()));
    let h = flomo(&server);
    let row = h.find("flomo 今天读完了", |r| {
        r.id == "plugins:dev.helmp.flomo/memo"
    });
    assert_eq!(row.title, "Save to flomo");
    let out = h.run(
        &row.id,
        "flomo 今天读完了",
        &["--form", "content=今天读完了 \"Rust\" #reading\n第二行"],
    );
    assert!(out.contains("Saved to flomo"), "{out}");
    assert!(
        !out.contains("/iwh/"),
        "webhook 地址是密钥，不能出现在输出里：{out}"
    );

    let requests = server.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/iwh/tok/sec/");
    assert!(
        request
            .headers
            .iter()
            .any(|(k, v)| k == "content-type" && v.starts_with("application/json")),
        "{request:?}"
    );
    let body: Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(
        body,
        json!({"content": "今天读完了 \"Rust\" #reading\n第二行"})
    );
}

#[test]
fn flomo_without_form_uses_the_typed_text() {
    // 「flomo 一句话」直接回车（命令行不给 --form）时，正文就是关键词后面的文字。
    let server = MockServer::start(|_| (200, json!({"code": 0}).to_string()));
    let h = flomo(&server);
    let out = h.run(
        "plugins:dev.helmp.flomo/memo",
        "flomo quick thought #idea",
        &[],
    );
    assert!(out.contains("Saved to flomo"), "{out}");
    let body: Value = serde_json::from_str(&server.requests()[0].body).unwrap();
    assert_eq!(body["content"], "quick thought #idea");
}

#[test]
fn flomo_failures_are_errors_not_success() {
    // 服务端出错（HTTP 500）或返回 code != 0（webhook 失效、非会员）时，绝不能显示「已记录」。
    let server = MockServer::start(|_| (500, json!({"message": "server exploded"}).to_string()));
    let h = flomo(&server);
    let err = h.cli_err(&["run", "plugins:dev.helmp.flomo/memo", "flomo hi"]);
    assert!(
        err.contains("HTTP 500") && err.contains("server exploded"),
        "{err}"
    );
    assert!(!err.contains("Saved"), "{err}");

    let server = MockServer::start(|_| {
        (
            200,
            json!({"code": -1, "message": "webhook expired"}).to_string(),
        )
    });
    let h = flomo(&server);
    let err = h.cli_err(&["run", "plugins:dev.helmp.flomo/memo", "flomo hi"]);
    assert!(err.contains("webhook expired"), "{err}");
    assert!(!err.contains("Saved"), "{err}");
}

#[test]
fn flomo_without_a_webhook_points_to_settings() {
    // 没填 webhook 时不能带着空地址发请求，要引导去设置。
    let h = Helmp::new();
    let out = h.run("plugins:dev.helmp.flomo/memo", "flomo hi", &[]);
    assert!(out.contains("Set up flomo first"), "{out}");
    assert!(
        out.contains("Webhook URL") && out.contains("Open Plugin Settings"),
        "{out}"
    );
}

#[test]
fn real_hosts_are_never_contacted_in_dry_run_and_secrets_are_masked() {
    // 演练模式下发往真实主机的请求只回报不发送；回报里的 webhook（密钥）要打码。
    let secrets = json!({"dev.helmp.flomo/webhook": "https://flomoapp.com/iwh/abc123/def456/"});
    let h = Helmp::new().env("HELMP_TEST_SECRETS", secrets.to_string());
    let out = h.run("plugins:dev.helmp.flomo/memo", "flomo hi", &[]);
    assert!(out.contains("[dry-run] http POST ••••"), "{out}");
    assert!(out.contains(r#"{"content":"hi"}"#), "{out}");
    assert!(!out.contains("abc123"), "{out}");
}

#[test]
fn http_to_a_host_outside_the_permissions_is_blocked() {
    // 偏好设置里的地址被改成别的主机（填错或被诱导）时，请求不能发出去。
    let secrets = json!({"dev.helmp.flomo/webhook": "https://evil.test/iwh/a/b/"});
    let h = Helmp::new().env("HELMP_TEST_SECRETS", secrets.to_string());
    let err = h.cli_err(&["run", "plugins:dev.helmp.flomo/memo", "flomo hi"]);
    assert!(
        err.contains("evil.test") && err.contains("Blocked"),
        "{err}"
    );
}

#[test]
fn quicklinks_open_correctly_encoded_urls() {
    // 「npm @types/node」这种带 @ / & 的查询必须编码，否则打开的是另一个搜索。
    let h = Helmp::new();
    let row = h.find("npm @types/node & co", |r| {
        r.id == "plugins:dev.helmp.quicklinks/npm"
    });
    assert_eq!(
        row.subtitle,
        "https://www.npmjs.com/search?q=%40types%2Fnode%20%26%20co"
    );
    let out = h.run(&row.id, "npm @types/node & co", &[]);
    assert_eq!(
        out.trim(),
        "[dry-run] open https://www.npmjs.com/search?q=%40types%2Fnode%20%26%20co"
    );

    // select 偏好设置进模板：默认译成简体中文，改成英文后链接跟着变。
    let out = h.run("plugins:dev.helmp.quicklinks/gt", "gt 你好", &[]);
    assert!(out.contains("tl=zh-CN&text=%E4%BD%A0%E5%A5%BD"), "{out}");
    let en = Helmp::new()
        .config(|c| c["plugins"]["dev.helmp.quicklinks"] = json!({"prefs": {"translateTo": "en"}}));
    assert!(
        en.run("plugins:dev.helmp.quicklinks/gt", "gt 你好", &[])
            .contains("tl=en&")
    );

    // 只打了关键词、没给内容：提示怎么用，而不是打开一个空搜索。
    let out = h.run("plugins:dev.helmp.quicklinks/gh", "gh", &[]);
    assert!(
        out.contains("needs input") && !out.contains("[dry-run]"),
        "{out}"
    );
}

#[test]
fn bundled_plugins_can_be_turned_off() {
    // 自带插件只读但可以停用：停用后不再出现在搜索里。
    let h =
        Helmp::new().config(|c| c["plugins"]["dev.helmp.quicklinks"] = json!({"enabled": false}));
    assert!(
        !h.query("npm react")
            .iter()
            .any(|r| r.id.starts_with("plugins:dev.helmp.quicklinks"))
    );
}

#[test]
fn obsidian_capture_builds_the_url_scheme() {
    // Obsidian 快速记录走 obsidian://new，仓库名和内容都要编码。
    let h = Helmp::new()
        .config(|c| c["plugins"]["dev.helmp.obsidian"] = json!({"prefs": {"vault": "My Notes"}}));
    let out = h.run(
        "plugins:dev.helmp.obsidian/capture",
        "obsidian idea",
        &["--form", "content=a & b", "--form", "name=Inbox note"],
    );
    assert_eq!(
        out.trim(),
        "[dry-run] open obsidian://new?vault=My%20Notes&name=Inbox%20note&content=a%20%26%20b"
    );
}

#[test]
fn script_list_plugin_returns_items_and_runs_the_chosen_one() {
    // 脚本输出 Alfred Script Filter JSON，宿主渲染列表；选中一项执行插件声明的动作。
    let h = Helmp::new().config(enable("com.example.list", &["exec", "open:https"]));
    user_plugin(
        &h,
        "com.example.list",
        &json!({
            "schemaVersion": 1, "id": "com.example.list", "name": "List", "version": "1.0.0", "author": "Tests",
            "commands": [{
                "id": "items", "title": "My List", "keywords": ["mylist"],
                "list": {
                    "script": {"path": "bin/list.sh", "args": ["{{query}}"]},
                    "action": {"type": "open", "url": "https://example.com/{{item.arg|url}}"}
                }
            }]
        }),
        &[(
            "bin/list.sh",
            "#!/bin/sh\nprintf '{\"items\":[{\"uid\":\"a\",\"title\":\"Result for %s\",\"subtitle\":\"first\",\"arg\":\"%s\"},{\"uid\":\"b\",\"title\":\"Second\",\"arg\":\"two\"}]}' \"$1\" \"$1\"\n",
        )],
    );
    let rows = h.query("mylist foo bar");
    let first = rows
        .iter()
        .find(|r| r.id == "plugins:com.example.list/items#a")
        .unwrap_or_else(|| panic!("{rows:#?}"));
    assert_eq!(first.title, "Result for foo bar");
    assert_eq!(first.subtitle, "first");
    assert!(
        rows.iter()
            .any(|r| r.id == "plugins:com.example.list/items#b")
    );
    let out = h.run(&first.id, "mylist foo bar", &[]);
    assert_eq!(out.trim(), "[dry-run] open https://example.com/foo%20bar");
}

#[test]
fn script_list_errors_are_shown_not_swallowed() {
    // 列表脚本坏了要显示原因，而不是一个空列表。
    let h = Helmp::new().config(enable("com.example.list", &["exec", "open:*"]));
    user_plugin(
        &h,
        "com.example.list",
        &json!({
            "schemaVersion": 1, "id": "com.example.list", "name": "List", "version": "1.0.0", "author": "Tests",
            "commands": [{"id": "items", "title": "My List", "keywords": ["mylist"], "list": {"script": {"path": "bin/list.sh"}}}]
        }),
        &[("bin/list.sh", "#!/bin/sh\necho 'not json'\n")],
    );
    let row = h.find("mylist x", |r| {
        r.id == "plugins:com.example.list/items#error"
    });
    assert!(row.subtitle.contains("didn't print JSON"), "{row:?}");
}

#[test]
fn script_actions_get_argv_env_and_stdin_without_a_shell() {
    // 参数按 argv 传：查询里的 `; rm -rf` 只是普通文字，不会被 shell 执行。偏好设置走环境变量，正文走 stdin。
    let h = Helmp::new().config(|c| {
        enable("com.example.greet", &["exec"])(c);
        c["plugins"]["com.example.greet"]["prefs"] = json!({"greeting": "Hi"});
    });
    user_plugin(
        &h,
        "com.example.greet",
        &json!({
            "schemaVersion": 1, "id": "com.example.greet", "name": "Greet", "version": "1.0.0", "author": "Tests",
            "preferences": [{"id": "greeting", "type": "text", "title": "Greeting", "default": "Hello"}],
            "commands": [{
                "id": "greet", "title": "Greet", "keywords": ["greet"], "argument": {},
                "action": {"type": "script", "path": "bin/greet.sh", "args": ["{{query}}"],
                           "env": {"GREETING": "{{pref.greeting}}"}, "stdin": "{{query|upper}}"}
            }]
        }),
        &[(
            "bin/greet.sh",
            "#!/bin/sh\nprintf '%s|%s|%s\\n' \"$1\" \"$GREETING\" \"$(cat)\"\n",
        )],
    );
    let out = h.run(
        "plugins:com.example.greet/greet",
        "greet a b; rm -rf x",
        &[],
    );
    assert!(out.contains("a b; rm -rf x|Hi|A B; RM -RF X"), "{out}");
}

#[test]
fn clipboard_is_never_read_in_dry_run() {
    // {{clipboard}} 只在声明了 clipboard.read 时可用；测试（演练模式）里不读用户真实的剪贴板。
    let h = Helmp::new().config(enable(
        "com.example.clip",
        &["clipboard:read", "clipboard:write"],
    ));
    user_plugin(
        &h,
        "com.example.clip",
        &json!({
            "schemaVersion": 1, "id": "com.example.clip", "name": "Clip", "version": "1.0.0", "author": "Tests",
            "permissions": ["clipboard.read"],
            "commands": [{"id": "up", "title": "Uppercase Clipboard", "keywords": ["upclip"],
                          "action": {"type": "copy", "text": "{{clipboard|upper}}"}}]
        }),
        &[],
    );
    let out = h.run("plugins:com.example.clip/up", "upclip", &[]);
    assert_eq!(out.trim(), "[dry-run] copy [CLIPBOARD]");
}

#[test]
fn validate_reports_ok_for_a_good_plugin() {
    // 开发者自查工具：合格的插件给出 OK 和推导出的权限。
    let h = Helmp::new();
    let good = repo_file("tests/fixtures/plugins/good");
    let out = h.cli(&["plugins", "validate", good.to_str().unwrap()]);
    assert!(
        out.contains("OK com.example.good 1.2.3 (2 commands)"),
        "{out}"
    );
    assert!(out.contains("Network access: api.example.com"), "{out}");
    assert!(out.contains("Run scripts from the plugin folder"), "{out}");
}

#[test]
fn validate_lists_every_problem_of_a_bad_plugin() {
    // 写错的插件：一次列出全部问题并失败退出（CI 里能拦住）。
    let h = Helmp::new();
    let bad = repo_file("tests/fixtures/plugins/bad");
    let err = h.cli_err(&["plugins", "validate", bad.to_str().unwrap()]);
    for needle in [
        "id:",
        "host can't be a template",
        "clipboard.read",
        "bin/missing.sh not found",
        "4 problem(s)",
    ] {
        assert!(err.contains(needle), "missing {needle:?} in:\n{err}");
    }
    let missing = h.cli_err(&["plugins", "validate", h.root.join("nope").to_str().unwrap()]);
    assert!(missing.contains("has no plugin.json"), "{missing}");
}

#[test]
fn manage_plugins_command_opens_the_settings_page() {
    // 启动器里输入「插件管理」直达设置 › 插件。
    let h = Helmp::new();
    h.find("manage plugins", |r| r.id == "plugins:manage");
    assert_eq!(
        h.run("plugins:manage", "manage plugins", &[]).trim(),
        "open settings: Plugins"
    );
    let zh = Helmp::new().lang("zh_hans");
    let row = zh.find("插件管理", |r| r.id == "plugins:manage");
    assert_eq!(row.title, "插件管理");
}
