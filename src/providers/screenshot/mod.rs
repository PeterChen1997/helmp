//! 截图 + OCR。
//!
//! 命令（空查询不返回；首页「快捷命令」里放「截图识别文字」）：
//! - `screenshot:clipboard`      区域截图到剪贴板
//! - `screenshot:save`           区域截图保存到 `config.screenshot.save_dir`（缺省 `<data_dir>/screenshots/`），并在 Finder 中显示
//! - `screenshot:ocr-screen`     区域截图 → OCR → 文字写剪贴板，详情展示截图与识别结果
//! - `screenshot:ocr-clipboard`  剪贴板图片 → OCR → 文字写剪贴板（不需要屏幕录制权限）
//! - `screenshot:ocr-file:<绝对路径>`  查询 `ocr /绝对路径` 时出现，只展示结果、不写剪贴板
//!
//! OCR 语言读 `config.screenshot.ocr_languages`。详情里展示的截图存在 `<data_dir>/ocr/`，只保留最近几张。

mod capture;
mod ocr;
/// 脚本插件的「复制输出」也用它写剪贴板。
pub(super) mod pasteboard;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use anyhow::{Context as _, Result};
use objc2_foundation::{NSDate, NSDateFormatter, NSLocale, NSString};

use crate::engine::COMMANDS_SECTION;
use crate::matcher;
use crate::paths;
use crate::provider::{
    Accessory, Action, Block, Detail, Icon, Item, NamedAction, Outcome, Provider, Tint,
};

const BADGE: &str = "截图";
const TINT: Tint = Tint::Indigo;
/// 首页「快捷命令」里的分数：排在第一（设计稿顺序：截图识别文字、翻译、剪贴板历史）。
const OCR_SUGGESTION_SCORE: u32 = 720;
/// `<data_dir>/ocr/` 里保留的截图张数：详情区要显示最近一次的截图，更早的没用。
const KEEP_OCR_IMAGES: usize = 10;
/// 翻译插件的范围 id（翻译插件由另一个模块提供）。
const TRANSLATE_SCOPE: &str = "translate";

/// 最近一次识别出的文字，供 ⌘K 里的「复制识别结果」「翻译」使用。
static LAST_TEXT: Mutex<Option<String>> = Mutex::new(None);

struct Cmd {
    key: &'static str,
    title: &'static str,
    subtitle: &'static str,
    keywords: &'static [&'static str],
    symbol: &'static str,
    action_title: &'static str,
    /// OCR 命令带「复制识别结果」「翻译」次级操作。
    ocr: bool,
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
        symbol: "square-dashed",
        action_title: "截图",
        ocr: false,
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
        symbol: "image-down",
        action_title: "截图",
        ocr: false,
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
        symbol: "scan-text",
        action_title: "截图识别",
        ocr: true,
        run: ocr_screen,
    },
    Cmd {
        key: "ocr-clipboard",
        title: "识别剪贴板图片中的文字",
        subtitle: "Clipboard Image OCR — 识别后复制到剪贴板",
        keywords: &["ocr", "识别文字", "识别剪贴板图片中的文字", "clipboard ocr"],
        symbol: "scan-text",
        action_title: "识别",
        ocr: true,
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
                Some(command_item(cmd, score, BADGE))
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
                accessories: vec![Accessory::keys("⌘⇧2")],
                ..command_item(cmd, OCR_SUGGESTION_SCORE, COMMANDS_SECTION)
            })
            .collect()
    }
}

fn command_item(cmd: &Cmd, score: u32, badge: &str) -> Item {
    Item {
        id: format!("screenshot:{}", cmd.key),
        title: cmd.title.into(),
        subtitle: cmd.subtitle.into(),
        badge: badge.into(),
        score,
        icon: Icon::Symbol {
            name: cmd.symbol,
            tint: TINT,
        },
        action: Action::new(cmd.run),
        action_title: cmd.action_title.into(),
        actions: if cmd.ocr { ocr_actions() } else { Vec::new() },
        ..Item::blank()
    }
}

/// OCR 结果的次级操作（设计稿：复制、翻译 ⌘T）。作用于最近一次识别结果。
fn ocr_actions() -> Vec<NamedAction> {
    vec![
        NamedAction::new(
            "复制识别结果",
            Some("⌘C"),
            Action::new(|| {
                pasteboard::write_text(&pasteboard::general(), &last_text()?)?;
                Ok(Outcome::Done)
            }),
        ),
        NamedAction::new(
            "翻译识别结果",
            Some("⌘T"),
            Action::new(|| {
                // 识别结果已经在剪贴板里；翻译插件从那里取。
                last_text()?;
                Ok(Outcome::EnterScope(TRANSLATE_SCOPE))
            }),
        ),
    ]
}

fn last_text() -> Result<String> {
    LAST_TEXT
        .lock()
        .expect("ocr last text poisoned")
        .clone()
        .filter(|text| !text.is_empty())
        .context("还没有识别结果：先运行一次「截图识别文字」")
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
        icon: Icon::Symbol {
            name: "file-image",
            tint: TINT,
        },
        action: Action::new(move || ocr_file(&path, &ocr_dir()?, &languages())),
        action_title: "识别".into(),
        actions: ocr_actions(),
        ..Item::blank()
    })
}

fn languages() -> Vec<String> {
    crate::config::get().screenshot.ocr_languages.clone()
}

/// 截图详情的存放目录。
fn ocr_dir() -> Result<PathBuf> {
    Ok(paths::data_dir()?.join("ocr"))
}

/// 「截图并保存」的目录：配置优先（支持 `~`），缺省 `<data_dir>/screenshots`。
fn save_dir(configured: Option<&str>) -> Result<PathBuf> {
    match configured.map(str::trim).filter(|dir| !dir.is_empty()) {
        Some("~") => paths::home_dir(),
        Some(dir) => match dir.strip_prefix("~/") {
            Some(rest) => Ok(paths::home_dir()?.join(rest)),
            None => Ok(PathBuf::from(dir)),
        },
        None => Ok(paths::data_dir()?.join("screenshots")),
    }
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
    let dir = save_dir(crate::config::get().screenshot.save_dir.as_deref())?;
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
    let dir = ocr_dir()?;
    let path = new_image_path(&dir)?;
    if !capture::to_file(&path)? {
        return Ok(Outcome::Done);
    }
    prune_images(&dir)?;
    let image = std::fs::read(&path).with_context(|| format!("读取 {}", path.display()))?;
    let png = ocr::to_png(&image)?;
    let text = ocr::recognize(&image, &languages())?;
    copy_and_show(text, Shown::new(path, &png, "区域尺寸"), &languages())
}

fn ocr_clipboard() -> Result<Outcome> {
    let image = pasteboard::image_data(&pasteboard::general())
        .context("剪贴板里没有图片（支持 PNG / TIFF）")?;
    let png = ocr::to_png(&image)?;
    let path = keep_image(&ocr_dir()?, &png.bytes)?;
    let text = ocr::recognize(&image, &languages())?;
    copy_and_show(text, Shown::new(path, &png, "图片尺寸"), &languages())
}

fn ocr_file(path: &Path, dir: &Path, languages: &[String]) -> Result<Outcome> {
    let image = std::fs::read(path).with_context(|| format!("读取 {}", path.display()))?;
    let text = ocr::recognize(&image, languages)?;
    let png = ocr::to_png(&image)?;
    let shown = Shown::new(keep_image(dir, &png.bytes)?, &png, "图片尺寸");
    remember(&text);
    Ok(Outcome::Show(ocr_detail(&text, &shown, false, languages)))
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
fn copy_and_show(text: String, shown: Shown, languages: &[String]) -> Result<Outcome> {
    let copied = !text.is_empty();
    if copied {
        pasteboard::write_text(&pasteboard::general(), &text)?;
    }
    remember(&text);
    Ok(Outcome::Show(ocr_detail(&text, &shown, copied, languages)))
}

fn remember(text: &str) {
    *LAST_TEXT.lock().expect("ocr last text poisoned") = Some(text.to_string());
}

/// 设计稿「截图识别」：标题说明识别了几行、是否已复制；截图在上，识别结果在下；元数据 语言 / 尺寸。
fn ocr_detail(text: &str, shown: &Shown, copied: bool, languages: &[String]) -> Detail {
    let lines = text.lines().filter(|line| !line.trim().is_empty()).count();
    let title = match (lines, copied) {
        (0, _) => "没有识别到文字".to_string(),
        (n, true) => format!("已识别 {n} 行文字，已复制到剪贴板"),
        (n, false) => format!("已识别 {n} 行文字"),
    };
    let mut blocks = vec![Block::Image(shown.path.clone())];
    if !text.is_empty() {
        blocks.push(Block::Text(text.to_string()));
    }
    let mut metadata = vec![("语言".to_string(), language_names(languages))];
    if shown.width > 0 && shown.height > 0 {
        metadata.push((
            shown.size_label.to_string(),
            format!("{} × {}", shown.width, shown.height),
        ));
    }
    Detail {
        icon: Icon::Symbol {
            name: "scan-text",
            tint: TINT,
        },
        title,
        tag: None,
        blocks,
        metadata,
    }
}

/// `zh-Hans, en-US` → `简体中文、英文`。
fn language_names(languages: &[String]) -> String {
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
                "zh-Hans" | "zh-CN" => "简体中文",
                "zh-Hant" | "zh-TW" | "zh-HK" => "繁体中文",
                "yue-Hans" | "yue-Hant" => "粤语",
                "ja-JP" | "ja" => "日文",
                "ko-KR" | "ko" => "韩文",
                "fr-FR" | "fr" => "法文",
                "de-DE" | "de" => "德文",
                "es-ES" | "es" => "西班牙文",
                "it-IT" | "it" => "意大利文",
                "pt-BR" | "pt" => "葡萄牙文",
                "ru-RU" | "ru" => "俄文",
                other if other.starts_with("en") => "英文",
                other => other,
            };
            name.to_string()
        })
        .collect::<Vec<_>>()
        .join("、")
}

/// `<dir>/ocr-<毫秒>-<序号>.png`（目录不存在就创建）。按名字排序即按时间排序。
fn new_image_path(dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("创建 {}", dir.display()))?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    // 同一毫秒内的多张按序号区分；定宽序号保证字典序 = 时间序（清理时按名字删最旧的）。
    (0..1000)
        .map(|seq| dir.join(format!("ocr-{millis:013}-{seq:03}.png")))
        .find(|path| !path.exists())
        .with_context(|| format!("{} 里同一毫秒的截图太多", dir.display()))
}

/// 把要展示的 PNG 存进 `dir`，并清掉旧的。
fn keep_image(dir: &Path, png: &[u8]) -> Result<PathBuf> {
    let path = new_image_path(dir)?;
    std::fs::write(&path, png).with_context(|| format!("写入 {}", path.display()))?;
    prune_images(dir)?;
    Ok(path)
}

/// 只保留最新的 `KEEP_OCR_IMAGES` 张。
fn prune_images(dir: &Path) -> Result<()> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("读取 {}", dir.display()))?
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
        std::fs::remove_file(old).with_context(|| format!("删除旧截图 {}", old.display()))?;
    }
    Ok(())
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
        let items = Screenshot.suggestions();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.id, "screenshot:ocr-screen");
        assert_eq!(item.badge, COMMANDS_SECTION);
        assert!(item.score < 800);
        assert_eq!(item.accessories, vec![Accessory::keys("⌘⇧2")]);
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
        // 设计稿「截图识别」：标题说明行数，图在上、文字在下，元数据 语言 / 尺寸；
        // 指定文件的识别不碰剪贴板，所以标题里不能说「已复制」。
        let dir = TempDir::new("file");
        let languages = ["zh-Hans".to_string(), "en-US".to_string()];
        let outcome = ocr_file(Path::new(ocr::tests::FIXTURE), &dir.0, &languages).unwrap();
        let Outcome::Show(detail) = outcome else {
            panic!("应返回详情：{outcome:?}");
        };
        assert_eq!(detail.title, "已识别 2 行文字");
        let [Block::Image(image), Block::Text(text)] = detail.blocks.as_slice() else {
            panic!("{:?}", detail.blocks);
        };
        ocr::tests::assert_fixture_text(text);
        assert!(image.starts_with(&dir.0) && image.exists());
        assert_eq!(detail.metadata[0], ("语言".into(), "简体中文、英文".into()));
        assert_eq!(detail.metadata[1].0, "图片尺寸");
        assert!(detail.metadata[1].1.contains(" × "));
        // 次级操作「复制识别结果」读的就是这次的文字。
        assert_eq!(last_text().unwrap(), *text);
    }

    #[test]
    fn ocr_file_missing_file_fails_loudly() {
        // 路径打错要报错，不能显示「没有识别到文字」误导用户。
        let dir = TempDir::new("missing");
        assert!(ocr_file(Path::new("/nonexistent/helmp-ocr.png"), &dir.0, &[]).is_err());
    }

    #[test]
    fn detail_title_reflects_copy_and_empty_results() {
        let shown = Shown {
            path: PathBuf::from("/tmp/x.png"),
            size_label: "区域尺寸",
            width: 640,
            height: 220,
        };
        let detail = ocr_detail("你好 Huan 2026\nThe quick brown fox", &shown, true, &[]);
        assert_eq!(detail.title, "已识别 2 行文字，已复制到剪贴板");
        assert_eq!(
            detail.metadata,
            [
                ("语言".to_string(), "简体中文、英文".to_string()),
                ("区域尺寸".to_string(), "640 × 220".to_string())
            ]
        );
        // 没识别到文字时不能说「已复制」，详情里只有截图。
        let empty = ocr_detail("", &shown, false, &["ja-JP".to_string()]);
        assert_eq!(empty.title, "没有识别到文字");
        assert_eq!(empty.blocks, [Block::Image(PathBuf::from("/tmp/x.png"))]);
        assert_eq!(empty.metadata[0].1, "日文");
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
        ocr::tests::assert_fixture_text(&ocr::recognize(&image, &[]).unwrap());
    }

    #[test]
    fn symbol_icons_exist_in_bundled_assets() {
        // 图标名拼错时界面只会画一个空白方块，不会报错；这里把用到的名字逐个对一遍资源包。
        use gpui_kit::AssetSource as _;
        let names = COMMANDS
            .iter()
            .map(|cmd| cmd.symbol)
            .chain(["file-image", "scan-text"]);
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
