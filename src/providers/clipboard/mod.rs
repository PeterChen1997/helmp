//! 剪贴板历史：后台轮询采集、SQLite 持久化（无上限）、前缀查询、选中写回并粘贴。
//!
//! 查询规则：
//! - `cb ` / `clip ` / `剪贴板 ` / `剪切板 ` 前缀进入剪贴板模式：后面为空列最近 50 条，
//!   否则在全部历史里做子串匹配，结果排在其他 provider 之前。
//! - 不带前缀：查询 ≥ 3 个字符时，最多给 3 条首行高度匹配的历史，分数压在 300 以下，
//!   不淹没应用结果。
//!
//! `query()` 只读内存索引（每条最多 `PREVIEW_CHARS` 个字符的小写预览），不碰 SQLite。

mod macos;
mod store;
mod watcher;

use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};

use crate::engine::MAX_RESULTS;
use crate::provider::{Action, Item, Outcome, Provider};
use store::{Entry, Kind, NewClip, Source, Store};
use watcher::Read;

const PROVIDER_ID: &str = "clipboard";
const BADGE: &str = "剪贴板";
const PREFIXES: &[&str] = &["cb", "clip", "剪贴板", "剪切板"];
/// 前缀后为空时列出的条数。
const RECENT_LIMIT: usize = 50;
/// 内存索引里每条保留的字符数。超出部分不参与搜索（避免巨型文本拖慢每次按键）。
const PREVIEW_CHARS: usize = 4096;
const TITLE_CHARS: usize = 80;
/// 剪贴板模式下的分数基线：远高于 matcher 的 0–1000，保证排在其他 provider 之前；
/// 引擎按分数排序（同分按标题），所以用「基线 − 名次」把我们自己的顺序编码进分数。
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
/// 设为 `0` 时选中只复制不粘贴（端到端测试用，免得往前台 App 里乱贴）。
const AUTOPASTE_ENV: &str = "HELMP_CLIPBOARD_AUTOPASTE";

pub struct Clipboard {
    inner: Arc<Inner>,
}

impl Clipboard {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner::default()),
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

        let pasteboard = macos::SystemPasteboard;
        let mut last = None;
        // 启动时采集一次当前剪贴板。来源未知（此时前台是终端或启动器自己），记为空。
        self.inner
            .handle(watcher::poll(&pasteboard, &mut last), None)?;

        let inner = Arc::clone(&self.inner);
        std::thread::Builder::new()
            .name("helmp-clipboard-watch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(watcher::POLL_INTERVAL);
                    let read = watcher::poll(&pasteboard, &mut last);
                    if read == Read::Unchanged {
                        continue;
                    }
                    if let Err(err) = inner.handle(read, macos::frontmost_app()) {
                        eprintln!("helmp: 剪贴板采集失败：{err:#}");
                    }
                }
            })
            .context("启动剪贴板轮询线程")?;
        Ok(())
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let index = self.inner.index.read().expect("clipboard index poisoned");
        let now = now_ms();
        search(&index, query)
            .into_iter()
            .map(|(position, score)| {
                let indexed = &index[position];
                let id = indexed.id;
                let inner = Arc::clone(&self.inner);
                Item {
                    id: format!("{PROVIDER_ID}:{id}"),
                    title: indexed.title.clone(),
                    subtitle: indexed.subtitle(now),
                    badge: BADGE.into(),
                    score,
                    action: Action::new(move || inner.paste(id)),
                }
            })
            .collect()
    }
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
        *self.index.write().expect("clipboard index poisoned") =
            entries.iter().map(Indexed::new).collect();
        *guard = Some(store);
        Ok(())
    }

    fn handle(&self, read: Read, source: Option<Source>) -> Result<()> {
        match read {
            Read::Clip(clip) => self.record(clip, source.as_ref()),
            Read::Unchanged | Read::Private(_) | Read::Empty => Ok(()),
        }
    }

    fn record(&self, clip: NewClip, source: Option<&Source>) -> Result<()> {
        let mut guard = self.store.lock().expect("clipboard store poisoned");
        let store = guard.as_mut().context("剪贴板存储未初始化")?;
        let (entry, _) = store.add(clip, source, now_ms())?;
        self.move_to_front(&entry);
        Ok(())
    }

    fn move_to_front(&self, entry: &Entry) {
        let mut index = self.index.write().expect("clipboard index poisoned");
        index.retain(|indexed| indexed.id != entry.id);
        index.insert(0, Indexed::new(&truncated(entry)));
    }

    /// 选中某条：写回系统剪贴板、移到最前；有辅助功能权限时再模拟 ⌘V。
    fn paste(&self, id: i64) -> Result<Outcome> {
        let (entry, image_path) = {
            let guard = self.store.lock().expect("clipboard store poisoned");
            let store = guard.as_ref().context("剪贴板存储未初始化")?;
            let entry = store
                .get(id)?
                .with_context(|| format!("剪贴板历史里没有第 {id} 条"))?;
            let image_path = store.image_path(&entry);
            (entry, image_path)
        };
        macos::write(&entry, &image_path)?;
        {
            let mut guard = self.store.lock().expect("clipboard store poisoned");
            let store = guard.as_mut().context("剪贴板存储未初始化")?;
            let touched = store.touch(id, now_ms())?;
            self.move_to_front(&touched);
        }
        if autopaste_enabled() && macos::accessibility_trusted() {
            // 等启动器面板隐藏、键盘焦点回到原来的 App。
            std::thread::sleep(std::time::Duration::from_millis(80));
            macos::send_paste()?;
        }
        Ok(Outcome::Done)
    }
}

fn autopaste_enabled() -> bool {
    std::env::var(AUTOPASTE_ENV).map_or(true, |value| value != "0")
}

/// 内存索引中的一条。
#[derive(Debug, Clone)]
struct Indexed {
    id: i64,
    kind: Kind,
    title: String,
    title_lower: String,
    /// 小写化的可搜索文本（截断到 `PREVIEW_CHARS`）。
    haystack: String,
    /// 文本字数 / 文件个数。
    count: usize,
    image_format: &'static str,
    source: Option<String>,
    copied_at_ms: i64,
}

impl Indexed {
    fn new(entry: &Entry) -> Self {
        let title = title_of(entry);
        let (haystack, count) = match entry.kind {
            Kind::Text => (entry.content.to_lowercase(), entry.chars),
            Kind::Files => (entry.content.to_lowercase(), entry.content.lines().count()),
            Kind::Image => (title.to_lowercase(), 0),
        };
        let image_format = match entry.kind {
            Kind::Image => match store::ImageFormat::from_file_name(&entry.content) {
                Ok(store::ImageFormat::Png) => "PNG",
                Ok(store::ImageFormat::Tiff) => "TIFF",
                Err(_) => "图片",
            },
            _ => "",
        };
        Self {
            id: entry.id,
            kind: entry.kind,
            title_lower: title.to_lowercase(),
            title,
            haystack,
            count,
            image_format,
            source: entry.source.as_ref().map(|source| source.name.clone()),
            copied_at_ms: entry.copied_at_ms,
        }
    }

    /// 「来源 App · 3 分钟前 · N 字」。
    fn subtitle(&self, now_ms: i64) -> String {
        let source = self.source.as_deref().unwrap_or("未知来源");
        let size = match self.kind {
            Kind::Text => format!("{} 字", self.count),
            Kind::Files => format!("{} 个文件", self.count),
            Kind::Image => self.image_format.to_string(),
        };
        format!("{source} · {} · {size}", ago(now_ms - self.copied_at_ms))
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

fn title_of(entry: &Entry) -> String {
    match entry.kind {
        Kind::Text => entry
            .content
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|line| ellipsize(line, TITLE_CHARS))
            .unwrap_or_else(|| "（空白）".into()),
        Kind::Files => {
            let mut paths = entry.content.lines();
            let first = paths.next().unwrap_or_default();
            let name = std::path::Path::new(first)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| first.to_string());
            match paths.count() {
                0 => name,
                rest => format!("{name} 等 {} 个文件", rest + 1),
            }
        }
        Kind::Image if entry.width > 0 && entry.height > 0 => {
            format!("图片 {}×{}", entry.width, entry.height)
        }
        Kind::Image => "图片".into(),
    }
}

fn ellipsize(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

fn ago(elapsed_ms: i64) -> String {
    let seconds = elapsed_ms.max(0) / 1000;
    match seconds {
        0..60 => "刚刚".into(),
        60..3600 => format!("{} 分钟前", seconds / 60),
        3600..86_400 => format!("{} 小时前", seconds / 3600),
        _ => format!("{} 天前", seconds / 86_400),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Mode<'a> {
    /// 带前缀：剪贴板模式，参数是前缀后的查询（已 trim，可为空）。
    Browse(&'a str),
    /// 不带前缀：顺带给几条低分结果。
    Ambient(&'a str),
}

fn parse_mode(query: &str) -> Mode<'_> {
    let trimmed = query.trim_start();
    for prefix in PREFIXES {
        let Some(head) = trimmed.get(..prefix.len()) else {
            continue;
        };
        let rest = &trimmed[prefix.len()..];
        if head.eq_ignore_ascii_case(prefix) && rest.starts_with(char::is_whitespace) {
            return Mode::Browse(rest.trim());
        }
    }
    Mode::Ambient(query.trim())
}

/// 返回 (索引下标, 分数)，已按我们想要的顺序排好。
fn search(index: &[Indexed], query: &str) -> Vec<(usize, u32)> {
    match parse_mode(query) {
        Mode::Browse("") => (0..index.len().min(RECENT_LIMIT))
            .map(|position| (position, browse_score(position)))
            .collect(),
        Mode::Browse(text) => {
            let mut hits = matches(index, text, BODY_TIER);
            // 稳定排序：同档位保持新的在前。
            hits.sort_by_key(|&(_, tier)| std::cmp::Reverse(tier));
            hits.truncate(MAX_RESULTS);
            hits.into_iter()
                .enumerate()
                .map(|(rank, (position, _))| (position, browse_score(rank)))
                .collect()
        }
        Mode::Ambient(text) if text.chars().count() >= AMBIENT_MIN_CHARS => {
            // 只要首行就命中（子串及以上）的，正文命中不算「高度匹配」。
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
        Mode::Ambient(_) => Vec::new(),
    }
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

    #[test]
    fn parses_all_prefixes_case_insensitively() {
        assert_eq!(parse_mode("cb "), Mode::Browse(""));
        assert_eq!(parse_mode("CB foo bar "), Mode::Browse("foo bar"));
        assert_eq!(parse_mode("clip x"), Mode::Browse("x"));
        assert_eq!(parse_mode("剪贴板 你好"), Mode::Browse("你好"));
        assert_eq!(parse_mode("剪切板　全角空格"), Mode::Browse("全角空格"));
        // 没有空格不算前缀：用户可能正在输入 `cbr`、`clipboard` 之类的应用名。
        assert_eq!(parse_mode("cb"), Mode::Ambient("cb"));
        assert_eq!(parse_mode("clipboard"), Mode::Ambient("clipboard"));
    }

    #[test]
    fn empty_browse_lists_recent_fifty_newest_first() {
        // 前缀后为空就是「看看刚才复制了什么」，最新的必须在最上面。
        let dir = TempDir::new("recent");
        let clips: Vec<String> = (0..60).map(|i| format!("clip {i}")).collect();
        let refs: Vec<&str> = clips.iter().map(String::as_str).collect();
        let provider = provider_with(&dir, &refs);
        let mut items = provider.query("cb ");
        assert_eq!(items.len(), RECENT_LIMIT);
        // 模拟引擎的排序（分数降序、同分按标题），顺序必须不被打乱。
        items.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.title.cmp(&b.title)));
        assert_eq!(items[0].title, "clip 59");
        assert_eq!(items[49].title, "clip 10");
        assert!(items.iter().all(|item| item.badge == BADGE));
        assert!(items.iter().all(|item| item.id.starts_with("clipboard:")));
    }

    #[test]
    fn browse_searches_whole_history_and_outranks_other_providers() {
        let dir = TempDir::new("browse");
        let provider = provider_with(
            &dir,
            &[
                "deploy script\nsecond line",
                "foo",
                "第一行\n正文里有 deploy",
            ],
        );
        let items = provider.query("clip deploy");
        // 首行命中排在正文命中之前；正文第二行的命中也要找得到。
        assert_eq!(titles(&items), vec!["deploy script", "第一行"]);
        // 剪贴板模式下必须压过应用等 provider（matcher 满分 1000）。
        assert!(items.iter().all(|item| item.score > 1000));
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
        // 正文才命中的不算高度匹配。
        assert!(!titles(&items).contains(&"body"));
        // 新的在前。
        assert_eq!(items[0].title, "safari tips 4");
        assert!(items[0].score > items[1].score && items[1].score > items[2].score);
    }

    #[test]
    fn concealed_clip_never_reaches_disk_or_results() {
        // 端到端地走一遍采集管线：密码管理器的内容既不能入库也不能被搜到。
        let dir = TempDir::new("concealed");
        let provider = provider_with(&dir, &[]);
        let pb = FakePasteboard::default();
        pb.set_text("hunter2-secret", &["org.nspasteboard.ConcealedType"]);
        let mut last = None;
        provider
            .inner
            .handle(watcher::poll(&pb, &mut last), None)
            .unwrap();
        assert!(provider.query("cb ").is_empty());
        let store = Store::open(&dir.0).unwrap();
        assert!(store.load_all(100).unwrap().is_empty());
    }

    #[test]
    fn duplicate_copy_moves_to_front_in_index() {
        let dir = TempDir::new("dupindex");
        let provider = provider_with(&dir, &["a", "b", "a"]);
        let items = provider.query("cb ");
        assert_eq!(titles(&items), vec!["a", "b"]);
    }

    #[test]
    fn image_is_saved_to_disk_with_dimensions() {
        // 图片不进数据库正文而是落盘：库保持小，写回时按原格式读文件。
        let dir = TempDir::new("image");
        let provider = provider_with(&dir, &[]);
        let pb = FakePasteboard::default();
        pb.change.set(7);
        *pb.types.borrow_mut() = vec!["public.png".into()];
        *pb.image.borrow_mut() = Some(watcher::Image {
            bytes: b"\x89PNG fake".to_vec(),
            format: ImageFormat::Png,
            width: 1280,
            height: 720,
        });
        let mut last = None;
        let source = Source {
            name: "预览".into(),
            bundle_id: Some("com.apple.Preview".into()),
        };
        provider
            .inner
            .handle(watcher::poll(&pb, &mut last), Some(source))
            .unwrap();

        let items = provider.query("cb ");
        assert_eq!(titles(&items), vec!["图片 1280×720"]);
        assert!(items[0].subtitle.starts_with("预览 · 刚刚 · PNG"));

        let store = Store::open(&dir.0).unwrap();
        let entry = store.load_all(100).unwrap().remove(0);
        let path = store.image_path(&entry);
        assert_eq!(path.extension().unwrap(), "png");
        assert_eq!(std::fs::read(path).unwrap(), b"\x89PNG fake");
    }

    #[test]
    fn titles_and_subtitles() {
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
        assert_eq!(
            title_of(&entry(Kind::Text, "\n  第一行  \n第二行")),
            "第一行"
        );
        assert_eq!(title_of(&entry(Kind::Text, " \n ")), "（空白）");
        assert_eq!(
            title_of(&entry(Kind::Text, &"字".repeat(100))),
            format!("{}…", "字".repeat(TITLE_CHARS))
        );
        assert_eq!(title_of(&entry(Kind::Files, "/a/b/报告.pdf")), "报告.pdf");
        assert_eq!(
            title_of(&entry(Kind::Files, "/a/x.txt\n/a/y.txt")),
            "x.txt 等 2 个文件"
        );

        let indexed = Indexed::new(&entry(Kind::Text, "hello"));
        assert_eq!(
            indexed.subtitle(3 * 60 * 1000 + 5),
            "未知来源 · 3 分钟前 · 5 字"
        );
        assert_eq!(ago(2 * 86_400_000), "2 天前");
    }

    #[test]
    fn files_clip_round_trips_through_store() {
        let dir = TempDir::new("files");
        let provider = provider_with(&dir, &[]);
        provider
            .inner
            .record(
                NewClip::Files(vec![
                    PathBuf::from("/tmp/a.txt"),
                    PathBuf::from("/tmp/b.txt"),
                ]),
                None,
            )
            .unwrap();
        let items = provider.query("cb b.txt");
        assert_eq!(titles(&items), vec!["a.txt 等 2 个文件"]);
        assert!(items[0].subtitle.ends_with("2 个文件"));
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
                index.push(Indexed::new(&Entry {
                    id: i,
                    kind: Kind::Text,
                    chars: content.chars().count(),
                    content,
                    width: 0,
                    height: 0,
                    source: None,
                    copied_at_ms: 0,
                    seq: i,
                }));
            }
        }
        for query in [
            "cb ",
            "cb realism",
            "cb 99999",
            "cb 不存在的内容",
            "words for",
        ] {
            let started = std::time::Instant::now();
            let items = provider.query(query);
            let elapsed = started.elapsed();
            eprintln!(
                "100k 条历史 query({query:?}) → {} 条，{elapsed:?}",
                items.len()
            );
            assert!(
                elapsed < std::time::Duration::from_millis(100),
                "{query:?} 耗时 {elapsed:?}"
            );
        }
    }
}
