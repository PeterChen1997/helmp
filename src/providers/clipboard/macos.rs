//! 真实的 NSPasteboard 读写、前台 App 查询、模拟 ⌘V。
//!
//! 每次调用都在独立的 autoreleasepool 里取 `generalPasteboard`，所以可以在后台轮询线程里用
//! （NSPasteboard / NSWorkspace 在 objc2-app-kit 里都不是 MainThreadOnly）。

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail, ensure};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSBitmapImageRep, NSPasteboard, NSPasteboardType, NSPasteboardTypeFileURL, NSPasteboardTypePNG,
    NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardWriting, NSWorkspace,
};
use objc2_foundation::{NSArray, NSData, NSString, NSURL};

use super::store::{Entry, ImageFormat, Kind, MAX_BYTES, Source};
use super::watcher::{Image, Pasteboard, TRANSIENT_TYPE};
use crate::i18n::{tr, trf};

/// 换掉系统剪贴板：`HELMP_PASTEBOARD=<名字>` 时剪贴板历史改读写这个名字的私有 pasteboard。
/// 端到端测试用——真按 ⌘V 走完「收起面板 → 焦点还回去 → 粘贴」整条链路，又不碰用户的系统剪贴板。
const PASTEBOARD_ENV: &str = "HELMP_PASTEBOARD";

/// 剪贴板历史读写的 pasteboard：平时是 `generalPasteboard`，见 [`PASTEBOARD_ENV`]。
fn pasteboard() -> Retained<NSPasteboard> {
    match std::env::var(PASTEBOARD_ENV) {
        Ok(name) if !name.is_empty() => {
            NSPasteboard::pasteboardWithName(&NSString::from_str(&name))
        }
        _ => NSPasteboard::generalPasteboard(),
    }
}

pub struct SystemPasteboard;

impl Pasteboard for SystemPasteboard {
    fn change_count(&self) -> isize {
        autoreleasepool(|_| pasteboard().changeCount())
    }

    fn types(&self) -> Vec<String> {
        autoreleasepool(|_| {
            pasteboard()
                .types()
                .map(|types| types.iter().map(|ty| ty.to_string()).collect())
                .unwrap_or_default()
        })
    }

    fn file_paths(&self) -> Vec<PathBuf> {
        autoreleasepool(|_| {
            let Some(items) = pasteboard().pasteboardItems() else {
                return Vec::new();
            };
            items
                .iter()
                .filter_map(|item| item.stringForType(file_url_type()))
                .filter_map(|url| NSURL::URLWithString(&url))
                .filter_map(|url| url.path())
                .map(|path| PathBuf::from(path.to_string()))
                .collect()
        })
    }

    fn text(&self) -> Option<String> {
        autoreleasepool(|_| {
            pasteboard()
                .stringForType(string_type())
                .map(|text| text.to_string())
        })
    }

    fn image(&self) -> Option<Result<Image, u64>> {
        autoreleasepool(|_| {
            let pasteboard = pasteboard();
            [
                (png_type(), ImageFormat::Png),
                (tiff_type(), ImageFormat::Tiff),
            ]
            .into_iter()
            .find_map(|(ty, format)| {
                let data = pasteboard.dataForType(ty)?;
                let bytes = data.length() as u64;
                if bytes > MAX_BYTES {
                    return Some(Err(bytes));
                }
                let (width, height) = NSBitmapImageRep::imageRepWithData(&data)
                    .map(|rep| (pixels(rep.pixelsWide()), pixels(rep.pixelsHigh())))
                    .unwrap_or((0, 0));
                Some(Ok(Image {
                    bytes: data.to_vec(),
                    format,
                    width,
                    height,
                }))
            })
        })
    }
}

fn pixels(value: isize) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

/// 当前前台 App。仅在主线程 runloop 运转时（图形界面模式）才会随切换更新。
pub fn frontmost_app() -> Option<Source> {
    autoreleasepool(|_| {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        let name = app.localizedName()?.to_string();
        Some(Source {
            name,
            bundle_id: app.bundleIdentifier().map(|id| id.to_string()),
        })
    })
}

/// 把历史条目写回系统剪贴板，并打上 TransientType 标记。
pub fn write(entry: &Entry, image_path: &Path) -> Result<()> {
    // 图片在进 autoreleasepool 前读好，IO 错误直接带路径上报。
    let image = match entry.kind {
        Kind::Image => Some((
            std::fs::read(image_path).with_context(|| {
                trf!(
                    "读取剪贴板图片 {}",
                    "Couldn’t read clipboard image {}",
                    image_path.display()
                )
            })?,
            ImageFormat::from_file_name(&entry.content)?,
        )),
        _ => None,
    };
    autoreleasepool(|_| {
        let pasteboard = pasteboard();
        pasteboard.clearContents();
        let written = match (entry.kind, &image) {
            (Kind::Text, _) => {
                pasteboard.setString_forType(&NSString::from_str(&entry.content), string_type())
            }
            (Kind::Image, Some((bytes, format))) => {
                let ty = match format {
                    ImageFormat::Png => png_type(),
                    ImageFormat::Tiff => tiff_type(),
                };
                pasteboard.setData_forType(Some(&NSData::with_bytes(bytes)), ty)
            }
            (Kind::Image, None) => bail!(tr(
                "图片条目没有读到图片数据",
                "The image item has no image data"
            )),
            (Kind::Files, _) => {
                let urls: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = entry
                    .content
                    .lines()
                    .map(|path| {
                        ProtocolObject::from_retained(NSURL::fileURLWithPath(&NSString::from_str(
                            path,
                        )))
                    })
                    .collect();
                ensure!(
                    !urls.is_empty(),
                    tr("文件条目没有路径", "The file item has no paths")
                );
                pasteboard.writeObjects(&NSArray::from_retained_slice(&urls))
            }
        };
        ensure!(
            written,
            trf!(
                "写入系统剪贴板失败（第 {} 条）",
                "Couldn’t write item #{} to the clipboard",
                entry.id
            )
        );
        let marked = pasteboard
            .setString_forType(&NSString::from_str(""), &NSString::from_str(TRANSIENT_TYPE));
        ensure!(
            marked,
            tr(
                "给剪贴板内容打 TransientType 标记失败",
                "Couldn’t mark the clipboard content as transient"
            )
        );
        Ok(())
    })
}

/// 把一段纯文本写进系统剪贴板，打上 TransientType 标记（剪贴板历史不记录它）。片段粘贴用。
pub fn write_text(text: &str) -> Result<()> {
    autoreleasepool(|_| {
        let pasteboard = pasteboard();
        pasteboard.clearContents();
        ensure!(
            pasteboard.setString_forType(&NSString::from_str(text), string_type()),
            tr("写入系统剪贴板失败", "Couldn’t write to the clipboard")
        );
        ensure!(
            pasteboard
                .setString_forType(&NSString::from_str(""), &NSString::from_str(TRANSIENT_TYPE)),
            tr(
                "给剪贴板内容打 TransientType 标记失败",
                "Couldn’t mark the clipboard content as transient"
            )
        );
        Ok(())
    })
}

fn string_type() -> &'static NSPasteboardType {
    // SAFETY: AppKit 导出的常量，进程生命周期内有效。
    unsafe { NSPasteboardTypeString }
}

fn file_url_type() -> &'static NSPasteboardType {
    // SAFETY: 同上。
    unsafe { NSPasteboardTypeFileURL }
}

fn png_type() -> &'static NSPasteboardType {
    // SAFETY: 同上。
    unsafe { NSPasteboardTypePNG }
}

fn tiff_type() -> &'static NSPasteboardType {
    // SAFETY: 同上。
    unsafe { NSPasteboardTypeTIFF }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceCreate(state_id: i32) -> *mut c_void;
    fn CGEventCreateKeyboardEvent(source: *mut c_void, keycode: u16, key_down: bool)
    -> *mut c_void;
    fn CGEventSetFlags(event: *mut c_void, flags: u64);
    fn CGEventPost(tap: u32, event: *mut c_void);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// 是否已有辅助功能权限。只查询，不弹授权框（不用 `AXIsProcessTrustedWithOptions` 的 prompt 选项）。
pub fn accessibility_trusted() -> bool {
    // SAFETY: 无参数的只读查询。
    unsafe { AXIsProcessTrusted() }
}

/// 向前台 App 发送 ⌘V。调用方需先确认 `accessibility_trusted()`，否则事件会被系统静默丢弃。
pub fn send_paste() -> Result<()> {
    const KEYCODE_V: u16 = 9;
    send_command_key(KEYCODE_V)
}

/// 向前台 App 发送 ⌘C（翻译取词的后备方式）。同样需要辅助功能权限。
pub fn send_copy() -> Result<()> {
    const KEYCODE_C: u16 = 8;
    send_command_key(KEYCODE_C)
}

/// 向前台 App 发送 `count` 次 ←（片段 `{cursor}` 把光标移回占位处）。同样需要辅助功能权限。
pub fn send_left_arrows(count: usize) -> Result<()> {
    const KEYCODE_LEFT: u16 = 123;
    for _ in 0..count {
        send_key(KEYCODE_LEFT, 0)?;
    }
    Ok(())
}

const FLAG_COMMAND: u64 = 0x0010_0000;

/// 发送 ⌘ + 某个键（按下、抬起）。修饰键只带 ⌘：用户此刻可能还按着唤起快捷键的 ⌥⇧。
fn send_command_key(keycode: u16) -> Result<()> {
    send_key(keycode, FLAG_COMMAND)
}

/// 等焦点回到别的 App 最多等多久。
const FOCUS_TIMEOUT: Duration = Duration::from_millis(1500);
/// 前台刚换过去时再等一会儿：系统先把前台记到对方名下，对方的窗口成为 key window 还要处理完激活事件。
const FOCUS_SETTLE: Duration = Duration::from_millis(60);

/// 发键前看一眼前台：该发、再等等，还是放弃。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    Send,
    Wait,
    GiveUp,
}

/// 面板唤起时会激活 Helmp，收起时才把前台还回去（异步）。前台还是 Helmp（或暂时没有前台 App）时发出的
/// ⌘V 会落回 Helmp 自己、什么都不发生；前台是用户刚点过去的别的 App 时，⌘V 会贴进不相干的地方。
/// 所以：知道面板把焦点还给了谁（`target`）就等到它回到前台才发，不知道时至少等前台不是 Helmp；
/// 等不到就放弃并报错，绝不在超时后照发（以前就是这样悄悄丢掉粘贴的）。
fn gate(front: Option<i32>, me: i32, target: Option<i32>, waited: Duration) -> Gate {
    let ready = match (front, target) {
        (Some(pid), Some(target)) => pid == target,
        (Some(pid), None) => pid != me,
        (None, _) => false,
    };
    if ready {
        Gate::Send
    } else if waited >= FOCUS_TIMEOUT {
        Gate::GiveUp
    } else {
        Gate::Wait
    }
}

fn frontmost_pid() -> Option<i32> {
    autoreleasepool(|_| {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|app| app.processIdentifier())
    })
}

/// 等到该收键的 App 在前台（见 [`gate`]），等不到时返回错误：调用方把它显示出来，用户知道没粘贴上。
fn wait_for_target_app() -> Result<()> {
    let me = i32::try_from(std::process::id()).unwrap_or(i32::MAX);
    let target = crate::panel::take_paste_target();
    let started = Instant::now();
    let mut switched = false;
    loop {
        let front = frontmost_pid();
        match gate(front, me, target, started.elapsed()) {
            Gate::Send => {
                if switched {
                    std::thread::sleep(FOCUS_SETTLE);
                }
                crate::trace::log(format!(
                    "发键：前台 pid {front:?}（目标 {target:?}），等了 {} ms",
                    started.elapsed().as_millis()
                ));
                return Ok(());
            }
            Gate::Wait => {
                switched = true;
                std::thread::sleep(Duration::from_millis(10));
            }
            Gate::GiveUp => bail!(trf!(
                "等了 {} 秒，焦点还没回到原来的 App，没有发送按键（内容已在剪贴板，可以手动 ⌘V）",
                "Focus didn’t return to your app within {} s, so no keystroke was sent (the content is on the clipboard; press ⌘V yourself)",
                FOCUS_TIMEOUT.as_secs_f32()
            )),
        }
    }
}

/// 发送一个键（按下、抬起），修饰键固定为 `flags`（不继承用户此刻按着的修饰键）。
fn send_key(keycode: u16, flags: u64) -> Result<()> {
    wait_for_target_app()?;
    post_key_now(keycode, flags)
}

/// 立即从 HID 层投递一个键（按下、抬起），不等焦点。图形自检测输入法时用（此时前台就是 Helmp）。
pub fn post_key_now(keycode: u16, flags: u64) -> Result<()> {
    const COMBINED_SESSION_STATE: i32 = 0;
    const HID_EVENT_TAP: u32 = 0;

    // SAFETY: 标准 CoreGraphics 调用；每个 Create 得到的对象都在本函数内 CFRelease。
    unsafe {
        let source = CGEventSourceCreate(COMBINED_SESSION_STATE);
        if source.is_null() {
            bail!(tr("CGEventSourceCreate 失败", "CGEventSourceCreate failed"));
        }
        let mut result = Ok(());
        for key_down in [true, false] {
            let event = CGEventCreateKeyboardEvent(source, keycode, key_down);
            if event.is_null() {
                result = Err(anyhow::anyhow!(tr(
                    "CGEventCreateKeyboardEvent 失败",
                    "CGEventCreateKeyboardEvent failed"
                )));
                break;
            }
            CGEventSetFlags(event, flags);
            CGEventPost(HID_EVENT_TAP, event);
            CFRelease(event);
        }
        CFRelease(source);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: i32 = 100;

    const TARGET: i32 = 42;
    const OTHER: i32 = 77;
    const SOON: Duration = Duration::from_millis(200);

    #[test]
    fn waits_for_the_app_the_panel_returned_focus_to() {
        // 从面板里粘贴：⌘V 必须落到唤起面板前的那个 App。
        assert_eq!(gate(Some(TARGET), ME, Some(TARGET), Duration::ZERO), Gate::Send);
        // 前台还是 Helmp（面板刚收起、激活还没切过去）或暂时没有前台 App：等。
        assert_eq!(gate(Some(ME), ME, Some(TARGET), SOON), Gate::Wait);
        assert_eq!(gate(None, ME, Some(TARGET), SOON), Gate::Wait);
        // 用户这时点进了别的 App：不能把 ⌘V 贴进去，等原来的 App 回来，等不到就放弃。
        assert_eq!(gate(Some(OTHER), ME, Some(TARGET), SOON), Gate::Wait);
        assert_eq!(gate(Some(OTHER), ME, Some(TARGET), FOCUS_TIMEOUT), Gate::GiveUp);
    }

    #[test]
    fn without_a_known_target_only_avoids_helmp_itself() {
        // 不是从面板里执行的（全局快捷键取词）：前台就是用户正在用的 App，不是 Helmp 就发。
        assert_eq!(gate(Some(OTHER), ME, None, Duration::ZERO), Gate::Send);
        assert_eq!(gate(Some(ME), ME, None, SOON), Gate::Wait);
    }

    #[test]
    fn gives_up_loudly_instead_of_pasting_into_helmp() {
        // 超时后照发等于把 ⌘V 发给 Helmp 自己：用户只看到剪贴板变了、却没粘贴上。必须放弃并报错。
        assert_eq!(gate(Some(ME), ME, None, FOCUS_TIMEOUT), Gate::GiveUp);
        assert_eq!(gate(Some(ME), ME, Some(TARGET), FOCUS_TIMEOUT), Gate::GiveUp);
        assert_eq!(gate(None, ME, None, FOCUS_TIMEOUT), Gate::GiveUp);
    }
}
