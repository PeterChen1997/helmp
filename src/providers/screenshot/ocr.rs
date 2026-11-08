//! 文字识别：Apple Vision `VNRecognizeTextRequest`（accurate，按配置的语言顺序，语言纠正）。
//!
//! 输入统一是图片文件的原始字节（PNG / TIFF / JPEG 等 CIImage 能解码的格式），
//! 这样「截图文件」「剪贴板图片」「指定图片」三条路径共用一个入口。不需要任何系统权限。

use anyhow::{Context as _, Result};
use objc2::AnyThread as _;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

/// 配置里没写语言时的识别优先级：简体中文在前（主要场景），英文其次。
pub const DEFAULT_LANGUAGES: [&str; 2] = ["zh-Hans", "en-US"];

/// 识别图片字节中的文字，按 Vision 返回的顺序（大致自上而下）每行一段，用换行连接。
/// `languages` 为空时用 `DEFAULT_LANGUAGES`。
/// 没有文字时返回空字符串；图片无法解码或 Vision 报错时返回 Err。
pub fn recognize(image: &[u8], languages: &[String]) -> Result<String> {
    anyhow::ensure!(!image.is_empty(), "图片数据为空");
    autoreleasepool(|_| {
        let data = NSData::with_bytes(image);
        let handler = VNImageRequestHandler::initWithData_options(
            VNImageRequestHandler::alloc(),
            &data,
            &NSDictionary::new(),
        );

        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        let languages: Vec<_> = if languages.is_empty() {
            DEFAULT_LANGUAGES
                .iter()
                .map(|l| NSString::from_str(l))
                .collect()
        } else {
            languages.iter().map(|l| NSString::from_str(l)).collect()
        };
        request.setRecognitionLanguages(&NSArray::from_retained_slice(&languages));
        request.setUsesLanguageCorrection(true);

        let as_request: &VNRequest = &request;
        handler
            .performRequests_error(&NSArray::from_slice(&[as_request]))
            .map_err(|err| anyhow::anyhow!("{}", err.localizedDescription()))
            .context("Vision 文字识别失败（图片可能无法解码）")?;

        let lines: Vec<String> = request
            .results()
            .map(|observations| {
                observations
                    .iter()
                    .filter_map(|observation| observation.topCandidates(1).firstObject())
                    .map(|text| text.string().to_string())
                    .collect()
            })
            .unwrap_or_default();
        Ok(lines.join("\n"))
    })
}

/// 详情区要展示的图片：统一成 PNG（TIFF 等格式界面不一定画得出来），附像素尺寸。
pub struct Png {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn to_png(image: &[u8]) -> Result<Png> {
    autoreleasepool(|_| {
        let rep = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(image))
            .context("图片无法解码")?;
        let width = u32::try_from(rep.pixelsWide()).unwrap_or(0);
        let height = u32::try_from(rep.pixelsHigh()).unwrap_or(0);
        let bytes = if image.starts_with(b"\x89PNG\r\n\x1a\n") {
            image.to_vec()
        } else {
            // SAFETY: properties 传空字典即可。
            unsafe {
                rep.representationUsingType_properties(
                    NSBitmapImageFileType::PNG,
                    &NSDictionary::new(),
                )
            }
            .context("PNG 编码失败")?
            .to_vec()
        };
        Ok(Png {
            bytes,
            width,
            height,
        })
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// 由 `tests/fixtures/make-ocr-fixture.swift` 生成：「你好 Huan 2026」+ 一行英文句子。
    pub const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ocr-zh-en.png");

    pub fn assert_fixture_text(text: &str) {
        for expected in ["你好", "Huan", "2026", "quick brown fox"] {
            assert!(
                text.contains(expected),
                "OCR 结果缺少 {expected:?}：{text:?}"
            );
        }
    }

    #[test]
    fn recognizes_chinese_and_english_in_fixture() {
        // 中英混排识别是这个模块存在的理由（国内用户截图里几乎都是中文夹英文/数字）：
        // 语言列表、识别级别或结果拼接任何一处配错，中文就会丢或变成乱码，这里必须挂。
        // 空列表（配置里删光了）也要回落到默认语言，而不是交给 Vision 的英文默认值。
        let image = std::fs::read(FIXTURE).expect("读取 fixture");
        assert_fixture_text(&recognize(&image, &[]).expect("OCR"));
        let configured = ["zh-Hans".to_string(), "en-US".to_string()];
        assert_fixture_text(&recognize(&image, &configured).expect("OCR"));
    }

    #[test]
    fn undecodable_image_fails_loudly() {
        // 剪贴板里的数据可能不是图片；这时必须报错，而不是返回空串让用户以为「图里没字」。
        assert!(recognize(b"definitely not an image", &[]).is_err());
        assert!(recognize(b"", &[]).is_err());
        assert!(to_png(b"definitely not an image").is_err());
    }

    #[test]
    fn tiff_is_converted_to_png_with_size() {
        // 剪贴板图片常是 TIFF（预览、AppKit 应用）；详情区要能画出来，统一转 PNG，尺寸不能变。
        let png = std::fs::read(FIXTURE).unwrap();
        let original = to_png(&png).unwrap();
        assert_eq!(original.bytes, png, "PNG 输入不应重新编码");
        assert!(original.width > 0 && original.height > 0);
        let tiff = autoreleasepool(|_| {
            NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(&png))
                .unwrap()
                .TIFFRepresentation()
                .unwrap()
                .to_vec()
        });
        let converted = to_png(&tiff).unwrap();
        assert!(converted.bytes.starts_with(b"\x89PNG"));
        assert_eq!(
            (converted.width, converted.height),
            (original.width, original.height)
        );
    }
}
