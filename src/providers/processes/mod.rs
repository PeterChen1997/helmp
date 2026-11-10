//! 进程与端口：列出当前用户的进程（CPU / 内存，GUI App 带图标），按名字搜；查谁占着某个端口。
//!
//! - 范围「进程」：前缀 `kill ` / `进程 ` / `ps `。空查询按 CPU 从高到低列出；输入文字按名字（含拼音）搜。
//!   范围里输入 `:3000` / `3000` / `port 3000` 列出监听这个端口的进程，只输 `:` 列出全部监听端口。
//! - 全局搜索：`port 3000` / `端口 3000` / `:3000` 直接把占用端口的进程顶到第一条；
//!   「进程」「端口」「强制退出」等关键词给出进入范围的入口。
//! - 动作：回车退出（App 走 `NSRunningApplication.terminate`，普通进程发 SIGTERM）；⌘↵ 强制退出
//!   （SIGKILL，先出确认页）；⌘C 复制 PID。系统关键进程（PID ≤ 1、root、WindowServer 等）连普通退出也要确认；
//!   Helmp 自己不能被结束。
//! - 进程表来自 `ps`、端口来自 `lsof -nP -iTCP -sTCP:LISTEN`（只看得到当前用户的进程，正好）。
//!   范围内每次按键都会查询：进程表缓存 2 秒。
//! - 所有结束进程的动作都先过 `effects::dry`：演练模式下打印 `[dry-run] kill SIGTERM <pid> <名字>`。

mod parse;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};

use crate::i18n::{tr, trf};
use crate::matcher;
use crate::provider::{
    Accessory, Action, Detail, Icon, Item, NamedAction, Outcome, Provider, Scope, Tint, Tone,
};
use parse::{Listener, Proc, parse_lsof, parse_ps, port_query};

const PROVIDER_ID: &str = "processes";
const PREFIXES: [&str; 3] = ["kill ", "进程 ", "ps "];
/// 进程表缓存多久（范围里每次按键都查询）。
const CACHE_TTL: Duration = Duration::from_secs(2);
/// 端口查询命中时的分数：压过应用（≤1000）与计算器（1100），用户明确在问端口。
const PORT_SCORE: u32 = 1200;
/// 范围内一次最多列多少个进程。
const LIMIT: usize = 150;

/// 结束了会让系统没法用（或立即被系统拉起来）的进程：普通退出也要确认。
const CRITICAL_NAMES: [&str; 12] = [
    "launchd",
    "kernel_task",
    "WindowServer",
    "loginwindow",
    "Dock",
    "Finder",
    "SystemUIServer",
    "ControlCenter",
    "coreaudiod",
    "cfprefsd",
    "distnoted",
    "UserEventAgent",
];

/// 全局搜索里进入范围的入口：(关键词, 进入后的初始输入)。
const ENTRY_KEYWORDS: [&str; 9] = [
    "进程",
    "杀进程",
    "结束进程",
    "强制退出",
    "退出应用",
    "kill process",
    "process",
    "force quit",
    "quit app",
];
const PORT_KEYWORDS: [&str; 5] = ["端口", "占用端口", "监听端口", "port", "listening ports"];

/// 一个可以被结束的进程（进程行和端口行共用）。
#[derive(Debug, Clone)]
struct Target {
    pid: i32,
    /// 显示名：App 用本地化名字，否则进程名。
    name: String,
    /// GUI App：走 `terminate`，图标用 App 图标。
    app_bundle: Option<PathBuf>,
    critical: bool,
}

#[derive(Debug, Clone)]
struct Row {
    target: Target,
    proc_name: String,
    cpu: f32,
    rss_kb: u64,
}

#[derive(Default)]
pub struct Processes {
    cache: Mutex<Option<(Instant, Arc<Vec<Row>>)>>,
}

impl Provider for Processes {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let text = query.trim();
        if text.is_empty() {
            return Vec::new();
        }
        // 全局只认明确的端口写法（`3000` 单独一个数字更可能是算式）。
        if !text.starts_with(|c: char| c.is_ascii_digit())
            && let Some(Some(port)) = port_query(text)
        {
            return self.port_items(Some(port), PORT_SCORE);
        }
        let mut items = Vec::new();
        if let Some(score) = best(text, &ENTRY_KEYWORDS) {
            items.push(entry(
                "processes:open",
                tr("进程", "Processes"),
                tr(
                    "查看、退出或强制退出正在运行的进程",
                    "View, quit, or force quit running processes",
                ),
                "activity",
                score,
                Action::enter_scope(PROVIDER_ID),
            ));
        }
        if let Some(score) = best(text, &PORT_KEYWORDS) {
            items.push(entry(
                "processes:ports",
                tr("监听端口", "Listening Ports"),
                tr(
                    "谁占着哪个端口；输入端口号筛选",
                    "Which process is listening on which port",
                ),
                "plug",
                score,
                Action::new(|| Ok(Outcome::EnterScopeWith(PROVIDER_ID.into(), ":".into()))),
            ));
        }
        items
    }

    fn scope(&self) -> Option<Scope> {
        Some(Scope {
            id: PROVIDER_ID.into(),
            label: tr("进程", "Processes").into(),
            tint: Tint::Red,
            prefixes: PREFIXES.iter().map(|p| p.to_string()).collect(),
            placeholder: tr(
                "按名字搜进程，输入 :3000 查端口",
                "Search processes; type :3000 for a port",
            )
            .into(),
            filters: Vec::new(),
        })
    }

    fn query_scoped(&self, query: &str, _filter: &str) -> Vec<Item> {
        if let Some(port) = port_query(query) {
            return self.port_items(port, PORT_SCORE);
        }
        let rows = match self.rows() {
            Ok(rows) => rows,
            Err(err) => return vec![error_item(&err)],
        };
        let text = query.trim();
        let mut hits: Vec<(&Row, u32)> = rows
            .iter()
            .filter_map(|row| {
                if text.is_empty() {
                    return Some((row, 0));
                }
                let score = [row.target.name.as_str(), row.proc_name.as_str()]
                    .into_iter()
                    .filter_map(|name| matcher::score(text, name))
                    .chain(text.parse::<i32>().ok().filter(|pid| *pid == row.target.pid).map(|_| 1000))
                    .max()?;
                Some((row, score))
            })
            .collect();
        hits.sort_by(|(a, sa), (b, sb)| {
            sb.cmp(sa)
                .then(b.cpu.total_cmp(&a.cpu))
                .then(b.rss_kb.cmp(&a.rss_kb))
        });
        hits.into_iter()
            .take(LIMIT)
            .enumerate()
            .map(|(rank, (row, _))| process_item(row, 10_000 - rank as u32))
            .collect()
    }
}

impl Processes {
    /// 当前用户的进程（缓存 `CACHE_TTL`）。
    fn rows(&self) -> Result<Arc<Vec<Row>>> {
        let mut cache = self.cache.lock().expect("process cache poisoned");
        if let Some((at, rows)) = cache.as_ref()
            && at.elapsed() < CACHE_TTL
        {
            return Ok(Arc::clone(rows));
        }
        let rows = Arc::new(snapshot()?);
        *cache = Some((Instant::now(), Arc::clone(&rows)));
        Ok(rows)
    }

    fn port_items(&self, port: Option<u16>, score: u32) -> Vec<Item> {
        let listeners = match listeners(port) {
            Ok(listeners) => listeners,
            Err(err) => return vec![error_item(&err)],
        };
        if listeners.is_empty() {
            // 查不到要明说，否则用户分不清是没人占用还是功能坏了。
            let title = match port {
                Some(port) => trf!("没有进程在监听 {port} 端口", "Nothing is listening on port {port}"),
                None => tr("没有监听中的 TCP 端口", "No listening TCP ports").into(),
            };
            return vec![Item {
                id: "processes:port-none".into(),
                title,
                subtitle: tr(
                    "只能看到当前用户的进程；系统服务的端口需要管理员权限才能查",
                    "Only your own processes are visible; system services need admin rights",
                )
                .into(),
                badge: tr("端口", "Ports").into(),
                score,
                icon: Icon::Symbol {
                    name: "plug",
                    tint: Tint::Gray,
                },
                action_title: String::new(),
                ..Item::blank()
            }];
        }
        let apps = running_apps();
        let rows = self.rows().ok();
        listeners
            .into_iter()
            .enumerate()
            .map(|(rank, listener)| {
                let row = rows
                    .as_ref()
                    .and_then(|rows| rows.iter().find(|row| row.target.pid == listener.pid));
                let target = match row {
                    Some(row) => row.target.clone(),
                    None => target(listener.pid, &listener.command, 0, &apps),
                };
                port_item(&listener, target, row, score.saturating_sub(rank as u32))
            })
            .collect()
    }
}

fn best(text: &str, keywords: &[&str]) -> Option<u32> {
    keywords
        .iter()
        .filter_map(|keyword| matcher::score(text, keyword))
        .max()
}

fn entry(
    id: &str,
    title: &str,
    subtitle: &str,
    icon: &'static str,
    score: u32,
    action: Action,
) -> Item {
    Item {
        id: id.into(),
        title: title.into(),
        subtitle: subtitle.into(),
        badge: badge().into(),
        score,
        icon: Icon::Symbol {
            name: icon,
            tint: Tint::Red,
        },
        action,
        action_title: tr("打开", "Open").into(),
        ..Item::blank()
    }
}

fn badge() -> &'static str {
    tr("进程", "Processes")
}

fn error_item(err: &anyhow::Error) -> Item {
    Item {
        id: "processes:error".into(),
        title: tr("读取进程列表失败", "Couldn’t list processes").into(),
        subtitle: format!("{err:#}"),
        badge: badge().into(),
        icon: Icon::Symbol {
            name: "circle-x",
            tint: Tint::Red,
        },
        ..Item::blank()
    }
}

fn own_pid() -> i32 {
    i32::try_from(std::process::id()).unwrap_or(i32::MAX)
}

/// GUI App：pid → (本地化名字, .app 路径)。
fn running_apps() -> HashMap<i32, (String, Option<PathBuf>)> {
    objc2::rc::autoreleasepool(|_| {
        NSWorkspace::sharedWorkspace()
            .runningApplications()
            .iter()
            .filter(|app| app.activationPolicy() == NSApplicationActivationPolicy::Regular)
            .filter_map(|app| {
                let name = app.localizedName()?.to_string();
                let bundle = app
                    .bundleURL()
                    .and_then(|url| url.path())
                    .map(|path| PathBuf::from(path.to_string()));
                Some((app.processIdentifier(), (name, bundle)))
            })
            .collect()
    })
}

fn target(pid: i32, proc_name: &str, uid: u32, apps: &HashMap<i32, (String, Option<PathBuf>)>) -> Target {
    let app = apps.get(&pid);
    Target {
        pid,
        name: app.map_or_else(|| proc_name.to_string(), |(name, _)| name.clone()),
        app_bundle: app.and_then(|(_, bundle)| bundle.clone()),
        critical: is_critical(pid, uid, proc_name),
    }
}

fn is_critical(pid: i32, uid: u32, name: &str) -> bool {
    pid <= 1 || uid == 0 || CRITICAL_NAMES.contains(&name)
}

/// 当前用户的进程（Helmp 自己除外：它不能被自己结束）。
fn snapshot() -> Result<Vec<Row>> {
    let output = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,uid=,pcpu=,rss=,comm="])
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .output()
        .context(tr("运行 ps 失败", "Couldn’t run ps"))?;
    if !output.status.success() {
        bail!(trf!(
            "ps 失败（{}）",
            "ps failed ({})",
            output.status
        ));
    }
    // SAFETY: 无参数的只读查询。
    let uid = unsafe { libc::getuid() };
    let apps = running_apps();
    let own = own_pid();
    Ok(parse_ps(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .filter(|proc| proc.uid == uid && proc.pid != own)
        .map(|proc: Proc| Row {
            target: target(proc.pid, proc.name(), proc.uid, &apps),
            proc_name: proc.name().to_string(),
            cpu: proc.cpu,
            rss_kb: proc.rss_kb,
        })
        .collect())
}

/// 监听中的 TCP 端口；`port` 为 None 时列出全部。没有任何监听时 lsof 退出码是 1，不算错。
fn listeners(port: Option<u16>) -> Result<Vec<Listener>> {
    let filter = match port {
        Some(port) => format!("-iTCP:{port}"),
        None => "-iTCP".into(),
    };
    let output = Command::new("/usr/sbin/lsof")
        .args(["-nP", &filter, "-sTCP:LISTEN", "-Fpcn"])
        .output()
        .context(tr("运行 lsof 失败", "Couldn’t run lsof"))?;
    let own = own_pid();
    let mut listeners: Vec<Listener> = parse_lsof(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .filter(|listener| listener.pid != own)
        .collect();
    listeners.sort_by_key(|listener| (listener.port, listener.pid));
    Ok(listeners)
}

fn memory(rss_kb: u64) -> String {
    let mb = rss_kb as f64 / 1024.;
    if mb >= 1024. {
        format!("{:.1} GB", mb / 1024.)
    } else {
        format!("{mb:.0} MB")
    }
}

fn icon(target: &Target) -> Icon {
    match &target.app_bundle {
        Some(bundle) => Icon::File(bundle.clone()),
        None => Icon::Symbol {
            name: "cpu",
            tint: Tint::Graphite,
        },
    }
}

fn process_item(row: &Row, score: u32) -> Item {
    let target = &row.target;
    let subtitle = format!(
        "pid {} · {:.1}% CPU · {}",
        target.pid,
        row.cpu,
        memory(row.rss_kb)
    );
    let mut item = with_actions(target.clone());
    item.id = format!("{PROVIDER_ID}:{}", target.pid);
    item.title = target.name.clone();
    item.subtitle = if target.name == row.proc_name {
        subtitle
    } else {
        format!("{subtitle} · {}", row.proc_name)
    };
    item.score = score;
    item
}

fn port_item(listener: &Listener, target: Target, row: Option<&Row>, score: u32) -> Item {
    let mut item = with_actions(target);
    item.id = format!("{PROVIDER_ID}:port:{}:{}", listener.port, listener.pid);
    item.title = format!(":{} · {}", listener.port, item.title);
    let mut subtitle = trf!(
        "pid {} · 监听 {}:{}",
        "pid {} · listening on {}:{}",
        listener.pid,
        listener.address,
        listener.port
    );
    if let Some(row) = row {
        subtitle = format!("{subtitle} · {}", memory(row.rss_kb));
    }
    item.subtitle = subtitle;
    item.badge = tr("端口", "Ports").into();
    item.score = score;
    item
}

/// 结果的骨架：标题、图标、回车退出、⌘K 强制退出 / 复制 PID。
fn with_actions(target: Target) -> Item {
    let quit = quit_action(&target);
    let force = force_quit_action(&target);
    let mut accessories = Vec::new();
    if target.critical {
        accessories.push(Accessory::Tag {
            text: tr("系统", "System").into(),
            tone: Tone::Waiting,
        });
    }
    Item {
        title: target.name.clone(),
        badge: badge().into(),
        icon: icon(&target),
        accessories,
        action: quit,
        action_title: tr("退出", "Quit").into(),
        actions: vec![
            NamedAction::new(tr("强制退出", "Force Quit"), Some("⌘↵"), force),
            NamedAction::new(
                tr("复制 PID", "Copy PID"),
                Some("⌘C"),
                Action::copy(target.pid.to_string()),
            ),
        ],
        ..Item::blank()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signal {
    Term,
    Kill,
}

impl Signal {
    fn name(self) -> &'static str {
        match self {
            Signal::Term => "SIGTERM",
            Signal::Kill => "SIGKILL",
        }
    }

    fn number(self) -> i32 {
        match self {
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        }
    }
}

/// 回车：正常退出。系统关键进程先确认。
fn quit_action(target: &Target) -> Action {
    let run = {
        let target = target.clone();
        Action::new(move || quit(&target))
    };
    if !target.critical {
        return run;
    }
    let detail = Detail::confirm(
        icon(target),
        trf!("确定要退出「{}」吗？", "Quit “{}”?", target.name),
        tr(
            "这是系统进程，退出后系统可能工作异常或立即把它重新启动。",
            "This is a system process. Quitting it may make macOS misbehave, or macOS may restart it right away.",
        )
        .into(),
        tr("退出", "Quit").into(),
        run,
    );
    Action::new(move || Ok(Outcome::Show(detail.clone()))).keep_open()
}

/// ⌘↵：强制退出（SIGKILL），一律先确认。
fn force_quit_action(target: &Target) -> Action {
    let run = {
        let target = target.clone();
        Action::new(move || send(&target, Signal::Kill))
    };
    let detail = Detail::confirm(
        icon(target),
        trf!(
            "确定要强制退出「{}」吗？",
            "Force quit “{}”?",
            target.name
        ),
        trf!(
            "进程 {} 会立即结束（SIGKILL），没保存的内容会丢失。",
            "Process {} will stop immediately (SIGKILL). Unsaved work will be lost.",
            target.pid
        ),
        tr("强制退出", "Force Quit").into(),
        run,
    );
    Action::new(move || Ok(Outcome::Show(detail.clone()))).keep_open()
}

/// Helmp 自己不能被结束（面板会直接消失，用户以为崩溃了）。
fn guard(pid: i32) -> Result<()> {
    if pid == own_pid() {
        bail!(tr("不能用 Helmp 结束 Helmp 自己", "Helmp can’t quit itself from here"));
    }
    Ok(())
}

fn quit(target: &Target) -> Result<Outcome> {
    guard(target.pid)?;
    if target.app_bundle.is_none() {
        return send(target, Signal::Term);
    }
    if let Some(outcome) =
        crate::effects::dry("terminate", &format!("{} {}", target.pid, target.name))
    {
        return Ok(outcome);
    }
    let accepted = objc2::rc::autoreleasepool(|_| {
        NSRunningApplication::runningApplicationWithProcessIdentifier(target.pid)
            .map(|app| app.terminate())
    });
    match accepted {
        Some(true) => Ok(Outcome::Done),
        Some(false) => bail!(trf!(
            "「{}」没有响应退出请求，可以试试强制退出（⌘↵）",
            "“{}” didn’t accept the quit request. Try Force Quit (⌘↵).",
            target.name
        )),
        None => bail!(trf!(
            "「{}」已经退出了",
            "“{}” has already quit",
            target.name
        )),
    }
}

fn send(target: &Target, signal: Signal) -> Result<Outcome> {
    guard(target.pid)?;
    if let Some(outcome) = crate::effects::dry(
        "kill",
        &format!("{} {} {}", signal.name(), target.pid, target.name),
    ) {
        return Ok(outcome);
    }
    // SAFETY: kill(2) 只读参数；pid 已排除 Helmp 自己。
    if unsafe { libc::kill(target.pid, signal.number()) } == 0 {
        return Ok(Outcome::Done);
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ESRCH) => bail!(trf!(
            "进程 {}（{}）已经不在了",
            "Process {} ({}) is no longer running",
            target.pid,
            target.name
        )),
        Some(libc::EPERM) => bail!(trf!(
            "没有权限结束进程 {}（{}）",
            "Not allowed to stop process {} ({})",
            target.pid,
            target.name
        )),
        _ => Err(err).context(trf!(
            "结束进程 {} 失败",
            "Couldn’t stop process {}",
            target.pid
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(critical: bool) -> Target {
        Target {
            pid: 4242,
            name: "sleep".into(),
            app_bundle: None,
            critical,
        }
    }

    #[test]
    fn system_processes_need_confirmation_even_to_quit() {
        // 误杀 WindowServer / loginwindow 会让用户直接被登出：普通退出也必须先出确认页。
        assert!(is_critical(1, 501, "launchd"));
        assert!(is_critical(300, 501, "WindowServer"));
        assert!(is_critical(300, 0, "anything"));
        assert!(!is_critical(300, 501, "node"));
        let detail = match quit_action(&target(true)).run().unwrap() {
            Outcome::Show(detail) => detail,
            other => panic!("关键进程应先确认：{other:?}"),
        };
        assert_eq!(detail.actions.len(), 2);
    }

    #[test]
    fn force_quit_always_confirms_first() {
        // SIGKILL 不给进程保存的机会：回车只能出确认页，不能直接杀。
        assert!(matches!(
            force_quit_action(&target(false)).run().unwrap(),
            Outcome::Show(_)
        ));
    }

    #[test]
    fn helmp_refuses_to_kill_itself() {
        // 自己杀自己面板直接消失，用户会以为崩溃了。
        let me = Target {
            pid: own_pid(),
            ..target(false)
        };
        assert!(send(&me, Signal::Kill).is_err());
        assert!(quit(&me).is_err());
    }

    #[test]
    fn memory_is_human_readable() {
        assert_eq!(memory(204_800), "200 MB");
        assert_eq!(memory(3 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn symbol_icons_exist_in_bundled_assets() {
        use gpui_kit::AssetSource as _;
        for name in ["activity", "plug", "cpu", "circle-x"] {
            let path = format!("icons/{name}.svg");
            assert!(
                gpui_kit::assets::AllAssets.load(&path).unwrap().is_some(),
                "缺少图标 {path}"
            );
        }
    }
}
