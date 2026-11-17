//! 便签窗口的 AppKit 原生封装：无边框置顶、跨全屏 Space、圆角毛玻璃、
//! 非激活态与编辑态（激活 Helmp 支持中文输入法组字）切换，以及全局鼠标状态查询。
//!
//! 参照 `src/panel.rs` 与 `src/capture/native.rs` 模式在模块内独立实现，不改动 `src/capture/`。

use std::ptr::NonNull;

use block2::RcBlock;
use gpui_kit::Window;
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationDidChangeScreenParametersNotification, NSAutoresizingMaskOptions,
    NSBezierPath, NSColor, NSEvent, NSGlassEffectView, NSGlassEffectViewStyle, NSImage,
    NSPopUpMenuWindowLevel, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_foundation::{
    NSEdgeInsets, NSNotification, NSNotificationCenter, NSObjectProtocol, NSPoint, NSRect, NSSize,
};

use super::screen::{cocoa_to_tl, point_cocoa_to_tl, primary_height, tl_to_cocoa};
use super::store::Frame;

pub const CARD_CORNER_RADIUS: f64 = 12.0;
pub const TAB_CORNER_RADIUS: f64 = 8.0;

pub fn offscreen() -> bool {
    crate::settings::self_test_offscreen()
        && std::env::var_os("HELMP_SELF_TEST_KEY_WINDOW").is_none()
}

pub fn note_style_mask(editing: bool) -> NSWindowStyleMask {
    if editing {
        NSWindowStyleMask::Borderless
    } else {
        NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel
    }
}

/// 在 GPUI 窗口创建后调用一次：配置置顶/跨 Space/常驻属性、垫圆角毛玻璃并摆放到 `frame_tl`。
pub fn setup(window: &Window, frame_tl: Frame, radius: f64, hidden: bool) {
    let Some(ns_window) = crate::panel::ns_window(window) else {
        eprintln!("helmp: 便签窗口取不到 NSWindow，无法配置层级和位置");
        return;
    };
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    ns_window.setStyleMask(note_style_mask(false));
    ns_window.setLevel(NSPopUpMenuWindowLevel);
    ns_window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    ns_window.setHidesOnDeactivate(false);
    ns_window.setCanHide(false);

    install_glass(&ns_window, radius, mtm);
    crate::panel::ensure_text_input(&ns_window);

    ns_window.setOpaque(false);
    ns_window.setBackgroundColor(Some(&NSColor::clearColor()));
    ns_window.setHasShadow(true);

    let rect = if offscreen() {
        NSRect::new(
            NSPoint::new(-20000.0, -20000.0),
            NSSize::new(frame_tl.w, frame_tl.h),
        )
    } else {
        to_ns_rect(tl_to_cocoa(frame_tl, primary_height(mtm)))
    };
    ns_window.setFrame_display(rect, true);
    ns_window.invalidateShadow();
    if !hidden && !offscreen() {
        ns_window.orderFrontRegardless();
    } else if hidden {
        ns_window.orderOut(None);
    }
}

fn install_glass(ns_window: &NSWindow, radius: f64, mtm: MainThreadMarker) {
    let Some(content) = ns_window.contentView() else {
        return;
    };
    let fill =
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable;
    if crate::panel::liquid_glass_available() {
        let glass = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), content.frame());
        glass.setStyle(NSGlassEffectViewStyle::Regular);
        glass.setCornerRadius(radius);
        glass.setAutoresizingMask(fill);
        ns_window.setContentView(Some(&glass));
        content.setFrame(glass.bounds());
        content.setAutoresizingMask(fill);
        glass.setContentView(Some(&content));
    } else {
        let effect = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), content.frame());
        effect.setMaterial(NSVisualEffectMaterial::Popover);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setState(NSVisualEffectState::Active);
        effect.setMaskImage(Some(&rounded_mask(radius)));
        effect.setAutoresizingMask(fill);
        ns_window.setContentView(Some(&effect));
        content.setFrame(effect.bounds());
        content.setAutoresizingMask(fill);
        effect.addSubview(&content);
    }
}

fn update_glass_radius(ns_window: &NSWindow, radius: f64) {
    let Some(content) = ns_window.contentView() else {
        return;
    };
    if crate::panel::liquid_glass_available() {
        if let Ok(glass) = content.downcast::<NSGlassEffectView>() {
            glass.setCornerRadius(radius);
        }
    } else if let Ok(effect) = content.downcast::<NSVisualEffectView>() {
        effect.setMaskImage(Some(&rounded_mask(radius)));
    }
}

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

fn to_ns_rect(frame: Frame) -> NSRect {
    NSRect::new(
        NSPoint::new(frame.x, frame.y),
        NSSize::new(frame.w, frame.h),
    )
}

/// 在 GPUI 更新之外读写并操纵原生窗口的句柄。
#[derive(Clone)]
pub struct NoteFrameHandle(Retained<NSWindow>);

impl NoteFrameHandle {
    pub fn of(window: &Window) -> Option<Self> {
        crate::panel::ns_window(window).map(Self)
    }

    pub fn ns_window(&self) -> &NSWindow {
        &self.0
    }

    pub fn frame_tl(&self) -> Option<Frame> {
        let mtm = MainThreadMarker::new()?;
        let rect = self.0.frame();
        Some(cocoa_to_tl(
            Frame {
                x: rect.origin.x,
                y: rect.origin.y,
                w: rect.size.width,
                h: rect.size.height,
            },
            primary_height(mtm),
        ))
    }

    pub fn set_frame_tl(&self, frame_tl: Frame, radius: f64) {
        if offscreen() {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        update_glass_radius(&self.0, radius);
        let target = to_ns_rect(tl_to_cocoa(frame_tl, primary_height(mtm)));
        let current = self.0.frame();
        let same = (current.origin.x - target.origin.x).abs() < 0.5
            && (current.origin.y - target.origin.y).abs() < 0.5
            && (current.size.width - target.size.width).abs() < 0.5
            && (current.size.height - target.size.height).abs() < 0.5;
        if !same {
            self.0.setFrame_display(target, true);
            self.0.invalidateShadow();
        }
    }

    pub fn set_origin_tl(&self, x: f64, y: f64, h: f64) {
        if offscreen() {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let cocoa_y = primary_height(mtm) - (y + h);
        self.0.setFrameOrigin(NSPoint::new(x.round(), cocoa_y.round()));
    }

    pub fn set_visible(&self, visible: bool) {
        if offscreen() {
            return;
        }
        if visible {
            self.0.orderFrontRegardless();
        } else {
            self.0.orderOut(None);
        }
    }

    pub fn order_front(&self) {
        if !offscreen() {
            self.0.orderFrontRegardless();
        }
    }

    /// 切换激活状态：进入编辑时移除 `NonactivatingPanel` 并激活 Helmp 以支持中文 IME 组字；
    /// 退出编辑时恢复 `NonactivatingPanel`（之后由调用方按需 `panel::return_focus`）。
    pub fn set_editing(&self, editing: bool) {
        self.0.setStyleMask(note_style_mask(editing));
        self.0.invalidateShadow();
        if editing && !offscreen() {
            crate::panel::ensure_text_input(&self.0);
            if let Some(mtm) = MainThreadMarker::new() {
                #[allow(deprecated)]
                NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
            }
            self.0.makeKeyAndOrderFront(None);
        }
    }
}

/// 当前全局鼠标坐标（Top-Left 全局坐标，单位 pt）。
pub fn mouse_location_tl() -> Option<(f64, f64)> {
    let mtm = MainThreadMarker::new()?;
    let p = NSEvent::mouseLocation();
    Some(point_cocoa_to_tl(p.x, p.y, primary_height(mtm)))
}

/// 鼠标左键此刻是否处于物理按下状态。
pub fn left_mouse_pressed() -> bool {
    (NSEvent::pressedMouseButtons() & 1) != 0
}

/// 保活屏幕参数变化观察者，析构时自动向通知中心注销。
pub struct NotificationObserver(Retained<ProtocolObject<dyn NSObjectProtocol>>);

impl Drop for NotificationObserver {
    fn drop(&mut self) {
        // SAFETY: `self.0` 是 `addObserverForName:object:queue:usingBlock:` 返回的注册令牌。
        unsafe {
            NSNotificationCenter::defaultCenter().removeObserver(self.0.as_ref());
        }
    }
}

/// 订阅显示器增删、分辨率及可见区域（Dock / 菜单栏）变化通知。
pub fn observe_screen_parameters(on_change: impl Fn() + Send + Sync + 'static) -> NotificationObserver {
    let block = RcBlock::new(move |_: NonNull<NSNotification>| {
        on_change();
    });
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: 传入静态通知名与合法 block，返回的 token 由 `NotificationObserver` 持有并在 Drop 时注销。
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSApplicationDidChangeScreenParametersNotification),
            None,
            None,
            &block,
        )
    };
    NotificationObserver(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_mask_toggles_nonactivating_panel_for_ime() {
        // 常驻浮窗必须有 NonactivatingPanel（不打断其他 App 输入）；编辑态必须去掉它，
        // 否则中文输入法无法建立组字会话。
        let idle = note_style_mask(false);
        assert!(idle.contains(NSWindowStyleMask::Borderless));
        assert!(idle.contains(NSWindowStyleMask::NonactivatingPanel));

        let editing = note_style_mask(true);
        assert!(editing.contains(NSWindowStyleMask::Borderless));
        assert!(!editing.contains(NSWindowStyleMask::NonactivatingPanel));
    }

    #[test]
    fn screen_parameters_observer_receives_notification_and_unregisters_on_drop() {
        // 运行中显示器热插拔或 Dock 变更依赖该通知唤醒布局重排；Drop 时必须注销避免悬空观察者。
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let hits = Arc::new(AtomicUsize::new(0));
        let hits_clone = hits.clone();
        let observer = observe_screen_parameters(move || {
            hits_clone.fetch_add(1, Ordering::SeqCst);
        });
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: 向默认通知中心发送标准屏幕参数变化通知，仅通知已注册 observer。
        unsafe {
            center.postNotificationName_object(
                NSApplicationDidChangeScreenParametersNotification,
                None,
            );
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        drop(observer);
        // SAFETY: 同上，用于验证 observer Drop 后不再收到回调。
        unsafe {
            center.postNotificationName_object(
                NSApplicationDidChangeScreenParametersNotification,
                None,
            );
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
