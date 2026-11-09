//! 取词：读取前台应用里选中的文字。
//!
//! 1. 辅助功能 AX：focused element 的 `AXSelectedText`。不碰剪贴板，首选。
//! 2. AX 读不到（Chrome、Electron 等不提供这个属性）且调用方允许时，模拟 ⌘C：
//!    先给剪贴板拍快照，复制完读出文字，再把快照原样写回——用户剪贴板里原来是什么，取词后还是什么。
//!    AX 明确告诉我们「没有选中」时不走这一步：VS Code 等在没选中时 ⌘C 会复制整行，取到的不是用户想要的。
//!
//! - 没有辅助功能权限直接返回 None，不弹授权框（模拟按键同样需要这个权限）。
//! - 面板唤起后 Helmp 自己是 key window，但它是 Accessory 应用，不接管菜单栏：
//!   `menuBarOwningApplication` 仍是用户刚才所在的应用，所以从它取，而不是 system-wide 的焦点
//!   （那会是 Helmp 自己的输入框）。模拟 ⌘C 只能在面板显示之前做，否则按键会落到面板上。
//! - 每次 AX 调用最多等 `TIMEOUT`，卡死的应用不会拖住线程。

use std::ffi::c_void;
use std::time::{Duration, Instant};

use anyhow::Result;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardType, NSPasteboardTypeString, NSPasteboardWriting,
    NSWorkspace,
};
use objc2_foundation::{NSArray, NSData, NSString};

/// 单次 AX 消息的超时（秒）。系统默认 6 秒，等不起。
const TIMEOUT: f32 = 0.25;

/// 模拟 ⌘C 后最多等目标 App 写剪贴板这么久。
const COPY_WAIT: Duration = Duration::from_millis(300);

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

/// AX 读到的结果。
#[derive(Debug, PartialEq, Eq)]
enum Ax {
    Text(String),
    /// App 支持这个属性，但当前没有选中文字。
    Empty,
    /// App 不提供选中文字（或读失败）。
    Unsupported,
}

fn ax_selection() -> Ax {
    let Some(pid) = target_pid() else {
        return Ax::Unsupported;
    };
    // SAFETY: 返回 +1 引用（可能为空），交给 Owned。
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return Ax::Unsupported;
    }
    let app = Owned(app);
    // SAFETY: app 是有效的 AXUIElement。
    unsafe { AXUIElementSetMessagingTimeout(app.0, TIMEOUT) };
    let Some(focused) = copy_attribute(&app, "AXFocusedUIElement") else {
        return Ax::Unsupported;
    };
    // SAFETY: focused 是有效的 AXUIElement。
    unsafe { AXUIElementSetMessagingTimeout(focused.0, TIMEOUT) };
    let Some(selected) = copy_attribute(&focused, "AXSelectedText") else {
        return Ax::Unsupported;
    };
    // SAFETY: 先确认类型是 CFString，再按无缝桥接当 NSString 读，不转移所有权（Owned 负责释放）。
    let text = unsafe {
        if CFGetTypeID(selected.0) != CFStringGetTypeID() {
            return Ax::Unsupported;
        }
        let string: &NSString = &*selected.0.cast::<NSString>();
        string.to_string()
    };
    match text.trim() {
        "" => Ax::Empty,
        text => Ax::Text(text.to_string()),
    }
}

/// 取词。`allow_copy`：AX 读不到时是否模拟 ⌘C（只能在面板显示之前用，见模块说明）。
pub fn capture(allow_copy: bool) -> Option<String> {
    // SAFETY: 无参数的只读查询，不弹框。
    if !unsafe { AXIsProcessTrusted() } {
        return None;
    }
    match ax_selection() {
        Ax::Text(text) => Some(text),
        Ax::Empty => None,
        Ax::Unsupported if allow_copy => autoreleasepool(|_| {
            let _paused = crate::providers::clipboard::pause_recording(COPY_WAIT * 3);
            copy_selection(
                &NSPasteboard::generalPasteboard(),
                crate::providers::clipboard::macos::send_copy,
                COPY_WAIT,
            )
        }),
        Ax::Unsupported => None,
    }
}

/// 剪贴板快照：每个 item 的每种类型及其数据。
struct Snapshot(Vec<Vec<(Retained<NSPasteboardType>, Retained<NSData>)>>);

fn snapshot(pasteboard: &NSPasteboard) -> Snapshot {
    let items = pasteboard
        .pasteboardItems()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.types()
                        .iter()
                        // 拿不到数据的类型（文件承诺等）跳过，写回时它们本来也无法还原。
                        .filter_map(|ty| item.dataForType(&ty).map(|data| (ty, data)))
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default();
    Snapshot(items)
}

fn restore(pasteboard: &NSPasteboard, snapshot: &Snapshot) -> Result<()> {
    pasteboard.clearContents();
    if snapshot.0.is_empty() {
        return Ok(());
    }
    let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = snapshot
        .0
        .iter()
        .map(|types| {
            let item = NSPasteboardItem::new();
            for (ty, data) in types {
                item.setData_forType(data, ty);
            }
            ProtocolObject::from_retained(item)
        })
        .collect();
    anyhow::ensure!(
        pasteboard.writeObjects(&NSArray::from_retained_slice(&items)),
        crate::i18n::tr(
            "取词后还原剪贴板失败",
            "Couldn’t restore the clipboard after reading the selection"
        )
    );
    Ok(())
}

/// 模拟复制取词：拍快照 → `send_copy` → 等剪贴板变化（最多 `wait`）→ 读文字 → 还原快照。
/// 剪贴板没变（没有选中文字、按键被吞）时不碰剪贴板，返回 None。
fn copy_selection(
    pasteboard: &NSPasteboard,
    send_copy: impl FnOnce() -> Result<()>,
    wait: Duration,
) -> Option<String> {
    let before = pasteboard.changeCount();
    let saved = snapshot(pasteboard);
    if let Err(err) = send_copy() {
        eprintln!("helmp: 模拟 ⌘C 取词失败：{err:#}");
        return None;
    }
    let deadline = Instant::now() + wait;
    while pasteboard.changeCount() == before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if pasteboard.changeCount() == before {
        return None;
    }
    // SAFETY: AppKit 导出的常量 NSString，进程内始终有效。
    let text = pasteboard
        .stringForType(unsafe { NSPasteboardTypeString })
        .map(|text| text.to_string());
    if let Err(err) = restore(pasteboard, &saved) {
        // 大声失败：用户的剪贴板被改了，必须让人知道。
        eprintln!("helmp: {err:#}");
    }
    text.map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 私有 pasteboard，drop 时注销，测试不碰用户真实剪贴板。
    struct Private(Retained<NSPasteboard>);

    impl Private {
        fn new() -> Self {
            Self(NSPasteboard::pasteboardWithUniqueName())
        }
    }

    impl Drop for Private {
        fn drop(&mut self) {
            // SAFETY: `-[NSPasteboard releaseGlobally]` 无参数无返回值。
            unsafe { objc2::msg_send![&*self.0, releaseGlobally] }
        }
    }

    fn string_type() -> &'static NSPasteboardType {
        // SAFETY: AppKit 导出的常量。
        unsafe { NSPasteboardTypeString }
    }

    fn put(pb: &NSPasteboard, text: &str, extra: &[(&str, &[u8])]) {
        pb.clearContents();
        assert!(pb.setString_forType(&NSString::from_str(text), string_type()));
        for (ty, data) in extra {
            assert!(pb.setData_forType(Some(&NSData::with_bytes(data)), &NSString::from_str(ty)));
        }
    }

    fn data(pb: &NSPasteboard, ty: &str) -> Option<Vec<u8>> {
        pb.dataForType(&NSString::from_str(ty)).map(|d| d.to_vec())
    }

    #[test]
    fn copy_fallback_restores_users_clipboard() {
        // 取词靠模拟 ⌘C 时会覆盖剪贴板：读完必须原样还原（含自定义类型，如富文本、App 私有格式），
        // 否则用户翻译一次，之前复制的东西就没了。
        let pb = Private::new();
        put(&pb.0, "用户原来复制的", &[("com.example.private", b"\x00\x01rich")]);
        let got = copy_selection(
            &pb.0,
            || {
                put(&pb.0, "  selected words \n", &[]);
                Ok(())
            },
            Duration::from_millis(200),
        );
        assert_eq!(got.as_deref(), Some("selected words"));
        assert_eq!(
            pb.0.stringForType(string_type()).map(|s| s.to_string()).as_deref(),
            Some("用户原来复制的")
        );
        assert_eq!(
            data(&pb.0, "com.example.private").as_deref(),
            Some(&b"\x00\x01rich"[..])
        );
    }

    #[test]
    fn nothing_copied_leaves_clipboard_untouched() {
        // 没选中文字时 ⌘C 什么也不写：不能拿剪贴板里的旧内容冒充选中文字，也不能动它。
        let pb = Private::new();
        put(&pb.0, "旧内容", &[]);
        let before = pb.0.changeCount();
        let got = copy_selection(&pb.0, || Ok(()), Duration::from_millis(50));
        assert_eq!(got, None);
        assert_eq!(pb.0.changeCount(), before, "剪贴板不应被改写");
    }

    #[test]
    fn empty_clipboard_is_restored_empty() {
        // 用户剪贴板本来是空的：取词后也要是空的，不能留下别人的选中文字。
        let pb = Private::new();
        pb.0.clearContents();
        let got = copy_selection(
            &pb.0,
            || {
                put(&pb.0, "selected", &[]);
                Ok(())
            },
            Duration::from_millis(200),
        );
        assert_eq!(got.as_deref(), Some("selected"));
        assert_eq!(pb.0.stringForType(string_type()), None);
    }

    #[test]
    fn failed_keystroke_reports_nothing() {
        let pb = Private::new();
        put(&pb.0, "旧内容", &[]);
        let got = copy_selection(
            &pb.0,
            || Err(anyhow::anyhow!("no permission")),
            Duration::from_millis(50),
        );
        assert_eq!(got, None);
    }
}
