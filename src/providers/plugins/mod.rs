//! 清单插件（L1.5）：一个文件夹 + 一份声明式的 `plugin.json`，不写代码就能接 flomo、Obsidian、
//! 各类「关键词 → 网址」；动态列表用脚本输出 Alfred Script Filter JSON。
//!
//! - 清单格式、校验：`manifest`；模板语法：`template`；权限推导：`permissions`。
//! - 发现、启用状态、热重载：`registry`（自带插件编译进来，用户插件在 `<data_dir>/plugins/`）。
//! - 执行动作：`exec`；secret 偏好设置：`secrets`（只存钥匙串）。
//! - 每个命令一个结果，id 为 `plugins:<插件 id>/<命令 id>`；列表命令的每一项为 `…#<uid>`。
//! - 匹配：命令标题（中英、拼音、首字母都行）与 keywords；命令接受输入（argument / form / list）时，
//!   「关键词 + 空格 + 文字」把文字作为 `{{query}}`。
//! - 另有「插件管理」命令，打开设置 › 插件。
//!
//! 开发者文档：docs/plugins.md。

pub mod exec;
pub mod manifest;
pub mod permissions;
pub mod registry;
pub mod scaffold;
pub mod secrets;
pub mod template;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::i18n::{tr, trf};
use crate::matcher;
use crate::provider::{
    Accessory, Action, ActionInput, Block, Detail, Field, FieldKind, Form, Icon, Item, NamedAction,
    Outcome, Provider, Tint,
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
                let Some((score, rest)) = match_command(command, query) else {
                    continue;
                };
                match (&command.list, rest) {
                    (Some(list), Some(rest)) if rest.chars().count() >= list.min_query_length => {
                        items.extend(self.list_items(plugin, command, list, &rest));
                    }
                    (_, rest) => items.push(command_item(
                        plugin,
                        command,
                        score,
                        rest.as_deref().unwrap_or_default(),
                    )),
                }
            }
        }
        items
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
fn accepts_input(command: &Command) -> bool {
    command.argument.is_some() || !command.form.is_empty() || command.list.is_some()
}

/// (分数, 关键词后面的文字)。文字为 None 表示只是模糊命中了名字。
fn match_command(command: &Command, query: &str) -> Option<(u32, Option<String>)> {
    if accepts_input(command) {
        for name in names(command) {
            if let Some(rest) = strip_name(query, name) {
                return Some((PREFIX_SCORE, Some(rest.to_string())));
            }
        }
    }
    // 只认字面或拼音命中，不认「字母按顺序出现」的模糊匹配：插件命令名普遍较长，
    // 模糊匹配会让 uuid 这类查询冒出「Obsidian 快速记录」之类毫不相关的结果。
    names(command)
        .into_iter()
        .filter_map(|name| matcher::score(query, name))
        .filter(|score| *score >= MIN_NAME_SCORE)
        .max()
        .map(|score| (score, None))
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
    let hint = || {
        let what = command
            .argument
            .as_ref()
            .and_then(|argument| argument.placeholder.as_ref())
            .map(|placeholder| placeholder.get().to_string())?;
        let keyword = trigger(command);
        Some(trf!(
            "输入 {keyword} 加空格，再输入{what}",
            "Type {keyword}, a space, then {what}"
        ))
    };
    let subtitle = preview
        .or_else(|| command.subtitle.as_ref().map(|text| text.get().to_string()))
        .or_else(|| {
            command.list.is_some().then(|| {
                trf!(
                    "输入 {} 加空格开始搜索",
                    "Type {} and a space to search",
                    trigger(command)
                )
            })
        })
        .or_else(hint)
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
        id: format!("{PROVIDER_ID}:{}/{}", plugin.id(), command.id),
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
            "输入 {} 加空格，再输入要找的内容",
            "Type {}, a space, then what you're looking for",
            trigger(command)
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
            "「{title}」需要输入内容：输入 {} 加空格，再输入{what}",
            "“{title}” needs input: type {}, a space, then {what}",
            trigger(command)
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
    /// 在搜索框里显示的查询（关键词 + 空格 + q），让面板出现同一个结果。
    pub query: String,
    /// 只打开链接、不需要表单的命令直接执行；会发请求、跑脚本、碰剪贴板的命令只显示出来，等用户回车
    /// （网页里的链接也能触发深链接，不能让它替用户提交内容）。
    pub run_now: bool,
}

pub fn link_target(plugin_id: &str, command_id: &str, q: &str) -> Result<LinkTarget, String> {
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
    let plugin = entry
        .plugin()
        .ok_or_else(|| trf!("插件 {plugin_id} 出错", "Plugin {plugin_id} has errors"))?;
    let command = plugin.manifest.command(command_id).ok_or_else(|| {
        trf!(
            "插件 {plugin_id} 没有命令 {command_id}",
            "Plugin {plugin_id} has no command {command_id}"
        )
    })?;
    let keyword = trigger(command);
    let query = if q.is_empty() {
        keyword
    } else {
        format!("{keyword} {q}")
    };
    Ok(LinkTarget {
        item: command_item(plugin, command, PREFIX_SCORE, q),
        query,
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
    fn keyword_plus_space_passes_the_rest_as_query() {
        // 「gh react」要把 react 当查询，而不是去模糊匹配整句。
        let plugin = bundled("dev.helmp.quicklinks");
        let gh = plugin.manifest.command("gh").unwrap();
        assert_eq!(
            match_command(gh, "gh react hooks"),
            Some((PREFIX_SCORE, Some("react hooks".into())))
        );
        // `ghx` 不是 `gh` + 参数。
        assert_ne!(
            match_command(gh, "ghx").and_then(|(_, rest)| rest),
            Some("x".into())
        );
        let item = command_item(&plugin, gh, PREFIX_SCORE, "a&b");
        assert_eq!(
            item.subtitle,
            "https://github.com/search?q=a%26b&type=repositories"
        );
        assert_eq!(item.id, "plugins:dev.helmp.quicklinks/gh");
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
        // 「flomo 今天学到了」直接把文字带进表单，少打一遍。
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
