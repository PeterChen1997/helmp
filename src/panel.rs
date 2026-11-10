//! 面板窗口的平台适配层。
//!
//! - 显示 / 隐藏：面板是 GPUI 的 `WindowKind::PopUp`（NSPanel），但创建后去掉了 NonactivatingPanel
//!   （见 [`make_activating`]）：唤起时激活 Helmp，中文等输入法才能组字；收起时把前台还给唤起前的 App
//!   （[`restore_previous_app`]），粘贴类动作发 ⌘V 前会等焦点回去。回归检查：script/ime-check.sh。
//! - 毛玻璃：GPUI 的 Blurred 背景做不出圆角，这里在 GPUI 的视图下面垫一层
//!   `NSVisualEffectView`（材质 popover，跟随系统明暗），用圆角遮罩图裁成设计稿的 24 px 圆角，
//!   窗口本身透明、阴影跟随遮罩形状。GPUI 只画半透明的内容层。
//! - 定位：每次显示前按 `config.general.panel_screen` 选屏（鼠标所在屏或主屏），
//!   水平居中、顶边落在可见区域自上而下 1/4 处（类 Spotlight）。坐标换算是纯函数，见测试。

use std::sync::atomic::{AtomicBool, Ordering};

use block2::RcBlock;
use gpui_kit::*;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezierPath, NSColor, NSEvent, NSImage, NSScreen, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow,
};
use objc2_foundation::{NSEdgeInsets, NSPoint, NSRect, NSSize};

use crate::config::PanelScreen;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// 面板圆角，与设计稿一致。
pub const CORNER_RADIUS: f64 = 24.0;

/// 把面板窗口改成圆角毛玻璃。在窗口创建后调用一次。
pub fn install_glass(window: &Window) {
    let (Some(ns_window), Some(mtm)) = (ns_window(window), MainThreadMarker::new()) else {
        eprintln!("helmp: 无法取得 NSWindow，面板不使用毛玻璃");
        return;
    };
    let Some(content) = ns_window.contentView() else {
        eprintln!("helmp: 面板没有 contentView，不使用毛玻璃");
        return;
    };

    let fill =
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable;
    if liquid_glass_available() {
        // macOS 26 起用系统的液态玻璃（NSGlassEffectView）：折射、高光随背景变化，圆角由它自己裁。
        let glass = objc2_app_kit::NSGlassEffectView::initWithFrame(
            objc2_app_kit::NSGlassEffectView::alloc(mtm),
            content.frame(),
        );
        glass.setStyle(objc2_app_kit::NSGlassEffectViewStyle::Regular);
        glass.setCornerRadius(CORNER_RADIUS);
        glass.setAutoresizingMask(fill);
        ns_window.setContentView(Some(&glass));
        content.setFrame(glass.bounds());
        content.setAutoresizingMask(fill);
        glass.setContentView(Some(&content));
        crate::trace::log("面板使用液态玻璃（NSGlassEffectView）");
    } else {
        let effect =
            NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), content.frame());
        effect.setMaterial(NSVisualEffectMaterial::Popover);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setState(NSVisualEffectState::Active);
        effect.setMaskImage(Some(&rounded_mask(CORNER_RADIUS)));
        effect.setAutoresizingMask(fill);
        // GPUI 的视图挪到毛玻璃视图里面，仍然铺满窗口。
        ns_window.setContentView(Some(&effect));
        content.setFrame(effect.bounds());
        content.setAutoresizingMask(fill);
        effect.addSubview(&content);
    }
    ensure_text_input(&ns_window);

    ns_window.setOpaque(false);
    ns_window.setBackgroundColor(Some(&NSColor::clearColor()));
    ns_window.setHasShadow(true);
    ns_window.invalidateShadow();
}

/// 系统有没有液态玻璃（macOS 26+ 的 NSGlassEffectView）。环境变量 HELMP_NO_LIQUID_GLASS 可强制退回旧毛玻璃。
pub fn liquid_glass_available() -> bool {
    std::env::var_os("HELMP_NO_LIQUID_GLASS").is_none()
        && objc2::runtime::AnyClass::get(c"NSGlassEffectView").is_some()
}

/// 可拉伸的圆角遮罩：四角按 radius 保留，中间拉伸。
fn rounded_mask(radius: f64) -> Retained<NSImage> {
    let edge = radius * 2.0 + 1.0;
    let handler = RcBlock::new(move |rect: NSRect| -> Bool {
        let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius);
        NSColor::blackColor().set();
        path.fill();
        Bool::YES
    });
    let image =
        NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(edge, edge), false, &handler);
    image.setCapInsets(NSEdgeInsets {
        top: radius,
        left: radius,
        bottom: radius,
        right: radius,
    });
    image
}

/// 先用非激活方式让面板成为 key window；系统不允许时（后台 App 在没有用户输入事件时
/// 被较新的 macOS 拒绝成为 key）退回到激活整个 App，保证键盘一定能输入。
pub fn show(window: &mut Window, cx: &mut App) {
    let Some(ns_window) = ns_window(window) else {
        eprintln!("helmp: 无法取得 NSWindow，退回 GPUI 激活");
        cx.activate(true);
        window.activate_window();
        return;
    };
    crate::trace::log(format!("panel.show 前：is_key={}", ns_window.isKeyWindow()));
    if let Some(view) = ns_window.contentView() {
        // 隐藏期间 GPUI 不重绘，显示前标脏，避免闪出上一帧。
        view.setNeedsDisplay(true);
    }
    position(&ns_window);
    ensure_text_input(&ns_window);
    // HELMP_SELF_TEST_KEY_WINDOW：自检里需要面板真的成为 key window（测真实按键与输入法）时才设，
    // 会短暂抢焦点；平时自检仍放在屏幕外、不抢键盘。
    if std::env::var_os("HELMP_SELF_TEST").is_some()
        && std::env::var_os("HELMP_SELF_TEST_KEY_WINDOW").is_none()
    {
        // 图形自检不能抢用户的键盘：摆在屏幕外，也不成为 key window（按键由自检直接注入）。
        ns_window.setFrameOrigin(objc2_foundation::NSPoint::new(-10000.0, -10000.0));
        ns_window.orderFrontRegardless();
        crate::trace::log(format!("panel.show 自检：{}", input_context_state(&ns_window)));
        return;
    }
    remember_previous_app();
    // 先激活 Helmp 再让面板成为 key：输入法只给激活的 App 建立组字会话（见 make_activating）。
    cx.activate(true);
    ns_window.makeKeyAndOrderFront(None);
    crate::trace::log(format!(
        "panel.show 非激活方式后：is_key={} {}",
        ns_window.isKeyWindow(),
        input_context_state(&ns_window)
    ));
    if ns_window.isKeyWindow() {
        return;
    }
    if !FALLBACK_LOGGED.swap(true, Ordering::Relaxed) {
        eprintln!("helmp: 面板未能以非激活方式成为 key window，改为激活 App");
    }
    cx.activate(true);
    ns_window.makeKeyAndOrderFront(None);
    crate::trace::log(format!(
        "panel.show 激活 App 后：is_key={}",
        ns_window.isKeyWindow()
    ));
}

/// GPUI 的窗口内容视图是一层普通 NSView 容器，真正接收文字输入的 `GPUIView` 在它里面。
/// 第一响应者若停在容器上，按键仍会被窗口转发给 GPUI，但 AppKit 不给容器建输入上下文，
/// ABC 等键盘布局下字插不进去（豆包这类输入法另有办法送字，所以还能打）。
/// 每次显示前把第一响应者校正到 `GPUIView`。
pub fn ensure_text_input(ns_window: &NSWindow) {
    let Some(root) = ns_window.contentView() else {
        return;
    };
    let Some(target) = find_gpui_view(&root) else {
        eprintln!("helmp: 面板里找不到 GPUIView，文字输入可能不可用");
        return;
    };
    let already = ns_window
        .firstResponder()
        .is_some_and(|current| std::ptr::eq(Retained::as_ptr(&current).cast::<()>(), Retained::as_ptr(&target).cast::<()>()));
    if !already {
        let ok = ns_window.makeFirstResponder(Some(&target));
        crate::trace::log(format!("第一响应者改为 GPUIView：{ok}"));
    }
}

/// 深度优先找类名（或父类名）为 GPUIView 的视图（AccessKit 可能把它换成动态子类）。
fn find_gpui_view(view: &objc2_app_kit::NSView) -> Option<Retained<objc2_app_kit::NSView>> {
    let mut class = Some(view.class());
    while let Some(c) = class {
        if c.name().to_bytes() == b"GPUIView" {
            return Some(objc2::Message::retain(view));
        }
        class = c.superclass();
    }
    view.subviews().iter().find_map(|child| find_gpui_view(&child))
}

/// 诊断：第一响应者的输入上下文是否存在、App 是否激活（ABC 等键盘布局靠输入上下文插字）。
fn input_context_state(ns_window: &NSWindow) -> String {
    let responder = ns_window.firstResponder();
    let class = responder
        .as_ref()
        .map(|r| r.class().name().to_string_lossy().into_owned());
    let context = responder
        .and_then(|r| r.downcast::<objc2_app_kit::NSView>().ok())
        .and_then(|view| view.inputContext());
    let active = MainThreadMarker::new()
        .map(|mtm| objc2_app_kit::NSApplication::sharedApplication(mtm).isActive());
    format!(
        "first_responder={class:?} input_context={} app_active={active:?}",
        context.is_some()
    )
}

/// 把面板改成「会激活 App」的面板（去掉 NonactivatingPanel）。
///
/// macOS 只给激活的 App 建立输入法组字会话：非激活面板虽然能成为 key window、收得到按键，
/// 豆包、系统拼音等输入法却只会把字母原样上屏，打不出中文（实测：同一面板里的原生
/// NSTextField 也一样，去掉这个样式后组字恢复正常）。Alfred、Raycast 这类启动器也是唤起时
/// 激活自己、收起时把焦点还给原来的 App——见 [`remember_previous_app`] / [`restore_previous_app`]。
/// 在面板创建后调用一次；授权浮窗、「已在运行」卡片不需要打字，仍保持非激活。
pub fn make_activating(window: &Window) {
    let Some(ns_window) = ns_window(window) else {
        eprintln!("helmp: 无法取得 NSWindow，面板保持非激活（中文输入法可能无法组字）");
        return;
    };
    let mut mask = ns_window.styleMask();
    mask.remove(objc2_app_kit::NSWindowStyleMask::NonactivatingPanel);
    ns_window.setStyleMask(mask);
}

/// 唤起前的前台 App（不是 Helmp 自己时）的 pid：收起时把焦点还给它。
static PREVIOUS_APP: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

fn remember_previous_app() {
    let me = std::process::id();
    let pid = objc2_app_kit::NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier())
        .filter(|pid| u32::try_from(*pid).ok() != Some(me))
        .unwrap_or(0);
    PREVIOUS_APP.store(pid, Ordering::Relaxed);
}

/// 收起面板后把焦点还给唤起前的 App：还有别的 Helmp 窗口（设置、欢迎页）开着时只激活那个 App，
/// 否则隐藏 Helmp（系统会把前台还给上一个 App，和 Alfred 的做法一样）。
fn restore_previous_app(panel: &NSWindow) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    if !app.isActive() {
        return;
    }
    let pid = PREVIOUS_APP.swap(0, Ordering::Relaxed);
    if pid == 0 {
        // 从 Helmp 自己的窗口（如设置）里唤起的：留在 Helmp。
        return;
    }
    let other_windows = app.windows().iter().any(|window| {
        !std::ptr::eq(&*window, panel)
            && window.isVisible()
            && !window.styleMask().contains(objc2_app_kit::NSWindowStyleMask::NonactivatingPanel)
    });
    if other_windows {
        if let Some(previous) =
            objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        {
            #[allow(deprecated)]
            previous.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions::empty());
        }
    } else {
        app.hide(None);
    }
}

static FALLBACK_LOGGED: AtomicBool = AtomicBool::new(false);

pub fn hide(window: &mut Window) {
    match ns_window(window) {
        Some(ns_window) => {
            ns_window.orderOut(None);
            restore_previous_app(&ns_window);
        }
        // 拿不到原生窗口说明平台层变了，大声报出来而不是假装隐藏成功。
        None => eprintln!("helmp: 无法取得 NSWindow，面板未隐藏"),
    }
}

/// 按配置把面板挪到目标屏幕上。拿不到屏幕信息时保持原位并说明。
fn position(ns_window: &NSWindow) {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("helmp: 不在主线程，面板不重新定位");
        return;
    };
    let screens: Vec<(NSRect, NSRect)> = NSScreen::screens(mtm)
        .iter()
        .map(|screen| (screen.frame(), screen.visibleFrame()))
        .collect();
    let frames: Vec<NSRect> = screens.iter().map(|(frame, _)| *frame).collect();
    let prefer = crate::config::get().general.panel_screen;
    let Some(index) = pick_screen(&frames, NSEvent::mouseLocation(), prefer) else {
        eprintln!("helmp: 没有可用的屏幕，面板不重新定位");
        return;
    };
    let (_, visible) = screens[index];
    let origin = panel_origin(visible, ns_window.frame().size);
    crate::trace::log(format!(
        "panel.position screen={index} origin=({}, {})",
        origin.x, origin.y
    ));
    ns_window.setFrameOrigin(origin);
}

/// 选屏。`frames` 是 `NSScreen.screens` 的 frame（AppKit 全局坐标，原点在主屏左下，
/// y 向上；第一个是带菜单栏的主屏）。鼠标不在任何屏上（理论上不会）时退回主屏。
pub fn pick_screen(frames: &[NSRect], mouse: NSPoint, prefer: PanelScreen) -> Option<usize> {
    if frames.is_empty() {
        return None;
    }
    if prefer == PanelScreen::Primary {
        return Some(0);
    }
    // 包含右 / 上边界：鼠标顶到屏幕最上沿时 y 恰好等于 maxY，NSPointInRect 会判为不在屏内。
    let contains = |rect: &NSRect| {
        mouse.x >= rect.origin.x
            && mouse.x <= rect.origin.x + rect.size.width
            && mouse.y >= rect.origin.y
            && mouse.y <= rect.origin.y + rect.size.height
    };
    Some(frames.iter().position(contains).unwrap_or(0))
}

/// 面板左下角在 AppKit 坐标里的位置：在 `visible`（屏幕去掉菜单栏和 Dock 的区域）里水平居中，
/// 顶边位于自上而下 1/4 处；放不下时贴着可见区域，宁可超出底部也不盖住菜单栏。
pub fn panel_origin(visible: NSRect, size: NSSize) -> NSPoint {
    let x = visible.origin.x + (visible.size.width - size.width) / 2.0;
    let top_edge = visible.origin.y + visible.size.height;
    let top = top_edge - visible.size.height / 4.0;
    let mut y = top - size.height;
    if y < visible.origin.y {
        y = visible.origin.y;
    }
    if y + size.height > top_edge {
        y = top_edge - size.height;
    }
    NSPoint::new(x.round(), y.round())
}

/// 把面板此刻的画面存成 PNG（图形自检用它截 README 配图）。
///
/// 系统截图接口（CGWindowListCreateImage / ScreenCaptureKit）连自己的窗口都要屏幕录制权限，
/// 所以这里让 GPUI 把当前状态离屏画一遍再读回像素。只有 GPUI 画的内容层（预乘 alpha）：
/// 毛玻璃是系统合成的，不在图里，背景是半透明的，配图时需要自己垫底色。
#[cfg(feature = "readme-shots")]
pub fn snapshot(window: &mut Window, cx: &mut App, path: &std::path::Path) -> anyhow::Result<()> {
    use anyhow::Context as _;
    // 屏幕外的窗口不会自己出新帧，截之前先按当前状态画一帧。
    window.draw(cx).clear(cx);
    let image = window.render_to_image()?;
    image
        .save(path)
        .with_context(|| format!("写入 {}", path.display()))
}

#[cfg(not(feature = "readme-shots"))]
pub fn snapshot(_window: &mut Window, _cx: &mut App, _path: &std::path::Path) -> anyhow::Result<()> {
    anyhow::bail!("截图需要用 `--features readme-shots` 编译")
}

pub fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // SAFETY: AppKit 句柄里的 ns_view 是 GPUI 持有的、仍然存活的 NSView；
    // 我们只在主线程（GPUI 的窗口回调里）借用它。
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window()
}

#[cfg(test)]
mod tests {
    // gpui_kit::* 里也有一个 `test` 属性宏，经 `super::*` 引入后会遮住标准库的。
    use core::prelude::v1::test;

    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
    }

    /// 主屏 1512×982 在原点；外接屏 2560×1440 在右侧且底边更低；第三块竖屏在主屏上方。
    fn screens() -> Vec<NSRect> {
        vec![
            rect(0.0, 0.0, 1512.0, 982.0),
            rect(1512.0, -300.0, 2560.0, 1440.0),
            rect(200.0, 982.0, 1080.0, 1920.0),
        ]
    }

    #[test]
    fn follows_the_mouse_across_screens() {
        // 多屏用户在哪块屏上按快捷键，面板就该出现在哪块屏上；选错屏等于面板「没出来」。
        let frames = screens();
        let at = |x, y| pick_screen(&frames, NSPoint::new(x, y), PanelScreen::Mouse);
        assert_eq!(at(100.0, 100.0), Some(0));
        // 外接屏在主屏原点以下的部分（AppKit y 为负）。
        assert_eq!(at(2000.0, -200.0), Some(1));
        // 主屏上方的竖屏。
        assert_eq!(at(500.0, 2500.0), Some(2));
        // 鼠标顶到竖屏最上沿：y == maxY 仍算在这块屏上。
        assert_eq!(at(500.0, 982.0 + 1920.0), Some(2));
        // 不在任何屏上时退回主屏。
        assert_eq!(at(-5000.0, 0.0), Some(0));
    }

    #[test]
    fn primary_preference_ignores_the_mouse() {
        // 用户选了「主屏幕」，鼠标在副屏上也要回到主屏。
        let frames = screens();
        let mouse = NSPoint::new(2000.0, 0.0);
        assert_eq!(pick_screen(&frames, mouse, PanelScreen::Primary), Some(0));
        assert_eq!(pick_screen(&[], mouse, PanelScreen::Mouse), None);
    }

    #[test]
    fn centers_horizontally_with_top_at_a_quarter() {
        // 类 Spotlight：水平居中、顶边在可见区域上 1/4 处。
        // 主屏可见区域去掉了 37 的菜单栏：y 0..945。
        let visible = rect(0.0, 0.0, 1512.0, 945.0);
        let origin = panel_origin(visible, NSSize::new(880.0, 540.0));
        assert_eq!(origin.x, 316.0);
        // 顶边 = 945 - 945/4 = 708.75，左下角 y = 708.75 - 540 = 168.75 → 169。
        assert_eq!(origin.y, 169.0);
    }

    #[test]
    fn positions_on_a_secondary_screen_with_negative_origin() {
        // 副屏的坐标可以是负数；算错就会把面板放到两块屏之间。
        let visible = rect(1512.0, -300.0, 2560.0, 1415.0);
        let size = NSSize::new(880.0, 540.0);
        let origin = panel_origin(visible, size);
        assert_eq!(origin.x, 1512.0 + 840.0);
        let top = origin.y + size.height;
        let expected_top = -300.0 + 1415.0 - 1415.0 / 4.0;
        assert!((top - expected_top).abs() <= 0.5, "top={top}");
        assert!(origin.x >= visible.origin.x);
        assert!(origin.x + size.width <= visible.origin.x + visible.size.width);
    }

    #[test]
    fn small_screen_never_covers_the_menu_bar() {
        // 可见区域比面板还矮时，宁可超出底部，也不能盖住菜单栏（那样就点不到菜单栏图标了）。
        let visible = rect(0.0, 0.0, 1024.0, 500.0);
        let size = NSSize::new(880.0, 540.0);
        let origin = panel_origin(visible, size);
        assert_eq!(origin.y + size.height, 500.0);
        // 稍矮一点的屏幕：1/4 处放不下时贴底，但仍完整在可见区域内。
        let visible = rect(0.0, 0.0, 1024.0, 600.0);
        let origin = panel_origin(visible, size);
        assert_eq!(origin.y, 0.0);
    }
}
