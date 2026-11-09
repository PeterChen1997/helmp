//! 剪贴板历史：后台轮询采集、SQLite 持久化（无上限）、范围内浏览 / 搜索、选中写回并粘贴。
//!
//! 查询规则：
//! - 范围「剪贴板」（前缀 `cb ` / `clip ` / `剪贴板 ` / `剪切板 `，由框架解析）：
//!   查询为空时按时间列出（按天分组），否则在全部历史里做子串匹配；筛选 全部 / 文本 / 图片 / 文件。
//! - 全局搜索：查询 ≥ 3 个字符时，最多给 3 条首行高度匹配的历史，分数压在 300 以下，
//!   不淹没应用结果。
//! - 首页「快捷命令」里放「剪贴板历史」入口。
//!
//! 采集遵守 `config.clipboard`：enabled、retention_days、record_images、skip_concealed、
//! ignore_apps、auto_paste（环境变量 `HELMP_CLIPBOARD_AUTOPASTE` 可覆盖）。
//!
//! `query*()` 只读内存索引（每条最多 `PREVIEW_CHARS` 个字符的小写预览），不碰 SQLite。

pub mod macos;
mod present;
mod store;
mod watcher;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};

use crate::config::ClipboardConfig;
use crate::engine::{MAX_RESULTS, commands_section};
use crate::i18n::{Lang, tr, trf};
use crate::provider::{Accessory, Action, Icon, Item, NamedAction, Outcome, Provider, Scope, Tint};
use present::{Clock, TextKind};
use store::{Entry, Kind, NewClip, Source, Store};
use watcher::Read;

const PROVIDER_ID: &str = "clipboard";
const PREFIXES: [&str; 4] = ["cb ", "clip ", "剪贴板 ", "剪切板 "];
/// 分段筛选：(中文, 英文, 类型)。筛选名既是显示文字也是传回 `query_scoped` 的键，
/// 进入范围后可能切换语言，所以比较时中英两种名字都认。
const FILTERS: [(&str, &str, Option<Kind>); 4] = [
    ("全部", "All", None),
    ("文本", "Text", Some(Kind::Text)),
    ("图片", "Images", Some(Kind::Image)),
    ("文件", "Files", Some(Kind::Files)),
];
/// 内存索引里每条保留的字符数。超出部分不参与搜索（避免巨型文本拖慢每次按键）。
const PREVIEW_CHARS: usize = 4096;
const TITLE_CHARS: usize = 80;
/// 范围内的分数基线：引擎按分数排序（同分按标题），用「基线 − 名次」把我们自己的顺序编码进分数。
const BROWSE_BASE: u32 = 100_000;
/// 无前缀时的分数上限（不含），低于应用的子序列匹配分 400。
const AMBIENT_CEILING: u32 = 300;
const AMBIENT_LIMIT: usize = 3;
const AMBIENT_MIN_CHARS: usize = 3;
/// 首行档位，与 `matcher::score` 的完全相等 / 前缀 / 子串三档对齐。
const EXACT_TIER: u32 = 1000;
const PREFIX_TIER: u32 = 900;
const SUBSTRING_TIER: u32 = 700;
/// 正文命中但首行不命中时的档位（介于子串 700 与子序列 400 之间）。
const BODY_TIER: u32 = 600;
/// 首页「快捷命令」里的分数：排在「截图识别文字」（720）与翻译之后。
const SUGGESTION_SCORE: u32 = 700;
/// 覆盖 `clipboard.auto_paste`：`0` 只复制不粘贴（端到端测试用，免得往前台 App 里乱贴），`1` 强制粘贴。
const AUTOPASTE_ENV: &str = "HELMP_CLIPBOARD_AUTOPASTE";
/// 过期清理的间隔；保留天数改了会立即清理一次。
const PRUNE_INTERVAL: Duration = Duration::from_secs(3600);

/// 已启动的实例，供设置窗口「清空全部历史」使用（它拿不到 provider 对象）。
static SHARED: OnceLock<Arc<Inner>> = OnceLock::new();

pub struct Clipboard {
    inner: Arc<Inner>,
}

impl Clipboard {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner::default()),
        }
    }

    fn item(&self, indexed: &Indexed, score: u32, badge: String, view: &View) -> Item {
        let id = indexed.id;
        let pick = |mode: PasteMode| {
            let inner = Arc::clone(&self.inner);
            Action::new(move || inner.paste(id, mode))
        };
        let mut actions = vec![NamedAction::new(
            tr("复制", "Copy"),
            Some("⌘C"),
            pick(PasteMode::Copy),
        )];
        if indexed.kind != Kind::Image {
            actions.push(NamedAction::new(
                tr("粘贴为纯文本", "Paste as Plain Text"),
                Some("⌥↵"),
                pick(PasteMode::PlainText),
            ));
        }
        if indexed.text_kind == TextKind::Link
            && let Some(url) = present::link_url(&indexed.preview)
        {
            actions.push(NamedAction::new(
                tr("在浏览器中打开", "Open in Browser"),
                Some("⌘O"),
                Action::open(url),
            ));
        }
        if indexed.kind == Kind::Files
            && let Some(first) = indexed.preview.lines().next()
        {
            let path = first.to_string();
            actions.push(NamedAction::new(
                tr("在访达中显示", "Show in Finder"),
                Some("⌘↵"),
                Action::new(move || reveal_in_finder(&path)),
            ));
        }
        let inner = Arc::clone(&self.inner);
        actions.push(NamedAction::new(
            tr("删除", "Delete"),
            Some("⌘⌫"),
            // 删除后面板不隐藏、原地刷新列表。
            Action::new(move || {
                inner.delete(id)?;
                Ok(Outcome::Done)
            })
            .keep_open(),
        ));
        Item {
            id: format!("{PROVIDER_ID}:{id}"),
            title: indexed.title_in(crate::i18n::lang()),
            subtitle: present::subtitle(indexed, view.clock),
            badge,
            score,
            action: pick(PasteMode::Paste),
            action_title: view.action_title.clone(),
            icon: present::icon(indexed),
            actions,
            detail: Some(present::detail(indexed, view.clock)),
            ..Item::blank()
        }
    }
}

impl Provider for Clipboard {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn start(&self) -> Result<()> {
        let dir = crate::paths::data_dir()?;
        self.inner.open(Store::open(&dir)?)?;
        if SHARED.set(Arc::clone(&self.inner)).is_err() {
            eprintln!("helmp: 剪贴板 provider 被启动了两次，清空历史只作用于第一个实例");
        }

        let config = crate::config::get();
        let mut retention = config.clipboard.retention_days;
        self.inner.prune(&config.clipboard)?;
        let mut pruned_at = Instant::now();

        // 预热 NSWorkspace / 辅助功能查询：首次调用有上百毫秒的一次性初始化，别让第一次按键来付。
        let _ = View::now();

        let pasteboard = macos::SystemPasteboard;
        let mut last = None;
        // 启动时采集一次当前剪贴板。来源未知（此时前台是终端或启动器自己），记为空。
        self.inner.handle(
            watcher::poll(&pasteboard, &mut last, &config.clipboard, None),
            None,
        )?;

        let inner = Arc::clone(&self.inner);
        std::thread::Builder::new()
            .name("helmp-clipboard-watch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(watcher::POLL_INTERVAL);
                    let config = crate::config::get();
                    let config = &config.clipboard;
                    if retention != config.retention_days || pruned_at.elapsed() >= PRUNE_INTERVAL {
                        if let Err(err) = inner.prune(config) {
                            eprintln!("helmp: 清理过期剪贴板历史失败：{err:#}");
                        }
                        retention = config.retention_days;
                        pruned_at = Instant::now();
                    }
                    if last == Some(watcher::Pasteboard::change_count(&pasteboard)) {
                        continue;
                    }
                    let source = macos::frontmost_app();
                    let read = watcher::poll(&pasteboard, &mut last, config, source.as_ref());
                    if let Err(err) = inner.handle(read, source) {
                        eprintln!("helmp: 剪贴板采集失败：{err:#}");
                    }
                }
            })
            .context(tr(
                "启动剪贴板轮询线程",
                "Couldn’t start the clipboard polling thread",
            ))?;
        Ok(())
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let index = self.inner.index.read().expect("clipboard index poisoned");
        let hits = ambient(&index, query);
        if hits.is_empty() {
            return Vec::new();
        }
        let view = View::now();
        hits.into_iter()
            .map(|(position, score)| self.item(&index[position], score, badge().into(), &view))
            .collect()
    }

    fn scope(&self) -> Option<Scope> {
        let count = self
            .inner
            .index
            .read()
            .expect("clipboard index poisoned")
            .len();
        Some(Scope {
            id: PROVIDER_ID,
            label: tr("剪贴板", "Clipboard").into(),
            tint: Tint::Orange,
            prefixes: PREFIXES.iter().map(|prefix| prefix.to_string()).collect(),
            placeholder: if count == 1 {
                tr("搜索 1 条记录", "Search 1 item").into()
            } else {
                trf!(
                    "搜索 {} 条记录",
                    "Search {} items",
                    present::thousands(count)
                )
            },
            filters: FILTERS
                .iter()
                .map(|(zh, en, _)| tr(zh, en).to_string())
                .collect(),
        })
    }

    fn query_scoped(&self, query: &str, filter: &str) -> Vec<Item> {
        let index = self.inner.index.read().expect("clipboard index poisoned");
        let view = View::now();
        let query = query.trim();
        browse(&index, query, kind_filter(filter))
            .into_iter()
            .map(|(position, score)| {
                let indexed = &index[position];
                // 空查询按时间浏览，按天分组（设计稿的「今天」）；搜索时按匹配度排序，不再分天。
                let badge = if query.is_empty() {
                    present::day_label(view.clock, indexed.copied_at_ms)
                } else {
                    badge().into()
                };
                self.item(indexed, score, badge, &view)
            })
            .collect()
    }

    fn suggestions(&self) -> Vec<Item> {
        vec![Item {
            id: format!("{PROVIDER_ID}:open"),
            title: tr("剪贴板历史", "Clipboard History").into(),
            subtitle: tr("Clipboard History", "Browse and paste what you’ve copied").into(),
            badge: commands_section().into(),
            score: SUGGESTION_SCORE,
            icon: Icon::Symbol {
                name: "clipboard-list",
                tint: Tint::Orange,
            },
            accessories: vec![Accessory::keys("⌘⇧V")],
            action: Action::enter_scope(PROVIDER_ID),
            action_title: tr("打开", "Open").into(),
            ..Item::blank()
        }]
    }
}

/// 清空全部剪贴板历史（含图片文件），返回删除的条数。设置窗口「清空全部历史…」调用。
/// provider 已启动时同时清掉内存索引；未启动时直接清数据库。
// 设置窗口由另一个子任务接入（需要 providers/mod.rs 把 clipboard 改成 pub），接入前没有调用方。
#[allow(dead_code)]
pub fn clear_history() -> Result<usize> {
    match SHARED.get() {
        Some(inner) => inner.clear(),
        None => Store::open(&crate::paths::data_dir()?)?.clear(),
    }
}

/// 剪贴板历史的条数与磁盘占用（字节），给设置窗口「共 3,482 条 · 占用 46 MB」。
#[allow(dead_code)]
pub fn history_stats() -> Result<(usize, u64)> {
    let dir = crate::paths::data_dir()?;
    match SHARED.get() {
        Some(inner) => {
            let guard = inner.store.lock().expect("clipboard store poisoned");
            guard.as_ref().context(not_ready())?.stats(&dir)
        }
        None => Store::open(&dir)?.stats(&dir),
    }
}

/// 一次查询共用的上下文：时间、主操作的名字（粘贴到哪个 App）。
struct View {
    clock: Clock,
    action_title: String,
}

impl View {
    fn now() -> Self {
        let action_title = if will_autopaste(&crate::config::get().clipboard) {
            // 面板是非激活面板，前台应用就是用户正在用的 App。
            match macos::frontmost_app() {
                Some(app) => trf!("粘贴到{}", "Paste to {}", app.name),
                None => tr("粘贴到前台应用", "Paste to Frontmost App").into(),
            }
        } else {
            tr("复制到剪贴板", "Copy to Clipboard").into()
        };
        Self {
            clock: Clock::now(),
            action_title,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PasteMode {
    /// 按原格式写回并粘贴（主操作）。
    Paste,
    /// 只写回剪贴板。
    Copy,
    /// 以纯文本写回并粘贴（文件条目粘贴成路径）。
    PlainText,
}

#[derive(Default)]
struct Inner {
    /// `start` 之前为 `None`。
    store: Mutex<Option<Store>>,
    /// 新的在前。锁顺序：先 store 后 index。
    index: RwLock<Vec<Indexed>>,
}

impl Inner {
    fn open(&self, store: Store) -> Result<()> {
        let entries = store.load_all(PREVIEW_CHARS)?;
        let mut guard = self.store.lock().expect("clipboard store poisoned");
        *self.index.write().expect("clipboard index poisoned") = entries
            .iter()
            .map(|entry| Indexed::new(entry, image_path(&store, entry)))
            .collect();
        *guard = Some(store);
        Ok(())
    }

    fn handle(&self, read: Read, source: Option<Source>) -> Result<()> {
        match read {
            Read::Clip(clip) => self.record(clip, source.as_ref()),
            Read::Unchanged | Read::Private(_) | Read::Empty | Read::Ignored(_) => Ok(()),
        }
    }

    fn record(&self, clip: NewClip, source: Option<&Source>) -> Result<()> {
        let mut guard = self.store.lock().expect("clipboard store poisoned");
        let store = guard.as_mut().context(not_ready())?;
        let (entry, _) = store.add(clip, source, now_ms())?;
        self.move_to_front(&entry, image_path(store, &entry));
        Ok(())
    }

    fn move_to_front(&self, entry: &Entry, image_path: Option<PathBuf>) {
        let mut index = self.index.write().expect("clipboard index poisoned");
        index.retain(|indexed| indexed.id != entry.id);
        index.insert(0, Indexed::new(&truncated(entry), image_path));
    }

    /// 按保留天数删掉过期条目，返回删除条数。
    fn prune(&self, config: &ClipboardConfig) -> Result<usize> {
        let Some(cutoff) = retention_cutoff(config, now_ms()) else {
            return Ok(0);
        };
        let mut guard = self.store.lock().expect("clipboard store poisoned");
        let store = guard.as_mut().context(not_ready())?;
        let removed = store.prune_before(cutoff)?;
        if !removed.is_empty() {
            self.index
                .write()
                .expect("clipboard index poisoned")
                .retain(|indexed| !removed.contains(&indexed.id));
        }
        Ok(removed.len())
    }

    fn delete(&self, id: i64) -> Result<()> {
        let mut guard = self.store.lock().expect("clipboard store poisoned");
        let store = guard.as_mut().context(not_ready())?;
        anyhow::ensure!(store.delete(id)?, missing(id));
        self.index
            .write()
            .expect("clipboard index poisoned")
            .retain(|indexed| indexed.id != id);
        Ok(())
    }

    fn clear(&self) -> Result<usize> {
        let mut guard = self.store.lock().expect("clipboard store poisoned");
        let store = guard.as_mut().context(not_ready())?;
        let removed = store.clear()?;
        self.index
            .write()
            .expect("clipboard index poisoned")
            .clear();
        Ok(removed)
    }

    /// 选中某条：写回系统剪贴板、移到最前；需要粘贴且有辅助功能权限时再模拟 ⌘V。
    fn paste(&self, id: i64, mode: PasteMode) -> Result<Outcome> {
        let (entry, image_path) = {
            let guard = self.store.lock().expect("clipboard store poisoned");
            let store = guard.as_ref().context(not_ready())?;
            let entry = store.get(id)?.with_context(|| missing(id))?;
            let image_path = store.image_path(&entry);
            (entry, image_path)
        };
        let written = match mode {
            PasteMode::PlainText => as_plain_text(&entry)?,
            PasteMode::Paste | PasteMode::Copy => entry,
        };
        macos::write(&written, &image_path)?;
        {
            let mut guard = self.store.lock().expect("clipboard store poisoned");
            let store = guard.as_mut().context(not_ready())?;
            let touched = store.touch(id, now_ms())?;
            let path = self::image_path(store, &touched);
            self.move_to_front(&touched, path);
        }
        if mode != PasteMode::Copy && will_autopaste(&crate::config::get().clipboard) {
            // 等启动器面板隐藏、键盘焦点回到原来的 App。
            std::thread::sleep(Duration::from_millis(80));
            macos::send_paste()?;
        }
        Ok(Outcome::Done)
    }
}

fn image_path(store: &Store, entry: &Entry) -> Option<PathBuf> {
    (entry.kind == Kind::Image).then(|| store.image_path(entry))
}

/// 文本原样；文件变成每行一个路径；图片没有纯文本形式。
fn as_plain_text(entry: &Entry) -> Result<Entry> {
    anyhow::ensure!(
        entry.kind != Kind::Image,
        tr("图片没有纯文本形式", "Images have no plain-text form")
    );
    Ok(Entry {
        kind: Kind::Text,
        ..entry.clone()
    })
}

fn reveal_in_finder(path: &str) -> Result<Outcome> {
    let status = std::process::Command::new("open")
        .args(["-R", path])
        .status()
        .context(tr("启动 open", "Couldn’t run open"))?;
    anyhow::ensure!(
        status.success(),
        trf!(
            "在访达中显示 {path} 失败：{status}",
            "Couldn’t show {path} in Finder: {status}"
        )
    );
    Ok(Outcome::Done)
}

/// 自动粘贴开关：环境变量优先，其次配置；没有辅助功能权限时事件会被系统静默丢弃，所以也算关。
fn will_autopaste(config: &ClipboardConfig) -> bool {
    autopaste_wanted(config, std::env::var(AUTOPASTE_ENV).ok().as_deref())
        && macos::accessibility_trusted()
}

fn autopaste_wanted(config: &ClipboardConfig, env: Option<&str>) -> bool {
    match env {
        Some("0") => false,
        Some("1") => true,
        _ => config.auto_paste,
    }
}

/// 早于这个时间（Unix 毫秒）的条目已过保留期；`None` 表示永久保留。
fn retention_cutoff(config: &ClipboardConfig, now_ms: i64) -> Option<i64> {
    let days = i64::from(config.retention_days?);
    Some(now_ms - days * 86_400_000)
}

/// 筛选名 → 类型，中英两种名字都认；未知（如 `find` 重放时传空串）按「全部」。
fn kind_filter(filter: &str) -> Option<Kind> {
    FILTERS
        .iter()
        .find(|(zh, en, _)| filter == *zh || filter == *en)
        .and_then(|(_, _, kind)| *kind)
}

/// 分组名（也是分组键）：在查询里取，查询已按语言固定。
fn badge() -> &'static str {
    tr("剪贴板", "Clipboard")
}

fn not_ready() -> &'static str {
    tr("剪贴板存储未初始化", "Clipboard storage isn’t ready yet")
}

fn missing(id: i64) -> String {
    trf!(
        "剪贴板历史里没有第 {id} 条",
        "Clipboard history has no item #{id}"
    )
}

/// 内存索引中的一条。
#[derive(Debug, Clone)]
struct Indexed {
    id: i64,
    kind: Kind,
    /// 文本条目的细分（非文本为 `Plain`）。
    text_kind: TextKind,
    /// 标题里不随语言变的部分：文本首行、第一个文件名；图片和空白文本为空。显示用 `title_in`。
    title: String,
    /// 首行档位用的小写标题，固定按中文标题算（与界面语言无关，搜索结果不随切换语言变）。
    title_lower: String,
    /// 小写化的可搜索文本（截断到 `PREVIEW_CHARS`）。
    haystack: String,
    /// 详情区展示的原文（截断到 `present::DETAIL_CHARS`）；文件是每行一个路径。
    preview: String,
    /// 文本字数 / 文件个数。
    count: usize,
    image_path: Option<PathBuf>,
    image_format: &'static str,
    width: u32,
    height: u32,
    source: Option<String>,
    copied_at_ms: i64,
}

impl Indexed {
    fn new(entry: &Entry, image_path: Option<PathBuf>) -> Self {
        let title = title_of(entry);
        let (haystack, count) = match entry.kind {
            Kind::Text => (entry.content.to_lowercase(), entry.chars),
            Kind::Files => (entry.content.to_lowercase(), entry.content.lines().count()),
            Kind::Image => (String::new(), 0),
        };
        let preview = match entry.kind {
            Kind::Text | Kind::Files => prefix_chars(&entry.content, present::DETAIL_CHARS),
            Kind::Image => String::new(),
        };
        let text_kind = match entry.kind {
            Kind::Text => present::classify(&entry.content),
            Kind::Files | Kind::Image => TextKind::Plain,
        };
        let mut indexed = Self {
            id: entry.id,
            kind: entry.kind,
            text_kind,
            title_lower: String::new(),
            title,
            haystack,
            preview,
            count,
            image_format: match entry.kind {
                Kind::Image => present::image_format(std::path::Path::new(&entry.content)),
                Kind::Text | Kind::Files => "",
            },
            image_path,
            width: entry.width,
            height: entry.height,
            source: entry.source.as_ref().map(|source| source.name.clone()),
            copied_at_ms: entry.copied_at_ms,
        };
        indexed.title_lower = indexed.title_in(Lang::Zh).to_lowercase();
        if entry.kind == Kind::Image {
            // 图片没有正文，只能按标题搜：中英两种叫法（「图片」/ image）都要搜得到。
            indexed.haystack = format!(
                "{}\n{}",
                indexed.title_lower,
                indexed.title_in(Lang::En).to_lowercase()
            );
        }
        indexed
    }

    /// 列表标题：按语言补上「（空白）」「等 N 个文件」「图片 W × H」。
    /// 渲染时现取，不缓存，切换语言后立即生效。
    fn title_in(&self, lang: Lang) -> String {
        let zh = lang == Lang::Zh;
        match self.kind {
            Kind::Text if self.title.is_empty() => if zh { "（空白）" } else { "(Blank)" }.into(),
            Kind::Text => self.title.clone(),
            Kind::Files if self.count > 1 => {
                if zh {
                    format!("{} 等 {} 个文件", self.title, self.count)
                } else {
                    format!("{} and {} more", self.title, self.count - 1)
                }
            }
            Kind::Files => self.title.clone(),
            Kind::Image if self.width > 0 && self.height > 0 => {
                let label = if zh { "图片" } else { "Image" };
                format!("{label} {} × {}", self.width, self.height)
            }
            Kind::Image => if zh { "图片" } else { "Image" }.into(),
        }
    }
}

fn prefix_chars(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((cut, _)) => text[..cut].to_string(),
        None => text.to_string(),
    }
}

/// 只保留预览长度，避免巨型文本常驻内存。
fn truncated(entry: &Entry) -> Entry {
    let mut entry = entry.clone();
    if let Some((cut, _)) = entry.content.char_indices().nth(PREVIEW_CHARS) {
        entry.content.truncate(cut);
    }
    entry
}

/// 标题里不随语言变的部分（见 `Indexed::title`）。
fn title_of(entry: &Entry) -> String {
    match entry.kind {
        Kind::Text => entry
            .content
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|line| ellipsize(line, TITLE_CHARS))
            .unwrap_or_default(),
        Kind::Files => {
            let first = entry.content.lines().next().unwrap_or_default();
            std::path::Path::new(first)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| first.to_string())
        }
        Kind::Image => String::new(),
    }
}

fn ellipsize(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

/// 范围内：空查询按时间列出，否则全部历史子串匹配。返回 (索引下标, 分数)，已按想要的顺序编码进分数。
fn browse(index: &[Indexed], text: &str, kind: Option<Kind>) -> Vec<(usize, u32)> {
    let wanted = |indexed: &Indexed| kind.is_none_or(|kind| indexed.kind == kind);
    let positions: Vec<usize> = if text.is_empty() {
        (0..index.len())
            .filter(|&position| wanted(&index[position]))
            .take(MAX_RESULTS)
            .collect()
    } else {
        let mut hits: Vec<(usize, u32)> = matches(index, text, BODY_TIER)
            .into_iter()
            .filter(|&(position, _)| wanted(&index[position]))
            .collect();
        // 稳定排序：同档位保持新的在前。
        hits.sort_by_key(|&(_, tier)| std::cmp::Reverse(tier));
        hits.truncate(MAX_RESULTS);
        hits.into_iter().map(|(position, _)| position).collect()
    };
    positions
        .into_iter()
        .enumerate()
        .map(|(rank, position)| (position, browse_score(rank)))
        .collect()
}

/// 全局搜索里顺带给的几条：只要首行就命中（子串及以上）的，正文命中不算「高度匹配」。
fn ambient(index: &[Indexed], query: &str) -> Vec<(usize, u32)> {
    let text = query.trim();
    if text.chars().count() < AMBIENT_MIN_CHARS {
        return Vec::new();
    }
    let mut hits: Vec<(usize, u32)> = matches(index, text, 0)
        .into_iter()
        .filter(|&(_, tier)| tier >= SUBSTRING_TIER)
        .collect();
    hits.sort_by_key(|&(_, tier)| std::cmp::Reverse(tier));
    hits.truncate(AMBIENT_LIMIT);
    hits.into_iter()
        .enumerate()
        .map(|(rank, (position, tier))| {
            // 700..=1000 映射到 250..=280，再按名次递减，保证 < 300。
            let score = 250 + (tier - SUBSTRING_TIER) / 10 - rank as u32;
            debug_assert!(score < AMBIENT_CEILING);
            (position, score)
        })
        .collect()
}

/// 子串预筛（小写预览里包含查询），再给首行打档位；首行不含查询时用 `body_tier`。
///
/// 没有直接调 `matcher::score`：它每次都把候选串小写化并分配内存，十万条全命中时要 ~40 ms；
/// 这里用预先小写好的首行做同样的三档判断（略去首字母档，对剪贴板内容意义不大），降到个位数毫秒。
fn matches(index: &[Indexed], text: &str, body_tier: u32) -> Vec<(usize, u32)> {
    let needle = text.to_lowercase();
    index
        .iter()
        .enumerate()
        .filter(|(_, indexed)| indexed.haystack.contains(&needle))
        .map(|(position, indexed)| {
            let title = indexed.title_lower.as_str();
            let tier = if title == needle {
                EXACT_TIER
            } else if title.starts_with(&needle) {
                PREFIX_TIER
            } else if title.contains(&needle) {
                SUBSTRING_TIER
            } else {
                body_tier
            };
            (position, tier)
        })
        .collect()
}

fn browse_score(rank: usize) -> u32 {
    BROWSE_BASE.saturating_sub(u32::try_from(rank).unwrap_or(BROWSE_BASE))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::store::ImageFormat;
    use super::store::tests::TempDir;
    use super::watcher::tests::FakePasteboard;
    use super::*;
    use crate::provider::{Block, Detail};

    fn provider_with(dir: &TempDir, clips: &[&str]) -> Clipboard {
        let provider = Clipboard::new();
        provider.inner.open(Store::open(&dir.0).unwrap()).unwrap();
        for clip in clips {
            provider
                .inner
                .record(NewClip::Text(clip.to_string()), None)
                .unwrap();
        }
        provider
    }

    fn titles(items: &[Item]) -> Vec<&str> {
        items.iter().map(|item| item.title.as_str()).collect()
    }

    fn png(provider: &Clipboard, bytes: &[u8], width: u32, height: u32, source: Option<Source>) {
        let pb = FakePasteboard::default();
        pb.change.set(7);
        *pb.types.borrow_mut() = vec!["public.png".into()];
        *pb.image.borrow_mut() = Some(watcher::Image {
            bytes: bytes.to_vec(),
            format: ImageFormat::Png,
            width,
            height,
        });
        let mut last = None;
        let read = watcher::poll(&pb, &mut last, &ClipboardConfig::default(), None);
        provider.inner.handle(read, source).unwrap();
    }

    fn files(provider: &Clipboard, paths: &[&str]) {
        provider
            .inner
            .record(
                NewClip::Files(paths.iter().map(PathBuf::from).collect()),
                None,
            )
            .unwrap();
    }

    #[test]
    fn scope_declares_prefixes_filters_and_live_count() {
        // 前缀解析交给框架：前缀必须带结尾空格，否则输入 `cbr`、`clipboard` 会被误判成进入剪贴板。
        let dir = TempDir::new("scope");
        let provider = provider_with(&dir, &["a", "b", "c"]);
        let scope = provider.scope().unwrap();
        assert_eq!(scope.id, "clipboard");
        assert_eq!(scope.tint, Tint::Orange);
        assert!(scope.prefixes.iter().all(|prefix| prefix.ends_with(' ')));
        assert!(scope.prefixes.contains(&"剪切板 ".to_string()));
        assert_eq!(scope.placeholder, "搜索 3 条记录");
        assert_eq!(scope.filters, ["全部", "文本", "图片", "文件"]);
    }

    #[test]
    fn empty_scoped_query_lists_history_newest_first_grouped_by_day() {
        // 进入范围不输入就是「看看刚才复制了什么」，最新的必须在最上面，并归在「今天」下。
        let dir = TempDir::new("recent");
        let clips: Vec<String> = (0..60).map(|i| format!("clip {i}")).collect();
        let refs: Vec<&str> = clips.iter().map(String::as_str).collect();
        let provider = provider_with(&dir, &refs);
        let mut items = provider.query_scoped("", "全部");
        assert_eq!(items.len(), 60);
        // 模拟引擎的排序（分数降序、同分按标题），顺序必须不被打乱。
        items.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.title.cmp(&b.title)));
        assert_eq!(items[0].title, "clip 59");
        assert_eq!(items[59].title, "clip 0");
        assert!(items.iter().all(|item| item.badge == "今天"));
        assert!(items.iter().all(|item| item.id.starts_with("clipboard:")));
    }

    #[test]
    fn scoped_filters_narrow_by_kind() {
        // 分段筛选 文本 / 图片 / 文件：选「图片」时不能混进文字，否则筛选形同虚设。
        let dir = TempDir::new("filter");
        let provider = provider_with(&dir, &["hello text"]);
        png(&provider, b"\x89PNG a", 640, 220, None);
        files(&provider, &["/tmp/Q4 预算.xlsx"]);
        assert_eq!(provider.query_scoped("", "全部").len(), 3);
        assert_eq!(titles(&provider.query_scoped("", "文本")), ["hello text"]);
        assert_eq!(
            titles(&provider.query_scoped("", "图片")),
            ["图片 640 × 220"]
        );
        assert_eq!(titles(&provider.query_scoped("", "文件")), ["Q4 预算.xlsx"]);
        // 搜索与筛选叠加。
        assert!(provider.query_scoped("hello", "文件").is_empty());
        // 未知筛选（如 `find` 重放时传空串）按「全部」处理。
        assert_eq!(provider.query_scoped("", "").len(), 3);
    }

    #[test]
    fn scoped_search_covers_whole_history_with_title_hits_first() {
        let dir = TempDir::new("browse");
        let provider = provider_with(
            &dir,
            &[
                "deploy script\nsecond line",
                "foo",
                "第一行\n正文里有 deploy",
            ],
        );
        let items = provider.query_scoped(" deploy ", "全部");
        // 首行命中排在正文命中之前；正文第二行的命中也要找得到。
        assert_eq!(titles(&items), vec!["deploy script", "第一行"]);
        assert!(items.iter().all(|item| item.badge == badge()));
    }

    #[test]
    fn ambient_needs_three_chars_and_stays_below_apps() {
        // 不带前缀时剪贴板只是「顺带」，不能把应用结果挤下去。
        let dir = TempDir::new("ambient");
        let provider = provider_with(
            &dir,
            &[
                "safari tips 1",
                "safari tips 2",
                "safari tips 3",
                "safari tips 4",
                "body\nsafari",
            ],
        );
        assert!(provider.query("sa").is_empty());
        let items = provider.query("safari");
        assert_eq!(items.len(), AMBIENT_LIMIT);
        assert!(items.iter().all(|item| item.score < AMBIENT_CEILING));
        assert!(items.iter().all(|item| item.badge == badge()));
        // 正文才命中的不算高度匹配。
        assert!(!titles(&items).contains(&"body"));
        // 新的在前。
        assert_eq!(items[0].title, "safari tips 4");
        assert!(items[0].score > items[1].score && items[1].score > items[2].score);
        // 前缀由框架剥掉，全局搜索里不再自己认 `cb `。
        assert!(provider.query("cb ").is_empty());
    }

    #[test]
    fn concealed_clip_never_reaches_disk_or_results() {
        // 端到端地走一遍采集管线：密码管理器的内容既不能入库也不能被搜到。
        let dir = TempDir::new("concealed");
        let provider = provider_with(&dir, &[]);
        let pb = FakePasteboard::default();
        pb.set_text("hunter2-secret", &["org.nspasteboard.ConcealedType"]);
        let mut last = None;
        let read = watcher::poll(&pb, &mut last, &ClipboardConfig::default(), None);
        provider.inner.handle(read, None).unwrap();
        assert!(provider.query_scoped("", "全部").is_empty());
        let store = Store::open(&dir.0).unwrap();
        assert!(store.load_all(100).unwrap().is_empty());
    }

    #[test]
    fn duplicate_copy_moves_to_front_in_index() {
        let dir = TempDir::new("dupindex");
        let provider = provider_with(&dir, &["a", "b", "a"]);
        let items = provider.query_scoped("", "全部");
        assert_eq!(titles(&items), vec!["a", "b"]);
    }

    #[test]
    fn text_item_has_icon_subtitle_detail_and_actions() {
        // 设计稿：文本图标 + 「来源 · 时间」副标题，详情是全文 + 来源 / 复制于 / 内容，
        // ⌘K 里有复制、纯文本粘贴、删除。
        let dir = TempDir::new("textitem");
        let provider = provider_with(&dir, &[]);
        let wechat = Source {
            name: "微信".into(),
            bundle_id: Some("com.tencent.xinWeChat".into()),
        };
        provider
            .inner
            .record(NewClip::Text("会议改到周四下午三点".into()), Some(&wechat))
            .unwrap();
        let item = provider.query_scoped("", "全部").remove(0);
        assert_eq!(item.subtitle, "微信 · 刚刚");
        assert_eq!(
            item.icon,
            Icon::Symbol {
                name: "text-align-start",
                tint: Tint::Gray
            }
        );
        assert!(
            item.action_title.starts_with("粘贴到") || item.action_title == "复制到剪贴板",
            "{}",
            item.action_title
        );
        let detail = item.detail.expect("详情");
        assert_eq!(
            detail.blocks,
            vec![Block::Text("会议改到周四下午三点".into())]
        );
        let meta: Vec<(&str, &str)> = detail
            .metadata
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(meta[0], ("来源", "微信"));
        assert_eq!(meta[1].0, "复制于");
        assert!(meta[1].1.starts_with("今天 "), "{}", meta[1].1);
        assert_eq!(meta[2], ("内容", "纯文本 · 10 字"));
        let actions: Vec<(&str, Option<&str>)> = item
            .actions
            .iter()
            .map(|a| (a.title.as_str(), a.shortcut.as_deref()))
            .collect();
        assert_eq!(
            actions,
            [
                ("复制", Some("⌘C")),
                ("粘贴为纯文本", Some("⌥↵")),
                ("删除", Some("⌘⌫"))
            ]
        );
    }

    #[test]
    fn long_text_detail_is_truncated_with_total_count() {
        // 几十万字的日志也可能被复制；详情区全量渲染会卡住面板，截断并说明总字数。
        let dir = TempDir::new("long");
        let long = "字".repeat(5000);
        let provider = provider_with(&dir, &[long.as_str()]);
        let detail = provider.query_scoped("", "全部").remove(0).detail.unwrap();
        let Some(Block::Text(body)) = detail.blocks.first() else {
            panic!("{detail:?}");
        };
        assert_eq!(body.chars().count(), present::DETAIL_CHARS);
        assert_eq!(
            detail.blocks.get(1),
            Some(&Block::Text(format!(
                "……共 5,000 字，这里只显示前 {} 字",
                present::DETAIL_CHARS
            )))
        );
    }

    #[test]
    fn image_item_shows_thumbnail_and_image_detail() {
        // 图片不进数据库正文而是落盘：库保持小，写回时按原格式读文件；列表用缩略图，详情显示原图。
        let dir = TempDir::new("image");
        let provider = provider_with(&dir, &[]);
        let source = Source {
            name: "截图".into(),
            bundle_id: Some("com.apple.screenshot.launcher".into()),
        };
        png(&provider, b"\x89PNG fake", 640, 220, Some(source));

        let item = provider.query_scoped("", "全部").remove(0);
        assert_eq!(item.title, "图片 640 × 220");
        assert_eq!(item.subtitle, "截图 · 刚刚");
        let Icon::Image(thumb) = &item.icon else {
            panic!("图片条目应显示缩略图：{:?}", item.icon);
        };
        assert_eq!(std::fs::read(thumb).unwrap(), b"\x89PNG fake");
        let detail: Detail = item.detail.unwrap();
        assert_eq!(detail.blocks, vec![Block::Image(thumb.clone())]);
        assert_eq!(
            detail.metadata[2],
            ("内容".to_string(), "PNG 图片 · 640 × 220".to_string())
        );
        // 图片没有纯文本形式，不提供「粘贴为纯文本」。
        assert!(item.actions.iter().all(|a| a.title != "粘贴为纯文本"));
    }

    #[test]
    fn files_and_links_get_their_own_icons_and_actions() {
        let dir = TempDir::new("files");
        let provider = provider_with(
            &dir,
            &["github.com/zed-industries/zed", "git rebase -i HEAD~3"],
        );
        files(&provider, &["/tmp/a.txt", "/tmp/b.txt"]);
        let items = provider.query_scoped("", "全部");
        let by_title = |title: &str| items.iter().find(|i| i.title == title).unwrap();

        let files = by_title("a.txt 等 2 个文件");
        assert!(files.subtitle.ends_with("· 2 个文件"), "{}", files.subtitle);
        assert_eq!(
            files.icon,
            Icon::Symbol {
                name: "files",
                tint: Tint::Gray
            }
        );
        assert!(files.actions.iter().any(|a| a.title == "在访达中显示"));

        let link = by_title("github.com/zed-industries/zed");
        assert_eq!(
            link.icon,
            Icon::Symbol {
                name: "link",
                tint: Tint::Gray
            }
        );
        assert!(link.actions.iter().any(|a| a.title == "在浏览器中打开"));

        let command = by_title("git rebase -i HEAD~3");
        assert_eq!(
            command.detail.as_ref().unwrap().blocks[0],
            Block::Code("git rebase -i HEAD~3".into())
        );
    }

    #[test]
    fn delete_removes_from_store_and_index() {
        // ⌘⌫ 删除要真的从库里删：只从列表隐藏的话，重启后又冒出来，敏感内容也还在磁盘上。
        let dir = TempDir::new("delete");
        let provider = provider_with(&dir, &["keep", "drop me"]);
        let item = provider
            .query_scoped("drop", "全部")
            .into_iter()
            .next()
            .unwrap();
        let delete = item.actions.iter().find(|a| a.title == "删除").unwrap();
        // 删除后面板原地刷新（keep_open），不再先隐藏再重开、也不清空查询。
        assert!(delete.action.keeps_open());
        assert_eq!(delete.action.run().unwrap(), Outcome::Done);
        assert_eq!(titles(&provider.query_scoped("", "全部")), ["keep"]);
        let store = Store::open(&dir.0).unwrap();
        let left: Vec<String> = store
            .load_all(100)
            .unwrap()
            .into_iter()
            .map(|e| e.content)
            .collect();
        assert_eq!(left, ["keep"]);
        // 再删一次要报错，而不是静默成功。
        assert!(delete.action.run().is_err());
    }

    #[test]
    fn retention_prunes_index_and_store() {
        // 保留天数到期的条目要同时从搜索结果和磁盘消失；None（永久）不能删任何东西。
        let dir = TempDir::new("retention");
        let provider = provider_with(&dir, &[]);
        {
            let mut guard = provider.inner.store.lock().unwrap();
            let store = guard.as_mut().unwrap();
            let old = now_ms() - 10 * 86_400_000;
            let (entry, _) = store
                .add(NewClip::Text("ancient".into()), None, old)
                .unwrap();
            provider.inner.move_to_front(&entry, None);
        }
        provider
            .inner
            .record(NewClip::Text("fresh".into()), None)
            .unwrap();

        let forever = ClipboardConfig::default();
        assert_eq!(retention_cutoff(&forever, 1_000), None);
        assert_eq!(provider.inner.prune(&forever).unwrap(), 0);

        let week = ClipboardConfig {
            retention_days: Some(7),
            ..ClipboardConfig::default()
        };
        assert_eq!(retention_cutoff(&week, 8 * 86_400_000), Some(86_400_000));
        assert_eq!(provider.inner.prune(&week).unwrap(), 1);
        assert_eq!(titles(&provider.query_scoped("", "全部")), ["fresh"]);
        assert_eq!(Store::open(&dir.0).unwrap().load_all(10).unwrap().len(), 1);
    }

    #[test]
    fn clear_empties_store_and_index() {
        // 设置里的「清空全部历史」：面板上必须同时清空，不能等重启。
        let dir = TempDir::new("clearall");
        let provider = provider_with(&dir, &["a", "b"]);
        assert_eq!(provider.inner.clear().unwrap(), 2);
        assert!(provider.query_scoped("", "全部").is_empty());
        assert_eq!(provider.scope().unwrap().placeholder, "搜索 0 条记录");
    }

    #[test]
    fn autopaste_env_overrides_config() {
        // 环境变量是端到端测试的保险：设成 0 时无论配置如何都不能往前台 App 里贴。
        let on = ClipboardConfig::default();
        let off = ClipboardConfig {
            auto_paste: false,
            ..ClipboardConfig::default()
        };
        assert!(autopaste_wanted(&on, None));
        assert!(!autopaste_wanted(&off, None));
        assert!(!autopaste_wanted(&on, Some("0")));
        assert!(autopaste_wanted(&off, Some("1")));
        assert!(autopaste_wanted(&on, Some("garbage")));
    }

    #[test]
    fn suggestion_enters_clipboard_scope() {
        // 首页「快捷命令」的入口：回车进入剪贴板范围，而不是执行某条历史。
        let provider = Clipboard::new();
        let items = provider.suggestions();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.badge, commands_section());
        assert!(item.score < 800);
        assert_eq!(item.accessories, vec![Accessory::keys("⌘⇧V")]);
        assert_eq!(item.action.run().unwrap(), Outcome::EnterScope("clipboard"));
    }

    #[test]
    fn symbol_icons_exist_in_bundled_assets() {
        // 图标名拼错时界面只会画一个空白方块，不会报错；这里把用到的名字逐个对一遍资源包。
        use gpui_kit::AssetSource as _;
        for name in [
            "clipboard-list",
            "text-align-start",
            "link",
            "square-terminal",
            "file",
            "files",
            "image",
        ] {
            let path = format!("icons/{name}.svg");
            assert!(
                gpui_kit::assets::AllAssets.load(&path).unwrap().is_some(),
                "缺少图标 {path}"
            );
        }
    }

    #[test]
    fn query_is_fast_on_large_history() {
        // query 在每次按键时同步调用，十万条历史下也要毫秒级。
        let dir = TempDir::new("perf");
        let provider = provider_with(&dir, &[]);
        {
            let mut index = provider.inner.index.write().unwrap();
            for i in 0..100_000 {
                let content = format!(
                    "第 {i} 条剪贴板记录 some english words for realism {}\nsecond line",
                    i * 7919
                );
                index.push(Indexed::new(
                    &Entry {
                        id: i,
                        kind: Kind::Text,
                        chars: content.chars().count(),
                        content,
                        width: 0,
                        height: 0,
                        source: None,
                        copied_at_ms: 0,
                        seq: i,
                    },
                    None,
                ));
            }
        }
        // NSWorkspace / 辅助功能查询第一次调用有上百毫秒的一次性初始化（`start` 里预热），
        // 这里测的是每次按键的稳态耗时。
        let _ = View::now();
        for (query, scoped) in [
            ("", true),
            ("realism", true),
            ("99999", true),
            ("不存在的内容", true),
            ("words for", false),
        ] {
            let started = std::time::Instant::now();
            let items = if scoped {
                provider.query_scoped(query, "全部")
            } else {
                provider.query(query)
            };
            let elapsed = started.elapsed();
            eprintln!(
                "100k 条历史 query({query:?}, scoped={scoped}) → {} 条，{elapsed:?}",
                items.len()
            );
            assert!(
                elapsed < Duration::from_millis(100),
                "{query:?} 耗时 {elapsed:?}"
            );
        }
    }

    #[test]
    fn titles_for_each_kind() {
        let entry = |kind, content: &str| Entry {
            id: 1,
            kind,
            content: content.into(),
            chars: content.chars().count(),
            width: 0,
            height: 0,
            source: None,
            copied_at_ms: 0,
            seq: 1,
        };
        let title =
            |kind, content: &str| Indexed::new(&entry(kind, content), None).title_in(Lang::Zh);
        assert_eq!(title(Kind::Text, "\n  第一行  \n第二行"), "第一行");
        assert_eq!(title(Kind::Text, " \n "), "（空白）");
        assert_eq!(
            title(Kind::Text, &"字".repeat(100)),
            format!("{}…", "字".repeat(TITLE_CHARS))
        );
        assert_eq!(title(Kind::Files, "/a/b/报告.pdf"), "报告.pdf");
        assert_eq!(
            title(Kind::Files, "/a/x.txt\n/a/y.txt"),
            "x.txt 等 2 个文件"
        );
    }

    #[test]
    fn english_ui_labels_filters_and_days_while_search_stays_the_same() {
        // 英文界面：分组（Today）、筛选名、动作名、标题都要是英文；
        // 筛选名会原样传回，进入范围后切了语言，中英两种名字都得认，否则筛选失效。
        // 搜索与界面语言无关：中文内容照样能搜到，图片在英文界面下按 image 也能搜到。
        let dir = TempDir::new("english");
        let provider = provider_with(&dir, &["会议改到周四下午三点"]);
        png(&provider, b"\x89PNG en", 640, 220, None);
        files(&provider, &["/tmp/a.txt", "/tmp/b.txt"]);
        crate::i18n::with_lang(Lang::En, || {
            let scope = provider.scope().unwrap();
            assert_eq!(scope.label, "Clipboard");
            assert_eq!(scope.filters, ["All", "Text", "Images", "Files"]);
            assert_eq!(scope.placeholder, "Search 3 items");

            let items = provider.query_scoped("", "All");
            assert_eq!(items.len(), 3);
            assert!(items.iter().all(|item| item.badge == "Today"));
            let image = &provider.query_scoped("", "Images")[0];
            assert_eq!(image.title, "Image 640 × 220");
            assert_eq!(image.subtitle, "Unknown source · Just now");
            // 进入范围时还是中文，筛选名「图片」原样传回来。
            assert_eq!(
                titles(&provider.query_scoped("", "图片")),
                ["Image 640 × 220"]
            );
            assert_eq!(
                titles(&provider.query_scoped("", "Files")),
                ["a.txt and 1 more"]
            );

            let found = provider.query_scoped("周四", "All");
            assert_eq!(titles(&found), ["会议改到周四下午三点"]);
            assert_eq!(found[0].badge, "Clipboard");
            let actions: Vec<&str> = found[0].actions.iter().map(|a| a.title.as_str()).collect();
            assert_eq!(actions, ["Copy", "Paste as Plain Text", "Delete"]);
            assert_eq!(
                titles(&provider.query_scoped("image", "All")),
                ["Image 640 × 220"]
            );
            assert_eq!(
                titles(&provider.query_scoped("图片", "All")),
                ["Image 640 × 220"]
            );
        });
        // 中文界面下用英文筛选名（语言从英文切回中文时）也要生效。
        assert_eq!(
            titles(&provider.query_scoped("", "Images")),
            ["图片 640 × 220"]
        );
    }
}
