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
use crate::engine::commands_section;
use crate::i18n::{tr, trf};
use crate::matcher;
use crate::models;
use crate::provider::{
    Action, ActionInput, Block, Detail, Field, FieldKind, Form, Icon, Item, NamedAction, Outcome,
    Provider, Scope, Tint,
};

const SCOPE: &str = "translate";
const ICON: Icon = Icon::Symbol {
    name: "languages",
    tint: Tint::Teal,
};

/// 分组名（也是分组键）：在查询里取，查询已按语言固定。
fn badge() -> &'static str {
    tr("翻译", "Translate")
}

fn no_model() -> &'static str {
    tr(
        "先在设置 › 模型里接入一个模型",
        "Add a model in Settings › Models first",
    )
}

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
            label: tr("翻译", "Translate").into(),
            tint: Tint::Teal,
            prefixes: vec!["fy ".into(), "tr ".into(), "翻译 ".into()],
            placeholder: tr("输入要翻译的文字", "Type text to translate").into(),
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
            .map(|score| vec![entry(badge(), score)])
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
        // 设计稿首页顺序：截图识别文字（720）、翻译（710）、剪贴板历史（700）。
        vec![entry(commands_section(), 710)]
    }
}

/// 首页 / 全局搜索里的「翻译」入口：进入翻译范围。
fn entry(badge: &str, score: u32) -> Item {
    Item {
        id: "translate:open".into(),
        title: tr("翻译", "Translate").into(),
        subtitle: tr(
            "用你自己的模型翻译输入或选中的文字",
            "Translate typed or selected text with your own model",
        )
        .into(),
        badge: badge.into(),
        score,
        icon: ICON,
        accessories: crate::hotkey::hint_keys(SCOPE),
        action: Action::enter_scope(SCOPE),
        action_title: tr("进入", "Open").into(),
        ..Item::blank()
    }
}

fn no_model_item() -> Item {
    Item {
        id: "translate:setup".into(),
        title: tr("翻译", "Translate").into(),
        subtitle: no_model().into(),
        badge: badge().into(),
        score: 1000,
        icon: ICON,
        action: Action::new(|| Ok(Outcome::OpenSettings)),
        action_title: tr("打开设置", "Open Settings").into(),
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
            .map(|(code, label, english)| (tr(label, english).to_string(), code.to_string()))
    };
    let mut sources = vec![(
        tr("自动检测", "Detect Language").to_string(),
        "auto".to_string(),
    )];
    sources.extend(languages());
    let mut targets = vec![(
        tr("中英互译（自动）", "Chinese ⇄ English (Auto)").to_string(),
        "auto".to_string(),
    )];
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
                    tr("原文 · 来自选中文字", "Source Text · From Selection").into()
                } else {
                    tr("原文", "Source Text").into()
                },
                kind: FieldKind::Text {
                    placeholder: tr("输入或粘贴要翻译的文字", "Type or paste text to translate")
                        .into(),
                    multiline: true,
                },
                value: defaults.text.clone(),
                required: true,
            },
            select(
                "source",
                tr("源语言", "Source Language"),
                sources,
                &defaults.source,
            ),
            select(
                "target",
                tr("目标语言", "Target Language"),
                targets,
                &defaults.target,
            ),
            select("model", tr("模型", "Model"), model_options, &defaults.model),
        ],
    };
    let model_name = models::translate_model(config)
        .map(|m| m.name.clone())
        .unwrap_or_default();
    let subtitle = if text.is_empty() {
        trf!(
            "输入文字后回车 · {model_name}",
            "Type text, then press Return · {model_name}"
        )
    } else {
        format!("{} · {model_name}", one_line(text, 60))
    };
    let run_defaults = defaults.clone();
    Item {
        // 表单状态按 id 缓存：输入变了 id 才变，后台每秒刷新不会冲掉用户在表单里的修改。
        id: format!("translate:{}", digest(typed)),
        title: tr("翻译", "Translate").into(),
        subtitle,
        badge: badge().into(),
        score: 1000,
        icon: ICON,
        action: Action::with_input(move |input| run(&run_defaults, input)),
        action_title: tr("翻译", "Translate").into(),
        actions: vec![
            NamedAction::new(
                tr("复制上次译文", "Copy Last Translation"),
                Some("⌘⇧C"),
                Action::new(copy_last),
            ),
            NamedAction::new(
                tr("打开模型设置", "Open Model Settings"),
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
        bail!(tr(
            "没有要翻译的文字：在原文里输入，或先在其他应用里选中文字再唤起",
            "Nothing to translate: type some text, or select text in another app before opening Helmp"
        ));
    }
    let model_id = value(input, "model", &defaults.model);
    let model = config
        .models
        .iter()
        .find(|model| model.id == model_id)
        .or_else(|| models::translate_model(&config))
        .context(tr(
            "找不到所选模型，可能已在设置里移除",
            "Can’t find the selected model; it may have been removed in Settings",
        ))?;
    let source = value(input, "source", &defaults.source);
    let target = value(input, "target", &defaults.target);
    Ok(Outcome::Show(translate(model, source, target, text)?))
}

/// 调模型翻译，记下译文，返回详情。
fn translate(model: &ModelConfig, source: &str, target: &str, text: &str) -> Result<Detail> {
    let target = resolve_target(target, text);
    let translation =
        models::complete(model, &system_prompt(source, target), text).with_context(|| {
            trf!(
                "用「{}」翻译失败",
                "Couldn’t translate with “{}”",
                model.name
            )
        })?;
    let translation = translation.trim().to_string();
    *LAST.lock().expect("last translation poisoned") = Some(translation.clone());
    Ok(detail(model, target, translation))
}

fn detail(model: &ModelConfig, target: &str, translation: String) -> Detail {
    let target_label = match crate::i18n::lang() {
        crate::i18n::Lang::Zh => models::language_label(target),
        crate::i18n::Lang::En => models::language_english(target),
    }
    .unwrap_or(target);
    Detail {
        // 出结果后回车复制译文；⌘↵ 复制并替换前台 App 里的选中文字。
        actions: vec![
            NamedAction::new(
                tr("复制译文", "Copy Translation"),
                None,
                Action::copy(translation.clone()),
            ),
            NamedAction::new(
                tr("替换选中文字", "Replace Selection"),
                Some("⌘↵"),
                replace_selection(translation.clone()),
            ),
        ],
        icon: ICON,
        title: trf!("译文 · {target_label}", "Translation · {target_label}"),
        tag: None,
        blocks: vec![Block::Text(translation)],
        metadata: vec![
            (
                tr("模型", "Model").into(),
                trf!("由 {} 翻译", "Translated by {}", model.name),
            ),
            (tr("调用", "Usage").into(), billing_note(model).into()),
        ],
    }
}

/// 复制译文并向前台 App 发送 ⌘V。面板执行这个动作前已经隐藏，按键会落到用户原来的 App。
/// 没有辅助功能权限时只复制，并说明原因。
fn replace_selection(text: String) -> Action {
    Action::new(move || {
        Action::copy(text.clone()).run()?;
        if !crate::providers::clipboard::macos::accessibility_trusted() {
            return Ok(Outcome::Message(
                tr(
                    "译文已复制。没有辅助功能权限，无法自动替换，请手动粘贴（⌘V）。",
                    "Translation copied. Helmp can’t replace the selection without Accessibility permission, so paste it yourself (⌘V).",
                )
                .into(),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
        crate::providers::clipboard::macos::send_paste()?;
        Ok(Outcome::Done)
    })
}

fn billing_note(model: &ModelConfig) -> &'static str {
    if models::is_local(model) {
        tr("本机 Ollama", "Local Ollama")
    } else if model.kind == config::ModelKind::Ollama {
        tr("你自己的 Ollama 服务", "Your own Ollama server")
    } else {
        tr("使用你自己的 API Key", "Your own API key")
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
        None => Ok(Outcome::Message(
            tr(
                "还没有译文，先翻译一段文字",
                "No translation yet. Translate some text first.",
            )
            .into(),
        )),
    }
}

/// 清空剪贴板并写入纯文本。
fn write_text(pasteboard: &NSPasteboard, text: &str) -> Result<()> {
    pasteboard.clearContents();
    // SAFETY: AppKit 导出的常量 NSString，进程内始终有效。
    let ty = unsafe { NSPasteboardTypeString };
    anyhow::ensure!(
        pasteboard.setString_forType(&NSString::from_str(text), ty),
        tr("写入剪贴板失败", "Couldn’t write to the clipboard")
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
        assert_eq!(item.subtitle, no_model());
        assert!(item.form.is_none());
        assert_eq!(item.action.run().unwrap(), Outcome::OpenSettings);
    }

    #[test]
    fn suggestion_enters_scope_from_commands_section() {
        let item = Translate::default().suggestions().remove(0);
        assert_eq!(item.badge, commands_section());
        assert!(item.score < 800);
        assert_eq!(item.icon, ICON);
        // 键帽只显示真正生效的全局快捷键；单测里没有注册任何热键，就不能显示 ⌥⇧T。
        assert!(item.accessories.is_empty(), "{:?}", item.accessories);
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
    fn english_ui_translates_labels_but_keeps_values_and_search() {
        // 英文界面：标题、分组、表单标签和选项显示名换成英文，但选项的值（语言代码）不能变，
        // 否则提交给模型的目标语言就错了；输入中文「翻译」或拼音 fy 仍要找到入口。
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let found = Translate::default().query("翻译");
            let entry = found.first().expect("英文界面输入中文应找到翻译");
            assert_eq!(entry.title, "Translate");
            assert_eq!(entry.badge, "Translate");
            assert!(!Translate::default().query("fy").is_empty());

            let config = config_with(vec![model("a", "Claude", ModelKind::Anthropic)]);
            let fields = form_item(&config, "", "selected", true)
                .form
                .unwrap()
                .fields;
            assert_eq!(fields[0].label, "Source Text · From Selection");
            assert_eq!(fields[1].label, "Source Language");
            let FieldKind::Select { options } = &fields[2].kind else {
                panic!("目标语言应是下拉框");
            };
            assert!(options.contains(&("Chinese ⇄ English (Auto)".into(), "auto".into())));
            assert!(options.contains(&("Japanese".into(), "ja".into())));

            let d = detail(
                &model("a", "Claude Haiku 5.5", ModelKind::Anthropic),
                "en",
                "Hi".into(),
            );
            assert_eq!(d.title, "Translation · English");
            assert_eq!(
                d.metadata,
                vec![
                    ("Model".into(), "Translated by Claude Haiku 5.5".into()),
                    ("Usage".into(), "Your own API key".into()),
                ]
            );
        });
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
