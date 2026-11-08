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

use crate::matcher::{self, Candidate};
use crate::provider::{Action, Item, Provider};

/// 应用根目录下再往下找几层子目录（`/Applications/Utilities`、`/Applications/Setapp` 等）。
const MAX_SUBDIR_DEPTH: usize = 2;
/// 目录变化后等这么久没有新事件再重建，避免应用安装 / 更新时连续重扫。
const REBUILD_DEBOUNCE: Duration = Duration::from_secs(1);

#[derive(Debug)]
struct App {
    path: String,
    title: String,
    subtitle: String,
    names: Vec<Candidate>,
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
        search(&apps, query)
    }
}

fn search(apps: &[App], query: &str) -> Vec<Item> {
    apps.iter()
        .filter_map(|app| {
            let score = app
                .names
                .iter()
                .filter_map(|name| matcher::score_candidate(query, name))
                .max()?;
            Some(Item {
                id: format!("apps:{}", app.path),
                title: app.title.clone(),
                subtitle: app.subtitle.clone(),
                badge: "应用".into(),
                score,
                action: Action::open(app.path.clone()),
                ..Item::blank()
            })
        })
        .collect()
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
    let english = localized::bundle_names(path).unwrap_or_else(|err| {
        eprintln!("helmp: apps 读取 Info.plist 失败：{err:#}");
        Vec::new()
    });
    let chinese = localized::chinese_name(path).unwrap_or_else(|err| {
        eprintln!("helmp: apps 读取中文名失败：{err:#}");
        None
    });

    let title = chinese.clone().unwrap_or_else(|| stem.clone());
    let mut seen = HashSet::new();
    let names = std::iter::once(stem)
        .chain(english)
        .chain(chinese)
        .filter(|name| !name.trim().is_empty() && seen.insert(name.to_lowercase()))
        .map(|name| Candidate::new(&name))
        .collect();

    let path_str = path.to_string_lossy().into_owned();
    App {
        subtitle: abbreviate_home(&path_str, home),
        path: path_str,
        title,
        names,
    }
}

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
            title: title.into(),
            subtitle: path.into(),
            names: names.iter().map(|name| Candidate::new(name)).collect(),
        }
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
