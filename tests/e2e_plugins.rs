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
    // 启用后的插件命令和自带的一样：要 Tab 进入它的范围再输入（见 gh 的用例）。
    let out = on.run_in(&row.id, &row.id, "rust & go", &[]);
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

const FLOMO: &str = "plugins:dev.helmp.flomo/memo";

#[test]
fn root_search_never_fills_or_submits_a_flomo_memo() {
    // 用户的原话：调用 flomo 这类插件要 Tab 二次确认，不能「flomo 空格」就行——日常搜索打出
    // 「flomo hello」只能列出这个命令（Tab 进入），不能把 hello 填进表单，更不能发出请求。
    let server = MockServer::start(|_| (200, json!({"code": 0}).to_string()));
    let h = flomo(&server);
    let row = h.find("flomo hello", |r| r.id == FLOMO);
    assert_eq!(row.title, "Save to flomo");
    assert!(!row.subtitle.contains("hello"), "{row:?}");
    // 在全局搜索里对它回车：只是进入命令范围，什么都没提交。
    let out = h.run(FLOMO, "flomo hello", &[]);
    assert_eq!(out.trim(), format!("enter scope {FLOMO}"));
    // 哪怕命令行带了表单值，在全局搜索里执行的也只是「进入范围」。
    let out = h.run(FLOMO, "flomo", &["--form", "content=hello"]);
    assert_eq!(out.trim(), format!("enter scope {FLOMO}"));
    assert!(server.requests().is_empty(), "{:?}", server.requests());

    // Tab 进入后（范围里）才是可以提交的表单，范围里打的字是正文的初值，回车才发出去。
    let inside = h.query_in(FLOMO, "hello");
    assert_eq!(inside.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), [FLOMO]);
    let out = h.run_in(FLOMO, FLOMO, "hello", &[]);
    assert!(out.contains("Saved to flomo"), "{out}");
    let requests = server.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    let body: Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(body, json!({"content": "hello"}));
}

#[test]
fn flomo_posts_the_memo_as_json_and_reports_success() {
    // flomo 记录 API：POST JSON {"content": ...} 到用户的 webhook；#标签就是正文的一部分。成功要说「已记录」。
    let server = MockServer::start(|_| (200, json!({"code": 0, "message": "已记录"}).to_string()));
    let h = flomo(&server);
    let row = h.find("flomo", |r| r.id == FLOMO);
    assert_eq!(row.title, "Save to flomo");
    let out = h.run_in(
        FLOMO,
        &row.id,
        "",
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
    // 进入 flomo 范围时带着的文字（深链接的 q、范围里打的字）就是正文：没改表单直接回车也发它。
    // 全局搜索里没有这个结果时，`helmp run <命令 id> <文字>` 就在命令自己的范围里找（不用写 --scope）。
    let server = MockServer::start(|_| (200, json!({"code": 0}).to_string()));
    let h = flomo(&server);
    let out = h.run(FLOMO, "quick thought #idea", &[]);
    assert!(out.contains("Saved to flomo"), "{out}");
    let body: Value = serde_json::from_str(&server.requests()[0].body).unwrap();
    assert_eq!(body["content"], "quick thought #idea");
}

#[test]
fn flomo_failures_are_errors_not_success() {
    // 服务端出错（HTTP 500）或返回 code != 0（webhook 失效、非会员）时，绝不能显示「已记录」。
    let server = MockServer::start(|_| (500, json!({"message": "server exploded"}).to_string()));
    let h = flomo(&server);
    let err = h.cli_err(&["run", FLOMO, "hi", "--scope", FLOMO]);
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
    let err = h.cli_err(&["run", FLOMO, "hi", "--scope", FLOMO]);
    assert!(err.contains("webhook expired"), "{err}");
    assert!(!err.contains("Saved"), "{err}");
}

#[test]
fn flomo_without_a_webhook_points_to_settings() {
    // 没填 webhook 时不能带着空地址发请求，要引导去设置。
    let h = Helmp::new();
    let out = h.run_in(FLOMO, FLOMO, "hi", &[]);
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
    let out = h.run_in(FLOMO, FLOMO, "hi", &[]);
    assert!(out.contains("[dry-run] http POST ••••"), "{out}");
    assert!(out.contains(r#"{"content":"hi"}"#), "{out}");
    assert!(!out.contains("abc123"), "{out}");
}

#[test]
fn http_to_a_host_outside_the_permissions_is_blocked() {
    // 偏好设置里的地址被改成别的主机（填错或被诱导）时，请求不能发出去。
    let secrets = json!({"dev.helmp.flomo/webhook": "https://evil.test/iwh/a/b/"});
    let h = Helmp::new().env("HELMP_TEST_SECRETS", secrets.to_string());
    let err = h.cli_err(&["run", FLOMO, "hi", "--scope", FLOMO]);
    assert!(
        err.contains("evil.test") && err.contains("Blocked"),
        "{err}"
    );
}

const GH: &str = "plugins:dev.helmp.quicklinks/gh";

#[test]
fn gh_offers_the_search_url_only_inside_its_scope() {
    // 全局搜索里的「gh rust」不能就是一个回车就打开的 GitHub 搜索（日常搜索会误触发），
    // 只列出 GitHub 命令；Tab 进入 gh 范围后，搜索框里的 rust 才是参数，给出现成的网址。
    let h = Helmp::new();
    let row = h.find("gh rust", |r| r.id == GH);
    assert!(!row.subtitle.contains("github.com"), "{row:?}");
    assert_eq!(h.run(GH, "gh rust", &[]).trim(), format!("enter scope {GH}"));

    let inside = h.query_in(GH, "rust");
    let ready = inside.iter().find(|r| r.id == GH).unwrap_or_else(|| panic!("{inside:#?}"));
    assert_eq!(ready.subtitle, "https://github.com/search?q=rust&type=repositories");
    assert_eq!(
        h.run_in(GH, GH, "rust", &[]).trim(),
        "[dry-run] open https://github.com/search?q=rust&type=repositories"
    );
    // 不存在 / 停用的命令没有范围：不能悄悄回到全局搜索。
    let err = h.cli_err(&["query", "--scope", "plugins:dev.helmp.quicklinks/nope", "rust"]);
    assert!(err.contains("No scope"), "{err}");
}

#[test]
fn quicklinks_open_correctly_encoded_urls() {
    // 「npm @types/node」这种带 @ / & 的查询必须编码，否则打开的是另一个搜索。
    let h = Helmp::new();
    let npm = "plugins:dev.helmp.quicklinks/npm";
    let row = h
        .query_in(npm, "@types/node & co")
        .into_iter()
        .find(|r| r.id == npm)
        .expect("npm 范围里的结果");
    assert_eq!(
        row.subtitle,
        "https://www.npmjs.com/search?q=%40types%2Fnode%20%26%20co"
    );
    let out = h.run_in(npm, npm, "@types/node & co", &[]);
    assert_eq!(
        out.trim(),
        "[dry-run] open https://www.npmjs.com/search?q=%40types%2Fnode%20%26%20co"
    );

    // select 偏好设置进模板：默认译成简体中文，改成英文后链接跟着变。
    let gt = "plugins:dev.helmp.quicklinks/gt";
    let out = h.run_in(gt, gt, "你好", &[]);
    assert!(out.contains("tl=zh-CN&text=%E4%BD%A0%E5%A5%BD"), "{out}");
    let en = Helmp::new()
        .config(|c| c["plugins"]["dev.helmp.quicklinks"] = json!({"prefs": {"translateTo": "en"}}));
    assert!(en.run_in(gt, gt, "你好", &[]).contains("tl=en&"));

    // 进了范围、还没输入内容就回车：提示怎么用，而不是打开一个空搜索。
    let out = h.run_in(GH, GH, "", &[]);
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
    let capture = "plugins:dev.helmp.obsidian/capture";
    let out = h.run_in(
        capture,
        capture,
        "idea",
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
    // 全局搜索里只有命令入口，不跑脚本；Tab 进入列表命令的范围后，按范围里的文字跑脚本。
    let scope = "plugins:com.example.list/items";
    let root = h.query("mylist foo bar");
    assert!(root.iter().any(|r| r.id == scope), "{root:#?}");
    assert!(!root.iter().any(|r| r.id.starts_with(&format!("{scope}#"))), "{root:#?}");
    let rows = h.query_in(scope, "foo bar");
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
    let out = h.run_in(scope, &first.id, "foo bar", &[]);
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
    let row = h
        .query_in("plugins:com.example.list/items", "x")
        .into_iter()
        .find(|r| r.id == "plugins:com.example.list/items#error")
        .expect("出错的那一行");
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
    let greet = "plugins:com.example.greet/greet";
    let out = h.run_in(greet, greet, "a b; rm -rf x", &[]);
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

/// 图形自检（屏幕外、不抢焦点）一行行的输出里，找以 `prefix` 开头的那一行。
fn self_test_line<'a>(out: &'a str, prefix: &str) -> &'a str {
    out.lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("没有 {prefix:?}：\n{out}"))
}

#[test]
#[ignore = "开真实的图形界面（屏幕外、不抢焦点，但菜单栏会短暂出现图标）。手动运行：cargo test --release --test e2e_plugins -- --ignored"]
fn gui_tab_enters_the_flomo_scope_and_esc_or_backspace_leave() {
    // 面板里的完整流程：全局搜索「flomo hello」只列出命令；Tab 进入 flomo 范围，搜索栏出现标签、
    // 光标直接在表单里，且没有把 hello 带进去（不吃全局搜索的字）；Esc 回到全局搜索；
    // 表单为空时 ⌫ 也回到全局搜索。
    let h = Helmp::new()
        .env("HELMP_NO_WELCOME", "1")
        .env("HELMP_SELF_TEST", "flomo hello")
        .env("HELMP_SELF_TEST_KEYS", "tab ~hi escape ~flomo tab backspace");
    let out = h.cli(&[]);
    let scope = r#"scope=Some("plugins:dev.helmp.flomo/memo")"#;
    let tab = self_test_line(&out, "self-test key tab:");
    assert!(tab.contains(scope) && tab.contains("focus=form"), "{tab}");
    assert!(tab.contains(r#"form=[("content", "")]"#), "全局搜索的 hello 不能带进表单：{tab}");
    let typed = self_test_line(&out, "self-test key ~hi:");
    assert!(typed.contains(r#"form=[("content", "hi")]"#), "{typed}");
    let escape = self_test_line(&out, "self-test key escape:");
    assert!(escape.contains("scope=None") && escape.contains("focus=search"), "{escape}");
    let backspace = self_test_line(&out, "self-test key backspace:");
    assert!(backspace.contains("scope=None"), "表单为空时 ⌫ 退出范围：{backspace}");

    // 深链接 helmp://plugin/…/memo?q= 直接打开 flomo 范围，q 填进表单，但不提交。
    let h = Helmp::new()
        .env("HELMP_NO_WELCOME", "1")
        .env("HELMP_SELF_TEST", "1")
        .env("HELMP_SELF_TEST_LINK", "helmp://plugin/dev.helmp.flomo/memo?q=from%20link");
    let out = h.cli(&[]);
    let link = self_test_line(&out, "self-test link ");
    assert!(link.contains(scope) && link.contains("focus=form"), "{link}");
    assert!(link.contains(r#"form=[("content", "from link")]"#), "{link}");
    assert!(link.contains(r#"result="""#), "深链接不能替用户提交：{link}");
}
