//! 截图界面各窗口的原生设置：GPUI 只给出带标题栏样式的面板，这里改成无边框、调层级、摆位置，
//! 并在 GPUI 视图下面垫一层 NSImageView 显示截图（整屏冻结图可能有 5K，交给 Core Animation 画，
//! 不进 GPUI 的纹理图集；钉图同理）。GPUI 只画半透明的选区、工具栏这些内容。

use std::path::Path;

use gpui_kit::Window;
use objc2::rc::Retained;
use objc2::{AnyThread as _, MainThreadMarker};
use objc2_app_kit::{
    NSColor, NSImage, NSImageScaling, NSImageView, NSPopUpMenuWindowLevel,
    NSScreenSaverWindowLevel, NSWindowCollectionBehavior, NSWindowLevel, NSWindowOrderingMode,
    NSWindowStyleMask,
};
use objc2_foundation::{NSData, NSPoint, NSRect, NSSize, NSString};

use super::geometry::Rect;

/// 截图窗口的种类，决定层级、能否激活 Helmp、隐藏 Helmp 时是否留下。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 冻结屏幕的遮罩：盖住菜单栏和程序坞（屏保层级），激活 Helmp 拿键盘。
    Overlay,
    /// 识别结果窗口：浮在其他 App 上面，激活 Helmp 拿键盘（回车复制、Esc 关闭）。
    Result,
    /// 钉图：始终置顶，点它不激活 Helmp（不打断用户在别的 App 里的输入），Helmp 隐藏时也留着。
    Pin,
    /// 「已复制」之类的提示：不接收鼠标，一会儿自己消失。
    Toast,
}

/// 窗口创建后调用一次：去掉标题栏样式、设层级与跨桌面行为、摆到 `frame`（AppKit 全局坐标）。
/// `offscreen`（图形自检）时摆到屏幕外，并且不前置成 key window，不抢用户的键盘。
pub fn setup(window: &Window, kind: Kind, frame: Rect, offscreen: bool) {
    let Some(ns_window) = crate::panel::ns_window(window) else {
        eprintln!("helmp: 截图窗口取不到 NSWindow，无法设置层级和位置");
        return;
    };
    // 无边框：带标题栏样式的窗口会被系统限制在菜单栏下面，盖不住整块屏幕。
    // 去掉 NonactivatingPanel 的窗口会激活 Helmp（要键盘的遮罩和结果窗口）；钉图和提示保留它。
    let mut mask = NSWindowStyleMask::Borderless;
    if matches!(kind, Kind::Pin | Kind::Toast) {
        mask |= NSWindowStyleMask::NonactivatingPanel;
    }
    ns_window.setStyleMask(mask);
    let level: NSWindowLevel = match kind {
        Kind::Overlay => NSScreenSaverWindowLevel,
        _ => NSPopUpMenuWindowLevel,
    };
    ns_window.setLevel(level);
    ns_window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    // NSPanel 默认在 App 失去前台时隐藏：钉图、结果窗口必须一直留着，用户随时能看到。
    ns_window.setHidesOnDeactivate(false);
    // 隐藏 Helmp（把焦点还给别的 App）时钉图和提示要留在屏幕上。
    ns_window.setCanHide(!matches!(kind, Kind::Pin | Kind::Toast));
    ns_window.setOpaque(false);
    ns_window.setBackgroundColor(Some(&NSColor::clearColor()));
    ns_window.setHasShadow(matches!(kind, Kind::Pin | Kind::Result | Kind::Toast));
    if kind == Kind::Toast {
        ns_window.setIgnoresMouseEvents(true);
    }
    let rect = if offscreen {
        NSRect::new(
            NSPoint::new(-20000.0, -20000.0),
            NSSize::new(frame.w, frame.h),
        )
    } else {
        ns_rect(frame)
    };
    ns_window.setFrame_display(rect, true);
    ns_window.invalidateShadow();
}

pub fn ns_rect(rect: Rect) -> NSRect {
    NSRect::new(NSPoint::new(rect.x, rect.y), NSSize::new(rect.w, rect.h))
}

/// 前置窗口。`key`：成为 key window 并激活 Helmp（遮罩、结果窗口要键盘）；否则只是不抢焦点地浮到最前
/// （钉图、提示）。无论哪种，都用 orderFrontRegardless：Helmp 不在前台时窗口也必须出现。
pub fn present(window: &Window, key: bool, offscreen: bool) {
    let Some(ns_window) = crate::panel::ns_window(window) else {
        return;
    };
    ns_window.orderFrontRegardless();
    if key && !offscreen {
        if let Some(mtm) = MainThreadMarker::new() {
            #[allow(deprecated)]
            objc2_app_kit::NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        }
        ns_window.makeKeyAndOrderFront(None);
    }
    crate::panel::ensure_text_input(&ns_window);
}

/// 暂时收起 / 重新显示（另存为面板出现期间，屏保层级的遮罩会盖住它）。
pub fn set_visible(window: &Window, visible: bool, offscreen: bool) {
    let Some(ns_window) = crate::panel::ns_window(window) else {
        return;
    };
    if visible {
        present(window, true, offscreen);
    } else {
        ns_window.orderOut(None);
    }
}

/// 在 GPUI 视图下面垫一层图片：`size` 是点尺寸，图片按它铺满（Retina 截图 2 倍像素正好清晰）。
pub fn underlay_file(window: &Window, image: &Path, size: (f64, f64)) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let path = NSString::from_str(&image.to_string_lossy());
    match NSImage::initWithContentsOfFile(NSImage::alloc(), &path) {
        Some(image) => underlay(window, &image, size, mtm),
        None => eprintln!("helmp: 读不出截图 {}", image.display()),
    }
}

/// 同 [`underlay_file`]，图片来自内存里的 PNG（钉图）。
pub fn underlay_png(window: &Window, png: &[u8], size: (f64, f64)) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    match NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(png)) {
        Some(image) => underlay(window, &image, size, mtm),
        None => eprintln!("helmp: 钉图的图片无法解码"),
    }
}

fn underlay(window: &Window, image: &NSImage, size: (f64, f64), mtm: MainThreadMarker) {
    let Some(ns_window) = crate::panel::ns_window(window) else {
        return;
    };
    let Some(content) = ns_window.contentView() else {
        eprintln!("helmp: 截图窗口没有 contentView，画不出截图");
        return;
    };
    let size = NSSize::new(size.0, size.1);
    image.setSize(size);
    let view: Retained<NSImageView> = NSImageView::imageViewWithImage(image, mtm);
    view.setImageScaling(NSImageScaling::ScaleAxesIndependently);
    view.setFrame(content.bounds());
    view.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    // GPUI 的视图是 contentView 的第一个子视图：图片插到它下面。
    let gpui = content.subviews().firstObject();
    content.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Below, gpui.as_deref());
}

/// 窗口现在的位置（AppKit 全局坐标）。
pub fn frame(window: &Window) -> Option<Rect> {
    let frame = crate::panel::ns_window(window)?.frame();
    Some(Rect::new(
        frame.origin.x,
        frame.origin.y,
        frame.size.width,
        frame.size.height,
    ))
}

/// 鼠标此刻的位置（AppKit 全局坐标）：遮罩出现时让鼠标所在屏的那块成为 key window。
pub fn mouse_location() -> (f64, f64) {
    let p = objc2_app_kit::NSEvent::mouseLocation();
    (p.x, p.y)
}

/// 窗口所在屏幕的 frame 与 visibleFrame（AppKit 全局坐标）。钉图上识别文字时摆放结果窗口用。
pub fn screen_of(window: &Window) -> Option<(Rect, Rect)> {
    let screen = crate::panel::ns_window(window)?.screen()?;
    let rect = |r: NSRect| Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height);
    Some((rect(screen.frame()), rect(screen.visibleFrame())))
}
