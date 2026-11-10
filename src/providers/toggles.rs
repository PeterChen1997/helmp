//! 系统开关：防休眠、音量 / 静音、显示或隐藏隐藏文件、弹出所有磁盘。
//! （切换深色模式已在 `system.rs`，这里不重复。）
//!
//! - 防休眠：由 Helmp 管理一个 `caffeinate -d -i` 子进程，带 `-w <Helmp 的 pid>`，Helmp 退出时它跟着退出，
//!   不会留下孤儿进程。`防休眠 2h` / `caffeinate 30m` 到时自动关闭；开着时同一条结果变成「关闭防休眠」，
//!   右侧标签显示剩余时间。
//! - 音量：`音量 30` / `vol 30` 设置输出音量（同时取消静音），`静音` / `取消静音`，都走 osascript。
//! - 显示隐藏文件：`defaults write com.apple.finder AppleShowAllFiles` 后重启访达（会关掉访达窗口、
//!   打断进行中的拷贝），所以先出确认页；弹出所有磁盘同样先确认。确认规则与系统命令一致：
//!   设置 › 扩展 › 系统命令里可关闭，查询末尾加 ` !` 直接执行。
//! - 执行层是可替换的函数（`Executor`），单测只记录会执行哪个 `Op`；`HELMP_DRY_RUN=1` 时只回报命令行。

use std::path::PathBuf;
use std::process::{Child, Command as Process};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

use crate::config;
use crate::i18n::{tr, trf};
use crate::matcher;
use crate::provider::{
    Accessory, Action, Detail, Icon, Item, NamedAction, Outcome, Provider, Tint, Tone,
};

use super::scripts::args::strip_confirmation;

/// 一次要执行的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// 开启防休眠，`None` = 一直开着直到手动关闭。
    Caffeinate(Option<u64>),
    /// 关闭 Helmp 开启的防休眠。
    Decaffeinate,
    /// 把输出音量设为 0–100，并取消静音。
    Volume(u8),
    Mute(bool),
    /// 显示（true）或隐藏（false）隐藏文件，然后重启访达。
    ShowHiddenFiles(bool),
    /// 弹出所有可弹出的磁盘（外置盘、磁盘映像、网络卷）。
    EjectAll,
}

const CAFFEINATE: &str = "/usr/bin/caffeinate";
const OSASCRIPT: &str = "/usr/bin/osascript";
const DEFAULTS: &str = "/usr/bin/defaults";
const KILLALL: &str = "/usr/bin/killall";
const EJECT_SCRIPT: &str =
    r#"tell application "Finder" to eject (every disk whose ejectable is true)"#;

impl Op {
    /// 要依次运行的命令（程序 + 参数）。防休眠的开关不在这里：它是常驻子进程，见 `caffeinate_args`。
    fn commands(&self) -> Vec<Vec<String>> {
        let osascript = |script: String| vec![OSASCRIPT.to_string(), "-e".into(), script];
        match self {
            Op::Caffeinate(_) | Op::Decaffeinate => Vec::new(),
            Op::Volume(level) => vec![osascript(format!(
                "set volume output volume {level} without output muted"
            ))],
            Op::Mute(true) => vec![osascript("set volume with output muted".into())],
            Op::Mute(false) => vec![osascript("set volume without output muted".into())],
            Op::ShowHiddenFiles(show) => vec![
                [
                    DEFAULTS,
                    "write",
                    "com.apple.finder",
                    "AppleShowAllFiles",
                    "-bool",
                    if *show { "true" } else { "false" },
                ]
                .map(str::to_string)
                .to_vec(),
                vec![KILLALL.to_string(), "Finder".into()],
            ],
            Op::EjectAll => vec![osascript(EJECT_SCRIPT.into())],
        }
    }

    /// 给人看的命令行（dry-run 输出）。
    pub fn render(&self) -> String {
        match self {
            Op::Caffeinate(_) => caffeinate_args(self)
                .map(|args| {
                    std::iter::once(CAFFEINATE.to_string())
                        .chain(args)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default(),
            Op::Decaffeinate => tr(
                "结束 Helmp 开启的 caffeinate 进程",
                "stop the caffeinate process started by Helmp",
            )
            .into(),
            _ => self
                .commands()
                .iter()
                .map(|command| {
                    command
                        .iter()
                        .map(|part| {
                            if part.contains(' ') {
                                format!("'{part}'")
                            } else {
                                part.clone()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect::<Vec<_>>()
                .join(" && "),
        }
    }
}

/// `caffeinate` 的参数：阻止显示器与系统空闲睡眠，Helmp 退出时跟着退出。
fn caffeinate_args(op: &Op) -> Option<Vec<String>> {
    let Op::Caffeinate(seconds) = op else {
        return None;
    };
    let mut args = vec!["-d".to_string(), "-i".to_string()];
    if let Some(seconds) = seconds {
        args.extend(["-t".to_string(), seconds.to_string()]);
    }
    args.extend(["-w".to_string(), std::process::id().to_string()]);
    Some(args)
}

/// Helmp 开启的防休眠进程。
struct Awake {
    child: Child,
    started: Instant,
    duration: Option<Duration>,
}

static AWAKE: Mutex<Option<Awake>> = Mutex::new(None);

/// 防休眠的当前状态：None = 没开；Some(None) = 一直开着；Some(Some(剩余)) = 到时自动关闭。
fn awake_state() -> Option<Option<Duration>> {
    let mut guard = AWAKE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let awake = guard.as_mut()?;
    // 到时自己退出了（-t），或被别人结束了。
    if !matches!(awake.child.try_wait(), Ok(None)) {
        *guard = None;
        return None;
    }
    Some(
        awake
            .duration
            .map(|total| total.saturating_sub(awake.started.elapsed())),
    )
}

/// 执行层：真实实现见 `execute`，测试换成只记录的函数。
pub type Executor = Arc<dyn Fn(&Op) -> Result<()> + Send + Sync>;

/// 防休眠状态：测试里换成固定值。
type AwakeProbe = fn() -> Option<Option<Duration>>;

/// 显示隐藏文件的当前设置：测试里换成固定值。
type HiddenProbe = fn() -> bool;

pub struct Toggles {
    exec: Executor,
    confirm_required: fn() -> bool,
    awake: AwakeProbe,
    hidden_shown: HiddenProbe,
}

impl Toggles {
    pub fn new() -> Self {
        Self {
            exec: Arc::new(execute),
            confirm_required: || config::get().system_commands.confirm_dangerous,
            awake: awake_state,
            hidden_shown: hidden_files_shown,
        }
    }
}

fn badge() -> &'static str {
    tr("系统开关", "System Toggles")
}

const fn symbol(name: &'static str, tint: Tint) -> Icon {
    Icon::Symbol { name, tint }
}

const AWAKE_KEYWORDS: &[&str] = &[
    "防休眠",
    "防止休眠",
    "保持唤醒",
    "咖啡",
    "caffeinate",
    "keep awake",
    "awake",
    "amphetamine",
];
const VOLUME_KEYWORDS: &[&str] = &["音量", "声音大小", "volume", "vol"];
const MUTE_KEYWORDS: &[&str] = &["静音", "mute"];
const UNMUTE_KEYWORDS: &[&str] = &["取消静音", "unmute"];
const HIDDEN_KEYWORDS: &[&str] = &[
    "隐藏文件",
    "显示隐藏文件",
    "隐藏隐藏文件",
    "hidden files",
    "show hidden files",
    "hide hidden files",
    "dotfiles",
];
const EJECT_KEYWORDS: &[&str] = &[
    "弹出所有磁盘",
    "弹出磁盘",
    "推出磁盘",
    "eject all disks",
    "eject all",
    "eject",
];

/// 关键词里的最高分。
fn best(text: &str, keywords: &[&str]) -> Option<u32> {
    keywords
        .iter()
        .filter_map(|keyword| matcher::score(text, keyword))
        .max()
}

/// 把查询拆成「关键词部分」和末尾的参数（如 `防休眠 2h` → (`防休眠`, `2h`)）。
fn split_argument(text: &str) -> (&str, Option<&str>) {
    match text.rsplit_once(char::is_whitespace) {
        Some((head, tail))
            if !head.trim().is_empty() && tail.chars().any(|c| c.is_ascii_digit()) =>
        {
            (head.trim(), Some(tail))
        }
        _ => (text, None),
    }
}

/// `90`（分钟）、`30m`、`2h`、`1h30m`、`45s`、`2小时`、`30分钟` → 秒。
pub fn parse_duration(text: &str) -> Option<u64> {
    let text = text
        .trim()
        .to_lowercase()
        .replace("小时", "h")
        .replace("分钟", "m")
        .replace('分', "m")
        .replace('秒', "s");
    if text.is_empty() {
        return None;
    }
    if let Ok(minutes) = text.parse::<u64>() {
        return (minutes > 0).then(|| minutes * 60);
    }
    let mut total = 0u64;
    let mut number = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            number.push(ch);
            continue;
        }
        let value: f64 = number.parse().ok()?;
        number.clear();
        let unit = match ch {
            'h' => 3600.0,
            'm' => 60.0,
            's' => 1.0,
            _ => return None,
        };
        total += (value * unit).round() as u64;
    }
    if !number.is_empty() {
        return None;
    }
    (total > 0).then_some(total)
}

/// `30`、`30%` → 30（0–100）。
fn parse_level(text: &str) -> Option<u8> {
    let level: u32 = text.trim().trim_end_matches('%').parse().ok()?;
    u8::try_from(level).ok().filter(|level| *level <= 100)
}

/// `1:42`（时:分）或 `12 分钟`。
fn remaining_label(remaining: Duration) -> String {
    let minutes = remaining.as_secs().div_ceil(60);
    if minutes >= 60 {
        trf!("剩余 {}:{:02}", "{}:{:02} left", minutes / 60, minutes % 60)
    } else {
        trf!("剩余 {minutes} 分钟", "{minutes} min left")
    }
}

fn duration_label(seconds: u64) -> String {
    let (h, m) = (seconds / 3600, (seconds % 3600).div_ceil(60));
    match (h, m) {
        (0, m) => trf!("{m} 分钟", "{m} min"),
        (h, 0) => trf!("{h} 小时", "{h} h"),
        (h, m) => trf!("{h} 小时 {m} 分钟", "{h} h {m} min"),
    }
}

impl Provider for Toggles {
    fn id(&self) -> &'static str {
        "toggles"
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let (query, confirmed) = match strip_confirmation(query) {
            Some(stripped) => (stripped, true),
            None => (query, false),
        };
        let (head, argument) = split_argument(query);
        let mut items = Vec::new();

        // 防休眠：带时长参数时只认 `关键词 时长`。
        let awake_hit = match argument.map(parse_duration) {
            Some(Some(seconds)) => best(head, AWAKE_KEYWORDS).map(|score| (score, Some(seconds))),
            Some(None) => None,
            None => best(query, AWAKE_KEYWORDS).map(|score| (score, None)),
        };
        if let Some((score, seconds)) = awake_hit {
            items.push(self.awake_item(score, seconds));
        }

        // 音量：`音量 30`；没有数字时给出用法提示。
        match argument.map(parse_level) {
            Some(Some(level)) => {
                if let Some(score) = best(head, VOLUME_KEYWORDS) {
                    items.push(self.volume_item(score, Some(level)));
                }
            }
            Some(None) => {}
            None => {
                if let Some(score) = best(query, VOLUME_KEYWORDS) {
                    items.push(self.volume_item(score, None));
                }
            }
        }
        if argument.is_none() {
            if let Some(score) = best(query, MUTE_KEYWORDS) {
                items.push(self.simple_item(
                    "mute",
                    tr("静音", "Mute"),
                    tr("关闭声音输出", "Silence the sound output"),
                    symbol("volume-x", Tint::Gray),
                    score,
                    Op::Mute(true),
                ));
            }
            if let Some(score) = best(query, UNMUTE_KEYWORDS) {
                items.push(self.simple_item(
                    "unmute",
                    tr("取消静音", "Unmute"),
                    tr("恢复声音输出", "Turn the sound back on"),
                    symbol("volume-2", Tint::Blue),
                    score,
                    Op::Mute(false),
                ));
            }
            if let Some(score) = best(query, HIDDEN_KEYWORDS) {
                items.push(self.hidden_files_item(score, confirmed));
            }
            if let Some(score) = best(query, EJECT_KEYWORDS) {
                items.push(self.eject_item(score, confirmed));
            }
        }
        items
    }
}

impl Toggles {
    fn awake_item(&self, score: u32, seconds: Option<u64>) -> Item {
        let icon = symbol("coffee", Tint::Orange);
        match (self.awake)() {
            Some(remaining) => Item {
                id: "toggles:caffeinate".into(),
                title: tr("关闭防休眠", "Turn Off Keep Awake").into(),
                subtitle: tr(
                    "防休眠已开启，回车让电脑恢复正常睡眠",
                    "Keep Awake is on; press Return to let the Mac sleep normally",
                )
                .into(),
                badge: badge().into(),
                score,
                icon,
                accessories: vec![Accessory::Tag {
                    text: match remaining {
                        Some(left) => remaining_label(left),
                        None => tr("已开启", "On").into(),
                    },
                    tone: Tone::Running,
                }],
                action: run(Arc::clone(&self.exec), Op::Decaffeinate),
                action_title: tr("关闭", "Turn Off").into(),
                ..Item::blank()
            },
            None => {
                let subtitle = match seconds {
                    Some(seconds) => trf!(
                        "阻止电脑和显示器睡眠，{} 后自动关闭",
                        "Keep the Mac and display awake; turns off after {}",
                        duration_label(seconds)
                    ),
                    None => tr(
                        "阻止电脑和显示器睡眠，再执行一次关闭 · 加时长如「防休眠 2h」到时自动关闭",
                        "Keep the Mac and display awake until you turn it off · Add a duration like “caffeinate 2h”",
                    )
                    .into(),
                };
                let presets = [
                    (tr("1 小时后自动关闭", "Turn Off After 1 Hour"), 3600),
                    (tr("2 小时后自动关闭", "Turn Off After 2 Hours"), 7200),
                ];
                Item {
                    id: "toggles:caffeinate".into(),
                    title: tr("开启防休眠", "Keep Awake").into(),
                    subtitle,
                    badge: badge().into(),
                    score,
                    icon,
                    action: run(Arc::clone(&self.exec), Op::Caffeinate(seconds)),
                    action_title: tr("开启", "Turn On").into(),
                    actions: presets
                        .into_iter()
                        .map(|(title, seconds)| {
                            NamedAction::new(
                                title,
                                None,
                                run(Arc::clone(&self.exec), Op::Caffeinate(Some(seconds))),
                            )
                        })
                        .collect(),
                    ..Item::blank()
                }
            }
        }
    }

    fn volume_item(&self, score: u32, level: Option<u8>) -> Item {
        let (title, subtitle, action) = match level {
            Some(level) => (
                trf!("把音量调到 {level}%", "Set Volume to {level}%"),
                tr("设置输出音量并取消静音", "Set the output volume and unmute").to_string(),
                run(Arc::clone(&self.exec), Op::Volume(level)),
            ),
            None => (
                tr("音量", "Volume").to_string(),
                tr(
                    "在后面加数字设置音量，如「音量 30」",
                    "Add a number to set the volume, e.g. “vol 30”",
                )
                .to_string(),
                Action::new(|| {
                    Ok(Outcome::Message(
                        tr(
                            "在后面加 0–100 的数字，如「音量 30」",
                            "Add a number from 0 to 100, e.g. “vol 30”",
                        )
                        .into(),
                    ))
                })
                .keep_open(),
            ),
        };
        Item {
            id: "toggles:volume".into(),
            title,
            subtitle,
            badge: badge().into(),
            score,
            icon: symbol("volume-1", Tint::Blue),
            action,
            action_title: tr("设置", "Set").into(),
            ..Item::blank()
        }
    }

    fn simple_item(
        &self,
        key: &str,
        title: &str,
        subtitle: &str,
        icon: Icon,
        score: u32,
        op: Op,
    ) -> Item {
        Item {
            id: format!("toggles:{key}"),
            title: title.into(),
            subtitle: subtitle.into(),
            badge: badge().into(),
            score,
            icon,
            action: run(Arc::clone(&self.exec), op),
            action_title: tr("执行", "Run").into(),
            ..Item::blank()
        }
    }

    fn hidden_files_item(&self, score: u32, confirmed: bool) -> Item {
        let show = !(self.hidden_shown)();
        let (title, subtitle, question) = if show {
            (
                tr("显示隐藏文件", "Show Hidden Files"),
                tr(
                    "让访达显示以 . 开头的文件，会重启访达",
                    "Show dotfiles in Finder; Finder will relaunch",
                ),
                tr(
                    "显示隐藏文件并重启访达？",
                    "Show hidden files and relaunch Finder?",
                ),
            )
        } else {
            (
                tr("隐藏隐藏文件", "Hide Hidden Files"),
                tr(
                    "访达不再显示以 . 开头的文件，会重启访达",
                    "Hide dotfiles in Finder; Finder will relaunch",
                ),
                tr(
                    "隐藏隐藏文件并重启访达？",
                    "Hide hidden files and relaunch Finder?",
                ),
            )
        };
        self.guarded_item(
            "hidden-files",
            title,
            subtitle,
            symbol(if show { "eye" } else { "eye-off" }, Tint::Graphite),
            score,
            confirmed,
            question,
            tr(
                "访达会退出并重新打开：打开的访达窗口会关闭，正在进行的拷贝会中断。",
                "Finder will quit and reopen: its windows close and copies in progress are interrupted.",
            ),
            Op::ShowHiddenFiles(show),
        )
    }

    fn eject_item(&self, score: u32, confirmed: bool) -> Item {
        self.guarded_item(
            "eject-all",
            tr("弹出所有磁盘", "Eject All Disks"),
            tr(
                "外置磁盘、磁盘映像与网络卷",
                "External disks, disk images and network volumes",
            ),
            symbol("hard-drive", Tint::Gray),
            score,
            confirmed,
            tr("弹出所有磁盘？", "Eject all disks?"),
            tr(
                "所有可弹出的磁盘都会被推出，正在读写它们的应用可能报错。",
                "Every ejectable disk will be ejected; apps using them may report errors.",
            ),
            Op::EjectAll,
        )
    }

    /// 需要确认的开关：与系统命令同一套规则（设置可关，末尾 ` !` 视为已确认）。
    #[allow(clippy::too_many_arguments)]
    fn guarded_item(
        &self,
        key: &str,
        title: &str,
        subtitle: &str,
        icon: Icon,
        score: u32,
        confirmed: bool,
        question: &str,
        consequence: &str,
        op: Op,
    ) -> Item {
        let execute = run(Arc::clone(&self.exec), op);
        let needs_page = (self.confirm_required)() && !confirmed;
        let (subtitle, accessories, action) = if needs_page {
            let detail = Detail {
                // 这类操作能恢复，不该挂「无法撤销」。
                tag: None,
                ..Detail::confirm(
                    icon.clone(),
                    question.into(),
                    consequence.into(),
                    title.into(),
                    execute,
                )
            };
            (
                trf!(
                    "{subtitle} · 回车后需确认",
                    "{subtitle} · Asks for confirmation"
                ),
                vec![Accessory::Tag {
                    text: tr("需确认", "Confirm").into(),
                    tone: Tone::Waiting,
                }],
                Action::new(move || Ok(Outcome::Show(detail.clone()))).keep_open(),
            )
        } else {
            (subtitle.to_string(), Vec::new(), execute)
        };
        Item {
            id: format!("toggles:{key}"),
            title: title.into(),
            subtitle,
            badge: badge().into(),
            score,
            icon,
            accessories,
            action,
            action_title: tr("执行", "Run").into(),
            ..Item::blank()
        }
    }
}

fn run(exec: Executor, op: Op) -> Action {
    Action::new(move || {
        if let Some(outcome) = crate::effects::dry("run", &op.render()) {
            return Ok(outcome);
        }
        exec(&op)?;
        Ok(Outcome::Done)
    })
}

/// 真实执行。
fn execute(op: &Op) -> Result<()> {
    match op {
        Op::Caffeinate(seconds) => start_awake(op, *seconds),
        Op::Decaffeinate => {
            if let Some(mut awake) = AWAKE.lock().unwrap_or_else(|p| p.into_inner()).take() {
                awake.child.kill().ok();
                awake.child.wait().ok();
            }
            Ok(())
        }
        _ => {
            for command in op.commands() {
                let (program, args) = command.split_first().context("empty command")?;
                let output = Process::new(program)
                    .args(args)
                    .env("LANG", "en_US.UTF-8")
                    .env("LC_ALL", "en_US.UTF-8")
                    .output()
                    .with_context(|| trf!("启动 {program} 失败", "Couldn’t start {program}"))?;
                // 访达没在运行时 killall 返回 1，不算失败：设置已经写进去了。
                if !output.status.success() && program != KILLALL {
                    bail!(trf!(
                        "{program} 失败（{}）：{}",
                        "{program} failed ({}): {}",
                        output.status,
                        String::from_utf8_lossy(&output.stderr).trim()
                    ));
                }
            }
            Ok(())
        }
    }
}

fn start_awake(op: &Op, seconds: Option<u64>) -> Result<()> {
    let args = caffeinate_args(op).context("not a caffeinate op")?;
    let mut guard = AWAKE.lock().unwrap_or_else(|p| p.into_inner());
    // 已经开着：换成新的时长。
    if let Some(mut old) = guard.take() {
        old.child.kill().ok();
        old.child.wait().ok();
    }
    let child = Process::new(CAFFEINATE)
        .args(&args)
        .spawn()
        .with_context(|| trf!("启动 {CAFFEINATE} 失败", "Couldn’t start {CAFFEINATE}"))?;
    *guard = Some(Awake {
        child,
        started: Instant::now(),
        duration: seconds.map(Duration::from_secs),
    });
    Ok(())
}

/// 访达当前是否显示隐藏文件：读 `~/Library/Preferences/com.apple.finder.plist`（HOME 相对，测试里是隔离目录）。
/// 读不到按「未显示」处理。
fn hidden_files_shown() -> bool {
    let Ok(home) = crate::paths::home_dir() else {
        return false;
    };
    let path: PathBuf = home.join("Library/Preferences/com.apple.finder.plist");
    let Ok(value) = plist::Value::from_file(path) else {
        return false;
    };
    match value
        .as_dictionary()
        .and_then(|d| d.get("AppleShowAllFiles"))
    {
        Some(plist::Value::Boolean(on)) => *on,
        Some(plist::Value::String(text)) => {
            matches!(text.to_lowercase().as_str(), "yes" | "true" | "1")
        }
        Some(plist::Value::Integer(n)) => n.as_signed() == Some(1),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording(
        confirm: fn() -> bool,
        awake: AwakeProbe,
        hidden: HiddenProbe,
    ) -> (Toggles, Arc<Mutex<Vec<Op>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&log);
        let toggles = Toggles {
            exec: Arc::new(move |op| {
                sink.lock().unwrap().push(op.clone());
                Ok(())
            }),
            confirm_required: confirm,
            awake,
            hidden_shown: hidden,
        };
        (toggles, log)
    }

    fn off() -> Option<Option<Duration>> {
        None
    }

    fn find(toggles: &Toggles, query: &str, key: &str) -> Option<Item> {
        let id = format!("toggles:{key}");
        toggles.query(query).into_iter().find(|item| item.id == id)
    }

    #[test]
    fn parses_durations_people_type() {
        // 「防休眠 2h」「caffeinate 90」都要能到时自动关；解析错了电脑会一直不睡或提前睡。
        assert_eq!(parse_duration("2h"), Some(7200));
        assert_eq!(parse_duration("90"), Some(5400));
        assert_eq!(parse_duration("1h30m"), Some(5400));
        assert_eq!(parse_duration("45s"), Some(45));
        assert_eq!(parse_duration("1.5h"), Some(5400));
        assert_eq!(parse_duration("2小时"), Some(7200));
        assert_eq!(parse_duration("30分钟"), Some(1800));
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("0"), None);
        assert_eq!(parse_duration("2x"), None);
    }

    #[test]
    fn keep_awake_starts_with_duration_and_reports_state() {
        // 没开时回车开启（带时长就到时自动关）；开着时同一条变成「关闭」，标签显示剩余时间。
        let (toggles, log) = recording(|| true, off, || false);
        let item = find(&toggles, "防休眠 2h", "caffeinate").unwrap();
        assert_eq!(item.title, "开启防休眠");
        assert!(item.subtitle.contains("2 小时"), "{}", item.subtitle);
        assert_eq!(item.action.run().unwrap(), Outcome::Done);
        assert_eq!(*log.lock().unwrap(), vec![Op::Caffeinate(Some(7200))]);

        let item = find(&toggles, "fxm", "caffeinate").unwrap();
        item.actions[0].action.run().unwrap();
        assert_eq!(
            log.lock().unwrap().last(),
            Some(&Op::Caffeinate(Some(3600)))
        );

        let (on, log) = recording(|| true, || Some(Some(Duration::from_secs(6120))), || false);
        let item = find(&on, "caffeinate", "caffeinate").unwrap();
        assert_eq!(item.title, "关闭防休眠");
        assert_eq!(
            item.accessories,
            vec![Accessory::Tag {
                text: "剩余 1:42".into(),
                tone: Tone::Running
            }]
        );
        item.action.run().unwrap();
        assert_eq!(*log.lock().unwrap(), vec![Op::Decaffeinate]);
    }

    #[test]
    fn caffeinate_exits_with_helmp() {
        // 不带 -w 的话 Helmp 退出后 caffeinate 留在后台，电脑就再也不睡了。
        let args = caffeinate_args(&Op::Caffeinate(Some(60))).unwrap();
        assert_eq!(args[..4], ["-d", "-i", "-t", "60"]);
        assert_eq!(
            args[4..],
            ["-w".to_string(), std::process::id().to_string()]
        );
        let forever = caffeinate_args(&Op::Caffeinate(None)).unwrap();
        assert!(!forever.contains(&"-t".to_string()));
    }

    #[test]
    fn volume_sets_level_and_unmutes() {
        // 「音量 30」要真的设成 30 并取消静音，否则静音状态下调音量没反应。
        let (toggles, log) = recording(|| true, off, || false);
        for query in ["音量 30", "vol 30%", "yl 30", "volume 30"] {
            let item = find(&toggles, query, "volume").unwrap_or_else(|| panic!("{query}"));
            item.action.run().unwrap();
            assert_eq!(log.lock().unwrap().last(), Some(&Op::Volume(30)), "{query}");
        }
        assert_eq!(
            Op::Volume(30).render(),
            "/usr/bin/osascript -e 'set volume output volume 30 without output muted'"
        );
        assert!(
            find(&toggles, "音量 300", "volume").is_none(),
            "超出 0–100 不给结果"
        );
        let hint = find(&toggles, "音量", "volume").unwrap();
        assert!(matches!(hint.action.run().unwrap(), Outcome::Message(_)));
        assert_eq!(log.lock().unwrap().len(), 4, "没有数字时不执行");

        find(&toggles, "静音", "mute")
            .unwrap()
            .action
            .run()
            .unwrap();
        assert_eq!(log.lock().unwrap().last(), Some(&Op::Mute(true)));
        find(&toggles, "unmute", "unmute")
            .unwrap()
            .action
            .run()
            .unwrap();
        assert_eq!(log.lock().unwrap().last(), Some(&Op::Mute(false)));
    }

    #[test]
    fn hidden_files_toggle_follows_current_state_and_confirms() {
        // 标题跟随当前设置（显示 ↔ 隐藏）；会重启访达，所以先确认，确认后才执行。
        let (toggles, log) = recording(|| true, off, || false);
        let item = find(&toggles, "隐藏文件", "hidden-files").unwrap();
        assert_eq!(item.title, "显示隐藏文件");
        let Outcome::Show(page) = item.action.run().unwrap() else {
            panic!("应先显示确认页");
        };
        assert!(log.lock().unwrap().is_empty());
        page.actions[0].action.run().unwrap();
        assert_eq!(*log.lock().unwrap(), vec![Op::ShowHiddenFiles(true)]);
        assert_eq!(
            Op::ShowHiddenFiles(true).render(),
            "/usr/bin/defaults write com.apple.finder AppleShowAllFiles -bool true && /usr/bin/killall Finder"
        );

        let (shown, log) = recording(|| true, off, || true);
        let item = find(&shown, "hidden files !", "hidden-files").unwrap();
        assert_eq!(item.title, "隐藏隐藏文件");
        item.action.run().unwrap();
        assert_eq!(*log.lock().unwrap(), vec![Op::ShowHiddenFiles(false)]);
    }

    #[test]
    fn eject_all_confirms_unless_turned_off() {
        let (toggles, log) = recording(|| true, off, || false);
        let item = find(&toggles, "弹出所有磁盘", "eject-all").unwrap();
        assert!(item.action.keeps_open());
        assert!(matches!(item.action.run().unwrap(), Outcome::Show(_)));
        assert!(log.lock().unwrap().is_empty());
        let (no_confirm, log) = recording(|| false, off, || false);
        find(&no_confirm, "eject", "eject-all")
            .unwrap()
            .action
            .run()
            .unwrap();
        assert_eq!(*log.lock().unwrap(), vec![Op::EjectAll]);
    }

    #[test]
    fn found_by_chinese_english_and_pinyin() {
        let (toggles, _) = recording(|| true, off, || false);
        for (query, key) in [
            ("防休眠", "caffeinate"),
            ("fangxiumian", "caffeinate"),
            ("keep awake", "caffeinate"),
            ("静音", "mute"),
            ("jy", "mute"),
            ("隐藏文件", "hidden-files"),
            ("ycwj", "hidden-files"),
            ("tcsyc", "eject-all"),
            ("eject", "eject-all"),
        ] {
            assert!(
                find(&toggles, query, key).is_some(),
                "{query:?} 应找到 {key}"
            );
        }
        assert!(toggles.query("").is_empty(), "首页不出现");
    }

    #[test]
    fn symbol_icons_exist_in_bundled_assets() {
        use gpui_kit::AssetSource as _;
        for name in [
            "coffee",
            "volume-1",
            "volume-2",
            "volume-x",
            "eye",
            "eye-off",
            "hard-drive",
        ] {
            let path = format!("icons/{name}.svg");
            assert!(
                gpui_kit::assets::AllAssets.load(&path).unwrap().is_some(),
                "{name}"
            );
        }
    }
}
