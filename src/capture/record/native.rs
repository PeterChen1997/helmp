//! 录屏时的原生部分：菜单栏上的计时按钮（点一下停止），以及 Esc 停止的按键监听。
//!
//! AppKit 回调只把「停止」投进 channel，由 GPUI 前台任务执行（同 `menubar`）。

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSColor, NSEvent, NSEventMask, NSEventModifierFlags, NSImage, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};

use super::StopBy;
use crate::i18n::tr;

/// Esc 的虚拟键码（kVK_Escape）。
const ESCAPE: u16 = 53;

/// 菜单栏上的录制计时：「⏹ 00:12」，点一下停止。drop 时移除。
pub struct StatusItem {
    item: Retained<NSStatusItem>,
    _target: Retained<Target>,
}

impl StatusItem {
    pub fn new(stop: async_channel::Sender<StopBy>) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let target = Target::new(stop, mtm);
        let button = item.button(mtm)?;
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("stop.circle.fill"),
            Some(&NSString::from_str(tr("停止录屏", "Stop Recording"))),
        );
        button.setImage(image.as_deref());
        button.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageLeading);
        button.setContentTintColor(Some(&NSColor::systemRedColor()));
        button.setToolTip(Some(&NSString::from_str(tr(
            "正在录屏 · 点击停止",
            "Recording · Click to stop",
        ))));
        // SAFETY: target 活得和 item 一样久（存在 StatusItem 里）；action 签名与 Target 的方法一致。
        unsafe {
            button.setTarget(Some(&target));
            button.setAction(Some(sel!(stopRecording:)));
        }
        let this = Self {
            item,
            _target: target,
        };
        this.set_elapsed(0);
        Some(this)
    }

    pub fn set_elapsed(&self, seconds: u64) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        if let Some(button) = self.item.button(mtm) {
            button.setTitle(&NSString::from_str(&format!(" {}", super::clock(seconds))));
        }
    }
}

impl Drop for StatusItem {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}

struct TargetIvars {
    stop: async_channel::Sender<StopBy>,
}

define_class!(
    // SAFETY: NSObject 没有子类化要求；Target 没有实现 Drop。
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetIvars]
    struct Target;

    impl Target {
        // SAFETY: NSControl 的 action 签名（一个 sender 参数，无返回值）。
        #[unsafe(method(stopRecording:))]
        fn stop_recording(&self, _sender: &AnyObject) {
            let _ = self.ivars().stop.try_send(StopBy::MenuBar);
        }
    }

    // SAFETY: NSObjectProtocol 没有额外安全要求。
    unsafe impl NSObjectProtocol for Target {}
);

impl Target {
    fn new(stop: async_channel::Sender<StopBy>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars { stop });
        // SAFETY: NSObject 的 init。
        unsafe { msg_send![super(this), init] }
    }
}

/// 录制中按 Esc 停止。
///
/// 别的 App 在前台时用全局监听（只旁听、不吞掉按键，被录的 App 照常收到 Esc；要辅助功能权限，
/// Helmp 粘贴本来就要它），Helmp 自己在前台时用本地监听。drop 时移除。
pub struct EscMonitor {
    monitors: Vec<Retained<AnyObject>>,
}

impl EscMonitor {
    pub fn new(stop: async_channel::Sender<StopBy>) -> Self {
        let is_escape = |event: &NSEvent| {
            let modifiers = event.modifierFlags()
                & (NSEventModifierFlags::Command
                    | NSEventModifierFlags::Option
                    | NSEventModifierFlags::Control
                    | NSEventModifierFlags::Shift);
            event.keyCode() == ESCAPE && modifiers.is_empty()
        };
        let global_stop = stop.clone();
        let global = RcBlock::new(move |event: NonNull<NSEvent>| {
            // SAFETY: 回调期间 event 有效。
            if is_escape(unsafe { event.as_ref() }) {
                let _ = global_stop.try_send(StopBy::Esc);
            }
        });
        let local = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: 回调期间 event 有效。
            if is_escape(unsafe { event.as_ref() }) {
                let _ = stop.try_send(StopBy::Esc);
            }
            event.as_ptr()
        });
        let mut monitors = Vec::new();
        monitors.extend(NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
            NSEventMask::KeyDown,
            &global,
        ));
        // SAFETY: block 原样返回传进来的事件（有效指针）。
        monitors.extend(unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &local)
        });
        Self { monitors }
    }
}

impl Drop for EscMonitor {
    fn drop(&mut self) {
        for monitor in &self.monitors {
            // SAFETY: 这些都是 addGlobal/LocalMonitor 返回的监听对象。
            unsafe { NSEvent::removeMonitor(monitor) };
        }
    }
}
