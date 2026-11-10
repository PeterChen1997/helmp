//! 文本片段（Snippets）：常用文字存起来，搜名字 / 关键词 / 正文（含拼音）找到，回车粘贴到前台 App。
//!
//! - 范围「片段」：前缀 `sn ` / `片段 `。空查询按名字列出全部；列表末尾总有「新建片段」「导入片段」。
//! - 全局搜索：「片段」「新建片段」「导入片段」等命令入口。
//! - 回车：展开占位符（见 [`template`]），写进剪贴板并粘贴到唤起 Helmp 之前的 App（与剪贴板历史同一条
//!   粘贴路径：需要辅助功能权限，没有时只复制并提示），约 300 ms 后还原原来的纯文本剪贴板；
//!   `{cursor}` 粘贴后用 ← 键把光标移回去。带 `{argument}` 的片段先出表单。
//! - ⌘K：复制（⌘C）、编辑（⌘E，打开设置 › 片段并选中它）、复制一份（⌘D）、删除（⌘⌫，先确认）。
//! - 存储在 `<数据目录>/snippets.json`（见 [`store`]）；设置 › 片段 可增删改、从 Raycast / Alfred 导入。
//! - 自动展开（在任意 App 里打关键词自动替换）v1 不做：需要全局键盘监听（输入监控权限），留到以后。
//! - 测试接缝：`HELMP_NOW`（Unix 秒，或 `2026-11-10T12:00:00Z` / 不带 Z 的本地时间）固定「现在」；
//!   `HELMP_CLIPBOARD_FIXTURE=<文件>` 提供 `{clipboard}` 的内容。演练模式下没给夹具时不读真实剪贴板，
//!   `{clipboard}` 原样保留。

mod store;
pub mod template;

pub use store::{ImportReport, Snippet};

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::Result;

use crate::i18n::{tr, trf};
use crate::matcher;
use crate::provider::{
    Accessory, Action, ActionInput, Block, Detail, Field, FieldKind, Form, Icon, Item,
    NamedAction, Outcome, Provider, Scope, Tint, Tone,
};
use template::{Ctx, Expanded, Now};

const PROVIDER_ID: &str = "snippets";
const PREFIXES: [&str; 2] = ["sn ", "片段 "];
const TINT: Tint = Tint::Purple;
const ENTRY_KEYWORDS: [&str; 7] = ["片段", "文本片段", "常用语", "模板", "snippet", "snippets", "template"];
const NEW_KEYWORDS: [&str; 5] = ["新建片段", "添加片段", "new snippet", "create snippet", "add snippet"];
const IMPORT_KEYWORDS: [&str; 5] = [
    "导入片段",
    "import snippets",
    "raycast snippets",
    "alfred snippets",
    "导入 raycast 片段",
];
const NOW_ENV: &str = "HELMP_NOW";
const CLIPBOARD_FIXTURE_ENV: &str = "HELMP_CLIPBOARD_FIXTURE";

/// 「编辑」时要在设置 › 片段里选中的片段 id（设置窗口打开后取走）。
static PENDING_EDIT: Mutex<Option<String>> = Mutex::new(None);

/// 设置窗口取走「编辑」要选中的片段。
pub fn take_pending_edit() -> Option<String> {
    PENDING_EDIT.lock().expect("snippet edit poisoned").take()
}

pub fn load() -> Result<Vec<Snippet>> {
    store::load()
}

/// 片段文件的修改时间（设置页据此发现启动器里新建 / 删除的片段）。
pub fn modified() -> Option<SystemTime> {
    store::path()
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok())
}

/// 修改全部片段并保存（设置窗口用）。
pub fn update<T>(f: impl FnOnce(&mut Vec<Snippet>) -> Result<T>) -> Result<T> {
    store::update(f)
}

pub fn new_snippet() -> Snippet {
    Snippet {
        id: store::new_id(),
        name: String::new(),
        keyword: String::new(),
        content: String::new(),
        tags: Vec::new(),
    }
}

pub fn validate(snippets: &[Snippet], snippet: &Snippet) -> Result<()> {
    store::validate(snippets, snippet)
}

pub fn parse_tags(text: &str) -> Vec<String> {
    store::parse_tags(text)
}

/// 从 Raycast / Alfred 的导出文件导入，返回新增与跳过的条数。
pub fn import(path: &std::path::Path) -> Result<ImportReport> {
    let incoming = store::read_import(path)?;
    store::update(|snippets| Ok(store::merge(snippets, incoming)))
}

/// (文件修改时间, 片段)：文件没变就不重新读。
type Cache = Option<(Option<SystemTime>, Arc<Vec<Snippet>>)>;

#[derive(Default)]
pub struct Snippets {
    cache: Mutex<Cache>,
}

impl Snippets {
    fn snippets(&self) -> Result<Arc<Vec<Snippet>>> {
        let path = store::path()?;
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let mut cache = self.cache.lock().expect("snippet cache poisoned");
        if let Some((at, snippets)) = cache.as_ref()
            && *at == modified
            && modified.is_some()
        {
            return Ok(Arc::clone(snippets));
        }
        let snippets = Arc::new(store::load_from(&path)?);
        *cache = Some((modified, Arc::clone(&snippets)));
        Ok(snippets)
    }
}

impl Provider for Snippets {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let text = query.trim();
        if text.is_empty() {
            return Vec::new();
        }
        let mut items = Vec::new();
        if let Some(score) = best(text, &ENTRY_KEYWORDS) {
            items.push(Item {
                id: "snippets:open".into(),
                title: tr("文本片段", "Snippets").into(),
                subtitle: tr("搜索并粘贴常用文字", "Search and paste saved text").into(),
                badge: badge().into(),
                score,
                icon: symbol("text-quote"),
                action: Action::enter_scope(PROVIDER_ID),
                ..Item::blank()
            });
        }
        if let Some(score) = best(text, &NEW_KEYWORDS) {
            items.push(new_item(String::new(), score));
        }
        if let Some(score) = best(text, &IMPORT_KEYWORDS) {
            items.push(import_item(score));
        }
        items
    }

    fn scope(&self) -> Option<Scope> {
        let count = self.snippets().map(|s| s.len()).unwrap_or(0);
        Some(Scope {
            id: PROVIDER_ID,
            label: tr("片段", "Snippets").into(),
            tint: TINT,
            prefixes: PREFIXES.iter().map(|p| p.to_string()).collect(),
            placeholder: trf!(
                "搜索 {count} 个片段",
                "Search {count} snippets"
            ),
            filters: Vec::new(),
        })
    }

    fn query_scoped(&self, query: &str, _filter: &str) -> Vec<Item> {
        let all = match self.snippets() {
            Ok(all) => all,
            Err(err) => {
                return vec![Item {
                    id: "snippets:error".into(),
                    title: tr("读取片段失败", "Couldn’t load snippets").into(),
                    subtitle: format!("{err:#}"),
                    badge: badge().into(),
                    icon: symbol("circle-x"),
                    ..Item::blank()
                }];
            }
        };
        let text = query.trim();
        let mut hits: Vec<(usize, u32)> = all
            .iter()
            .enumerate()
            .filter_map(|(index, snippet)| match_score(snippet, text).map(|score| (index, score)))
            .collect();
        hits.sort_by(|(a, sa), (b, sb)| sb.cmp(sa).then(all[*a].name.cmp(&all[*b].name)));
        let mut items: Vec<Item> = hits
            .into_iter()
            .enumerate()
            .map(|(rank, (index, _))| snippet_item(&all[index], &all, 10_000 - rank as u32))
            .collect();
        // 搜不到时「新建片段」排第一，名字预填成刚才输入的内容。
        let tail = if items.is_empty() { 20_000 } else { 2 };
        items.push(new_item(text.to_string(), tail));
        items.push(import_item(tail - 1));
        items
    }
}

fn badge() -> &'static str {
    tr("片段", "Snippets")
}

fn symbol(name: &'static str) -> Icon {
    Icon::Symbol { name, tint: TINT }
}

fn best(text: &str, keywords: &[&str]) -> Option<u32> {
    keywords
        .iter()
        .filter_map(|keyword| matcher::score(text, keyword))
        .max()
}

/// 关键词完全一致最高；其次名字、关键词、标签（含拼音）；最后正文子串。空查询全部命中。
fn match_score(snippet: &Snippet, text: &str) -> Option<u32> {
    if text.is_empty() {
        return Some(0);
    }
    if !snippet.keyword.is_empty() && snippet.keyword.eq_ignore_ascii_case(text) {
        return Some(2000);
    }
    let named = [snippet.name.as_str(), snippet.keyword.as_str()]
        .into_iter()
        .filter(|candidate| !candidate.is_empty())
        .filter_map(|candidate| matcher::score(text, candidate));
    let tagged = snippet
        .tags
        .iter()
        .filter_map(|tag| matcher::score(text, tag))
        .map(|score| score.saturating_sub(100));
    let body = snippet
        .content
        .to_lowercase()
        .contains(&text.to_lowercase())
        .then_some(300);
    named.chain(tagged).chain(body).max()
}

fn first_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    match line.char_indices().nth(80) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_string(),
    }
}

fn snippet_item(snippet: &Snippet, all: &Arc<Vec<Snippet>>, score: u32) -> Item {
    let arguments = template::arguments(&snippet.content);
    let preview_args: BTreeMap<String, String> = BTreeMap::new();
    let preview = expand_now(&snippet.content, all, &preview_args, false);
    let mut metadata = Vec::new();
    if !snippet.keyword.is_empty() {
        metadata.push((tr("关键词", "Keyword").into(), snippet.keyword.clone()));
    }
    if !snippet.tags.is_empty() {
        metadata.push((tr("标签", "Tags").into(), snippet.tags.join(tr("、", ", "))));
    }
    let form = (!arguments.is_empty()).then(|| Form {
        fields: arguments
            .iter()
            .map(|argument| Field {
                key: argument.name.clone(),
                label: argument.name.clone(),
                kind: FieldKind::Text {
                    placeholder: argument.default.clone().unwrap_or_default(),
                    multiline: false,
                },
                value: argument.default.clone().unwrap_or_default(),
                required: argument.default.is_none(),
            })
            .collect(),
    });
    let paste = {
        let (content, all) = (snippet.content.clone(), Arc::clone(all));
        Action::with_input(move |input: &ActionInput| {
            let expanded = expand_now(&content, &all, &input.form, true);
            crate::providers::clipboard::paste_text_then(
                &expanded.text,
                expanded.cursor_back.unwrap_or(0),
            )
        })
    };
    let copy = {
        let (content, all) = (snippet.content.clone(), Arc::clone(all));
        Action::with_input(move |input: &ActionInput| {
            Action::copy(expand_now(&content, &all, &input.form, true).text).run()
        })
    };
    let edit = {
        let id = snippet.id.clone();
        Action::new(move || {
            *PENDING_EDIT.lock().expect("snippet edit poisoned") = Some(id.clone());
            Ok(Outcome::OpenSettingsPage(crate::settings::Page::Snippets))
        })
    };
    let duplicate = {
        let original = snippet.clone();
        Action::new(move || {
            let copy = Snippet {
                id: store::new_id(),
                name: trf!("{} 副本", "{} Copy", original.name),
                keyword: String::new(),
                ..original.clone()
            };
            store::update(|list| {
                list.push(copy);
                Ok(())
            })?;
            Ok(Outcome::Done)
        })
        .keep_open()
    };
    let delete = {
        let (id, name) = (snippet.id.clone(), snippet.name.clone());
        let run = Action::new(move || {
            store::update(|list| {
                let before = list.len();
                list.retain(|s| s.id != id);
                anyhow::ensure!(
                    list.len() < before,
                    trf!("片段「{name}」已经不在了", "The snippet “{name}” no longer exists")
                );
                Ok(())
            })?;
            Ok(Outcome::Dismiss)
        })
        .keep_open();
        let detail = Detail::confirm(
            symbol("text-quote"),
            trf!("删除片段「{}」？", "Delete the snippet “{}”?", snippet.name),
            tr("删除后无法恢复。", "It can’t be recovered once deleted.").into(),
            tr("删除", "Delete").into(),
            run,
        );
        Action::new(move || Ok(Outcome::Show(detail.clone()))).keep_open()
    };
    Item {
        id: format!("{PROVIDER_ID}:{}", snippet.id),
        title: snippet.name.clone(),
        subtitle: first_line(&snippet.content),
        badge: badge().into(),
        score,
        icon: symbol("text-quote"),
        accessories: if snippet.keyword.is_empty() {
            Vec::new()
        } else {
            vec![Accessory::Tag {
                text: snippet.keyword.clone(),
                tone: Tone::Neutral,
            }]
        },
        action: paste,
        action_title: tr("粘贴", "Paste").into(),
        actions: vec![
            NamedAction::new(tr("复制", "Copy"), Some("⌘C"), copy),
            NamedAction::new(tr("编辑", "Edit"), Some("⌘E"), edit),
            NamedAction::new(tr("复制一份", "Duplicate"), Some("⌘D"), duplicate),
            NamedAction::new(tr("删除", "Delete"), Some("⌘⌫"), delete),
        ],
        detail: Some(Detail {
            icon: symbol("text-quote"),
            title: snippet.name.clone(),
            blocks: vec![Block::Text(preview.text)],
            metadata,
            ..Detail::default()
        }),
        form,
        ..Item::blank()
    }
}

/// 「新建片段」表单。
fn new_item(name: String, score: u32) -> Item {
    let text = |key: &str, label: &str, placeholder: &str, value: String, multiline, required| Field {
        key: key.into(),
        label: label.into(),
        kind: FieldKind::Text {
            placeholder: placeholder.into(),
            multiline,
        },
        value,
        required,
    };
    Item {
        id: "snippets:new".into(),
        title: tr("新建片段", "Create Snippet").into(),
        subtitle: tr(
            "支持 {date} {time} {clipboard} {cursor} {uuid} {argument name=\"…\"}",
            "Supports {date} {time} {clipboard} {cursor} {uuid} {argument name=\"…\"}",
        )
        .into(),
        badge: badge().into(),
        score,
        icon: symbol("file-plus"),
        action: Action::with_input(|input| {
            let get = |key: &str| input.form.get(key).cloned().unwrap_or_default();
            let snippet = Snippet {
                name: get("name").trim().to_string(),
                keyword: get("keyword").trim().to_string(),
                content: get("content"),
                tags: store::parse_tags(&get("tags")),
                ..new_snippet()
            };
            let name = snippet.name.clone();
            store::update(|list| {
                store::validate(list, &snippet)?;
                list.push(snippet);
                Ok(())
            })?;
            Ok(Outcome::Message(trf!(
                "已保存片段「{name}」",
                "Saved the snippet “{name}”"
            )))
        }),
        action_title: tr("保存", "Save").into(),
        form: Some(Form {
            fields: vec![
                text("name", tr("名称", "Name"), tr("如 邮件签名", "e.g. Email Signature"), name, false, true),
                text("keyword", tr("关键词", "Keyword"), tr("可选，如 qm", "Optional, e.g. sig"), String::new(), false, false),
                text("content", tr("内容", "Content"), tr("要粘贴的文字", "Text to paste"), String::new(), true, true),
                text("tags", tr("标签", "Tags"), tr("可选，逗号分隔", "Optional, comma-separated"), String::new(), false, false),
            ],
        }),
        ..Item::blank()
    }
}

/// 「导入片段」表单：填 Raycast / Alfred 导出文件的路径。
fn import_item(score: u32) -> Item {
    Item {
        id: "snippets:import".into(),
        title: tr("导入片段", "Import Snippets").into(),
        subtitle: tr(
            "从 Raycast 导出的 JSON 或 Alfred 的 .alfredsnippets 导入",
            "From Raycast’s exported JSON or Alfred’s .alfredsnippets",
        )
        .into(),
        badge: badge().into(),
        score,
        icon: symbol("import"),
        action: Action::with_input(|input| {
            let raw = input.form.get("path").cloned().unwrap_or_default();
            let path = expand_home(raw.trim());
            let report = import(&path)?;
            Ok(Outcome::Message(trf!(
                "导入了 {} 个片段，跳过 {} 个重复的",
                "Imported {} snippets, skipped {} duplicates",
                report.added,
                report.skipped
            )))
        }),
        action_title: tr("导入", "Import").into(),
        form: Some(Form {
            fields: vec![Field {
                key: "path".into(),
                label: tr("文件路径", "File Path").into(),
                kind: FieldKind::Text {
                    placeholder: "~/Downloads/snippets.json".into(),
                    multiline: false,
                },
                value: String::new(),
                required: true,
            }],
        }),
        ..Item::blank()
    }
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => crate::paths::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|_| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

/// 用此刻的时间、剪贴板展开。`read_clipboard` 为假（列表预览）时不读剪贴板。
fn expand_now(
    content: &str,
    all: &[Snippet],
    args: &BTreeMap<String, String>,
    read_clipboard: bool,
) -> Expanded {
    let clipboard = if read_clipboard { clipboard_text() } else { None };
    let lookup = |name: &str| {
        all.iter()
            .find(|snippet| snippet.name == name)
            .map(|snippet| snippet.content.clone())
    };
    template::expand(
        content,
        &Ctx {
            now: now(),
            clipboard: clipboard.as_deref(),
            args,
            snippet: &lookup,
            uuid: &store::uuid_v4,
        },
    )
}

/// `{clipboard}` 的内容：夹具优先；演练模式不读真实剪贴板。
fn clipboard_text() -> Option<String> {
    if let Some(path) = std::env::var_os(CLIPBOARD_FIXTURE_ENV) {
        return std::fs::read_to_string(path).ok();
    }
    if crate::effects::dry_run() {
        return None;
    }
    crate::providers::clipboard::current_text()
}

/// 此刻的本地时间；`HELMP_NOW` 可固定（测试用）。
fn now() -> Now {
    if let Ok(value) = std::env::var(NOW_ENV)
        && let Some(now) = parse_now(&value)
    {
        return now;
    }
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    local(seconds)
}

/// `1731200000`（Unix 秒，按本地时区）、`2026-11-10T12:00:00Z`（UTC，按本地时区）、
/// `2026-11-10T12:00:00`（就是本地时间）。
fn parse_now(value: &str) -> Option<Now> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<i64>() {
        return Some(local(seconds));
    }
    let (body, utc) = match value.strip_suffix('Z') {
        Some(body) => (body, true),
        None => (value, false),
    };
    let (date, time) = body.split_once('T')?;
    let mut d = date.split('-').map(str::parse::<i64>);
    let mut t = time.split(':').map(str::parse::<i64>);
    let (year, month, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let (hour, minute) = (t.next()?.ok()?, t.next()?.ok()?);
    let second = t.next().and_then(Result::ok).unwrap_or(0);
    if utc {
        let days = days_from_civil(year, month, day);
        return Some(local(days * 86_400 + hour * 3600 + minute * 60 + second));
    }
    Some(Now {
        year: i32::try_from(year).ok()?,
        month: u32::try_from(month).ok()?,
        day: u32::try_from(day).ok()?,
        hour: u32::try_from(hour).ok()?,
        minute: u32::try_from(minute).ok()?,
        second: u32::try_from(second).ok()?,
    })
}

/// 公历日期 → 1970-01-01 起的天数（Howard Hinnant 的算法）。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn local(seconds: i64) -> Now {
    let time: libc::time_t = seconds;
    // SAFETY: tm 是输出参数，全部字段由 localtime_r 填写。
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: 两个指针都指向本函数里的有效变量。
    unsafe { libc::localtime_r(&time, &mut tm) };
    Now {
        year: tm.tm_year + 1900,
        month: u32::try_from(tm.tm_mon + 1).unwrap_or(1),
        day: u32::try_from(tm.tm_mday).unwrap_or(1),
        hour: u32::try_from(tm.tm_hour).unwrap_or(0),
        minute: u32::try_from(tm.tm_min).unwrap_or(0),
        second: u32::try_from(tm.tm_sec).unwrap_or(0),
    }
}

/// 设置页显示用：现在展开会得到什么（不读剪贴板）。
pub fn preview(content: &str, all: &[Snippet]) -> String {
    expand_now(content, all, &BTreeMap::new(), false).text
}

/// 设置页导入失败时给出的说明。
pub fn import_message(result: &Result<ImportReport>) -> String {
    match result {
        Ok(report) => trf!(
            "导入了 {} 个片段，跳过 {} 个重复的",
            "Imported {} snippets, skipped {} duplicates",
            report.added,
            report.skipped
        ),
        Err(err) => trf!("导入失败：{err:#}", "Import failed: {err:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet(name: &str, keyword: &str, content: &str, tags: &[&str]) -> Snippet {
        Snippet {
            id: name.into(),
            name: name.into(),
            keyword: keyword.into(),
            content: content.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn search_by_keyword_name_pinyin_tags_and_body() {
        // 片段要能用各种记忆线索找到：关键词最准，其次名字（含拼音首字母），再次标签和正文。
        let sig = snippet("邮件签名", "sig", "— 陈\nhelmp.dev", &["工作"]);
        assert_eq!(match_score(&sig, "sig"), Some(2000));
        assert!(match_score(&sig, "yjqm").is_some(), "拼音首字母");
        assert!(match_score(&sig, "youjian").is_some(), "全拼");
        assert!(match_score(&sig, "gz").is_some(), "标签");
        assert_eq!(match_score(&sig, "helmp.dev"), Some(300), "正文");
        assert_eq!(match_score(&sig, ""), Some(0));
        assert_eq!(match_score(&sig, "zzzz"), None);
        assert!(match_score(&sig, "sig").unwrap() > match_score(&sig, "yjqm").unwrap());
    }

    #[test]
    fn helmp_now_fixes_the_clock() {
        // 测试要固定「现在」，{date} 才能断言；三种写法都要认。
        assert_eq!(
            parse_now("2026-11-10T08:30:00"),
            Some(Now { year: 2026, month: 11, day: 10, hour: 8, minute: 30, second: 0 })
        );
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 11, 10), 20_767);
        assert!(parse_now("2026-11-10T12:00:00Z").is_some());
        assert!(parse_now("1731200000").is_some());
        assert_eq!(parse_now("nope"), None);
    }

    #[test]
    fn arguments_turn_into_a_form() {
        // 带 {argument} 的片段回车前必须先填表单，否则会把空白粘进去。
        let all = Arc::new(vec![snippet("回信", "", "Hi {argument name=\"收件人\"}", &[])]);
        let item = snippet_item(&all[0], &all, 1);
        let form = item.form.expect("应该有表单");
        assert_eq!(form.fields.len(), 1);
        assert_eq!(form.fields[0].key, "收件人");
        assert!(form.fields[0].required);
        let plain = snippet_item(&snippet("签名", "", "— 陈", &[]), &all, 1);
        assert!(plain.form.is_none());
    }

    #[test]
    fn symbol_icons_exist_in_bundled_assets() {
        use gpui_kit::AssetSource as _;
        for name in ["text-quote", "file-plus", "import", "circle-x"] {
            let path = format!("icons/{name}.svg");
            assert!(
                gpui_kit::assets::AllAssets.load(&path).unwrap().is_some(),
                "缺少图标 {path}"
            );
        }
    }
}
