//! 面板显示 / 隐藏的平台适配层。
//!
//! 面板是 GPUI 的 `WindowKind::PopUp`，即带 NonactivatingPanel 的 NSPanel：
//! 它能成为 key window 接收键盘，但不会把整个 App 激活。所以这里刻意不调
//! `NSApp.activate` / `NSApp.hide`，只对这一个窗口 makeKeyAndOrderFront / orderOut，
//! 唤起和收起都不打扰用户当前的前台 App（见 _research/launcher/04 §1）。

use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::*;
use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSWindow};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

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

fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // SAFETY: AppKit 句柄里的 ns_view 是 GPUI 持有的、仍然存活的 NSView；
    // 我们只在主线程（GPUI 的窗口回调里）借用它。
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window()
}
