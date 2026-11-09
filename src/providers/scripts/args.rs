//! 脚本参数：表单里每个参数一个字段，初值从查询文本里取。约定：
//! 查询以脚本标题（或 packageName）开头时，余下文本就是参数；
//! 多个参数按空白切分，最后一个参数吃掉剩余全部文本。
//! 执行时以表单值为准（`from_values`），同样做必填、可选项与 URL 编码检查。

use super::header::{ArgKind, Argument, DropdownOption};
use crate::i18n::trf;

/// 参数不满足声明时的说明，用于 subtitle 和执行时的提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgError {
    /// 必填参数缺失，内容是它们的界面名（`Argument::label`）。
    Missing(Vec<String>),
    /// dropdown 参数的值不在可选项里。
    InvalidChoice {
        placeholder: String,
        given: String,
        options: Vec<String>,
    },
}

impl ArgError {
    pub fn describe(&self) -> String {
        match self {
            Self::Missing(placeholders) => trf!(
                "需要参数：{}",
                "Missing arguments: {}",
                placeholders.join(crate::i18n::tr("、", ", "))
            ),
            Self::InvalidChoice {
                placeholder,
                given,
                options,
            } => trf!(
                "参数「{placeholder}」不能是 {given}，可选：{}",
                "“{placeholder}” can’t be {given}. Choose from: {}",
                options.join(" / ")
            ),
        }
    }
}

/// 按空白把 `text` 切成最多 `count` 段，最后一段保留剩余文本（含内部空白）。
pub fn split(text: &str, count: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() && pieces.len() + 1 < count {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let (piece, tail) = rest.split_at(end);
        pieces.push(piece.to_string());
        rest = tail.trim_start();
    }
    if !rest.is_empty() && count > 0 {
        pieces.push(rest.to_string());
    }
    pieces
}

/// 表单初值：查询里的参数文本按位置切开；dropdown 认 title 或 value，给出 value，认不出给空串
/// （表单会落到第一个选项）。不做必填检查——那是执行时的事。
pub fn initial_values(arguments: &[Argument], text: &str) -> Vec<String> {
    let pieces = split(text, arguments.len());
    arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| {
            let raw = pieces.get(index).map(String::as_str).unwrap_or_default();
            match &argument.kind {
                ArgKind::Dropdown(options) => find_option(options, raw)
                    .map(|option| option.value.clone())
                    .unwrap_or_default(),
                ArgKind::Text | ArgKind::Password => raw.to_string(),
            }
        })
        .collect()
}

fn find_option<'a>(options: &'a [DropdownOption], raw: &str) -> Option<&'a DropdownOption> {
    options
        .iter()
        .find(|o| o.value.eq_ignore_ascii_case(raw) || o.title.eq_ignore_ascii_case(raw))
}

/// 按位置给出的参数值（表单值，或从查询切出的片段）→ 传给脚本的值。空串视为没填。
pub fn from_values(arguments: &[Argument], given: &[String]) -> Result<Vec<String>, ArgError> {
    let mut values = Vec::with_capacity(arguments.len());
    let mut missing = Vec::new();
    for (index, argument) in arguments.iter().enumerate() {
        let Some(raw) = given.get(index).filter(|raw| !raw.is_empty()) else {
            if !argument.optional {
                missing.push(argument.label(index + 1));
            }
            values.push(String::new());
            continue;
        };
        let value = match &argument.kind {
            ArgKind::Dropdown(options) => find_option(options, raw)
                .map(|o| o.value.clone())
                .ok_or_else(|| ArgError::InvalidChoice {
                    placeholder: argument.label(index + 1),
                    given: raw.clone(),
                    options: options.iter().map(|o| o.title.clone()).collect(),
                })?,
            ArgKind::Text | ArgKind::Password => raw.clone(),
        };
        values.push(if argument.percent_encoded {
            percent_encode(&value)
        } else {
            value
        });
    }
    if missing.is_empty() {
        Ok(values)
    } else {
        Err(ArgError::Missing(missing))
    }
}

/// 查询以「空白 + !」结尾表示确认执行，返回去掉标记后的查询。
pub fn strip_confirmation(query: &str) -> Option<&str> {
    let rest = query.trim_end().strip_suffix('!')?;
    rest.ends_with(char::is_whitespace).then(|| rest.trim_end())
}

/// 大小写不敏感地去掉前缀；`prefix` 之后必须是空白或结尾，避免 `ipx` 被当成 `ip` + 参数 `x`。
pub fn strip_name<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    if name.is_empty() {
        return None;
    }
    let mut chars = text.char_indices();
    for expected in name.chars() {
        let (_, actual) = chars.next()?;
        if !actual.to_lowercase().eq(expected.to_lowercase()) {
            return None;
        }
    }
    let rest = chars.next().map_or("", |(index, _)| &text[index..]);
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
}

/// RFC 3986：保留 unreserved 字符，其余按 UTF-8 字节编码为 `%XX`。
pub fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一行参数文本 → 传给脚本的值（「标题 + 参数」直接回车、命令行 run 走的就是这条路）。
    fn resolve(arguments: &[Argument], text: &str) -> Result<Vec<String>, ArgError> {
        from_values(arguments, &split(text, arguments.len()))
    }

    fn text(placeholder: &str, optional: bool) -> Argument {
        Argument {
            kind: ArgKind::Text,
            placeholder: placeholder.into(),
            optional,
            percent_encoded: false,
        }
    }

    #[test]
    fn last_argument_takes_the_rest() {
        // 「翻译 en 你好 世界」：第一个参数是语言，第二个是整句；整句被切断就翻错了。
        assert_eq!(split("en  你好 世界 ", 2), vec!["en", "你好 世界"]);
        assert_eq!(split("a b c d", 3), vec!["a", "b", "c d"]);
        assert_eq!(split("only one", 1), vec!["only one"]);
        assert_eq!(split("   ", 2), Vec::<String>::new());
        assert_eq!(split("x", 0), Vec::<String>::new());
    }

    #[test]
    fn missing_required_arguments_are_reported_by_placeholder() {
        // 缺参数时脚本拿到空串会做错事（比如搜索空字符串），必须拦下并告诉用户缺什么。
        let args = [text("from", false), text("to", false), text("date", true)];
        assert_eq!(
            resolve(&args, "北京"),
            Err(ArgError::Missing(vec!["to".into()]))
        );
        assert_eq!(
            resolve(&args, ""),
            Err(ArgError::Missing(vec!["from".into(), "to".into()]))
        );
        assert_eq!(
            resolve(&args, "北京 上海"),
            Ok(vec!["北京".into(), "上海".into(), String::new()])
        );
    }

    #[test]
    fn dropdown_accepts_title_or_value_and_passes_value() {
        // 用户看到的是 title，脚本要的是 value；传错会让脚本收到未知值。
        let args = [Argument {
            kind: ArgKind::Dropdown(vec![
                DropdownOption {
                    title: "Economy".into(),
                    value: "e".into(),
                },
                DropdownOption {
                    title: "Business".into(),
                    value: "b".into(),
                },
            ]),
            placeholder: "class".into(),
            optional: false,
            percent_encoded: false,
        }];
        assert_eq!(resolve(&args, "business"), Ok(vec!["b".into()]));
        assert_eq!(resolve(&args, "e"), Ok(vec!["e".into()]));
        assert!(matches!(
            resolve(&args, "first"),
            Err(ArgError::InvalidChoice { .. })
        ));
    }

    #[test]
    fn form_initial_values_and_submitted_values() {
        // 表单初值来自「标题 + 余下文本」：dropdown 要预选到对应项（存 value），认不出就留空让表单落到第一项；
        // 提交时空字段等于没填，必填项照样要拦下。
        let args = [
            text("from", false),
            Argument {
                kind: ArgKind::Dropdown(vec![DropdownOption {
                    title: "Business".into(),
                    value: "b".into(),
                }]),
                placeholder: "class".into(),
                optional: true,
                percent_encoded: false,
            },
        ];
        assert_eq!(
            initial_values(&args, "北京 business"),
            vec!["北京".to_string(), "b".to_string()]
        );
        assert_eq!(
            initial_values(&args, "北京 头等舱"),
            vec!["北京".to_string(), String::new()]
        );
        assert_eq!(
            initial_values(&args, ""),
            vec![String::new(), String::new()]
        );
        assert_eq!(
            from_values(&args, &[String::new(), "b".into()]),
            Err(ArgError::Missing(vec!["from".into()]))
        );
        assert_eq!(
            from_values(&args, &["上海 浦东".into(), String::new()]),
            Ok(vec!["上海 浦东".into(), String::new()])
        );
    }

    #[test]
    fn percent_encoded_arguments_are_url_safe() {
        // 这类参数会被直接拼进 URL，未编码的空格/中文会让链接断掉。
        let mut arg = text("q", false);
        arg.percent_encoded = true;
        assert_eq!(
            resolve(&[arg], "a b&中"),
            Ok(vec!["a%20b%26%E4%B8%AD".into()])
        );
    }

    #[test]
    fn confirmation_marker_needs_preceding_space() {
        assert_eq!(strip_confirmation("清空废纸篓 !"), Some("清空废纸篓"));
        assert_eq!(strip_confirmation("say hi  ! "), Some("say hi"));
        assert_eq!(strip_confirmation("hello!"), None);
        assert_eq!(strip_confirmation("hello"), None);
    }

    #[test]
    fn name_prefix_is_case_insensitive_and_word_bounded() {
        assert_eq!(
            strip_name("Search Flights 北京 上海", "search flights"),
            Some("北京 上海")
        );
        assert_eq!(strip_name("当前时间", "当前时间"), Some(""));
        // 不在词边界的前缀不算：否则查询 `ipx` 会被当成脚本 `ip` 加参数 `x`。
        assert_eq!(strip_name("ipx", "ip"), None);
        assert_eq!(strip_name("i", "ip"), None);
    }
}
