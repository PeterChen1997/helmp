//! 文字识别：Apple Vision `VNRecognizeTextRequest`（accurate，按配置的语言顺序，语言纠正），
//! 同一次处理里再跑 `VNDetectBarcodesRequest` 读出二维码 / 条码里的内容（如链接）。
//!
//! 输入统一是图片文件的原始字节（PNG / TIFF / JPEG 等 CIImage 能解码的格式），
//! 这样「截图文件」「剪贴板图片」「指定图片」三条路径共用一个入口。不需要任何系统权限。

use anyhow::{Context as _, Result};

use crate::i18n::tr;
use objc2::AnyThread as _;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_vision::{
    VNDetectBarcodesRequest, VNImageRequestHandler, VNRecognizeTextRequest, VNRequest,
    VNRequestTextRecognitionLevel,
};

/// 配置里没写语言时的识别优先级：简体中文在前（主要场景），英文其次。
pub const DEFAULT_LANGUAGES: [&str; 2] = ["zh-Hans", "en-US"];

/// 一次识别的结果。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Recognition {
    /// 识别出的文字：一行一段，段落之间空一行（按行距判断）。没有文字时为空串。
    pub text: String,
    /// 二维码 / 条码里的内容，按出现顺序去重。
    pub codes: Vec<String>,
}

/// 一行文字和它在图里的竖直位置（归一化坐标，原点在左下角，所以上面的行 y 更大）。
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub bottom: f64,
    pub height: f64,
}

/// 把 Vision 按阅读顺序给出的行拼起来：相邻两行之间的空白超过较矮那行高度的 0.75 倍，就当作换段，多空一行。
pub fn join_lines(lines: &[Line]) -> String {
    let mut out = String::new();
    let mut previous: Option<&Line> = None;
    for line in lines.iter().filter(|line| !line.text.trim().is_empty()) {
        if let Some(prev) = previous {
            let gap = prev.bottom - (line.bottom + line.height);
            let paragraph = gap > prev.height.min(line.height) * 0.75;
            out.push_str(if paragraph { "\n\n" } else { "\n" });
        }
        out.push_str(line.text.trim_end());
        previous = Some(line);
    }
    out
}

/// 识别图片字节中的文字和二维码。`languages` 为空时用 `DEFAULT_LANGUAGES`。
/// 没有文字时 `text` 为空串；图片无法解码或 Vision 报错时返回 Err。
/// Vision 同时跑多个识别请求时偶尔报 CRImageReaderError（并行测试里能稳定复现）。
/// 识别本来就是用户一次触发一次，串行化没有代价。
static VISION: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn recognize(image: &[u8], languages: &[String]) -> Result<Recognition> {
    anyhow::ensure!(!image.is_empty(), tr("图片数据为空", "Image data is empty"));
    let _serial = VISION.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
        // SAFETY: 无参数构造；默认识别全部支持的码制（含二维码）。
        let barcodes = unsafe { VNDetectBarcodesRequest::new() };

        let as_requests: [&VNRequest; 2] = [&request, &barcodes];
        handler
            .performRequests_error(&NSArray::from_slice(&as_requests))
            .map_err(|err| anyhow::anyhow!("{}", err.localizedDescription()))
            .with_context(|| {
                tr(
                    "Vision 文字识别失败（图片可能无法解码）",
                    "Vision text recognition failed (the image may not be decodable)",
                )
            })?;

        let lines: Vec<Line> = request
            .results()
            .map(|observations| {
                observations
                    .iter()
                    .filter_map(|observation| {
                        let text = observation.topCandidates(1).firstObject()?;
                        // SAFETY: Vision 填好的归一化矩形，只读。
                        let rect = unsafe { observation.boundingBox() };
                        Some(Line {
                            text: text.string().to_string(),
                            bottom: rect.origin.y,
                            height: rect.size.height,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut codes: Vec<String> = Vec::new();
        // SAFETY: 请求执行完后读取结果数组，只读。
        if let Some(observations) = unsafe { barcodes.results() } {
            for observation in observations.iter() {
                // SAFETY: 同上。
                if let Some(payload) = unsafe { observation.payloadStringValue() } {
                    let payload = payload.to_string();
                    if !payload.trim().is_empty() && !codes.contains(&payload) {
                        codes.push(payload);
                    }
                }
            }
        }
        Ok(Recognition {
            text: join_lines(&lines),
            codes,
        })
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
            .with_context(|| tr("图片无法解码", "Couldn't decode the image"))?;
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
            .with_context(|| tr("PNG 编码失败", "Couldn't encode the image as PNG"))?
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
pub(crate) mod tests {
    use super::*;

    /// 由 `tests/fixtures/make-ocr-fixture.swift` 生成：「你好 Huan 2026」+ 一行英文句子。
    pub const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ocr-zh-en.png");

    /// 由 `tests/fixtures/make-qr-fixture.swift` 生成：指向 https://example.com/helmp 的二维码 + 「扫码打开 Helmp」。
    pub const QR_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ocr-qr.png");

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
        let found = recognize(&image, &[]).expect("OCR");
        assert_fixture_text(&found.text);
        assert!(found.codes.is_empty(), "没有二维码的图不该报出码：{:?}", found.codes);
        let configured = ["zh-Hans".to_string(), "en-US".to_string()];
        assert_fixture_text(&recognize(&image, &configured).expect("OCR").text);
    }

    #[test]
    fn extra_languages_do_not_break_chinese_and_english() {
        // 设置里加了日文 / 韩文 / 德文等识别语言后，原来的中英文识别不能坏（Vision 对某些组合会直接报错）。
        let image = std::fs::read(FIXTURE).expect("读取 fixture");
        for extra in ["ja-JP", "ko-KR", "de-DE", "fr-FR"] {
            let languages = ["zh-Hans".to_string(), "en-US".to_string(), extra.to_string()];
            let found = recognize(&image, &languages)
                .unwrap_or_else(|err| panic!("加上 {extra} 后识别失败：{err:#}"));
            assert_fixture_text(&found.text);
        }
    }

    #[test]
    fn reads_qr_code_links() {
        // 截图里有二维码时要读出链接，才能给「打开链接」；同一张图里的文字照常识别。
        let image = std::fs::read(QR_FIXTURE).expect("读取 fixture");
        let found = recognize(&image, &[]).expect("OCR");
        assert_eq!(found.codes, ["https://example.com/helmp"]);
        assert!(found.text.contains("扫码打开"), "{:?}", found.text);
    }

    #[test]
    fn keeps_line_breaks_and_separates_paragraphs() {
        // 识别结果要保留原来的换行；行距明显变大的地方是换段，多空一行，复制出去才像原文。
        let line = |text: &str, bottom: f64| Line {
            text: text.into(),
            bottom,
            height: 0.05,
        };
        let lines = [
            line("第一段第一行", 0.90),
            line("第一段第二行", 0.84),
            line("第二段", 0.60),
            line("  ", 0.50),
            line("第二段第二行", 0.54),
        ];
        assert_eq!(
            join_lines(&lines),
            "第一段第一行\n第一段第二行\n\n第二段\n第二段第二行"
        );
        assert_eq!(join_lines(&[]), "");
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
