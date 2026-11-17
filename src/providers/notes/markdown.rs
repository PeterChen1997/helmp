//! 便签正文的轻量 Markdown 切片：只认标题、列表、复选框、粗体 / 斜体、行内代码和链接。
//!
//! 正文始终按 Markdown 原文存；这里只算出每段该用什么样式，偏移用 UTF-16（与 `NSString` 一致），
//! 原生编辑器据此给 `NSTextStorage` 上属性：光标所在行露出标记，其余行把标记藏起来（所见即所得）。

use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// `#`、`**`、`` ` ``、`[`、`](url)` 这类语法符号：光标不在这一行时隐藏。
    Marker,
    Heading(u8),
    Bold,
    Italic,
    Code,
    Link(String),
    /// 复选框的 `[ ]` / `[x]`：光标不在这一行时画成勾选框。
    TaskBox { checked: bool },
    /// 已勾选任务的正文：划掉、变淡。
    Done,
    /// 无序列表的 `-` / `*` / `+`：光标不在这一行时画成圆点。
    Bullet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// UTF-16 偏移。
    pub range: Range<usize>,
    pub kind: Kind,
    /// 所在行号（从 0 起），用来判断光标是否在这一行。
    pub line: usize,
}

/// 把全文切成带样式的片段。
pub fn parse(text: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut offset = 0;
    for (line_no, line) in text.split('\n').enumerate() {
        parse_line(line, offset, line_no, &mut spans);
        offset += utf16_len(line) + 1;
    }
    spans
}

/// 每行（含末尾换行前）的 UTF-16 区间，用来把光标位置换算成行号。
pub fn line_ranges(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut offset = 0;
    for line in text.split('\n') {
        let len = utf16_len(line);
        out.push(offset..offset + len);
        offset += len + 1;
    }
    out
}

/// 去掉语法符号后的一行文字（耳标竖排字、卡片标题用）。
pub fn plain_line(line: &str) -> String {
    let mut spans = Vec::new();
    parse_line(line, 0, 0, &mut spans);
    let hidden: Vec<Range<usize>> = spans
        .iter()
        .filter(|s| matches!(s.kind, Kind::Marker | Kind::TaskBox { .. } | Kind::Bullet))
        .map(|s| s.range.clone())
        .collect();
    let mut out = String::new();
    let mut pos = 0;
    for ch in line.chars() {
        if !hidden.iter().any(|r| r.contains(&pos)) {
            out.push(ch);
        }
        pos += ch.len_utf16();
    }
    out.trim().to_string()
}

/// 正文里用到了哪几类语法（埋点用，只发类别名，不发内容）。
pub fn used_kinds(text: &str) -> Vec<&'static str> {
    let spans = parse(text);
    let mut out = Vec::new();
    let mut push = |name: &'static str| {
        if !out.contains(&name) {
            out.push(name);
        }
    };
    for span in &spans {
        match span.kind {
            Kind::Heading(_) => push("heading"),
            Kind::Bold => push("bold"),
            Kind::Italic => push("italic"),
            Kind::Code => push("code"),
            Kind::Link(_) => push("link"),
            Kind::TaskBox { .. } => push("task"),
            Kind::Bullet => push("list"),
            Kind::Marker | Kind::Done => {}
        }
    }
    out.sort_unstable();
    out
}

/// 点勾选框时要改写的那个字符（`[ ]` 中间的空格或 `x`）及替换成什么。
pub fn toggle_task(span: &Span) -> Option<(usize, &'static str)> {
    match span.kind {
        Kind::TaskBox { checked } => Some((span.range.start + 1, if checked { " " } else { "x" })),
        _ => None,
    }
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn parse_line(line: &str, base: usize, line_no: usize, out: &mut Vec<Span>) {
    let chars: Vec<char> = line.chars().collect();
    // u16[i] = 第 i 个字符起点的 UTF-16 偏移。
    let mut u16 = Vec::with_capacity(chars.len() + 1);
    let mut acc = base;
    for ch in &chars {
        u16.push(acc);
        acc += ch.len_utf16();
    }
    u16.push(acc);
    let mut push = |from: usize, to: usize, kind: Kind| {
        if from < to {
            out.push(Span {
                range: u16[from]..u16[to],
                kind,
                line: line_no,
            });
        }
    };

    let indent = chars.iter().take_while(|c| **c == ' ' || **c == '\t').count();
    let rest = &chars[indent..];
    let mut body_start = indent;

    let hashes = rest.iter().take_while(|c| **c == '#').count();
    if (1..=6).contains(&hashes) && indent == 0 && rest.get(hashes) == Some(&' ') {
        push(0, hashes + 1, Kind::Marker);
        push(hashes + 1, chars.len(), Kind::Heading(u8::try_from(hashes).unwrap_or(6)));
        body_start = hashes + 1;
    } else if matches!(rest.first(), Some('-' | '*' | '+')) && rest.get(1) == Some(&' ') {
        let is_task = rest.get(2) == Some(&'[')
            && matches!(rest.get(3), Some(' ' | 'x' | 'X'))
            && rest.get(4) == Some(&']')
            && matches!(rest.get(5), None | Some(' '));
        if is_task {
            let checked = matches!(rest.get(3), Some('x' | 'X'));
            push(indent, indent + 2, Kind::Marker);
            push(indent + 2, indent + 5, Kind::TaskBox { checked });
            body_start = (indent + 6).min(chars.len());
            if checked {
                push(body_start, chars.len(), Kind::Done);
            }
        } else {
            push(indent, indent + 1, Kind::Bullet);
            body_start = indent + 2;
        }
    }
    inline(&chars, body_start, chars.len(), &mut push);
}

/// 行内语法：代码 → 链接 → 粗体 → 斜体，从左到右；粗体 / 斜体里还能嵌其他行内语法。
fn inline(chars: &[char], from: usize, to: usize, push: &mut impl FnMut(usize, usize, Kind)) {
    let mut i = from;
    while i < to {
        let c = chars[i];
        if c == '`'
            && let Some(end) = find(chars, i + 1, to, &['`'])
            && end > i + 1
        {
            push(i, i + 1, Kind::Marker);
            push(i + 1, end, Kind::Code);
            push(end, end + 1, Kind::Marker);
            i = end + 1;
            continue;
        } else if c == '[' {
            if let Some(close) = find(chars, i + 1, to, &[']'])
                && close > i + 1
                && chars.get(close + 1) == Some(&'(')
                && let Some(paren) = find(chars, close + 2, to, &[')'])
                && paren > close + 2
            {
                let url: String = chars[close + 2..paren].iter().collect();
                push(i, i + 1, Kind::Marker);
                push(i + 1, close, Kind::Link(url));
                push(close, paren + 1, Kind::Marker);
                i = paren + 1;
                continue;
            }
        } else if (c == '*' || c == '_') && chars.get(i + 1) == Some(&c) {
            let delim = [c, c];
            if opens(chars, i, 2, to)
                && let Some(end) = find(chars, i + 2, to, &delim)
                && end > i + 2
            {
                push(i, i + 2, Kind::Marker);
                push(i + 2, end, Kind::Bold);
                inline(chars, i + 2, end, push);
                push(end, end + 2, Kind::Marker);
                i = end + 2;
                continue;
            }
        } else if (c == '*' || c == '_')
            && opens(chars, i, 1, to)
            && let Some(end) = find_single(chars, i + 1, to, c)
            && end > i + 1
        {
            push(i, i + 1, Kind::Marker);
            push(i + 1, end, Kind::Italic);
            inline(chars, i + 1, end, push);
            push(end, end + 1, Kind::Marker);
            i = end + 1;
            continue;
        }
        i += 1;
    }
}

/// 开头的定界符后面紧跟非空白；`_` 前面还不能是字母数字（`snake_case` 不算斜体）。
fn opens(chars: &[char], i: usize, width: usize, to: usize) -> bool {
    let next = i + width;
    next < to
        && !chars[next].is_whitespace()
        && (chars[i] != '_' || i == 0 || !chars[i - 1].is_alphanumeric())
}

fn find(chars: &[char], from: usize, to: usize, pat: &[char]) -> Option<usize> {
    (from..to).find(|&j| j + pat.len() <= to && chars[j..j + pat.len()] == *pat)
}

/// 单个 `*` / `_` 的收尾：跳过成对的 `**`，收尾前不能是空白。
fn find_single(chars: &[char], from: usize, to: usize, c: char) -> Option<usize> {
    let mut j = from;
    while j < to {
        if chars[j] == c {
            if chars.get(j + 1) == Some(&c) {
                j += 2;
                continue;
            }
            if !chars[j - 1].is_whitespace() {
                return Some(j);
            }
        }
        j += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(String, Kind)> {
        let units: Vec<u16> = text.encode_utf16().collect();
        parse(text)
            .into_iter()
            .map(|s| (String::from_utf16_lossy(&units[s.range.clone()]), s.kind))
            .collect()
    }

    #[test]
    fn headings_lists_and_tasks_are_recognised_per_line() {
        // 设计稿里要求支持的块级语法：标题、无序列表、复选框（勾选的正文要划掉）。
        let got = kinds("## 发版检查\n- 买菜\n- [ ] clippy\n- [x] 测试");
        assert!(got.contains(&("## ".into(), Kind::Marker)));
        assert!(got.contains(&("发版检查".into(), Kind::Heading(2))));
        assert!(got.contains(&("-".into(), Kind::Bullet)));
        assert!(got.contains(&("[ ]".into(), Kind::TaskBox { checked: false })));
        assert!(got.contains(&("[x]".into(), Kind::TaskBox { checked: true })));
        assert!(got.contains(&("测试".into(), Kind::Done)));
        assert!(!got.iter().any(|(t, k)| t == "clippy" && *k == Kind::Done));
    }

    #[test]
    fn inline_styles_and_links_mark_their_delimiters() {
        let got = kinds("用 **粗体**、*斜体*、`cargo test` 和 [落地页](https://helmp.app)");
        assert!(got.contains(&("粗体".into(), Kind::Bold)));
        assert!(got.contains(&("斜体".into(), Kind::Italic)));
        assert!(got.contains(&("cargo test".into(), Kind::Code)));
        assert!(got.contains(&("落地页".into(), Kind::Link("https://helmp.app".into()))));
        assert!(got.contains(&("](https://helmp.app)".into(), Kind::Marker)));
        let markers = got.iter().filter(|(_, k)| *k == Kind::Marker).count();
        assert_eq!(markers, 2 + 2 + 2 + 2, "每种行内语法前后各一个定界符");
    }

    #[test]
    fn plain_text_that_looks_like_syntax_is_left_alone() {
        // 误判会把用户的正文藏掉：下划线变量名、乘号、没闭合的符号、#号后没空格都不是语法。
        for text in ["snake_case_name", "3 * 4 * 5", "**没闭合", "#话题", "a ` b", "[x](", "-不是列表"] {
            assert!(
                kinds(text).iter().all(|(_, k)| !matches!(k, Kind::Marker | Kind::Bold | Kind::Italic | Kind::Bullet)),
                "{text}: {:?}",
                kinds(text)
            );
        }
    }

    #[test]
    fn offsets_are_utf16_so_emoji_and_cjk_line_up_with_nsstring() {
        // NSTextStorage 用 UTF-16 下标：😀 占两个单位，算错就会把样式套到相邻的字上。
        let text = "😀 **好**\n- [ ] 下一行";
        let spans = parse(text);
        let bold = spans.iter().find(|s| s.kind == Kind::Bold).unwrap();
        assert_eq!(bold.range, 5..6);
        let task = spans.iter().find(|s| matches!(s.kind, Kind::TaskBox { .. })).unwrap();
        assert_eq!(task.line, 1);
        assert_eq!(task.range, 11..14);
        assert_eq!(line_ranges(text), vec![0..8, 9..18]);
    }

    #[test]
    fn plain_line_strips_markers_for_tab_and_title() {
        // 耳标竖排字和卡片标题不能显示成「#」「-」「[」。
        assert_eq!(plain_line("## 发版检查"), "发版检查");
        assert_eq!(plain_line("- [x] **买菜**"), "买菜");
        assert_eq!(plain_line("[落地页](https://a.b) 更新"), "落地页 更新");
        assert_eq!(plain_line("普通一行"), "普通一行");
    }

    #[test]
    fn toggling_a_task_rewrites_only_the_box_character() {
        let text = "- [ ] a\n- [x] b";
        let spans = parse(text);
        let boxes: Vec<_> = spans.iter().filter_map(toggle_task).collect();
        assert_eq!(boxes, vec![(3, "x"), (11, " ")]);
    }

    #[test]
    fn used_kinds_reports_categories_only() {
        assert_eq!(used_kinds("纯文本"), Vec::<&str>::new());
        assert_eq!(
            used_kinds("# 标题\n- [ ] 任务 **重点** [链接](https://x.y)"),
            vec!["bold", "heading", "link", "task"]
        );
    }
}
