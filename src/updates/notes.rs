//! 更新说明的显示格式。
//!
//! 清单里的 `notes` 原样取自 CHANGELOG（Markdown：`###` 小标题、`-` 列表、`<kbd>`、`` ` ``、`**`、链接）。
//! 设置 › 关于里没有 Markdown 渲染器，原文直接贴出来全是 `### Added`、`<kbd>⇧⌘1</kbd>` 这样的符号，
//! 所以在这里拆成「小标题 / 列表项 / 段落」，并去掉行内标记，只留文字。

/// 一块要显示的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(String),
    Bullet(String),
    Paragraph(String),
}

/// 把 Markdown 更新说明拆成块。空行分隔段落；列表项的续行（缩进或紧跟的非空行）并进同一项。
pub fn blocks(markdown: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    // 上一行是否还能接续（列表项或段落的后续行）。
    let mut open = false;
    for raw in markdown.lines() {
        let line = raw.trim();
        if line.is_empty() {
            open = false;
            continue;
        }
        if let Some(title) = heading(line) {
            out.push(Block::Heading(inline(title)));
            open = false;
            continue;
        }
        if let Some(item) = bullet(line) {
            out.push(Block::Bullet(inline(item)));
            open = true;
            continue;
        }
        let text = inline(line);
        match out.last_mut() {
            Some(Block::Bullet(prev) | Block::Paragraph(prev)) if open => {
                join(prev, &text);
            }
            _ => {
                out.push(Block::Paragraph(text));
                open = true;
            }
        }
    }
    out
}

fn heading(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches('#');
    (rest.len() < line.len() && rest.starts_with(' ')).then(|| rest.trim())
}

fn bullet(line: &str) -> Option<&str> {
    ["- ", "* ", "+ "]
        .iter()
        .find_map(|marker| line.strip_prefix(marker))
        .map(str::trim)
}

/// 续行接到上一行：中文之间不补空格，其余补一个。
fn join(prev: &mut String, next: &str) {
    let cjk = |c: char| matches!(c as u32, 0x2E80..=0x9FFF | 0xFF00..=0xFFEF);
    let glue = prev.chars().last().is_some_and(cjk) && next.chars().next().is_some_and(cjk);
    if !glue {
        prev.push(' ');
    }
    prev.push_str(next);
}

/// 去掉行内标记：`<kbd>x</kbd>` 等 HTML 标签、`**粗体**`、`` `代码` ``，`[文字](链接)` 只留文字。
fn inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
            && is_tag(&rest[1..end])
        {
            rest = &rest[end + 1..];
            continue;
        }
        if c == '['
            && let Some(close) = rest.find("](")
            && let Some(paren) = rest[close..].find(')')
        {
            out.push_str(&rest[1..close]);
            rest = &rest[close + paren + 1..];
            continue;
        }
        if rest.starts_with("**") {
            rest = &rest[2..];
            continue;
        }
        if c == '`' {
            rest = &rest[1..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// `<kbd>`、`</kbd>`、`<br/>` 之类：字母开头的标签名。别把「a < b > c」这种正文当标签吞掉。
fn is_tag(inner: &str) -> bool {
    let name = inner.trim_start_matches('/').trim_end_matches('/').trim();
    !name.is_empty() && name.len() <= 10 && name.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changelog_notes_become_headings_and_bullets_without_markup() {
        // 关于页没有 Markdown 渲染器：原样显示时用户看到的是 `### Added`、`<kbd>⇧⌘1</kbd>`（真机上发生过）。
        let notes = "### Added\n\n- Snipaste-style screenshots. \"Screenshot\" (<kbd>⇧⌘1</kbd>) uses `screencapture`.\n- See [docs/updates.md](docs/updates.md) and **this**.\n\n### Fixed\n\n- Results no longer vanish.";
        assert_eq!(
            blocks(notes),
            vec![
                Block::Heading("Added".into()),
                Block::Bullet(
                    "Snipaste-style screenshots. \"Screenshot\" (⇧⌘1) uses screencapture.".into()
                ),
                Block::Bullet("See docs/updates.md and this.".into()),
                Block::Heading("Fixed".into()),
                Block::Bullet("Results no longer vanish.".into()),
            ]
        );
    }

    #[test]
    fn chinese_notes_and_continuation_lines() {
        let notes = "#### 新增\n\n- 第一行\n  接着写\n- 第二项\nsecond line\n\n收尾段落。";
        assert_eq!(
            blocks(notes),
            vec![
                Block::Heading("新增".into()),
                Block::Bullet("第一行接着写".into()),
                Block::Bullet("第二项 second line".into()),
                Block::Paragraph("收尾段落。".into()),
            ]
        );
    }

    #[test]
    fn angle_brackets_in_prose_are_kept() {
        // 只去掉像标签的东西；正文里的比较符号不能被吞。
        assert_eq!(inline("a < b and c > d"), "a < b and c > d");
        assert_eq!(inline("x<br/>y"), "xy");
    }
}
