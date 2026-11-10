//! 文件搜索与最近文件：范围「文件」（前缀 `f ` / `file ` / `文件 `），根搜索里也混入少量结果。
//!
//! 两路结果合并打分：
//! - **Spotlight**：每次查询在后台跑 `mdfind -onlyin <范围>`，按文件名的单词前缀（Spotlight 自带中文分词，
//!   `luodi` 能命中「设计落地清单」）或子串匹配。输入期间防抖 120 ms，新查询会结束上一次还没跑完的 mdfind；
//!   结果按 (范围, 查询) 缓存 30 秒。查询线程最多等 `wait`（范围内 600 ms；根搜索不等，免得拖慢应用等其他结果），
//!   没等到就先返回已有结果，启动器每秒的后台刷新会把晚到的结果补上。
//! - **最近文件**：最近 30 天用过的文件（`kMDItemLastUsedDate`），在内存里用拼音匹配器打分，
//!   所以拼音首字母（`cesbg` → 测试报告.pdf）能命中最近用过的中文文件名。空查询列出最近文件。
//!
//! 排除：设置里的目录名（任意层级）或路径前缀、隐藏目录、`.app` 包（交给应用搜索）。
//!
//! 只给测试用的环境变量（端到端测试不能依赖 Spotlight 给临时目录建索引）：
//! - `HELMP_FILES_ROOTS`：覆盖搜索范围，`:` 分隔。
//! - `HELMP_FILES_BACKEND=walk`：不用 Spotlight，直接遍历目录（名称子串匹配；「最近」= 30 天内修改过）。
//! - `HELMP_FILES_WAIT_MS`：查询最多等搜索多久（命令行一次性查询要等到结果）。

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead as _, BufReader};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;

use crate::config;
use crate::i18n::{tr, trf};
use crate::matcher::{self, Candidate};
use crate::provider::{
    Accessory, Action, Block, Detail, Icon, Item, NamedAction, Provider, Scope, Tint, Tone,
};

const SCOPE: &str = "files";
const DEBOUNCE: Duration = Duration::from_millis(120);
const SCOPED_WAIT: Duration = Duration::from_millis(600);
const ROOT_WAIT: Duration = Duration::ZERO;
const CACHE_TTL: Duration = Duration::from_secs(30);
const CACHE_SIZE: usize = 16;
/// 一次 mdfind 最多读这么多行（常见词在家目录下能有上万条）。
const READ_CAP: usize = 2000;
/// 过滤后最多保留这么多条去取文件信息、打分。
const KEEP_CAP: usize = 400;
const RECENT_DAYS: u32 = 30;
const RECENT_CAP: usize = 5000;
const RECENT_TTL: Duration = Duration::from_secs(120);
const SCOPED_LIMIT: usize = 50;
const ROOT_LIMIT: usize = 5;
/// Spotlight 命中但拼音 / 字面匹配器解释不了（如 Spotlight 的中文分词拼音）时的分数。
const SPOTLIGHT_ONLY: u32 = 450;
/// 最近用过的文件同分时优先。
const RECENT_BONUS: u32 = 20;

const ICON: Icon = Icon::Symbol {
    name: "file-search",
    tint: Tint::Blue,
};

// ---------------------------------------------------------------- 搜索后端

/// 文件名搜索的实现：Spotlight（默认）或目录遍历（测试）。
pub trait Backend: Send + Sync {
    /// `root` 下名称匹配 `query` 的路径（不必排序，可以含要排除的路径），读到 `cap` 条为止；
    /// `cancelled()` 为真时尽快返回。
    fn search(
        &self,
        root: &Path,
        query: &str,
        cancelled: &(dyn Fn() -> bool + Sync),
        cap: usize,
    ) -> Vec<PathBuf>;
    /// `root` 下最近 `days` 天用过的文件和最近使用时间。
    fn recent(&self, root: &Path, days: u32, cap: usize) -> Vec<(PathBuf, Option<SystemTime>)>;
}

/// Spotlight：`mdfind` 子进程。
pub struct Spotlight;

/// Spotlight 查询语句里的字符串转义。
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('*', "\\*")
}

/// 每个词都要命中：文件名的单词前缀（含 Spotlight 的中文分词）或子串，不区分大小写和变音符。
pub fn spotlight_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|word| {
            let word = escape(word);
            format!(r#"(kMDItemDisplayName == "{word}*"cdw || kMDItemFSName == "*{word}*"cd)"#)
        })
        .collect::<Vec<_>>()
        .join(" && ")
}

/// 跑一次 mdfind，逐行读出路径；取消或读满时结束子进程。
fn mdfind(args: &[&str], cancelled: &(dyn Fn() -> bool + Sync), cap: usize) -> Vec<String> {
    let Ok(mut child) = Command::new("/usr/bin/mdfind")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        eprintln!("helmp: 启动 mdfind 失败");
        return Vec::new();
    };
    let Some(stdout) = child.stdout.take() else {
        return Vec::new();
    };
    // 取不到合法 pid 时不发信号：kill(0) 会结束整个进程组。
    let pid = libc::pid_t::try_from(child.id())
        .ok()
        .filter(|pid| *pid > 0);
    let done = AtomicBool::new(false);
    let lines = std::thread::scope(|scope| {
        // 读行会阻塞，取消要靠另一个线程结束子进程（管道关闭后读循环随之结束）。
        scope.spawn(|| {
            while !done.load(Ordering::SeqCst) {
                if cancelled() {
                    if let Some(pid) = pid {
                        // SAFETY: pid 是本函数刚启动、尚未回收的子进程。
                        unsafe { libc::kill(pid, libc::SIGTERM) };
                    }
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let mut lines = Vec::new();
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if !line.is_empty() {
                lines.push(line);
            }
            if lines.len() >= cap {
                break;
            }
        }
        done.store(true, Ordering::SeqCst);
        lines
    });
    let _ = child.kill();
    let _ = child.wait();
    lines
}

impl Backend for Spotlight {
    fn search(
        &self,
        root: &Path,
        query: &str,
        cancelled: &(dyn Fn() -> bool + Sync),
        cap: usize,
    ) -> Vec<PathBuf> {
        let Some(root) = root.to_str() else {
            return Vec::new();
        };
        let expression = spotlight_query(query);
        if expression.is_empty() {
            return Vec::new();
        }
        mdfind(&["-onlyin", root, &expression], cancelled, cap)
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }

    fn recent(&self, root: &Path, days: u32, cap: usize) -> Vec<(PathBuf, Option<SystemTime>)> {
        let Some(root) = root.to_str() else {
            return Vec::new();
        };
        let expression = format!("kMDItemLastUsedDate >= $time.today(-{days})");
        mdfind(
            &["-onlyin", root, "-attr", "kMDItemLastUsedDate", &expression],
            &|| false,
            cap,
        )
        .iter()
        .filter_map(|line| parse_attr_line(line))
        .collect()
    }
}

/// `mdfind -attr kMDItemLastUsedDate` 的一行：`<路径>   kMDItemLastUsedDate = 2026-11-09 14:26:20 +0000`。
fn parse_attr_line(line: &str) -> Option<(PathBuf, Option<SystemTime>)> {
    let (path, value) = line.rsplit_once("   kMDItemLastUsedDate = ")?;
    Some((PathBuf::from(path), parse_spotlight_date(value.trim())))
}

/// `2026-11-09 14:26:20 +0000` → 时间点。
fn parse_spotlight_date(text: &str) -> Option<SystemTime> {
    let mut parts = text.split_whitespace();
    let (date, time, zone) = (parts.next()?, parts.next()?, parts.next()?);
    let mut ymd = date.split('-').map(|n| n.parse::<i64>());
    let (y, m, d) = (ymd.next()?.ok()?, ymd.next()?.ok()?, ymd.next()?.ok()?);
    let mut hms = time.split(':').map(|n| n.parse::<i64>());
    let (hh, mm, ss) = (hms.next()?.ok()?, hms.next()?.ok()?, hms.next()?.ok()?);
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let zone = zone.trim_start_matches(['+', '-']);
    let offset = sign
        * (zone.get(..2)?.parse::<i64>().ok()? * 3600 + zone.get(2..4)?.parse::<i64>().ok()? * 60);
    let seconds = days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss - offset;
    let seconds = u64::try_from(seconds).ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(seconds))
}

/// 公历日期 → 1970-01-01 起的天数（Howard Hinnant 的算法）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 目录遍历：测试用（Spotlight 不一定给临时目录建索引）。名称子串匹配，「最近」= 修改时间。
pub struct Walk;

const WALK_DEPTH: usize = 8;
const WALK_LIMIT: usize = 50_000;

fn walk(root: &Path, mut visit: impl FnMut(&Path) -> bool) {
    let mut stack = vec![(root.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > WALK_LIMIT {
                return;
            }
            let path = entry.path();
            if !visit(&path) {
                return;
            }
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if !hidden && depth < WALK_DEPTH && entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push((path, depth + 1));
            }
        }
    }
}

impl Backend for Walk {
    fn search(
        &self,
        root: &Path,
        query: &str,
        cancelled: &(dyn Fn() -> bool + Sync),
        cap: usize,
    ) -> Vec<PathBuf> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let mut out = Vec::new();
        walk(root, |path| {
            let name = file_name(path).to_lowercase();
            if !words.is_empty() && words.iter().all(|word| name.contains(word.as_str())) {
                out.push(path.to_path_buf());
            }
            out.len() < cap && !cancelled()
        });
        out
    }

    fn recent(&self, root: &Path, days: u32, cap: usize) -> Vec<(PathBuf, Option<SystemTime>)> {
        let since = SystemTime::now() - Duration::from_secs(u64::from(days) * 86_400);
        let mut out = Vec::new();
        walk(root, |path| {
            let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            if path.is_file() && modified.is_some_and(|t| t >= since) {
                out.push((path.to_path_buf(), modified));
            }
            out.len() < cap
        });
        out
    }
}

// ---------------------------------------------------------------- 设置与排除

/// 排除规则：目录名（任意层级）或路径前缀。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Rule {
    Name(String),
    Prefix(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Settings {
    roots: Vec<PathBuf>,
    rules: Vec<Rule>,
    in_root: bool,
}

fn expand_home(text: &str, home: Option<&Path>) -> Option<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    match (text.strip_prefix('~'), home) {
        (Some(rest), Some(home)) => Some(home.join(rest.trim_start_matches('/'))),
        (Some(_), None) => None,
        (None, _) => Some(PathBuf::from(text)),
    }
}

fn settings_from(
    config: &config::FilesConfig,
    roots_override: Option<String>,
    home: Option<&Path>,
) -> Settings {
    let roots: Vec<PathBuf> = match roots_override {
        Some(paths) => std::env::split_paths(&paths)
            .filter(|p| !p.as_os_str().is_empty())
            .collect(),
        None => config
            .roots
            .iter()
            .filter_map(|root| expand_home(root, home))
            .collect(),
    };
    let rules = config
        .excluded
        .iter()
        .map(|rule| rule.trim())
        .filter(|rule| !rule.is_empty())
        .filter_map(|rule| {
            if rule.contains('/') {
                expand_home(rule, home).map(Rule::Prefix)
            } else {
                Some(Rule::Name(rule.to_string()))
            }
        })
        .collect();
    Settings {
        roots,
        rules,
        in_root: config.in_root_search,
    }
}

fn current_settings() -> Settings {
    let home = crate::paths::home_dir().ok();
    settings_from(
        &config::get().files,
        std::env::var("HELMP_FILES_ROOTS").ok(),
        home.as_deref(),
    )
}

/// 是否排除：命中排除规则、在隐藏目录里、是 `.app` 包或在包里面。
fn excluded(path: &Path, root: &Path, rules: &[Rule]) -> bool {
    if rules
        .iter()
        .any(|rule| matches!(rule, Rule::Prefix(prefix) if path.starts_with(prefix)))
    {
        return true;
    }
    let relative = path.strip_prefix(root).unwrap_or(path);
    let parts: Vec<&str> = relative
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    let last = parts.len().saturating_sub(1);
    parts.iter().enumerate().any(|(i, name)| {
        (i < last && name.starts_with('.'))
            || name.ends_with(".app")
            || rules
                .iter()
                .any(|rule| matches!(rule, Rule::Name(n) if n == name))
    })
}

// ---------------------------------------------------------------- 索引与缓存

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// 一个文件结果：路径、名称的匹配预处理、文件信息（在后台线程取好）。
#[derive(Debug, Clone)]
struct Entry {
    path: PathBuf,
    name: String,
    candidate: Candidate,
    used: Option<SystemTime>,
    modified: Option<SystemTime>,
    size: Option<u64>,
    is_dir: bool,
}

impl Entry {
    fn new(path: PathBuf, used: Option<SystemTime>) -> Self {
        let name = file_name(&path);
        let meta = std::fs::metadata(&path).ok();
        Self {
            candidate: Candidate::new(&name),
            modified: meta.as_ref().and_then(|m| m.modified().ok()),
            size: meta
                .as_ref()
                .filter(|m| m.is_file())
                .map(std::fs::Metadata::len),
            is_dir: meta.as_ref().is_some_and(std::fs::Metadata::is_dir),
            used,
            name,
            path,
        }
    }

    /// 排序与显示用的时间：最近使用优先，没有就用修改时间。
    fn when(&self) -> Option<SystemTime> {
        self.used.or(self.modified)
    }
}

#[derive(Default)]
struct State {
    /// 每次发起新搜索加一；后台线程发现自己不是最新的就停下。
    generation: u64,
    /// 正在跑的搜索的键。
    inflight: Option<String>,
    cache: VecDeque<(String, Instant, Arc<Vec<Entry>>)>,
}

impl State {
    fn fresh(&self, key: &str) -> Option<Arc<Vec<Entry>>> {
        self.cache
            .iter()
            .find(|(k, at, _)| k == key && at.elapsed() < CACHE_TTL)
            .map(|(_, _, entries)| entries.clone())
    }
}

#[derive(Default)]
struct Recent {
    roots: Vec<PathBuf>,
    at: Option<Instant>,
    entries: Arc<Vec<Entry>>,
}

struct Inner {
    backend: Box<dyn Backend>,
    state: Mutex<State>,
    ready: Condvar,
    recent: RwLock<Recent>,
    recent_loading: AtomicBool,
}

impl Inner {
    fn generation(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .generation
    }

    /// Spotlight（或测试后端）的结果；最多等 `wait`，等不到返回 None。
    fn search(
        self: &Arc<Self>,
        settings: &Settings,
        query: &str,
        wait: Duration,
    ) -> Option<Arc<Vec<Entry>>> {
        let key = format!("{:?}\n{query}", settings.roots);
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(hit) = state.fresh(&key) {
            return Some(hit);
        }
        if state.inflight.as_deref() != Some(key.as_str()) {
            state.generation += 1;
            state.inflight = Some(key.clone());
            let (inner, settings, query, key, generation) = (
                Arc::clone(self),
                settings.clone(),
                query.to_string(),
                key.clone(),
                state.generation,
            );
            std::thread::spawn(move || inner.run_search(&settings, &query, key, generation));
        }
        let deadline = Instant::now() + wait;
        loop {
            if let Some(hit) = state.fresh(&key) {
                return Some(hit);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            state = self
                .ready
                .wait_timeout(state, left)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }

    fn run_search(&self, settings: &Settings, query: &str, key: String, generation: u64) {
        std::thread::sleep(DEBOUNCE);
        let current = || self.generation() == generation;
        let cancelled = || !current();
        if cancelled() {
            return;
        }
        let mut paths = Vec::new();
        for root in &settings.roots {
            paths.extend(
                self.backend
                    .search(root, query, &cancelled, READ_CAP)
                    .into_iter()
                    .filter(|path| !excluded(path, root, &settings.rules)),
            );
        }
        if !current() {
            return;
        }
        paths.sort();
        paths.dedup();
        paths.truncate(KEEP_CAP);
        let entries: Vec<Entry> = paths
            .into_iter()
            .map(|path| Entry::new(path, None))
            .collect();
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.inflight.as_deref() == Some(key.as_str()) {
            state.inflight = None;
        }
        state.cache.retain(|(k, _, _)| k != &key);
        state
            .cache
            .push_front((key, Instant::now(), Arc::new(entries)));
        state.cache.truncate(CACHE_SIZE);
        drop(state);
        self.ready.notify_all();
    }

    fn load_recent(&self, settings: &Settings) {
        let mut entries: Vec<Entry> = Vec::new();
        for root in &settings.roots {
            entries.extend(
                self.backend
                    .recent(root, RECENT_DAYS, RECENT_CAP)
                    .into_iter()
                    .filter(|(path, _)| !excluded(path, root, &settings.rules))
                    .map(|(path, used)| Entry::new(path, used)),
            );
        }
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.when()));
        entries.dedup_by(|a, b| a.path == b.path);
        *self.recent.write().unwrap_or_else(|p| p.into_inner()) = Recent {
            roots: settings.roots.clone(),
            at: Some(Instant::now()),
            entries: Arc::new(entries),
        };
    }

    /// 最近文件；过期或范围变了就在后台重读（这次先用旧的）。
    fn recent(self: &Arc<Self>, settings: &Settings) -> Arc<Vec<Entry>> {
        let recent = self.recent.read().unwrap_or_else(|p| p.into_inner());
        let stale =
            recent.roots != settings.roots || recent.at.is_none_or(|at| at.elapsed() > RECENT_TTL);
        let entries = recent.entries.clone();
        drop(recent);
        if stale && !self.recent_loading.swap(true, Ordering::SeqCst) {
            let (inner, settings) = (Arc::clone(self), settings.clone());
            std::thread::spawn(move || {
                inner.load_recent(&settings);
                inner.recent_loading.store(false, Ordering::SeqCst);
            });
        }
        entries
    }
}

// ---------------------------------------------------------------- Provider

pub struct Files {
    inner: Arc<Inner>,
    settings: fn() -> Settings,
}

impl Files {
    pub fn new() -> Self {
        let backend: Box<dyn Backend> = match std::env::var("HELMP_FILES_BACKEND").as_deref() {
            Ok("walk") => Box::new(Walk),
            _ => Box::new(Spotlight),
        };
        Self::with_backend(backend, current_settings)
    }

    fn with_backend(backend: Box<dyn Backend>, settings: fn() -> Settings) -> Self {
        Self {
            inner: Arc::new(Inner {
                backend,
                state: Mutex::default(),
                ready: Condvar::new(),
                recent: RwLock::default(),
                recent_loading: AtomicBool::new(false),
            }),
            settings,
        }
    }

    fn wait(scoped: bool) -> Duration {
        std::env::var("HELMP_FILES_WAIT_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
            .map(Duration::from_millis)
            .unwrap_or(if scoped { SCOPED_WAIT } else { ROOT_WAIT })
    }

    /// 合并最近文件与 Spotlight 的命中，按分数（同分按时间新旧）排序。
    fn hits(&self, settings: &Settings, query: &str, scoped: bool) -> Vec<(Entry, u32)> {
        let recent = self.inner.recent(settings);
        let mut hits: HashMap<PathBuf, (Entry, u32)> = HashMap::new();
        for entry in recent.iter() {
            if let Some(score) = matcher::score_candidate(query, &entry.candidate) {
                hits.insert(entry.path.clone(), (entry.clone(), score + RECENT_BONUS));
            }
        }
        if let Some(found) = self.inner.search(settings, query, Self::wait(scoped)) {
            for entry in found.iter() {
                let score =
                    matcher::score_candidate(query, &entry.candidate).unwrap_or(SPOTLIGHT_ONLY);
                hits.entry(entry.path.clone())
                    .or_insert_with(|| (entry.clone(), score));
            }
        }
        let mut hits: Vec<(Entry, u32)> = hits.into_values().collect();
        hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.when().cmp(&a.0.when())));
        hits
    }
}

fn badge() -> &'static str {
    tr("文件", "Files")
}

impl Provider for Files {
    fn id(&self) -> &'static str {
        "files"
    }

    fn start(&self) -> Result<()> {
        self.inner.load_recent(&(self.settings)());
        Ok(())
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let mut items = Vec::new();
        if let Some(score) = [
            "文件",
            "找文件",
            "文件搜索",
            "最近文件",
            "file",
            "files",
            "find file",
            "recent files",
        ]
        .iter()
        .filter_map(|keyword| matcher::score(query, keyword))
        .max()
        {
            items.push(entry(score));
        }
        let settings = (self.settings)();
        if settings.in_root && query.chars().count() >= 2 {
            let home = crate::paths::home_dir().ok();
            items.extend(
                self.hits(&settings, query, false)
                    .into_iter()
                    .take(ROOT_LIMIT)
                    // 压在应用（700+）之下，又高于剪贴板顺带的结果。
                    .map(|(entry, score)| item(&entry, score * 3 / 5, home.as_deref())),
            );
        }
        items
    }

    fn scope(&self) -> Option<Scope> {
        Some(Scope {
            id: SCOPE,
            label: tr("文件", "Files").into(),
            tint: Tint::Blue,
            prefixes: ["f ", "file ", "文件 "].map(str::to_string).to_vec(),
            placeholder: tr(
                "搜索文件名 · 留空列出最近用过的文件",
                "Search file names · Leave empty for recent files",
            )
            .into(),
            filters: Vec::new(),
        })
    }

    fn query_scoped(&self, query: &str, _filter: &str) -> Vec<Item> {
        let query = query.trim();
        let settings = (self.settings)();
        let home = crate::paths::home_dir().ok();
        if query.is_empty() {
            return self
                .inner
                .recent(&settings)
                .iter()
                .take(SCOPED_LIMIT)
                .enumerate()
                // 保持最近使用的先后顺序（分数递减）。
                .map(|(rank, entry)| {
                    item(
                        entry,
                        900u32.saturating_sub(u32::try_from(rank).unwrap_or(900)),
                        home.as_deref(),
                    )
                })
                .collect();
        }
        self.hits(&settings, query, true)
            .into_iter()
            .take(SCOPED_LIMIT)
            .map(|(entry, score)| item(&entry, score, home.as_deref()))
            .collect()
    }
}

/// 根搜索里的「文件搜索」入口：进入文件范围。
fn entry(score: u32) -> Item {
    Item {
        id: "files:open".into(),
        title: tr("文件搜索", "Search Files").into(),
        subtitle: tr(
            "按文件名搜索，留空列出最近用过的文件",
            "Search by file name, or list recent files",
        )
        .into(),
        badge: crate::engine::commands_section().into(),
        score,
        icon: ICON,
        action: Action::enter_scope(SCOPE),
        action_title: tr("进入", "Open").into(),
        ..Item::blank()
    }
}

/// 把家目录显示成 `~`。
fn abbreviate(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

fn item(entry: &Entry, score: u32, home: Option<&Path>) -> Item {
    let path = entry.path.to_string_lossy().into_owned();
    let location = entry
        .path
        .parent()
        .map(|dir| abbreviate(dir, home))
        .unwrap_or_default();
    Item {
        id: format!("files:{path}"),
        title: entry.name.clone(),
        subtitle: location.clone(),
        badge: badge().into(),
        score,
        icon: Icon::File(entry.path.clone()),
        accessories: entry
            .when()
            .map(|when| Accessory::Tag {
                text: time_label(when, SystemTime::now()),
                tone: Tone::Neutral,
            })
            .into_iter()
            .collect(),
        action: Action::open(path.clone()),
        action_title: tr("打开", "Open").into(),
        actions: vec![
            NamedAction::new(
                tr("在访达中显示", "Show in Finder"),
                Some("⌘↵"),
                Action::reveal(path.clone()),
            ),
            NamedAction::new(tr("复制路径", "Copy Path"), Some("⌘⇧C"), Action::copy(path)),
        ],
        detail: Some(detail(entry, location)),
        ..Item::blank()
    }
}

fn detail(entry: &Entry, location: String) -> Detail {
    let ext = entry
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase());
    let kind = if entry.is_dir {
        tr("文件夹", "Folder").to_string()
    } else {
        ext.as_deref()
            .map(str::to_uppercase)
            .unwrap_or_else(|| tr("文件", "File").into())
    };
    let mut metadata = vec![(tr("类型", "Kind").into(), kind)];
    if let Some(size) = entry.size {
        metadata.push((tr("大小", "Size").into(), size_label(size)));
    }
    if let Some(modified) = entry.modified {
        metadata.push((tr("修改时间", "Modified").into(), full_time(modified)));
    }
    if let Some(used) = entry.used {
        metadata.push((tr("上次打开", "Last Opened").into(), full_time(used)));
    }
    metadata.push((tr("位置", "Where").into(), location));
    let preview = matches!(ext.as_deref(), Some("png" | "jpg" | "jpeg" | "gif"));
    Detail {
        icon: Icon::File(entry.path.clone()),
        title: entry.name.clone(),
        tag: None,
        blocks: if preview {
            vec![Block::Image(entry.path.clone())]
        } else {
            Vec::new()
        },
        metadata,
        actions: Vec::new(),
    }
}

fn size_label(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// 本地时间的 (年, 月, 日, 时, 分)。
fn local(time: SystemTime) -> Option<(i32, u32, u32, u32, u32)> {
    let secs = libc::time_t::try_from(time.duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()?;
    // SAFETY: localtime_r 只写入我们提供的 tm。
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return None;
    }
    Some((
        tm.tm_year + 1900,
        u32::try_from(tm.tm_mon + 1).ok()?,
        u32::try_from(tm.tm_mday).ok()?,
        u32::try_from(tm.tm_hour).ok()?,
        u32::try_from(tm.tm_min).ok()?,
    ))
}

/// 列表右侧的短时间：今天显示时刻，今年显示月日，更早显示年月日。
fn time_label(when: SystemTime, now: SystemTime) -> String {
    let (Some(t), Some(n)) = (local(when), local(now)) else {
        return String::new();
    };
    if (t.0, t.1, t.2) == (n.0, n.1, n.2) {
        format!("{:02}:{:02}", t.3, t.4)
    } else if t.0 == n.0 {
        trf!("{}月{}日", "{}/{}", t.1, t.2)
    } else {
        format!("{}-{:02}-{:02}", t.0, t.1, t.2)
    }
}

fn full_time(when: SystemTime) -> String {
    local(when)
        .map(|t| format!("{}-{:02}-{:02} {:02}:{:02}", t.0, t.1, t.2, t.3, t.4))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("helmp-files-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn touch(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "x").unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn spotlight_query_matches_each_word_and_escapes() {
        // 用户输入里的引号、星号不能把 Spotlight 查询语句弄坏（也不能被当成通配符）。
        assert_eq!(
            spotlight_query("rep"),
            r#"(kMDItemDisplayName == "rep*"cdw || kMDItemFSName == "*rep*"cd)"#
        );
        assert_eq!(
            spotlight_query(r#"a"b c*"#),
            r#"(kMDItemDisplayName == "a\"b*"cdw || kMDItemFSName == "*a\"b*"cd) && (kMDItemDisplayName == "c\**"cdw || kMDItemFSName == "*c\**"cd)"#
        );
        assert_eq!(spotlight_query("   "), "");
    }

    #[test]
    fn parses_last_used_dates_from_mdfind() {
        // 最近文件按上次打开时间排序，解析错了顺序就乱了。
        let (path, when) = parse_attr_line(
            "/Users/a/My Doc.pdf   kMDItemLastUsedDate = 2026-11-09 14:26:20 +0000",
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/Users/a/My Doc.pdf"));
        assert_eq!(
            when.unwrap().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            1_794_234_380
        );
        let (_, none) = parse_attr_line("/x   kMDItemLastUsedDate = (null)").unwrap();
        assert!(none.is_none());
        assert_eq!(
            parse_spotlight_date("2026-11-09 22:26:20 +0800"),
            parse_spotlight_date("2026-11-09 14:26:20 +0000")
        );
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }

    #[test]
    fn excludes_configured_hidden_and_app_paths() {
        // node_modules、target 里的成千上万个文件会把真正要找的挤出去；.app 由应用搜索负责。
        let root = Path::new("/Users/a");
        let rules = vec![
            Rule::Name("node_modules".into()),
            Rule::Name("target".into()),
            Rule::Prefix(PathBuf::from("/Users/a/Archive")),
        ];
        let ex = |p: &str| excluded(Path::new(p), root, &rules);
        assert!(ex("/Users/a/code/x/node_modules/react/README.md"));
        assert!(ex("/Users/a/code/helmp/target/release/helmp"));
        assert!(ex("/Users/a/Archive/old.pdf"));
        assert!(ex("/Users/a/.cache/report.pdf"));
        assert!(ex("/Users/a/Applications/Foo.app"));
        assert!(ex("/Users/a/Applications/Foo.app/Contents/Info.plist"));
        assert!(!ex("/Users/a/code/report.pdf"));
        assert!(
            !ex("/Users/a/.zshrc"),
            "隐藏文件本身可以搜到，只排除隐藏目录里的"
        );
        // 范围本身在隐藏目录里（如 git worktree）时不能把整个范围排除。
        assert!(!excluded(
            Path::new("/r/.hidden/root/a.md"),
            Path::new("/r/.hidden/root"),
            &rules
        ));
    }

    #[test]
    fn settings_expand_home_and_honor_override() {
        let config = config::FilesConfig {
            roots: vec!["~".into(), "~/Work".into(), "/Volumes/Data".into()],
            excluded: vec!["target".into(), "~/Archive".into(), " ".into()],
            in_root_search: true,
        };
        let home = Path::new("/Users/a");
        let settings = settings_from(&config, None, Some(home));
        assert_eq!(
            settings.roots,
            [
                PathBuf::from("/Users/a"),
                PathBuf::from("/Users/a/Work"),
                PathBuf::from("/Volumes/Data")
            ]
        );
        assert_eq!(
            settings.rules,
            [
                Rule::Name("target".into()),
                Rule::Prefix(PathBuf::from("/Users/a/Archive"))
            ]
        );
        let overridden = settings_from(&config, Some("/tmp/a:/tmp/b".into()), Some(home));
        assert_eq!(
            overridden.roots,
            [PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );
    }

    /// 单测直接把范围传给 `hits` / `search`，provider 自己的设置用不到。
    fn no_settings() -> Settings {
        Settings {
            roots: Vec::new(),
            rules: Vec::new(),
            in_root: true,
        }
    }

    #[test]
    fn finds_recent_files_by_pinyin_and_others_by_name() {
        // 最近改过的中文文件名要能用拼音首字母找到；名称子串走搜索后端；排除目录里的不出现。
        let dir = TempDir::new("search");
        let report = dir.touch("docs/测试报告.pdf");
        dir.touch("docs/notes.md");
        dir.touch("node_modules/notes.md");
        let settings = Settings {
            roots: vec![dir.0.clone()],
            rules: vec![Rule::Name("node_modules".into())],
            in_root: true,
        };
        let files = Files::with_backend(Box::new(Walk), no_settings);
        files.inner.load_recent(&settings);
        let hits = files.hits(&settings, "cesbg", true);
        assert_eq!(
            hits.first().map(|(e, _)| e.path.clone()),
            Some(report.clone())
        );
        let hits = files.hits(&settings, "notes", true);
        let paths: Vec<_> = hits.iter().map(|(e, _)| e.path.clone()).collect();
        assert_eq!(paths, [dir.0.join("docs/notes.md")]);

        let home = Some(dir.0.as_path());
        let item = item(&hits[0].0, 900, home);
        assert_eq!(item.subtitle, "~/docs");
        assert_eq!(
            item.id,
            format!("files:{}", dir.0.join("docs/notes.md").display())
        );
        let titles: Vec<_> = item.actions.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, ["在访达中显示", "复制路径"]);
    }

    /// 只记录查询、可以卡住的假后端：测防抖、取消与缓存。
    struct Slow {
        calls: Arc<Mutex<Vec<String>>>,
        cancelled: Arc<Mutex<Vec<String>>>,
    }

    impl Backend for Slow {
        fn search(
            &self,
            _: &Path,
            query: &str,
            cancelled: &(dyn Fn() -> bool + Sync),
            _: usize,
        ) -> Vec<PathBuf> {
            self.calls.lock().unwrap().push(query.to_string());
            for _ in 0..30 {
                if cancelled() {
                    self.cancelled.lock().unwrap().push(query.to_string());
                    return Vec::new();
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            vec![PathBuf::from(format!("/r/{query}.txt"))]
        }

        fn recent(&self, _: &Path, _: u32, _: usize) -> Vec<(PathBuf, Option<SystemTime>)> {
            Vec::new()
        }
    }

    #[test]
    fn typing_debounces_cancels_and_caches() {
        // 每按一个键都跑一次 mdfind 会拖慢整机；被新输入取代的搜索要尽快结束，同一查询不重复跑。
        let calls = Arc::new(Mutex::new(Vec::new()));
        let cancelled = Arc::new(Mutex::new(Vec::new()));
        let files = Files::with_backend(
            Box::new(Slow {
                calls: calls.clone(),
                cancelled: cancelled.clone(),
            }),
            no_settings,
        );
        let settings = Settings {
            roots: vec![PathBuf::from("/r")],
            rules: Vec::new(),
            in_root: true,
        };
        // 连续快速输入：前两个还在防抖期就被取代，后端根本不会被调用。
        assert!(files.inner.search(&settings, "r", Duration::ZERO).is_none());
        assert!(
            files
                .inner
                .search(&settings, "re", Duration::ZERO)
                .is_none()
        );
        // 第三个开始跑后被第四个取代：要被取消。
        assert!(
            files
                .inner
                .search(&settings, "rep", Duration::from_millis(200))
                .is_none()
        );
        let found = files
            .inner
            .search(&settings, "repo", Duration::from_secs(5))
            .unwrap();
        assert_eq!(found[0].path, PathBuf::from("/r/repo.txt"));
        assert_eq!(*calls.lock().unwrap(), ["rep", "repo"]);
        assert_eq!(*cancelled.lock().unwrap(), ["rep"]);
        // 同一查询再来一次走缓存，不再调用后端。
        files
            .inner
            .search(&settings, "repo", Duration::from_secs(1))
            .unwrap();
        assert_eq!(calls.lock().unwrap().len(), 2);
    }

    #[test]
    fn time_labels_are_short() {
        let now = SystemTime::now();
        let label = time_label(now, now);
        assert_eq!(label.len(), 5, "今天显示时刻：{label}");
        assert!(label.contains(':'));
        let old = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        assert!(time_label(old, now).starts_with("2020-09-1"));
        assert_eq!(size_label(999), "999 B");
        assert_eq!(size_label(1_500_000), "1.5 MB");
    }

    #[test]
    fn icon_exists_in_bundled_assets() {
        use gpui_kit::AssetSource as _;
        assert!(
            gpui_kit::assets::AllAssets
                .load("icons/file-search.svg")
                .unwrap()
                .is_some()
        );
    }
}
