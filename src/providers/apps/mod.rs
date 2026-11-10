//! 应用搜索：扫描 .app，按文件名、英文名、中文本地化名（含拼音）匹配。
//!
//! - 索引在 `start` 里建；之后用 FSEvents（notify）监听应用目录，变化后防抖 1 秒重建。
//! - `query` 只读内存里的索引快照，不做 IO。

mod localized;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use notify::{RecursiveMode, Watcher as _};

use objc2_app_kit::NSWorkspace;

use crate::i18n::{tr, trf};
use crate::matcher::{self, Candidate, Match, MatchKind};
use crate::provider::{Action, Block, Detail, Icon, Item, NamedAction, Outcome, Provider};

/// 应用根目录下再往下找几层子目录（`/Applications/Utilities`、`/Applications/Setapp` 等）。
const MAX_SUBDIR_DEPTH: usize = 2;
/// 目录变化后等这么久没有新事件再重建，避免应用安装 / 更新时连续重扫。
const REBUILD_DEBOUNCE: Duration = Duration::from_secs(1);

#[derive(Debug)]
struct App {
    path: String,
    /// 解析过符号链接的路径，用来和 `NSRunningApplication.bundleURL` 比较。
    real_path: String,
    title: String,
    /// 与标题不同的英文名，显示在详情标题下（如「微信」下的 WeChat）。
    english: Option<String>,
    version: Option<String>,
    /// 缩写了家目录的路径。
    location: String,
    /// (原文, 预处理结果)：原文用来判断命中的是不是标题。
    names: Vec<(String, Candidate)>,
}

type Index = Arc<RwLock<Arc<Vec<App>>>>;

#[derive(Default)]
pub struct Apps {
    index: Index,
}

impl Provider for Apps {
    fn id(&self) -> &'static str {
        "apps"
    }

    fn start(&self) -> Result<()> {
        let home = Some(crate::paths::home_dir()?);
        let roots = roots(home.as_deref());
        let extras = [PathBuf::from("/System/Library/CoreServices/Finder.app")];
        // 先起监听再扫描：扫描期间装上的应用也会触发一次重建，不会漏。
        watch(
            self.index.clone(),
            roots.clone(),
            extras.to_vec(),
            home.clone(),
        )?;
        rebuild(&self.index, &roots, &extras, home.as_deref())
    }

    fn query(&self, query: &str) -> Vec<Item> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let apps = match self.index.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        search(&apps, query, running_app_paths)
    }
}

/// `running` 只在有命中时调用一次，返回正在运行的应用路径。
fn search(apps: &[App], query: &str, running: impl FnOnce() -> HashSet<String>) -> Vec<Item> {
    let hits: Vec<(&App, &str, Match)> = apps
        .iter()
        .filter_map(|app| {
            let (name, hit) = app
                .names
                .iter()
                .filter_map(|(name, candidate)| {
                    Some((name.as_str(), matcher::match_candidate(query, candidate)?))
                })
                .max_by_key(|(_, hit)| hit.score)?;
            Some((app, name, hit))
        })
        .collect();
    if hits.is_empty() {
        return Vec::new();
    }
    let running = running();
    hits.into_iter()
        .map(|(app, name, hit)| item(app, name, &hit, running.contains(&app.real_path)))
        .collect()
}

fn item(app: &App, name: &str, hit: &Match, running: bool) -> Item {
    let mut actions = vec![
        NamedAction::new(
            tr("在访达中显示", "Show in Finder"),
            Some("⌘↵"),
            Action::reveal(app.path.clone()),
        ),
        NamedAction::new(
            tr("复制路径", "Copy Path"),
            Some("⌘⇧C"),
            Action::copy(app.path.clone()),
        ),
    ];
    if running {
        actions.push(NamedAction::new(
            tr("退出应用", "Quit App"),
            Some("⌘Q"),
            quit_action(app.real_path.clone()),
        ));
    }
    Item {
        id: format!("apps:{}", app.path),
        title: app.title.clone(),
        subtitle: hint(app, name, hit),
        badge: tr("应用", "Applications").into(),
        score: hit.score,
        action: Action::open(app.path.clone()),
        action_title: tr("打开", "Open").into(),
        icon: Icon::File(PathBuf::from(&app.path)),
        actions,
        detail: Some(detail(app, hit)),
        ..Item::blank()
    }
}

/// 副标题：拼音命中时告诉用户怎么打出来的（设计稿「拼音 weixin · 首字母 wx」），否则显示位置。
fn hint(app: &App, name: &str, hit: &Match) -> String {
    match &hit.pinyin {
        Some(span) if span.whole => trf!(
            "拼音 {} · 首字母 {}",
            "Pinyin {} · Initials {}",
            span.full,
            span.initials
        ),
        Some(span) if hit.kind == MatchKind::PinyinInitials => {
            trf!("首字母 {}", "Initials {}", span.initials)
        }
        Some(span) => trf!("拼音 {}", "Pinyin {}", span.full),
        // 命中的是英文名（输入 lark 找到「飞书」），把英文名带上，不然看不出为什么命中。
        None if name != app.title => format!("{name} · {}", app.location),
        None => app.location.clone(),
    }
}

fn detail(app: &App, hit: &Match) -> Detail {
    let line: Vec<String> = app
        .english
        .iter()
        .cloned()
        .chain(app.version.iter().map(|v| trf!("版本 {v}", "Version {v}")))
        .collect();
    Detail {
        actions: Vec::new(),
        icon: Icon::File(PathBuf::from(&app.path)),
        title: app.title.clone(),
        tag: None,
        blocks: if line.is_empty() {
            Vec::new()
        } else {
            vec![Block::Text(line.join(" · "))]
        },
        metadata: vec![
            (tr("位置", "Location").into(), app.location.clone()),
            (tr("匹配方式", "Matched By").into(), hit.kind.label().into()),
        ],
    }
}

/// 正在运行的应用的 bundle 路径。
fn running_app_paths() -> HashSet<String> {
    objc2::rc::autoreleasepool(|_| {
        NSWorkspace::sharedWorkspace()
            .runningApplications()
            .iter()
            .filter_map(|app| Some(app.bundleURL()?.path()?.to_string()))
            .collect()
    })
}

/// 让应用正常退出（等同于在应用里按 ⌘Q，未保存的文档由应用自己提示）。
fn quit_action(real_path: String) -> Action {
    Action::new(move || {
        let asked = objc2::rc::autoreleasepool(|_| {
            let apps = NSWorkspace::sharedWorkspace().runningApplications();
            let matching: Vec<_> = apps
                .iter()
                .filter(|app| {
                    app.bundleURL()
                        .and_then(|url| url.path())
                        .is_some_and(|path| path.to_string() == real_path)
                })
                .collect();
            if matching.is_empty() {
                return None;
            }
            Some(matching.iter().all(|app| app.terminate()))
        });
        match asked {
            None => bail!(trf!(
                "{real_path} 已经没有在运行",
                "{real_path} is no longer running"
            )),
            Some(false) => bail!(trf!("{real_path} 拒绝退出", "{real_path} refused to quit")),
            Some(true) => Ok(Outcome::Done),
        }
    })
}

fn roots(home: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
    ];
    if let Some(home) = home {
        roots.push(home.join("Applications"));
    }
    roots
}

fn rebuild(
    index: &Index,
    roots: &[PathBuf],
    extras: &[PathBuf],
    home: Option<&Path>,
) -> Result<()> {
    let apps = scan(roots, extras, home)?;
    let mut guard = match index.write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = Arc::new(apps);
    Ok(())
}

/// 起一个后台线程持有 FSEvents 监听器；有变化就防抖后重建索引。
fn watch(
    index: Index,
    roots: Vec<PathBuf>,
    extras: Vec<PathBuf>,
    home: Option<PathBuf>,
) -> Result<()> {
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(tx).context("创建应用目录监听器")?;
    for root in roots.iter().filter(|root| root.is_dir()) {
        // FSEvents 上报的是真实路径；监听经过符号链接的路径（如 /var → /private/var）会收不到事件。
        let real = root
            .canonicalize()
            .with_context(|| format!("解析 {}", root.display()))?;
        watcher
            .watch(&real, RecursiveMode::Recursive)
            .with_context(|| format!("监听 {}", real.display()))?;
    }
    std::thread::Builder::new()
        .name("helmp-apps-watch".into())
        .spawn(move || {
            // 监听器随线程存活；通道断开（监听器出错退出）时线程结束。
            let _watcher = watcher;
            while let Ok(first) = rx.recv() {
                if let Err(err) = first {
                    eprintln!("helmp: apps 监听出错：{err}");
                }
                // 防抖：吃掉 REBUILD_DEBOUNCE 内连续到来的事件。
                while rx.recv_timeout(REBUILD_DEBOUNCE).is_ok() {}
                if let Err(err) = rebuild(&index, &roots, &extras, home.as_deref()) {
                    eprintln!("helmp: apps 重建索引失败：{err:#}");
                }
            }
        })
        .context("启动应用目录监听线程")?;
    Ok(())
}

/// 扫描所有根目录。根目录不存在（如没有 ~/Applications）是正常的；存在但读不了、
/// 或一个应用都没扫到，返回 `Err`。单个应用的元数据解析失败只告警，仍按文件名收录。
fn scan(roots: &[PathBuf], extras: &[PathBuf], home: Option<&Path>) -> Result<Vec<App>> {
    let mut bundles = Vec::new();
    for root in roots {
        if root.exists() {
            collect_bundles(root, 0, &mut bundles)?;
        }
    }
    bundles.extend(extras.iter().filter(|extra| extra.is_dir()).cloned());

    let mut seen = HashSet::new();
    bundles.retain(|path| seen.insert(path.clone()));
    bundles.sort();
    if bundles.is_empty() {
        bail!("在 {roots:?} 下没有找到任何 .app");
    }
    Ok(bundles.iter().map(|path| load_app(path, home)).collect())
}

fn collect_bundles(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("读取 {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("遍历 {}", dir.display()))?;
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        // 跟随符号链接判断是不是目录（/Applications 下有指向别处的 .app 链接）。
        if !path.is_dir() {
            continue;
        }
        if path.extension().is_some_and(|ext| ext == "app") {
            out.push(path);
        } else if depth < MAX_SUBDIR_DEPTH {
            // 子目录可能没有权限（其他用户的目录等），告警后跳过，不影响其余应用。
            if let Err(err) = collect_bundles(&path, depth + 1, out) {
                eprintln!("helmp: apps 跳过 {}：{err:#}", path.display());
            }
        }
    }
    Ok(())
}

fn load_app(path: &Path, home: Option<&Path>) -> App {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let info = localized::bundle_info(path).unwrap_or_else(|err| {
        eprintln!("helmp: apps 读取 Info.plist 失败：{err:#}");
        localized::BundleInfo::default()
    });
    let chinese = localized::chinese_name(path).unwrap_or_else(|err| {
        eprintln!("helmp: apps 读取中文名失败：{err:#}");
        None
    });

    let title = chinese.clone().unwrap_or_else(|| stem.clone());
    let mut seen = HashSet::new();
    let aliases = ALIASES
        .iter()
        .find(|(app, _)| *app == stem)
        .map(|(_, names)| names.iter().map(|name| name.to_string()).collect::<Vec<_>>())
        .unwrap_or_default();
    let names: Vec<String> = std::iter::once(stem)
        .chain(info.names)
        .chain(chinese)
        .chain(aliases)
        .filter(|name| !name.trim().is_empty() && seen.insert(name.to_lowercase()))
        .collect();
    let english = names
        .iter()
        .find(|name| !name.eq_ignore_ascii_case(&title))
        .cloned();

    let path_str = path.to_string_lossy().into_owned();
    // 解析失败（如悬空的符号链接）就用原路径：只影响「退出应用」能否认出它在运行。
    let real_path = path
        .canonicalize()
        .map(|real| real.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path_str.clone());
    App {
        location: abbreviate_home(&path_str, home),
        real_path,
        path: path_str,
        title,
        english,
        version: info.version,
        names: names
            .into_iter()
            .map(|name| {
                let candidate = Candidate::new(&name);
                (name, candidate)
            })
            .collect(),
    }
}

/// 系统 App 改过名后，用户仍习惯用旧名字搜（按 .app 文件名对应）。
const ALIASES: &[(&str, &[&str])] = &[(
    "System Settings",
    &["系统偏好设置", "偏好设置", "System Preferences"],
)];

fn abbreviate_home(path: &str, home: Option<&Path>) -> String {
    let Some(home) = home.and_then(Path::to_str) else {
        return path.to_string();
    };
    match path.strip_prefix(home) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(path: &str, title: &str, names: &[&str]) -> App {
        App {
            path: path.into(),
            real_path: path.into(),
            title: title.into(),
            english: names
                .iter()
                .find(|name| !name.eq_ignore_ascii_case(title))
                .map(|name| name.to_string()),
            version: None,
            location: path.into(),
            names: names
                .iter()
                .map(|name| (name.to_string(), Candidate::new(name)))
                .collect(),
        }
    }

    fn search(apps: &[App], query: &str) -> Vec<Item> {
        super::search(apps, query, HashSet::new)
    }

    fn fixture() -> Vec<App> {
        vec![
            app("/Applications/WeChat.app", "微信", &["WeChat", "微信"]),
            app(
                "/Applications/WechatExporter.app",
                "WechatExporter",
                &["WechatExporter"],
            ),
            app(
                "/Applications/wechatwebdevtools.app",
                "微信开发者工具",
                &["wechatwebdevtools", "微信开发者工具"],
            ),
            app(
                "/Applications/Lark.app",
                "飞书",
                &["Lark", "Feishu", "飞书"],
            ),
            app(
                "/Applications/Safari.app",
                "Safari浏览器",
                &["Safari", "Safari浏览器"],
            ),
            app("/Applications/FocuSee.app", "FocuSee", &["FocuSee"]),
        ]
    }

    fn top(query: &str) -> String {
        let mut items = search(&fixture(), query);
        items.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.title.cmp(&b.title)));
        items
            .first()
            .map(|item| item.title.clone())
            .unwrap_or_default()
    }

    #[test]
    fn chinese_queries_put_wechat_first() {
        // README 的 MVP 验收：wx / weixin / 微信 三种输入第一条都是微信，
        // 即使 WechatExporter、微信开发者工具也能匹配上。
        for query in ["wx", "weixin", "微信"] {
            assert_eq!(top(query), "微信", "{query}");
        }
        assert_eq!(top("fs"), "飞书");
        assert_eq!(top("feishu"), "飞书");
        assert_eq!(top("saf"), "Safari浏览器");
    }

    #[test]
    fn empty_query_returns_nothing() {
        // 空查询返回全部应用会把其他 provider（最近 agent 会话）挤出列表。
        let provider = Apps::default();
        *provider.index.write().unwrap() = Arc::new(fixture());
        assert!(provider.query("  ").is_empty());
        assert!(!provider.query("wx").is_empty());
    }

    #[test]
    fn items_follow_id_and_badge_convention() {
        let items = search(&fixture(), "lark");
        let item = items.first().unwrap();
        assert_eq!(item.id, "apps:/Applications/Lark.app");
        assert_eq!(item.title, "飞书");
        assert_eq!(item.badge, "应用");
    }

    fn find<'a>(items: &'a [Item], title: &str) -> &'a Item {
        items.iter().find(|item| item.title == title).unwrap()
    }

    #[test]
    fn subtitle_explains_pinyin_hits() {
        // 设计稿 Results：中文名靠拼音命中时，副标题说明是怎么打出来的，用户才知道 wx 就能找到微信。
        let items = search(&fixture(), "wx");
        assert_eq!(find(&items, "微信").subtitle, "拼音 weixin · 首字母 wx");
        assert_eq!(find(&items, "微信开发者工具").subtitle, "首字母 wx");
        // 命中英文名时带上英文名，否则「lark → 飞书」看起来像误中。
        let items = search(&fixture(), "lark");
        assert_eq!(
            find(&items, "飞书").subtitle,
            "Lark · /Applications/Lark.app"
        );
        // 直接命中标题时只给位置。
        let items = search(&fixture(), "focus");
        assert_eq!(
            find(&items, "FocuSee").subtitle,
            "/Applications/FocuSee.app"
        );
    }

    #[test]
    fn detail_shows_name_version_location_and_match_kind() {
        // 详情区回答三个问题：这是哪个版本、装在哪、为什么被搜出来（两个同名应用时靠位置区分）。
        let mut apps = fixture();
        apps[0].version = Some("4.1.2".into());
        let items = search(&apps, "wx");
        let detail = find(&items, "微信").detail.clone().unwrap();
        assert_eq!(
            detail.icon,
            Icon::File(PathBuf::from("/Applications/WeChat.app"))
        );
        assert_eq!(detail.title, "微信");
        assert_eq!(detail.blocks, [Block::Text("WeChat · 版本 4.1.2".into())]);
        assert_eq!(
            detail.metadata,
            [
                ("位置".to_string(), "/Applications/WeChat.app".to_string()),
                ("匹配方式".to_string(), "拼音首字母".to_string()),
            ]
        );
    }

    #[test]
    fn quit_action_only_for_running_apps() {
        // 「退出应用」只对正在运行的应用有意义；没运行时出现在 ⌘K 里，按了只会报错。
        let titles = |items: &[Item], title: &str| -> Vec<String> {
            find(items, title)
                .actions
                .iter()
                .map(|a| a.title.clone())
                .collect()
        };
        let idle = search(&fixture(), "lark");
        assert_eq!(titles(&idle, "飞书"), ["在访达中显示", "复制路径"]);
        let running = super::search(&fixture(), "lark", || {
            HashSet::from(["/Applications/Lark.app".to_string()])
        });
        assert_eq!(
            titles(&running, "飞书"),
            ["在访达中显示", "复制路径", "退出应用"]
        );
        let item = find(&running, "飞书");
        assert_eq!(item.action_title, "打开");
        assert_eq!(
            item.icon,
            Icon::File(PathBuf::from("/Applications/Lark.app"))
        );
        let shortcuts: Vec<_> = item
            .actions
            .iter()
            .map(|a| a.shortcut.as_deref().unwrap_or_default())
            .collect();
        assert_eq!(shortcuts, ["⌘↵", "⌘⇧C", "⌘Q"]);
    }

    #[test]
    fn english_ui_translates_labels_but_pinyin_search_still_works() {
        // 界面语言只影响文案：英文界面下 wx / weixin / 微信 仍必须第一条命中微信（索引里的中文名与拼音不受影响），
        // 分组、回车操作、⌘K 操作名和拼音提示则要换成英文。
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            for query in ["wx", "weixin", "微信", "wechat"] {
                assert_eq!(top(query), "微信", "{query}");
            }
            let items = search(&fixture(), "wx");
            let wechat = find(&items, "微信");
            assert_eq!(wechat.badge, "Applications");
            assert_eq!(wechat.action_title, "Open");
            assert_eq!(wechat.subtitle, "Pinyin weixin · Initials wx");
            assert_eq!(find(&items, "微信开发者工具").subtitle, "Initials wx");
            let titles: Vec<_> = wechat.actions.iter().map(|a| a.title.as_str()).collect();
            assert_eq!(titles, ["Show in Finder", "Copy Path"]);
            let keys: Vec<_> = wechat
                .detail
                .as_ref()
                .unwrap()
                .metadata
                .iter()
                .map(|(k, _)| k.as_str())
                .collect();
            assert_eq!(keys, ["Location", "Matched By"]);
        });
    }

    #[test]
    fn running_check_skipped_without_hits() {
        // 查运行中的应用要走 AppKit；没有命中时不该付这个代价（每次按键都会查询）。
        let items = super::search(&fixture(), "zzzz", || panic!("不该查询运行状态"));
        assert!(items.is_empty());
    }

    #[test]
    fn rebuilds_index_when_app_is_installed() {
        // 用户装完新应用马上就要能搜到，不能等重启 Helmp。
        let root = std::env::temp_dir().join(format!("helmp-watch-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Old.app")).unwrap();
        let index = Index::default();
        let roots = vec![root.clone()];
        rebuild(&index, &roots, &[], None).unwrap();
        watch(index.clone(), roots, Vec::new(), None).unwrap();

        // FSEvents 流是异步启动的，紧接着的改动可能早于流生效，等一下再装。
        std::thread::sleep(Duration::from_secs(1));
        std::fs::create_dir_all(root.join("Sub/Fresh.app")).unwrap();
        // FSEvents 在全量并行测试的负载下可能延迟数秒，上限放宽到 30 秒，避免偶发失败。
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let found = loop {
            let apps = index.read().unwrap().clone();
            if apps.iter().any(|app| app.title == "Fresh") {
                break true;
            }
            if std::time::Instant::now() > deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        std::fs::remove_dir_all(&root).unwrap();
        assert!(found, "新装的应用 10 秒内没进索引");
    }

    #[test]
    fn subtitle_abbreviates_home() {
        let home = Path::new("/Users/me");
        assert_eq!(
            abbreviate_home("/Users/me/Applications/A.app", Some(home)),
            "~/Applications/A.app"
        );
        // 只替换完整的目录前缀，/Users/meow 不是 /Users/me 的子目录。
        assert_eq!(
            abbreviate_home("/Users/meow/A.app", Some(home)),
            "/Users/meow/A.app"
        );
    }

    /// 本机真实数据的性能测量：`cargo test --release apps::tests::bench_real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_real_index() {
        let home = Some(crate::paths::home_dir().unwrap());
        let roots = roots(home.as_deref());
        let extras = [PathBuf::from("/System/Library/CoreServices/Finder.app")];
        let started = std::time::Instant::now();
        let apps = scan(&roots, &extras, home.as_deref()).unwrap();
        let build = started.elapsed();
        let queries = ["wx", "weixin", "微信", "fs", "saf", "jsq", "a", "zzzz"];
        let started = std::time::Instant::now();
        let rounds = 100;
        let mut worst = Duration::ZERO;
        for _ in 0..rounds {
            for query in queries {
                let one = std::time::Instant::now();
                std::hint::black_box(search(&apps, query));
                worst = worst.max(one.elapsed());
            }
        }
        let average = started.elapsed() / (rounds * queries.len() as u32);
        println!(
            "apps: {} 个，建索引 {build:?}，单次查询平均 {average:?}，最慢 {worst:?}",
            apps.len()
        );
    }
}
