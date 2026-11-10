//! 片段占位符展开（纯函数，单测覆盖）。语法照 Raycast，以后第三方清单也用同一套：
//!
//! - `{date}` `{time}` `{datetime}`：`2026-11-10`、`14:05`、`2026-11-10 14:05`；
//!   `{date format="yyyy/MM/dd HH:mm"}` 自定格式（yyyy yy MM M dd d HH H mm ss）。
//! - `{clipboard}`：当前剪贴板文字；拿不到（演练模式没给夹具）时原样保留。
//! - `{uuid}`：随机 UUID v4。
//! - `{cursor}`：粘贴后光标停在这里（第一个有效，展开后去掉）。
//! - `{argument name="收件人" default="…"}`：执行前出表单填写，同名的共用一个值。
//! - `{snippet name="签名"}`：插入另一个片段（最多嵌套 4 层）。
//! - 修饰符：`{clipboard | trim | uppercase}`，支持 trim、uppercase、lowercase、percent-encode、json-stringify。
//!
//! 认不出的 `{…}` 原样保留（正文里本来就可能有花括号，如代码片段）。

use std::collections::BTreeMap;

/// 展开时的「现在」（本地时间）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

pub struct Ctx<'a> {
    pub now: Now,
    pub clipboard: Option<&'a str>,
    /// 表单里填的参数值。
    pub args: &'a BTreeMap<String, String>,
    /// 按名字取另一个片段的正文。
    pub snippet: &'a dyn Fn(&str) -> Option<String>,
    pub uuid: &'a dyn Fn() -> String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    /// `{cursor}` 后面还有几个字符：粘贴后按这么多次 ← 把光标移回去。
    pub cursor_back: Option<usize>,
}

/// 片段里的一个表单参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argument {
    pub name: String,
    pub default: Option<String>,
}

/// 一个 `{…}` 占位符。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Placeholder {
    name: String,
    attrs: BTreeMap<String, String>,
    modifiers: Vec<String>,
}

const NAMES: [&str; 8] = [
    "clipboard", "date", "time", "datetime", "uuid", "cursor", "argument", "snippet",
];
const MAX_DEPTH: usize = 4;

/// 片段需要的参数（按第一次出现的顺序、按名字去重）。没写 name 的叫 `argument`。
pub fn arguments(content: &str) -> Vec<Argument> {
    let mut out: Vec<Argument> = Vec::new();
    for piece in split(content) {
        if let Piece::Placeholder(p) = piece
            && p.name == "argument"
        {
            let name = argument_name(&p);
            if !out.iter().any(|a| a.name == name) {
                out.push(Argument {
                    name,
                    default: p.attrs.get("default").cloned(),
                });
            }
        }
    }
    out
}

fn argument_name(p: &Placeholder) -> String {
    p.attrs
        .get("name")
        .filter(|name| !name.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "argument".into())
}

pub fn expand(content: &str, ctx: &Ctx) -> Expanded {
    let mut text = String::new();
    let mut cursor: Option<usize> = None;
    expand_into(content, ctx, 0, &mut text, &mut cursor);
    let cursor_back = cursor.map(|at| text[at..].chars().count());
    Expanded { text, cursor_back }
}

fn expand_into(content: &str, ctx: &Ctx, depth: usize, out: &mut String, cursor: &mut Option<usize>) {
    for piece in split(content) {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Placeholder(p) => {
                if p.name == "cursor" {
                    if cursor.is_none() {
                        *cursor = Some(out.len());
                    }
                    continue;
                }
                let value = match p.name.as_str() {
                    "clipboard" => ctx.clipboard.map(str::to_string),
                    "date" => Some(match p.attrs.get("format") {
                        Some(format) => format_date(ctx.now, format),
                        None => format_date(ctx.now, "yyyy-MM-dd"),
                    }),
                    "time" => Some(format_date(ctx.now, "HH:mm")),
                    "datetime" => Some(format_date(ctx.now, "yyyy-MM-dd HH:mm")),
                    "uuid" => Some((ctx.uuid)()),
                    "argument" => {
                        let name = argument_name(&p);
                        Some(
                            ctx.args
                                .get(&name)
                                .filter(|value| !value.is_empty())
                                .or(p.attrs.get("default"))
                                .cloned()
                                .unwrap_or_default(),
                        )
                    }
                    "snippet" if depth < MAX_DEPTH => {
                        p.attrs.get("name").and_then(|name| (ctx.snippet)(name)).map(|body| {
                            let mut nested = String::new();
                            // 被引用片段里的 {cursor} 不算（光标位置以外层为准）。
                            let mut ignored = Some(0);
                            expand_into(&body, ctx, depth + 1, &mut nested, &mut ignored);
                            nested
                        })
                    }
                    _ => None,
                };
                match value {
                    Some(value) => out.push_str(&apply_modifiers(value, &p.modifiers)),
                    None => out.push_str(&raw(&p)),
                }
            }
        }
    }
}

/// 认得但这次给不出值的占位符（如没有剪贴板）原样写回。
fn raw(p: &Placeholder) -> String {
    let mut text = format!("{{{}", p.name);
    for (key, value) in &p.attrs {
        text.push_str(&format!(" {key}=\"{value}\""));
    }
    for modifier in &p.modifiers {
        text.push_str(&format!(" | {modifier}"));
    }
    text.push('}');
    text
}

fn apply_modifiers(mut value: String, modifiers: &[String]) -> String {
    for modifier in modifiers {
        value = match modifier.as_str() {
            "trim" => value.trim().to_string(),
            "uppercase" => value.to_uppercase(),
            "lowercase" => value.to_lowercase(),
            "percent-encode" => percent_encode(&value),
            "json-stringify" => serde_json::to_string(&value).unwrap_or(value),
            _ => value,
        };
    }
    value
}

fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// 按 yyyy / yy / MM / M / dd / d / HH / H / mm / ss 格式化，其他字符原样。
pub fn format_date(now: Now, format: &str) -> String {
    const TOKENS: [&str; 10] = ["yyyy", "yy", "MM", "M", "dd", "d", "HH", "H", "mm", "ss"];
    let mut out = String::new();
    let mut rest = format;
    'outer: while !rest.is_empty() {
        for token in TOKENS {
            if let Some(tail) = rest.strip_prefix(token) {
                out.push_str(&match token {
                    "yyyy" => format!("{:04}", now.year),
                    "yy" => format!("{:02}", now.year.rem_euclid(100)),
                    "MM" => format!("{:02}", now.month),
                    "M" => now.month.to_string(),
                    "dd" => format!("{:02}", now.day),
                    "d" => now.day.to_string(),
                    "HH" => format!("{:02}", now.hour),
                    "H" => now.hour.to_string(),
                    "mm" => format!("{:02}", now.minute),
                    _ => format!("{:02}", now.second),
                });
                rest = tail;
                continue 'outer;
            }
        }
        let ch = rest.chars().next().unwrap_or_default();
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

enum Piece<'a> {
    Text(&'a str),
    Placeholder(Placeholder),
}

/// 切成普通文字和占位符。`{` 后面不是认识的占位符名时，这个 `{` 当普通文字。
fn split(content: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut search = 0;
    while let Some(open) = content[search..].find('{').map(|i| i + search) {
        let Some(close) = content[open..].find('}').map(|i| i + open) else {
            break;
        };
        match parse(&content[open + 1..close]) {
            Some(placeholder) => {
                if start < open {
                    pieces.push(Piece::Text(&content[start..open]));
                }
                pieces.push(Piece::Placeholder(placeholder));
                start = close + 1;
                search = start;
            }
            None => search = open + 1,
        }
    }
    if start < content.len() {
        pieces.push(Piece::Text(&content[start..]));
    }
    pieces
}

/// 解析 `name key="value" … | modifier | …`。名字不认识或属性写错返回 None。
fn parse(inner: &str) -> Option<Placeholder> {
    let mut parts = inner.split('|');
    let head = parts.next()?.trim();
    let modifiers: Vec<String> = parts.map(|m| m.trim().to_string()).collect();
    if modifiers.iter().any(String::is_empty) {
        return None;
    }
    let name_end = head.find(char::is_whitespace).unwrap_or(head.len());
    let name = &head[..name_end];
    if !NAMES.contains(&name) {
        return None;
    }
    let mut attrs = BTreeMap::new();
    let mut rest = head[name_end..].trim_start();
    while !rest.is_empty() {
        let eq = rest.find('=')?;
        let key = rest[..eq].trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            return None;
        }
        let after = rest[eq + 1..].trim_start().strip_prefix('"')?;
        let end = after.find('"')?;
        attrs.insert(key.to_string(), after[..end].to_string());
        rest = after[end + 1..].trim_start();
    }
    Some(Placeholder {
        name: name.to_string(),
        attrs,
        modifiers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Now = Now {
        year: 2026,
        month: 11,
        day: 10,
        hour: 9,
        minute: 5,
        second: 7,
    };

    fn run(content: &str, clipboard: Option<&str>, args: &[(&str, &str)]) -> Expanded {
        let args: BTreeMap<String, String> = args
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let snippet = |name: &str| (name == "签名").then(|| "— 陈 {date format=\"yyyy\"}".to_string());
        let uuid = || "00000000-0000-4000-8000-000000000000".to_string();
        expand(
            content,
            &Ctx {
                now: NOW,
                clipboard,
                args: &args,
                snippet: &snippet,
                uuid: &uuid,
            },
        )
    }

    #[test]
    fn dates_times_and_custom_formats() {
        // 日期占位符是片段最常见的用法（日报标题、文件名）；补零错了文件名排序就乱了。
        assert_eq!(run("{date} {time}", None, &[]).text, "2026-11-10 09:05");
        assert_eq!(run("{datetime}", None, &[]).text, "2026-11-10 09:05");
        assert_eq!(
            run("{date format=\"yy/M/d H:mm:ss\"}", None, &[]).text,
            "26/11/10 9:05:07"
        );
        assert_eq!(run("{date format=\"yyyy年MM月dd日\"}", None, &[]).text, "2026年11月10日");
    }

    #[test]
    fn clipboard_is_inserted_or_left_alone_when_unknown() {
        // 拿不到剪贴板（测试 / 演练）时不能悄悄换成空字符串，否则用户以为片段坏了。
        assert_eq!(run("引用：{clipboard}", Some("hi"), &[]).text, "引用：hi");
        assert_eq!(run("引用：{clipboard}", None, &[]).text, "引用：{clipboard}");
        assert_eq!(
            run("{clipboard | trim | uppercase}", Some("  ab c "), &[]).text,
            "AB C"
        );
        assert_eq!(
            run("q={clipboard | percent-encode}", Some("a b/中"), &[]).text,
            "q=a%20b%2F%E4%B8%AD"
        );
        assert_eq!(
            run("{clipboard | json-stringify}", Some("say \"hi\"\n"), &[]).text,
            "\"say \\\"hi\\\"\\n\""
        );
    }

    #[test]
    fn cursor_marks_where_the_caret_ends_up() {
        // {cursor} 要从正文里拿掉，并算出粘贴后要退回几个字符（按字符数，中文也是一个）。
        let out = run("<b>{cursor}</b>", None, &[]);
        assert_eq!(out.text, "<b></b>");
        assert_eq!(out.cursor_back, Some(4));
        let out = run("你好{cursor}世界{cursor}", None, &[]);
        assert_eq!(out.text, "你好世界");
        assert_eq!(out.cursor_back, Some(2), "只认第一个 {{cursor}}");
        assert_eq!(run("no cursor", None, &[]).cursor_back, None);
    }

    #[test]
    fn arguments_become_form_fields_and_fill_in() {
        // 带参数的片段先出表单：同名参数只问一次，没填时用默认值。
        let content = "Hi {argument name=\"收件人\"}, {argument name=\"问候\" default=\"早上好\"}! {argument name=\"收件人\"}";
        assert_eq!(
            arguments(content),
            vec![
                Argument { name: "收件人".into(), default: None },
                Argument { name: "问候".into(), default: Some("早上好".into()) },
            ]
        );
        assert_eq!(
            run(content, None, &[("收件人", "Ada")]).text,
            "Hi Ada, 早上好! Ada"
        );
        assert!(arguments("{date}").is_empty());
    }

    #[test]
    fn uuid_snippet_references_and_unknown_braces() {
        // 代码片段里的 {…} 不是占位符，必须原样保留；引用别的片段会再展开一层。
        assert_eq!(
            run("{uuid}", None, &[]).text,
            "00000000-0000-4000-8000-000000000000"
        );
        assert_eq!(run("fn main() { }", None, &[]).text, "fn main() { }");
        assert_eq!(run("{\"a\": {date}}", None, &[]).text, "{\"a\": 2026-11-10}");
        assert_eq!(run("{snippet name=\"签名\"}", None, &[]).text, "— 陈 2026");
        assert_eq!(run("{snippet name=\"没有\"}", None, &[]).text, "{snippet name=\"没有\"}");
        assert_eq!(run("{date | nope}", None, &[]).text, "2026-11-10", "不认识的修饰符跳过");
        assert_eq!(run("{date format=unquoted}", None, &[]).text, "{date format=unquoted}");
    }
}
