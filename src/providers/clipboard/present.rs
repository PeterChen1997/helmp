//! 剪贴板条目在面板里的样子：图标、副标题、详情、分组（今天 / 昨天 / 某月某日）。
//!
//! 全部是纯函数，只读内存索引里的 `Indexed`；时间相关的都显式接收 `Clock`，测试可控。

use std::path::Path;

use super::Indexed;
use super::store::Kind;
use crate::i18n::{tr, trf};
use crate::provider::{Block, Detail, Icon, Tint};

/// 详情区最多展示的字符数，超出部分提示总字数。
pub const DETAIL_CHARS: usize = 2000;

/// 一次查询共用的「现在」：当前时间与本地今天零点（Unix 毫秒）。
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    pub now_ms: i64,
    pub today_start_ms: i64,
}

impl Clock {
    pub fn now() -> Self {
        let now_ms = super::now_ms();
        Self {
            now_ms,
            today_start_ms: local_midnight(now_ms),
        }
    }

    fn yesterday_start_ms(self) -> i64 {
        self.today_start_ms - 86_400_000
    }
}

/// 文本条目的细分，决定图标和「内容」一栏。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    Plain,
    Link,
    Command,
}

/// 常见命令行程序：单行且以它们开头的文本按「命令」显示（等宽、终端图标）。
const COMMANDS: &[&str] = &[
    "git",
    "cd",
    "ls",
    "npm",
    "pnpm",
    "npx",
    "yarn",
    "cargo",
    "rustup",
    "brew",
    "ssh",
    "scp",
    "curl",
    "wget",
    "docker",
    "kubectl",
    "sudo",
    "cat",
    "echo",
    "grep",
    "rg",
    "find",
    "make",
    "python",
    "python3",
    "pip",
    "uv",
    "node",
    "go",
    "open",
    "mkdir",
    "rm",
    "cp",
    "mv",
    "chmod",
    "export",
    "source",
    "tar",
    "ffmpeg",
    "gh",
    "claude",
    "codex",
    "xcodebuild",
    "defaults",
];

pub fn classify(text: &str) -> TextKind {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains('\n') {
        return TextKind::Plain;
    }
    if !trimmed.contains(char::is_whitespace) && link_url(trimmed).is_some() {
        return TextKind::Link;
    }
    let first = trimmed.split_whitespace().next().unwrap_or_default();
    if trimmed.starts_with("$ ") || COMMANDS.contains(&first) {
        return TextKind::Command;
    }
    TextKind::Plain
}

/// 文本像链接时返回可以直接打开的 URL（没写协议的补 `https://`）。
pub fn link_url(text: &str) -> Option<String> {
    let text = text.trim();
    if text.contains(char::is_whitespace) {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    if ["http://", "https://"]
        .iter()
        .any(|scheme| lower.starts_with(scheme) && lower.len() > scheme.len())
    {
        return Some(text.to_string());
    }
    // `github.com/zed-industries/zed` 这种没写协议的：主机名带点、顶级域是字母、后面跟路径。
    let (host, path) = text.split_once('/')?;
    let tld = host.rsplit_once('.')?.1;
    let host_ok = !host.starts_with('.')
        && host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-');
    (host_ok
        && tld.len() >= 2
        && tld.chars().all(|ch| ch.is_ascii_alphabetic())
        && !path.is_empty())
    .then(|| format!("https://{text}"))
}

pub fn icon(indexed: &Indexed) -> Icon {
    let name = match indexed.kind {
        Kind::Image => match &indexed.image_path {
            Some(path) => return Icon::Image(path.clone()),
            None => "image",
        },
        Kind::Files if indexed.count > 1 => "files",
        Kind::Files => "file",
        Kind::Text => match indexed.text_kind {
            TextKind::Plain => "text-align-start",
            TextKind::Link => "link",
            TextKind::Command => "square-terminal",
        },
    };
    Icon::Symbol {
        name,
        tint: Tint::Gray,
    }
}

/// 「来源 App · 2 分钟前」，文件再加「· N 个文件」（与设计稿一致）。
pub fn subtitle(indexed: &Indexed, clock: Clock) -> String {
    let source = indexed
        .source
        .as_deref()
        .unwrap_or(tr("未知来源", "Unknown source"));
    let ago = ago(clock.now_ms - indexed.copied_at_ms);
    match indexed.kind {
        Kind::Files => format!("{source} · {ago} · {}", files_label(indexed.count)),
        Kind::Text | Kind::Image => format!("{source} · {ago}"),
    }
}

pub fn detail(indexed: &Indexed, clock: Clock) -> Detail {
    let mut blocks = Vec::new();
    match indexed.kind {
        Kind::Text => {
            let body = indexed.preview.clone();
            blocks.push(match indexed.text_kind {
                TextKind::Command => Block::Code(body),
                TextKind::Plain | TextKind::Link => Block::Text(body),
            });
            if indexed.count > DETAIL_CHARS {
                blocks.push(Block::Text(trf!(
                    "……共 {} 字，这里只显示前 {DETAIL_CHARS} 字",
                    "…{} characters in total; showing the first {DETAIL_CHARS}",
                    thousands(indexed.count)
                )));
            }
        }
        Kind::Files => blocks.push(Block::Code(indexed.preview.clone())),
        Kind::Image => {
            if let Some(path) = &indexed.image_path {
                blocks.push(Block::Image(path.clone()));
            }
        }
    }
    Detail {
        actions: Vec::new(),
        // 设计稿的剪贴板详情没有标题行，正文直接占满。
        icon: Icon::None,
        title: String::new(),
        tag: None,
        blocks,
        metadata: vec![
            (
                tr("来源", "Source").into(),
                indexed
                    .source
                    .clone()
                    .unwrap_or_else(|| tr("未知", "Unknown").into()),
            ),
            (
                tr("复制于", "Copied").into(),
                copied_at(clock, indexed.copied_at_ms),
            ),
            (tr("内容", "Content").into(), content_label(indexed)),
        ],
    }
}

fn content_label(indexed: &Indexed) -> String {
    let chars = match (crate::i18n::is_en(), indexed.count) {
        (false, count) => format!("{} 字", thousands(count)),
        (true, 1) => "1 character".into(),
        (true, count) => format!("{} characters", thousands(count)),
    };
    let image = if crate::i18n::is_en() {
        match indexed.image_format {
            "" => "Image".to_string(),
            format => format!("{format} image"),
        }
    } else {
        format!("{} 图片", indexed.image_format)
    };
    match indexed.kind {
        Kind::Text => match indexed.text_kind {
            TextKind::Plain => format!("{} · {chars}", tr("纯文本", "Plain text")),
            TextKind::Link => format!("{} · {chars}", tr("链接", "Link")),
            TextKind::Command => format!("{} · {chars}", tr("命令", "Command")),
        },
        Kind::Files => files_label(indexed.count),
        Kind::Image if indexed.width > 0 && indexed.height > 0 => {
            format!("{image} · {} × {}", indexed.width, indexed.height)
        }
        Kind::Image => image,
    }
}

/// 「3 个文件」/「3 files」。
fn files_label(count: usize) -> String {
    match (crate::i18n::is_en(), count) {
        (true, 1) => "1 file".into(),
        _ => trf!("{} 个文件", "{} files", thousands(count)),
    }
}

/// 英文日期用的月份缩写。
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// 列表小节：今天 / 昨天 / 11月3日 / 2025年11月3日（英文 Today / Yesterday / Nov 3 / Nov 3, 2025）。
/// 它也是分组键，在查询里现取（查询已按语言固定）。
pub fn day_label(clock: Clock, at_ms: i64) -> String {
    if at_ms >= clock.today_start_ms {
        return tr("今天", "Today").into();
    }
    if at_ms >= clock.yesterday_start_ms() {
        return tr("昨天", "Yesterday").into();
    }
    let then = local_time(at_ms);
    let this_year = then.year == local_time(clock.now_ms).year;
    if crate::i18n::is_en() {
        let month = usize::try_from(then.month - 1)
            .ok()
            .and_then(|index| MONTHS.get(index))
            .copied()
            .unwrap_or_default();
        if this_year {
            format!("{month} {}", then.day)
        } else {
            format!("{month} {}, {}", then.day, then.year)
        }
    } else if this_year {
        format!("{}月{}日", then.month, then.day)
    } else {
        format!("{}年{}月{}日", then.year, then.month, then.day)
    }
}

/// 「今天 13:43」「昨天 09:05」「11月3日 13:43」（英文 Today at 13:43）。
pub fn copied_at(clock: Clock, at_ms: i64) -> String {
    let then = local_time(at_ms);
    let day = day_label(clock, at_ms);
    let (hour, minute) = (then.hour, then.minute);
    trf!(
        "{day} {hour:02}:{minute:02}",
        "{day} at {hour:02}:{minute:02}"
    )
}

pub fn ago(elapsed_ms: i64) -> String {
    let seconds = elapsed_ms.max(0) / 1000;
    match seconds {
        0..60 => tr("刚刚", "Just now").into(),
        60..3600 => trf!("{} 分钟前", "{} min ago", seconds / 60),
        3600..86_400 => trf!("{} 小时前", "{} hr ago", seconds / 3600),
        86_400..172_800 => tr("1 天前", "1 day ago").into(),
        _ => trf!("{} 天前", "{} days ago", seconds / 86_400),
    }
}

/// `3482` → `3,482`。
pub fn thousands(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// 图片条目的格式名（PNG / TIFF），从落盘文件名推断。
pub fn image_format(file: &Path) -> &'static str {
    match file.extension().and_then(|ext| ext.to_str()) {
        Some("png") => "PNG",
        Some("tiff") => "TIFF",
        _ => "",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LocalTime {
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
}

fn local_tm(ms: i64) -> libc::tm {
    let seconds: libc::time_t = ms.div_euclid(1000);
    // SAFETY: tm 是纯数据结构，全零是合法初值；localtime_r 只写入我们传入的 tm。
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: 两个指针都指向本函数里的有效局部变量。
    unsafe { libc::localtime_r(&seconds, &mut tm) };
    tm
}

fn local_time(ms: i64) -> LocalTime {
    let tm = local_tm(ms);
    LocalTime {
        year: tm.tm_year + 1900,
        month: tm.tm_mon + 1,
        day: tm.tm_mday,
        hour: tm.tm_hour,
        minute: tm.tm_min,
    }
}

/// `ms` 所在那天的本地零点（Unix 毫秒）。
fn local_midnight(ms: i64) -> i64 {
    let mut tm = local_tm(ms);
    tm.tm_hour = 0;
    tm.tm_min = 0;
    tm.tm_sec = 0;
    tm.tm_isdst = -1;
    // SAFETY: tm 是上面 localtime_r 填好的有效结构；mktime 只读写它。
    let seconds = unsafe { libc::mktime(&mut tm) };
    seconds * 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_links_commands_and_plain_text() {
        // 图标和「内容」一栏靠它：链接要能一键打开，命令要等宽显示，误判会让普通句子显示成终端命令。
        assert_eq!(classify("github.com/zed-industries/zed"), TextKind::Link);
        assert_eq!(classify("https://example.com"), TextKind::Link);
        assert_eq!(classify("git rebase -i HEAD~3"), TextKind::Command);
        assert_eq!(classify("$ ls -la"), TextKind::Command);
        assert_eq!(classify("会议改到周四下午三点"), TextKind::Plain);
        assert_eq!(
            classify("open the door please\nsecond line"),
            TextKind::Plain
        );
        assert_eq!(classify("v1.2/3"), TextKind::Plain);
        assert_eq!(classify("e.g. this"), TextKind::Plain);
        assert_eq!(
            link_url("github.com/zed-industries/zed").as_deref(),
            Some("https://github.com/zed-industries/zed")
        );
        assert_eq!(link_url("http://"), None);
    }

    #[test]
    fn day_sections_and_copied_at_use_local_calendar_days() {
        // 设计稿按「今天 / 昨天 / 日期」分组；按 24 小时滑动窗口算会把昨晚 23 点算成「今天」。
        let clock = Clock::now();
        let at_1343 = clock.today_start_ms + (13 * 60 + 43) * 60_000;
        assert_eq!(day_label(clock, at_1343), "今天");
        assert_eq!(copied_at(clock, at_1343), "今天 13:43");
        let last_night = clock.today_start_ms - 60_000;
        assert_eq!(day_label(clock, last_night), "昨天");
        assert_eq!(copied_at(clock, last_night), "昨天 23:59");
        let older = clock.today_start_ms - 3 * 86_400_000 + 3_600_000;
        assert!(
            day_label(clock, older).ends_with('日'),
            "{}",
            day_label(clock, older)
        );
    }

    #[test]
    fn formats_counts_and_relative_time() {
        assert_eq!(thousands(3482), "3,482");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(ago(2 * 60_000 + 5), "2 分钟前");
        assert_eq!(ago(2 * 86_400_000), "2 天前");
    }

    #[test]
    fn english_days_and_relative_time() {
        // 英文界面的分组名（也是分组键）和时间要是地道英文，而不是夹着中文的半成品。
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let clock = Clock::now();
            let at_1343 = clock.today_start_ms + (13 * 60 + 43) * 60_000;
            assert_eq!(day_label(clock, at_1343), "Today");
            assert_eq!(copied_at(clock, at_1343), "Today at 13:43");
            let last_night = clock.today_start_ms - 60_000;
            assert_eq!(copied_at(clock, last_night), "Yesterday at 23:59");
            let older = day_label(clock, clock.today_start_ms - 3 * 86_400_000 + 3_600_000);
            assert!(
                MONTHS.iter().any(|month| older.starts_with(month)),
                "{older}"
            );
            assert_eq!(ago(5_000), "Just now");
            assert_eq!(ago(2 * 60_000 + 5), "2 min ago");
            assert_eq!(ago(86_400_000), "1 day ago");
            assert_eq!(ago(2 * 86_400_000), "2 days ago");
            assert_eq!(files_label(1), "1 file");
            assert_eq!(files_label(1200), "1,200 files");
        });
    }
}
