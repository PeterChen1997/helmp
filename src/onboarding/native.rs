//! 授权浮窗的 AppKit 部分：读系统设置窗口位置（CGWindowList，不需要录屏权限）、
//! 屏幕工作区、浮窗面板属性，以及真正的系统文件拖拽源（`HelmpDragTileView`）。
//!
//! GPUI 不支持把文件拖到别的 App，所以拖拽卡片的位置上叠一个透明的原生 NSView：
//! 卡片的样子由 GPUI 画，鼠标按下拖动时由这个 NSView 发起 `beginDraggingSession`，
//! 拖拽内容是 Helmp.app 的文件 URL，系统设置的权限列表接收它就完成添加。

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::Window;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{
    AnyThread as _, DefinedClass, MainThreadMarker, MainThreadOnly, Message as _, define_class,
    msg_send,
};
use objc2_app_kit::{
    NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession, NSDraggingSource,
    NSEvent, NSImage, NSRunningApplication, NSScreen, NSView, NSWindow, NSWindowCollectionBehavior,
    NSWorkspace,
};
use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID};
use objc2_foundation::{NSArray, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::placement::{Rect, SettingsState, WindowInfo, flip_y, settings_state};

/// Ventura 起改名为「系统设置」，bundle id 一直没变；按 bundle id 判断，不按本地化的进程名。
const SETTINGS_BUNDLE_ID: &str = "com.apple.systempreferences";

// ---------------------------------------------------------------- 系统设置窗口

/// 读一次系统设置窗口的状态。读失败返回 None（表示「不知道」，调用方不能当成「已关闭」）。
pub fn read_settings_state() -> Option<SettingsState> {
    let windows = read_windows()?;
    let running = !NSRunningApplication::runningApplicationsWithBundleIdentifier(
        &NSString::from_str(SETTINGS_BUNDLE_ID),
    )
    .is_empty();
    Some(settings_state(&windows, std::process::id() as i32, running))
}

fn read_windows() -> Option<Vec<WindowInfo>> {
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let list = CGWindowListCopyWindowInfo(options, kCGNullWindowID)?;
    // SAFETY: CFArray 与 NSArray 免费桥接；`list` 在本函数内一直持有，借用不会悬空。
    let array: &NSArray<AnyObject> = unsafe { &*(std::ptr::from_ref(&*list).cast()) };
    let mut is_settings_pid: HashMap<i32, bool> = HashMap::new();
    let mut windows = Vec::new();
    for entry in array.iter() {
        let Some(bounds) = object_for(&entry, "kCGWindowBounds") else {
            continue;
        };
        let rect = Rect::new(
            number(&bounds, "X")?,
            number(&bounds, "Y")?,
            number(&bounds, "Width")?,
            number(&bounds, "Height")?,
        );
        if ![rect.x, rect.y, rect.width, rect.height]
            .iter()
            .all(|v| v.is_finite())
        {
            continue;
        }
        let pid = number(&entry, "kCGWindowOwnerPID").unwrap_or(0.) as i32;
        let layer = number(&entry, "kCGWindowLayer").unwrap_or(-1.) as i64;
        let owner_name = object_for(&entry, "kCGWindowOwnerName")
            .and_then(|name| name.downcast::<NSString>().ok())
            .map(|name| name.to_string())
            .unwrap_or_default();
        let is_settings = *is_settings_pid.entry(pid).or_insert_with(|| {
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                .and_then(|app| app.bundleIdentifier())
                .is_some_and(|id| id.to_string() == SETTINGS_BUNDLE_ID)
        });
        windows.push(WindowInfo {
            owner_pid: pid,
            owner_name,
            layer,
            bounds: rect,
            is_settings,
        });
    }
    Some(windows)
}

fn object_for(dict: &AnyObject, key: &str) -> Option<Retained<AnyObject>> {
    let key = NSString::from_str(key);
    // SAFETY: `dict` 是 CGWindowList 返回的 NSDictionary；objectForKey: 返回自动释放的对象或 nil。
    unsafe { msg_send![dict, objectForKey: &*key] }
}

fn number(dict: &AnyObject, key: &str) -> Option<f64> {
    let value = object_for(dict, key)?;
    // SAFETY: 这些键的值都是 NSNumber；先确认响应 doubleValue 再调用。
    let responds: bool =
        unsafe { msg_send![&*value, respondsToSelector: objc2::sel!(doubleValue)] };
    responds.then(|| unsafe { msg_send![&*value, doubleValue] })
}

// ---------------------------------------------------------------- 屏幕与面板

/// 主屏（菜单栏所在屏）高度：AppKit 与 CG 坐标互转用。
fn primary_height(mtm: MainThreadMarker) -> Option<f64> {
    NSScreen::screens(mtm)
        .firstObject()
        .map(|s| s.frame().size.height)
}

fn to_rect(r: NSRect) -> Rect {
    Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

/// 浮窗所在屏的工作区（CG 坐标）：系统设置窗口所在的屏；没有时用主屏。
pub fn work_area_for(settings: Option<Rect>) -> Option<Rect> {
    let mtm = MainThreadMarker::new()?;
    let primary = primary_height(mtm)?;
    let screens = NSScreen::screens(mtm);
    let areas: Vec<(Rect, Rect)> = screens
        .iter()
        .map(|s| {
            (
                flip_y(to_rect(s.frame()), primary),
                flip_y(to_rect(s.visibleFrame()), primary),
            )
        })
        .collect();
    let chosen = settings
        .and_then(|settings| {
            areas
                .iter()
                .find(|(frame, _)| frame.contains_center_of(&settings))
        })
        .or(areas.first())?;
    Some(chosen.1)
}

/// 把浮窗放到 CG 坐标的 `rect`。
pub fn set_frame(ns_window: &NSWindow, rect: Rect) {
    let Some(primary) = MainThreadMarker::new().and_then(primary_height) else {
        return;
    };
    let r = flip_y(rect, primary);
    ns_window.setFrame_display(
        NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.width, r.height)),
        true,
    );
}

/// 浮窗跟着系统设置跨 Space、在全屏 App 上也能出现；App 不在前台时不能自动隐藏。
pub fn configure_panel(ns_window: &NSWindow) {
    ns_window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    ns_window.setHidesOnDeactivate(false);
}

/// GPUI 内容视图（拖拽卡片叠在它上面）。
pub fn gpui_view(window: &Window) -> Option<Retained<NSView>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // SAFETY: AppKit 句柄里的 ns_view 是 GPUI 持有的、仍然存活的 NSView；只在主线程使用。
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    Some(view.retain())
}

/// 当前进程所在的 .app 包；从 target/ 直接运行时为 None。
pub fn bundle_path() -> Option<PathBuf> {
    crate::settings::system::running_from_bundle().then(|| {
        PathBuf::from(
            objc2_foundation::NSBundle::mainBundle()
                .bundlePath()
                .to_string(),
        )
    })
}

// ---------------------------------------------------------------- 拖拽源

pub struct DragIvars {
    url: Retained<NSURL>,
    icon: Retained<NSImage>,
    dragging: Cell<bool>,
}

define_class!(
    // SAFETY: NSView 没有额外的子类化要求；本类不实现 Drop。
    #[unsafe(super(NSView, objc2_app_kit::NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "HelmpDragTileView"]
    #[ivars = DragIvars]
    pub struct DragTileView;

    unsafe impl NSObjectProtocol for DragTileView {}

    unsafe impl NSDraggingSource for DragTileView {
        // SAFETY: 签名与 NSDraggingSource 一致。
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn source_operation_mask(
            &self,
            _session: &NSDraggingSession,
            _context: NSDraggingContext,
        ) -> NSDragOperation {
            NSDragOperation::Copy | NSDragOperation::Link | NSDragOperation::Generic
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn ended(&self, _session: &NSDraggingSession, _point: NSPoint, _op: NSDragOperation) {
            self.ivars().dragging.set(false);
        }
    }

    impl DragTileView {
        // 非激活面板：第一次点击就要响应，不能先被吞掉用来激活窗口。
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            self.ivars().dragging.set(false);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            if self.ivars().dragging.replace(true) {
                return;
            }
            self.begin_drag(event);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.ivars().dragging.set(false);
        }
    }
);

impl DragTileView {
    fn new(mtm: MainThreadMarker, frame: NSRect, bundle: &Path) -> Retained<Self> {
        let path = NSString::from_str(&bundle.to_string_lossy());
        let icon = NSWorkspace::sharedWorkspace().iconForFile(&path);
        let url = NSURL::fileURLWithPath(&path);
        let this = Self::alloc(mtm).set_ivars(DragIvars {
            url,
            icon,
            dragging: Cell::new(false),
        });
        // SAFETY: NSView 的指定初始化方法。
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn begin_drag(&self, event: &NSEvent) {
        let ivars = self.ivars();
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let item = NSDraggingItem::initWithPasteboardWriter(
            NSDraggingItem::alloc(),
            ProtocolObject::from_ref(&*ivars.url),
        );
        let frame = NSRect::new(
            NSPoint::new(point.x - 24., point.y - 24.),
            NSSize::new(48., 48.),
        );
        let contents: &AnyObject = &ivars.icon;
        // SAFETY: contents 是 NSImage，符合 setDraggingFrame:contents: 的要求。
        unsafe { item.setDraggingFrame_contents(frame, Some(contents)) };
        let items = NSArray::from_retained_slice(&[item]);
        self.beginDraggingSessionWithItems_event_source(
            &items,
            event,
            ProtocolObject::from_ref(self),
        );
    }
}

/// 在 GPUI 视图上叠一个拖拽源，`frame` 为面板内左上角坐标系的位置（点）。
pub fn install_drag_tile(
    window: &Window,
    frame: Rect,
    bundle: &Path,
) -> Option<Retained<DragTileView>> {
    let mtm = MainThreadMarker::new()?;
    let host = gpui_view(window)?;
    let bounds = host.bounds();
    let y = if host.isFlipped() {
        frame.y
    } else {
        bounds.size.height - frame.y - frame.height
    };
    let rect = NSRect::new(
        NSPoint::new(frame.x, y),
        NSSize::new(frame.width, frame.height),
    );
    let tile = DragTileView::new(mtm, rect, bundle);
    host.addSubview(&tile);
    Some(tile)
}

#[cfg(test)]
mod tests {
    use super::read_settings_state;

    #[test]
    fn reads_window_list_without_screen_recording_permission() {
        // 浮窗在录屏权限授予之前就要跟随系统设置；读不到窗口列表它就只能永远停在屏幕底部。
        assert!(read_settings_state().is_some());
    }
}
