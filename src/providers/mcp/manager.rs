//! 服务器生命周期：
//! - 懒启动：第一次用到（调用工具、读工具列表）才启动；常驻服务器在 Helmp 启动和打开面板时启动。
//! - 空闲关闭：非常驻服务器空闲 `idle_secs`（默认 10 分钟）后停掉。
//! - 崩溃重启：意外退出或启动失败记一次失败，按 2、4、8…60 秒退避；常驻的到点自动重启，
//!   其余的等下次用到再启动。设置页的「重启」清零退避。
//! - 工具列表缓存在 `<data_dir>/mcp/tools.json`（按配置指纹作废），收到 `tools/list_changed`
//!   或重新连上时刷新。所以搜索只读内存，不会因为服务器慢而卡住输入。
//! - stderr 与启动 / 退出记录写进 `<data_dir>/mcp/logs/<服务器>.log`。
//!
//! 演练模式（`HELMP_DRY_RUN=1`）不启动真实服务器：只有测试模式（再加隔离的 `HELMP_DATA_DIR`）下
//! 命令或某个参数是数据目录里的文件的 stdio 服务器、以及本机地址（127.0.0.1 / localhost）的 http
//! 服务器会真的运行（端到端测试的假服务器），其余返回 `[dry-run] mcp-call …`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::client::{self, Conn, Event, Events, HttpConn, Launch, Log, StdioConn};
use super::schema::Tool;
use super::servers::{Spec, Transport, host_of_url, is_loopback};
use crate::i18n::{tr, trf};

/// 握手超时：npx / uvx 第一次运行要下载包，给足时间。
const INIT_TIMEOUT: Duration = Duration::from_secs(90);
const LIST_TIMEOUT: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_secs(15);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Stopped,
    Starting,
    Running,
    Error(String),
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Status::Stopped => tr("已停止", "Stopped"),
            Status::Starting => tr("启动中", "Starting"),
            Status::Running => tr("运行中", "Running"),
            Status::Error(_) => tr("出错", "Error"),
        }
    }

    /// 命令行输出用的稳定英文词。
    pub fn word(&self) -> &'static str {
        match self {
            Status::Stopped => "stopped",
            Status::Starting => "starting",
            Status::Running => "running",
            Status::Error(_) => "error",
        }
    }
}

/// 演练模式下拦住的服务器（调用方返回 `[dry-run] …`）。
#[derive(Debug)]
pub struct DryRunBlocked;

impl std::fmt::Display for DryRunBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[dry-run] server not started")
    }
}

impl std::error::Error for DryRunBlocked {}

pub fn is_dry_run_blocked(err: &anyhow::Error) -> bool {
    err.downcast_ref::<DryRunBlocked>().is_some()
}

// ---------------------------------------------------------------- 纯策略（单测用假时钟）

/// 第 n 次连续失败后的等待：2、4、8、16、32、60、60… 秒。
pub fn backoff(failures: u32) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    let secs = 2u64.saturating_pow(failures.min(16));
    Duration::from_secs(secs).min(MAX_BACKOFF)
}

/// 非常驻、运行中、空闲超时：该停了。
pub fn idle_expired(last_used: Instant, now: Instant, idle: Duration, resident: bool) -> bool {
    !resident && now.saturating_duration_since(last_used) >= idle
}

/// 退避期内不重试。
pub fn may_retry(retry_at: Option<Instant>, now: Instant) -> bool {
    retry_at.is_none_or(|at| now >= at)
}

// ---------------------------------------------------------------- 状态

#[derive(Debug, Clone)]
pub struct Info {
    pub status: Status,
    pub last_error: Option<String>,
    pub last_used: Instant,
    pub failures: u32,
    pub retry_at: Option<Instant>,
    /// 每次连接 +1；旧连接的「已退出」事件按它忽略。
    generation: u64,
    pub protocol: Option<String>,
    pub resident: bool,
}

impl Info {
    fn new() -> Self {
        Self {
            status: Status::Stopped,
            last_error: None,
            last_used: Instant::now(),
            failures: 0,
            retry_at: None,
            generation: 0,
            protocol: None,
            resident: false,
        }
    }
}

#[derive(Default)]
struct Slot {
    conn: Option<Arc<dyn Conn>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedTools {
    fingerprint: String,
    tools: Vec<Value>,
}

pub struct Manager {
    slots: Mutex<HashMap<String, Arc<Mutex<Slot>>>>,
    info: Mutex<HashMap<String, Info>>,
    cache: Mutex<Option<HashMap<String, CachedTools>>>,
}

pub fn global() -> &'static Manager {
    static MANAGER: OnceLock<Manager> = OnceLock::new();
    MANAGER.get_or_init(|| {
        let manager = Manager {
            slots: Mutex::default(),
            info: Mutex::default(),
            cache: Mutex::default(),
        };
        std::thread::Builder::new()
            .name("helmp-mcp-reaper".into())
            .spawn(|| {
                loop {
                    std::thread::sleep(TICK);
                    global().tick(Instant::now());
                }
            })
            .ok();
        manager
    })
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn mcp_dir() -> Result<PathBuf> {
    let dir = crate::paths::data_dir()?.join("mcp");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 工具缓存落盘：先写临时文件再改名。
fn persist(text: &str) {
    if let Ok(dir) = mcp_dir() {
        let tmp = dir.join("tools.json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join("tools.json"));
        }
    }
}

/// 服务器键 → 文件名（插件服务器的键带 `/`）。
fn safe_name(key: &str) -> String {
    key.chars()
        .map(|c| if c.is_alphanumeric() || "-_.".contains(c) { c } else { '_' })
        .collect()
}

pub fn log_path(key: &str) -> Result<PathBuf> {
    Ok(mcp_dir()?.join("logs").join(format!("{}.log", safe_name(key))))
}

/// 演练模式下这台服务器能不能真的运行（见模块说明）。
pub fn allowed(transport: &Transport) -> bool {
    if !crate::effects::dry_run() {
        return true;
    }
    match transport {
        Transport::Http { url, .. } => host_of_url(url).is_ok_and(|host| is_loopback(&host)),
        Transport::Stdio {
            command, args, cwd, ..
        } => {
            if !crate::providers::plugins::secrets::test_mode() {
                return false;
            }
            let Ok(data) = crate::paths::data_dir().and_then(|d| Ok(std::fs::canonicalize(d)?))
            else {
                return false;
            };
            let base = cwd.as_deref().map(PathBuf::from);
            let inside = |text: &str| {
                let path = Path::new(text);
                let path = match (&base, path.is_relative()) {
                    (Some(base), true) => base.join(path),
                    _ => path.to_path_buf(),
                };
                path.is_file()
                    && std::fs::canonicalize(&path).is_ok_and(|real| real.starts_with(&data))
            };
            inside(command) || args.iter().any(|arg| inside(arg))
        }
    }
}

impl Manager {
    fn slot(&self, key: &str) -> Arc<Mutex<Slot>> {
        Arc::clone(lock(&self.slots).entry(key.to_string()).or_default())
    }

    fn with_info<R>(&self, key: &str, f: impl FnOnce(&mut Info) -> R) -> R {
        let mut info = lock(&self.info);
        f(info.entry(key.to_string()).or_insert_with(Info::new))
    }

    pub fn info(&self, key: &str) -> Info {
        lock(&self.info).get(key).cloned().unwrap_or_else(Info::new)
    }

    // ------------------------------------------------------------ 工具缓存

    fn with_cache<R>(&self, f: impl FnOnce(&mut HashMap<String, CachedTools>) -> R) -> R {
        let mut cache = lock(&self.cache);
        let map = cache.get_or_insert_with(|| {
            mcp_dir()
                .ok()
                .and_then(|dir| std::fs::read_to_string(dir.join("tools.json")).ok())
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default()
        });
        f(map)
    }

    fn store_tools(&self, spec: &Spec, tools: Vec<Value>) {
        let text = self.with_cache(|map| {
            map.insert(
                spec.key.clone(),
                CachedTools {
                    fingerprint: spec.fingerprint(),
                    tools,
                },
            );
            serde_json::to_string_pretty(map).unwrap_or_default()
        });
        persist(&text);
    }

    /// 缓存的工具（配置变了的不算）。None = 还没读过。
    pub fn cached_tools(&self, spec: &Spec) -> Option<Vec<Tool>> {
        self.with_cache(|map| {
            map.get(&spec.key)
                .filter(|cached| cached.fingerprint == spec.fingerprint())
                .map(|cached| cached.tools.iter().filter_map(Tool::parse).collect())
        })
    }

    fn invalidate(&self, key: &str) {
        let text = self.with_cache(|map| {
            map.remove(key);
            serde_json::to_string_pretty(map).unwrap_or_default()
        });
        persist(&text);
    }

    pub fn forget(&self, key: &str) {
        self.stop(key);
        self.invalidate(key);
        lock(&self.info).remove(key);
    }

    // ------------------------------------------------------------ 连接

    /// 拿到活着的连接，必要时启动并握手（握手后顺便刷新工具列表）。
    fn connection(&self, spec: &Spec) -> Result<Arc<dyn Conn>> {
        let slot = self.slot(&spec.key);
        let mut slot = lock(&slot);
        self.with_info(&spec.key, |info| {
            info.last_used = Instant::now();
            info.resident = spec.resident;
        });
        if let Some(conn) = &slot.conn
            && conn.alive()
        {
            return Ok(Arc::clone(conn));
        }
        let info = self.info(&spec.key);
        if !may_retry(info.retry_at, Instant::now()) {
            let wait = info
                .retry_at
                .map(|at| at.saturating_duration_since(Instant::now()).as_secs() + 1)
                .unwrap_or_default();
            let last = info.last_error.unwrap_or_default();
            bail!(trf!(
                "「{}」刚出错，{wait} 秒后再试（上次的错误：{last}）",
                "“{}” just failed; try again in {wait} s (last error: {last})",
                spec.name
            ));
        }
        match self.connect(spec) {
            Ok(conn) => {
                slot.conn = Some(Arc::clone(&conn));
                Ok(conn)
            }
            Err(err) => {
                if !is_dry_run_blocked(&err) {
                    self.fail(&spec.key, &format!("{err:#}"));
                }
                Err(err)
            }
        }
    }

    fn fail(&self, key: &str, message: &str) {
        let now = Instant::now();
        self.with_info(key, |info| {
            info.failures += 1;
            info.retry_at = Some(now + backoff(info.failures));
            info.status = Status::Error(message.to_string());
            info.last_error = Some(message.to_string());
        });
        if let Ok(path) = log_path(key) {
            Log::new(path).line(&format!("error {message}"));
        }
    }

    fn connect(&self, spec: &Spec) -> Result<Arc<dyn Conn>> {
        let transport = spec.resolved()?;
        if !allowed(&transport) {
            return Err(anyhow!(DryRunBlocked));
        }
        let log = Log::new(log_path(&spec.key)?);
        let generation = self.with_info(&spec.key, |info| {
            info.generation += 1;
            info.status = Status::Starting;
            info.generation
        });
        let key = spec.key.clone();
        let events: Events = Arc::new(move |event| global().on_event(&key, generation, event));
        let conn: Arc<dyn Conn> = match &transport {
            Transport::Stdio {
                command,
                args,
                env,
                cwd,
            } => StdioConn::spawn(
                &Launch {
                    command,
                    args,
                    env,
                    cwd: cwd.as_deref(),
                },
                &log,
                events,
            )?,
            Transport::Http { url, headers } => HttpConn::new(url, headers, &log, events),
        };
        let secrets = spec.secret_values(&transport);
        let redact = |err: anyhow::Error| {
            let mut text = format!("{err:#}");
            for secret in &secrets {
                text = text.replace(secret.as_str(), "••••");
            }
            anyhow!(text)
        };
        let (version, server) = match client::initialize(conn.as_ref(), INIT_TIMEOUT) {
            Ok(done) => done,
            Err(err) => {
                conn.close();
                return Err(redact(err.context(tr("握手失败", "Handshake failed"))));
            }
        };
        let known = client::KNOWN_VERSIONS.contains(&version.as_str());
        log.line(&format!(
            "initialized server={server} protocol={version}{}",
            if known { "" } else { " (newer than Helmp knows; continuing)" }
        ));
        let tools = client::list_tools(conn.as_ref(), LIST_TIMEOUT).map_err(redact)?;
        log.line(&format!("tools/list: {} tools", tools.len()));
        self.store_tools(spec, tools);
        self.with_info(&spec.key, |info| {
            info.status = Status::Running;
            info.failures = 0;
            info.retry_at = None;
            info.last_error = None;
            info.protocol = Some(version);
        });
        Ok(conn)
    }

    fn on_event(&self, key: &str, generation: u64, event: Event) {
        let current = self.with_info(key, |info| info.generation == generation);
        if !current {
            return;
        }
        match event {
            Event::ToolsChanged => {
                // 先作废缓存（就算刷新没来得及完成、进程就退出了，下次也会重新读），再后台刷新。
                self.invalidate(key);
                let key = key.to_string();
                std::thread::spawn(move || global().refresh_tools(&key));
            }
            Event::Closed => {
                let running = self.with_info(key, |info| {
                    matches!(info.status, Status::Running | Status::Starting)
                });
                if running {
                    self.fail(
                        key,
                        tr("服务器意外退出（详见日志）", "The server exited unexpectedly (see the log)"),
                    );
                }
            }
        }
    }

    /// 收到 list_changed：用现有连接重新读工具列表。
    fn refresh_tools(&self, key: &str) {
        let Some(spec) = super::servers::find(&crate::config::get(), key) else {
            return;
        };
        let conn = lock(&self.slot(key)).conn.clone();
        let Some(conn) = conn.filter(|conn| conn.alive()) else {
            return;
        };
        match client::list_tools(conn.as_ref(), LIST_TIMEOUT) {
            Ok(tools) => {
                if let Ok(path) = log_path(key) {
                    Log::new(path).line(&format!("tools/list_changed: {} tools", tools.len()));
                }
                self.store_tools(&spec, tools);
            }
            Err(err) => eprintln!("helmp: MCP {key} 刷新工具列表失败：{err:#}"),
        }
    }

    /// 读工具列表：有缓存用缓存，否则启动服务器读一次。
    pub fn tools(&self, spec: &Spec) -> Result<Vec<Tool>> {
        if let Some(tools) = self.cached_tools(spec) {
            return Ok(tools);
        }
        self.connection(spec)?;
        Ok(self.cached_tools(spec).unwrap_or_default())
    }

    /// 强制重新读取工具列表（`helmp mcp-client tools`）。
    pub fn reload_tools(&self, spec: &Spec) -> Result<Vec<Tool>> {
        let conn = self.connection(spec)?;
        let tools = client::list_tools(conn.as_ref(), LIST_TIMEOUT)?;
        self.store_tools(spec, tools);
        Ok(self.cached_tools(spec).unwrap_or_default())
    }

    /// `tools/call`。返回原始结果（content / structuredContent / isError）。
    pub fn call(&self, spec: &Spec, tool: &str, arguments: Value) -> Result<Value> {
        let conn = self.connection(spec)?;
        let result = conn.request(
            "tools/call",
            json!({"name": tool, "arguments": arguments}),
            spec.timeout,
        );
        self.with_info(&spec.key, |info| info.last_used = Instant::now());
        result.with_context(|| trf!("调用 {tool} 失败", "Calling {tool} failed"))
    }

    pub fn stop(&self, key: &str) {
        let conn = lock(&self.slot(key)).conn.take();
        self.with_info(key, |info| {
            // 让旧连接的「已退出」事件作废，不被当成崩溃。
            info.generation += 1;
            if !matches!(info.status, Status::Error(_)) {
                info.status = Status::Stopped;
            }
        });
        if let Some(conn) = conn {
            conn.close();
            if let Ok(path) = log_path(key) {
                Log::new(path).line("stopped");
            }
        }
    }

    /// 设置页「重启」：清零退避，重新启动并读工具。
    pub fn restart(&self, spec: &Spec) -> Result<()> {
        self.stop(&spec.key);
        self.with_info(&spec.key, |info| {
            info.failures = 0;
            info.retry_at = None;
            info.last_error = None;
        });
        self.connection(spec).map(|_| ())
    }

    /// 定时：停掉空闲的、重启挂掉的常驻服务器。
    pub fn tick(&self, now: Instant) {
        let config = crate::config::get();
        let idle = Duration::from_secs(config.mcp.idle_secs.max(30));
        let specs = super::servers::all(&config);
        let infos: Vec<(String, Info)> = lock(&self.info)
            .iter()
            .map(|(key, info)| (key.clone(), info.clone()))
            .collect();
        for (key, info) in infos {
            let spec = specs.iter().find(|spec| spec.key == key);
            let gone = spec.is_none_or(|spec| !spec.enabled);
            let resident = spec.is_some_and(|spec| spec.resident);
            if info.status == Status::Running
                && (gone || idle_expired(info.last_used, now, idle, resident))
            {
                self.stop(&key);
            }
        }
        for spec in specs.iter().filter(|spec| spec.enabled && spec.resident) {
            let info = self.info(&spec.key);
            let down = !matches!(info.status, Status::Running | Status::Starting);
            if down && may_retry(info.retry_at, now) {
                let spec = spec.clone();
                std::thread::spawn(move || {
                    if let Err(err) = global().connection(&spec)
                        && !is_dry_run_blocked(&err)
                    {
                        eprintln!("helmp: MCP {} 启动失败：{err:#}", spec.key);
                    }
                });
            }
        }
    }

    /// 启动时：没有工具缓存的服务器启动一次读工具（非常驻的读完就停），常驻的保持运行。
    pub fn warm(&self, specs: &[Spec]) {
        let handles: Vec<_> = specs
            .iter()
            .filter(|spec| spec.enabled && (spec.resident || self.cached_tools(spec).is_none()))
            .cloned()
            .map(|spec| {
                std::thread::spawn(move || {
                    let manager = global();
                    match manager.connection(&spec) {
                        Ok(_) if !spec.resident => manager.stop(&spec.key),
                        Ok(_) => {}
                        Err(err) if is_dry_run_blocked(&err) => {}
                        Err(err) => eprintln!("helmp: MCP {} 启动失败：{err:#}", spec.key),
                    }
                })
            })
            .collect();
        for handle in handles {
            let _ = handle.join();
        }
    }

    /// 退出时停掉所有服务器（子进程不留到 Helmp 之后）。
    pub fn stop_all(&self) {
        let keys: Vec<String> = lock(&self.slots).keys().cloned().collect();
        for key in keys {
            self.stop(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps_at_a_minute() {
        // 崩溃后不能立刻反复重启（npx 下载失败时会把 CPU 打满），也不能等太久让用户以为坏了。
        let secs: Vec<u64> = (0..8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [0, 2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(backoff(u32::MAX), MAX_BACKOFF);
    }

    #[test]
    fn idle_servers_stop_unless_resident() {
        // 用假时钟：空闲满 10 分钟的停掉，常驻的不停，刚用过的不停。
        let start = Instant::now();
        let idle = Duration::from_secs(600);
        let later = start + Duration::from_secs(601);
        assert!(idle_expired(start, later, idle, false));
        assert!(!idle_expired(start, later, idle, true));
        assert!(!idle_expired(start, start + Duration::from_secs(599), idle, false));
        // 时钟回拨（last_used 在 now 之后）不算超时。
        assert!(!idle_expired(later, start, idle, false));
    }

    #[test]
    fn retries_wait_for_the_backoff_window() {
        let now = Instant::now();
        assert!(may_retry(None, now));
        assert!(!may_retry(Some(now + Duration::from_secs(2)), now));
        assert!(may_retry(Some(now), now));
    }

    #[test]
    fn log_names_are_file_safe() {
        assert_eq!(safe_name("com.example.p/files"), "com.example.p_files");
        assert_eq!(safe_name("我的 服务器"), "我的_服务器");
    }
}
