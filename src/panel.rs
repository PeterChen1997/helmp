//! 面板显示 / 隐藏的平台适配层。
//!
//! 面板是 GPUI 的 `WindowKind::PopUp`，即带 NonactivatingPanel 的 NSPanel：
//! 它能成为 key window 接收键盘，但不会把整个 App 激活。所以这里刻意不调
//! `NSApp.activate` / `NSApp.hide`，只对这一个窗口 makeKeyAndOrderFront / orderOut，
//! 唤起和收起都不打扰用户当前的前台 App（见 _research/launcher/04 §1）。

use gpui_kit::*;
use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSWindow};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub fn show(window: &mut Window) {
    // GPUI 在 mac 上的实现就是 makeKeyAndOrderFront:，不激活 App。
    window.activate_window();
}

pub fn hide(window: &mut Window) {
    match ns_window(window) {
        Some(ns_window) => ns_window.orderOut(None),
        // 拿不到原生窗口说明平台层变了，大声报出来而不是假装隐藏成功。
        None => eprintln!("huan: 无法取得 NSWindow，面板未隐藏"),
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
