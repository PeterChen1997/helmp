//! `plugin.json` 清单 v1：类型定义与严格校验（字段拼错、模板变量不存在、主机名是模板……都报错，不猜）。
//!
//! 字段说明见 docs/plugins.md 与 docs/plugin-schema.json；这里的校验是唯一的标准，schema 只给编辑器补全用。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde::Deserialize;
use serde_json::Value;

use super::template::{Template, Var, is_key};
use crate::i18n::{tr, trf};
use crate::provider::Tint;

pub const SCHEMA_VERSION: u32 = 1;
/// 清单里唯一可以声明的权限；其他权限都由清单内容推导出来。
pub const CLIPBOARD_READ: &str = "clipboard.read";
/// http 超时上限（毫秒）。
pub const MAX_HTTP_TIMEOUT_MS: u64 = 30_000;
/// 脚本动作的超时上限（毫秒）。
pub const MAX_SCRIPT_TIMEOUT_MS: u64 = 120_000;
/// 列表脚本每次输入都可能运行，超时上限要短（见 08 号文档「动态插件硬熔断」）。
pub const MAX_LIST_TIMEOUT_MS: u64 = 3_000;

/// 中英文案：纯字符串，或 `{"en": "...", "zh-Hans": "..."}`（必须有 en）。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Text {
    Plain(String),
    Localized(BTreeMap<String, String>),
}

impl Text {
    /// 按当前界面语言取；缺中文时用英文。
    pub fn get(&self) -> &str {
        match self {
            Text::Plain(text) => text,
            Text::Localized(map) => {
                let zh = map.get("zh-Hans").or_else(|| map.get("zh"));
                let en = map.get("en");
                let picked = if crate::i18n::is_en() {
                    en.or(zh)
                } else {
                    zh.or(en)
                };
                picked.map(String::as_str).unwrap_or_default()
            }
        }
    }

    /// 所有语言的文字（匹配用：英文界面下搜中文标题也要找得到）。
    pub fn all(&self) -> Vec<&str> {
        match self {
            Text::Plain(text) => vec![text.as_str()],
            Text::Localized(map) => map.values().map(String::as_str).collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Manifest {
    /// 给人看的注释（JSON 没有注释语法），任意内容，忽略。
    #[serde(rename = "$comment", default)]
    _comment: Option<serde::de::IgnoredAny>,
    /// 编辑器用的 schema 地址，忽略。
    #[serde(rename = "$schema", default)]
    _schema: Option<serde::de::IgnoredAny>,
    pub schema_version: u32,
    pub id: String,
    pub name: Text,
    #[serde(default)]
    pub description: Option<Text>,
    pub version: String,
    pub author: String,
    #[serde(default)]
    pub homepage: Option<String>,
    /// 插件的开源协议（SPDX 标识，如 MIT）。插件中心要求填写，Helmp 本身不强制。
    #[serde(default)]
    pub license: Option<String>,
    /// Lucide 图标名（`notebook-pen`），或插件目录里的 png（`icon.png`）。
    #[serde(default)]
    pub icon: Option<String>,
    /// 符号图标的底色：blue / indigo / purple / orange / teal / green / red / gray / graphite。
    #[serde(default)]
    pub tint: Option<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub preferences: Vec<Preference>,
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PrefKind {
    Text,
    /// 只存钥匙串，界面里掩码显示，日志和错误信息里打码。
    Secret,
    Select,
    Checkbox,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Preference {
    /// 给人看的注释（JSON 没有注释语法），任意内容，忽略。
    #[serde(rename = "$comment", default)]
    _comment: Option<serde::de::IgnoredAny>,
    pub id: String,
    #[serde(rename = "type")]
    pub kind: PrefKind,
    pub title: Text,
    #[serde(default)]
    pub description: Option<Text>,
    #[serde(default)]
    pub placeholder: Option<Text>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub options: Vec<Choice>,
    /// 这个偏好设置的值被用作 http 地址（整段 URL 来自偏好设置，如 flomo 的 webhook）时，允许的主机名。
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub title: Text,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Command {
    /// 给人看的注释（JSON 没有注释语法），任意内容，忽略。
    #[serde(rename = "$comment", default)]
    _comment: Option<serde::de::IgnoredAny>,
    pub id: String,
    pub title: Text,
    #[serde(default)]
    pub subtitle: Option<Text>,
    /// 额外的搜索词（拼音、缩写、英文名）。标题本身总能匹配（含拼音 / 首字母）。
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub tint: Option<String>,
    /// 关键词后面的文字作为 `{{query}}`（`gh react`）。
    #[serde(default)]
    pub argument: Option<Argument>,
    #[serde(default)]
    pub form: Vec<FormField>,
    #[serde(default)]
    pub action: Option<ActionSpec>,
    #[serde(default)]
    pub list: Option<ListSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Argument {
    #[serde(default)]
    pub placeholder: Option<Text>,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Text,
    Textarea,
    Select,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormField {
    /// 给人看的注释（JSON 没有注释语法），任意内容，忽略。
    #[serde(rename = "$comment", default)]
    _comment: Option<serde::de::IgnoredAny>,
    pub id: String,
    #[serde(rename = "type")]
    pub kind: FieldType,
    pub label: Text,
    #[serde(default)]
    pub placeholder: Option<Text>,
    #[serde(default)]
    pub required: bool,
    /// 初值模板：只能用 query、date、time、非密钥的 pref。
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub options: Vec<Choice>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ActionSpec {
    Open(OpenSpec),
    Http(Box<HttpSpec>),
    Script(ScriptSpec),
    Copy(TextSpec),
    Paste(TextSpec),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenSpec {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextSpec {
    pub text: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    pub fn name(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        }
    }

    fn has_body(self) -> bool {
        matches!(self, Method::Post | Method::Put | Method::Patch)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HttpSpec {
    pub method: Method,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// JSON 正文：任意 JSON，字符串叶子按模板渲染（不用手动转义）。
    #[serde(default)]
    pub json: Option<Value>,
    /// 文本正文模板（需要时配合 `|json` / `|url` 过滤器）。
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub success: Success,
    /// 成功提示模板，可以用 `{{response.<路径>}}`。
    #[serde(default)]
    pub message: Option<Text>,
    /// 失败时从响应 JSON 里取说明的路径（如 `message`）。
    #[serde(default)]
    pub error_path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Success {
    /// 允许的状态码；空 = 任意 2xx。
    #[serde(default)]
    pub status: Vec<u16>,
    /// 响应 JSON 里要检查的字段路径（`code`、`data.ok`）。
    #[serde(default)]
    pub json_path: Option<String>,
    #[serde(default)]
    pub equals: Option<Value>,
    /// 为真时字段不存在也算成功（只在字段存在时比较）。
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ScriptOutput {
    /// 在面板里展示 stdout（默认）。
    #[default]
    Message,
    Silent,
    Copy,
    Paste,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ScriptSpec {
    /// 相对插件目录的路径，不能跳出插件目录。
    pub path: String,
    /// 每个元素一个 argv 参数（模板），从不经过 shell。
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub stdin: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub output: ScriptOutput,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ListSpec {
    /// 输出 Alfred Script Filter JSON（`{"items":[...]}`）的脚本。
    pub script: ListScript,
    /// 选中某一项时的动作，可以用 `{{item.*}}`。缺省：打开 `item.arg`（网址或路径）。
    #[serde(default)]
    pub action: Option<ActionSpec>,
    /// 输入至少这么多个字符才运行脚本。
    #[serde(default)]
    pub min_query_length: usize,
    /// 同一输入的结果缓存秒数（0 = 只缓存到下一次不同的输入）。
    #[serde(default)]
    pub cache_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ListScript {
    pub path: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

impl Manifest {
    pub fn preference(&self, id: &str) -> Option<&Preference> {
        self.preferences.iter().find(|pref| pref.id == id)
    }

    pub fn command(&self, id: &str) -> Option<&Command> {
        self.commands.iter().find(|command| command.id == id)
    }

    pub fn reads_clipboard(&self) -> bool {
        self.permissions.iter().any(|p| p == CLIPBOARD_READ)
    }
}

impl ActionSpec {
    /// 这个动作里所有模板（及其位置说明），校验与权限推导共用。
    pub fn templates(&self) -> Vec<(String, &str)> {
        match self {
            ActionSpec::Open(spec) => vec![("url".into(), spec.url.as_str())],
            ActionSpec::Copy(spec) | ActionSpec::Paste(spec) => {
                vec![("text".into(), spec.text.as_str())]
            }
            ActionSpec::Http(spec) => {
                let mut out = vec![("url".into(), spec.url.as_str())];
                for (name, value) in &spec.headers {
                    out.push((format!("headers.{name}"), value.as_str()));
                }
                if let Some(body) = &spec.body {
                    out.push(("body".into(), body.as_str()));
                }
                if let Some(json) = &spec.json {
                    json_strings(json, "json", &mut out);
                }
                out
            }
            ActionSpec::Script(spec) => {
                let mut out = vec![];
                for (index, arg) in spec.args.iter().enumerate() {
                    out.push((format!("args[{index}]"), arg.as_str()));
                }
                for (name, value) in &spec.env {
                    out.push((format!("env.{name}"), value.as_str()));
                }
                if let Some(stdin) = &spec.stdin {
                    out.push(("stdin".into(), stdin.as_str()));
                }
                out
            }
        }
    }
}

fn json_strings<'v>(value: &'v Value, at: &str, out: &mut Vec<(String, &'v str)>) {
    match value {
        Value::String(text) => out.push((at.to_string(), text.as_str())),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                json_strings(item, &format!("{at}[{index}]"), out);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                json_strings(item, &format!("{at}.{key}"), out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- 校验

/// 解析并校验。`dir` 是插件目录（自带插件没有目录，不检查文件）。错误按出现位置列出，全部报告。
pub fn parse(text: &str, dir: Option<&Path>) -> Result<Manifest, Vec<String>> {
    let manifest: Manifest = serde_json::from_str(text).map_err(|err| {
        vec![trf!(
            "plugin.json 第 {} 行第 {} 列：{err}",
            "plugin.json line {}, column {}: {err}",
            err.line(),
            err.column()
        )]
    })?;
    let mut errors = Vec::new();
    validate(&manifest, dir, &mut errors);
    if errors.is_empty() {
        Ok(manifest)
    } else {
        Err(errors)
    }
}

/// 反向域名：至少两段，每段小写字母数字和 `-`，不以 `-` 开头结尾。
pub fn valid_id(id: &str) -> bool {
    let segments: Vec<&str> = id.split('.').collect();
    id.len() <= 100
        && segments.len() >= 2
        && segments.iter().all(|segment| {
            !segment.is_empty()
                && !segment.starts_with('-')
                && !segment.ends_with('-')
                && segment
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

/// 主机名（不含端口）：字母数字、`-`、`.`。
pub fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !host.starts_with('.')
        && !host.ends_with('.')
}

pub fn parse_tint(name: &str) -> Option<Tint> {
    Some(match name {
        "blue" => Tint::Blue,
        "indigo" => Tint::Indigo,
        "purple" => Tint::Purple,
        "orange" => Tint::Orange,
        "teal" => Tint::Teal,
        "green" => Tint::Green,
        "red" => Tint::Red,
        "gray" => Tint::Gray,
        "graphite" => Tint::Graphite,
        _ => return None,
    })
}

/// 内置的 Lucide 图标里有没有这个名字。
pub fn symbol_exists(name: &str) -> bool {
    use gpui_kit::AssetSource as _;
    is_key(name)
        && gpui_kit::assets::AllAssets
            .load(&format!("icons/{name}.svg"))
            .ok()
            .flatten()
            .is_some()
}

/// 相对插件目录、不跳出插件目录的路径。
pub fn safe_relative(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

/// URL 模板的 scheme 与主机部分。返回 (scheme, 主机模板) ——都是模板原文。
fn split_url(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some((scheme, &rest[..end]))
}

/// 去掉端口与 userinfo。
pub fn host_of(authority: &str) -> &str {
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    authority.split(':').next().unwrap_or(authority)
}

struct Checker<'a> {
    manifest: &'a Manifest,
    errors: &'a mut Vec<String>,
}

/// 模板出现在哪种位置，决定能用哪些变量。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    /// 命令的动作。
    Action,
    /// 列表项的动作（多了 item.*）。
    ItemAction,
    /// http 成功提示（多了 response.*）。
    Message,
    /// 表单初值：查询时就渲染，不能读密钥和剪贴板。
    Default,
    /// 列表脚本的参数 / 环境变量：每次输入都运行，不能读剪贴板和表单。
    ListScript,
}

const ITEM_KEYS: [&str; 5] = ["title", "subtitle", "arg", "uid", "url"];

impl Checker<'_> {
    fn error(&mut self, at: &str, message: String) {
        self.errors.push(format!("{at}: {message}"));
    }

    fn text(&mut self, at: &str, text: &Text) {
        match text {
            Text::Plain(value) if value.trim().is_empty() => {
                self.error(at, tr("不能为空", "must not be empty").into())
            }
            Text::Plain(_) => {}
            Text::Localized(map) => {
                if !map.contains_key("en") {
                    self.error(
                        at,
                        tr(
                            "多语言文案必须有 \"en\"（英文兜底）",
                            "localized text needs an \"en\" entry (the fallback)",
                        )
                        .into(),
                    );
                }
                for (lang, value) in map {
                    if !matches!(lang.as_str(), "en" | "zh-Hans") {
                        self.error(
                            at,
                            trf!(
                                "不支持的语言 {lang:?}（可用：en、zh-Hans）",
                                "unsupported language {lang:?} (use en or zh-Hans)"
                            ),
                        );
                    }
                    if value.trim().is_empty() {
                        self.error(
                            &format!("{at}.{lang}"),
                            tr("不能为空", "must not be empty").into(),
                        );
                    }
                }
            }
        }
    }

    fn key(&mut self, at: &str, key: &str) {
        if !is_key(key) {
            self.error(
                at,
                trf!(
                    "id {key:?} 只能用字母、数字、_ 和 -，并以字母或数字开头",
                    "id {key:?} may only contain letters, digits, _ and -, starting with a letter or digit"
                ),
            );
        }
    }

    fn icon(&mut self, at: &str, icon: &str, dir: Option<&Path>) {
        if icon.ends_with(".png") {
            if !safe_relative(icon) {
                self.error(
                    at,
                    tr(
                        "图标路径必须相对插件目录，不能含 ..",
                        "icon path must be relative to the plugin folder without ..",
                    )
                    .into(),
                );
            } else if let Some(dir) = dir
                && !dir.join(icon).is_file()
            {
                self.error(
                    at,
                    trf!("找不到图标文件 {icon}", "icon file {icon} not found"),
                );
            }
        } else if !symbol_exists(icon) {
            self.error(
                at,
                trf!(
                    "没有名为 {icon:?} 的内置图标（用 Lucide 图标名，如 notebook-pen，或插件目录里的 .png）",
                    "no built-in icon named {icon:?} (use a Lucide icon name such as notebook-pen, or a .png in the plugin folder)"
                ),
            );
        }
    }

    fn tint(&mut self, at: &str, tint: &str) {
        if parse_tint(tint).is_none() {
            self.error(
                at,
                trf!(
                    "不认识的底色 {tint:?}（可用：blue、indigo、purple、orange、teal、green、red、gray、graphite）",
                    "unknown tint {tint:?} (use blue, indigo, purple, orange, teal, green, red, gray or graphite)"
                ),
            );
        }
    }

    fn choices(&mut self, at: &str, options: &[Choice]) {
        if options.is_empty() {
            self.error(
                at,
                tr(
                    "select 至少要有一个选项",
                    "a select needs at least one option",
                )
                .into(),
            );
        }
        let mut seen = BTreeSet::new();
        for (index, option) in options.iter().enumerate() {
            self.text(&format!("{at}[{index}].title"), &option.title);
            if !seen.insert(option.value.as_str()) {
                self.error(
                    &format!("{at}[{index}].value"),
                    trf!(
                        "选项值 {:?} 重复",
                        "duplicate option value {:?}",
                        option.value
                    ),
                );
            }
        }
    }

    /// 解析模板并检查用到的变量在这个位置是否可用。返回解析结果供进一步检查。
    fn template(
        &mut self,
        at: &str,
        text: &str,
        place: Place,
        form: Option<&[FormField]>,
    ) -> Option<Template> {
        let template = match Template::parse(text) {
            Ok(template) => template,
            Err(err) => {
                self.error(at, err.to_string());
                return None;
            }
        };
        for var in template.vars() {
            let problem = match var {
                Var::Query | Var::Date(_) | Var::Time => None,
                Var::Clipboard if matches!(place, Place::Default | Place::ListScript) => Some(
                    tr(
                        "这里不能读剪贴板（查询时就会渲染）",
                        "the clipboard can't be read here (rendered while typing)",
                    )
                    .to_string(),
                ),
                Var::Clipboard if !self.manifest.reads_clipboard() => Some(
                    tr(
                        "用了 {{clipboard}}，要在 permissions 里声明 \"clipboard.read\"",
                        "uses {{clipboard}}, so permissions must include \"clipboard.read\"",
                    )
                    .to_string(),
                ),
                Var::Clipboard => None,
                Var::Form(_) if place == Place::ListScript => Some(
                    tr(
                        "列表脚本不能用表单字段",
                        "list scripts can't use form fields",
                    )
                    .to_string(),
                ),
                Var::Form(_) if place == Place::Default => Some(
                    tr(
                        "表单初值不能引用表单字段",
                        "form defaults can't reference form fields",
                    )
                    .to_string(),
                ),
                Var::Form(key) => match form {
                    Some(fields) if fields.iter().any(|field| field.id == *key) => None,
                    _ => Some(trf!(
                        "这个命令没有表单字段 {key:?}",
                        "this command has no form field {key:?}"
                    )),
                },
                Var::Pref(key) => match self.manifest.preference(key) {
                    None => Some(trf!(
                        "没有偏好设置 {key:?}（先在 preferences 里声明）",
                        "no preference {key:?} (declare it in preferences)"
                    )),
                    Some(pref) if pref.kind == PrefKind::Secret && place == Place::Default => Some(
                        tr(
                            "表单初值不能用 secret 偏好设置",
                            "form defaults can't use secret preferences",
                        )
                        .to_string(),
                    ),
                    Some(_) => None,
                },
                Var::Item(key) if place != Place::ItemAction => Some(trf!(
                    "{{{{item.{key}}}}} 只能用在 list.action 里",
                    "{{{{item.{key}}}}} is only available in list.action"
                )),
                Var::Item(key) if !ITEM_KEYS.contains(&key.as_str()) => Some(trf!(
                    "列表项没有字段 {key:?}（可用：title、subtitle、arg、uid、url）",
                    "list items have no field {key:?} (available: title, subtitle, arg, uid, url)"
                )),
                Var::Item(_) => None,
                Var::Response(_) if place != Place::Message => Some(
                    tr(
                        "{{response.*}} 只能用在 http 的 message 里",
                        "{{response.*}} is only available in an http message",
                    )
                    .to_string(),
                ),
                Var::Response(_) => None,
            };
            if let Some(problem) = problem {
                self.error(at, problem);
            }
        }
        Some(template)
    }

    fn timeout(&mut self, at: &str, value: Option<u64>, max: u64) {
        if let Some(value) = value
            && !(1..=max).contains(&value)
        {
            self.error(
                at,
                trf!(
                    "要在 1 到 {max} 毫秒之间",
                    "must be between 1 and {max} milliseconds"
                ),
            );
        }
    }

    fn script_path(&mut self, at: &str, path: &str, dir: Option<&Path>) {
        if !safe_relative(path) {
            self.error(
                at,
                tr(
                    "脚本路径必须相对插件目录，不能是绝对路径，也不能含 ..",
                    "script path must be relative to the plugin folder, not absolute, and without ..",
                )
                .into(),
            );
            return;
        }
        if let Some(dir) = dir
            && !dir.join(path).is_file()
        {
            self.error(at, trf!("找不到脚本 {path}", "script {path} not found"));
        }
    }

    fn env(&mut self, at: &str, env: &BTreeMap<String, String>) {
        for name in env.keys() {
            let ok = name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !ok {
                self.error(
                    &format!("{at}.{name}"),
                    tr(
                        "环境变量名只能用字母、数字和 _",
                        "environment variable names may only use letters, digits and _",
                    )
                    .into(),
                );
            }
        }
    }

    fn action(
        &mut self,
        at: &str,
        action: &ActionSpec,
        place: Place,
        form: Option<&[FormField]>,
        dir: Option<&Path>,
    ) {
        for (field, text) in action.templates() {
            self.template(&format!("{at}.{field}"), text, place, form);
        }
        match action {
            ActionSpec::Open(spec) => {
                let scheme_static = spec
                    .url
                    .split_once(':')
                    .is_some_and(|(scheme, _)| valid_scheme(scheme));
                if !scheme_static && !spec.url.starts_with("{{") {
                    self.error(
                        &format!("{at}.url"),
                        tr(
                            "要以 scheme 开头（https:、obsidian: …），或整段来自模板",
                            "must start with a scheme (https:, obsidian: …) or be a template",
                        )
                        .into(),
                    );
                }
            }
            ActionSpec::Http(spec) => self.http(at, spec),
            ActionSpec::Script(spec) => {
                self.script_path(&format!("{at}.path"), &spec.path, dir);
                self.env(&format!("{at}.env"), &spec.env);
                self.timeout(
                    &format!("{at}.timeoutMs"),
                    spec.timeout_ms,
                    MAX_SCRIPT_TIMEOUT_MS,
                );
            }
            ActionSpec::Copy(_) | ActionSpec::Paste(_) => {}
        }
    }

    fn http(&mut self, at: &str, spec: &HttpSpec) {
        let url_at = format!("{at}.url");
        match http_url_source(&spec.url, self.manifest) {
            Ok(_) => {}
            Err(problem) => self.error(&url_at, problem),
        }
        if spec.json.is_some() && spec.body.is_some() {
            self.error(
                at,
                tr(
                    "json 和 body 只能写一个",
                    "use either json or body, not both",
                )
                .into(),
            );
        }
        if (spec.json.is_some() || spec.body.is_some()) && !spec.method.has_body() {
            self.error(
                at,
                trf!(
                    "{} 请求不能带正文",
                    "{} requests can't have a body",
                    spec.method.name()
                ),
            );
        }
        for name in spec.headers.keys() {
            let token = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c));
            if !token {
                self.error(
                    &format!("{at}.headers"),
                    trf!("请求头名 {name:?} 不合法", "invalid header name {name:?}"),
                );
            }
        }
        self.timeout(
            &format!("{at}.timeoutMs"),
            spec.timeout_ms,
            MAX_HTTP_TIMEOUT_MS,
        );
        for (index, status) in spec.success.status.iter().enumerate() {
            if !(100..=599).contains(status) {
                self.error(
                    &format!("{at}.success.status[{index}]"),
                    trf!(
                        "{status} 不是 HTTP 状态码",
                        "{status} isn't an HTTP status code"
                    ),
                );
            }
        }
        if spec.success.equals.is_some() != spec.success.json_path.is_some() {
            self.error(
                &format!("{at}.success"),
                tr(
                    "jsonPath 和 equals 要一起写",
                    "jsonPath and equals go together",
                )
                .into(),
            );
        }
        if let Some(message) = &spec.message {
            self.text(&format!("{at}.message"), message);
            for text in message.all() {
                self.template(&format!("{at}.message"), text, Place::Message, None);
            }
        }
    }
}

fn valid_scheme(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
}

/// http 地址的来源：静态主机名，或整段来自一个声明了 allowedHosts 的偏好设置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlSource {
    Static { host: String },
    Pref { pref: String, hosts: Vec<String> },
}

pub fn http_url_source(url: &str, manifest: &Manifest) -> Result<UrlSource, String> {
    if let Some(rest) = url.strip_prefix("{{") {
        let name = rest.split("}}").next().unwrap_or_default().trim();
        let pref_id = name.strip_prefix("pref.").unwrap_or_default();
        let Some(pref) = manifest.preference(pref_id) else {
            return Err(tr(
                "地址的主机部分不能是模板；整段地址来自偏好设置时要写成 {{pref.<id>}} 开头",
                "the host can't be a template; when the whole URL comes from a preference, start with {{pref.<id>}}",
            )
            .into());
        };
        if pref.allowed_hosts.is_empty() {
            return Err(trf!(
                "地址来自偏好设置 {pref_id:?}，它必须声明 allowedHosts（允许访问的主机名）",
                "the URL comes from preference {pref_id:?}, which must declare allowedHosts"
            ));
        }
        return Ok(UrlSource::Pref {
            pref: pref_id.to_string(),
            hosts: pref.allowed_hosts.clone(),
        });
    }
    let Some((scheme, authority)) = split_url(url) else {
        return Err(tr(
            "要以 https:// 或 http:// 开头",
            "must start with https:// or http://",
        )
        .into());
    };
    if scheme != "https" && scheme != "http" {
        return Err(tr(
            "要以 https:// 或 http:// 开头",
            "must start with https:// or http://",
        )
        .into());
    }
    if authority.contains("{{") {
        return Err(tr(
            "主机名不能是模板（网络权限要在启用前就能确定）",
            "the host can't be a template (network access must be known before enabling)",
        )
        .into());
    }
    let host = host_of(authority).to_ascii_lowercase();
    if !valid_host(&host) {
        return Err(trf!("主机名 {host:?} 不合法", "invalid host {host:?}"));
    }
    Ok(UrlSource::Static { host })
}

fn validate(manifest: &Manifest, dir: Option<&Path>, errors: &mut Vec<String>) {
    let mut check = Checker { manifest, errors };
    if manifest.schema_version != SCHEMA_VERSION {
        check.error(
            "schemaVersion",
            trf!(
                "这个版本的 Helmp 只支持 {SCHEMA_VERSION}",
                "this version of Helmp only supports {SCHEMA_VERSION}"
            ),
        );
    }
    if !valid_id(&manifest.id) {
        check.error(
            "id",
            trf!(
                "{:?} 不是反向域名形式的 id（如 com.example.my-plugin，小写字母、数字、-）",
                "{:?} isn't a reverse-DNS id (e.g. com.example.my-plugin: lowercase letters, digits, -)",
                manifest.id
            ),
        );
    }
    check.text("name", &manifest.name);
    if let Some(description) = &manifest.description {
        check.text("description", description);
    }
    let version_ok = manifest.version.split('.').count() == 3
        && manifest
            .version
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    if !version_ok {
        check.error(
            "version",
            trf!(
                "{:?} 不是 x.y.z 形式的版本号",
                "{:?} isn't an x.y.z version",
                manifest.version
            ),
        );
    }
    if manifest.author.trim().is_empty() {
        check.error("author", tr("不能为空", "must not be empty").into());
    }
    if let Some(icon) = &manifest.icon {
        check.icon("icon", icon, dir);
    }
    if let Some(tint) = &manifest.tint {
        check.tint("tint", tint);
    }
    for (index, permission) in manifest.permissions.iter().enumerate() {
        if permission != CLIPBOARD_READ {
            check.error(
                &format!("permissions[{index}]"),
                trf!(
                    "不认识的权限 {permission:?}。只能声明 \"clipboard.read\"；网络、打开链接、运行脚本等权限由清单内容自动推导",
                    "unknown permission {permission:?}. Only \"clipboard.read\" can be declared; network, open and script permissions are derived from the manifest"
                ),
            );
        }
    }

    let mut pref_ids = BTreeSet::new();
    for (index, pref) in manifest.preferences.iter().enumerate() {
        let at = format!("preferences[{index}]");
        check.key(&format!("{at}.id"), &pref.id);
        if !pref_ids.insert(pref.id.as_str()) {
            check.error(
                &format!("{at}.id"),
                trf!("id {:?} 重复", "duplicate id {:?}", pref.id),
            );
        }
        check.text(&format!("{at}.title"), &pref.title);
        match pref.kind {
            PrefKind::Select => {
                check.choices(&format!("{at}.options"), &pref.options);
                if let Some(default) = &pref.default
                    && !pref.options.iter().any(|option| option.value == *default)
                {
                    check.error(
                        &format!("{at}.default"),
                        tr(
                            "默认值要是某个选项的 value",
                            "default must be one of the option values",
                        )
                        .into(),
                    );
                }
            }
            PrefKind::Checkbox => {
                if pref
                    .default
                    .as_deref()
                    .is_some_and(|d| d != "true" && d != "false")
                {
                    check.error(
                        &format!("{at}.default"),
                        tr(
                            "checkbox 的默认值只能是 \"true\" 或 \"false\"",
                            "a checkbox default must be \"true\" or \"false\"",
                        )
                        .into(),
                    );
                }
            }
            PrefKind::Secret if pref.default.is_some() => check.error(
                &format!("{at}.default"),
                tr(
                    "secret 不能有默认值（密钥不写进清单）",
                    "secrets can't have a default (keys don't belong in the manifest)",
                )
                .into(),
            ),
            PrefKind::Secret | PrefKind::Text => {}
        }
        if !pref.options.is_empty() && pref.kind != PrefKind::Select {
            check.error(
                &format!("{at}.options"),
                tr("只有 select 能有 options", "only selects have options").into(),
            );
        }
        if !pref.allowed_hosts.is_empty() && !matches!(pref.kind, PrefKind::Text | PrefKind::Secret)
        {
            check.error(
                &format!("{at}.allowedHosts"),
                tr(
                    "只有 text / secret 能声明 allowedHosts",
                    "only text and secret preferences can declare allowedHosts",
                )
                .into(),
            );
        }
        for (host_index, host) in pref.allowed_hosts.iter().enumerate() {
            if !valid_host(host) || host.to_ascii_lowercase() != *host {
                check.error(
                    &format!("{at}.allowedHosts[{host_index}]"),
                    trf!(
                        "{host:?} 不是小写主机名（不带协议和端口）",
                        "{host:?} isn't a lowercase host name (no scheme or port)"
                    ),
                );
            }
        }
    }

    if manifest.commands.is_empty() {
        check.error(
            "commands",
            tr("至少要有一个命令", "needs at least one command").into(),
        );
    }
    let mut command_ids = BTreeSet::new();
    for (index, command) in manifest.commands.iter().enumerate() {
        let at = format!("commands[{index}]");
        check.key(&format!("{at}.id"), &command.id);
        if !command_ids.insert(command.id.as_str()) {
            check.error(
                &format!("{at}.id"),
                trf!("id {:?} 重复", "duplicate id {:?}", command.id),
            );
        }
        check.text(&format!("{at}.title"), &command.title);
        if let Some(subtitle) = &command.subtitle {
            check.text(&format!("{at}.subtitle"), subtitle);
        }
        if let Some(icon) = &command.icon {
            check.icon(&format!("{at}.icon"), icon, dir);
        }
        if let Some(tint) = &command.tint {
            check.tint(&format!("{at}.tint"), tint);
        }
        for (k, keyword) in command.keywords.iter().enumerate() {
            if keyword.trim().is_empty() {
                check.error(
                    &format!("{at}.keywords[{k}]"),
                    tr("不能为空", "must not be empty").into(),
                );
            }
        }
        let mut field_ids = BTreeSet::new();
        for (f, field) in command.form.iter().enumerate() {
            let field_at = format!("{at}.form[{f}]");
            check.key(&format!("{field_at}.id"), &field.id);
            if !field_ids.insert(field.id.as_str()) {
                check.error(
                    &format!("{field_at}.id"),
                    trf!("id {:?} 重复", "duplicate id {:?}", field.id),
                );
            }
            check.text(&format!("{field_at}.label"), &field.label);
            if field.kind == FieldType::Select {
                check.choices(&format!("{field_at}.options"), &field.options);
            } else if !field.options.is_empty() {
                check.error(
                    &format!("{field_at}.options"),
                    tr("只有 select 能有 options", "only selects have options").into(),
                );
            }
            if let Some(default) = &field.default {
                check.template(
                    &format!("{field_at}.default"),
                    default,
                    Place::Default,
                    Some(&command.form),
                );
            }
        }
        match (&command.action, &command.list) {
            (Some(_), Some(_)) => check.error(
                &at,
                tr(
                    "action 和 list 只能写一个",
                    "use either action or list, not both",
                )
                .into(),
            ),
            (None, None) => check.error(
                &at,
                tr("要有 action 或 list", "needs an action or a list").into(),
            ),
            (Some(action), None) => {
                check.action(
                    &format!("{at}.action"),
                    action,
                    Place::Action,
                    Some(&command.form),
                    dir,
                );
            }
            (None, Some(list)) => {
                if !command.form.is_empty() {
                    check.error(
                        &format!("{at}.form"),
                        tr("列表命令不能带表单", "list commands can't have a form").into(),
                    );
                }
                check.script_path(&format!("{at}.list.script.path"), &list.script.path, dir);
                check.env(&format!("{at}.list.script.env"), &list.script.env);
                check.timeout(
                    &format!("{at}.list.script.timeoutMs"),
                    list.script.timeout_ms,
                    MAX_LIST_TIMEOUT_MS,
                );
                for (a, arg) in list.script.args.iter().enumerate() {
                    check.template(
                        &format!("{at}.list.script.args[{a}]"),
                        arg,
                        Place::ListScript,
                        None,
                    );
                }
                for (name, value) in &list.script.env {
                    check.template(
                        &format!("{at}.list.script.env.{name}"),
                        value,
                        Place::ListScript,
                        None,
                    );
                }
                if let Some(action) = &list.action {
                    check.action(
                        &format!("{at}.list.action"),
                        action,
                        Place::ItemAction,
                        None,
                        dir,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{
        "schemaVersion": 1,
        "id": "com.example.demo",
        "name": {"en": "Demo", "zh-Hans": "演示"},
        "version": "1.0.0",
        "author": "Ada",
        "preferences": [
            {"id": "hook", "type": "secret", "title": "Webhook", "allowedHosts": ["example.com"]}
        ],
        "commands": [
            {"id": "post", "title": "Post", "form": [{"id": "text", "type": "textarea", "label": "Text", "default": "{{query}}"}],
             "action": {"type": "http", "method": "POST", "url": "{{pref.hook}}", "json": {"content": "{{form.text}}"}}},
            {"id": "search", "title": "Search", "argument": {"required": true},
             "action": {"type": "open", "url": "https://example.com/?q={{query|url}}"}}
        ]
    }"#;

    fn errors(text: &str) -> String {
        crate::i18n::with_lang(crate::i18n::Lang::En, || match parse(text, None) {
            Ok(_) => String::new(),
            Err(errors) => errors.join("\n"),
        })
    }

    /// 在 GOOD 的基础上改一处。
    fn mutate(edit: impl FnOnce(&mut Value)) -> String {
        let mut value: Value = serde_json::from_str(GOOD).unwrap();
        edit(&mut value);
        value.to_string()
    }

    #[test]
    fn a_complete_manifest_validates() {
        // 文档里的写法必须能通过校验，否则照着教程写的插件装不上。
        assert_eq!(errors(GOOD), "");
        let manifest = parse(GOOD, None).unwrap();
        assert_eq!(manifest.commands.len(), 2);
        crate::i18n::with_lang(crate::i18n::Lang::Zh, || {
            assert_eq!(manifest.name.get(), "演示")
        });
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            assert_eq!(manifest.name.get(), "Demo")
        });
    }

    #[test]
    fn typos_and_wrong_types_point_at_the_line() {
        // 拼错字段名（titel）必须被拒绝并指出位置，不能当作没写悄悄忽略。
        let err = errors(&GOOD.replace("\"title\": \"Post\"", "\"titel\": \"Post\""));
        assert!(err.contains("titel") && err.contains("line"), "{err}");
        let err = errors(&GOOD.replace("\"schemaVersion\": 1", "\"schemaVersion\": \"1\""));
        assert!(err.contains("line"), "{err}");
    }

    #[test]
    fn bad_manifests_are_rejected_with_readable_reasons() {
        // 每条都是插件作者真会犯的错；错误信息要说出是哪个字段、怎么改。
        let cases: Vec<(String, &str)> = vec![
            (mutate(|v| v["id"] = "Demo".into()), "id"),
            (mutate(|v| v["schemaVersion"] = 2.into()), "schemaVersion"),
            (mutate(|v| v["version"] = "1.0".into()), "version"),
            (
                mutate(|v| v["name"] = serde_json::json!({"zh-Hans": "演示"})),
                "\"en\"",
            ),
            (
                mutate(|v| v["commands"] = serde_json::json!([])),
                "at least one command",
            ),
            (
                mutate(|v| v["icon"] = "no-such-icon-xyz".into()),
                "no-such-icon-xyz",
            ),
            (
                mutate(|v| v["permissions"] = serde_json::json!(["network"])),
                "network",
            ),
            (
                mutate(|v| {
                    v["commands"][1]["action"]["url"] = "https://example.com/?q={{qurey}}".into()
                }),
                "qurey",
            ),
            (
                mutate(|v| v["commands"][0]["action"]["json"]["content"] = "{{form.body}}".into()),
                "commands[0].action.json.content",
            ),
            (
                mutate(
                    |v| v["commands"][1]["action"] = serde_json::json!({"type": "http", "method": "GET", "url": "https://{{query}}.example.com/"}),
                ),
                "host can't be a template",
            ),
            (
                mutate(|v| v["preferences"][0]["allowedHosts"] = serde_json::json!([])),
                "allowedHosts",
            ),
            (
                mutate(|v| {
                    v["commands"][1]["action"] =
                        serde_json::json!({"type": "copy", "text": "{{clipboard}}"})
                }),
                "clipboard.read",
            ),
            (
                mutate(|v| v["commands"][0]["form"][0]["default"] = "{{pref.hook}}".into()),
                "secret",
            ),
            (
                mutate(|v| {
                    v["commands"][1]["action"] =
                        serde_json::json!({"type": "script", "path": "../x.sh"})
                }),
                "..",
            ),
            (
                mutate(
                    |v| v["commands"][1]["action"] = serde_json::json!({"type": "http", "method": "GET", "url": "https://a.com", "body": "x"}),
                ),
                "can't have a body",
            ),
            (
                mutate(|v| v["commands"][1]["id"] = "post".into()),
                "duplicate id",
            ),
            (
                mutate(|v| v["commands"][1]["action"] = serde_json::json!({"type": "launch"})),
                "launch",
            ),
            (
                mutate(|v| v["preferences"][0]["default"] = "sk-123".into()),
                "secrets can't have a default",
            ),
        ];
        for (text, needle) in cases {
            let err = errors(&text);
            assert!(
                err.contains(needle),
                "expected {needle:?} in:\n{err}\nfor {text}"
            );
        }
    }

    #[test]
    fn all_problems_are_reported_at_once() {
        // 一次列出所有问题：改一个跑一次 validate 太折磨人。
        let text = mutate(|v| {
            v["id"] = "x".into();
            v["version"] = "abc".into();
        });
        let err = errors(&text);
        assert!(err.contains("id:") && err.contains("version:"), "{err}");
    }

    #[test]
    fn script_paths_must_exist_inside_the_plugin_folder() {
        let dir = std::env::temp_dir().join(format!("helmp-manifest-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/ok.sh"), "#!/bin/sh\n").unwrap();
        let with = |path: &str| {
            mutate(|v| {
                v["commands"][1]["action"] = serde_json::json!({"type": "script", "path": path})
            })
        };
        let check = |text: String| {
            crate::i18n::with_lang(crate::i18n::Lang::En, || {
                parse(&text, Some(&dir))
                    .err()
                    .map(|e| e.join("\n"))
                    .unwrap_or_default()
            })
        };
        assert_eq!(check(with("bin/ok.sh")), "");
        assert!(check(with("bin/missing.sh")).contains("not found"));
        assert!(check(with("/bin/sh")).contains("relative"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
