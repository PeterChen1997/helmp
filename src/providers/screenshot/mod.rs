//! 截图 + OCR。
//!
//! 命令（空查询不返回；首页「快捷命令」里放「截图识别文字」）：
//! - `screenshot:clipboard`      「截图」：冻结屏幕后框选（见 `crate::capture`），回车 / 双击复制，
//!   工具栏可保存、钉图、识别文字。id 沿用旧的「截图到剪贴板」，最近使用和快捷键都按 id 找它。
//! - `screenshot:save`           同上，但回车是保存（另存为面板，默认目录 `config.screenshot.save_dir`，
//!   缺省 `<data_dir>/screenshots/`）
//! - `screenshot:ocr-screen`     冻结屏幕后框选，松开鼠标立即识别，结果在选区旁的浮窗里显示，文字自动复制
//! - `screenshot:ocr-clipboard`  剪贴板图片 → OCR → 文字写剪贴板（不需要屏幕录制权限）
//! - `screenshot:ocr-file:<绝对路径>`  查询 `ocr /绝对路径` 时出现，只展示结果、不写剪贴板
//!
//! 截图类命令本身只返回 `Outcome::Capture`，由界面线程打开遮罩（它不改动用户环境，所以不过演练开关；
//! 复制、保存这些真正的副作用在截图界面里各自过 `effects`）。没有录屏权限时返回「前往授权」详情。
//!
//! OCR 语言读 `config.screenshot.ocr_languages`。详情里展示的截图存在 `<data_dir>/ocr/`，只保留最近几张。
//!
//! 识别结果（对标 HapiGo 文本识别）：标题「识别到 N 行 · 语言」，保留换行与段落；回车复制全部，
//! ⌘T 翻译，⌘⇧C 复制为单行，⌘⌥C 去掉换行后复制；图里有二维码时列出内容并给「打开链接」。
//! 识别为空时明确说「没有识别到文字」。

mod capture;
/// 截图界面（`crate::capture`）的识别结果窗口也用它。
pub(crate) mod ocr;
/// 脚本插件的「复制输出」、截图界面的复制也用它写剪贴板。
pub(crate) mod pasteboard;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context as _, Result};
use objc2_foundation::{NSDate, NSDateFormatter, NSLocale, NSString};

use crate::engine::commands_section;
use crate::i18n::{tr, trf};
use crate::matcher;
use crate::paths;
use crate::provider::{
    Action, Block, Detail, Icon, Item, NamedAction, Outcome, Provider, Tint, Tone,
};

fn badge() -> &'static str {
    tr("截图", "Screenshot")
}
const TINT: Tint = Tint::Indigo;
/// 首页「快捷命令」里的分数：排在第一（设计稿顺序：截图识别文字、翻译、剪贴板历史）。
const OCR_SUGGESTION_SCORE: u32 = 720;
/// `<data_dir>/ocr/` 里保留的截图张数：详情区要显示最近一次的截图，更早的没用。
const KEEP_OCR_IMAGES: usize = 10;
/// 翻译插件的范围 id（翻译插件由另一个模块提供）。
pub(crate) const TRANSLATE_SCOPE: &str = "translate";

/// 最近一次识别出的文字，供 ⌘K 里的「复制识别结果」「翻译」使用。
static LAST_TEXT: Mutex<Option<String>> = Mutex::new(None);

/// 界面文案（中文, English），使用时按当前语言取。
type Text = (&'static str, &'static str);

fn text(pair: Text) -> &'static str {
    tr(pair.0, pair.1)
}

struct Cmd {
    key: &'static str,
    title: Text,
    subtitle: Text,
    /// 搜索关键词，与界面语言无关：中文名、拼音、英文名都在这里（含两种语言的标题）。
    keywords: &'static [&'static str],
    symbol: &'static str,
    action_title: Text,
    /// OCR 命令带「复制识别结果」「翻译」次级操作。
    ocr: bool,
    /// 执行就会改动用户环境（如写剪贴板），要过演练开关 `effects::dry`。只打开截图界面的命令不算。
    effects: bool,
    run: fn() -> Result<Outcome>,
}

const SCREENSHOT: [&str; 3] = ["截图", "jt", "screenshot"];

const COMMANDS: [Cmd; 4] = [
    // 主「截图」命令。id 沿用旧的「截图到剪贴板」：最近使用、快捷键、菜单栏都按 id 重新找它，
    // 回车（复制）也正是旧命令的行为。旧标题仍在关键词里，按旧名字搜也能找到。
    Cmd {
        key: "clipboard",
        title: ("截图", "Screenshot"),
        subtitle: (
            "冻结屏幕后框选，可复制、保存、钉在屏幕上",
            "Freeze the screen, select an area, then copy, save or pin it",
        ),
        keywords: &[
            SCREENSHOT[0],
            SCREENSHOT[1],
            SCREENSHOT[2],
            "截屏",
            "截图到剪贴板",
            "screenshot to clipboard",
            "钉图",
            "pin screenshot",
        ],
        symbol: "square-dashed",
        action_title: ("截图", "Capture"),
        ocr: false,
        effects: false,
        run: capture_and_copy,
    },
    Cmd {
        key: "save",
        title: ("截图并保存", "Screenshot and Save"),
        subtitle: (
            "框选后回车保存为 PNG",
            "Select an area and press Return to save it as PNG",
        ),
        keywords: &[
            SCREENSHOT[0],
            SCREENSHOT[1],
            SCREENSHOT[2],
            "截图并保存",
            "save screenshot",
            "screenshot and save",
        ],
        symbol: "image-down",
        action_title: ("截图", "Capture"),
        ocr: false,
        effects: false,
        run: capture_and_save,
    },
    Cmd {
        key: "ocr-screen",
        title: ("截图识别文字", "Recognize Text in Screenshot"),
        subtitle: (
            "Screenshot OCR — 框选后立即识别，文字自动复制",
            "Select an area to recognize its text and copy it",
        ),
        keywords: &[
            SCREENSHOT[0],
            SCREENSHOT[1],
            SCREENSHOT[2],
            "ocr",
            "识别文字",
            "截图识别文字",
            "screenshot ocr",
            "recognize text in screenshot",
        ],
        symbol: "scan-text",
        action_title: ("截图识别", "Capture Text"),
        ocr: true,
        effects: false,
        run: capture_and_recognize,
    },
    Cmd {
        key: "ocr-clipboard",
        title: (
            "识别剪贴板图片中的文字",
            "Recognize Text in Clipboard Image",
        ),
        subtitle: (
            "Clipboard Image OCR — 识别后复制到剪贴板",
            "Copy the recognized text to the clipboard",
        ),
        keywords: &[
            "ocr",
            "识别文字",
            "识别剪贴板图片中的文字",
            "clipboard ocr",
            "recognize text in clipboard image",
        ],
        symbol: "scan-text",
        action_title: ("识别", "Recognize"),
        ocr: true,
        effects: true,
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
                Some(command_item(cmd, score, badge()))
            })
            .collect();
        items.extend(ocr_file_item(query));
        items
    }

    fn suggestions(&self) -> Vec<Item> {
        COMMANDS
            .iter()
            .filter(|cmd| cmd.key == "ocr-screen")
            .map(|cmd| Item {
                accessories: crate::hotkey::hint_keys("screenshot"),
                ..command_item(cmd, OCR_SUGGESTION_SCORE, commands_section())
            })
            .collect()
    }
}

fn command_item(cmd: &Cmd, score: u32, badge: &str) -> Item {
    Item {
        id: format!("screenshot:{}", cmd.key),
        title: text(cmd.title).into(),
        subtitle: text(cmd.subtitle).into(),
        badge: badge.into(),
        score,
        icon: Icon::Symbol {
            name: cmd.symbol,
            tint: TINT,
        },
        action: Action::new({
            let key = cmd.key;
            let run = cmd.run;
            let effects = cmd.effects;
            move || match effects.then(|| crate::effects::dry("screenshot", key)).flatten() {
                Some(outcome) => Ok(outcome),
                None => run(),
            }
        }),
        action_title: text(cmd.action_title).into(),
        actions: if cmd.ocr { ocr_actions() } else { Vec::new() },
        ..Item::blank()
    }
}

/// OCR 结果的次级操作（设计稿：复制、翻译 ⌘T）。作用于最近一次识别结果。
fn ocr_actions() -> Vec<NamedAction> {
    vec![
        NamedAction::new(
            tr("复制识别结果", "Copy Recognized Text"),
            Some("⌘C"),
            Action::new(|| {
                pasteboard::write_text(&pasteboard::general(), &last_text()?)?;
                Ok(Outcome::Done)
            }),
        ),
        NamedAction::new(
            tr("翻译识别结果", "Translate Recognized Text"),
            Some("⌘T"),
            Action::new(|| Ok(Outcome::EnterScopeWith(TRANSLATE_SCOPE.into(), last_text()?))),
        ),
    ]
}

fn last_text() -> Result<String> {
    LAST_TEXT
        .lock()
        .expect("ocr last text poisoned")
        .clone()
        .filter(|text| !text.is_empty())
        .with_context(|| {
            tr(
                "还没有识别结果：先运行一次「截图识别文字」",
                "No recognized text yet. Run “Recognize Text in Screenshot” first.",
            )
        })
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
        title: tr("识别图片中的文字", "Recognize Text in Image").into(),
        subtitle: path.display().to_string(),
        badge: badge().into(),
        score: 1000,
        icon: Icon::Symbol {
            name: "file-image",
            tint: TINT,
        },
        action: Action::new(move || ocr_file(&path, &ocr_dir()?, &languages())),
        action_title: tr("识别", "Recognize").into(),
        actions: ocr_actions(),
        ..Item::blank()
    })
}

pub(crate) fn languages() -> Vec<String> {
    crate::config::get().screenshot.ocr_languages.clone()
}

/// 截图详情的存放目录。
fn ocr_dir() -> Result<PathBuf> {
    Ok(paths::data_dir()?.join("ocr"))
}

/// 「截图并保存」的目录：配置优先（支持 `~`），缺省 `<data_dir>/screenshots`。
pub(crate) fn save_dir(configured: Option<&str>) -> Result<PathBuf> {
    match configured.map(str::trim).filter(|dir| !dir.is_empty()) {
        Some("~") => paths::home_dir(),
        Some(dir) => match dir.strip_prefix("~/") {
            Some(rest) => Ok(paths::home_dir()?.join(rest)),
            None => Ok(PathBuf::from(dir)),
        },
        None => Ok(paths::data_dir()?.join("screenshots")),
    }
}

fn capture_and_copy() -> Result<Outcome> {
    open_capture(
        capture::has_screen_access(),
        crate::capture::Mode::Capture(crate::capture::Primary::Copy),
    )
}

fn capture_and_save() -> Result<Outcome> {
    open_capture(
        capture::has_screen_access(),
        crate::capture::Mode::Capture(crate::capture::Primary::Save),
    )
}

fn capture_and_recognize() -> Result<Outcome> {
    open_capture(capture::has_screen_access(), crate::capture::Mode::Ocr)
}

/// 打开截图界面。没有录屏权限时不能去截（截到的是一片空白或报错），改为展示「前往授权」详情；
/// 测试夹具（`HELMP_CAPTURE_FIXTURE`）不截屏，不需要权限。
fn open_capture(has_access: bool, mode: crate::capture::Mode) -> Result<Outcome> {
    if !has_access && crate::capture::fixture().is_none() {
        return Ok(Outcome::Show(permission_detail()));
    }
    Ok(Outcome::Capture(mode))
}

fn ocr_clipboard() -> Result<Outcome> {
    let image = pasteboard::image_data(&pasteboard::general()).with_context(|| {
        tr(
            "剪贴板里没有图片（支持 PNG / TIFF）",
            "No image on the clipboard (PNG or TIFF supported)",
        )
    })?;
    let png = ocr::to_png(&image)?;
    let path = keep_image(&ocr_dir()?, &png.bytes)?;
    let found = ocr::recognize(&image, &languages())?;
    let size_label = tr("图片尺寸", "Image Size");
    copy_and_show(found, Shown::new(path, &png, size_label), &languages())
}

fn ocr_file(path: &Path, dir: &Path, languages: &[String]) -> Result<Outcome> {
    let image =
        std::fs::read(path).with_context(|| trf!("读取 {}", "Couldn't read {}", path.display()))?;
    let found = ocr::recognize(&image, languages)?;
    let png = ocr::to_png(&image)?;
    let size_label = tr("图片尺寸", "Image Size");
    let shown = Shown::new(keep_image(dir, &png.bytes)?, &png, size_label);
    remember(&found.text);
    Ok(Outcome::Show(ocr_detail(&found, &shown, false, languages)))
}

/// 没有录屏权限时的详情：说明原因，回车直达「系统设置 › 录屏与系统录音」。
pub(crate) fn permission_detail() -> Detail {
    Detail {
        icon: Icon::Symbol {
            name: "shield-alert",
            tint: Tint::Orange,
        },
        title: tr("需要录屏权限", "Screen Recording permission needed").into(),
        tag: None,
        blocks: vec![Block::Text(capture::no_screen_access().into())],
        metadata: Vec::new(),
        actions: vec![NamedAction::new(
            tr("前往授权", "Open Privacy Settings"),
            None,
            Action::open(screen_recording_settings()),
        )],
    }
}

/// 系统设置里「录屏与系统录音」面板，与权限引导窗口用同一个地址。
fn screen_recording_settings() -> &'static str {
    crate::onboarding::Permission::ScreenRecording.settings_url()
}

/// 详情里展示的那张图。
struct Shown {
    path: PathBuf,
    size_label: &'static str,
    width: u32,
    height: u32,
}

impl Shown {
    fn new(path: PathBuf, png: &ocr::Png, size_label: &'static str) -> Self {
        Self {
            path,
            size_label,
            width: png.width,
            height: png.height,
        }
    }
}

/// OCR 后把文字写进剪贴板并展示；没识别出文字时不覆盖剪贴板。
fn copy_and_show(found: ocr::Recognition, shown: Shown, languages: &[String]) -> Result<Outcome> {
    let copied = !found.text.is_empty();
    if copied {
        pasteboard::write_text(&pasteboard::general(), &found.text)?;
    }
    remember(&found.text);
    Ok(Outcome::Show(ocr_detail(&found, &shown, copied, languages)))
}

/// 记下最近一次识别结果（⌘K 里的「复制识别结果」「翻译识别结果」用；截图界面的识别结果窗口也调用）。
pub(crate) fn remember(text: &str) {
    *LAST_TEXT.lock().expect("ocr last text poisoned") = Some(text.to_string());
}

/// 是不是东亚文字（中日韩、全角标点）：这些字之间换行时不该补空格。
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

/// 把几行接成一行：中文与中文之间直接相连，其余交界补一个空格。
fn join_smart<'a>(lines: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for line in lines.into_iter().map(str::trim).filter(|line| !line.is_empty()) {
        // 两边都是中文才直接相连；中英文交界照常留一个空格。
        let tight = out.chars().last().is_some_and(is_cjk)
            && line.chars().next().is_some_and(is_cjk);
        if !out.is_empty() && !tight {
            out.push(' ');
        }
        out.push_str(line);
    }
    out
}

/// 「复制为单行」：所有行接成一行。
pub(crate) fn single_line(text: &str) -> String {
    join_smart(text.lines())
}

/// 「去掉换行后复制」：去掉段落内部的换行（截图里被版面折断的句子接回去），段落之间仍然换行。
pub(crate) fn unwrap_lines(text: &str) -> String {
    text.split("\n\n")
        .map(|paragraph| join_smart(paragraph.lines()))
        .filter(|paragraph| !paragraph.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 识别出的文字是什么语言（只看字符，给标题用）：中文 / 英文 / 中英混合 / 日文 / 韩文……
fn script_label(text: &str) -> Option<&'static str> {
    let count = |f: fn(u32) -> bool| text.chars().filter(|c| f(*c as u32)).count();
    let han = count(|c| matches!(c, 0x4E00..=0x9FFF | 0x3400..=0x4DBF));
    let kana = count(|c| matches!(c, 0x3040..=0x30FF));
    let hangul = count(|c| matches!(c, 0xAC00..=0xD7AF));
    let cyrillic = count(|c| matches!(c, 0x0400..=0x04FF));
    let is_latin = |c: char| c.is_alphabetic() && (c as u32) < 0x0250;
    let latin = text.chars().filter(|c| is_latin(*c)).count();
    // 拉丁字母按单词数和汉字比：一个英文单词大致相当于一两个汉字。
    let words = text
        .split(|c: char| !is_latin(c))
        .filter(|word| !word.is_empty())
        .count();
    Some(if kana > 0 {
        tr("日文", "Japanese")
    } else if hangul > 0 {
        tr("韩文", "Korean")
    } else if cyrillic > latin {
        tr("俄文", "Russian")
    } else if han > 0 && words * 5 >= han {
        tr("中英混合", "Chinese & English")
    } else if han > 0 {
        tr("中文", "Chinese")
    } else if latin > 0 && text.chars().filter(char::is_ascii_alphabetic).count() == latin {
        tr("英文", "English")
    } else if latin > 0 {
        tr("拉丁字母", "Latin script")
    } else {
        return None;
    })
}

/// 二维码内容里的链接（http / https 及其他 `scheme://` 深链）。
pub(crate) fn links(codes: &[String]) -> Vec<&str> {
    codes
        .iter()
        .map(|code| code.trim())
        .filter(|code| {
            code.split_once("://").is_some_and(|(scheme, rest)| {
                !rest.is_empty()
                    && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                    && scheme
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            })
        })
        .collect()
}

/// 识别结果的标题：「识别到 N 行 · 语言」；只有二维码时「识别到二维码」；什么都没有时明确说「没有识别到文字」，
/// 不让用户对着一片空白猜是没识别到还是卡住了。面板详情和截图界面的识别结果窗口共用。
pub(crate) fn result_title(found: &ocr::Recognition) -> String {
    let text = found.text.as_str();
    let lines = text.lines().filter(|line| !line.trim().is_empty()).count();
    // 英文按单复数说 line / lines。
    let unit = if lines == 1 { "line" } else { "lines" };
    match (lines, script_label(text)) {
        (0, _) if !found.codes.is_empty() => tr("识别到二维码", "Found a QR code").to_string(),
        (0, _) => tr("没有识别到文字", "No text found").to_string(),
        (n, Some(script)) => trf!("识别到 {n} 行 · {script}", "{n} {unit} · {script}"),
        (n, None) => trf!("识别到 {n} 行", "{n} {unit}"),
    }
}

/// 设计稿「截图识别」：标题「识别到 N 行 · 语言」，已复制时带标签；截图在上，可选中的识别结果在下；
/// 有二维码时列出内容并给「打开链接」；元数据 识别语言 / 尺寸。没识别到时明确说没有，并给「再试一次」。
fn ocr_detail(
    found: &ocr::Recognition,
    shown: &Shown,
    copied: bool,
    languages: &[String],
) -> Detail {
    let text = found.text.as_str();
    let links = links(&found.codes);
    let title = result_title(found);
    let mut blocks = vec![Block::Image(shown.path.clone())];
    if !text.is_empty() {
        blocks.push(Block::Result {
            text: text.to_string(),
            large: false,
        });
    } else if found.codes.is_empty() {
        blocks.push(Block::Text(
            tr(
                "这张图里没有可识别的文字。可以把选区框得更紧一些再试；如果是日文、韩文等，\
                 到设置 › 截图里打开对应的识别语言。",
                "There’s no readable text in this image. Try a tighter selection; for Japanese, \
                 Korean and other languages, turn them on in Settings › Screenshot.",
            )
            .into(),
        ));
    }
    // 识别语言和尺寸压成文字下面的一行小字：面板高度有限，单独占一块键值表会把识别结果挤得只剩两行。
    let mut caption = language_names(languages);
    if shown.width > 0 && shown.height > 0 {
        caption.push_str(&format!(
            " · {} {} × {}",
            shown.size_label, shown.width, shown.height
        ));
    }
    blocks.push(Block::Caption(caption));
    if !found.codes.is_empty() {
        blocks.push(Block::Quote {
            label: tr("二维码", "QR Code").into(),
            text: found.codes.join("\n"),
        });
    }
    let open_link = |url: &str, shortcut: Option<&str>| {
        NamedAction::new(tr("打开链接", "Open Link"), shortcut, Action::open(url))
    };
    let mut actions = Vec::new();
    if !text.is_empty() {
        // 回车复制全部，⌘T 带着文字进入翻译。
        actions.push(NamedAction::new(
            tr("复制全部", "Copy All"),
            None,
            Action::copy(text),
        ));
        actions.push(NamedAction::new(
            tr("翻译", "Translate"),
            Some("⌘T"),
            Action::new({
                let text = text.to_string();
                move || Ok(Outcome::EnterScopeWith(TRANSLATE_SCOPE.into(), text.clone()))
            }),
        ));
        actions.push(NamedAction::new(
            tr("复制为单行", "Copy as One Line"),
            Some("⌘⇧C"),
            Action::copy(single_line(text)),
        ));
        actions.push(NamedAction::new(
            tr("去掉换行后复制", "Copy Without Line Breaks"),
            Some("⌘⌥C"),
            Action::copy(unwrap_lines(text)),
        ));
        if let Some(url) = links.first() {
            actions.push(open_link(url, Some("⌘O")));
        }
    } else if let Some(url) = links.first() {
        // 只有二维码：回车打开链接。
        actions.push(open_link(url, None));
    }
    for code in &found.codes {
        actions.push(NamedAction::new(
            trf!("复制二维码内容：{}", "Copy QR Content: {}", clip(code)),
            None,
            Action::copy(code.clone()),
        ));
    }
    if text.is_empty() && found.codes.is_empty() {
        actions.push(NamedAction::new(
            tr("再试一次", "Try Again"),
            None,
            Action::rerun(),
        ));
    }
    Detail {
        actions,
        icon: Icon::Symbol {
            name: "scan-text",
            tint: TINT,
        },
        title,
        tag: copied.then(|| (tr("已复制", "Copied").to_string(), Tone::Done)),
        blocks,
        metadata: Vec::new(),
    }
}

/// 菜单里显示的短内容：压成一行，最多 40 个字符。
fn clip(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(40) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line,
    }
}

/// `zh-Hans, en-US` → `简体中文、英文`（英文界面：`Simplified Chinese, English`）。
pub(crate) fn language_names(languages: &[String]) -> String {
    let defaults: Vec<String> = ocr::DEFAULT_LANGUAGES
        .iter()
        .map(|code| code.to_string())
        .collect();
    let languages = if languages.is_empty() {
        &defaults
    } else {
        languages
    };
    languages
        .iter()
        .map(|code| {
            let name = match code.as_str() {
                "zh-Hans" | "zh-CN" => tr("简体中文", "Simplified Chinese"),
                "zh-Hant" | "zh-TW" | "zh-HK" => tr("繁体中文", "Traditional Chinese"),
                "yue-Hans" | "yue-Hant" => tr("粤语", "Cantonese"),
                "ja-JP" | "ja" => tr("日文", "Japanese"),
                "ko-KR" | "ko" => tr("韩文", "Korean"),
                "fr-FR" | "fr" => tr("法文", "French"),
                "de-DE" | "de" => tr("德文", "German"),
                "es-ES" | "es" => tr("西班牙文", "Spanish"),
                "it-IT" | "it" => tr("意大利文", "Italian"),
                "pt-BR" | "pt" => tr("葡萄牙文", "Portuguese"),
                "ru-RU" | "ru" => tr("俄文", "Russian"),
                "uk-UA" | "uk" => tr("乌克兰文", "Ukrainian"),
                "th-TH" | "th" => tr("泰文", "Thai"),
                "vi-VT" | "vi" => tr("越南文", "Vietnamese"),
                other if other.starts_with("en") => tr("英文", "English"),
                other => other,
            };
            name.to_string()
        })
        .collect::<Vec<_>>()
        .join(tr("、", ", "))
}

/// `<dir>/ocr-<毫秒>-<序号>.png`（目录不存在就创建）。按名字排序即按时间排序。
fn new_image_path(dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)
        .with_context(|| trf!("创建 {}", "Couldn't create {}", dir.display()))?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    // 同一毫秒内的多张按序号区分；定宽序号保证字典序 = 时间序（清理时按名字删最旧的）。
    (0..1000)
        .map(|seq| dir.join(format!("ocr-{millis:013}-{seq:03}.png")))
        .find(|path| !path.exists())
        .with_context(|| {
            trf!(
                "{} 里同一毫秒的截图太多",
                "Too many screenshots in {} within the same millisecond",
                dir.display()
            )
        })
}

/// 把要展示的 PNG 存进 `dir`，并清掉旧的。
fn keep_image(dir: &Path, png: &[u8]) -> Result<PathBuf> {
    let path = new_image_path(dir)?;
    std::fs::write(&path, png)
        .with_context(|| trf!("写入 {}", "Couldn't write {}", path.display()))?;
    prune_images(dir)?;
    Ok(path)
}

/// 只保留最新的 `KEEP_OCR_IMAGES` 张。
fn prune_images(dir: &Path) -> Result<()> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| trf!("读取 {}", "Couldn't read {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("ocr-") && name.ends_with(".png"))
        })
        .collect();
    files.sort();
    let excess = files.len().saturating_sub(KEEP_OCR_IMAGES);
    for old in files.iter().take(excess) {
        std::fs::remove_file(old).with_context(|| {
            trf!(
                "删除旧截图 {}",
                "Couldn't delete old screenshot {}",
                old.display()
            )
        })?;
    }
    Ok(())
}

/// 本地时间 `YYYY-MM-DD HH.MM.SS`（用 NSDateFormatter，避免为一个时间戳引入 chrono）。
pub(crate) fn timestamp() -> String {
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

    /// 测试专用临时目录，析构时删除。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "helmp-screenshot-{tag}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

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
    fn home_suggests_ocr_with_shortcut() {
        // 设计稿首页「快捷命令」第一条是「截图识别文字 ⌘⇧2」，分数要低于「最近使用」（850）。
        // 右侧键帽取真正生效的全局快捷键（`hotkey::hint_keys`）；单测里没有注册热键，所以为空。
        let items = Screenshot.suggestions();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.id, "screenshot:ocr-screen");
        assert_eq!(item.badge, commands_section());
        assert!(item.score < 800);
        assert!(item.accessories.is_empty(), "{:?}", item.accessories);
        assert_eq!(
            item.icon,
            Icon::Symbol {
                name: "scan-text",
                tint: Tint::Indigo
            }
        );
        let actions: Vec<&str> = item.actions.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(actions, ["复制识别结果", "翻译识别结果"]);
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
    fn ocr_file_shows_image_text_and_metadata() {
        // 设计稿「截图识别」：标题「识别到 N 行 · 语言」，图在上、可选中的文字在下，元数据 语言 / 尺寸；
        // 指定文件的识别不碰剪贴板，所以不能标「已复制」。
        let dir = TempDir::new("file");
        let languages = ["zh-Hans".to_string(), "en-US".to_string()];
        let outcome = ocr_file(Path::new(ocr::tests::FIXTURE), &dir.0, &languages).unwrap();
        let Outcome::Show(detail) = outcome else {
            panic!("应返回详情：{outcome:?}");
        };
        assert_eq!(detail.title, "识别到 2 行 · 中英混合");
        assert_eq!(detail.tag, None);
        let [
            Block::Image(image),
            Block::Result { text, large: false },
            Block::Caption(caption),
        ] = detail.blocks.as_slice()
        else {
            panic!("{:?}", detail.blocks);
        };
        ocr::tests::assert_fixture_text(text);
        assert!(image.starts_with(&dir.0) && image.exists());
        assert!(caption.starts_with("简体中文、英文 · 图片尺寸 "), "{caption}");
        assert!(caption.contains(" × "), "{caption}");
        // 次级操作「复制识别结果」读的就是这次的文字。
        assert_eq!(last_text().unwrap(), *text);
    }

    #[test]
    fn ocr_file_missing_file_fails_loudly() {
        // 路径打错要报错，不能显示「没有识别到文字」误导用户。
        let dir = TempDir::new("missing");
        assert!(ocr_file(Path::new("/nonexistent/helmp-ocr.png"), &dir.0, &[]).is_err());
    }

    /// 识别结果下面那行「语言 · 尺寸」小字。
    fn caption_of(detail: &Detail) -> Option<&str> {
        detail.blocks.iter().find_map(|block| match block {
            Block::Caption(text) => Some(text.as_str()),
            _ => None,
        })
    }

    fn shown() -> Shown {
        Shown {
            path: PathBuf::from("/tmp/x.png"),
            size_label: "区域尺寸",
            width: 640,
            height: 220,
        }
    }

    fn found(text: &str, codes: &[&str]) -> ocr::Recognition {
        ocr::Recognition {
            text: text.into(),
            codes: codes.iter().map(|code| code.to_string()).collect(),
        }
    }

    fn action_titles(detail: &Detail) -> Vec<&str> {
        detail.actions.iter().map(|a| a.title.as_str()).collect()
    }

    #[test]
    fn detail_offers_copy_variants_and_translation() {
        // 回车复制全部；⌘T 翻译；截图里的文字常被版面折行，要能一键复制成单行或去掉段内换行。
        let detail = ocr_detail(
            &found("你好 Huan 2026\nThe quick brown fox", &[]),
            &shown(),
            true,
            &[],
        );
        assert_eq!(detail.title, "识别到 2 行 · 中英混合");
        assert_eq!(detail.tag, Some(("已复制".into(), Tone::Done)));
        assert_eq!(
            action_titles(&detail),
            ["复制全部", "翻译", "复制为单行", "去掉换行后复制"]
        );
        let shortcuts: Vec<_> = detail
            .actions
            .iter()
            .map(|a| a.shortcut.as_deref())
            .collect();
        assert_eq!(shortcuts, [None, Some("⌘T"), Some("⌘⇧C"), Some("⌘⌥C")]);
        assert_eq!(
            detail.actions[1].action.run().unwrap(),
            Outcome::EnterScopeWith("translate".into(), "你好 Huan 2026\nThe quick brown fox".into())
        );
        assert_eq!(
            caption_of(&detail),
            Some("简体中文、英文 · 区域尺寸 640 × 220")
        );
    }

    #[test]
    fn empty_result_says_so_instead_of_blank() {
        // 没识别到文字时空白一片，用户分不清是没识别到还是卡住了：要明确说「没有识别到文字」、给建议，
        // 不能说「已复制」，回车「再试一次」。
        let empty = ocr_detail(&found("", &[]), &shown(), false, &["ja-JP".to_string()]);
        assert_eq!(empty.title, "没有识别到文字");
        assert_eq!(empty.tag, None);
        assert!(matches!(&empty.blocks[..], [Block::Image(_), Block::Text(advice), Block::Caption(_)] if advice.contains("没有可识别的文字")));
        assert_eq!(action_titles(&empty), ["再试一次"]);
        assert!(empty.actions[0].action.is_rerun());
        assert_eq!(caption_of(&empty), Some("日文 · 区域尺寸 640 × 220"));
    }

    #[test]
    fn qr_code_links_can_be_opened() {
        // 截图里有二维码：读出的链接要能直接打开；只有二维码没有文字时，回车就是打开链接。
        let both = ocr_detail(
            &found("扫码打开 Helmp", &["https://example.com/helmp"]),
            &shown(),
            true,
            &[],
        );
        assert!(action_titles(&both).contains(&"打开链接"));
        assert!(matches!(
            both.blocks.last(),
            Some(Block::Quote { text, .. }) if text == "https://example.com/helmp"
        ));
        let only = ocr_detail(&found("", &["https://example.com/helmp"]), &shown(), false, &[]);
        assert_eq!(only.title, "识别到二维码");
        assert_eq!(action_titles(&only)[0], "打开链接");
        // 不是链接的码（如纯文本、条码数字）只给复制，不给打开。
        let plain = ocr_detail(&found("", &["6901234567892"]), &shown(), false, &[]);
        assert!(!action_titles(&plain).contains(&"打开链接"));
        assert_eq!(action_titles(&plain), ["复制二维码内容：6901234567892"]);
    }

    #[test]
    fn qr_fixture_end_to_end() {
        // 真实图片走一遍：二维码链接和旁边的文字都要出来，并给「打开链接」。
        let dir = TempDir::new("qr");
        let Outcome::Show(detail) = ocr_file(Path::new(ocr::tests::QR_FIXTURE), &dir.0, &[]).unwrap()
        else {
            panic!("应返回详情");
        };
        assert!(action_titles(&detail).contains(&"打开链接"), "{:?}", action_titles(&detail));
    }

    #[test]
    fn copy_variants_join_lines_sensibly() {
        // 中文行之间直接相连、英文单词之间补空格；「去掉换行」保留段落。
        let text = "截图里的文字被\n折成了两行\nand English words\nwrap too\n\n第二段";
        assert_eq!(
            single_line(text),
            "截图里的文字被折成了两行 and English words wrap too 第二段"
        );
        assert_eq!(
            unwrap_lines(text),
            "截图里的文字被折成了两行 and English words wrap too\n第二段"
        );
        assert_eq!(single_line("   "), "");
    }

    #[test]
    fn script_label_names_the_recognized_language() {
        assert_eq!(script_label("你好世界，今天天气不错"), Some("中文"));
        assert_eq!(script_label("The quick brown fox"), Some("英文"));
        assert_eq!(script_label("你好 Huan 2026\nThe quick brown fox"), Some("中英混合"));
        assert_eq!(script_label("こんにちは"), Some("日文"));
        assert_eq!(script_label("안녕하세요"), Some("韩文"));
        assert_eq!(script_label("2026 / 11 / 09"), None);
    }

    #[test]
    fn capture_commands_open_the_frozen_screen_overlay() {
        // 三个截图命令都打开冻结屏幕的选区界面，回车的默认动作各自保留旧命令的含义：
        // 「截图」复制（旧的「截图到剪贴板」）、「截图并保存」保存、「截图识别文字」松手即识别。
        use crate::capture::{Mode, Primary};
        assert_eq!(
            open_capture(true, Mode::Capture(Primary::Copy)).unwrap(),
            Outcome::Capture(Mode::Capture(Primary::Copy))
        );
        let ids = [
            ("screenshot:clipboard", Mode::Capture(Primary::Copy)),
            ("screenshot:save", Mode::Capture(Primary::Save)),
            ("screenshot:ocr-screen", Mode::Ocr),
        ];
        for (id, mode) in ids {
            let item = Screenshot.query("截图").into_iter().find(|item| item.id == id).unwrap();
            // 本机有没有录屏权限不由测试决定：有就直接打开界面，没有就给「前往授权」，两种都不能是别的。
            match item.action.run().unwrap() {
                Outcome::Capture(got) => assert_eq!(got, mode, "{id}"),
                Outcome::Show(detail) => assert_eq!(detail.title, "需要录屏权限", "{id}"),
                other => panic!("{id} 应打开截图界面：{other:?}"),
            }
        }
    }

    #[test]
    fn missing_permission_leads_to_system_settings() {
        // 没有录屏权限时不能去截（会截到一片空白或报错），而要给「前往授权」，直达对应的系统设置面板。
        if crate::capture::fixture().is_none() {
            let Outcome::Show(detail) = open_capture(false, crate::capture::Mode::Ocr).unwrap() else {
                panic!("没权限时应展示「前往授权」");
            };
            assert_eq!(detail.title, "需要录屏权限");
        }
        let detail = permission_detail();
        assert_eq!(detail.title, "需要录屏权限");
        assert_eq!(action_titles(&detail), ["前往授权"]);
        assert!(
            screen_recording_settings().ends_with("Privacy_ScreenCapture"),
            "{}",
            screen_recording_settings()
        );
    }
    #[test]
    fn english_ui_still_finds_ocr_by_chinese_query() {
        // 全局快捷键 ⌘⇧2 和菜单栏用 `RunItem { id: "screenshot:ocr-screen", query: "截图识别文字" }`
        // 重新查找这条命令：界面换成英文后，这个中文查询必须仍然命中同一个 id，否则快捷键会失灵。
        use crate::i18n::{Lang, with_lang};
        with_lang(Lang::En, || {
            let items = Screenshot.query("截图识别文字");
            let ocr = items
                .iter()
                .find(|item| item.id == "screenshot:ocr-screen")
                .expect("英文界面下「截图识别文字」应命中 screenshot:ocr-screen");
            assert_eq!(ocr.title, "Recognize Text in Screenshot");
            assert_eq!(ocr.badge, "Screenshot");
            assert_eq!(ocr.action_title, "Capture Text");
            let actions: Vec<&str> = ocr.actions.iter().map(|a| a.title.as_str()).collect();
            assert_eq!(
                actions,
                ["Copy Recognized Text", "Translate Recognized Text"]
            );
            // 拼音简写与英文标题也照常能找到。
            assert!(ids("jt").contains(&"screenshot:ocr-screen".to_string()));
            assert!(ids("recognize text").contains(&"screenshot:ocr-clipboard".to_string()));
        });
        // 中文界面下输入英文标题也能找到。
        assert!(
            ids("Recognize Text in Clipboard Image")
                .contains(&"screenshot:ocr-clipboard".to_string())
        );
    }

    #[test]
    fn english_detail_counts_lines_and_names_languages() {
        // 识别结果详情是用户最常看的界面：英文下要说清行数（单复数）、语言、是否已复制，语言名也要是英文。
        use crate::i18n::{Lang, with_lang};
        with_lang(Lang::En, || {
            let shown = Shown {
                path: PathBuf::from("/tmp/x.png"),
                size_label: tr("区域尺寸", "Selection Size"),
                width: 640,
                height: 220,
            };
            let detail = ocr_detail(&found("a\nb", &[]), &shown, true, &[]);
            assert_eq!(detail.title, "2 lines · English");
            assert_eq!(detail.tag, Some(("Copied".into(), Tone::Done)));
            assert_eq!(
                caption_of(&detail),
                Some("Simplified Chinese, English · Selection Size 640 × 220")
            );
            assert_eq!(
                ocr_detail(&found("你好", &[]), &shown, false, &[]).title,
                "1 line · Chinese"
            );
            assert_eq!(
                ocr_detail(&found("", &[]), &shown, false, &[]).title,
                "No text found"
            );
            assert_eq!(permission_detail().actions[0].title, "Open Privacy Settings");
            assert!(capture::no_screen_access().contains("Screen Recording"));
        });
    }

    #[test]
    fn keeps_only_recent_ocr_images() {
        // 每次识别都会留一张图给详情区显示；不清理的话数据目录会无限变大。
        let dir = TempDir::new("prune");
        let mut kept = Vec::new();
        for _ in 0..KEEP_OCR_IMAGES + 3 {
            kept.push(keep_image(&dir.0, b"png").unwrap());
        }
        let left = std::fs::read_dir(&dir.0).unwrap().count();
        assert_eq!(left, KEEP_OCR_IMAGES);
        assert!(kept.last().unwrap().exists(), "最新的一张必须留下");
        assert!(!kept[0].exists(), "最旧的一张应被清掉");
    }

    #[test]
    fn save_dir_honors_config_and_tilde() {
        // 用户在设置里选了目录就必须存到那里；`~/Desktop` 这种写法不能被当成相对路径。
        let home = paths::home_dir().unwrap();
        assert_eq!(save_dir(Some("~/Desktop")).unwrap(), home.join("Desktop"));
        assert_eq!(
            save_dir(Some("/tmp/shots")).unwrap(),
            PathBuf::from("/tmp/shots")
        );
        assert_eq!(save_dir(Some("~")).unwrap(), home);
    }

    #[test]
    fn clipboard_image_path_recognizes_text() {
        // 「识别剪贴板图片」是不需要屏幕录制权限的主路径：取图 → OCR 必须端到端可用。
        // 用私有 pasteboard，完全不碰用户真实剪贴板。
        let pb = pasteboard::tests::Private::new();
        pb.put_png(&std::fs::read(ocr::tests::FIXTURE).unwrap());
        let image = pasteboard::image_data(&pb.0).expect("应读到 PNG");
        ocr::tests::assert_fixture_text(&ocr::recognize(&image, &[]).unwrap().text);
    }

    #[test]
    fn symbol_icons_exist_in_bundled_assets() {
        // 图标名拼错时界面只会画一个空白方块，不会报错；这里把用到的名字逐个对一遍资源包。
        use gpui_kit::AssetSource as _;
        let names = COMMANDS
            .iter()
            .map(|cmd| cmd.symbol)
            .chain(["file-image", "scan-text", "shield-alert"]);
        for name in names {
            let path = format!("icons/{name}.svg");
            assert!(
                gpui_kit::assets::AllAssets.load(&path).unwrap().is_some(),
                "缺少图标 {path}"
            );
        }
    }

    #[test]
    fn timestamp_is_filename_safe() {
        // 文件名里不能有冒号（Finder 显示成斜杠），格式固定 `YYYY-MM-DD HH.MM.SS`。
        let ts = timestamp();
        assert_eq!(ts.len(), 19, "{ts}");
        assert!(!ts.contains(':') && !ts.contains('/'), "{ts}");
    }
}
