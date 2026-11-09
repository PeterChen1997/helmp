//! 菜单栏图标与下拉菜单（设计稿 Main.dc.html）。
//!
//! - 图标：`menubar-template.png`（模板图，跟随明暗）；有会话在等你时，图标右上角叠一个橙色圆点。
//!   模板图本身不能带颜色，所以圆点是按钮上的一个子视图（非模板图），图标仍保持系统的明暗与高亮处理。
//! - 菜单：顶部 App 图标 +「N 个会话在等你」，「Agent 会话」列出等待中的会话，下面是常用入口、
//!   设置、检查更新、退出。右侧快捷键取自 `hotkey::active`，即真实生效的组合。
//! - 每 2 秒用 `engine.query("")` 刷新等待中的会话；菜单展开期间不重建，避免条目在鼠标下跳动。
//! - 遵守 `config.general.show_menubar_icon`，通过 `config::subscribe` 实时生效。
//!
//! NSMenu 的回调在 AppKit 主线程触发：回调只把命令投进 channel，由 GPUI 前台任务轮询执行，
//! 不在回调里碰 GPUI 实体。唯一例外是「检查更新…」的提示框，它不涉及 GPUI，直接在回调里弹。

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use gpui_kit::*;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAlert, NSApplication, NSAutoresizingMaskOptions, NSBezierPath, NSColor, NSEventModifierFlags,
    NSFont, NSImage, NSImageView, NSMenu, NSMenuDelegate, NSMenuItem, NSMenuItemBadge, NSStatusBar,
    NSStatusItem, NSTextField, NSVariableStatusItemLength, NSView, NSWorkspace,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

use crate::engine::waiting_section;
use crate::hotkey::{self, Command};
use crate::i18n::{self, tr, trf};
use crate::launcher::LauncherHandle;
use crate::provider::{Accessory, Icon, Item};

const POLL_INTERVAL: Duration = Duration::from_millis(50);
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);
/// 菜单里最多列出几条等待中的会话，其余从「全部会话…」进。
const MAX_WAITING_IN_MENU: usize = 8;
const MENU_WIDTH: f64 = 280.0;
/// 圆点直径（含白色描边），与设计稿的 7 px + 1.5 px 描边接近。
const DOT_SIZE: f64 = 9.0;

/// 截图识别文字：与快捷键共用同一条命令。
const OCR: Command = Command::RunItem {
    id: "screenshot:ocr-screen",
    query: "截图识别文字",
};

/// 菜单项被点击后要做的事。
#[derive(Clone)]
enum MenuCommand {
    /// 打开面板（已打开时不动）。
    Show,
    /// 打开面板并进入范围（菜单里总是打开，不像快捷键那样再按一次收起）。
    Scope(&'static str),
    Run(Command),
    /// 执行等待中会话的主操作（跳转到会话）。
    RunItem(Box<Item>),
    Settings,
    CheckUpdates,
    Quit,
}

/// 安装菜单栏图标。启动时调用一次。
pub fn install(cx: &mut App) {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("helmp: 菜单栏图标必须在主线程创建，未安装");
        return;
    };
    let (tx, rx) = mpsc::channel::<MenuCommand>();
    let (config_tx, config_rx) = mpsc::channel::<()>();
    crate::config::subscribe(move |_| {
        // 接收端在轮询任务里，进程结束前一直存在。
        let _ = config_tx.send(());
    });

    let mut bar = MenuBar::new(tx, mtm);
    bar.apply_visibility();
    let mut placement_logged = false;
    let mut lang_generation = i18n::generation();

    // 自检：`HELMP_MENU_SELF_TEST=<菜单项标题>` 在菜单首次建好后程序化点击这一项，
    // 走完整的 target/action → channel → GPUI 链路（无法模拟真实鼠标点击时用）。
    let mut self_test = std::env::var("HELMP_MENU_SELF_TEST").ok();

    cx.spawn(async move |cx: &mut AsyncApp| {
        let mut last_refresh: Option<Instant> = None;
        loop {
            while let Ok(command) = rx.try_recv() {
                cx.update(|cx| run(command, cx));
            }
            if config_rx.try_recv().is_ok() {
                while config_rx.try_recv().is_ok() {}
                bar.apply_visibility();
                bar.dirty = true;
            }
            if hotkey::generation() != bar.hotkey_generation {
                bar.dirty = true;
            }
            if i18n::generation() != lang_generation {
                // 切换了界面语言：菜单要重建，等待会话的状态文字也要立即按新语言重查。
                lang_generation = i18n::generation();
                bar.dirty = true;
                last_refresh = None;
            }
            if !placement_logged && last_refresh.is_some() {
                placement_logged = true;
                bar.log_placement();
            }
            let due = last_refresh.is_none_or(|at| at.elapsed() >= REFRESH_INTERVAL);
            if due && bar.visible {
                last_refresh = Some(Instant::now());
                if let Some(engine) = cx.update(|cx| hotkey::engine(cx)) {
                    // 查询与按分组名筛选在同一种语言里做：分组名随语言变。
                    let waiting = cx
                        .background_executor()
                        .spawn(async move { i18n::pinned(|| waiting(engine.query(""))) })
                        .await;
                    bar.set_waiting(waiting);
                }
            }
            if bar.dirty && !bar.target.ivars().open.get() {
                bar.rebuild();
                if let Some(title) = self_test.take() {
                    bar.perform(&title);
                }
            }
            cx.background_executor().timer(POLL_INTERVAL).await;
        }
    })
    .detach();
}

fn run(command: MenuCommand, cx: &mut App) {
    match command {
        MenuCommand::Show => LauncherHandle::update(cx, |launcher, window, cx| {
            if !launcher.is_visible() {
                launcher.show(window, cx);
            }
        }),
        MenuCommand::Scope(scope) => LauncherHandle::update(cx, |launcher, window, cx| {
            launcher.open_scope(scope, window, cx)
        }),
        MenuCommand::Run(command) => hotkey::execute(command, cx),
        MenuCommand::RunItem(item) => hotkey::run_action(*item, cx),
        MenuCommand::Settings => crate::settings::open(cx),
        // 在回调里已经处理（见 MenuTarget::clicked）。
        MenuCommand::CheckUpdates => {}
        MenuCommand::Quit => cx.quit(),
    }
}

// ======================= 纯逻辑 =======================

/// 首页结果里「等你处理」的那一组。
fn waiting(items: Vec<Item>) -> Vec<Item> {
    items
        .into_iter()
        .filter(|item| item.badge == waiting_section())
        .collect()
}

fn waiting_summary(count: usize) -> String {
    match count {
        0 => tr("没有会话在等你", "No sessions waiting").into(),
        1 => tr("1 个会话在等你", "1 session waiting").into(),
        n => trf!("{n} 个会话在等你", "{n} sessions waiting"),
    }
}

/// 会话右侧的状态文字（「等你确认」）：优先取标签，其次取文字附件。
fn status_text(item: &Item) -> Option<String> {
    let tag = item
        .accessories
        .iter()
        .find_map(|accessory| match accessory {
            Accessory::Tag { text, .. } => Some(text.clone()),
            _ => None,
        });
    tag.or_else(|| {
        item.accessories
            .iter()
            .find_map(|accessory| match accessory {
                Accessory::Text(text) => Some(text.clone()),
                _ => None,
            })
    })
}

/// 菜单内容的指纹：没变就不重建。
fn signature(waiting: &[Item]) -> Vec<(String, String, Option<String>)> {
    waiting
        .iter()
        .map(|item| (item.id.clone(), item.title.clone(), status_text(item)))
        .collect()
}

/// 资源文件的候选位置：打包后在 `Contents/Resources`，从 target/ 直接运行时在仓库的 `resources/`。
fn resource_candidates(exe: Option<&Path>, name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(macos_dir) = exe.and_then(Path::parent) {
        out.push(macos_dir.join("../Resources").join(name));
    }
    out.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join(name),
    );
    out
}

fn resource(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    resource_candidates(exe.as_deref(), name)
        .into_iter()
        .find(|path| path.is_file())
}

/// 圆点放在按钮右上角。AppKit 视图默认原点在左下，翻转过的视图原点在左上。
fn dot_frame(bounds: NSRect, flipped: bool) -> NSRect {
    let inset = 2.0;
    let x = bounds.origin.x + bounds.size.width - DOT_SIZE - inset;
    let y = if flipped {
        bounds.origin.y + inset
    } else {
        bounds.origin.y + bounds.size.height - DOT_SIZE - inset
    };
    NSRect::new(NSPoint::new(x, y), NSSize::new(DOT_SIZE, DOT_SIZE))
}

// ======================= AppKit =======================

struct TargetIvars {
    /// 当前菜单的命令表，菜单项的 tag 是下标。
    commands: RefCell<Vec<MenuCommand>>,
    tx: mpsc::Sender<MenuCommand>,
    /// 菜单是否展开（展开期间不重建）。
    open: Cell<bool>,
}

define_class!(
    // SAFETY: NSObject 没有子类化要求；MenuTarget 没有实现 Drop。
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetIvars]
    struct MenuTarget;

    impl MenuTarget {
        // SAFETY: 签名与 NSMenuItem 的 action 约定一致（一个 sender 参数，无返回值）。
        #[unsafe(method(menuItemClicked:))]
        fn clicked(&self, sender: &NSMenuItem) {
            let tag = sender.tag();
            let command = usize::try_from(tag)
                .ok()
                .and_then(|index| self.ivars().commands.borrow().get(index).cloned());
            match command {
                Some(MenuCommand::CheckUpdates) => show_update_notice(self.mtm()),
                Some(command) => {
                    if self.ivars().tx.send(command).is_err() {
                        eprintln!("helmp: 菜单命令无人接收（轮询任务已结束）");
                    }
                }
                None => eprintln!("helmp: 菜单项 tag {tag} 没有对应命令"),
            }
        }
    }

    // SAFETY: NSObjectProtocol 没有额外安全要求。
    unsafe impl NSObjectProtocol for MenuTarget {}

    // SAFETY: 两个方法的签名与 NSMenuDelegate 一致。
    unsafe impl NSMenuDelegate for MenuTarget {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _menu: &NSMenu) {
            self.ivars().open.set(true);
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _menu: &NSMenu) {
            self.ivars().open.set(false);
        }
    }
);

impl MenuTarget {
    fn new(tx: mpsc::Sender<MenuCommand>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars {
            commands: RefCell::new(Vec::new()),
            tx,
            open: Cell::new(false),
        });
        // SAFETY: NSObject 的 init 签名正确。
        unsafe { msg_send![super(this), init] }
    }
}

/// 暂无发布地址：只说明当前版本。
fn show_update_notice(mtm: MainThreadMarker) {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&format!(
        "Helmp {}",
        env!("CARGO_PKG_VERSION")
    )));
    let version = env!("CARGO_PKG_VERSION");
    alert.setInformativeText(&NSString::from_str(&trf!(
        "当前版本 {version}。Helmp 还没有发布渠道，暂时无法自动检查更新。",
        "You're running version {version}. Helmp doesn't have a release channel yet, so it can't check for updates automatically."
    )));
    // 菜单栏工具型 App 不激活的话，提示框会躲在别的窗口后面。
    NSApplication::sharedApplication(mtm).activate();
    alert.runModal();
}

struct MenuBar {
    mtm: MainThreadMarker,
    item: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
    target: Retained<MenuTarget>,
    dot: Option<Retained<NSImageView>>,
    waiting: Vec<Item>,
    waiting_signature: Vec<(String, String, Option<String>)>,
    hotkey_generation: u64,
    visible: bool,
    dirty: bool,
}

impl MenuBar {
    fn new(tx: mpsc::Sender<MenuCommand>, mtm: MainThreadMarker) -> Self {
        // 按图标宽度占位（约 24 pt），比方形（约 38 pt）省地方：带刘海的屏幕上菜单栏放不下时，
        // 系统会把最左边的状态项藏起来。
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        menu.setMinimumWidth(MENU_WIDTH);
        let target = MenuTarget::new(tx, mtm);
        menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        item.setMenu(Some(&menu));

        let mut dot = None;
        match item.button(mtm) {
            Some(button) => {
                button.setImage(Some(&menubar_image()));
                button.setToolTip(Some(&NSString::from_str("Helmp")));
                let view = NSImageView::imageViewWithImage(&dot_image(), mtm);
                view.setFrame(dot_frame(button.bounds(), button.isFlipped()));
                // 跟着按钮右上角走。
                let anchor = if button.isFlipped() {
                    NSAutoresizingMaskOptions::ViewMinXMargin
                        | NSAutoresizingMaskOptions::ViewMaxYMargin
                } else {
                    NSAutoresizingMaskOptions::ViewMinXMargin
                        | NSAutoresizingMaskOptions::ViewMinYMargin
                };
                view.setAutoresizingMask(anchor);
                view.setHidden(true);
                button.addSubview(&view);
                dot = Some(view);
            }
            None => eprintln!("helmp: 菜单栏图标没有按钮，无法显示图标"),
        }

        Self {
            mtm,
            item,
            menu,
            target,
            dot,
            waiting: Vec::new(),
            waiting_signature: Vec::new(),
            hotkey_generation: u64::MAX,
            visible: true,
            dirty: true,
        }
    }

    /// 诊断：状态项按钮所在窗口的位置与是否真的可见。带刘海的屏幕上菜单栏放不下时，
    /// 系统会把最左边的状态项藏到刘海后面，且不给任何提示。
    fn log_placement(&self) {
        let Some(button) = self.item.button(self.mtm) else {
            crate::trace::log("menubar placement：没有按钮");
            return;
        };
        match button.window() {
            Some(window) => {
                let frame = window.frame();
                let visible = window
                    .occlusionState()
                    .contains(objc2_app_kit::NSWindowOcclusionState::Visible);
                if frame.size.height < 1.0 {
                    eprintln!(
                        "helmp: 菜单栏放不下 Helmp 的图标（被刘海或其他状态项挤掉），可用 ⌥Space 或面板里的 ⌘, 进入设置"
                    );
                }
                crate::trace::log(format!(
                    "menubar placement：x={:.0} y={:.0} w={:.0} h={:.0} occlusion_visible={visible} item_visible={}",
                    frame.origin.x,
                    frame.origin.y,
                    frame.size.width,
                    frame.size.height,
                    self.item.isVisible()
                ));
            }
            None => crate::trace::log("menubar placement：按钮没有窗口（未被放进菜单栏）"),
        }
    }

    fn apply_visibility(&mut self) {
        let show = crate::config::get().general.show_menubar_icon;
        if show != self.visible || self.item.isVisible() != show {
            self.item.setVisible(show);
            self.visible = show;
        }
    }

    fn set_waiting(&mut self, waiting: Vec<Item>) {
        let signature = signature(&waiting);
        if signature != self.waiting_signature {
            self.waiting_signature = signature;
            self.dirty = true;
        }
        self.waiting = waiting;
        let count = self.waiting.len();
        if let Some(button) = self.item.button(self.mtm) {
            let tip = match count {
                0 => "Helmp".to_string(),
                _ => trf!("Helmp，{}", "Helmp — {}", waiting_summary(count)),
            };
            button.setToolTip(Some(&NSString::from_str(&tip)));
            if let Some(dot) = &self.dot {
                // 按钮尺寸在首次布局后才确定，每次刷新都校正一下位置。
                dot.setFrame(dot_frame(button.bounds(), button.isFlipped()));
                dot.setHidden(count == 0);
            }
        }
    }

    fn rebuild(&mut self) {
        let mtm = self.mtm;
        self.dirty = false;
        self.hotkey_generation = hotkey::generation();
        self.menu.removeAllItems();
        let mut commands: Vec<MenuCommand> = Vec::new();

        let header = NSMenuItem::new(mtm);
        header.setView(Some(&header_view(self.waiting.len(), mtm)));
        self.menu.addItem(&header);
        self.menu.addItem(&NSMenuItem::separatorItem(mtm));

        self.menu.addItem(&NSMenuItem::sectionHeaderWithTitle(
            &NSString::from_str(tr("Agent 会话", "Agent Sessions")),
            mtm,
        ));
        for item in self.waiting.iter().take(MAX_WAITING_IN_MENU) {
            let entry = self.add(
                &mut commands,
                &item.title,
                MenuCommand::RunItem(Box::new(item.clone())),
                None,
            );
            entry.setImage(item_image(&item.icon).as_deref());
            if let Some(status) = status_text(item) {
                let badge = NSMenuItemBadge::initWithString(
                    NSMenuItemBadge::alloc(),
                    &NSString::from_str(&status),
                );
                entry.setBadge(Some(&badge));
            }
            if !item.subtitle.is_empty() {
                entry.setToolTip(Some(&NSString::from_str(&item.subtitle)));
            }
        }
        if self.waiting.is_empty() {
            let empty = NSMenuItem::new(mtm);
            empty.setTitle(&NSString::from_str(tr(
                "暂时没有等你处理的会话",
                "No sessions waiting for you",
            )));
            empty.setEnabled(false);
            self.menu.addItem(&empty);
        }
        self.add(
            &mut commands,
            tr("全部会话…", "All Sessions…"),
            MenuCommand::Scope("agents"),
            hotkey::active(Command::Scope("agents")),
        );
        self.menu.addItem(&NSMenuItem::separatorItem(mtm));

        let entries: [(&str, &str, MenuCommand, Command); 4] = [
            (
                tr("打开 Helmp", "Open Helmp"),
                "magnifyingglass",
                MenuCommand::Show,
                Command::Toggle,
            ),
            (
                tr("剪贴板历史", "Clipboard History"),
                "list.clipboard",
                MenuCommand::Scope("clipboard"),
                Command::Scope("clipboard"),
            ),
            (
                tr("截图识别文字", "Recognize Text in Screenshot"),
                "text.viewfinder",
                MenuCommand::Run(OCR),
                OCR,
            ),
            (
                tr("翻译选中文字", "Translate Selection"),
                "translate",
                MenuCommand::Scope("translate"),
                Command::Scope("translate"),
            ),
        ];
        for (title, symbol, command, shortcut) in entries {
            let entry = self.add(&mut commands, title, command, hotkey::active(shortcut));
            entry.setImage(symbol_image(symbol, title).as_deref());
        }
        self.menu.addItem(&NSMenuItem::separatorItem(mtm));

        let settings = self.add(&mut commands, tr("设置…", "Settings…"), MenuCommand::Settings, None);
        settings.setKeyEquivalent(&NSString::from_str(","));
        settings.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
        self.add(
            &mut commands,
            tr("检查更新…", "Check for Updates…"),
            MenuCommand::CheckUpdates,
            None,
        );
        self.menu.addItem(&NSMenuItem::separatorItem(mtm));
        let quit = self.add(&mut commands, tr("退出 Helmp", "Quit Helmp"), MenuCommand::Quit, None);
        quit.setKeyEquivalent(&NSString::from_str("q"));
        quit.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);

        *self.target.ivars().commands.borrow_mut() = commands;
        self.trace();
    }

    /// `HELMP_TRACE` 时把菜单内容打出来：标题、右侧快捷键、状态徽标。
    fn trace(&self) {
        let lines: Vec<String> = self
            .menu
            .itemArray()
            .iter()
            .map(|entry| {
                if entry.isSeparatorItem() {
                    return "  ----".to_string();
                }
                if entry.view().is_some() {
                    return format!("  [header] {}", waiting_summary(self.waiting.len()));
                }
                let key = entry.keyEquivalent().to_string();
                let shortcut = if key.is_empty() {
                    String::new()
                } else {
                    format!(
                        "  key={key:?} mods={:#x}",
                        entry.keyEquivalentModifierMask().0
                    )
                };
                let enabled = if entry.isEnabled() { "" } else { " (disabled)" };
                format!("  {}{shortcut}{enabled}", entry.title())
            })
            .collect();
        crate::trace::log(format!(
            "menubar rebuilt visible={}:\n{}",
            self.item.isVisible(),
            lines.join("\n")
        ));
    }

    /// 程序化点击标题为 `title` 的菜单项（自检用）。
    fn perform(&self, title: &str) {
        let index = self
            .menu
            .itemArray()
            .iter()
            .position(|entry| entry.title().to_string() == title);
        match index {
            Some(index) => {
                crate::trace::log(format!("menubar self-test: 点击「{title}」"));
                self.menu.performActionForItemAtIndex(index as isize);
            }
            None => eprintln!("helmp: 菜单里没有「{title}」"),
        }
    }

    /// 加一个可点击的菜单项；`shortcut` 显示在右侧（用 keyEquivalent，系统负责画符号）。
    fn add(
        &self,
        commands: &mut Vec<MenuCommand>,
        title: &str,
        command: MenuCommand,
        shortcut: Option<global_hotkey::hotkey::HotKey>,
    ) -> Retained<NSMenuItem> {
        let tag = commands.len();
        commands.push(command);
        // SAFETY: menuItemClicked: 由 MenuTarget 实现，签名匹配；target 在 MenuBar 里一直活着。
        let entry = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm),
                &NSString::from_str(title),
                Some(sel!(menuItemClicked:)),
                &NSString::from_str(""),
            )
        };
        let target: &AnyObject = self.target.as_ref();
        // SAFETY: target 实现了 action 对应的方法，并由 MenuBar 持有，活得比菜单项久。
        unsafe { entry.setTarget(Some(target)) };
        entry.setTag(tag as isize);
        if let Some(hotkey) = shortcut
            && let Some(key) = hotkey::key_equivalent(&hotkey)
        {
            entry.setKeyEquivalent(&NSString::from_str(&key.to_string()));
            entry.setKeyEquivalentModifierMask(modifier_flags(&hotkey));
        }
        self.menu.addItem(&entry);
        entry
    }
}

fn modifier_flags(hotkey: &global_hotkey::hotkey::HotKey) -> NSEventModifierFlags {
    use global_hotkey::hotkey::Modifiers;
    let mut flags = NSEventModifierFlags::empty();
    for (modifier, flag) in [
        (Modifiers::META, NSEventModifierFlags::Command),
        (Modifiers::ALT, NSEventModifierFlags::Option),
        (Modifiers::CONTROL, NSEventModifierFlags::Control),
        (Modifiers::SHIFT, NSEventModifierFlags::Shift),
    ] {
        if hotkey.mods.contains(modifier) {
            flags |= flag;
        }
    }
    flags
}

/// 菜单顶部：App 图标 + Helmp + 等待数。
fn header_view(count: usize, mtm: MainThreadMarker) -> Retained<NSView> {
    let rect = |x, y, w, h| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
    let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, MENU_WIDTH, 50.0));
    if let Some(icon) = app_icon() {
        let image = NSImageView::imageViewWithImage(&icon, mtm);
        image.setFrame(rect(14.0, 9.0, 32.0, 32.0));
        view.addSubview(&image);
    }
    let title = NSTextField::labelWithString(&NSString::from_str("Helmp"), mtm);
    title.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
    title.setFrame(rect(54.0, 25.0, MENU_WIDTH - 64.0, 17.0));
    view.addSubview(&title);

    let summary = NSTextField::labelWithString(&NSString::from_str(&waiting_summary(count)), mtm);
    summary.setFont(Some(&NSFont::systemFontOfSize(12.0)));
    let color = if count > 0 {
        NSColor::systemOrangeColor()
    } else {
        NSColor::secondaryLabelColor()
    };
    summary.setTextColor(Some(&color));
    summary.setFrame(rect(54.0, 8.0, MENU_WIDTH - 64.0, 16.0));
    view.addSubview(&summary);
    view
}

fn load_image(path: &Path) -> Option<Retained<NSImage>> {
    NSImage::initWithContentsOfFile(
        NSImage::alloc(),
        &NSString::from_str(&path.to_string_lossy()),
    )
}

/// 菜单栏模板图（18 pt，带 @2x）。读不到时退回 SF Symbol 并说明。
fn menubar_image() -> Retained<NSImage> {
    let image = resource("menubar-template.png").and_then(|path| load_image(&path));
    match image {
        Some(image) => {
            if let Some(retina) = resource("menubar-template@2x.png").and_then(|p| load_image(&p)) {
                for rep in retina.representations().iter() {
                    image.addRepresentation(&rep);
                }
            }
            image.setSize(NSSize::new(18.0, 18.0));
            image.setTemplate(true);
            image
        }
        None => {
            eprintln!("helmp: 找不到 menubar-template.png，菜单栏改用系统符号");
            let fallback = symbol_image("sparkle.magnifyingglass", "Helmp").unwrap_or_default();
            fallback.setTemplate(true);
            fallback
        }
    }
}

fn app_icon() -> Option<Retained<NSImage>> {
    let image = resource("AppIcon.icns")
        .or_else(|| resource("AppIcon-64.png"))
        .and_then(|path| load_image(&path));
    if image.is_none() {
        eprintln!("helmp: 找不到 App 图标，菜单顶部不显示图标");
    }
    image
}

fn symbol_image(name: &str, description: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(description)),
    )
}

/// 会话图标（16 pt）。符号图标在菜单里没有彩色底块，不显示。
fn item_image(icon: &Icon) -> Option<Retained<NSImage>> {
    let image = match icon {
        Icon::Image(path) => load_image(path)?,
        Icon::File(path) => {
            NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(&path.to_string_lossy()))
        }
        Icon::Symbol { .. } | Icon::None => return None,
    };
    image.setSize(NSSize::new(16.0, 16.0));
    Some(image)
}

/// 橙色圆点，带一圈浅色描边，在深浅菜单栏上都看得清。
fn dot_image() -> Retained<NSImage> {
    let handler = RcBlock::new(|rect: NSRect| -> Bool {
        NSColor::colorWithWhite_alpha(1.0, 0.9).set();
        NSBezierPath::bezierPathWithOvalInRect(rect).fill();
        let inner = NSRect::new(
            NSPoint::new(rect.origin.x + 1.25, rect.origin.y + 1.25),
            NSSize::new(rect.size.width - 2.5, rect.size.height - 2.5),
        );
        NSColor::systemOrangeColor().set();
        NSBezierPath::bezierPathWithOvalInRect(inner).fill();
        Bool::YES
    });
    NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(DOT_SIZE, DOT_SIZE), false, &handler)
}

#[cfg(test)]
mod tests {
    // gpui_kit::* 里也有一个 `test` 属性宏，经 `super::*` 引入后会遮住标准库的。
    use core::prelude::v1::test;

    use super::*;
    use crate::provider::Tone;

    fn item(title: &str, badge: &str, accessories: Vec<Accessory>) -> Item {
        Item {
            id: format!("agents:{title}"),
            title: title.into(),
            badge: badge.into(),
            accessories,
            ..Item::blank()
        }
    }

    #[test]
    fn only_waiting_sessions_are_listed() {
        // 菜单的「Agent 会话」只列等你处理的；把最近使用、快捷命令混进来，圆点和计数就失去意义。
        let items = vec![
            item("Finder daily radar", waiting_section(), vec![]),
            item("微信", crate::engine::recent_section(), vec![]),
            item("Godot 框架调研", waiting_section(), vec![]),
            item("剪贴板历史", crate::engine::commands_section(), vec![]),
        ];
        let titles: Vec<_> = waiting(items).into_iter().map(|item| item.title).collect();
        assert_eq!(titles, ["Finder daily radar", "Godot 框架调研"]);
    }

    #[test]
    fn summary_matches_design_copy() {
        // 设计稿文案：「2 个会话在等你」；没有时不能显示「0 个会话在等你」这种噪音。
        assert_eq!(waiting_summary(2), "2 个会话在等你");
        assert_eq!(waiting_summary(0), "没有会话在等你");
    }

    #[test]
    fn english_menu_finds_waiting_sessions_and_counts_them() {
        // 英文界面下「等你处理」的分组名变成英文：菜单栏若还按中文名筛，圆点和列表就永远是空的。
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let items = vec![
                item("Fix login", waiting_section(), vec![]),
                item("WeChat", crate::engine::recent_section(), vec![]),
            ];
            let titles: Vec<_> = waiting(items).into_iter().map(|i| i.title).collect();
            assert_eq!(titles, ["Fix login"]);
            // 英文要分单复数，0 时不说「0 sessions」。
            assert_eq!(waiting_summary(0), "No sessions waiting");
            assert_eq!(waiting_summary(1), "1 session waiting");
            assert_eq!(waiting_summary(3), "3 sessions waiting");
        });
    }

    #[test]
    fn status_prefers_the_tag() {
        // 右侧显示「等你确认 / 等你回答」，来自插件给的状态标签。
        let tagged = item(
            "a",
            waiting_section(),
            vec![
                Accessory::Text("3 分钟前".into()),
                Accessory::Tag {
                    text: "等你确认".into(),
                    tone: Tone::Waiting,
                },
            ],
        );
        assert_eq!(status_text(&tagged).as_deref(), Some("等你确认"));
        let plain = item(
            "b",
            waiting_section(),
            vec![Accessory::Text("3 分钟前".into())],
        );
        assert_eq!(status_text(&plain).as_deref(), Some("3 分钟前"));
        assert_eq!(status_text(&item("c", waiting_section(), vec![])), None);
    }

    #[test]
    fn signature_changes_only_when_visible_content_changes() {
        // 每 2 秒刷新一次；内容没变时重建菜单是浪费，变了却不重建就会显示过期会话。
        let a = vec![item("a", waiting_section(), vec![])];
        let b = vec![item("a", waiting_section(), vec![])];
        assert_eq!(signature(&a), signature(&b));
        let c = vec![item(
            "a",
            waiting_section(),
            vec![Accessory::Tag {
                text: "等你回答".into(),
                tone: Tone::Waiting,
            }],
        )];
        assert_ne!(signature(&a), signature(&c));
    }

    #[test]
    fn resources_prefer_the_app_bundle() {
        // 打包后必须从 Contents/Resources 读；从 target/ 直接运行时退回仓库的 resources/。
        let exe = Path::new("/Applications/Helmp.app/Contents/MacOS/helmp");
        let candidates = resource_candidates(Some(exe), "menubar-template.png");
        assert_eq!(
            candidates.first().unwrap(),
            Path::new("/Applications/Helmp.app/Contents/MacOS/../Resources/menubar-template.png")
        );
        assert!(
            candidates
                .last()
                .unwrap()
                .ends_with("resources/menubar-template.png")
        );
        // 仓库里的资源确实存在，开发期运行能找到图标。
        assert!(candidates.last().unwrap().is_file());
    }

    #[test]
    fn dot_sits_in_the_top_right_corner() {
        // 圆点要在图标右上角；AppKit 默认坐标 y 向上，翻转视图 y 向下，两种都要对。
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(24.0, 22.0));
        let up = dot_frame(bounds, false);
        assert_eq!((up.origin.x, up.origin.y), (13.0, 11.0));
        let down = dot_frame(bounds, true);
        assert_eq!((down.origin.x, down.origin.y), (13.0, 2.0));
    }
}
