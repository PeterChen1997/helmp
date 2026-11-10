//! 端到端测试框架：每个用例一个隔离环境，通过真实的 `helmp` 可执行文件走「查询 → 执行」。
//!
//! 隔离：临时 `HELMP_DATA_DIR`、临时 `HOME`、`CLAUDE_CONFIG_DIR`、`CODEX_HOME`，剪贴板采集关闭，
//! `HELMP_DRY_RUN=1`（复制、粘贴、打开、截图等只回报不执行，见 src/effects.rs）。
//! 所以测试不会读写用户的剪贴板、钥匙串、~/.claude、~/.codex，也不会打开浏览器。

#![allow(dead_code)]

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// 一行查询结果：badge \t title \t subtitle \t id。
#[derive(Debug, Clone)]
pub struct Row {
    pub badge: String,
    pub title: String,
    pub subtitle: String,
    pub id: String,
}

pub struct Helmp {
    pub root: PathBuf,
    pub data: PathBuf,
    pub home: PathBuf,
    config: Value,
    envs: Vec<(String, String)>,
}

impl Helmp {
    /// 英文界面（断言用英文文案，不随系统语言变）。
    pub fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "helmp-e2e-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let data = root.join("data");
        let home = root.join("home");
        for dir in [&data, &home, &root.join("claude"), &root.join("codex")] {
            std::fs::create_dir_all(dir).expect("create e2e dir");
        }
        let config = json!({
            "general": {"language": "en", "launch_at_login": false, "auto_check_updates": false},
            "clipboard": {"enabled": false, "record_images": false},
            "onboarding_done": true
        });
        Self { root, data, home, config, envs: Vec::new() }
    }

    /// 改配置（写进 config.json 的 JSON，会与默认值合并）。
    pub fn config(mut self, edit: impl FnOnce(&mut Value)) -> Self {
        edit(&mut self.config);
        self
    }

    pub fn env(mut self, key: &str, value: impl Into<String>) -> Self {
        self.envs.push((key.to_string(), value.into()));
        self
    }

    pub fn lang(self, lang: &str) -> Self {
        let lang = lang.to_string();
        self.config(move |c| c["general"]["language"] = json!(lang))
    }

    /// 在数据目录里写一个文件（如脚本、插件清单、夹具），返回绝对路径。
    pub fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, content).expect("write fixture");
        path
    }

    fn command(&self) -> Command {
        std::fs::write(
            self.data.join("config.json"),
            serde_json::to_string_pretty(&self.config).expect("config json"),
        )
        .expect("write config");
        let mut command = Command::new(env!("CARGO_BIN_EXE_helmp"));
        command
            .env("HELMP_DATA_DIR", &self.data)
            .env("HOME", &self.home)
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("CODEX_HOME", self.root.join("codex"))
            .env("HELMP_DRY_RUN", "1")
            .env("HELMP_SCRIPT_DIRS", self.root.join("scripts"))
            .env_remove("HELMP_TRACE");
        for (key, value) in &self.envs {
            command.env(key, value);
        }
        command
    }

    /// `helmp <args>`，要求成功退出，返回 stdout。
    pub fn cli(&self, args: &[&str]) -> String {
        let out = self.command().args(args).output().expect("run helmp");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "helmp {args:?} 失败（{}）\nstdout:\n{stdout}\nstderr:\n{stderr}",
            out.status
        );
        stdout
    }

    /// `helmp <args>`，要求失败退出，返回 stderr。
    pub fn cli_err(&self, args: &[&str]) -> String {
        let out = self.command().args(args).output().expect("run helmp");
        assert!(!out.status.success(), "helmp {args:?} 应该失败，实际成功：{}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    pub fn query(&self, text: &str) -> Vec<Row> {
        self.cli(&["query", text])
            .lines()
            .filter_map(|line| {
                let mut cols = line.split('\t');
                Some(Row {
                    badge: cols.next()?.to_string(),
                    title: cols.next()?.to_string(),
                    subtitle: cols.next()?.to_string(),
                    id: cols.next()?.to_string(),
                })
            })
            .collect()
    }

    /// 查询后找出第一条满足条件的结果，找不到时把全部结果打出来。
    pub fn find(&self, text: &str, pred: impl Fn(&Row) -> bool) -> Row {
        let rows = self.query(text);
        rows.iter()
            .find(|row| pred(row))
            .cloned()
            .unwrap_or_else(|| panic!("查询 {text:?} 没有符合条件的结果：{rows:#?}"))
    }

    /// `helmp run <id> <query> [extra...]`。
    pub fn run(&self, id: &str, query: &str, extra: &[&str]) -> String {
        let mut args = vec!["run", id, query];
        args.extend_from_slice(extra);
        self.cli(&args)
    }

    pub fn data_file(&self, name: &str) -> PathBuf {
        self.data.join(name)
    }
}

impl Drop for Helmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn repo_file(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// 本地假 HTTP 服务：记录每个请求（方法、路径、正文），按给定函数回复 (状态码, JSON 正文)。
pub struct MockServer {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl MockServer {
    pub fn start(reply: impl Fn(&Request) -> (u16, String) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                let mut headers = Vec::new();
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = header.trim().split_once(':') {
                        if k.eq_ignore_ascii_case("content-length") {
                            length = v.trim().parse().unwrap_or(0);
                        }
                        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let request = Request { method, path, headers, body: String::from_utf8_lossy(&body).into_owned() };
                let (status, payload) = reply(&request);
                seen.lock().expect("mock lock").push(request);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                    payload.len()
                );
            }
        });
        Self { url, requests }
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().expect("mock lock").clone()
    }
}
