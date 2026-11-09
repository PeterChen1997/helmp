//! 系统命令：锁屏、睡眠、清空废纸篓、显示桌面、隐藏其他应用、切换深色模式、重启、关机、注销、
//! 打开「系统设置」、退出所有应用。
//!
//! - 匹配看中英关键词，中文名自带拼音 / 首字母（`suoping`、`sp` → 锁屏），与界面语言无关。
//! - 危险命令（清空废纸篓、重启、关机、注销、退出所有应用）回车后先在面板里显示确认页
//!   （`Detail::confirm`），再按一次回车才执行，Esc 取消。查询末尾带 ` !` 视为已确认，直接执行
//!   （与脚本插件 `needsConfirmation` 的老习惯一致）。设置 › 扩展里可关掉确认。
//! - 执行层是可替换的函数（`Executor`），测试只记录会执行哪个 `Op`，不碰真实系统。
//!   `HELMP_DRY_RUN=1` 时只返回将要执行的命令（命令行自查用）。

use std::process::Command as Process;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};

use crate::config;
use crate::i18n::{tr, trf};
use crate::matcher;
use crate::provider::{Accessory, Action, Detail, Icon, Item, Outcome, Provider, Tint, Tone};

use super::scripts::args::strip_confirmation;

/// 一条命令真正要做的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// 直接运行程序（不经过 shell）。
    Run(&'static str, &'static [&'static str]),
    /// `osascript -e <脚本>`。
    AppleScript(&'static str),
    /// 隐藏除当前应用以外的所有应用。
    HideOtherApps,
    /// 让所有应用正常退出（等同 ⌘Q，未保存的文档由应用自己提示），Finder 与 Helmp 自己除外。
    QuitAllApps,
}

impl Op {
    /// 给人看的形式（dry-run 输出）。
    fn render(self) -> String {
        match self {
            Op::Run(program, args) => std::iter::once(program)
                .chain(args.iter().copied())
                .map(|part| {
                    if part.contains(' ') {
                        format!("'{part}'")
                    } else {
                        part.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
            Op::AppleScript(script) => format!("osascript -e '{script}'"),
            Op::HideOtherApps => "NSRunningApplication.hide (other apps)".into(),
            Op::QuitAllApps => {
                "NSRunningApplication.terminate (all apps except Finder, Helmp)".into()
            }
        }
    }
}

/// 界面文字存 (中文, 英文)，查询时按当前语言取；匹配只看 `keywords`。
struct Command {
    key: &'static str,
    title: (&'static str, &'static str),
    subtitle: (&'static str, &'static str),
    keywords: &'static [&'static str],
    icon: Icon,
    /// 不可撤销的命令执行前先显示确认页。
    confirm: Option<Confirm>,
    op: Op,
}

impl Command {
    fn dangerous(&self) -> bool {
        self.confirm.is_some()
    }
}

/// 确认页的文字：(中文, 英文)。
struct Confirm {
    question: (&'static str, &'static str),
    consequence: (&'static str, &'static str),
}

const fn symbol(name: &'static str, tint: Tint) -> Icon {
    Icon::Symbol { name, tint }
}

static COMMANDS: [Command; 11] = [
    Command {
        key: "lock",
        title: ("锁屏", "Lock Screen"),
        subtitle: ("锁定屏幕，回来时需要密码", "Lock the screen"),
        keywords: &["锁屏", "锁定屏幕", "lock screen", "lock"],
        icon: symbol("lock", Tint::Indigo),
        confirm: None,
        // ⌃⌘Q 是系统的锁屏快捷键；没有公开 API 可以直接锁屏。需要辅助功能权限（粘贴已经要了）。
        op: Op::AppleScript(
            r#"tell application "System Events" to keystroke "q" using {control down, command down}"#,
        ),
    },
    Command {
        key: "sleep",
        title: ("睡眠", "Sleep"),
        subtitle: ("让电脑进入睡眠", "Put the Mac to sleep"),
        keywords: &["睡眠", "休眠", "sleep"],
        icon: symbol("moon", Tint::Indigo),
        confirm: None,
        op: Op::Run("/usr/bin/pmset", &["sleepnow"]),
    },
    Command {
        key: "empty-trash",
        title: ("清空废纸篓", "Empty Trash"),
        subtitle: (
            "永久删除废纸篓里的所有项目",
            "Permanently erase everything in the Trash",
        ),
        keywords: &["清空废纸篓", "废纸篓", "垃圾桶", "empty trash", "trash"],
        icon: symbol("trash", Tint::Red),
        confirm: Some(Confirm {
            question: ("确定要清空废纸篓吗？", "Empty the Trash?"),
            consequence: (
                "废纸篓里的所有项目会被永久删除，无法恢复。",
                "Everything in the Trash will be permanently erased and can’t be recovered.",
            ),
        }),
        op: Op::AppleScript(r#"tell application "Finder" to empty trash"#),
    },
    Command {
        key: "show-desktop",
        title: ("显示桌面", "Show Desktop"),
        subtitle: (
            "把所有窗口移开，再执行一次恢复",
            "Move all windows aside; run again to restore",
        ),
        keywords: &["显示桌面", "桌面", "show desktop", "desktop"],
        icon: symbol("monitor", Tint::Blue),
        confirm: None,
        // 「调度中心」带参数 1 就是系统的「显示桌面」（同 F11）。
        op: Op::Run(
            "/System/Applications/Mission Control.app/Contents/MacOS/Mission Control",
            &["1"],
        ),
    },
    Command {
        key: "hide-others",
        title: ("隐藏其他应用", "Hide Other Apps"),
        subtitle: (
            "只留下当前应用的窗口",
            "Keep only the current app’s windows",
        ),
        keywords: &["隐藏其他应用", "隐藏其他", "hide others", "hide other apps"],
        icon: symbol("eye-off", Tint::Gray),
        confirm: None,
        op: Op::HideOtherApps,
    },
    Command {
        key: "dark-mode",
        title: ("切换深色模式", "Toggle Dark Mode"),
        subtitle: (
            "在浅色与深色外观之间切换",
            "Switch between Light and Dark appearance",
        ),
        keywords: &[
            "切换深色模式",
            "深色模式",
            "暗色模式",
            "夜间模式",
            "toggle dark mode",
            "dark mode",
            "appearance",
        ],
        icon: symbol("sun-moon", Tint::Graphite),
        confirm: None,
        op: Op::AppleScript(
            r#"tell application "System Events" to tell appearance preferences to set dark mode to not dark mode"#,
        ),
    },
    Command {
        key: "restart",
        title: ("重启", "Restart"),
        subtitle: ("重新启动电脑", "Restart the Mac"),
        keywords: &["重启", "重新启动", "restart", "reboot"],
        icon: symbol("rotate-ccw", Tint::Orange),
        confirm: Some(Confirm {
            question: ("确定要重启吗？", "Restart now?"),
            consequence: (
                "所有应用都会退出，未保存的文档由各应用询问是否保存。",
                "All apps will quit; apps with unsaved documents will ask you to save them.",
            ),
        }),
        op: Op::AppleScript(r#"tell application "System Events" to restart"#),
    },
    Command {
        key: "shutdown",
        title: ("关机", "Shut Down"),
        subtitle: ("关闭电脑", "Shut down the Mac"),
        keywords: &["关机", "关闭电脑", "shut down", "shutdown", "power off"],
        icon: symbol("power", Tint::Red),
        confirm: Some(Confirm {
            question: ("确定要关机吗？", "Shut down now?"),
            consequence: (
                "所有应用都会退出并关闭电脑，未保存的文档由各应用询问是否保存。",
                "All apps will quit and the Mac will turn off; apps with unsaved documents will ask you to save them.",
            ),
        }),
        op: Op::AppleScript(r#"tell application "System Events" to shut down"#),
    },
    Command {
        key: "logout",
        title: ("注销", "Log Out"),
        subtitle: ("退出当前用户的登录", "Log out of this user account"),
        keywords: &["注销", "退出登录", "登出", "log out", "logout", "sign out"],
        icon: symbol("log-out", Tint::Orange),
        confirm: Some(Confirm {
            question: ("确定要注销吗？", "Log out now?"),
            consequence: (
                "所有应用都会退出并回到登录界面，未保存的文档由各应用询问是否保存。",
                "All apps will quit and you’ll return to the login window; apps with unsaved documents will ask you to save them.",
            ),
        }),
        op: Op::AppleScript(r#"tell application "System Events" to log out"#),
    },
    Command {
        key: "system-settings",
        title: ("系统设置", "System Settings"),
        subtitle: ("打开 macOS「系统设置」", "Open macOS System Settings"),
        keywords: &[
            "系统设置",
            "系统偏好设置",
            "system settings",
            "system preferences",
        ],
        icon: symbol("settings", Tint::Gray),
        confirm: None,
        op: Op::Run("/usr/bin/open", &["-b", "com.apple.systempreferences"]),
    },
    Command {
        key: "quit-all",
        title: ("退出所有应用", "Quit All Apps"),
        subtitle: (
            "Finder 与 Helmp 除外，未保存的文档由各应用提示",
            "Except Finder and Helmp; apps ask about unsaved documents",
        ),
        keywords: &[
            "退出所有应用",
            "退出全部应用",
            "关闭所有应用",
            "quit all apps",
            "quit all applications",
            "close all apps",
        ],
        icon: symbol("octagon-x", Tint::Red),
        confirm: Some(Confirm {
            question: ("确定要退出所有应用吗？", "Quit all apps?"),
            consequence: (
                "除 Finder 与 Helmp 外的应用都会退出，未保存的文档由各应用询问是否保存。",
                "Every app except Finder and Helmp will quit; apps with unsaved documents will ask you to save them.",
            ),
        }),
        op: Op::QuitAllApps,
    },
];

/// 执行层：真实实现见 `execute`，测试换成只记录的函数。
pub type Executor = Arc<dyn Fn(Op) -> Result<()> + Send + Sync>;

pub struct System {
    exec: Executor,
    /// 危险命令是否要确认（设置 › 扩展）。查询时读取。
    confirm_required: fn() -> bool,
}

impl System {
    pub fn new() -> Self {
        Self {
            exec: Arc::new(execute),
            confirm_required: || config::get().system_commands.confirm_dangerous,
        }
    }
}

fn badge() -> &'static str {
    tr("系统命令", "System")
}

impl Provider for System {
    fn id(&self) -> &'static str {
        "system"
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let (text, confirmed) = match strip_confirmation(query) {
            Some(stripped) => (stripped, true),
            None => (query, false),
        };
        let confirm_required = (self.confirm_required)();
        COMMANDS
            .iter()
            .filter_map(|command| {
                let score = command
                    .keywords
                    .iter()
                    .filter_map(|keyword| matcher::score(text, keyword))
                    .max()?;
                let guard = if command.dangerous() && confirm_required {
                    Some(confirmed)
                } else {
                    None
                };
                Some(self.item(command, score, guard))
            })
            .collect()
    }
}

impl System {
    /// `guard`：None = 不需要确认；Some(false) = 需要确认但还没确认；Some(true) = 已确认。
    fn item(&self, command: &Command, score: u32, guard: Option<bool>) -> Item {
        let title = tr(command.title.0, command.title.1);
        let description = tr(command.subtitle.0, command.subtitle.1);
        let execute = run(Arc::clone(&self.exec), command.op);
        let (subtitle, accessories, action) = match (guard, &command.confirm) {
            (Some(false), Some(confirm)) => (
                trf!(
                    "{description} · 回车后需确认",
                    "{description} · Asks for confirmation"
                ),
                vec![Accessory::Tag {
                    text: tr("需确认", "Confirm").into(),
                    tone: Tone::Waiting,
                }],
                // 第一次回车只显示确认页（不收起面板）；确认页的主操作才真正执行。
                Action::new({
                    let detail = Detail::confirm(
                        command.icon.clone(),
                        tr(confirm.question.0, confirm.question.1).into(),
                        tr(confirm.consequence.0, confirm.consequence.1).into(),
                        title.into(),
                        execute,
                    );
                    move || Ok(Outcome::Show(detail.clone()))
                })
                .keep_open(),
            ),
            (Some(true), _) => (
                trf!(
                    "{description} · 已确认，回车立即执行",
                    "{description} · Confirmed; press Return to run now"
                ),
                vec![Accessory::Tag {
                    text: tr("已确认", "Confirmed").into(),
                    tone: Tone::Running,
                }],
                execute,
            ),
            _ => (description.to_string(), Vec::new(), execute),
        };
        Item {
            id: format!("system:{}", command.key),
            title: title.into(),
            subtitle,
            badge: badge().into(),
            score,
            icon: command.icon.clone(),
            accessories,
            action,
            action_title: tr("执行", "Run").into(),
            ..Item::blank()
        }
    }
}

fn run(exec: Executor, op: Op) -> Action {
    Action::new(move || {
        if std::env::var("HELMP_DRY_RUN").as_deref() == Ok("1") {
            return Ok(Outcome::Message(trf!(
                "将执行：{}",
                "Would run: {}",
                op.render()
            )));
        }
        exec(op)?;
        Ok(Outcome::Done)
    })
}

/// 真实执行。
fn execute(op: Op) -> Result<()> {
    match op {
        Op::Run(program, args) => run_process(program, args),
        Op::AppleScript(script) => run_process("/usr/bin/osascript", &["-e", script]),
        Op::HideOtherApps => hide_other_apps(),
        Op::QuitAllApps => quit_all_apps(),
    }
}

fn run_process(program: &str, args: &[&str]) -> Result<()> {
    // 子进程一律用 UTF-8 locale：Helmp 可能从 LANG 为空的环境启动。
    let output = Process::new(program)
        .args(args)
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .output()
        .with_context(|| trf!("启动 {program} 失败", "Couldn’t start {program}"))?;
    if !output.status.success() {
        bail!(trf!(
            "{program} 失败（{}）：{}",
            "{program} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// 正在运行的应用的快照，用于挑出要处理的那些（纯逻辑，便于测试）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct RunningApp {
    pid: i32,
    bundle_id: Option<String>,
    /// 有 Dock 图标的普通应用；菜单栏工具与后台进程不算。
    regular: bool,
}

const FINDER: &str = "com.apple.finder";

/// 「退出所有应用」要退出的进程：普通应用，Finder 与 Helmp 自己除外。
fn quit_targets(apps: &[RunningApp], own_pid: i32) -> Vec<i32> {
    apps.iter()
        .filter(|app| app.regular && app.pid != own_pid && app.bundle_id.as_deref() != Some(FINDER))
        .map(|app| app.pid)
        .collect()
}

/// 「隐藏其他应用」要隐藏的进程：除当前应用与 Helmp 以外的普通应用。
fn hide_targets(apps: &[RunningApp], own_pid: i32, keep_pid: i32) -> Vec<i32> {
    apps.iter()
        .filter(|app| app.regular && app.pid != own_pid && app.pid != keep_pid)
        .map(|app| app.pid)
        .collect()
}

fn own_pid() -> i32 {
    i32::try_from(std::process::id()).unwrap_or(i32::MAX)
}

fn snapshot(app: &NSRunningApplication) -> RunningApp {
    RunningApp {
        pid: app.processIdentifier(),
        bundle_id: app.bundleIdentifier().map(|id| id.to_string()),
        regular: app.activationPolicy() == NSApplicationActivationPolicy::Regular,
    }
}

/// 对挑中的进程逐个执行 `act`，返回失败的应用名。
fn apply(
    pick: impl FnOnce(&[RunningApp]) -> Vec<i32>,
    act: impl Fn(&NSRunningApplication) -> bool,
) -> Vec<String> {
    objc2::rc::autoreleasepool(|_| {
        let apps = NSWorkspace::sharedWorkspace().runningApplications();
        let snapshots: Vec<RunningApp> = apps.iter().map(|app| snapshot(&app)).collect();
        let targets = pick(&snapshots);
        apps.iter()
            .filter(|app| targets.contains(&app.processIdentifier()))
            .filter(|app| !act(app))
            .map(|app| {
                app.localizedName()
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| app.processIdentifier().to_string())
            })
            .collect()
    })
}

fn quit_all_apps() -> Result<()> {
    let own = own_pid();
    let failed = apply(|apps| quit_targets(apps, own), |app| app.terminate());
    if !failed.is_empty() {
        bail!(trf!(
            "这些应用没有响应退出请求：{}",
            "These apps didn’t accept the quit request: {}",
            failed.join("、")
        ));
    }
    Ok(())
}

fn hide_other_apps() -> Result<()> {
    let own = own_pid();
    // 面板刚隐藏，前台可能还是 Helmp：稍等系统把焦点还给原来的应用。
    let deadline = Instant::now() + Duration::from_millis(500);
    let keep = loop {
        let front = objc2::rc::autoreleasepool(|_| {
            NSWorkspace::sharedWorkspace()
                .frontmostApplication()
                .map(|app| app.processIdentifier())
        });
        match front {
            Some(pid) if pid != own => break pid,
            _ if Instant::now() >= deadline => bail!(tr(
                "找不到当前在用的应用，没有隐藏任何应用",
                "Couldn’t tell which app is in use, so nothing was hidden"
            )),
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    let failed = apply(|apps| hide_targets(apps, own, keep), |app| app.hide());
    if !failed.is_empty() {
        bail!(trf!(
            "这些应用没能隐藏：{}",
            "Couldn’t hide these apps: {}",
            failed.join("、")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// 只记录、不执行的系统命令层。
    fn recording(confirm: fn() -> bool) -> (System, Arc<Mutex<Vec<Op>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&log);
        let system = System {
            exec: Arc::new(move |op| {
                sink.lock().unwrap().push(op);
                Ok(())
            }),
            confirm_required: confirm,
        };
        (system, log)
    }

    fn find(system: &System, query: &str, key: &str) -> Option<Item> {
        let id = format!("system:{key}");
        system.query(query).into_iter().find(|item| item.id == id)
    }

    fn command(key: &str) -> &'static Command {
        COMMANDS.iter().find(|c| c.key == key).unwrap()
    }

    #[test]
    fn irreversible_commands_are_the_dangerous_ones() {
        // 这五个一旦执行就收不回来（文件删了、未保存的工作丢了），必须走确认；其余的随手可撤销，不该多一步。
        let mut dangerous: Vec<_> = COMMANDS
            .iter()
            .filter(|c| c.dangerous())
            .map(|c| c.key)
            .collect();
        dangerous.sort_unstable();
        assert_eq!(
            dangerous,
            ["empty-trash", "logout", "quit-all", "restart", "shutdown"]
        );
    }

    #[test]
    fn dangerous_commands_never_run_without_confirmation() {
        // 误按回车不能关机、清空废纸篓：第一次回车只出确认页，执行层一次都不能被调用；
        // 确认页上回车（主操作）才执行，「取消」只回到列表。
        let (system, log) = recording(|| true);
        for command in COMMANDS.iter().filter(|c| c.dangerous()) {
            let query = command.keywords[0];
            let item = find(&system, query, command.key).unwrap_or_else(|| panic!("{query}"));
            assert!(
                item.action.keeps_open(),
                "确认页应直接出现在面板里，不先收起"
            );
            assert_eq!(
                item.accessories,
                vec![Accessory::Tag {
                    text: "需确认".into(),
                    tone: Tone::Waiting
                }]
            );
            let Outcome::Show(page) = item.action.run().unwrap() else {
                panic!("{query} 第一次回车应显示确认页");
            };
            assert!(log.lock().unwrap().is_empty(), "未确认就执行了：{log:?}");
            let confirm = command.confirm.as_ref().unwrap();
            assert_eq!(page.title, confirm.question.0);
            let titles: Vec<_> = page.actions.iter().map(|a| a.title.as_str()).collect();
            assert_eq!(
                titles,
                [command.title.0, "取消"],
                "主操作（回车）是执行，其次是取消"
            );

            assert_eq!(page.actions[1].action.run().unwrap(), Outcome::Dismiss);
            assert!(
                page.actions[1].action.keeps_open(),
                "取消只回到列表，不收起面板"
            );
            assert!(log.lock().unwrap().is_empty(), "取消后不应执行");

            assert_eq!(page.actions[0].action.run().unwrap(), Outcome::Done);
            assert_eq!(*log.lock().unwrap(), vec![command.op]);
            log.lock().unwrap().clear();
        }
    }

    #[test]
    fn trailing_bang_still_skips_the_confirmation_page() {
        // 老用户习惯在末尾加 ` !` 直接执行（脚本插件的 needsConfirmation 也是这样），不能因为加了确认页就失效。
        let (system, log) = recording(|| true);
        for command in COMMANDS.iter().filter(|c| c.dangerous()) {
            let query = format!("{} !", command.keywords[0]);
            let item = find(&system, &query, command.key).unwrap();
            assert_eq!(item.action.run().unwrap(), Outcome::Done);
            assert_eq!(log.lock().unwrap().last(), Some(&command.op));
        }
    }

    #[test]
    fn confirmation_can_be_turned_off() {
        // 设置里关掉确认后，危险命令和普通命令一样回车即执行。
        let (system, log) = recording(|| false);
        let item = find(&system, "关机", "shutdown").unwrap();
        assert!(item.accessories.is_empty());
        assert_eq!(item.action.run().unwrap(), Outcome::Done);
        assert_eq!(*log.lock().unwrap(), vec![command("shutdown").op]);
    }

    #[test]
    fn safe_commands_run_immediately() {
        // 锁屏、睡眠这类随时可恢复的命令不该多一步确认。
        let (system, log) = recording(|| true);
        for key in [
            "lock",
            "sleep",
            "show-desktop",
            "hide-others",
            "dark-mode",
            "system-settings",
        ] {
            let command = command(key);
            let item = find(&system, command.keywords[0], key).unwrap();
            assert!(!item.action.keeps_open());
            assert_eq!(item.action.run().unwrap(), Outcome::Done);
            assert_eq!(log.lock().unwrap().last(), Some(&command.op));
        }
    }

    #[test]
    fn runs_the_expected_system_command() {
        // 只验证会调用哪个命令；真正执行要碰系统，测试里绝不跑。
        assert_eq!(
            command("sleep").op,
            Op::Run("/usr/bin/pmset", &["sleepnow"])
        );
        assert_eq!(
            command("empty-trash").op,
            Op::AppleScript(r#"tell application "Finder" to empty trash"#)
        );
        assert_eq!(
            command("system-settings").op,
            Op::Run("/usr/bin/open", &["-b", "com.apple.systempreferences"])
        );
        assert_eq!(command("quit-all").op, Op::QuitAllApps);
        assert_eq!(command("sleep").op.render(), "/usr/bin/pmset sleepnow");
    }

    #[test]
    fn found_by_chinese_english_and_pinyin() {
        // 中文用户打拼音或首字母，英文用户打英文，都要找得到；与界面语言无关。
        let (system, _) = recording(|| true);
        for (query, key) in [
            ("锁屏", "lock"),
            ("lock", "lock"),
            ("suoping", "lock"),
            ("sp", "lock"),
            ("关机", "shutdown"),
            ("shut down", "shutdown"),
            ("gj", "shutdown"),
            ("清空废纸篓", "empty-trash"),
            ("qkfzl", "empty-trash"),
            ("empty trash", "empty-trash"),
            ("深色", "dark-mode"),
            ("dark", "dark-mode"),
            ("xszm", "show-desktop"),
            ("注销", "logout"),
            ("log out", "logout"),
            ("系统设置", "system-settings"),
            ("tcsyyy", "quit-all"),
        ] {
            assert!(
                find(&system, query, key).is_some(),
                "{query:?} 应找到 {key}"
            );
        }
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let item = find(&system, "锁屏", "lock").unwrap();
            assert_eq!(item.title, "Lock Screen");
            assert_eq!(item.badge, "System");
        });
    }

    #[test]
    fn never_on_the_home_page() {
        // 空查询（首页）不能出现关机、注销：误按回车的代价太大。
        let (system, _) = recording(|| false);
        assert!(system.query("").is_empty());
        assert!(system.query("  ").is_empty());
    }

    #[test]
    fn quit_all_spares_finder_and_helmp() {
        // 退出 Finder 会让桌面消失；退出 Helmp 自己会让命令半途而废。菜单栏工具也不该被退出。
        let app = |pid, bundle: &str, regular| RunningApp {
            pid,
            bundle_id: Some(bundle.into()),
            regular,
        };
        let apps = [
            app(10, "com.apple.finder", true),
            app(11, "com.tencent.xinWeChat", true),
            app(12, "dev.helmp.launcher", true),
            app(13, "com.apple.Safari", true),
            app(14, "com.example.menubar", false),
        ];
        assert_eq!(quit_targets(&apps, 12), [11, 13]);
        // 隐藏其他应用：当前应用（Safari）留下，Finder 也一并隐藏。
        assert_eq!(hide_targets(&apps, 12, 13), [10, 11]);
    }

    #[test]
    fn symbol_icons_exist_in_bundled_assets() {
        // 图标名拼错时界面只会画一个空白方块，不会报错。
        use gpui_kit::AssetSource as _;
        for command in &COMMANDS {
            let Icon::Symbol { name, .. } = command.icon else {
                panic!("{} 应使用内置符号", command.key);
            };
            let path = format!("icons/{name}.svg");
            assert!(
                gpui_kit::assets::AllAssets.load(&path).unwrap().is_some(),
                "{name}"
            );
        }
    }
}
