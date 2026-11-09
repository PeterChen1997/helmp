//! 真实的 NSPasteboard 读写、前台 App 查询、模拟 ⌘V。
//!
//! 每次调用都在独立的 autoreleasepool 里取 `generalPasteboard`，所以可以在后台轮询线程里用
//! （NSPasteboard / NSWorkspace 在 objc2-app-kit 里都不是 MainThreadOnly）。

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail, ensure};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSBitmapImageRep, NSPasteboard, NSPasteboardType, NSPasteboardTypeFileURL, NSPasteboardTypePNG,
    NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardWriting, NSWorkspace,
};
use objc2_foundation::{NSArray, NSData, NSString, NSURL};

use super::store::{Entry, ImageFormat, Kind, Source};
use super::watcher::{Image, Pasteboard, TRANSIENT_TYPE};
use crate::i18n::{tr, trf};

pub struct SystemPasteboard;

impl Pasteboard for SystemPasteboard {
    fn change_count(&self) -> isize {
        autoreleasepool(|_| NSPasteboard::generalPasteboard().changeCount())
    }

    fn types(&self) -> Vec<String> {
        autoreleasepool(|_| {
            NSPasteboard::generalPasteboard()
                .types()
                .map(|types| types.iter().map(|ty| ty.to_string()).collect())
                .unwrap_or_default()
        })
    }

    fn file_paths(&self) -> Vec<PathBuf> {
        autoreleasepool(|_| {
            let Some(items) = NSPasteboard::generalPasteboard().pasteboardItems() else {
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
            NSPasteboard::generalPasteboard()
                .stringForType(string_type())
                .map(|text| text.to_string())
        })
    }

    fn image(&self) -> Option<Image> {
        autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            [
                (png_type(), ImageFormat::Png),
                (tiff_type(), ImageFormat::Tiff),
            ]
            .into_iter()
            .find_map(|(ty, format)| {
                let data = pasteboard.dataForType(ty)?;
                let (width, height) = NSBitmapImageRep::imageRepWithData(&data)
                    .map(|rep| (pixels(rep.pixelsWide()), pixels(rep.pixelsHigh())))
                    .unwrap_or((0, 0));
                Some(Image {
                    bytes: data.to_vec(),
                    format,
                    width,
                    height,
                })
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
        let pasteboard = NSPasteboard::generalPasteboard();
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

/// 发送 ⌘ + 某个键（按下、抬起）。修饰键只带 ⌘：用户此刻可能还按着唤起快捷键的 ⌥⇧。
fn send_command_key(keycode: u16) -> Result<()> {
    const COMBINED_SESSION_STATE: i32 = 0;
    const HID_EVENT_TAP: u32 = 0;
    const FLAG_COMMAND: u64 = 0x0010_0000;

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
            CGEventSetFlags(event, FLAG_COMMAND);
            CGEventPost(HID_EVENT_TAP, event);
            CFRelease(event);
        }
        CFRelease(source);
        result
    }
}
