//! 面板显示 / 隐藏的平台适配层。
//! 现状：NSApp 级 activate / hide（会抢前台 App 焦点）。
//! 目标：对 NSPanel 直接 makeKeyAndOrderFront / orderOut，保住 NonactivatingPanel（见 _research/launcher/04）。

use gpui_kit::*;

pub fn show(window: &mut Window, cx: &mut App) {
    cx.activate(true);
    window.activate_window();
}

pub fn hide(cx: &mut App) {
    cx.hide();
}
