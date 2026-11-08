//! 面板窗口的平台适配层。
//!
//! - 显示 / 隐藏：面板是 GPUI 的 `WindowKind::PopUp`，即带 NonactivatingPanel 的 NSPanel：
//!   能成为 key window 接收键盘，但不会把整个 App 激活。所以只对这一个窗口
//!   makeKeyAndOrderFront / orderOut，唤起和收起都不打扰前台 App（见 _research/launcher/04 §1）。
//! - 毛玻璃：GPUI 的 Blurred 背景做不出圆角，这里在 GPUI 的视图下面垫一层
//!   `NSVisualEffectView`（材质 popover，跟随系统明暗），用圆角遮罩图裁成设计稿的 24 px 圆角，
//!   窗口本身透明、阴影跟随遮罩形状。GPUI 只画半透明的内容层。

use std::sync::atomic::{AtomicBool, Ordering};

use block2::RcBlock;
use gpui_kit::*;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezierPath, NSColor, NSImage, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow,
};
use objc2_foundation::{NSEdgeInsets, NSRect, NSSize};
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
