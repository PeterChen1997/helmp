//! 文字识别：Apple Vision `VNRecognizeTextRequest`（accurate，zh-Hans + en-US，语言纠正）。
//!
//! 输入统一是图片文件的原始字节（PNG / TIFF / JPEG 等 CIImage 能解码的格式），
//! 这样「截图文件」「剪贴板图片」「指定图片」三条路径共用一个入口。不需要任何系统权限。

use anyhow::{Context as _, Result};
use objc2::AnyThread as _;
use objc2::rc::autoreleasepool;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

/// 识别优先级：简体中文在前（主要场景），英文其次。
const LANGUAGES: [&str; 2] = ["zh-Hans", "en-US"];

/// 识别图片字节中的文字，按 Vision 返回的顺序（大致自上而下）每行一段，用换行连接。
/// 没有文字时返回空字符串；图片无法解码或 Vision 报错时返回 Err。
pub fn recognize(image: &[u8]) -> Result<String> {
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
        let languages: Vec<_> = LANGUAGES.iter().map(|l| NSString::from_str(l)).collect();
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
        let image = std::fs::read(FIXTURE).expect("读取 fixture");
        assert_fixture_text(&recognize(&image).expect("OCR"));
    }

    #[test]
    fn undecodable_image_fails_loudly() {
        // 剪贴板里的数据可能不是图片；这时必须报错，而不是返回空串让用户以为「图里没字」。
        assert!(recognize(b"definitely not an image").is_err());
        assert!(recognize(b"").is_err());
    }
}
