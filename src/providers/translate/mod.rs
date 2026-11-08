//! 翻译（设计稿 Translate）：用用户自己接入的模型翻译输入或前台选中的文字。
//!
//! - 范围 `translate`：前缀 `fy ` / `tr ` / `翻译 `。范围内只有一项，带表单：
//!   原文（初值 = 输入；为空时取前台应用选中的文字）、源语言、目标语言、模型。
//!   回车 → 调模型 → `Outcome::Show` 在右侧显示译文。
//! - 没接入任何模型：项提示去设置，回车 `Outcome::OpenSettings`。
//! - 首页「快捷命令」里有「翻译」入口；全局搜索 `翻译` / `fy` / `translate` 也能找到它。
//! - ⌘K：复制上次译文（⌘⇧C）、打开模型设置。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

use crate::config::{self, Config, ModelConfig};
use crate::engine::COMMANDS_SECTION;
use crate::matcher;
use crate::models;
use crate::provider::{
    Accessory, Action, ActionInput, Block, Detail, Field, FieldKind, Form, Icon, Item, NamedAction,
    Outcome, Provider, Scope, Tint,
};

const SCOPE: &str = "translate";
const BADGE: &str = "翻译";
const ICON: Icon = Icon::Symbol {
    name: "languages",
    tint: Tint::Teal,
};
/// 打开翻译的全局快捷键（显示用；⌘⇧T 会抢浏览器「恢复标签页」，改成 ⌥⇧T）。
const HOTKEY: &str = "⌥⇧T";
const NO_MODEL: &str = "先在设置 › 模型里接入一个模型";

/// 选中文字的缓存时长：面板可见时每秒重查一次，不必每次都走 AX。
const SELECTION_TTL: Duration = Duration::from_millis(1500);

/// 上次的译文，供「复制上次译文」使用。
static LAST: Mutex<Option<String>> = Mutex::new(None);

#[derive(Default)]
pub struct Translate {
    selection: Mutex<Option<(Instant, Option<String>)>>,
}

impl Translate {
    fn selection(&self) -> Option<String> {
        let mut cache = self.selection.lock().expect("selection cache poisoned");
        if let Some((at, text)) = cache.as_ref()
            && at.elapsed() < SELECTION_TTL
        {
            return text.clone();
        }
        let text = models::selected_text();
        *cache = Some((Instant::now(), text.clone()));
        text
    }
}

impl Provider for Translate {
    fn id(&self) -> &'static str {
        SCOPE
    }

    fn scope(&self) -> Option<Scope> {
        Some(Scope {
            id: SCOPE,
            label: "翻译".into(),
            tint: Tint::Teal,
            prefixes: vec!["fy ".into(), "tr ".into(), "翻译 ".into()],
            placeholder: "输入要翻译的文字".into(),
            filters: Vec::new(),
        })
    }

    fn query(&self, query: &str) -> Vec<Item> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        ["翻译", "fy", "translate"]
            .iter()
            .filter_map(|keyword| matcher::score(query, keyword))
            .max()
            .map(|score| vec![entry(BADGE, score)])
            .unwrap_or_default()
    }

    fn query_scoped(&self, query: &str, _filter: &str) -> Vec<Item> {
        let config = config::get();
        if config.models.is_empty() {
            return vec![no_model_item()];
        }
        let typed = query.trim();
        let (text, from_selection) = if typed.is_empty() {
            match self.selection() {
                Some(text) => (text, true),
                None => (String::new(), false),
            }
        } else {
            (typed.to_string(), false)
        };
        vec![form_item(&config, typed, &text, from_selection)]
    }

    fn suggestions(&self) -> Vec<Item> {
        vec![entry(COMMANDS_SECTION, 700)]
    }
}

/// 首页 / 全局搜索里的「翻译」入口：进入翻译范围。
fn entry(badge: &str, score: u32) -> Item {
    Item {
        id: "translate:open".into(),
        title: "翻译".into(),
        subtitle: "用你自己的模型翻译输入或选中的文字".into(),
        badge: badge.into(),
        score,
        icon: ICON,
        accessories: vec![Accessory::keys(HOTKEY)],
        action: Action::enter_scope(SCOPE),
        action_title: "进入".into(),
        ..Item::blank()
    }
}

fn no_model_item() -> Item {
    Item {
        id: "translate:setup".into(),
        title: "翻译".into(),
        subtitle: NO_MODEL.into(),
        badge: BADGE.into(),
        score: 1000,
        icon: ICON,
        action: Action::new(|| Ok(Outcome::OpenSettings)),
        action_title: "打开设置".into(),
        ..Item::blank()
    }
}

/// 表单的初值；CLI 等不带表单值执行时也用它们。
#[derive(Clone, Debug, PartialEq)]
struct Defaults {
    text: String,
    source: String,
    target: String,
    model: String,
}

fn form_item(config: &Config, typed: &str, text: &str, from_selection: bool) -> Item {
    let model = models::translate_model(config)
        .map(|m| m.id.clone())
        .unwrap_or_default();
    let defaults = Defaults {
        text: text.to_string(),
        source: "auto".into(),
        target: config.translate.target.clone(),
        model,
    };
    let languages = || {
        models::LANGUAGES
            .iter()
            .map(|(code, label, _)| (label.to_string(), code.to_string()))
    };
    let mut sources = vec![("自动检测".to_string(), "auto".to_string())];
    sources.extend(languages());
    let mut targets = vec![("中英互译（自动）".to_string(), "auto".to_string())];
    targets.extend(languages());
    let model_options = config
        .models
        .iter()
        .map(|model| (model.name.clone(), model.id.clone()))
        .collect();
    let form = Form {
        fields: vec![
            Field {
                key: "text".into(),
                label: if from_selection {
                    "原文 · 来自选中文字".into()
                } else {
                    "原文".into()
                },
                kind: FieldKind::Text {
                    placeholder: "输入或粘贴要翻译的文字".into(),
                    multiline: true,
                },
                value: defaults.text.clone(),
                required: true,
            },
            select("source", "源语言", sources, &defaults.source),
            select("target", "目标语言", targets, &defaults.target),
            select("model", "模型", model_options, &defaults.model),
        ],
    };
    let model_name = models::translate_model(config)
        .map(|m| m.name.clone())
        .unwrap_or_default();
    let subtitle = if text.is_empty() {
        format!("输入文字后回车 · {model_name}")
    } else {
        format!("{} · {model_name}", one_line(text, 60))
    };
    let run_defaults = defaults.clone();
    Item {
        // 表单状态按 id 缓存：输入变了 id 才变，后台每秒刷新不会冲掉用户在表单里的修改。
        id: format!("translate:{}", digest(typed)),
        title: "翻译".into(),
        subtitle,
        badge: BADGE.into(),
        score: 1000,
        icon: ICON,
        action: Action::with_input(move |input| run(&run_defaults, input)),
        action_title: "翻译".into(),
        actions: vec![
            NamedAction::new("复制上次译文", Some("⌘⇧C"), Action::new(copy_last)),
            NamedAction::new(
                "打开模型设置",
                None,
                Action::new(|| Ok(Outcome::OpenSettings)),
            ),
        ],
        form: Some(form),
        ..Item::blank()
    }
}

fn select(key: &str, label: &str, options: Vec<(String, String)>, value: &str) -> Field {
    Field {
        key: key.into(),
        label: label.into(),
        kind: FieldKind::Select { options },
        value: value.into(),
        required: true,
    }
}

fn digest(text: &str) -> String {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn one_line(text: &str, max: usize) -> String {
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line,
    }
}

/// 表单值优先，缺了（CLI 执行）用初值。
fn value<'a>(input: &'a ActionInput, key: &str, default: &'a str) -> &'a str {
    input.form.get(key).map(String::as_str).unwrap_or(default)
}

fn run(defaults: &Defaults, input: &ActionInput) -> Result<Outcome> {
    let config = config::get();
    if config.models.is_empty() {
        return Ok(Outcome::OpenSettings);
    }
    let text = value(input, "text", &defaults.text).trim();
    if text.is_empty() {
        bail!("没有要翻译的文字：在原文里输入，或先在其他应用里选中文字再唤起");
    }
    let model_id = value(input, "model", &defaults.model);
    let model = config
        .models
        .iter()
        .find(|model| model.id == model_id)
        .or_else(|| models::translate_model(&config))
        .context("找不到所选模型，可能已在设置里移除")?;
    let source = value(input, "source", &defaults.source);
    let target = value(input, "target", &defaults.target);
    Ok(Outcome::Show(translate(model, source, target, text)?))
}

/// 调模型翻译，记下译文，返回详情。
fn translate(model: &ModelConfig, source: &str, target: &str, text: &str) -> Result<Detail> {
    let target = resolve_target(target, text);
    let translation = models::complete(model, &system_prompt(source, target), text)
        .with_context(|| format!("用「{}」翻译失败", model.name))?;
    let translation = translation.trim().to_string();
    *LAST.lock().expect("last translation poisoned") = Some(translation.clone());
    Ok(detail(model, target, translation))
}

fn detail(model: &ModelConfig, target: &str, translation: String) -> Detail {
    let target_label = models::language_label(target).unwrap_or(target);
    Detail {
        icon: ICON,
        title: format!("译文 · {target_label}"),
        tag: None,
        blocks: vec![Block::Text(translation)],
        metadata: vec![
            ("模型".into(), format!("由 {} 翻译", model.name)),
            ("调用".into(), billing_note(model).into()),
        ],
    }
}

fn billing_note(model: &ModelConfig) -> &'static str {
    if models::is_local(model) {
        "本机 Ollama"
    } else if model.kind == config::ModelKind::Ollama {
        "你自己的 Ollama 服务"
    } else {
        "使用你自己的 API Key"
    }
}

/// 汉字占多数（日文假名不算）视为中文。
fn is_mostly_chinese(text: &str) -> bool {
    let han = text
        .chars()
        .filter(|c| matches!(*c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF))
        .count();
    let kana = text
        .chars()
        .filter(|c| matches!(*c as u32, 0x3040..=0x30FF))
        .count();
    let latin = text.chars().filter(char::is_ascii_alphabetic).count();
    han > 0 && kana == 0 && han * 2 >= latin
}

/// `auto` = 中英互译：中文译成英文，其他译成简体中文。在本地决定，标题才能写准「译文 · 英文」。
fn resolve_target<'a>(target: &'a str, text: &str) -> &'a str {
    match target {
        "auto" if is_mostly_chinese(text) => "en",
        "auto" => "zh-Hans",
        other => other,
    }
}

fn system_prompt(source: &str, target: &str) -> String {
    let target = models::language_english(target).unwrap_or(target);
    let source = match source {
        "auto" => String::new(),
        code => format!(
            " The source language is {}.",
            models::language_english(code).unwrap_or(code)
        ),
    };
    format!(
        "You are a translation engine. Translate the user's text into {target}.{source} \
         Output only the translation: no explanations, notes, quotes or the original text. \
         Keep line breaks, Markdown, code, URLs and proper nouns as they are."
    )
}

fn copy_last() -> Result<Outcome> {
    let last = LAST.lock().expect("last translation poisoned").clone();
    match last {
        Some(text) => {
            write_text(&NSPasteboard::generalPasteboard(), &text)?;
            Ok(Outcome::Done)
        }
        None => Ok(Outcome::Message("还没有译文，先翻译一段文字".into())),
    }
}

/// 清空剪贴板并写入纯文本。
fn write_text(pasteboard: &NSPasteboard, text: &str) -> Result<()> {
    pasteboard.clearContents();
    // SAFETY: AppKit 导出的常量 NSString，进程内始终有效。
    let ty = unsafe { NSPasteboardTypeString };
    anyhow::ensure!(
        pasteboard.setString_forType(&NSString::from_str(text), ty),
        "写入剪贴板失败"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ModelKind;

    fn config_with(models: Vec<ModelConfig>) -> Config {
        Config {
            models,
            ..Config::default()
        }
    }

    fn model(id: &str, name: &str, kind: ModelKind) -> ModelConfig {
        ModelConfig {
            id: id.into(),
            name: name.into(),
            kind,
            base_url: String::new(),
            model: "m".into(),
        }
    }

    #[test]
    fn auto_target_swaps_chinese_and_english() {
        // 默认「中英互译」：方向判断错了，用户拿到的是把中文再翻成中文。
        assert_eq!(
            resolve_target("auto", "Rust 的所有权机制让内存安全在编译期得到保证"),
            "en"
        );
        assert_eq!(
            resolve_target("auto", "ownership guarantees memory safety"),
            "zh-Hans"
        );
        assert_eq!(resolve_target("auto", "日本語のテキスト"), "zh-Hans");
        assert_eq!(resolve_target("ja", "你好"), "ja");
    }

    #[test]
    fn form_defaults_come_from_config() {
        // 设置里选的翻译模型和目标语言必须是表单初值，否则「设置」形同虚设。
        let mut config = config_with(vec![
            model("a", "Claude", ModelKind::Anthropic),
            model("b", "Qwen", ModelKind::Ollama),
        ]);
        config.translate.model_id = Some("b".into());
        config.translate.target = "ja".into();
        let item = form_item(&config, "hello", "hello", false);
        let fields = item.form.unwrap().fields;
        let values: Vec<_> = fields
            .iter()
            .map(|f| (f.key.as_str(), f.value.as_str()))
            .collect();
        assert_eq!(
            values,
            [
                ("text", "hello"),
                ("source", "auto"),
                ("target", "ja"),
                ("model", "b")
            ]
        );
        assert_eq!(fields[0].label, "原文");
    }

    #[test]
    fn selection_is_labelled() {
        let config = config_with(vec![model("a", "Claude", ModelKind::Anthropic)]);
        let item = form_item(&config, "", "选中的文字", true);
        let form = item.form.unwrap();
        assert_eq!(form.fields[0].label, "原文 · 来自选中文字");
        assert_eq!(form.fields[0].value, "选中的文字");
    }

    #[test]
    fn form_id_is_stable_for_same_input() {
        // 面板每秒刷新一次结果：id 变了表单就会重建，用户改到一半的原文会被冲掉。
        let config = config_with(vec![model("a", "Claude", ModelKind::Anthropic)]);
        let a = form_item(&config, "", "选中 A", true);
        let b = form_item(&config, "", "选中 B", true);
        assert_eq!(a.id, b.id);
        assert_ne!(a.id, form_item(&config, "hello", "hello", false).id);
    }

    #[test]
    fn without_models_points_to_settings() {
        let item = no_model_item();
        assert_eq!(item.subtitle, NO_MODEL);
        assert!(item.form.is_none());
        assert_eq!(item.action.run().unwrap(), Outcome::OpenSettings);
    }

    #[test]
    fn suggestion_enters_scope_from_commands_section() {
        let item = Translate::default().suggestions().remove(0);
        assert_eq!(item.badge, COMMANDS_SECTION);
        assert!(item.score < 800);
        assert_eq!(item.icon, ICON);
        assert_eq!(item.accessories, vec![Accessory::keys("⌥⇧T")]);
        assert_eq!(item.action.run().unwrap(), Outcome::EnterScope("translate"));
    }

    #[test]
    fn detail_names_model_and_billing() {
        // 用户要知道这次是谁翻的、花的是不是自己的钱。
        let d = detail(
            &model("a", "Claude Haiku 5.5", ModelKind::Anthropic),
            "en",
            "Hi".into(),
        );
        assert_eq!(d.title, "译文 · 英文");
        assert_eq!(d.blocks, vec![Block::Text("Hi".into())]);
        assert_eq!(
            d.metadata,
            vec![
                ("模型".into(), "由 Claude Haiku 5.5 翻译".into()),
                ("调用".into(), "使用你自己的 API Key".into()),
            ]
        );
        let local = detail(
            &model("b", "Qwen", ModelKind::Ollama),
            "zh-Hans",
            "你好".into(),
        );
        assert_eq!(local.metadata[1].1, "本机 Ollama");
    }

    #[test]
    fn translates_through_model_and_remembers_result() {
        // 端到端（mock Ollama）：原文原样作为 user 消息发出、目标语言写进提示、译文进详情并可再复制。
        let (url, rx) = crate::models::client::tests::mock(
            200,
            r#"{"message":{"role":"assistant","content":"  Hello, world \n"},"done":true}"#,
        );
        let mut m = model("o", "Qwen3 8B", ModelKind::Ollama);
        m.base_url = url;
        let d = translate(&m, "auto", "auto", "你好，世界").unwrap();
        assert_eq!(d.title, "译文 · 英文");
        assert_eq!(d.blocks, vec![Block::Text("Hello, world".into())]);
        assert_eq!(LAST.lock().unwrap().as_deref(), Some("Hello, world"));
        let got = rx.recv().unwrap();
        assert_eq!(got.body["messages"][1]["content"], "你好，世界");
        let system = got.body["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("into English"), "{system}");
    }

    #[test]
    fn prompt_names_languages() {
        let prompt = system_prompt("ja", "en");
        assert!(prompt.contains("into English"), "{prompt}");
        assert!(prompt.contains("source language is Japanese"), "{prompt}");
        assert!(!system_prompt("auto", "zh-Hans").contains("source language"));
    }

    #[test]
    fn write_text_uses_private_pasteboard() {
        // 「复制上次译文」写的是纯文本；测试用私有 pasteboard，不碰用户剪贴板。
        let pb = NSPasteboard::pasteboardWithUniqueName();
        write_text(&pb, "Hello 世界").unwrap();
        // SAFETY: AppKit 导出的常量。
        let ty = unsafe { NSPasteboardTypeString };
        let got = pb.stringForType(ty).map(|s| s.to_string());
        // SAFETY: `-[NSPasteboard releaseGlobally]` 无参数无返回值。
        unsafe { objc2::msg_send![&*pb, releaseGlobally] }
        assert_eq!(got.as_deref(), Some("Hello 世界"));
    }
}
