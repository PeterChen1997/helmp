//! 面板窗口的平台适配层。
//!
//! - 显示 / 隐藏：面板是 GPUI 的 `WindowKind::PopUp`，即带 NonactivatingPanel 的 NSPanel：
//!   能成为 key window 接收键盘，但不会把整个 App 激活。所以只对这一个窗口
//!   makeKeyAndOrderFront / orderOut，唤起和收起都不打扰前台 App（见 _research/launcher/04 §1）。
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

    let effect = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), content.frame());
    effect.setMaterial(NSVisualEffectMaterial::Popover);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::Active);
    effect.setMaskImage(Some(&rounded_mask(CORNER_RADIUS)));
    let fill =
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable;
    effect.setAutoresizingMask(fill);

    // GPUI 的视图挪到毛玻璃视图里面，仍然铺满窗口、仍然是第一响应者。
    ns_window.setContentView(Some(&effect));
    content.setFrame(effect.bounds());
    content.setAutoresizingMask(fill);
    effect.addSubview(&content);
    ns_window.makeFirstResponder(Some(&content));

    ns_window.setOpaque(false);
    ns_window.setBackgroundColor(Some(&NSColor::clearColor()));
    ns_window.setHasShadow(true);
    ns_window.invalidateShadow();
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
    if std::env::var_os("HELMP_SELF_TEST").is_some() {
        // 图形自检不能抢用户的键盘：摆在屏幕外，也不成为 key window（按键由自检直接注入）。
        ns_window.setFrameOrigin(objc2_foundation::NSPoint::new(-10000.0, -10000.0));
        ns_window.orderFrontRegardless();
        return;
    }
    ns_window.makeKeyAndOrderFront(None);
    crate::trace::log(format!(
        "panel.show 非激活方式后：is_key={}",
        ns_window.isKeyWindow()
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

static FALLBACK_LOGGED: AtomicBool = AtomicBool::new(false);

pub fn hide(window: &mut Window) {
    match ns_window(window) {
        Some(ns_window) => ns_window.orderOut(None),
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
