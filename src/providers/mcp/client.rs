//! 最小 MCP 客户端：JSON-RPC 2.0 over stdio（换行分隔）与 Streamable HTTP（POST，回复为 JSON 或 SSE）。
//!
//! 只实现启动器要用的部分：`initialize` 握手、`tools/list`（分页）、`tools/call`、`ping`、
//! `notifications/tools/list_changed`、超时后发 `notifications/cancelled`。
//! 没用官方 Rust SDK（rmcp）：它要 tokio 运行时和 reqwest，而 Helmp 全程同步线程 + ureq，
//! 为这几个方法引入第二套异步与 HTTP 栈不划算。
//!
//! 协议版本：发起 `2025-11-25`，接受服务器回的任何版本（`2025-06-18`、`2025-03-26`、`2024-11-05`
//! 及更新的版本都行：工具相关的消息在这些版本里兼容），协商结果记进日志、HTTP 请求头带上它。

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead as _, BufReader, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::i18n::{tr, trf};

pub const PROTOCOL_VERSION: &str = "2025-11-25";
/// 认识的旧版本（日志里标注用；不认识的新版本也照常继续）。
pub const KNOWN_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// 服务器发来的、管理器关心的事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    ToolsChanged,
    Closed,
}

pub type Events = Arc<dyn Fn(Event) + Send + Sync>;

/// 一条连接（stdio 子进程或 HTTP 会话）。
pub trait Conn: Send + Sync {
    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value>;
    fn notify(&self, method: &str, params: Value) -> Result<()>;
    fn alive(&self) -> bool;
    fn close(&self);
}

fn rpc_error(error: &Value) -> anyhow::Error {
    let code = error.get("code").and_then(Value::as_i64).unwrap_or_default();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    anyhow!(trf!(
        "服务器返回错误 {code}：{message}",
        "The server returned error {code}: {message}"
    ))
}

// ---------------------------------------------------------------- 日志

/// 每个服务器一个日志文件（stderr、启动 / 退出、协议版本）。超过 1 MB 时只留后一半。
#[derive(Clone)]
pub struct Log {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

const LOG_LIMIT: u64 = 1024 * 1024;

impl Log {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Arc::default(),
        }
    }

    pub fn line(&self, text: &str) {
        let _guard = self.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::metadata(&self.path).is_ok_and(|meta| meta.len() > LOG_LIMIT)
            && let Ok(old) = std::fs::read(&self.path)
        {
            let keep = old.len() / 2;
            let _ = std::fs::write(&self.path, &old[old.len() - keep..]);
        }
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "[{stamp}] {text}");
        }
    }
}

// ---------------------------------------------------------------- stdio

type Pending = Arc<Mutex<HashMap<u64, SyncSender<Result<Value, Value>>>>>;

pub struct StdioConn {
    child: Mutex<Child>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    pending: Pending,
    next_id: AtomicU64,
    alive: Arc<AtomicBool>,
}

/// GUI 启动的 App 拿到的 PATH 很短（没有 Homebrew、~/.local/bin），npx / uvx 会找不到：补上常见目录。
pub fn augmented_path() -> String {
    let mut dirs: Vec<String> = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|dir| !dir.is_empty())
        .map(str::to_string)
        .collect();
    let home = crate::paths::home_dir().unwrap_or_default();
    let extra = [
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".local/bin"),
        home.join(".cargo/bin"),
        home.join(".bun/bin"),
        home.join(".volta/bin"),
        home.join(".deno/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ];
    for dir in extra {
        let dir = dir.display().to_string();
        if !dirs.contains(&dir) && Path::new(&dir).is_dir() {
            dirs.push(dir);
        }
    }
    dirs.join(":")
}

/// 在 PATH 里找命令；带 `/` 的按路径（`~` 展开）。
pub fn resolve_command(command: &str, path: &str) -> Option<PathBuf> {
    if let Some(rest) = command.strip_prefix("~/") {
        return crate::paths::home_dir().ok().map(|home| home.join(rest));
    }
    if command.contains('/') {
        return Some(PathBuf::from(command));
    }
    path.split(':')
        .map(|dir| Path::new(dir).join(command))
        .find(|candidate| candidate.is_file())
}

pub struct Launch<'a> {
    pub command: &'a str,
    pub args: &'a [String],
    pub env: &'a BTreeMap<String, String>,
    pub cwd: Option<&'a str>,
}

impl StdioConn {
    pub fn spawn(launch: &Launch, log: &Log, events: Events) -> Result<Arc<Self>> {
        let path = augmented_path();
        let program = resolve_command(launch.command, &path).with_context(|| {
            trf!(
                "找不到命令 {}（PATH 里没有；装好后重试，或写完整路径）",
                "Command {} not found (not on PATH; install it or use a full path)",
                launch.command
            )
        })?;
        let cwd = match launch.cwd.filter(|cwd| !cwd.is_empty()) {
            Some(cwd) => match cwd.strip_prefix("~/") {
                Some(rest) => crate::paths::home_dir()?.join(rest),
                None => PathBuf::from(cwd),
            },
            None => crate::paths::home_dir()?,
        };
        let mut command = Command::new(&program);
        command
            .args(launch.args)
            .current_dir(&cwd)
            .env("PATH", &path)
            .envs(launch.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // 自成一个进程组：停止时连 npx 拉起的 node 子进程一起结束，不留孤儿。
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn().with_context(|| {
            trf!(
                "启动 {} 失败",
                "Couldn't start {}",
                program.display()
            )
        })?;
        log.line(&format!(
            "start pid={} {} {}",
            child.id(),
            program.display(),
            launch.args.join(" ")
        ));
        let stdout = child.stdout.take().context("stdout")?;
        let stderr = child.stderr.take().context("stderr")?;
        let stdin = Arc::new(Mutex::new(child.stdin.take()));
        let conn = Arc::new(Self {
            child: Mutex::new(child),
            stdin: Arc::clone(&stdin),
            pending: Pending::default(),
            next_id: AtomicU64::new(1),
            alive: Arc::new(AtomicBool::new(true)),
        });

        let stderr_log = log.clone();
        std::thread::Builder::new()
            .name("helmp-mcp-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    stderr_log.line(&format!("stderr {line}"));
                }
            })?;

        let (pending, alive, log) = (
            Arc::clone(&conn.pending),
            Arc::clone(&conn.alive),
            log.clone(),
        );
        std::thread::Builder::new()
            .name("helmp-mcp-stdout".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let Ok(message) = serde_json::from_str::<Value>(&line) else {
                        log.line(&format!("stdout (not JSON) {line}"));
                        continue;
                    };
                    let messages = match message {
                        Value::Array(items) => items,
                        other => vec![other],
                    };
                    for message in messages {
                        dispatch(&message, &pending, &events, &|reply| {
                            write_line(&stdin, &reply)
                        });
                    }
                }
                alive.store(false, Ordering::SeqCst);
                log.line("stdout closed");
                // 进程退出：等着回复的请求立即失败，不用等到超时。
                for (_, waiter) in pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .drain()
                {
                    let _ = waiter.try_send(Err(json!({"code": -32000, "message": "server exited"})));
                }
                events(Event::Closed);
            })?;
        Ok(conn)
    }
}

fn write_line(stdin: &Mutex<Option<ChildStdin>>, message: &Value) -> Result<()> {
    let mut guard = stdin.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let pipe = guard
        .as_mut()
        .context(tr("服务器已关闭", "The server is closed"))?;
    let mut line = message.to_string();
    line.push('\n');
    pipe.write_all(line.as_bytes())?;
    pipe.flush()?;
    Ok(())
}

/// 处理服务器发来的一条消息：回复、通知、或服务器发起的请求（只回 ping，其余回「方法不存在」）。
fn dispatch(
    message: &Value,
    pending: &Pending,
    events: &Events,
    reply: &dyn Fn(Value) -> Result<()>,
) {
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id");
    match (method, id) {
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else { return };
            let waiter = pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id);
            if let Some(waiter) = waiter {
                let result = match message.get("error") {
                    Some(error) => Err(error.clone()),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = waiter.try_send(result);
            }
        }
        (Some(method), Some(id)) => {
            let response = if method == "ping" {
                json!({"jsonrpc": "2.0", "id": id, "result": {}})
            } else {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("Method not found: {method}")}})
            };
            let _ = reply(response);
        }
        (Some("notifications/tools/list_changed"), None) => events(Event::ToolsChanged),
        _ => {}
    }
}

impl Conn for StdioConn {
    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        if !self.alive() {
            bail!(tr("服务器已退出", "The server has exited"));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = mpsc::sync_channel(1);
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, sender);
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if let Err(err) = write_line(&self.stdin, &message) {
            self.pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id);
            return Err(err.context(tr("写入服务器失败", "Couldn't write to the server")));
        }
        match receiver.recv_timeout(timeout) {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error)) => Err(rpc_error(&error)),
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&id);
                let _ = self.notify(
                    "notifications/cancelled",
                    json!({"requestId": id, "reason": "timeout"}),
                );
                bail!(trf!(
                    "{method} 超时（{} 秒没有回应）",
                    "{method} timed out (no answer in {} s)",
                    timeout.as_secs()
                ))
            }
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<()> {
        write_line(
            &self.stdin,
            &json!({"jsonrpc": "2.0", "method": method, "params": params}),
        )
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// 关 stdin → 等 1 秒 → SIGTERM 整个进程组 → 再等 1 秒 → SIGKILL（规范建议的顺序）。
    fn close(&self) {
        self.stdin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let mut child = self
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pid = i32::try_from(child.id()).unwrap_or_default();
        let exited = |child: &mut Child| {
            for _ in 0..20 {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        };
        if !exited(&mut child) && pid > 0 {
            // SAFETY: 向自己启动的进程组发信号。
            unsafe { libc::kill(-pid, libc::SIGTERM) };
            if !exited(&mut child) {
                unsafe { libc::kill(-pid, libc::SIGKILL) };
                let _ = child.wait();
            }
        } else if pid > 0 {
            // 主进程退出了，进程组里可能还有它拉起的子进程。
            unsafe { libc::kill(-pid, libc::SIGTERM) };
        }
        self.alive.store(false, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------- Streamable HTTP

pub struct HttpConn {
    url: String,
    headers: BTreeMap<String, String>,
    session: Mutex<Option<String>>,
    protocol: Mutex<Option<String>>,
    next_id: AtomicU64,
    alive: AtomicBool,
    events: Events,
    log: Log,
}

impl HttpConn {
    pub fn new(url: &str, headers: &BTreeMap<String, String>, log: &Log, events: Events) -> Arc<Self> {
        log.line(&format!("connect {url}"));
        Arc::new(Self {
            url: url.to_string(),
            headers: headers.clone(),
            session: Mutex::default(),
            protocol: Mutex::default(),
            next_id: AtomicU64::new(1),
            alive: AtomicBool::new(true),
            events,
            log: log.clone(),
        })
    }

    fn set_protocol(&self, version: &str) {
        *self
            .protocol
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(version.to_string());
    }

    /// 握手成功后记下协商的协议版本，之后的请求头都带上它（2025-06-18 起的要求）。
    fn after(&self, method: &str, result: &Result<Value>) {
        if method == "initialize"
            && let Ok(result) = result
            && let Some(version) = result.get("protocolVersion").and_then(Value::as_str)
        {
            self.set_protocol(version);
        }
    }

    fn post(&self, body: &Value, timeout: Duration) -> Result<ureq::http::Response<ureq::Body>> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            // 不跟随重定向：跳转目标可能是别的主机。
            .max_redirects(0)
            .build()
            .into();
        let mut request = agent
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        for (name, value) in &self.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(session) = self.session.lock().ok().and_then(|s| s.clone()) {
            request = request.header("mcp-session-id", session.as_str());
        }
        if let Some(version) = self.protocol.lock().ok().and_then(|p| p.clone()) {
            request = request.header("mcp-protocol-version", version.as_str());
        }
        let response = request.send(body.to_string()).map_err(|err| {
            anyhow!(trf!(
                "连不上 {}：{err}",
                "Couldn't reach {}: {err}",
                self.url
            ))
        })?;
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            *self
                .session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session.to_string());
        }
        Ok(response)
    }
}

/// 从 SSE 流里读消息，直到拿到 `id` 的回复。中途的通知照常处理。
fn read_sse(reader: impl Read, id: u64, events: &Events) -> Result<Value> {
    let pending: Pending = Pending::default();
    let (sender, receiver) = mpsc::sync_channel(1);
    pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id, sender);
    let mut data = String::new();
    for line in BufReader::new(reader).lines() {
        let line = line?;
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        } else if line.is_empty() && !data.is_empty() {
            if let Ok(message) = serde_json::from_str::<Value>(&std::mem::take(&mut data)) {
                dispatch(&message, &pending, events, &|_| Ok(()));
            }
            if let Ok(result) = receiver.try_recv() {
                return result.map_err(|error| rpc_error(&error));
            }
        }
    }
    if !data.is_empty()
        && let Ok(message) = serde_json::from_str::<Value>(&data)
    {
        dispatch(&message, &pending, events, &|_| Ok(()));
    }
    match receiver.try_recv() {
        Ok(result) => result.map_err(|error| rpc_error(&error)),
        Err(_) => bail!(tr(
            "服务器的事件流结束了，但没有回复",
            "The server's event stream ended without a reply"
        )),
    }
}

impl Conn for HttpConn {
    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut response = self.post(&body, timeout)?;
        let status = response.status().as_u16();
        if status == 404 && self.session.lock().is_ok_and(|s| s.is_some()) {
            // 会话过期：下次重新握手。
            self.alive.store(false, Ordering::SeqCst);
            (self.events)(Event::Closed);
            bail!(tr("服务器的会话已过期，请重试", "The server session expired; try again"));
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !(200..300).contains(&status) {
            let text = response.body_mut().read_to_string().unwrap_or_default();
            let text: String = text.chars().take(300).collect();
            if status == 401 || status == 403 {
                bail!(trf!(
                    "服务器拒绝访问（HTTP {status}）：检查请求头里的密钥。{text}",
                    "The server refused access (HTTP {status}): check the key in the headers. {text}"
                ));
            }
            bail!(trf!("HTTP {status}：{text}", "HTTP {status}: {text}"));
        }
        if content_type.starts_with("text/event-stream") {
            let result = read_sse(response.body_mut().as_reader(), id, &self.events);
            self.after(method, &result);
            return result;
        }
        let text = response.body_mut().read_to_string()?;
        let message: Value = serde_json::from_str(&text).with_context(|| {
            trf!(
                "服务器的回复不是 JSON：{}",
                "The server's reply isn't JSON: {}",
                text.chars().take(200).collect::<String>()
            )
        })?;
        let result = match message.get("error") {
            Some(error) => Err(rpc_error(error)),
            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        };
        self.after(method, &result);
        result
    }

    fn notify(&self, method: &str, params: Value) -> Result<()> {
        let body = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let response = self.post(&body, Duration::from_secs(10))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            self.log.line(&format!("notify {method}: HTTP {status}"));
        }
        Ok(())
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// 结束会话：DELETE（尽力而为，服务器不支持就算了）。
    fn close(&self) {
        self.alive.store(false, Ordering::SeqCst);
        let Some(session) = self.session.lock().ok().and_then(|s| s.clone()) else {
            return;
        };
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(3)))
            .http_status_as_error(false)
            .build()
            .into();
        let mut request = agent.delete(&self.url).header("mcp-session-id", session.as_str());
        for (name, value) in &self.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let _ = request.call();
        self.log.line("session closed");
    }
}

// ---------------------------------------------------------------- 握手与工具

/// `initialize` + `notifications/initialized`。返回协商出的协议版本和服务器名。
pub fn initialize(conn: &dyn Conn, timeout: Duration) -> Result<(String, String)> {
    let result = conn.request(
        "initialize",
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "helmp", "title": "Helmp", "version": env!("CARGO_PKG_VERSION")},
        }),
        timeout,
    )?;
    let version = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_VERSION)
        .to_string();
    let server = result
        .pointer("/serverInfo/name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    conn.notify("notifications/initialized", json!({}))?;
    Ok((version, server))
}

/// `tools/list`，跟着 `nextCursor` 翻页（最多 50 页，防止服务器给出死循环的游标）。
pub fn list_tools(conn: &dyn Conn, timeout: Duration) -> Result<Vec<Value>> {
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..50 {
        let params = match &cursor {
            Some(cursor) => json!({"cursor": cursor}),
            None => json!({}),
        };
        let result = conn.request("tools/list", params, timeout)?;
        if let Some(items) = result.get("tools").and_then(Value::as_array) {
            tools.extend(items.iter().cloned());
        }
        cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
            .map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    Ok(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_replies_are_matched_by_id_and_notifications_are_seen() {
        // Streamable HTTP 的回复可能是一段 SSE：前面夹着通知，要按 id 找到自己的回复。
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let events: Events = Arc::new(move |event| sink.lock().unwrap().push(event));
        let stream = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n\
                      data: {\"jsonrpc\":\"2.0\",\"id\":7,\n\
                      data: \"result\":{\"ok\":true}}\n\n";
        let result = read_sse(stream.as_bytes(), 7, &events).unwrap();
        assert_eq!(result, json!({"ok": true}));
        assert_eq!(*seen.lock().unwrap(), [Event::ToolsChanged]);
        let error = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32602,\"message\":\"bad\"}}\n\n";
        assert!(format!("{:#}", read_sse(error.as_bytes(), 1, &events).unwrap_err()).contains("bad"));
        assert!(read_sse("data: {}\n\n".as_bytes(), 1, &events).is_err());
    }

    #[test]
    fn commands_are_found_on_the_augmented_path() {
        let path = augmented_path();
        assert!(path.split(':').any(|dir| dir == "/usr/bin"));
        assert_eq!(resolve_command("sh", "/bin:/usr/bin"), Some(PathBuf::from("/bin/sh")));
        assert_eq!(resolve_command("definitely-not-a-command-xyz", &path), None);
        assert_eq!(resolve_command("/abs/x", &path), Some(PathBuf::from("/abs/x")));
    }
}
