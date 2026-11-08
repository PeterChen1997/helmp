//! 截图 + OCR。
//!
//! 命令（空查询不返回）：
//! - `screenshot:clipboard`      区域截图到剪贴板
//! - `screenshot:save`           区域截图保存到 `<data_dir>/screenshots/`，并在 Finder 中显示
//! - `screenshot:ocr-screen`     区域截图 → OCR → 文字写剪贴板
//! - `screenshot:ocr-clipboard`  剪贴板图片 → OCR → 文字写剪贴板（不需要屏幕录制权限）
//! - `screenshot:ocr-file:<绝对路径>`  查询 `ocr /绝对路径` 时出现，只展示结果、不写剪贴板

mod capture;
mod ocr;
mod pasteboard;

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result};
use objc2_foundation::{NSDate, NSDateFormatter, NSLocale, NSString};

use crate::matcher;
use crate::paths;
use crate::provider::{Action, Item, Outcome, Provider};

const BADGE: &str = "截图";

struct Cmd {
    key: &'static str,
    title: &'static str,
    subtitle: &'static str,
    keywords: &'static [&'static str],
    run: fn() -> Result<Outcome>,
}

const SCREENSHOT: [&str; 3] = ["截图", "jt", "screenshot"];

const COMMANDS: [Cmd; 4] = [
    Cmd {
        key: "clipboard",
        title: "截图到剪贴板",
        subtitle: "Screenshot to Clipboard",
        keywords: &[
            SCREENSHOT[0],
            SCREENSHOT[1],
            SCREENSHOT[2],
            "截图到剪贴板",
            "screenshot to clipboard",
        ],
        run: screenshot_to_clipboard,
    },
    Cmd {
        key: "save",
        title: "截图并保存",
        subtitle: "Screenshot and Save",
        keywords: &[
            SCREENSHOT[0],
            SCREENSHOT[1],
            SCREENSHOT[2],
            "截图并保存",
            "save screenshot",
        ],
        run: screenshot_and_save,
    },
    Cmd {
        key: "ocr-screen",
        title: "截图识别文字",
        subtitle: "Screenshot OCR — 识别后复制到剪贴板",
        keywords: &[
            SCREENSHOT[0],
            SCREENSHOT[1],
            SCREENSHOT[2],
            "ocr",
            "识别文字",
            "截图识别文字",
            "screenshot ocr",
        ],
        run: ocr_screen,
    },
    Cmd {
        key: "ocr-clipboard",
        title: "识别剪贴板图片中的文字",
        subtitle: "Clipboard Image OCR — 识别后复制到剪贴板",
        keywords: &["ocr", "识别文字", "识别剪贴板图片中的文字", "clipboard ocr"],
        run: ocr_clipboard,
    },
];

pub struct Screenshot;

impl Provider for Screenshot {
    fn id(&self) -> &'static str {
        "screenshot"
    }

    fn query(&self, query: &str) -> Vec<Item> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let mut items: Vec<Item> = COMMANDS
            .iter()
            .filter_map(|cmd| {
                let score = cmd
                    .keywords
                    .iter()
                    .filter_map(|keyword| matcher::score(query, keyword))
                    .max()?;
                Some(Item {
                    id: format!("screenshot:{}", cmd.key),
                    title: cmd.title.into(),
                    subtitle: cmd.subtitle.into(),
                    badge: BADGE.into(),
                    score,
                    action: Action::new(cmd.run),
                    ..Item::blank()
                })
            })
            .collect();
        items.extend(ocr_file_item(query));
        items
    }
}

/// `ocr /绝对路径/图片.png` → 对该文件 OCR 的一项。只解析字符串，不碰文件系统（query 不做 IO）。
fn ocr_file_item(query: &str) -> Option<Item> {
    let query = query.trim();
    let (prefix, rest) = query.split_at_checked(4)?;
    if !prefix.eq_ignore_ascii_case("ocr ") {
        return None;
    }
    let path = rest.trim();
    if !path.starts_with('/') {
        return None;
    }
    let path = PathBuf::from(path);
    Some(Item {
        id: format!("screenshot:ocr-file:{}", path.display()),
        title: "识别图片中的文字".into(),
        subtitle: path.display().to_string(),
        badge: BADGE.into(),
        score: 1000,
        action: Action::new(move || ocr_file(&path)),
        ..Item::blank()
    })
}

fn screenshot_to_clipboard() -> Result<Outcome> {
    if !capture::has_screen_access() {
        return Ok(Outcome::Message(capture::NO_SCREEN_ACCESS.into()));
    }
    capture::to_clipboard()?;
    Ok(Outcome::Done)
}

fn screenshot_and_save() -> Result<Outcome> {
    if !capture::has_screen_access() {
        return Ok(Outcome::Message(capture::NO_SCREEN_ACCESS.into()));
    }
    let dir = paths::data_dir()?.join("screenshots");
    std::fs::create_dir_all(&dir).with_context(|| format!("创建 {}", dir.display()))?;
    let path = dir.join(format!("Helmp 截图 {}.png", timestamp()));
    if capture::to_file(&path)? {
        let status = Command::new("open").arg("-R").arg(&path).status()?;
        anyhow::ensure!(
            status.success(),
            "在 Finder 中显示 {} 失败：{status}",
            path.display()
        );
    }
    Ok(Outcome::Done)
}

fn ocr_screen() -> Result<Outcome> {
    if !capture::has_screen_access() {
        return Ok(Outcome::Message(capture::NO_SCREEN_ACCESS.into()));
    }
    let path = std::env::temp_dir().join(format!(
        "helmp-ocr-{}-{}.png",
        std::process::id(),
        timestamp()
    ));
    let captured = capture::to_file(&path);
    let image = match captured {
        Ok(true) => std::fs::read(&path).with_context(|| format!("读取 {}", path.display())),
        Ok(false) => return Ok(Outcome::Done),
        Err(err) => Err(err),
    };
    // 临时文件无论成败都删；删除失败不影响结果，但要说出来。
    if path.exists()
        && let Err(err) = std::fs::remove_file(&path)
    {
        eprintln!("helmp: 删除临时截图 {} 失败：{err}", path.display());
    }
    copy_recognized(&image?)
}

fn ocr_clipboard() -> Result<Outcome> {
    let image = pasteboard::image_data(&pasteboard::general())
        .context("剪贴板里没有图片（支持 PNG / TIFF）")?;
    copy_recognized(&image)
}

fn ocr_file(path: &Path) -> Result<Outcome> {
    let image = std::fs::read(path).with_context(|| format!("读取 {}", path.display()))?;
    Ok(Outcome::Message(non_empty(ocr::recognize(&image)?)))
}

/// OCR 后把文字写进剪贴板并展示；没识别出文字时不覆盖剪贴板。
fn copy_recognized(image: &[u8]) -> Result<Outcome> {
    let text = ocr::recognize(image)?;
    if !text.is_empty() {
        pasteboard::write_text(&pasteboard::general(), &text)?;
    }
    Ok(Outcome::Message(non_empty(text)))
}

fn non_empty(text: String) -> String {
    if text.is_empty() {
        "没有识别到文字".into()
    } else {
        text
    }
}

/// 本地时间 `YYYY-MM-DD HH.MM.SS`（用 NSDateFormatter，避免为一个时间戳引入 chrono）。
fn timestamp() -> String {
    let formatter = NSDateFormatter::new();
    formatter.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(
        &NSString::from_str("en_US_POSIX"),
    )));
    formatter.setDateFormat(Some(&NSString::from_str("yyyy-MM-dd HH.mm.ss")));
    formatter.stringFromDate(&NSDate::now()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(query: &str) -> Vec<String> {
        Screenshot
            .query(query)
            .into_iter()
            .map(|item| item.id)
            .collect()
    }

    #[test]
    fn empty_query_returns_nothing() {
        // 截图命令不该霸占空查询的结果列表（那里留给最近的 agent 会话等）。
        assert!(ids("").is_empty());
        assert!(ids("   ").is_empty());
    }

    #[test]
    fn short_and_english_aliases_find_capture_commands() {
        // 用户最常用「jt」「截图」「screenshot」唤起，三个截图命令都要出来。
        for query in ["截图", "jt", "screenshot"] {
            let found = ids(query);
            for id in [
                "screenshot:clipboard",
                "screenshot:save",
                "screenshot:ocr-screen",
            ] {
                assert!(found.contains(&id.to_string()), "{query:?} 没找到 {id}");
            }
        }
        let ocr = ids("识别文字");
        assert!(ocr.contains(&"screenshot:ocr-screen".to_string()));
        assert!(ocr.contains(&"screenshot:ocr-clipboard".to_string()));
    }

    #[test]
    fn ocr_with_absolute_path_offers_file_item() {
        // `helmp run screenshot:ocr-file:<路径> "ocr <路径>"` 依赖这个 id 格式做命令行测试。
        let found = ids("OCR /tmp/a b.png");
        assert!(found.contains(&"screenshot:ocr-file:/tmp/a b.png".to_string()));
        // 相对路径不认，避免依赖当前工作目录。
        assert!(
            !ids("ocr a.png")
                .iter()
                .any(|id| id.starts_with("screenshot:ocr-file:"))
        );
    }

    #[test]
    fn ocr_file_action_returns_recognized_text() {
        let query = format!("ocr {}", ocr::tests::FIXTURE);
        let item = Screenshot
            .query(&query)
            .into_iter()
            .find(|item| item.id.starts_with("screenshot:ocr-file:"))
            .expect("ocr-file 项");
        let Outcome::Message(text) = item.action.run().unwrap() else {
            panic!("应返回识别文字");
        };
        ocr::tests::assert_fixture_text(&text);
    }

    #[test]
    fn ocr_file_missing_file_fails_loudly() {
        // 路径打错要报错，不能显示「没有识别到文字」误导用户。
        assert!(ocr_file(Path::new("/nonexistent/helmp-ocr.png")).is_err());
    }

    #[test]
    fn clipboard_image_path_recognizes_text() {
        // 「识别剪贴板图片」是不需要屏幕录制权限的主路径：取图 → OCR 必须端到端可用。
        // 用私有 pasteboard，完全不碰用户真实剪贴板。
        let pb = pasteboard::tests::Private::new();
        pb.put_png(&std::fs::read(ocr::tests::FIXTURE).unwrap());
        let image = pasteboard::image_data(&pb.0).expect("应读到 PNG");
        ocr::tests::assert_fixture_text(&ocr::recognize(&image).unwrap());
    }

    #[test]
    fn timestamp_is_filename_safe() {
        // 文件名里不能有冒号（Finder 显示成斜杠），格式固定 `YYYY-MM-DD HH.MM.SS`。
        let ts = timestamp();
        assert_eq!(ts.len(), 19, "{ts}");
        assert!(!ts.contains(':') && !ts.contains('/'), "{ts}");
    }
}
