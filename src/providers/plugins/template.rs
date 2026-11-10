//! 清单插件共用的模板语法：`{{变量|过滤器|过滤器}}`。
//!
//! 变量：
//! - `{{query}}`：关键词后面输入的文字（`gh react` 里的 `react`）。
//! - `{{form.<id>}}`：表单字段的值。
//! - `{{pref.<id>}}`：插件偏好设置（secret 类型从钥匙串取）。
//! - `{{item.<key>}}`：列表命令里被选中的那一项（title / subtitle / arg / uid / url）。
//! - `{{response.<路径>}}`：http 动作的成功提示里，响应 JSON 的某个字段（`data.id`、`items.0.title`）。
//! - `{{clipboard}}`：剪贴板文字。只有清单声明了 `"permissions": ["clipboard.read"]` 才能用，
//!   而且只在用到时才读；演练模式（HELMP_DRY_RUN=1）下不读，换成 `[clipboard]`。
//! - `{{date}}`（`2026-11-10`）、`{{time}}`（`14:05`）、`{{date:%Y/%m/%d %H%M}}`（strftime 格式）。
//!
//! 过滤器：`url`（RFC 3986 百分号编码）、`json`（JSON 字符串转义，不带引号）、`trim`、`upper`、`lower`。
//! 不支持表达式和条件：模板只做替换，插件逻辑复杂了就用 script 动作。

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde_json::Value;

use crate::i18n::trf;

/// 模板里引用的一个值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Var {
    Query,
    Clipboard,
    /// `None` = 默认格式 `%Y-%m-%d`。
    Date(Option<String>),
    Time,
    Form(String),
    Pref(String),
    Item(String),
    Response(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    Url,
    Json,
    Trim,
    Upper,
    Lower,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Literal(String),
    Value { var: Var, filters: Vec<Filter> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    parts: Vec<Part>,
}

/// strftime 格式的长度上限（防止把整段文字当格式写进来）。
const MAX_DATE_FORMAT: usize = 64;

impl Template {
    /// 解析模板；错误信息直接展示给插件作者，所以要说清楚哪里错。
    pub fn parse(text: &str) -> Result<Self> {
        let mut parts = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find("{{") {
            if start > 0 {
                parts.push(Part::Literal(rest[..start].to_string()));
            }
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else {
                bail!(trf!(
                    "模板 {text:?} 里的 {{{{ 没有配对的 }}}}",
                    "Template {text:?} has a {{{{ without a matching }}}}"
                ));
            };
            parts.push(parse_expression(&after[..end])?);
            rest = &after[end + 2..];
        }
        if rest.contains("}}") {
            bail!(trf!(
                "模板 {text:?} 里有多余的 }}}}",
                "Template {text:?} has a stray }}}}"
            ));
        }
        if !rest.is_empty() {
            parts.push(Part::Literal(rest.to_string()));
        }
        Ok(Self { parts })
    }

    /// 模板里用到的变量（按出现顺序，可能重复）。
    pub fn vars(&self) -> impl Iterator<Item = &Var> {
        self.parts.iter().filter_map(|part| match part {
            Part::Value { var, .. } => Some(var),
            Part::Literal(_) => None,
        })
    }

    pub fn render(&self, values: &dyn Values) -> Result<String> {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Literal(text) => out.push_str(text),
                Part::Value { var, filters } => {
                    let mut value = values.value(var)?;
                    for filter in filters {
                        value = apply(*filter, &value);
                    }
                    out.push_str(&value);
                }
            }
        }
        Ok(out)
    }
}

fn parse_expression(expression: &str) -> Result<Part> {
    let mut pieces = expression.split('|').map(str::trim);
    let name = pieces.next().unwrap_or_default();
    let var = parse_var(name)?;
    let filters = pieces
        .map(|filter| match filter {
            "url" => Ok(Filter::Url),
            "json" => Ok(Filter::Json),
            "trim" => Ok(Filter::Trim),
            "upper" => Ok(Filter::Upper),
            "lower" => Ok(Filter::Lower),
            other => bail!(trf!(
                "不认识的过滤器 {other:?}（可用：url、json、trim、upper、lower）",
                "Unknown filter {other:?} (available: url, json, trim, upper, lower)"
            )),
        })
        .collect::<Result<_>>()?;
    Ok(Part::Value { var, filters })
}

fn parse_var(name: &str) -> Result<Var> {
    if let Some(format) = name.strip_prefix("date:") {
        if format.is_empty() || format.len() > MAX_DATE_FORMAT {
            bail!(trf!(
                "{{{{date:…}}}} 的格式要写成 strftime 格式，且不超过 {MAX_DATE_FORMAT} 个字符",
                "{{{{date:…}}}} needs a strftime format of at most {MAX_DATE_FORMAT} characters"
            ));
        }
        return Ok(Var::Date(Some(format.to_string())));
    }
    let keyed = |prefix: &str| {
        name.strip_prefix(prefix)
            .filter(|key| is_key(key))
            .map(str::to_string)
    };
    Ok(match name {
        "query" => Var::Query,
        "clipboard" => Var::Clipboard,
        "date" => Var::Date(None),
        "time" => Var::Time,
        _ => {
            if let Some(key) = keyed("form.") {
                Var::Form(key)
            } else if let Some(key) = keyed("pref.") {
                Var::Pref(key)
            } else if let Some(key) = keyed("item.") {
                Var::Item(key)
            } else if let Some(path) = name
                .strip_prefix("response.")
                .filter(|path| path.split('.').all(is_key))
            {
                Var::Response(path.to_string())
            } else {
                bail!(trf!(
                    "不认识的模板变量 {{{{{name}}}}}（可用：query、form.<id>、pref.<id>、item.<key>、response.<路径>、clipboard、date、time）",
                    "Unknown template variable {{{{{name}}}}} (available: query, form.<id>, pref.<id>, item.<key>, response.<path>, clipboard, date, time)"
                ));
            }
        }
    })
}

/// 字段 id：字母开头，字母数字、`_`、`-`。
pub fn is_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn apply(filter: Filter, value: &str) -> String {
    match filter {
        Filter::Url => crate::providers::scripts::args::percent_encode(value),
        Filter::Json => {
            let quoted = Value::String(value.to_string()).to_string();
            quoted[1..quoted.len() - 1].to_string()
        }
        Filter::Trim => value.trim().to_string(),
        Filter::Upper => value.to_uppercase(),
        Filter::Lower => value.to_lowercase(),
    }
}

/// 渲染时取值。
pub trait Values {
    fn value(&self, var: &Var) -> Result<String>;
}

/// 一次执行的全部输入。偏好设置和剪贴板用闭包按需取：用不到的密钥不读钥匙串，用不到的剪贴板不读。
pub struct Context<'a> {
    pub query: &'a str,
    pub form: &'a BTreeMap<String, String>,
    pub pref: &'a dyn Fn(&str) -> Result<String>,
    pub clipboard: &'a dyn Fn() -> Result<String>,
    pub item: Option<&'a BTreeMap<String, String>>,
    pub response: Option<&'a Value>,
    /// Unix 秒，`date` / `time` 用。
    pub now: i64,
}

impl Values for Context<'_> {
    fn value(&self, var: &Var) -> Result<String> {
        Ok(match var {
            Var::Query => self.query.to_string(),
            Var::Clipboard => (self.clipboard)()?,
            Var::Date(format) => strftime(self.now, format.as_deref().unwrap_or("%Y-%m-%d")),
            Var::Time => strftime(self.now, "%H:%M"),
            Var::Form(key) => self.form.get(key).cloned().unwrap_or_default(),
            Var::Pref(key) => (self.pref)(key)?,
            Var::Item(key) => self
                .item
                .and_then(|item| item.get(key))
                .cloned()
                .unwrap_or_default(),
            Var::Response(path) => match self.response.and_then(|value| json_path(value, path)) {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Null) | None => String::new(),
                Some(other) => other.to_string(),
            },
        })
    }
}

/// 简单的 JSON 路径：`code`、`data.id`、`items.0.title`；可以带 `$.` 前缀。
pub fn json_path<'v>(value: &'v Value, path: &str) -> Option<&'v Value> {
    let path = path.strip_prefix("$.").unwrap_or(path);
    path.split('.')
        .try_fold(value, |current, key| match current {
            Value::Array(items) => key.parse::<usize>().ok().and_then(|index| items.get(index)),
            Value::Object(map) => map.get(key),
            _ => None,
        })
}

/// 本地时区格式化（libc strftime），不为一个日期引入时间库。
pub fn strftime(unix: i64, format: &str) -> String {
    let Ok(c_format) = std::ffi::CString::new(format) else {
        return String::new();
    };
    let time: libc::time_t = unix;
    // SAFETY: tm 全零是合法初值；localtime_r 只写 tm；strftime 最多写 buf.len() 字节并返回写入长度。
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&time, &mut tm).is_null() {
            return String::new();
        }
        let mut buf = [0u8; 256];
        let written = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), c_format.as_ptr(), &tm);
        String::from_utf8_lossy(buf.get(..written).unwrap_or_default()).into_owned()
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or_default())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(text: &str, query: &str, form: &[(&str, &str)]) -> String {
        let form: BTreeMap<String, String> = form
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let pref = |key: &str| Ok(format!("<{key}>"));
        let clipboard = || Ok("clip".to_string());
        let context = Context {
            query,
            form: &form,
            pref: &pref,
            clipboard: &clipboard,
            item: None,
            response: None,
            now: 0,
        };
        Template::parse(text).unwrap().render(&context).unwrap()
    }

    #[test]
    fn substitutes_variables_and_keeps_literal_text() {
        // 插件作者写的模板就是最终发出去的 URL / 正文，替换错一个字符请求就发错地方。
        assert_eq!(
            render(
                "a {{query}} b {{form.x}} c {{pref.token}}",
                "q",
                &[("x", "X")]
            ),
            "a q b X c <token>"
        );
        assert_eq!(render("no vars", "q", &[]), "no vars");
        assert_eq!(render("{{ query | upper }}", "abc", &[]), "ABC");
        // 没填的表单字段渲染成空串，不是报错：可选字段很常见。
        assert_eq!(render("[{{form.missing}}]", "", &[]), "[]");
    }

    #[test]
    fn url_filter_encodes_everything_that_would_change_the_url() {
        // 不编码的话 `&`、`#`、空格、中文会截断或改写查询参数，用户搜「c++ a&b」就搜成了别的东西。
        assert_eq!(
            render("https://x.com/?q={{query|url}}", "c++ a&b #微信", &[]),
            "https://x.com/?q=c%2B%2B%20a%26b%20%23%E5%BE%AE%E4%BF%A1"
        );
    }

    #[test]
    fn json_filter_escapes_quotes_and_newlines() {
        // 文本正文里手拼 JSON 时，引号和换行不转义会让请求体变成坏 JSON（或被注入额外字段）。
        assert_eq!(
            render(
                r#"{"content":"{{form.c|json}}"}"#,
                "",
                &[("c", "say \"hi\"\nok")]
            ),
            r#"{"content":"say \"hi\"\nok"}"#
        );
        let parsed: Value = serde_json::from_str(&render(
            r#"{"c":"{{query|json}}"}"#,
            "a\"}, \"x\": \"y",
            &[],
        ))
        .unwrap();
        assert_eq!(parsed["c"], "a\"}, \"x\": \"y");
        assert!(parsed.get("x").is_none());
    }

    #[test]
    fn filters_chain_left_to_right() {
        assert_eq!(render("{{query|trim|url}}", "  a b ", &[]), "a%20b");
    }

    #[test]
    fn bad_templates_are_rejected_with_a_reason() {
        // 拼错变量名不能悄悄渲染成空串：用户会以为插件坏了却不知道为什么。
        for (text, needle) in [
            ("{{qurey}}", "qurey"),
            ("{{query|base64}}", "base64"),
            ("{{query", "{{"),
            ("x }} y", "}}"),
            ("{{form.}}", "form."),
            ("{{date:}}", "date"),
        ] {
            let err = crate::i18n::with_lang(crate::i18n::Lang::En, || {
                Template::parse(text).unwrap_err().to_string()
            });
            assert!(err.contains(needle), "{text}: {err}");
        }
    }

    #[test]
    fn lists_used_variables() {
        // 权限推导（剪贴板）和校验（表单字段是否存在）都靠它找出模板用到了什么。
        let template =
            Template::parse("{{clipboard}} {{form.a|url}} {{pref.b}} {{date:%Y}}").unwrap();
        let vars: Vec<_> = template.vars().cloned().collect();
        assert_eq!(
            vars,
            [
                Var::Clipboard,
                Var::Form("a".into()),
                Var::Pref("b".into()),
                Var::Date(Some("%Y".into()))
            ]
        );
    }

    #[test]
    fn json_paths_walk_objects_and_arrays() {
        let value: Value = serde_json::json!({"code": 0, "data": {"items": [{"t": "x"}]}});
        assert_eq!(json_path(&value, "code"), Some(&serde_json::json!(0)));
        assert_eq!(json_path(&value, "$.code"), Some(&serde_json::json!(0)));
        assert_eq!(
            json_path(&value, "data.items.0.t"),
            Some(&serde_json::json!("x"))
        );
        assert_eq!(json_path(&value, "data.nope"), None);
    }

    #[test]
    fn dates_use_strftime_formats() {
        // 日期占位常用于笔记标题（Obsidian 快速记录），格式错了文件名就乱了。
        let text = strftime(86_400 * 365, "%Y");
        assert!(text == "1971" || text == "1970", "{text}");
        assert_eq!(strftime(0, "%%"), "%");
        assert_eq!(render("{{date:%Y}}", "", &[]).len(), 4);
    }
}
