//! 工具描述 → 启动器表单，表单值 → `tools/call` 的 arguments，以及「要不要先确认」。
//!
//! 输入 JSON Schema 的映射：
//! - string：文本框；有 `enum` 时下拉；`maxLength` ≥ 200 或名字像正文（content / body / text …）时多行。
//! - number / integer：文本框，提交时检查是不是数字 / 整数。
//! - boolean：下拉「是 / 否」。
//! - 其他（数组、对象、anyOf…）：该字段用多行 JSON 文本框，提交时按 JSON 解析。
//! - 顶层不是带 properties 的 object：整份参数用一个 JSON 文本框。
//!
//! 字段顺序：必填在前，其余保持 schema 里的顺序。

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::i18n::{tr, trf};
use crate::provider::{Field, FieldKind, Form};

/// 整份参数用 JSON 填时的字段键。
pub const RAW_JSON_KEY: &str = "__arguments_json";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Annotations {
    pub title: Option<String>,
    pub read_only: Option<bool>,
    pub destructive: Option<bool>,
    pub idempotent: Option<bool>,
    pub open_world: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub input_schema: Value,
    pub annotations: Annotations,
}

impl Tool {
    pub fn parse(value: &Value) -> Option<Tool> {
        let name = value.get("name")?.as_str()?.to_string();
        let text = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_string);
        let flag = |key: &str| {
            value
                .get("annotations")
                .and_then(|a| a.get(key))
                .and_then(Value::as_bool)
        };
        Some(Tool {
            title: text(value.get("title"))
                .or_else(|| text(value.pointer("/annotations/title")))
                .filter(|t| !t.trim().is_empty()),
            description: text(value.get("description")).unwrap_or_default(),
            input_schema: value
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"type": "object"})),
            annotations: Annotations {
                title: text(value.pointer("/annotations/title")),
                read_only: flag("readOnlyHint"),
                destructive: flag("destructiveHint"),
                idempotent: flag("idempotentHint"),
                open_world: flag("openWorldHint"),
            },
            name,
        })
    }

    /// 显示名：title 优先，没有就用 name。
    pub fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }
}

/// 调用前要不要确认：明确标了 `destructiveHint: true`，或明确「会写（readOnlyHint: false）」
/// 且没说只碰封闭环境（openWorldHint 不是 false）、也没说不具破坏性。什么都没标的不确认
/// （按规范默认值算，几乎所有工具都要确认，会把确认页变成噪音）。
pub fn needs_confirmation(annotations: &Annotations) -> bool {
    if annotations.destructive == Some(true) {
        return true;
    }
    annotations.read_only == Some(false)
        && annotations.open_world != Some(false)
        && annotations.destructive != Some(false)
}

/// `"type": ["string", "null"]` 取第一个非 null。
fn type_of(schema: &Value) -> Option<&str> {
    match schema.get("type") {
        Some(Value::String(kind)) => Some(kind.as_str()),
        Some(Value::Array(kinds)) => kinds
            .iter()
            .filter_map(Value::as_str)
            .find(|kind| *kind != "null"),
        _ if schema.get("enum").is_some() => Some("string"),
        _ => None,
    }
}

const LONG_NAMES: [&str; 12] = [
    "content", "body", "text", "code", "script", "prompt", "message", "markdown", "query_text",
    "contents", "html", "note",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Number,
    Integer,
    Boolean,
    Enum,
    Json,
}

fn kind_of(schema: &Value) -> Kind {
    if schema.get("enum").and_then(Value::as_array).is_some_and(|values| {
        !values.is_empty() && values.iter().all(|v| v.is_string())
    }) {
        return Kind::Enum;
    }
    match type_of(schema) {
        Some("string") => Kind::Text,
        Some("number") => Kind::Number,
        Some("integer") => Kind::Integer,
        Some("boolean") => Kind::Boolean,
        _ => Kind::Json,
    }
}

/// 顶层的属性表。没有 properties 的 object 算「没有参数」（空表）；不是 object 或用了组合关键字的返回 None。
fn properties(schema: &Value) -> Option<Map<String, Value>> {
    let is_object = type_of(schema).is_none_or(|kind| kind == "object");
    let combined = ["anyOf", "oneOf", "allOf", "$ref"]
        .iter()
        .any(|key| schema.get(*key).is_some());
    if !is_object || combined {
        return None;
    }
    match schema.get("properties") {
        None => Some(Map::new()),
        Some(props) => props.as_object().cloned(),
    }
}

fn required(schema: &Value) -> Vec<&str> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// 有序的 (键, 子 schema, 是否必填)。
fn ordered(schema: &Value) -> Option<Vec<(String, Value, bool)>> {
    let props = properties(schema)?;
    let required = required(schema);
    let mut fields: Vec<(String, Value, bool)> = props
        .into_iter()
        .map(|(key, sub)| {
            let needed = required.contains(&key.as_str());
            (key, sub, needed)
        })
        .collect();
    // 稳定排序：必填在前，其余保持原顺序。
    fields.sort_by_key(|field| std::cmp::Reverse(field.2));
    Some(fields)
}

fn default_text(sub: &Value) -> String {
    match sub.get("default") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// 输入 schema → 表单。没有参数的工具返回 None（回车直接调用）。
pub fn form_for(schema: &Value) -> Option<Form> {
    let Some(fields) = ordered(schema) else {
        return Some(Form {
            fields: vec![Field {
                key: RAW_JSON_KEY.into(),
                label: tr("参数（JSON）", "Arguments (JSON)").into(),
                kind: FieldKind::Text {
                    placeholder: "{}".into(),
                    multiline: true,
                },
                value: String::new(),
                required: false,
            }],
        });
    };
    if fields.is_empty() {
        return None;
    }
    let fields = fields
        .into_iter()
        .map(|(key, sub, required)| {
            let (key, sub) = (key.as_str(), &sub);
            let title = sub
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(key)
                .to_string();
            let description = sub
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let unset = || (tr("（不填）", "(not set)").to_string(), String::new());
            let (label, kind) = match kind_of(sub) {
                Kind::Enum => {
                    let mut options: Vec<(String, String)> = Vec::new();
                    if !required {
                        options.push(unset());
                    }
                    options.extend(
                        sub["enum"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .map(|v| (v.to_string(), v.to_string())),
                    );
                    (title, FieldKind::Select { options })
                }
                Kind::Boolean => {
                    let mut options = Vec::new();
                    if !required {
                        options.push(unset());
                    }
                    options.push((tr("是", "Yes").into(), "true".into()));
                    options.push((tr("否", "No").into(), "false".into()));
                    (title, FieldKind::Select { options })
                }
                Kind::Text => {
                    let long = sub
                        .get("maxLength")
                        .and_then(Value::as_u64)
                        .is_some_and(|max| max >= 200)
                        || LONG_NAMES.contains(&key.to_ascii_lowercase().as_str());
                    (
                        title,
                        FieldKind::Text {
                            placeholder: description,
                            multiline: long,
                        },
                    )
                }
                Kind::Number | Kind::Integer => {
                    let what = if kind_of(sub) == Kind::Integer {
                        tr("整数", "integer")
                    } else {
                        tr("数字", "number")
                    };
                    let placeholder = if description.is_empty() {
                        what.to_string()
                    } else {
                        format!("{description}（{what}）")
                    };
                    (
                        title,
                        FieldKind::Text {
                            placeholder,
                            multiline: false,
                        },
                    )
                }
                Kind::Json => (
                    format!("{title} (JSON)"),
                    FieldKind::Text {
                        placeholder: if description.is_empty() {
                            "JSON".into()
                        } else {
                            description
                        },
                        multiline: true,
                    },
                ),
            };
            Field {
                key: key.to_string(),
                label,
                kind,
                value: default_text(sub),
                required,
            }
        })
        .collect();
    Some(Form { fields })
}

/// 表单值 → arguments。不合法的字段一次全部列出（`字段：原因`）。空的选填字段不传。
pub fn arguments(schema: &Value, form: &BTreeMap<String, String>) -> Result<Value, Vec<String>> {
    let Some(fields) = ordered(schema) else {
        let raw = form.get(RAW_JSON_KEY).map(|s| s.trim()).unwrap_or_default();
        if raw.is_empty() {
            return Ok(Value::Object(Map::new()));
        }
        return match serde_json::from_str::<Value>(raw) {
            Ok(value @ Value::Object(_)) => Ok(value),
            Ok(_) => Err(vec![
                tr("参数要是一个 JSON 对象（{…}）", "Arguments must be a JSON object ({…})").into(),
            ]),
            Err(err) => Err(vec![trf!("参数不是合法的 JSON：{err}", "Arguments aren't valid JSON: {err}")]),
        };
    };
    let mut out = Map::new();
    let mut errors = Vec::new();
    for (key, sub, required) in fields {
        let (key, sub) = (key.as_str(), &sub);
        let raw = form.get(key).map(|s| s.trim()).unwrap_or_default();
        let label = sub.get("title").and_then(Value::as_str).unwrap_or(key);
        if raw.is_empty() {
            if required {
                errors.push(trf!("{label}：必填", "{label}: required"));
            }
            continue;
        }
        let value = match kind_of(sub) {
            Kind::Text | Kind::Enum => Ok(Value::String(raw.to_string())),
            Kind::Boolean => match raw {
                "true" | "是" | "yes" => Ok(Value::Bool(true)),
                "false" | "否" | "no" => Ok(Value::Bool(false)),
                _ => Err(trf!("{label}：要选「是」或「否」", "{label}: choose Yes or No")),
            },
            Kind::Integer => raw
                .parse::<i64>()
                .map(Value::from)
                .map_err(|_| trf!("{label}：要填整数", "{label}: must be an integer")),
            Kind::Number => raw
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .ok_or_else(|| trf!("{label}：要填数字", "{label}: must be a number")),
            Kind::Json => serde_json::from_str::<Value>(raw)
                .map_err(|err| trf!("{label}：不是合法的 JSON（{err}）", "{label}: invalid JSON ({err})")),
        };
        match value {
            Ok(value) => {
                out.insert(key.to_string(), value);
            }
            Err(err) => errors.push(err),
        }
    }
    if errors.is_empty() {
        Ok(Value::Object(out))
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn en<T>(f: impl FnOnce() -> T) -> T {
        crate::i18n::with_lang(crate::i18n::Lang::En, f)
    }

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "What to say"},
                "mood": {"type": "string", "enum": ["happy", "sad"]},
                "loud": {"type": "boolean"},
                "times": {"type": "integer", "default": 1},
                "ratio": {"type": ["number", "null"]},
                "tags": {"type": "array", "items": {"type": "string"}},
                "subject": {"type": "string", "maxLength": 500}
            },
            "required": ["text", "mood"]
        })
    }

    #[test]
    fn schema_types_map_to_fitting_controls() {
        // 用户不该对着 JSON 填参数：每种常见类型都要有合适的控件，必填项标出来并排在前面。
        let form = en(|| form_for(&schema()).unwrap());
        let keys: Vec<&str> = form.fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["text", "mood", "loud", "times", "ratio", "tags", "subject"]);
        let field = |key: &str| form.fields.iter().find(|f| f.key == key).unwrap();
        assert!(field("mood").required && field("text").required && !field("loud").required);
        assert_eq!(
            field("mood").kind,
            FieldKind::Select {
                options: vec![("happy".into(), "happy".into()), ("sad".into(), "sad".into())]
            },
            "必填的 enum 不提供「不填」"
        );
        assert_eq!(
            field("loud").kind,
            FieldKind::Select {
                options: vec![
                    ("(not set)".into(), String::new()),
                    ("Yes".into(), "true".into()),
                    ("No".into(), "false".into())
                ]
            }
        );
        assert!(matches!(field("text").kind, FieldKind::Text { multiline: true, ref placeholder } if placeholder == "What to say"));
        assert!(matches!(field("subject").kind, FieldKind::Text { multiline: true, .. }));
        assert!(matches!(field("times").kind, FieldKind::Text { multiline: false, .. }));
        assert_eq!(field("times").value, "1", "schema 的 default 作为初值");
        assert_eq!(field("tags").label, "tags (JSON)", "数组退回 JSON 文本框");
    }

    #[test]
    fn form_values_become_typed_arguments() {
        let form: BTreeMap<String, String> = [
            ("text", "hi"),
            ("mood", "sad"),
            ("loud", "true"),
            ("times", "3"),
            ("ratio", "0.5"),
            ("tags", r#"["a","b"]"#),
            ("subject", ""),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        assert_eq!(
            arguments(&schema(), &form).unwrap(),
            json!({"text": "hi", "mood": "sad", "loud": true, "times": 3, "ratio": 0.5, "tags": ["a", "b"]}),
            "空的选填字段不传，类型按 schema 转换"
        );
    }

    #[test]
    fn invalid_values_are_all_reported() {
        let form: BTreeMap<String, String> =
            [("times", "1.5"), ("ratio", "abc"), ("tags", "[oops")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        let errors = en(|| arguments(&schema(), &form).unwrap_err());
        assert_eq!(errors.len(), 5, "{errors:?}");
        assert!(errors.contains(&"text: required".to_string()));
        assert!(errors.contains(&"times: must be an integer".to_string()));
        assert!(errors.contains(&"ratio: must be a number".to_string()));
    }

    #[test]
    fn unsupported_schemas_fall_back_to_one_json_box() {
        let schema = json!({"anyOf": [{"type": "object"}, {"type": "string"}]});
        let form = form_for(&schema).unwrap();
        assert_eq!(form.fields.len(), 1);
        assert_eq!(form.fields[0].key, RAW_JSON_KEY);
        let mut values = BTreeMap::new();
        values.insert(RAW_JSON_KEY.to_string(), r#"{"a": 1}"#.to_string());
        assert_eq!(arguments(&schema, &values).unwrap(), json!({"a": 1}));
        values.insert(RAW_JSON_KEY.to_string(), "[1]".to_string());
        assert!(arguments(&schema, &values).is_err());
        assert_eq!(arguments(&schema, &BTreeMap::new()).unwrap(), json!({}));
    }

    #[test]
    fn tools_without_parameters_have_no_form() {
        assert!(form_for(&json!({"type": "object", "properties": {}})).is_none());
        assert!(form_for(&json!({"type": "object"})).is_none());
    }

    #[test]
    fn confirmation_follows_the_annotations() {
        // 删东西、往外发东西的工具先确认；只读的、没标注的不打扰。
        let a = |read_only, destructive, open_world| Annotations {
            read_only,
            destructive,
            open_world,
            ..Annotations::default()
        };
        assert!(needs_confirmation(&a(None, Some(true), None)));
        assert!(needs_confirmation(&a(Some(false), None, None)), "会写、开放世界：默认算危险");
        assert!(needs_confirmation(&a(Some(false), None, Some(true))));
        assert!(!needs_confirmation(&a(Some(false), None, Some(false))), "只改封闭环境");
        assert!(!needs_confirmation(&a(Some(false), Some(false), None)), "明确说了不具破坏性");
        assert!(!needs_confirmation(&a(Some(true), None, None)));
        assert!(!needs_confirmation(&a(None, None, None)), "没标注的不确认");
    }

    #[test]
    fn tools_parse_titles_and_hints() {
        let tool = Tool::parse(&json!({
            "name": "delete_file",
            "description": "Delete",
            "annotations": {"title": "Delete File", "destructiveHint": true}
        }))
        .unwrap();
        assert_eq!(tool.display_title(), "Delete File");
        assert_eq!(tool.annotations.destructive, Some(true));
        assert_eq!(tool.input_schema, json!({"type": "object"}));
        assert!(Tool::parse(&json!({"description": "no name"})).is_none());
    }
}
