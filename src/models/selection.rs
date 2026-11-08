//! 读取前台应用里选中的文字（辅助功能 AX：focused element 的 `AXSelectedText`）。
//!
//! - 没有辅助功能权限直接返回 None，不弹授权框。
//! - 面板唤起后 Helmp 自己是 key window，但它是 Accessory 应用，不接管菜单栏：
//!   `menuBarOwningApplication` 仍是用户刚才所在的应用，所以从它取，而不是 system-wide 的焦点
//!   （那会是 Helmp 自己的输入框）。
//! - 每次 AX 调用最多等 `TIMEOUT`，卡死的应用不会拖住查询线程。

use std::ffi::c_void;

use objc2::rc::{Retained, autoreleasepool};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::NSString;

/// 单次 AX 消息的超时（秒）。系统默认 6 秒，查询线程等不起。
const TIMEOUT: f32 = 0.25;

type CFTypeRef = *const c_void;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> i32;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFTypeRef,
        value: *mut CFTypeRef,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
}

/// 拥有一个 CF 对象，drop 时 CFRelease。
struct Owned(CFTypeRef);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: 只包装 Create / Copy 规则得到的非空对象。
        unsafe { CFRelease(self.0) }
    }
}

/// 读一个属性；失败（无此属性、超时、无权限）返回 None。
fn copy_attribute(element: &Owned, name: &str) -> Option<Owned> {
    let name = NSString::from_str(name);
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: NSString 与 CFString 无缝桥接；value 按 Copy 规则返回 +1 引用，交给 Owned 释放。
    let err = unsafe {
        AXUIElementCopyAttributeValue(element.0, Retained::as_ptr(&name).cast(), &mut value)
    };
    (err == 0 && !value.is_null()).then(|| Owned(value))
}

/// 用户所在应用的 pid（排除 Helmp 自己）。
fn target_pid() -> Option<i32> {
    let me = i32::try_from(std::process::id()).ok()?;
    autoreleasepool(|_| {
        let workspace = NSWorkspace::sharedWorkspace();
        [
            workspace.menuBarOwningApplication(),
            workspace.frontmostApplication(),
        ]
        .into_iter()
        .flatten()
        .map(|app| app.processIdentifier())
        .find(|&pid| pid > 0 && pid != me)
    })
}

/// 前台应用里选中的文字；没有权限、没有选中、应用不支持 AX 时返回 None。
pub fn selected_text() -> Option<String> {
    // SAFETY: 无参数的只读查询，不弹框。
    if !unsafe { AXIsProcessTrusted() } {
        return None;
    }
    let pid = target_pid()?;
    // SAFETY: 返回 +1 引用（可能为空），交给 Owned。
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return None;
    }
    let app = Owned(app);
    // SAFETY: app 是有效的 AXUIElement。
    unsafe { AXUIElementSetMessagingTimeout(app.0, TIMEOUT) };
    let focused = copy_attribute(&app, "AXFocusedUIElement")?;
    // SAFETY: focused 是有效的 AXUIElement。
    unsafe { AXUIElementSetMessagingTimeout(focused.0, TIMEOUT) };
    let selected = copy_attribute(&focused, "AXSelectedText")?;
    // SAFETY: 先确认类型是 CFString，再按无缝桥接当 NSString 读，不转移所有权（Owned 负责释放）。
    let text = unsafe {
        if CFGetTypeID(selected.0) != CFStringGetTypeID() {
            return None;
        }
        let string: &NSString = &*selected.0.cast::<NSString>();
        string.to_string()
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}
