//! 从查询文本里取脚本参数。启动器没有参数表单，约定：
//! 查询以脚本标题（或 packageName）开头时，余下文本就是参数；
//! 多个参数按空白切分，最后一个参数吃掉剩余全部文本。

use super::header::{ArgKind, Argument};

/// 参数不满足声明时的说明，用于 subtitle 和执行时的提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgError {
    /// 必填参数缺失，内容是它们的 placeholder。
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
            Self::Missing(placeholders) => format!("需要参数：{}", placeholders.join("、")),
            Self::InvalidChoice {
                placeholder,
                given,
                options,
            } => format!(
                "参数「{placeholder}」不能是 {given}，可选：{}",
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

/// 把参数文本对应到声明的参数上，返回按位置传给脚本的值（可选参数缺省传空串，与 Raycast 一致）。
pub fn resolve(arguments: &[Argument], text: &str) -> Result<Vec<String>, ArgError> {
    let pieces = split(text, arguments.len());
    let mut values = Vec::with_capacity(arguments.len());
    let mut missing = Vec::new();
    for (index, argument) in arguments.iter().enumerate() {
        let Some(raw) = pieces.get(index) else {
            if !argument.optional {
                missing.push(argument.placeholder.clone());
            }
            values.push(String::new());
            continue;
        };
        let value = match &argument.kind {
            ArgKind::Dropdown(options) => options
                .iter()
                .find(|o| o.value.eq_ignore_ascii_case(raw) || o.title.eq_ignore_ascii_case(raw))
                .map(|o| o.value.clone())
                .ok_or_else(|| ArgError::InvalidChoice {
                    placeholder: argument.placeholder.clone(),
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
    use crate::providers::scripts::header::DropdownOption;

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
