//! 清单插件（L1.5）：一个文件夹 + 一份声明式的 `plugin.json`，不写代码就能接 flomo、Obsidian、
//! 各类「关键词 → 网址」；动态列表用脚本输出 Alfred Script Filter JSON。
//!
//! - 清单格式、校验：`manifest`；模板语法：`template`；权限推导：`permissions`。
//! - 发现、启用状态、热重载：`registry`（自带插件编译进来，用户插件在 `<data_dir>/plugins/`）。
//! - 执行动作：`exec`；secret 偏好设置：`secrets`（只存钥匙串）。
//! - 每个命令一个结果，id 为 `plugins:<插件 id>/<命令 id>`；列表命令的每一项为 `…#<uid>`。
//! - 匹配：命令标题（中英、拼音、首字母都行）与 keywords；「关键词 + 空格 + 文字」也算命中这个命令。
//! - 接受输入的命令（argument / form / list）在全局搜索里只是一个入口，不吃掉关键词后面的文字：
//!   Tab（或回车）进入这个命令自己的范围（id 同结果 id，搜索栏左侧出现插件标签），在范围里再输入，
//!   回车才执行。这样日常搜索打出「flomo 什么」也不会误把内容提交出去（二次确认）。
//!   范围里：表单命令（flomo）焦点直接进表单、回车提交；参数命令（gh）搜索框的字就是 `{{query}}`；
//!   列表命令按搜索框的字运行脚本。不接受输入的命令照旧在全局搜索里回车直接执行。
//! - 另有「插件管理」命令，打开设置 › 插件。
//!
//! 开发者文档：docs/plugins.md。

pub mod center;
pub mod exec;
pub mod manifest;
pub mod permissions;
pub mod registry;
pub mod scaffold;
pub mod secrets;
pub mod submit;
pub mod template;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::i18n::{tr, trf};
use crate::matcher;
use crate::provider::{
    Accessory, Action, ActionInput, Block, Detail, Field, FieldKind, Form, Icon, Item, NamedAction,
    Outcome, Provider, Scope, Tint,
};
use crate::settings::Page;
use manifest::{ActionSpec, Command, FieldType, PrefKind};
use registry::{Plugin, State};
use template::{Context, Template, Var};

use super::scripts::args::strip_name;

pub const PROVIDER_ID: &str = "plugins";
/// 「关键词 + 空格」开头，或完整关键词：等同完全匹配。
const PREFIX_SCORE: u32 = 1000;
const MANAGE_ICON: Icon = Icon::Symbol {
    name: "puzzle",
    tint: Tint::Blue,
};

fn badge() -> &'static str {
    tr("插件", "Plugins")
}

/// 列表命令上一次的结果（同一输入不重复跑脚本）。
struct CachedList {
    query: String,
    at: Instant,
    result: Result<Vec<exec::ListItem>, String>,
}

pub struct Plugins {
    lists: Mutex<HashMap<String, CachedList>>,
}

impl Plugins {
    pub fn new() -> Self {
        Self {
            lists: Mutex::default(),
        }
    }
}

impl Provider for Plugins {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn start(&self) -> Result<()> {
        registry::watch()
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let config = crate::config::get();
        let mut items: Vec<Item> = manage_item(query).into_iter().collect();
        for entry in registry::current().iter() {
            if entry.state(&config) != State::Enabled {
                continue;
            }
            let Some(plugin) = entry.plugin() else {
                continue;
            };
            for command in &plugin.manifest.commands {
                let Some(score) = match_command(command, query) else {
                    continue;
                };
                items.push(if accepts_input(command) {
                    entry_item(plugin, command, score)
                } else {
                    command_item(plugin, command, score, "")
                });
            }
        }
        items
    }

    fn command_scope(&self, id: &str) -> Option<Scope> {
        let (plugin, command_id) = scoped_command(id)?;
        let command = plugin.manifest.command(&command_id)?;
        Some(command_scope(&plugin, command))
    }

    fn query_command(&self, id: &str, query: &str) -> Vec<Item> {
        let Some((plugin, command_id)) = scoped_command(id) else {
            return Vec::new();
        };
        let Some(command) = plugin.manifest.command(&command_id) else {
            return Vec::new();
        };
        match &command.list {
            Some(list) if query.chars().count() >= list.min_query_length => {
                self.list_items(&plugin, command, list, query)
            }
            _ => vec![command_item(&plugin, command, PREFIX_SCORE, query)],
        }
    }
}

/// 命令范围 id（就是入口结果的 id）→ 已启用的插件与命令 id。只认接受输入的命令。
fn scoped_command(id: &str) -> Option<(Arc<Plugin>, String)> {
    let rest = id.strip_prefix(PROVIDER_ID)?.strip_prefix(':')?;
    let (plugin_id, command_id) = rest.split_once('/')?;
    let plugin = enabled_plugin(plugin_id).ok()?;
    let command = plugin.manifest.command(command_id)?;
    accepts_input(command).then(|| (plugin, command_id.to_string()))
}

/// 已启用、加载成功的插件；否则返回给人看的原因（深链接打印到 stderr）。
fn enabled_plugin(plugin_id: &str) -> Result<Arc<Plugin>, String> {
    if !crate::config::extension_enabled(PROVIDER_ID) {
        return Err(tr("插件扩展已停用", "The Plugins extension is turned off").into());
    }
    let entries = registry::current();
    let entry = entries
        .iter()
        .find(|entry| entry.id == plugin_id)
        .ok_or_else(|| trf!("没有插件 {plugin_id}", "No plugin {plugin_id}"))?;
    if entry.state(&crate::config::get()) != State::Enabled {
        return Err(trf!(
            "插件 {plugin_id} 未启用",
            "Plugin {plugin_id} isn't enabled"
        ));
    }
    entry
        .plugin()
        .cloned()
        .ok_or_else(|| trf!("插件 {plugin_id} 出错", "Plugin {plugin_id} has errors"))
}

fn item_id(plugin: &Plugin, command: &Command) -> String {
    format!("{PROVIDER_ID}:{}/{}", plugin.id(), command.id)
}

/// 命令范围：标签写插件名（只有一个命令的插件，如 flomo）或命令名（快捷搜索里的 GitHub、npm…），
/// 颜色跟命令图标；占位文字说明在范围里要输入什么。
fn command_scope(plugin: &Plugin, command: &Command) -> Scope {
    let label = if plugin.manifest.commands.len() == 1 {
        plugin.manifest.name.get().to_string()
    } else {
        command.title.get().to_string()
    };
    let tint = match command_icon(plugin, command) {
        Icon::Symbol { tint, .. } => tint,
        _ => command
            .tint
            .as_ref()
            .or(plugin.manifest.tint.as_ref())
            .and_then(|tint| manifest::parse_tint(tint))
            .unwrap_or(Tint::Gray),
    };
    let placeholder = if command.form.is_empty() {
        input_hint(command)
    } else {
        tr(
            "在下面填写，回车提交 · Esc 返回",
            "Fill in the form, then press Return · Esc to go back",
        )
        .to_string()
    };
    Scope {
        id: item_id(plugin, command),
        label,
        tint,
        prefixes: Vec::new(),
        placeholder,
        filters: Vec::new(),
    }
}

/// 参数 / 列表命令在范围里要输入什么：清单里参数的 placeholder，没有就用通用说法。
fn input_hint(command: &Command) -> String {
    command
        .argument
        .as_ref()
        .and_then(|argument| argument.placeholder.as_ref())
        .map(|placeholder| placeholder.get().to_string())
        .unwrap_or_else(|| {
            if command.list.is_some() {
                tr("输入要找的内容", "Type to search").to_string()
            } else {
                tr("输入内容", "Type here").to_string()
            }
        })
}

/// 全局搜索里接受输入的命令：只是一个入口，Tab 或回车进入它的范围（不带上搜索框里的文字）。
fn entry_item(plugin: &Arc<Plugin>, command: &Command, score: u32) -> Item {
    let id = item_id(plugin, command);
    let subtitle = command
        .subtitle
        .as_ref()
        .map(|text| text.get().to_string())
        .unwrap_or_else(|| {
            let what = input_hint(command);
            trf!("按 Tab 进入后输入：{what}", "Press Tab, then type: {what}")
        });
    Item {
        id: id.clone(),
        title: command.title.get().to_string(),
        subtitle,
        badge: badge().into(),
        score,
        icon: command_icon(plugin, command),
        accessories: vec![
            Accessory::Text(plugin.manifest.name.get().to_string()),
            Accessory::Keys(vec!["Tab".into()]),
        ],
        // 进入范围不隐藏面板：先收起再弹出会闪一下。
        action: Action::enter_scope(id.clone()).keep_open(),
        action_title: tr("进入", "Open").into(),
        actions: vec![NamedAction::new(
            tr("插件设置", "Plugin Settings"),
            None,
            open_settings(),
        )],
        opens_scope: Some(id),
        ..Item::blank()
    }
}

/// 匹配用的名字：keywords 加各语言的标题。
fn names(command: &Command) -> Vec<&str> {
    command
        .keywords
        .iter()
        .map(String::as_str)
        .chain(command.title.all())
        .collect()
}

/// 命令是否接受「关键词后面的文字」。
pub fn accepts_input(command: &Command) -> bool {
    command.argument.is_some() || !command.form.is_empty() || command.list.is_some()
}

/// 命中分数。接受输入的命令，「关键词 + 空格 + 文字」也算完全命中（只用来排序，
/// 后面的文字不会被当作输入：要 Tab 进入命令范围后再输入）。
fn match_command(command: &Command, query: &str) -> Option<u32> {
    if accepts_input(command)
        && names(command)
            .into_iter()
            .any(|name| strip_name(query, name).is_some())
    {
        return Some(PREFIX_SCORE);
    }
    // 只认字面或拼音命中，不认「字母按顺序出现」的模糊匹配：插件命令名普遍较长，
    // 模糊匹配会让 uuid 这类查询冒出「Obsidian 快速记录」之类毫不相关的结果。
    names(command)
        .into_iter()
        .filter_map(|name| matcher::score(query, name))
        .filter(|score| *score >= MIN_NAME_SCORE)
        .max()
}

/// 命令名匹配的最低分：拼音中段命中（550）及以上；字面/拼音模糊匹配（400 / 300）不算。
const MIN_NAME_SCORE: u32 = 550;

/// 内置符号名要 `&'static str`：清单里的名字去重后常驻（数量有限，最多几百个）。
fn intern(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<BTreeSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(found) = names.get(name) {
        return found;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.insert(leaked);
    leaked
}

/// 图标：命令的优先，其次插件的；png 相对插件目录。
pub fn icon_for(plugin: &Plugin, icon: Option<&String>, tint: Option<&String>) -> Icon {
    let icon = icon.or(plugin.manifest.icon.as_ref());
    let tint = tint
        .or(plugin.manifest.tint.as_ref())
        .and_then(|tint| manifest::parse_tint(tint))
        .unwrap_or(Tint::Gray);
    match icon {
        Some(name) if name.ends_with(".png") => match &plugin.dir {
            Some(dir) => Icon::Image(dir.join(name)),
            None => Icon::Symbol {
                name: "puzzle",
                tint,
            },
        },
        Some(name) => Icon::Symbol {
            name: intern(name),
            tint,
        },
        None => Icon::Symbol {
            name: "puzzle",
            tint,
        },
    }
}

fn command_icon(plugin: &Plugin, command: &Command) -> Icon {
    icon_for(plugin, command.icon.as_ref(), command.tint.as_ref())
}

/// 查询时渲染（表单初值、地址预览）：只用 query、日期和非密钥偏好设置，不读钥匙串和剪贴板。
fn render_light(plugin: &Plugin, text: &str, query: &str) -> Option<String> {
    let template = Template::parse(text).ok()?;
    let light = template.vars().all(|var| match var {
        Var::Query | Var::Date(_) | Var::Time => true,
        Var::Pref(key) => plugin
            .manifest
            .preference(key)
            .is_some_and(|pref| pref.kind != PrefKind::Secret),
        Var::Clipboard | Var::Form(_) | Var::Item(_) | Var::Response(_) => false,
    });
    if !light {
        return None;
    }
    let pref = |key: &str| Ok(exec::plain_pref_value(plugin, key));
    let clipboard = || Ok(String::new());
    let form = BTreeMap::new();
    let context = Context {
        query,
        form: &form,
        pref: &pref,
        clipboard: &clipboard,
        item: None,
        response: None,
        now: template::now(),
    };
    template.render(&context).ok()
}

/// 表单：每个字段一个输入框，初值是渲染后的 default。
fn form_for(plugin: &Plugin, command: &Command, query: &str) -> Option<Form> {
    if command.form.is_empty() {
        return None;
    }
    let fields = command
        .form
        .iter()
        .map(|field| {
            let placeholder = field
                .placeholder
                .as_ref()
                .map(|text| text.get().to_string())
                .unwrap_or_default();
            Field {
                key: field.id.clone(),
                label: field.label.get().to_string(),
                kind: match field.kind {
                    FieldType::Text => FieldKind::Text {
                        placeholder,
                        multiline: false,
                    },
                    FieldType::Textarea => FieldKind::Text {
                        placeholder,
                        multiline: true,
                    },
                    FieldType::Select => FieldKind::Select {
                        options: field
                            .options
                            .iter()
                            .map(|option| (option.title.get().to_string(), option.value.clone()))
                            .collect(),
                    },
                },
                value: field
                    .default
                    .as_deref()
                    .and_then(|default| render_light(plugin, default, query))
                    .unwrap_or_default(),
                required: field.required,
            }
        })
        .collect();
    Some(Form { fields })
}

/// 用户没在表单里填的字段用初值（命令行 `helmp run` 只给部分 `--form` 时）。
fn effective_form(
    plugin: &Plugin,
    command: &Command,
    query: &str,
    given: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for field in &command.form {
        let value = given.get(&field.id).cloned().unwrap_or_else(|| {
            field
                .default
                .as_deref()
                .and_then(|default| render_light(plugin, default, query))
                .unwrap_or_default()
        });
        values.insert(field.id.clone(), value);
    }
    values
}

fn open_settings() -> Action {
    Action::new(|| Ok(Outcome::OpenSettingsPage(Page::Plugins)))
}

/// 缺必填偏好设置时的引导页（而不是带着空 webhook 发请求）。
fn setup_needed(plugin: &Plugin, icon: &Icon, title: &str) -> Result<Option<Outcome>> {
    let mut missing = Vec::new();
    for pref in plugin
        .manifest
        .preferences
        .iter()
        .filter(|pref| pref.required)
    {
        if exec::pref_value(plugin, &pref.id)?.trim().is_empty() {
            missing.push(pref.title.get().to_string());
        }
    }
    if missing.is_empty() {
        return Ok(None);
    }
    let name = plugin.manifest.name.get();
    let list = missing.join(tr("、", ", "));
    Ok(Some(Outcome::Show(Detail {
        icon: icon.clone(),
        title: trf!("先设置「{name}」", "Set up {name} first"),
        tag: None,
        blocks: vec![Block::Text(trf!(
            "「{title}」要先在 设置 › 插件 里填写：{list}",
            "“{title}” needs these settings in Settings › Plugins first: {list}"
        ))],
        metadata: Vec::new(),
        actions: vec![NamedAction::new(
            tr("打开插件设置", "Open Plugin Settings"),
            None,
            open_settings(),
        )],
    })))
}

fn trigger(command: &Command) -> String {
    command
        .keywords
        .first()
        .cloned()
        .unwrap_or_else(|| command.title.get().to_string())
}

fn action_title(command: &Command) -> &'static str {
    match &command.action {
        Some(ActionSpec::Open(_)) => tr("打开", "Open"),
        Some(ActionSpec::Copy(_)) => tr("复制", "Copy"),
        Some(ActionSpec::Paste(_)) => tr("粘贴", "Paste"),
        Some(ActionSpec::Http(_)) if !command.form.is_empty() => tr("提交", "Submit"),
        _ => tr("运行", "Run"),
    }
}

fn command_item(plugin: &Arc<Plugin>, command: &Command, score: u32, query: &str) -> Item {
    let icon = command_icon(plugin, command);
    let title = command.title.get().to_string();
    let preview = match &command.action {
        Some(ActionSpec::Open(spec)) if !query.is_empty() && command.form.is_empty() => {
            render_light(plugin, &spec.url, query)
        }
        _ => None,
    };
    let subtitle = preview
        .or_else(|| command.subtitle.as_ref().map(|text| text.get().to_string()))
        .or_else(|| accepts_input(command).then(|| input_hint(command)))
        .unwrap_or_else(|| plugin.manifest.name.get().to_string());
    let action = {
        let (plugin, command_id, query, icon) = (
            Arc::clone(plugin),
            command.id.clone(),
            query.to_string(),
            icon.clone(),
        );
        Action::with_input(move |input: &ActionInput| {
            run_command(&plugin, &command_id, &query, &icon, input)
        })
    };
    Item {
        id: item_id(plugin, command),
        title,
        subtitle,
        badge: badge().into(),
        score,
        icon,
        accessories: vec![Accessory::Text(plugin.manifest.name.get().to_string())],
        form: form_for(plugin, command, query),
        action,
        action_title: action_title(command).into(),
        actions: vec![NamedAction::new(
            tr("插件设置", "Plugin Settings"),
            None,
            open_settings(),
        )],
        ..Item::blank()
    }
}

fn run_command(
    plugin: &Arc<Plugin>,
    command_id: &str,
    query: &str,
    icon: &Icon,
    input: &ActionInput,
) -> Result<Outcome> {
    let Some(command) = plugin.manifest.command(command_id) else {
        anyhow::bail!(trf!(
            "插件里已经没有命令 {command_id}",
            "The plugin no longer has command {command_id}"
        ));
    };
    let title = command.title.get();
    if let Some(outcome) = setup_needed(plugin, icon, title)? {
        return Ok(outcome);
    }
    if command.list.is_some() {
        return Ok(Outcome::Message(trf!(
            "「{title}」：先在搜索框里输入要找的内容",
            "“{title}”: type what you're looking for in the search bar first"
        )));
    }
    if command
        .argument
        .as_ref()
        .is_some_and(|argument| argument.required)
        && query.trim().is_empty()
    {
        let what = command
            .argument
            .as_ref()
            .and_then(|argument| argument.placeholder.as_ref())
            .map(|placeholder| placeholder.get().to_string())
            .unwrap_or_else(|| tr("内容", "something").to_string());
        return Ok(Outcome::Message(trf!(
            "「{title}」需要输入内容：在搜索框里输入{what}",
            "“{title}” needs input: type {what} in the search bar"
        )));
    }
    let form = effective_form(plugin, command, query, &input.form);
    let missing: Vec<&str> = command
        .form
        .iter()
        .filter(|field| {
            field.required
                && form
                    .get(&field.id)
                    .is_none_or(|value| value.trim().is_empty())
        })
        .map(|field| field.label.get())
        .collect();
    if !missing.is_empty() {
        return Ok(Outcome::Message(trf!(
            "「{title}」还没填：{}",
            "“{title}” is missing: {}",
            missing.join(tr("、", ", "))
        )));
    }
    let Some(action) = &command.action else {
        anyhow::bail!(trf!("「{title}」没有动作", "“{title}” has no action"));
    };
    exec::run(
        &exec::Input {
            plugin,
            title,
            icon: icon.clone(),
            query,
            form: &form,
            item: None,
        },
        action,
    )
}

impl Plugins {
    fn cached_list(
        &self,
        plugin: &Plugin,
        command: &Command,
        list: &manifest::ListSpec,
        query: &str,
    ) -> Result<Vec<exec::ListItem>, String> {
        let key = format!("{}/{}", plugin.id(), command.id);
        let ttl = Duration::from_secs(list.cache_seconds.max(1));
        {
            let cache = self
                .lists
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(hit) = cache.get(&key)
                && hit.query == query
                && hit.at.elapsed() < ttl
            {
                return hit.result.clone();
            }
        }
        let result = exec::run_list(plugin, command.title.get(), list, query)
            .map_err(|err| format!("{err:#}"));
        self.lists
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                key,
                CachedList {
                    query: query.to_string(),
                    at: Instant::now(),
                    result: result.clone(),
                },
            );
        result
    }

    fn list_items(
        &self,
        plugin: &Arc<Plugin>,
        command: &Command,
        list: &manifest::ListSpec,
        query: &str,
    ) -> Vec<Item> {
        let icon = command_icon(plugin, command);
        let title = command.title.get();
        let base = format!("{PROVIDER_ID}:{}/{}", plugin.id(), command.id);
        let rows = match self.cached_list(plugin, command, list, query) {
            Ok(rows) => rows,
            Err(err) => {
                // 出错要显示出来：悄悄返回空列表，作者会以为只是没命中。
                let message = err.clone();
                return vec![Item {
                    id: format!("{base}#error"),
                    title: trf!("「{title}」出错", "“{title}” failed"),
                    subtitle: err.lines().next().unwrap_or_default().to_string(),
                    badge: badge().into(),
                    score: PREFIX_SCORE,
                    // 和设置 › 插件里出错的插件同一个图标：一眼看出是错误，不是普通结果。
                    icon: Icon::Symbol {
                        name: "triangle-alert",
                        tint: Tint::Red,
                    },
                    action: Action::new(move || Ok(Outcome::Message(message.clone()))),
                    action_title: tr("查看错误", "Show Error").into(),
                    ..Item::blank()
                }];
            }
        };
        rows.into_iter()
            .enumerate()
            .map(|(index, row)| {
                let key = row
                    .fields
                    .get("uid")
                    .cloned()
                    .unwrap_or_else(|| index.to_string());
                let fields = row.fields.clone();
                let action = if row.valid {
                    let (plugin, command_id, query, icon) = (
                        Arc::clone(plugin),
                        command.id.clone(),
                        query.to_string(),
                        icon.clone(),
                    );
                    Action::new(move || run_list_item(&plugin, &command_id, &query, &icon, &fields))
                } else {
                    // Alfred 的 valid:false 只是提示行，回车什么也不做。
                    Action::new(|| Ok(Outcome::Done)).keep_open()
                };
                let copy_text = row
                    .copy
                    .clone()
                    .or_else(|| row.fields.get("arg").cloned())
                    .unwrap_or_else(|| row.fields["title"].clone());
                Item {
                    id: format!("{base}#{key}"),
                    title: row.fields["title"].clone(),
                    subtitle: row.fields.get("subtitle").cloned().unwrap_or_default(),
                    badge: badge().into(),
                    score: PREFIX_SCORE
                        .saturating_sub(u32::try_from(index).unwrap_or(PREFIX_SCORE)),
                    icon: row.icon.clone().unwrap_or_else(|| icon.clone()),
                    accessories: vec![Accessory::Text(plugin.manifest.name.get().to_string())],
                    action,
                    action_title: match &list.action {
                        Some(ActionSpec::Copy(_)) => tr("复制", "Copy"),
                        Some(ActionSpec::Paste(_)) => tr("粘贴", "Paste"),
                        Some(ActionSpec::Open(_)) | None => tr("打开", "Open"),
                        _ => tr("运行", "Run"),
                    }
                    .into(),
                    actions: vec![NamedAction::new(
                        tr("复制", "Copy"),
                        Some("⌘C"),
                        Action::copy(copy_text),
                    )],
                    ..Item::blank()
                }
            })
            .collect()
    }
}

fn run_list_item(
    plugin: &Arc<Plugin>,
    command_id: &str,
    query: &str,
    icon: &Icon,
    fields: &BTreeMap<String, String>,
) -> Result<Outcome> {
    let Some(command) = plugin.manifest.command(command_id) else {
        anyhow::bail!(trf!(
            "插件里已经没有命令 {command_id}",
            "The plugin no longer has command {command_id}"
        ));
    };
    let title = command.title.get();
    if let Some(outcome) = setup_needed(plugin, icon, title)? {
        return Ok(outcome);
    }
    let default = ActionSpec::Open(manifest::OpenSpec {
        url: "{{item.arg}}".into(),
    });
    let action = command
        .list
        .as_ref()
        .and_then(|list| list.action.as_ref())
        .unwrap_or(&default);
    let form = BTreeMap::new();
    exec::run(
        &exec::Input {
            plugin,
            title,
            icon: icon.clone(),
            query,
            form: &form,
            item: Some(fields),
        },
        action,
    )
}

fn manage_item(query: &str) -> Option<Item> {
    const KEYWORDS: [&str; 7] = [
        "插件管理",
        "管理插件",
        "插件",
        "manage plugins",
        "plugins",
        "plugin manager",
        "extensions",
    ];
    let score = KEYWORDS
        .iter()
        .filter_map(|keyword| matcher::score(query, keyword))
        .max()?;
    Some(Item {
        id: format!("{PROVIDER_ID}:manage"),
        title: tr("插件管理", "Manage Plugins").into(),
        subtitle: tr(
            "启用插件、填写偏好设置、新建插件",
            "Turn plugins on, fill in their settings, create your own",
        )
        .into(),
        badge: tr("命令", "Commands").into(),
        score,
        icon: MANAGE_ICON,
        action: open_settings(),
        action_title: tr("打开设置", "Open Settings").into(),
        ..Item::blank()
    })
}

/// 深链接 `helmp://plugin/<插件 id>/<命令 id>?q=` 的目标。
pub struct LinkTarget {
    pub item: Item,
    /// 接受输入的命令：面板直接进入这个命令范围，q 作为范围里的输入（表单初值 / 参数 / 列表搜索词）。
    pub scope: Option<String>,
    /// 不接受输入的命令：在搜索框里显示的查询（命令关键词），让面板出现这个命令，等用户回车。
    pub query: String,
    /// 只打开链接、不需要表单的命令直接执行；会发请求、跑脚本、碰剪贴板的命令只显示出来，等用户回车
    /// （网页里的链接也能触发深链接，不能让它替用户提交内容）。
    pub run_now: bool,
}

pub fn link_target(plugin_id: &str, command_id: &str, q: &str) -> Result<LinkTarget, String> {
    let plugin = enabled_plugin(plugin_id)?;
    let command = plugin.manifest.command(command_id).ok_or_else(|| {
        trf!(
            "插件 {plugin_id} 没有命令 {command_id}",
            "Plugin {plugin_id} has no command {command_id}"
        )
    })?;
    Ok(LinkTarget {
        item: command_item(&plugin, command, PREFIX_SCORE, q),
        scope: accepts_input(command).then(|| item_id(&plugin, command)),
        query: trigger(command),
        run_now: link_runs_now(command, q),
    })
}

/// 深链接能不能直接执行：只有「打开链接、没有表单、参数已给」的命令可以。
fn link_runs_now(command: &Command, q: &str) -> bool {
    let needs_input = command
        .argument
        .as_ref()
        .is_some_and(|argument| argument.required)
        && q.trim().is_empty();
    command.form.is_empty()
        && command.list.is_none()
        && matches!(command.action, Some(ActionSpec::Open(_)))
        && !needs_input
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只用自带插件（不扫描用户的数据目录）。
    fn bundled(id: &str) -> Arc<Plugin> {
        registry::load_bundled()
            .into_iter()
            .find(|entry| entry.id == id)
            .and_then(|entry| entry.plugin().cloned())
            .unwrap()
    }

    #[test]
    fn root_search_only_offers_an_entry_and_never_consumes_the_text() {
        // 用户日常搜索里打出「gh rust」「flomo 什么」不能就地变成可执行的网址 / 填好的表单（误触发），
        // 只给一个命令入口：Tab 或回车进入命令自己的范围，在范围里输入后才执行。
        let plugin = bundled("dev.helmp.quicklinks");
        let gh = plugin.manifest.command("gh").unwrap();
        // 「关键词 + 空格 + 文字」仍算命中这个命令（排在前面，方便 Tab），但后面的文字不被拿走。
        assert_eq!(match_command(gh, "gh react hooks"), Some(PREFIX_SCORE));
        assert_eq!(match_command(gh, "gh"), Some(PREFIX_SCORE));
        let entry = entry_item(&plugin, gh, PREFIX_SCORE);
        let id = "plugins:dev.helmp.quicklinks/gh";
        assert_eq!(entry.id, id);
        assert_eq!(entry.opens_scope.as_deref(), Some(id));
        assert!(entry.form.is_none(), "入口不能带表单");
        assert!(!entry.subtitle.contains("github.com"), "入口不能给出可执行的网址：{}", entry.subtitle);
        assert_eq!(entry.action.run().unwrap(), Outcome::EnterScope(id.into()));
        assert!(entry.action.keeps_open(), "进入范围不该先收起面板");

        // 范围里搜索框的字才是 {{query}}：给出编码正确的网址，回车打开。
        let ready = command_item(&plugin, gh, PREFIX_SCORE, "a&b");
        assert_eq!(ready.subtitle, "https://github.com/search?q=a%26b&type=repositories");
        assert_eq!(ready.id, id);
        assert!(ready.opens_scope.is_none(), "范围里的结果不能再进范围");
    }

    #[test]
    fn command_scopes_name_the_plugin_and_explain_the_input() {
        // 进入范围后搜索栏左侧的标签要让用户知道进了哪个插件：只有一个命令的插件写插件名（flomo），
        // 快捷搜索这类多命令插件写命令名；颜色跟命令图标；占位文字说明要输入什么。
        crate::i18n::with_lang(crate::i18n::Lang::Zh, || {
            let flomo = bundled("dev.helmp.flomo");
            let scope = command_scope(&flomo, flomo.manifest.command("memo").unwrap());
            assert_eq!(scope.id, "plugins:dev.helmp.flomo/memo");
            assert_eq!(scope.label, "flomo");
            assert_eq!(scope.tint, Tint::Green);
            assert!(scope.placeholder.contains("回车提交"), "{}", scope.placeholder);
            assert!(scope.prefixes.is_empty(), "命令范围不能靠打字前缀进入（那就又是误触发）");
            let quicklinks = bundled("dev.helmp.quicklinks");
            let scope = command_scope(&quicklinks, quicklinks.manifest.command("gh").unwrap());
            assert_eq!(scope.label, "GitHub 仓库");
            assert_eq!(scope.placeholder, "仓库名");
            assert_eq!(scope.tint, Tint::Graphite);
        });
    }

    #[test]
    fn chinese_titles_match_by_pinyin() {
        // 中文标题的命令要像应用一样能用拼音和首字母找到。
        let plugin = bundled("dev.helmp.quicklinks");
        let xhs = plugin.manifest.command("xhs").unwrap();
        for query in ["xiaohongshu", "小红书", "xhs"] {
            assert!(match_command(xhs, query).is_some(), "{query}");
        }
        let gt = plugin.manifest.command("gt").unwrap();
        assert!(match_command(gt, "ggfy").is_some(), "谷歌翻译的首字母");
    }

    #[test]
    fn form_defaults_render_the_query() {
        // flomo 范围里的文字（深链接的 q、范围里打的字）带进表单，少打一遍。
        let plugin = bundled("dev.helmp.flomo");
        let memo = plugin.manifest.command("memo").unwrap();
        let form = form_for(&plugin, memo, "今天学到了").unwrap();
        assert_eq!(form.fields[0].value, "今天学到了");
        assert!(matches!(
            form.fields[0].kind,
            FieldKind::Text {
                multiline: true,
                ..
            }
        ));
    }

    #[test]
    fn deep_links_only_run_open_commands_directly() {
        // 网页里的链接也能触发 helmp://：直接执行只限于「打开一个链接」；
        // 提交 flomo、跑脚本这类命令只能显示出来，等用户自己回车。
        let quicklinks = bundled("dev.helmp.quicklinks");
        let npm = quicklinks.manifest.command("npm").unwrap();
        assert!(link_runs_now(npm, "react"));
        assert!(
            !link_runs_now(npm, ""),
            "缺必填参数时不能直接打开一个空搜索"
        );
        let flomo = bundled("dev.helmp.flomo");
        assert!(!link_runs_now(
            flomo.manifest.command("memo").unwrap(),
            "hello"
        ));
        let obsidian = bundled("dev.helmp.obsidian");
        assert!(!link_runs_now(
            obsidian.manifest.command("capture").unwrap(),
            "hello"
        ));
    }

    #[test]
    fn manage_command_opens_the_plugins_page() {
        let item = manage_item("插件管理").unwrap();
        assert_eq!(
            item.action.run().unwrap(),
            Outcome::OpenSettingsPage(Page::Plugins)
        );
        assert!(manage_item("manage plugins").is_some());
    }

    #[test]
    fn symbol_icons_of_bundled_plugins_exist() {
        // 图标名拼错时界面只会画一个空白方块，不会报错。
        for entry in registry::load_bundled() {
            let plugin = entry.plugin().unwrap();
            for command in &plugin.manifest.commands {
                if let Icon::Symbol { name, .. } = command_icon(plugin, command) {
                    assert!(manifest::symbol_exists(name), "{name}");
                }
            }
        }
    }
}
