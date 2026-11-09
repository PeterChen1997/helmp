//! 菜单栏工具平时不进 Dock、不参与 ⌘Tab（Accessory）。但引导、设置这类普通窗口开着时，
//! 用户会跳到系统设置去授权，没有 Dock 图标和 ⌘Tab 就切不回来。
//! 所以这类窗口开着期间临时切成 Regular，最后一个关掉后再切回 Accessory。

use std::sync::atomic::{AtomicUsize, Ordering};

use gpui_kit::{ActivationPolicy, Context};

static OPEN_WINDOWS: AtomicUsize = AtomicUsize::new(0);

/// 在普通窗口根视图的构造函数里调用：视图释放（窗口关闭）时自动计数减一。
pub fn track_window<T: 'static>(cx: &mut Context<T>) {
    if OPEN_WINDOWS.fetch_add(1, Ordering::SeqCst) == 0 {
        cx.set_activation_policy(ActivationPolicy::Regular);
    }
    cx.on_release(|_, cx| {
        if OPEN_WINDOWS.fetch_sub(1, Ordering::SeqCst) == 1 {
            cx.set_activation_policy(ActivationPolicy::Accessory);
        }
    })
    .detach();
}
