//! NSPasteboard 读写：取图片字节、写入识别出的文字。
//!
//! 函数都接收 pasteboard 参数而不是写死 generalPasteboard，测试用私有 pasteboard，
//! 不碰用户真实剪贴板。

use anyhow::Result;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF,
};
use objc2_foundation::NSString;

pub fn general() -> Retained<NSPasteboard> {
    NSPasteboard::generalPasteboard()
}

/// 剪贴板里的图片原始字节，优先 PNG（screencapture -c、浏览器复制图片），其次 TIFF（预览等 AppKit 应用）。
pub fn image_data(pasteboard: &NSPasteboard) -> Option<Vec<u8>> {
    // SAFETY: AppKit 导出的常量 NSString，进程内始终有效。
    let types = unsafe { [NSPasteboardTypePNG, NSPasteboardTypeTIFF] };
    types
        .into_iter()
        .find_map(|ty| pasteboard.dataForType(ty))
        .map(|data| data.to_vec())
}

/// 清空并写入纯文本。
pub fn write_text(pasteboard: &NSPasteboard, text: &str) -> Result<()> {
    pasteboard.clearContents();
    // SAFETY: 同上，AppKit 导出的常量。
    let ty = unsafe { NSPasteboardTypeString };
    anyhow::ensure!(
        pasteboard.setString_forType(&NSString::from_str(text), ty),
        "写入剪贴板失败"
    );
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use objc2_foundation::NSData;

    /// 私有 pasteboard，drop 时从 pasteboard server 注销，避免泄漏。
    pub struct Private(pub Retained<NSPasteboard>);

    impl Private {
        pub fn new() -> Self {
            Self(NSPasteboard::pasteboardWithUniqueName())
        }

        pub fn put_png(&self, png: &[u8]) {
            self.0.clearContents();
            // SAFETY: AppKit 导出的常量。
            let ty = unsafe { NSPasteboardTypePNG };
            assert!(self.0.setData_forType(Some(&NSData::with_bytes(png)), ty));
        }
    }

    impl Drop for Private {
        fn drop(&mut self) {
            // objc2-app-kit 0.3.2 没有绑定 releaseGlobally，直接发消息。
            // SAFETY: `-[NSPasteboard releaseGlobally]` 无参数无返回值。
            unsafe { objc2::msg_send![&*self.0, releaseGlobally] }
        }
    }

    #[test]
    fn write_text_replaces_previous_content() {
        // OCR 结果写进剪贴板后用户直接 ⌘V；旧的图片类型如果残留，粘贴到富文本应用会贴出图片而不是文字。
        let pb = Private::new();
        pb.put_png(b"old");
        write_text(&pb.0, "识别结果 text").unwrap();
        assert_eq!(image_data(&pb.0), None);
        // SAFETY: AppKit 导出的常量。
        let ty = unsafe { NSPasteboardTypeString };
        assert_eq!(
            pb.0.stringForType(ty).map(|s| s.to_string()).as_deref(),
            Some("识别结果 text")
        );
    }
}
