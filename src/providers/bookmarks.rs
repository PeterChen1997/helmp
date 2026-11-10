//! 浏览器书签：读 Chrome / Edge / Brave / Arc（Chromium 的 `Bookmarks` JSON，每个 profile 一份）
//! 与 Safari（`~/Library/Safari/Bookmarks.plist`），按标题（含拼音）和网址搜索。
//!
//! - 文件都在 `$HOME/Library` 下；HOME 由环境变量决定，所以端到端测试换个临时 HOME 放夹具即可。
//! - Safari 的书签受「完全磁盘访问」保护：读不到时在书签范围里给一条说明和「打开设置」，不报错、不崩。
//! - 索引在 `start` 里读一次；之后查询时若距上次读取超过 `RELOAD_AFTER`，在后台线程重读（查询本身只读内存）。
//! - 根搜索里查询 ≥ 2 个字符才混入，分数压在应用之下；范围「书签」（前缀 `bm ` / `书签 `）里列出全部。
//! - 回车在来源浏览器里打开（`open -b <bundle id>`），⌘↵ 用默认浏览器，⌘⇧C 复制链接。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::Value;

use crate::i18n::{tr, trf};
use crate::matcher::{self, Candidate};
use crate::provider::{
    Accessory, Action, Icon, Item, NamedAction, Outcome, Provider, Scope, Tint, Tone,
};

const SCOPE: &str = "bookmarks";
const RELOAD_AFTER: Duration = Duration::from_secs(60);
/// 根搜索里最多混入几条。
const ROOT_LIMIT: usize = 6;
const FULL_DISK_ACCESS_URL: &str =
    "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles";

/// 一个浏览器：显示名、bundle id、相对 `~/Library/Application Support` 的用户数据目录（Chromium 系）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Browser {
    pub key: &'static str,
    pub name: &'static str,
    pub bundle_id: &'static str,
    data_dir: Option<&'static str>,
}

pub const BROWSERS: [Browser; 5] = [
    Browser {
        key: "chrome",
        name: "Chrome",
        bundle_id: "com.google.Chrome",
        data_dir: Some("Google/Chrome"),
    },
    Browser {
        key: "edge",
        name: "Edge",
        bundle_id: "com.microsoft.edgemac",
        data_dir: Some("Microsoft Edge"),
    },
    Browser {
        key: "brave",
        name: "Brave",
        bundle_id: "com.brave.Browser",
        data_dir: Some("BraveSoftware/Brave-Browser"),
    },
    Browser {
        key: "arc",
        name: "Arc",
        bundle_id: "company.thebrowser.Browser",
        data_dir: Some("Arc/User Data"),
    },
    Browser {
        key: "safari",
        name: "Safari",
        bundle_id: "com.apple.Safari",
        data_dir: None,
    },
];

/// 一条书签。
#[derive(Debug, Clone)]
pub struct Mark {
    pub browser: Browser,
    /// Chromium 的 profile 目录名（`Default` 记为 None）。
    pub profile: Option<String>,
    pub title: String,
    pub url: String,
    /// 所在文件夹路径，如「书签栏 / 工作」。
    pub folder: String,
    title_candidate: Candidate,
    url_lower: String,
}

impl Mark {
    fn new(
        browser: Browser,
        profile: Option<String>,
        title: String,
        url: String,
        folder: String,
    ) -> Self {
        // 没有标题的书签用网址当标题，列表里不能是一行空白。
        let title = if title.trim().is_empty() {
            url.clone()
        } else {
            title
        };
        Self {
            title_candidate: Candidate::new(&title),
            url_lower: url.to_lowercase(),
            browser,
            profile,
            title,
            url,
            folder,
        }
    }
}

/// 读不到的书签来源：在范围里给一条说明，不默默跳过。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// Safari 书签需要「完全磁盘访问」。
    SafariPermission,
    /// 文件存在但读不了 / 解析失败。
    Unreadable { source: String, error: String },
}

#[derive(Debug, Default)]
struct Index {
    marks: Vec<Mark>,
    problems: Vec<Problem>,
}

#[derive(Default)]
pub struct Bookmarks {
    index: Arc<RwLock<Arc<Index>>>,
    loaded_at: Arc<Mutex<Option<Instant>>>,
    reloading: Arc<AtomicBool>,
}

fn badge() -> &'static str {
    tr("书签", "Bookmarks")
}

impl Provider for Bookmarks {
    fn id(&self) -> &'static str {
        "bookmarks"
    }

    fn start(&self) -> Result<()> {
        let home = crate::paths::home_dir()?;
        self.store(load(&home));
        Ok(())
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let query = query.trim();
        if query.chars().count() < 2 {
            return Vec::new();
        }
        let index = self.snapshot();
        let mut items: Vec<Item> = search(&index.marks, query)
            .into_iter()
            .take(ROOT_LIMIT)
            .map(|(mark, score)| item(mark, score * 3 / 5))
            .collect();
        if let Some(score) = ["书签", "浏览器书签", "bookmarks", "bookmark"]
            .iter()
            .filter_map(|keyword| matcher::score(query, keyword))
            .max()
        {
            items.push(entry(score));
        }
        items
    }

    fn scope(&self) -> Option<Scope> {
        Some(Scope {
            id: SCOPE.into(),
            label: tr("书签", "Bookmarks").into(),
            tint: Tint::Orange,
            prefixes: vec!["bm ".into(), "书签 ".into()],
            placeholder: tr(
                "搜索浏览器书签的标题或网址",
                "Search bookmark titles and URLs",
            )
            .into(),
            filters: Vec::new(),
        })
    }

    fn query_scoped(&self, query: &str, _filter: &str) -> Vec<Item> {
        let index = self.snapshot();
        let query = query.trim();
        let mut items: Vec<Item> = if query.is_empty() {
            index.marks.iter().map(|mark| item(mark, 500)).collect()
        } else {
            search(&index.marks, query)
                .into_iter()
                .map(|(mark, score)| item(mark, score))
                .collect()
        };
        items.extend(index.problems.iter().map(problem_item));
        if items.is_empty() {
            items.push(Item {
                id: "bookmarks:none".into(),
                title: tr("没有找到书签", "No bookmarks found").into(),
                subtitle: tr(
                    "支持 Chrome、Edge、Brave、Arc 与 Safari",
                    "Chrome, Edge, Brave, Arc and Safari are supported",
                )
                .into(),
                badge: badge().into(),
                icon: ICON,
                action: Action::new(|| Ok(Outcome::Done)).keep_open(),
                ..Item::blank()
            });
        }
        items
    }
}

const ICON: Icon = Icon::Symbol {
    name: "bookmark",
    tint: Tint::Orange,
};

impl Bookmarks {
    fn store(&self, index: Index) {
        *self.index.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(index);
        *self.loaded_at.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
    }

    /// 当前索引；过期了就在后台重读（这次先用旧的）。
    fn snapshot(&self) -> Arc<Index> {
        let stale = self
            .loaded_at
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_none_or(|at| at.elapsed() > RELOAD_AFTER);
        if stale && !self.reloading.swap(true, Ordering::SeqCst) {
            let (index, loaded_at, reloading) = (
                self.index.clone(),
                self.loaded_at.clone(),
                self.reloading.clone(),
            );
            std::thread::spawn(move || {
                if let Ok(home) = crate::paths::home_dir() {
                    *index.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(load(&home));
                }
                *loaded_at.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
                reloading.store(false, Ordering::SeqCst);
            });
        }
        self.index.read().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

/// 按标题（含拼音）和网址打分，高分在前。网址命中略低于标题命中。
fn search<'a>(marks: &'a [Mark], query: &str) -> Vec<(&'a Mark, u32)> {
    let lower = query.to_lowercase();
    let mut hits: Vec<(&Mark, u32)> = marks
        .iter()
        .filter_map(|mark| {
            let title = matcher::score_candidate(query, &mark.title_candidate);
            let url = mark.url_lower.contains(&lower).then_some(600);
            Some((mark, title.max(url)?))
        })
        .collect();
    hits.sort_by_key(|hit| std::cmp::Reverse(hit.1));
    hits
}

fn source_label(mark: &Mark) -> String {
    match &mark.profile {
        Some(profile) => format!("{} · {profile}", mark.browser.name),
        None => mark.browser.name.to_string(),
    }
}

fn domain(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

fn item(mark: &Mark, score: u32) -> Item {
    let host = domain(&mark.url);
    let subtitle = if mark.folder.is_empty() {
        host.to_string()
    } else {
        format!("{host} · {}", mark.folder)
    };
    let profile = mark.profile.as_deref().unwrap_or("Default");
    Item {
        id: format!("bookmarks:{}/{profile}:{}", mark.browser.key, mark.url),
        title: mark.title.clone(),
        subtitle,
        badge: badge().into(),
        score,
        icon: ICON,
        accessories: vec![Accessory::Tag {
            text: source_label(mark),
            tone: Tone::Neutral,
        }],
        action: open_in(mark.browser.bundle_id, mark.url.clone()),
        action_title: trf!("在 {} 中打开", "Open in {}", mark.browser.name),
        actions: vec![
            NamedAction::new(
                tr("用默认浏览器打开", "Open in Default Browser"),
                Some("⌘↵"),
                Action::open(mark.url.clone()),
            ),
            NamedAction::new(
                tr("复制链接", "Copy Link"),
                Some("⌘⇧C"),
                Action::copy(mark.url.clone()),
            ),
        ],
        ..Item::blank()
    }
}

/// 在指定浏览器里打开；那个浏览器已经卸载时退回默认浏览器。
fn open_in(bundle_id: &'static str, url: String) -> Action {
    Action::new(move || {
        if let Some(outcome) = crate::effects::dry("open", &format!("-b {bundle_id} {url}")) {
            return Ok(outcome);
        }
        let status = std::process::Command::new("open")
            .args(["-b", bundle_id, &url])
            .status()?;
        if !status.success() {
            let status = std::process::Command::new("open").arg(&url).status()?;
            anyhow::ensure!(
                status.success(),
                trf!("打开 {url} 失败：{status}", "Couldn't open {url}: {status}")
            );
        }
        Ok(Outcome::Done)
    })
}

/// 根搜索里的「书签」入口：进入书签范围。
fn entry(score: u32) -> Item {
    Item {
        id: "bookmarks:open".into(),
        title: tr("浏览器书签", "Browser Bookmarks").into(),
        subtitle: tr(
            "搜索 Chrome、Edge、Brave、Arc、Safari 的书签",
            "Search bookmarks from Chrome, Edge, Brave, Arc and Safari",
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

fn problem_item(problem: &Problem) -> Item {
    match problem {
        Problem::SafariPermission => Item {
            id: "bookmarks:safari-permission".into(),
            title: tr("读不到 Safari 书签", "Can’t read Safari bookmarks").into(),
            subtitle: tr(
                "需要在「隐私与安全性 › 完全磁盘访问」里允许 Helmp，回车打开设置",
                "Allow Helmp in Privacy & Security › Full Disk Access; press Return to open it",
            )
            .into(),
            badge: badge().into(),
            score: 1,
            icon: Icon::Symbol {
                name: "shield-alert",
                tint: Tint::Orange,
            },
            action: Action::open(FULL_DISK_ACCESS_URL),
            action_title: tr("打开设置", "Open Settings").into(),
            ..Item::blank()
        },
        Problem::Unreadable { source, error } => Item {
            id: format!("bookmarks:unreadable:{source}"),
            title: trf!("读不了 {source} 的书签", "Can’t read {source} bookmarks"),
            subtitle: error.clone(),
            badge: badge().into(),
            score: 1,
            icon: Icon::Symbol {
                name: "shield-alert",
                tint: Tint::Red,
            },
            action: Action::new(|| Ok(Outcome::Done)).keep_open(),
            ..Item::blank()
        },
    }
}

// ---------------------------------------------------------------- 读取

/// 读出 `home` 下所有浏览器的书签。同一浏览器 profile 里重复的网址只留第一条。
fn load(home: &Path) -> Index {
    let mut index = Index::default();
    let support = home.join("Library/Application Support");
    for browser in BROWSERS {
        match browser.data_dir {
            Some(dir) => {
                for (profile, path) in chromium_profiles(&support.join(dir)) {
                    let label = match &profile {
                        Some(p) => format!("{} · {p}", browser.name),
                        None => browser.name.to_string(),
                    };
                    match std::fs::read_to_string(&path)
                        .map_err(anyhow::Error::from)
                        .and_then(|text| parse_chromium(&text, browser, profile.clone()))
                    {
                        Ok(marks) => index.marks.extend(marks),
                        Err(err) => index.problems.push(Problem::Unreadable {
                            source: label,
                            error: format!("{err:#}"),
                        }),
                    }
                }
            }
            None => {
                let path = home.join("Library/Safari/Bookmarks.plist");
                match read_safari(&path) {
                    Ok(marks) => index.marks.extend(marks),
                    Err(SafariError::Missing) => {}
                    Err(SafariError::Permission) => index.problems.push(Problem::SafariPermission),
                    Err(SafariError::Other(error)) => index.problems.push(Problem::Unreadable {
                        source: browser.name.into(),
                        error,
                    }),
                }
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    index
        .marks
        .retain(|mark| seen.insert((mark.browser.key, mark.profile.clone(), mark.url.clone())));
    index
}

/// 用户数据目录下带 `Bookmarks` 文件的 profile：(`Default` → None, 路径)。按目录名排序。
fn chromium_profiles(root: &Path) -> Vec<(Option<String>, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut profiles: Vec<(Option<String>, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path().join("Bookmarks");
            path.is_file().then(|| {
                let name = entry.file_name().to_string_lossy().into_owned();
                ((name != "Default").then_some(name), path)
            })
        })
        .collect();
    profiles.sort_by(|a, b| a.0.cmp(&b.0));
    profiles
}

/// 解析 Chromium 的 `Bookmarks` JSON：`roots` 下的书签栏、其他书签、移动设备书签，递归文件夹。
pub fn parse_chromium(text: &str, browser: Browser, profile: Option<String>) -> Result<Vec<Mark>> {
    let json: Value = serde_json::from_str(text)?;
    let roots = json
        .get("roots")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!(tr("缺少 roots 字段", "missing the roots field")))?;
    let mut marks = Vec::new();
    for key in ["bookmark_bar", "other", "synced"] {
        if let Some(node) = roots.get(key) {
            let name = node.get("name").and_then(Value::as_str).unwrap_or_default();
            walk_chromium(node, name, browser, &profile, &mut marks);
        }
    }
    Ok(marks)
}

fn walk_chromium(
    node: &Value,
    folder: &str,
    browser: Browser,
    profile: &Option<String>,
    out: &mut Vec<Mark>,
) {
    let Some(children) = node.get("children").and_then(Value::as_array) else {
        return;
    };
    for child in children {
        let name = child
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match child.get("type").and_then(Value::as_str) {
            Some("url") => {
                if let Some(url) = child
                    .get("url")
                    .and_then(Value::as_str)
                    .filter(|url| openable(url))
                {
                    out.push(Mark::new(
                        browser,
                        profile.clone(),
                        name.into(),
                        url.into(),
                        folder.into(),
                    ));
                }
            }
            Some("folder") => {
                let path = join_folder(folder, name);
                walk_chromium(child, &path, browser, profile, out);
            }
            _ => {}
        }
    }
}

fn join_folder(parent: &str, name: &str) -> String {
    match (parent.is_empty(), name.is_empty()) {
        (true, _) => name.to_string(),
        (_, true) => parent.to_string(),
        _ => format!("{parent} / {name}"),
    }
}

/// `javascript:` 小书签用 `open` 打不开，不收。
fn openable(url: &str) -> bool {
    !url.trim().is_empty() && !url.to_ascii_lowercase().starts_with("javascript:")
}

#[derive(Debug)]
enum SafariError {
    Missing,
    Permission,
    Other(String),
}

fn read_safari(path: &Path) -> Result<Vec<Mark>, SafariError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return Err(match err.kind() {
                std::io::ErrorKind::NotFound => SafariError::Missing,
                // 没有完全磁盘访问时是 EPERM（Operation not permitted），文件权限不够是 EACCES。
                std::io::ErrorKind::PermissionDenied => SafariError::Permission,
                _ if err.raw_os_error() == Some(libc::EPERM) => SafariError::Permission,
                _ => SafariError::Other(err.to_string()),
            });
        }
    };
    parse_safari(&bytes).map_err(|err| SafariError::Other(format!("{err:#}")))
}

/// 解析 Safari 的 `Bookmarks.plist`（二进制或 XML 均可）。
pub fn parse_safari(bytes: &[u8]) -> Result<Vec<Mark>> {
    let value = plist::Value::from_reader(std::io::Cursor::new(bytes))?;
    let safari = BROWSERS[4];
    let mut marks = Vec::new();
    walk_safari(&value, "", safari, &mut marks);
    Ok(marks)
}

fn walk_safari(node: &plist::Value, folder: &str, browser: Browser, out: &mut Vec<Mark>) {
    let Some(dict) = node.as_dictionary() else {
        return;
    };
    let kind = dict
        .get("WebBookmarkType")
        .and_then(plist::Value::as_string);
    match kind {
        Some("WebBookmarkTypeLeaf") => {
            let url = dict
                .get("URLString")
                .and_then(plist::Value::as_string)
                .unwrap_or_default();
            let title = dict
                .get("URIDictionary")
                .and_then(plist::Value::as_dictionary)
                .and_then(|d| d.get("title"))
                .and_then(plist::Value::as_string)
                .unwrap_or_default();
            if openable(url) {
                out.push(Mark::new(
                    browser,
                    None,
                    title.into(),
                    url.into(),
                    folder.into(),
                ));
            }
        }
        Some("WebBookmarkTypeList") | None => {
            let title = dict
                .get("Title")
                .and_then(plist::Value::as_string)
                .unwrap_or_default();
            let name = match title {
                "BookmarksBar" => tr("个人收藏", "Favorites"),
                "BookmarksMenu" => tr("书签菜单", "Bookmarks Menu"),
                "com.apple.ReadingList" => tr("阅读列表", "Reading List"),
                other => other,
            };
            let path = join_folder(folder, name);
            if let Some(children) = dict.get("Children").and_then(plist::Value::as_array) {
                for child in children {
                    walk_safari(child, &path, browser, out);
                }
            }
        }
        // 历史记录等代理项。
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHROME: &str = r#"{
      "checksum": "x", "version": 1,
      "roots": {
        "bookmark_bar": {"name": "书签栏", "type": "folder", "children": [
          {"name": "GitHub 首页", "type": "url", "url": "https://github.com/"},
          {"name": "工作", "type": "folder", "children": [
            {"name": "飞书文档", "type": "url", "url": "https://www.feishu.cn/docs"},
            {"name": "bookmarklet", "type": "url", "url": "javascript:alert(1)"}
          ]}
        ]},
        "other": {"name": "其他书签", "type": "folder", "children": [
          {"name": "", "type": "url", "url": "https://example.com/a?b=1"}
        ]},
        "synced": {"name": "移动设备书签", "type": "folder", "children": []}
      }
    }"#;

    fn chrome() -> Browser {
        BROWSERS[0]
    }

    #[test]
    fn parses_chromium_folders_and_skips_bookmarklets() {
        // 文件夹路径要拼出来（副标题靠它区分同名书签），javascript: 小书签打不开不能出现。
        let marks = parse_chromium(CHROME, chrome(), None).unwrap();
        let rows: Vec<_> = marks
            .iter()
            .map(|m| (m.title.as_str(), m.url.as_str(), m.folder.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("GitHub 首页", "https://github.com/", "书签栏"),
                ("飞书文档", "https://www.feishu.cn/docs", "书签栏 / 工作"),
                (
                    "https://example.com/a?b=1",
                    "https://example.com/a?b=1",
                    "其他书签"
                ),
            ]
        );
        assert!(
            parse_chromium("{}", chrome(), None).is_err(),
            "缺 roots 要报错，不能当成没有书签"
        );
        assert!(parse_chromium("not json", chrome(), None).is_err());
    }

    #[test]
    fn parses_safari_plist() {
        let mut leaf = plist::Dictionary::new();
        leaf.insert("WebBookmarkType".into(), "WebBookmarkTypeLeaf".into());
        leaf.insert("URLString".into(), "https://developer.apple.com/".into());
        let mut uri = plist::Dictionary::new();
        uri.insert("title".into(), "苹果开发者".into());
        leaf.insert("URIDictionary".into(), uri.into());
        let mut bar = plist::Dictionary::new();
        bar.insert("WebBookmarkType".into(), "WebBookmarkTypeList".into());
        bar.insert("Title".into(), "BookmarksBar".into());
        bar.insert("Children".into(), vec![plist::Value::from(leaf)].into());
        let mut proxy = plist::Dictionary::new();
        proxy.insert("WebBookmarkType".into(), "WebBookmarkTypeProxy".into());
        let mut root = plist::Dictionary::new();
        root.insert("WebBookmarkType".into(), "WebBookmarkTypeList".into());
        root.insert(
            "Children".into(),
            vec![plist::Value::from(proxy), plist::Value::from(bar)].into(),
        );
        let mut bytes = Vec::new();
        plist::Value::from(root)
            .to_writer_binary(&mut bytes)
            .unwrap();
        let marks = parse_safari(&bytes).unwrap();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].title, "苹果开发者");
        assert_eq!(marks[0].folder, "个人收藏");
        assert_eq!(marks[0].browser.bundle_id, "com.apple.Safari");
    }

    #[test]
    fn finds_by_pinyin_title_and_url() {
        // 中文标题要能用拼音首字母找到；记得网址不记得标题时也要能找到。
        let marks = parse_chromium(CHROME, chrome(), None).unwrap();
        let titles = |q: &str| {
            search(&marks, q)
                .into_iter()
                .map(|(m, _)| m.title.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(titles("gthb").first(), Some(&"GitHub 首页"));
        assert_eq!(titles("fswd"), ["飞书文档"]);
        assert_eq!(titles("feishu.cn"), ["飞书文档"]);
        assert!(titles("zzzz").is_empty());
    }

    #[test]
    fn opens_in_the_browser_it_came_from() {
        let marks = parse_chromium(CHROME, chrome(), Some("Profile 1".into())).unwrap();
        let item = item(&marks[0], 900);
        assert_eq!(item.id, "bookmarks:chrome/Profile 1:https://github.com/");
        assert_eq!(item.subtitle, "github.com · 书签栏");
        assert_eq!(
            item.accessories,
            vec![Accessory::Tag {
                text: "Chrome · Profile 1".into(),
                tone: Tone::Neutral
            }]
        );
        assert_eq!(item.action_title, "在 Chrome 中打开");
    }

    #[test]
    fn missing_or_locked_sources_never_crash() {
        // 没装浏览器 = 没有书签；Safari 没授权 = 一条说明，而不是报错或崩溃。
        let home = std::env::temp_dir().join(format!("helmp-bookmarks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let index = load(&home);
        assert!(index.marks.is_empty() && index.problems.is_empty());

        let safari = home.join("Library/Safari/Bookmarks.plist");
        std::fs::create_dir_all(safari.parent().unwrap()).unwrap();
        std::fs::write(&safari, b"x").unwrap();
        std::fs::set_permissions(&safari, std::os::unix::fs::PermissionsExt::from_mode(0o000))
            .unwrap();
        let index = load(&home);
        assert_eq!(index.problems, [Problem::SafariPermission]);
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn icons_exist_in_bundled_assets() {
        use gpui_kit::AssetSource as _;
        for name in ["bookmark", "shield-alert"] {
            let path = format!("icons/{name}.svg");
            assert!(
                gpui_kit::assets::AllAssets.load(&path).unwrap().is_some(),
                "{name}"
            );
        }
    }
}
