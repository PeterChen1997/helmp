//! 窗口管理：把唤起 Helmp 之前的前台窗口摆到左半屏、右半屏、四角、三分之一、最大化、居中，
//! 恢复原位，或移到下一个显示器。
//!
//! - 面板是非激活面板，执行时前台 App 仍是用户在用的那个：对它的焦点窗口操作（辅助功能 API）。
//! - 没有辅助功能权限时，命令仍然列出（副标题注明），回车显示说明页和「打开辅助功能设置」，不静默失败。
//! - 目标位置由纯函数 [`geometry::target_frame`] 计算（多屏、菜单栏、Dock 都在可见区域里扣掉了）。
//! - 演练模式（`HELMP_DRY_RUN=1`）只打印 `[dry-run] window <命令> x,y WxH`，不动窗口。
//! - 测试接缝：`HELMP_WINDOW_FIXTURE` 是一段 JSON，给出假的屏幕和窗口（AX 坐标），
//!   有它时不读真实窗口、也不会移动任何窗口：
//!   `{"screens":[{"frame":[0,0,1440,900],"visible":[0,25,1440,800]}],"window":[100,100,800,600],"trusted":true}`。
//! - 全局快捷键 v1 不做：现有 `hotkey::EXTENSION_SHORTCUTS` 是「一个扩展一条快捷键」，
//!   窗口管理有 18 条命令，硬塞进去会改动共享的快捷键表和设置页结构。

mod ax;
mod geometry;

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

use crate::i18n::{tr, trf};
use crate::matcher;
use crate::provider::{
    Accessory, Action, Block, Detail, Icon, Item, NamedAction, Outcome, Provider, Tint, Tone,
};
use geometry::{Layout, Rect, Screen};

const PROVIDER_ID: &str = "window";
const FIXTURE_ENV: &str = "HELMP_WINDOW_FIXTURE";
const ACCESSIBILITY_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
/// 只打「窗口」这类泛称时，所有命令都列出来，但分数压低，让具体命令名的命中排前。
const GENERIC_KEYWORDS: [&str; 4] = ["窗口", "窗口管理", "window", "window management"];
const GENERIC_CEILING: u32 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Layout(Layout),
    Restore,
    NextDisplay,
}

struct Command {
    key: &'static str,
    title: (&'static str, &'static str),
    keywords: &'static [&'static str],
    icon: &'static str,
    op: Op,
}

const fn cmd(
    key: &'static str,
    title: (&'static str, &'static str),
    keywords: &'static [&'static str],
    icon: &'static str,
    op: Op,
) -> Command {
    Command {
        key,
        title,
        keywords,
        icon,
        op,
    }
}

static COMMANDS: [Command; 18] = [
    cmd("left-half", ("左半屏", "Left Half"), &["左半屏", "左半边", "left half"], "panel-left", Op::Layout(Layout::LeftHalf)),
    cmd("right-half", ("右半屏", "Right Half"), &["右半屏", "右半边", "right half"], "panel-right", Op::Layout(Layout::RightHalf)),
    cmd("top-half", ("上半屏", "Top Half"), &["上半屏", "上半边", "top half"], "panel-top", Op::Layout(Layout::TopHalf)),
    cmd("bottom-half", ("下半屏", "Bottom Half"), &["下半屏", "下半边", "bottom half"], "panel-bottom", Op::Layout(Layout::BottomHalf)),
    cmd("top-left", ("左上四分之一", "Top Left Quarter"), &["左上四分之一", "左上角", "top left"], "grid-2x2", Op::Layout(Layout::TopLeft)),
    cmd("top-right", ("右上四分之一", "Top Right Quarter"), &["右上四分之一", "右上角", "top right"], "grid-2x2", Op::Layout(Layout::TopRight)),
    cmd("bottom-left", ("左下四分之一", "Bottom Left Quarter"), &["左下四分之一", "左下角", "bottom left"], "grid-2x2", Op::Layout(Layout::BottomLeft)),
    cmd("bottom-right", ("右下四分之一", "Bottom Right Quarter"), &["右下四分之一", "右下角", "bottom right"], "grid-2x2", Op::Layout(Layout::BottomRight)),
    cmd("first-third", ("左三分之一", "First Third"), &["左三分之一", "first third", "left third"], "columns-3", Op::Layout(Layout::FirstThird)),
    cmd("center-third", ("中间三分之一", "Center Third"), &["中间三分之一", "中三分之一", "center third"], "columns-3", Op::Layout(Layout::CenterThird)),
    cmd("last-third", ("右三分之一", "Last Third"), &["右三分之一", "last third", "right third"], "columns-3", Op::Layout(Layout::LastThird)),
    cmd("first-two-thirds", ("左三分之二", "First Two Thirds"), &["左三分之二", "first two thirds"], "columns-2", Op::Layout(Layout::FirstTwoThirds)),
    cmd("last-two-thirds", ("右三分之二", "Last Two Thirds"), &["右三分之二", "last two thirds"], "columns-2", Op::Layout(Layout::LastTwoThirds)),
    cmd("maximize", ("最大化", "Maximize"), &["最大化", "窗口最大化", "maximize", "maximise"], "maximize", Op::Layout(Layout::Maximize)),
    cmd("almost-maximize", ("几乎最大化", "Almost Maximize"), &["几乎最大化", "almost maximize"], "maximize-2", Op::Layout(Layout::AlmostMaximize)),
    cmd("center", ("居中", "Center"), &["居中", "窗口居中", "center", "centre"], "square", Op::Layout(Layout::Center)),
    cmd("restore", ("恢复窗口位置", "Restore Window"), &["恢复窗口位置", "恢复原位", "恢复", "restore"], "undo-2", Op::Restore),
    cmd("next-display", ("移到下一个显示器", "Move to Next Display"), &["移到下一个显示器", "下一个显示器", "下一块屏幕", "next display", "next screen"], "monitor", Op::NextDisplay),
];

/// 「恢复」用：pid → (第一次被摆放前的位置, Helmp 最后摆到的位置)。只在内存里，退出 Helmp 后清空。
static MEMORY: Mutex<Option<HashMap<i32, (Rect, Rect)>>> = Mutex::new(None);

pub struct Window;

impl Provider for Window {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let text = query.trim();
        if text.is_empty() {
            return Vec::new();
        }
        let generic = GENERIC_KEYWORDS
            .iter()
            .filter_map(|keyword| matcher::score(text, keyword))
            .max()
            .map(|score| score.min(GENERIC_CEILING));
        let trusted = trusted();
        COMMANDS
            .iter()
            .filter_map(|command| {
                let specific = command
                    .keywords
                    .iter()
                    .filter_map(|keyword| matcher::score(text, keyword))
                    .max();
                let score = specific.max(generic)?;
                Some(item(command, score, trusted))
            })
            .collect()
    }
}

fn badge() -> &'static str {
    tr("窗口", "Window")
}

fn item(command: &'static Command, score: u32, trusted: bool) -> Item {
    let (subtitle, accessories) = if trusted {
        (
            tr("调整唤起 Helmp 前的前台窗口", "Arranges the window you were using").into(),
            Vec::new(),
        )
    } else {
        (
            tr(
                "需要辅助功能权限，回车查看如何授权",
                "Needs Accessibility permission; press Return to see how",
            )
            .into(),
            vec![Accessory::Tag {
                text: tr("需授权", "Needs Access").into(),
                tone: Tone::Waiting,
            }],
        )
    };
    let action = if trusted {
        Action::new(move || run(command))
    } else {
        Action::new(|| Ok(Outcome::Show(permission_detail()))).keep_open()
    };
    Item {
        id: format!("{PROVIDER_ID}:{}", command.key),
        title: tr(command.title.0, command.title.1).into(),
        subtitle,
        badge: badge().into(),
        score,
        icon: Icon::Symbol {
            name: command.icon,
            tint: Tint::Blue,
        },
        accessories,
        action,
        action_title: tr("执行", "Apply").into(),
        ..Item::blank()
    }
}

/// 没有辅助功能权限时的说明页：讲清楚为什么要、去哪开，回车直接打开那一页设置。
fn permission_detail() -> Detail {
    Detail {
        icon: Icon::Symbol {
            name: "shield-alert",
            tint: Tint::Orange,
        },
        title: tr("需要辅助功能权限", "Accessibility Permission Needed").into(),
        tag: None,
        blocks: vec![
            Block::Text(
                tr(
                    "移动和调整其他 App 的窗口要用 macOS 的辅助功能接口。",
                    "Moving and resizing other apps’ windows uses macOS Accessibility.",
                )
                .into(),
            ),
            Block::Text(
                tr(
                    "在「系统设置 › 隐私与安全性 › 辅助功能」里打开 Helmp，然后再执行一次。",
                    "Turn on Helmp in System Settings › Privacy & Security › Accessibility, then try again.",
                )
                .into(),
            ),
        ],
        metadata: Vec::new(),
        actions: vec![
            NamedAction::new(
                tr("打开辅助功能设置", "Open Accessibility Settings"),
                None,
                Action::open(ACCESSIBILITY_URL),
            ),
            NamedAction::new(tr("取消", "Cancel"), Some("Esc"), Action::dismiss()),
        ],
    }
}

/// 测试接缝：假的屏幕与窗口（AX 坐标，`[x, y, w, h]`）。
#[derive(Debug, Deserialize)]
struct Fixture {
    screens: Vec<FixtureScreen>,
    window: [f64; 4],
    #[serde(default = "yes")]
    trusted: bool,
    /// 「恢复」要回到的位置（真实运行时记在内存里）。
    restore: Option<[f64; 4]>,
}

#[derive(Debug, Deserialize)]
struct FixtureScreen {
    frame: [f64; 4],
    visible: [f64; 4],
}

fn yes() -> bool {
    true
}

fn rect([x, y, w, h]: [f64; 4]) -> Rect {
    Rect::new(x, y, w, h)
}

fn fixture() -> Option<Result<Fixture>> {
    let text = std::env::var(FIXTURE_ENV).ok()?;
    Some(serde_json::from_str(&text).with_context(|| {
        trf!("{FIXTURE_ENV} 不是合法的 JSON", "{FIXTURE_ENV} isn’t valid JSON")
    }))
}

fn trusted() -> bool {
    match fixture() {
        Some(Ok(fixture)) => fixture.trusted,
        Some(Err(_)) => true,
        None => crate::permissions::accessibility_trusted(),
    }
}

/// 算出目标位置。`restore` 是「恢复」要回到的位置（没有时恢复命令报错）。
fn plan(op: Op, current: Rect, screens: &[Screen], restore: Option<Rect>) -> Result<Rect> {
    let index = geometry::screen_index(current, screens)
        .context(tr("找不到任何显示器", "Couldn’t find any display"))?;
    let screen = screens[index];
    Ok(match op {
        Op::Layout(layout) => geometry::target_frame(layout, screen.visible, current),
        Op::Restore => restore.context(tr(
            "这个窗口还没被 Helmp 调整过，没有可恢复的位置",
            "Helmp hasn’t moved this window yet, so there’s nothing to restore",
        ))?,
        Op::NextDisplay => {
            if screens.len() < 2 {
                bail!(tr("只连接了一个显示器", "Only one display is connected"));
            }
            let next = screens[(index + 1) % screens.len()];
            geometry::move_to_screen(current, screen.visible, next.visible)
        }
    })
}

fn report(command: &Command, target: Rect) -> Option<Outcome> {
    crate::effects::dry("window", &format!("{} {target}", command.key))
}

fn run(command: &'static Command) -> Result<Outcome> {
    if let Some(fixture) = fixture() {
        let fixture = fixture?;
        let screens: Vec<Screen> = fixture
            .screens
            .iter()
            .map(|s| Screen {
                frame: rect(s.frame),
                visible: rect(s.visible),
            })
            .collect();
        let target = plan(command.op, rect(fixture.window), &screens, fixture.restore.map(rect))?;
        // 夹具里没有真窗口：不在演练模式也只报告。
        return Ok(report(command, target).unwrap_or_else(|| {
            Outcome::Message(format!("[fixture] window {} {target}", command.key))
        }));
    }
    if !crate::permissions::accessibility_trusted() {
        return Ok(Outcome::Show(permission_detail()));
    }
    let window = ax::Window::frontmost()?;
    let current = window.frame()?;
    let remembered = MEMORY
        .lock()
        .expect("window memory poisoned")
        .as_ref()
        .and_then(|memory| memory.get(&window.pid).copied());
    let restore = remembered.map(|(before, _)| before);
    let target = plan(command.op, current, &ax::screens(), restore)?;
    if let Some(outcome) = report(command, target) {
        return Ok(outcome);
    }
    if !window.settable("AXPosition") {
        bail!(trf!(
            "「{}」的窗口不能移动（可能处于全屏）",
            "“{}”’s window can’t be moved (it may be in full screen)",
            window.app
        ));
    }
    let resize = target.w != current.w || target.h != current.h;
    if resize && !window.settable("AXSize") {
        bail!(trf!(
            "「{}」的窗口不支持调整大小",
            "“{}”’s window can’t be resized",
            window.app
        ));
    }
    window.set_frame(target, resize)?;
    let mut memory = MEMORY.lock().expect("window memory poisoned");
    let memory = memory.get_or_insert_with(HashMap::new);
    if command.op == Op::Restore {
        memory.remove(&window.pid);
    } else {
        let entry = geometry::remember(remembered, current, target);
        memory.insert(window.pid, entry);
    }
    Ok(Outcome::Done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(items: &[Item]) -> Vec<String> {
        items.iter().map(|item| item.id.clone()).collect()
    }

    #[test]
    fn chinese_pinyin_and_english_all_find_commands() {
        // 中文用户打首字母（zbp = 左半屏、zdh = 最大化），英文用户打英文，都要第一条就命中。
        let provider = Window;
        for (query, id) in [
            ("zbp", "window:left-half"),
            ("zuobanping", "window:left-half"),
            ("left half", "window:left-half"),
            ("zdh", "window:maximize"),
            ("jz", "window:center"),
            ("xygxsq", "window:next-display"),
            ("next display", "window:next-display"),
        ] {
            let mut items = provider.query(query);
            items.sort_by(|a, b| b.score.cmp(&a.score));
            assert_eq!(items.first().map(|i| i.id.as_str()), Some(id), "{query}: {:?}", titles(&items));
        }
        assert!(provider.query("").is_empty());
    }

    #[test]
    fn generic_word_lists_every_command_below_specific_hits() {
        // 只打「窗口」时把所有命令列出来供挑；但分数要低于具体命令名的命中，不抢应用结果。
        let items = Window.query("窗口");
        assert_eq!(items.len(), COMMANDS.len());
        let left = items.iter().find(|i| i.id == "window:left-half").unwrap();
        assert!(left.score <= GENERIC_CEILING);
        let max = items.iter().find(|i| i.id == "window:maximize").unwrap();
        assert!(max.score > left.score, "「窗口最大化」名字里就有「窗口」，应排在泛称命中之前");
    }

    #[test]
    fn restore_without_memory_and_single_display_explain_themselves() {
        // 没法执行时要说清原因，而不是什么都不发生。
        let screen = Screen {
            frame: Rect::new(0., 0., 1440., 900.),
            visible: Rect::new(0., 25., 1440., 800.),
        };
        let window = Rect::new(10., 40., 500., 400.);
        assert!(plan(Op::Restore, window, &[screen], None).is_err());
        assert!(plan(Op::NextDisplay, window, &[screen], None).is_err());
        assert!(plan(Op::Layout(Layout::Maximize), window, &[], None).is_err());
        assert_eq!(
            plan(Op::Restore, window, &[screen], Some(Rect::new(1., 2., 3., 4.))).unwrap(),
            Rect::new(1., 2., 3., 4.)
        );
    }

    #[test]
    fn missing_permission_shows_how_to_grant_it() {
        // 没授权时静默失败最糟：用户按了回车什么也没发生。说明页的主操作要直达辅助功能设置。
        let detail = permission_detail();
        assert_eq!(detail.actions.len(), 2);
        assert!(ACCESSIBILITY_URL.ends_with("Privacy_Accessibility"));
    }

    #[test]
    fn symbol_icons_exist_in_bundled_assets() {
        use gpui_kit::AssetSource as _;
        for name in COMMANDS.iter().map(|c| c.icon).chain(["shield-alert"]) {
            let path = format!("icons/{name}.svg");
            assert!(
                gpui_kit::assets::AllAssets.load(&path).unwrap().is_some(),
                "缺少图标 {path}"
            );
        }
    }
}
