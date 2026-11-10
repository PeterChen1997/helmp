//! 自动更新端到端测试：`script/publish-release.sh` 生成的清单 → 本地假更新源 → 真实的 `helmp update`。
//! 隔离环境带 `HELMP_DRY_RUN=1`：只允许访问本机地址，只下载和校验，绝不替换任何 App。

mod support;

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use support::{Helmp, MockServer, repo_file};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const ZIP_BODY: &str = "pretend this is Helmp.zip";
const DMG_BODY: &str = "pretend this is Helmp.dmg";

/// 用发布脚本（只生成、不上传）为假安装包生成 latest.json：测试的是发布端与 App 端对格式的约定。
fn published_manifest(h: &Helmp) -> Value {
    let dist = h.root.join("dist");
    std::fs::create_dir_all(&dist).expect("mkdir dist");
    std::fs::write(dist.join(format!("Helmp-{VERSION}.zip")), ZIP_BODY).expect("zip");
    std::fs::write(dist.join(format!("Helmp-{VERSION}.dmg")), DMG_BODY).expect("dmg");
    let out = std::process::Command::new("bash")
        .arg(repo_file("script/publish-release.sh"))
        .env("HELMP_DIST", &dist)
        .output()
        .expect("run publish-release.sh");
    assert!(
        out.status.success(),
        "publish-release.sh 失败\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(dist.join("latest.json")).expect("latest.json");
    let snapshot = std::fs::read_to_string(dist.join(format!("{VERSION}.json"))).expect("snapshot");
    assert_eq!(text, snapshot, "按版本的快照应与 latest.json 相同");
    serde_json::from_str(&text).expect("latest.json 是 JSON")
}

/// 一个假更新源：/latest.json 回清单，/<name> 回对应的安装包内容。
/// 清单里的下载地址换成这个服务（脚本写的是七牛和 GitHub），版本可改成更新的。
fn feed(manifest: &Value, version: &str, zip_body: &'static str) -> MockServer {
    // 清单里要写服务自己的地址，而地址在服务起来后才知道：先起服务，再填清单。
    let text = Arc::new(Mutex::new(String::new()));
    let reply = text.clone();
    let zip_path = format!("/Helmp-{VERSION}.zip");
    let dmg_path = format!("/Helmp-{VERSION}.dmg");
    let server = MockServer::start(move |request| match request.path.as_str() {
        "/latest.json" => (200, reply.lock().expect("lock").clone()),
        path if path == zip_path => (200, zip_body.to_string()),
        path if path == dmg_path => (200, DMG_BODY.to_string()),
        _ => (404, String::new()),
    });
    let mut manifest = manifest.clone();
    manifest["version"] = json!(version);
    for asset in manifest["assets"].as_array_mut().expect("assets") {
        let name = asset["name"].as_str().expect("name").to_string();
        asset["urls"] = json!([format!("{}/{name}", server.url)]);
    }
    *text.lock().expect("lock") = manifest.to_string();
    server
}

#[test]
fn published_manifest_is_valid_and_lists_qiniu_before_github() {
    let h = Helmp::new();
    let manifest = published_manifest(&h);
    assert_eq!(manifest["version"], VERSION);
    assert_eq!(manifest["minimum_macos"], "15.0", "与 bundle.sh 的 LSMinimumSystemVersion 一致");
    // 两种语言的更新说明都取自 CHANGELOG，不能为空。
    assert!(!manifest["notes"]["en"].as_str().unwrap_or_default().is_empty());
    assert!(!manifest["notes"]["zh-Hans"].as_str().unwrap_or_default().is_empty());
    let zip = manifest["assets"]
        .as_array()
        .expect("assets")
        .iter()
        .find(|asset| asset["kind"] == "zip")
        .expect("zip asset");
    assert_eq!(zip["size"], ZIP_BODY.len());
    let urls: Vec<&str> = zip["urls"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    // 国内用户先走七牛；GitHub 的地址带版本标签，和 App 的第二个更新源（releases/latest/download）同一个发布。
    assert_eq!(
        urls,
        vec![
            format!("https://blog.peterchen97.cn/helmp/releases/{VERSION}/Helmp-{VERSION}.zip"),
            format!("https://github.com/PeterChen1997/helmp/releases/download/v{VERSION}/Helmp-{VERSION}.zip"),
        ]
    );
}

#[test]
fn check_reports_a_newer_published_version_with_notes() {
    let h = Helmp::new();
    let server = feed(&published_manifest(&h), "99.0.0", ZIP_BODY);
    let h = h.env("HELMP_UPDATE_FEED", format!("{}/latest.json", server.url));
    let out = h.cli(&["update", "check"]);
    assert!(out.contains(&format!("current: {VERSION}")), "{out}");
    assert!(out.contains("latest: 99.0.0"), "{out}");
    assert!(out.contains("status: update-available"), "{out}");
    // 英文界面显示英文更新说明（取自 CHANGELOG 的英文部分）。
    assert!(out.contains("notes: "), "{out}");
    // 检查只读清单，不下载安装包。
    assert!(server.requests().iter().all(|r| r.path == "/latest.json"));
}

#[test]
fn same_version_is_up_to_date() {
    let h = Helmp::new();
    let server = feed(&published_manifest(&h), VERSION, ZIP_BODY);
    let h = h.env("HELMP_UPDATE_FEED", format!("{}/latest.json", server.url));
    let out = h.cli(&["update", "check"]);
    assert!(out.contains("status: up-to-date"), "{out}");
}

#[test]
fn falls_back_to_the_next_feed_and_says_why() {
    // 七牛不可用时改用 GitHub：结果照常，但失败原因要打出来，而不是悄悄跳过。
    let h = Helmp::new();
    let server = feed(&published_manifest(&h), "99.0.0", ZIP_BODY);
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        format!("http://{}/latest.json", listener.local_addr().expect("addr"))
    };
    let h = h.env("HELMP_UPDATE_FEED", format!("{dead},{}/latest.json", server.url));
    let out = h.output(&["update", "check"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("latest: 99.0.0"), "{stdout}");
    assert!(stderr.contains(&dead) && stderr.contains("unavailable"), "{stderr}");
}

#[test]
fn dry_run_never_touches_the_real_feeds() {
    // 测试环境（演练模式）里默认的七牛 / GitHub 地址一律拒绝：测试不会联网，也不会被线上发布影响。
    let h = Helmp::new();
    let err = h.cli_err(&["update", "check"]);
    assert!(err.contains("No network in dry-run mode"), "{err}");
    assert!(err.contains("blog.peterchen97.cn") && err.contains("github.com"), "{err}");
}

#[test]
fn install_needs_explicit_yes() {
    // agent 替用户执行时必须先征得同意：不带 --yes 只打印计划并失败退出，不下载任何东西。
    let h = Helmp::new();
    let server = feed(&published_manifest(&h), "99.0.0", ZIP_BODY);
    let h = h.env("HELMP_UPDATE_FEED", format!("{}/latest.json", server.url));
    let out = h.output(&["update", "install"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stdout.contains("Update") && stdout.contains("99.0.0"), "{stdout}");
    assert!(stderr.contains("--yes"), "{stderr}");
    assert!(server.requests().iter().all(|r| r.path == "/latest.json"), "不带 --yes 不应下载");
}

#[test]
fn install_downloads_and_verifies_the_zip_but_replaces_nothing() {
    let h = Helmp::new();
    let server = feed(&published_manifest(&h), "99.0.0", ZIP_BODY);
    // 装在 ~/Applications 下、目录可写：走原地替换（zip）。演练模式下才允许用 HELMP_UPDATE_TARGET 指定。
    let app = h.home.join("Applications/Helmp.app");
    std::fs::create_dir_all(app.join("Contents/MacOS")).expect("fake app");
    let h = h
        .env("HELMP_UPDATE_FEED", format!("{}/latest.json", server.url))
        .env("HELMP_UPDATE_TARGET", app.display().to_string());
    let out = h.cli(&["update", "install", "--yes"]);
    assert!(out.contains(&format!("Helmp-{VERSION}.zip")) && out.contains("verified"), "{out}");
    assert!(out.contains("no app was replaced"), "{out}");
    assert!(
        server.requests().iter().any(|r| r.path == format!("/Helmp-{VERSION}.zip")),
        "应下载 zip"
    );
    // 假 App 原封不动，旁边也没有留下解压目录。
    let entries: Vec<_> = std::fs::read_dir(app.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec!["Helmp.app".to_string()]);
}

#[test]
fn tampered_download_is_refused() {
    // 安装包内容和清单对不上（被篡改或下载不完整）：拒绝安装，并说清是哪一项不符。
    let h = Helmp::new();
    // 大小相同、内容不同：只有 sha256 能发现（变长的会先被大小上限拦下）。
    let server = feed(&published_manifest(&h), "99.0.0", "pretend this is Helmp.zap");
    let app = h.home.join("Applications/Helmp.app");
    std::fs::create_dir_all(&app).expect("fake app");
    let h = h
        .env("HELMP_UPDATE_FEED", format!("{}/latest.json", server.url))
        .env("HELMP_UPDATE_TARGET", app.display().to_string());
    let err = h.cli_err(&["update", "install", "--yes"]);
    assert!(err.contains("sha256 mismatch") && err.contains("refusing to install"), "{err}");
}

#[test]
fn launcher_command_opens_the_about_page() {
    // 面板里的「检查更新」：结果显示在设置 › 关于，所以动作要把用户带过去。
    let h = Helmp::new();
    let row = h.find("check for updates", |r| r.id == "builtin:check-updates");
    assert_eq!(row.title, "Check for Helmp Updates");
    let out = h.run("builtin:check-updates", "check for updates", &[]);
    assert!(out.contains("open settings: About"), "{out}");
}
