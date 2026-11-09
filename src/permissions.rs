//! 系统权限的实时只读查询（不弹授权框）。引导窗口、拖拽浮窗、截图共用。
//!
//! - 辅助功能：`AXIsProcessTrusted` 本身就是实时的。
//! - 录屏：`CGPreflightScreenCaptureAccess` 在授权后到 App 重启前一直返回旧结果，用户在系统设置里
//!   打开开关后引导窗口还停在「前往授权」。所以再加一条实时判据：只有拿到录屏权限的进程，
//!   才能读到其他 App 窗口的标题（`kCGWindowName`）。

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSRunningApplication;
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID};
use objc2_foundation::{NSArray, NSString};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

/// 是否已有辅助功能权限。
pub fn accessibility_trusted() -> bool {
    // SAFETY: 无参数的只读查询。
    unsafe { AXIsProcessTrusted() }
}

/// 是否已有录屏权限（实时）。
pub fn screen_recording_granted() -> bool {
    CGPreflightScreenCaptureAccess() || can_read_other_window_titles()
}

/// 能否读到其他 App 普通窗口的标题。程序坞和窗口服务器的窗口不算（它们的标题对谁都可见）。
fn can_read_other_window_titles() -> bool {
    let options = CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCopyWindowInfo(options, kCGNullWindowID) else {
        return false;
    };
    // SAFETY: CFArray 与 NSArray 免费桥接；`list` 在本函数内一直持有。
    let array: &NSArray<AnyObject> = unsafe { &*(std::ptr::from_ref(&*list).cast()) };
    let own_pid = std::process::id() as i32;
    array.iter().any(|entry| {
        let pid = number(&entry, "kCGWindowOwnerPID").unwrap_or(0.) as i32;
        let layer = number(&entry, "kCGWindowLayer").unwrap_or(-1.) as i64;
        if pid == own_pid || layer != 0 {
            return false;
        }
        let has_title = object_for(&entry, "kCGWindowName")
            .and_then(|name| name.downcast::<NSString>().ok())
            .is_some_and(|name| !name.to_string().is_empty());
        has_title
            && NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                .and_then(|app| app.bundleIdentifier())
                .is_some_and(|id| id.to_string() != "com.apple.dock")
    })
}

fn object_for(dict: &AnyObject, key: &str) -> Option<Retained<AnyObject>> {
    let key = NSString::from_str(key);
    // SAFETY: `dict` 是 CGWindowList 返回的 NSDictionary；objectForKey: 返回对象或 nil。
    unsafe { msg_send![dict, objectForKey: &*key] }
}

fn number(dict: &AnyObject, key: &str) -> Option<f64> {
    let value = object_for(dict, key)?;
    // SAFETY: 先确认响应 doubleValue 再调用。
    let responds: bool = unsafe { msg_send![&*value, respondsToSelector: objc2::sel!(doubleValue)] };
    responds.then(|| unsafe { msg_send![&*value, doubleValue] })
}
