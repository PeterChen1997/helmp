//! 翻译（设计稿 Translate，体验对标 HapiGo 速译）：用用户自己接入的模型翻译输入或选中的文字。
//!
//! - 范围 `translate`：前缀 `fy ` / `tr ` / `翻译 `。范围内只有一项，带表单：
//!   原文（初值 = 输入；为空时取进入范围前取到的选中文字）、源语言、目标语言（只对这一次生效）、模型。
//! - 输入即译：表单项标了 `auto_run`，输入停顿约 400 ms 后自动翻译，结果原地显示在右侧。
//!   自动识别源语言；目标语言「自动」= 中文译成英文，其他译成中文。
//! - 结果：大字号、可选中的译文，下面一行小字「模型 · 源语言 → 目标语言 · 耗时」。
//!   回车复制译文，⌘↵ 替换选中文字，⌘⇧C 复制原文，⌘R 朗读译文。
//! - 出错（没配模型、Key 无效、超时、限流……）用人话说明，并给「打开设置」（可重试的错误还有「重试」）。
//! - 历史：最近的翻译留在内存里；范围内按 ↑ 调出原文，命中历史时直接显示、不再调用模型。
//! - 取词：全局快捷键唤起时，面板显示之前先读前台 App 的选中文字（见 `models::selection`：
//!   AX 优先，读不到再模拟 ⌘C 并还原剪贴板）。
//! - 首页「快捷命令」里有「翻译」入口；全局搜索 `翻译` / `fy` / `translate` 也能找到它。

use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::config::{self, Config, ModelConfig};
use crate::engine::commands_section;
use crate::i18n::{tr, trf};
use crate::matcher;
use crate::models::{self, FailureKind};
use crate::provider::{
    Action, ActionInput, Block, Detail, Field, FieldKind, Form, Icon, Item, NamedAction, Outcome,
    Provider, Scope, Tint,
};
use crate::settings::Page;

const SCOPE: &str = "translate";
const ICON: Icon = Icon::Symbol {
    name: "languages",
    tint: Tint::Teal,
};
const ERROR_ICON: Icon = Icon::Symbol {
    name: "triangle-alert",
    tint: Tint::Red,
};
/// 内存里保留的翻译记录条数。
const MAX_HISTORY: usize = 20;

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

/// 一次翻译的记录：既是 ↑ 调出的历史，也是缓存（同样的原文、语言、模型不再花钱调一次）。
#[derive(Clone, Debug)]
struct Record {
    key: String,
    source_text: String,
    translation: String,
    model_name: String,
    model_local: &'static str,
    from: String,
    to: String,
}

type History = Arc<Mutex<VecDeque<Record>>>;

#[derive(Default)]
pub struct Translate {
    /// 进入范围前取到的选中文字（`will_enter`）。
    captured: Mutex<Option<String>>,
    history: History,
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
            placeholder: tr(
                "输入要翻译的文字 · ↑ 最近的翻译",
                "Type text to translate · ↑ for recent translations",
            )
            .into(),
            filters: Vec::new(),
        })
    }

    fn will_enter(&self, app_in_front: bool) {
        // 每次进入都重新取：上一次的选中文字不能带到这一次。
        let text = models::capture_selection(app_in_front);
        *self.captured.lock().expect("translate selection poisoned") = text;
    }

    fn recall(&self) -> Vec<String> {
        let history = self.history.lock().expect("translate history poisoned");
        let mut seen = Vec::new();
        for record in history.iter() {
            let text = one_line(&record.source_text, usize::MAX);
            if !seen.contains(&text) {
                seen.push(text);
            }
        }
        seen
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
            match self
                .captured
                .lock()
                .expect("translate selection poisoned")
                .clone()
            {
                Some(text) => (text, true),
                None => (String::new(), false),
            }
        } else {
            (typed.to_string(), false)
        };
        vec![form_item(
            &config,
            typed,
            &text,
            from_selection,
            &self.history,
        )]
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
        action: Action::new(|| Ok(Outcome::OpenSettingsPage(Page::Models))),
        action_title: tr("打开设置", "Open Settings").into(),
        detail: Some(Detail {
            icon: ICON,
            title: tr("还没有可用的翻译模型", "No translation model yet").into(),
            blocks: vec![Block::Text(
                tr(
                    "Helmp 用你自己接入的模型翻译：Anthropic、OpenAI 兼容接口（DeepSeek 等），\
                     或者本机的 Ollama。按回车打开设置 › 模型，接入一个就能用。",
                    "Helmp translates with a model you connect yourself: Anthropic, any \
                     OpenAI-compatible API (DeepSeek and others), or a local Ollama. \
                     Press Return to open Settings › Models and add one.",
                )
                .into(),
            )],
            ..Detail::default()
        }),
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

fn form_item(
    config: &Config,
    typed: &str,
    text: &str,
    from_selection: bool,
    history: &History,
) -> Item {
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
            // 这里改的目标语言只对这一次生效，不改设置里的默认值。
            select(
                "target",
                tr("目标语言 · 仅本次", "Target Language · This Time"),
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
            "输入文字自动翻译 · {model_name}",
            "Type to translate · {model_name}"
        )
    } else {
        format!("{} · {model_name}", one_line(text, 60))
    };
    let run_defaults = defaults.clone();
    let history = Arc::clone(history);
    Item {
        // 表单状态按 id 缓存：输入变了 id 才变，后台每秒刷新不会冲掉用户在表单里的修改。
        id: format!("translate:{}", digest(typed)),
        title: tr("翻译", "Translate").into(),
        subtitle,
        badge: badge().into(),
        score: 1000,
        icon: ICON,
        action: Action::with_input(move |input| Ok(run(&run_defaults, input, &history))),
        action_title: tr("翻译", "Translate").into(),
        actions: vec![NamedAction::new(
            tr("打开模型设置", "Open Model Settings"),
            None,
            Action::new(|| Ok(Outcome::OpenSettingsPage(Page::Models))),
        )],
        // 还没有原文时，右侧说明怎么用（有原文时框架显示加载占位，随后是译文）。
        detail: text.is_empty().then(hint_detail),
        form: Some(form),
        auto_run: true,
        ..Item::blank()
    }
}

fn hint_detail() -> Detail {
    Detail {
        icon: ICON,
        title: tr("输入即译", "Translate as You Type").into(),
        blocks: vec![Block::Text(
            tr(
                "在搜索框里输入或粘贴文字，停顿一下就会自动翻译：中文译成英文，其他语言译成中文。\n\
                 在其他 App 里选中文字后按 ⌥⇧T，会直接翻译选中的内容。\n\
                 按 ↑ 调出最近的翻译。",
                "Type or paste text in the search field and it’s translated when you pause: \
                 Chinese into English, everything else into Chinese.\n\
                 Select text in another app and press ⌥⇧T to translate it right away.\n\
                 Press ↑ for recent translations.",
            )
            .into(),
        )],
        ..Detail::default()
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

/// 调一次模型：(模型, 系统提示, 原文) → 回复。测试里换成假的，不发网络请求、不读钥匙串。
type Complete<'a> = &'a dyn Fn(&ModelConfig, &str, &str) -> Result<String>;

fn run(defaults: &Defaults, input: &ActionInput, history: &History) -> Outcome {
    let config = config::get();
    if config.models.is_empty() {
        return Outcome::OpenSettingsPage(Page::Models);
    }
    let text = value(input, "text", &defaults.text).trim();
    if text.is_empty() {
        return Outcome::Show(hint_detail());
    }
    let model_id = value(input, "model", &defaults.model);
    let Some(model) = config
        .models
        .iter()
        .find(|model| model.id == model_id)
        .or_else(|| models::translate_model(&config))
    else {
        return Outcome::OpenSettingsPage(Page::Models);
    };
    let source = value(input, "source", &defaults.source);
    let target = value(input, "target", &defaults.target);
    Outcome::Show(translate(
        model,
        source,
        target,
        text,
        history,
        &models::complete,
    ))
}

/// 翻译一段文字：先查历史，没有再调模型。出错时返回说明 + 出路的详情，而不是一串报错。
fn translate(
    model: &ModelConfig,
    source: &str,
    target: &str,
    text: &str,
    history: &History,
    complete: Complete,
) -> Detail {
    let to = resolve_target(target, text).to_string();
    let from = match source {
        "auto" => detect_language(text).unwrap_or("auto").to_string(),
        code => code.to_string(),
    };
    let key = format!(
        "{}\u{1}{source}\u{1}{to}\u{1}{}",
        one_line(text, usize::MAX),
        model.id
    );
    let cached = history
        .lock()
        .expect("translate history poisoned")
        .iter()
        .find(|record| record.key == key)
        .cloned();
    if let Some(record) = cached {
        remember(history, record.clone());
        return result_detail(&record, None);
    }
    let started = Instant::now();
    match complete(model, &system_prompt(source, &to), text) {
        Ok(translation) => {
            let record = Record {
                key,
                source_text: text.to_string(),
                translation: translation.trim().to_string(),
                model_name: model.name.clone(),
                model_local: billing_note(model),
                from,
                to,
            };
            remember(history, record.clone());
            result_detail(&record, Some(started.elapsed()))
        }
        Err(err) => failure_detail(model, &err),
    }
}

/// 记一条（同一条移到最前），超出上限丢最旧的。
fn remember(history: &History, record: Record) {
    let mut history = history.lock().expect("translate history poisoned");
    history.retain(|old| old.key != record.key);
    history.push_front(record);
    history.truncate(MAX_HISTORY);
}

fn language_name(code: &str) -> &str {
    match code {
        "auto" => tr("自动检测", "Auto-detected"),
        code => models::language_label(code).unwrap_or(code),
    }
}

/// `elapsed` 为 None 表示来自历史记录（没有调用模型）。
fn result_detail(record: &Record, elapsed: Option<Duration>) -> Detail {
    let translation = record.translation.clone();
    let took = match elapsed {
        Some(elapsed) => trf!(
            "{:.1} 秒",
            "{:.1}s",
            elapsed.as_secs_f64()
        ),
        None => tr("来自历史记录", "From history").to_string(),
    };
    let caption = format!(
        "{} · {} → {} · {took}",
        record.model_name,
        language_name(&record.from),
        language_name(&record.to)
    );
    Detail {
        // 回车复制译文；⌘↵ 复制并替换前台 App 里的选中文字。
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
            NamedAction::new(
                tr("复制原文", "Copy Source Text"),
                Some("⌘⇧C"),
                Action::copy(record.source_text.clone()),
            ),
            NamedAction::new(
                tr("朗读译文", "Read Translation Aloud"),
                Some("⌘R"),
                speak(translation.clone(), record.to.clone()),
            ),
        ],
        icon: ICON,
        title: trf!(
            "译文 · {}",
            "Translation · {}",
            language_name(&record.to)
        ),
        tag: None,
        blocks: vec![
            Block::Result {
                text: translation,
                large: true,
            },
            Block::Caption(caption),
        ],
        metadata: vec![(tr("调用", "Usage").into(), record.model_local.into())],
    }
}

/// 出错时的详情：标题说清是什么问题，正文说下一步怎么办，附上原始错误便于排查。
/// 一定带「打开设置」；网络类、限流类的错误把「重试」放在回车上。
fn failure_detail(model: &ModelConfig, err: &anyhow::Error) -> Detail {
    let name = &model.name;
    let kind = models::failure_kind(err);
    let (title, advice) = match kind {
        Some(FailureKind::Setup) => (
            tr("模型还没配置好", "The model isn’t set up yet"),
            trf!(
                "到设置 › 模型里给「{name}」补上 API Key 或模型名。",
                "Open Settings › Models and fill in the API key or model name for “{name}”."
            ),
        ),
        Some(FailureKind::Auth) => (
            tr("API Key 无效", "Invalid API key"),
            trf!(
                "「{name}」的 API Key 不对，或者没有这个模型的权限。到设置 › 模型里检查一下。",
                "The API key for “{name}” is wrong or can’t use this model. Check it in Settings › Models."
            ),
        ),
        Some(FailureKind::NotFound) => (
            tr("找不到这个模型", "Model not found"),
            trf!(
                "检查「{name}」的模型名和 Base URL；用 Ollama 的话先 ollama pull 这个模型。",
                "Check the model name and base URL for “{name}”. With Ollama, run ollama pull first."
            ),
        ),
        Some(FailureKind::RateLimited) => (
            tr("请求太频繁", "Too many requests"),
            tr(
                "稍等片刻再试。如果一直这样，可能是账户额度用完了。",
                "Wait a moment and try again. If it keeps happening, your account may be out of credit.",
            )
            .to_string(),
        ),
        Some(FailureKind::Timeout) => (
            tr("网络超时", "The request timed out"),
            tr(
                "模型很久没有回应。检查网络后重试，或者在设置里换一个更快的模型。",
                "The model didn’t answer in time. Check your network and try again, or pick a faster model in Settings.",
            )
            .to_string(),
        ),
        Some(FailureKind::Unreachable) => (
            tr("连不上模型服务", "Can’t reach the model"),
            tr(
                "检查网络和 Base URL；用本机 Ollama 的话确认它已经启动。",
                "Check your network and the base URL. With a local Ollama, make sure it’s running.",
            )
            .to_string(),
        ),
        Some(FailureKind::Server) => (
            tr("模型服务出错", "The model service had an error"),
            tr(
                "服务端暂时出了问题，稍后重试。",
                "The service is having trouble. Try again in a moment.",
            )
            .to_string(),
        ),
        Some(FailureKind::BadResponse) => (
            tr("返回内容看不懂", "Unexpected response"),
            trf!(
                "多半是「{name}」的 Base URL 填错了，到设置 › 模型里检查。",
                "The base URL for “{name}” is probably wrong. Check it in Settings › Models."
            ),
        ),
        Some(FailureKind::Other) | None => (
            tr("翻译失败", "Translation failed"),
            tr(
                "可以重试，或者到设置 › 模型里检查配置。",
                "Try again, or check the model in Settings › Models.",
            )
            .to_string(),
        ),
    };
    let settings = NamedAction::new(
        tr("打开设置", "Open Settings"),
        Some("⌘,"),
        Action::new(|| Ok(Outcome::OpenSettingsPage(Page::Models))),
    );
    let retry = NamedAction::new(tr("重试", "Try Again"), Some("⌘R"), Action::rerun());
    let fix_in_settings = matches!(
        kind,
        Some(
            FailureKind::Setup
                | FailureKind::Auth
                | FailureKind::NotFound
                | FailureKind::BadResponse
        )
    );
    let actions = if fix_in_settings {
        vec![settings, retry]
    } else {
        vec![retry, settings]
    };
    Detail {
        actions,
        icon: ERROR_ICON,
        title: title.into(),
        tag: None,
        blocks: vec![Block::Text(advice), Block::Code(format!("{err:#}"))],
        metadata: Vec::new(),
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

/// 按文字里的字符判断语言（只认得出把握大的几种）：假名 → 日文，谚文 → 韩文，西里尔字母 → 俄文，
/// 汉字为主 → 中文，纯 ASCII 字母 → 英文；其他拉丁语言分不清，返回 None（显示「自动检测」）。
fn detect_language(text: &str) -> Option<&'static str> {
    let count = |f: fn(u32) -> bool| text.chars().filter(|c| f(*c as u32)).count();
    let han = count(|c| matches!(c, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF));
    let kana = count(|c| matches!(c, 0x3040..=0x30FF));
    let hangul = count(|c| matches!(c, 0xAC00..=0xD7AF | 0x1100..=0x11FF));
    let cyrillic = count(|c| matches!(c, 0x0400..=0x04FF));
    let ascii = text.chars().filter(char::is_ascii_alphabetic).count();
    let other_latin = text
        .chars()
        .filter(|c| c.is_alphabetic() && !c.is_ascii() && (*c as u32) < 0x0250)
        .count();
    if kana > 0 {
        Some("ja")
    } else if hangul > 0 && hangul >= han {
        Some("ko")
    } else if han > 0 && han * 2 >= ascii {
        Some("zh-Hans")
    } else if cyrillic > ascii {
        Some("ru")
    } else if ascii > 0 && other_latin == 0 {
        Some("en")
    } else {
        None
    }
}

/// `auto` = 中英互译：中文译成英文，其他译成简体中文。在本地决定，标题才能写准「译文 · 英文」。
fn resolve_target<'a>(target: &'a str, text: &str) -> &'a str {
    match target {
        "auto" if detect_language(text) == Some("zh-Hans") => "en",
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

// ---------------------------------------------------------------- 朗读

/// 正在朗读的 `say` 进程：再按一次朗读先停掉上一段。
static SPEAKING: Mutex<Option<Child>> = Mutex::new(None);

/// 用系统 `say` 朗读，按语言挑声音。不等读完就返回，面板保持打开。
fn speak(text: String, language: String) -> Action {
    Action::new(move || {
        let mut speaking = SPEAKING.lock().expect("speaking poisoned");
        if let Some(mut previous) = speaking.take() {
            let _ = previous.kill();
            let _ = previous.wait();
        }
        let mut command = Command::new("say");
        if let Some(voice) = pick_voice(voices(), &language) {
            command.arg("-v").arg(voice);
        }
        // 文字走 stdin：以 `-` 开头的原文不会被当成参数；强制 UTF-8，免得中文变问号。
        let mut child = command
            .env("LANG", "en_US.UTF-8")
            .env("LC_ALL", "en_US.UTF-8")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .map_err(|err| {
                anyhow::anyhow!(trf!(
                    "启动 say 失败：{err}",
                    "Couldn’t start say: {err}"
                ))
            })?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write as _;
            stdin.write_all(text.as_bytes())?;
        }
        *speaking = Some(child);
        Ok(Outcome::Done)
    })
    .keep_open()
}

/// `say -v ?` 列出的声音：(名字, 区域如 zh_CN)。只读一次。
fn voices() -> &'static [(String, String)] {
    static VOICES: OnceLock<Vec<(String, String)>> = OnceLock::new();
    VOICES.get_or_init(|| {
        Command::new("say")
            .args(["-v", "?"])
            .env("LANG", "en_US.UTF-8")
            .env("LC_ALL", "en_US.UTF-8")
            .output()
            .map(|output| parse_voices(&String::from_utf8_lossy(&output.stdout)))
            .unwrap_or_default()
    })
}

/// `Tingting (Chinese (China mainland)) zh_CN    # 你好…` → (`Tingting (Chinese (China mainland))`, `zh_CN`)。
fn parse_voices(listing: &str) -> Vec<(String, String)> {
    listing
        .lines()
        .filter_map(|line| {
            let left = line.split('#').next()?.trim_end();
            let (name, locale) = left.rsplit_once(char::is_whitespace)?;
            let name = name.trim();
            (!name.is_empty() && locale.contains('_')).then(|| (name.to_string(), locale.to_string()))
        })
        .collect()
}

/// 按语言挑声音：先挑系统里音质好的常用声音，再退到同语言的第一个非搞怪声音；都没有返回 None（用系统默认）。
fn pick_voice<'a>(voices: &'a [(String, String)], language: &str) -> Option<&'a str> {
    let (locales, preferred): (&[&str], &[&str]) = match language {
        "zh-Hans" => (&["zh_CN"], &["Tingting", "Lili", "Yu-shu"]),
        "zh-Hant" => (&["zh_TW", "zh_HK"], &["Meijia", "Sinji"]),
        "en" => (&["en_US", "en_GB", "en_"], &["Samantha", "Alex", "Daniel"]),
        "ja" => (&["ja_JP"], &["Kyoko", "Otoya"]),
        "ko" => (&["ko_KR"], &["Yuna"]),
        "fr" => (&["fr_FR", "fr_"], &["Thomas", "Amélie"]),
        "de" => (&["de_DE", "de_"], &["Anna"]),
        "es" => (&["es_ES", "es_"], &["Mónica", "Monica", "Paulina"]),
        "ru" => (&["ru_RU"], &["Milena"]),
        _ => return None,
    };
    const NOVELTY: &[&str] = &[
        "Albert", "Bad News", "Bahh", "Bells", "Boing", "Bubbles", "Cellos", "Good News",
        "Jester", "Organ", "Superstar", "Trinoids", "Whisper", "Wobble", "Zarvox",
    ];
    let base = |name: &str| name.split(" (").next().unwrap_or(name).to_string();
    let matches = |locale: &str| locales.iter().any(|want| locale.starts_with(want));
    preferred
        .iter()
        .find_map(|want| {
            voices
                .iter()
                .find(|(name, locale)| matches(locale) && base(name) == *want)
        })
        .or_else(|| {
            locales.iter().find_map(|want| {
                voices.iter().find(|(name, locale)| {
                    locale.starts_with(want) && !NOVELTY.contains(&base(name).as_str())
                })
            })
        })
        .map(|(name, _)| name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ModelKind;
    use std::cell::Cell;

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

    fn history() -> History {
        History::default()
    }

    fn titles(detail: &Detail) -> Vec<&str> {
        detail.actions.iter().map(|a| a.title.as_str()).collect()
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
    fn detects_source_language_for_the_caption() {
        // 结果下面写「中文 → 英文」：认错了会让用户以为模型理解错了原文。
        assert_eq!(detect_language("你好，世界"), Some("zh-Hans"));
        assert_eq!(detect_language("hello world"), Some("en"));
        assert_eq!(detect_language("こんにちは世界"), Some("ja"));
        assert_eq!(detect_language("안녕하세요"), Some("ko"));
        assert_eq!(detect_language("Привет, мир"), Some("ru"));
        // 分不清的拉丁语言不硬猜。
        assert_eq!(detect_language("où est la gare"), None);
        assert_eq!(detect_language("12345"), None);
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
        let item = form_item(&config, "hello", "hello", false, &history());
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
        assert_eq!(fields[2].label, "目标语言 · 仅本次");
    }

    #[test]
    fn translates_as_you_type() {
        // 「输入即译」靠 auto_run：丢了它，用户输入后还得再按一次回车。
        let config = config_with(vec![model("a", "Claude", ModelKind::Anthropic)]);
        let item = form_item(&config, "hello", "hello", false, &history());
        assert!(item.auto_run);
        assert!(item.detail.is_none(), "有原文时右侧留给加载占位和译文");
        let empty = form_item(&config, "", "", false, &history());
        assert_eq!(
            empty.detail.expect("没原文时要说明怎么用").title,
            "输入即译"
        );
    }

    #[test]
    fn captured_selection_fills_the_form() {
        // 从别的 App 选中文字后按快捷键：进入时取到的选中文字就是原文，并标明来源。
        let config = config_with(vec![model("a", "Claude", ModelKind::Anthropic)]);
        let item = form_item(&config, "", "选中的文字", true, &history());
        let form = item.form.unwrap();
        assert_eq!(form.fields[0].label, "原文 · 来自选中文字");
        assert_eq!(form.fields[0].value, "选中的文字");
    }

    #[test]
    fn form_id_is_stable_for_same_input() {
        // 面板每秒刷新一次结果：id 变了表单就会重建，用户改到一半的原文会被冲掉。
        let config = config_with(vec![model("a", "Claude", ModelKind::Anthropic)]);
        let a = form_item(&config, "", "选中 A", true, &history());
        let b = form_item(&config, "", "选中 B", true, &history());
        assert_eq!(a.id, b.id);
        assert_ne!(
            a.id,
            form_item(&config, "hello", "hello", false, &history()).id
        );
    }

    #[test]
    fn without_models_offers_open_settings() {
        // 没接模型时翻译做不了任何事：必须说明原因，并把回车给「打开设置」，而不是一个打不开的表单。
        let item = no_model_item();
        assert_eq!(item.subtitle, no_model());
        assert!(item.form.is_none());
        assert_eq!(item.action_title, "打开设置");
        // 说明里写的是「打开设置 › 模型」：回车要直接落在模型页，而不是设置首页让用户自己找。
        assert_eq!(
            item.action.run().unwrap(),
            Outcome::OpenSettingsPage(Page::Models)
        );
        assert!(item.detail.is_some(), "右侧要用人话说明");
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
    fn result_shows_large_text_with_model_languages_and_time() {
        // 用户要一眼看到译文，并知道是谁翻的、从什么译成什么、花了多久、花的是不是自己的钱。
        let fake = |_: &ModelConfig, _: &str, _: &str| Ok("  Hello, world \n".to_string());
        let d = translate(
            &model("a", "Claude Haiku 5.5", ModelKind::Anthropic),
            "auto",
            "auto",
            "你好，世界",
            &history(),
            &fake,
        );
        assert_eq!(d.title, "译文 · 英文");
        let [Block::Result { text, large: true }, Block::Caption(caption)] = d.blocks.as_slice()
        else {
            panic!("{:?}", d.blocks);
        };
        assert_eq!(text, "Hello, world");
        assert!(
            caption.starts_with("Claude Haiku 5.5 · 简体中文 → 英文 · "),
            "{caption}"
        );
        assert!(caption.ends_with(" 秒"), "{caption}");
        assert_eq!(d.metadata, vec![("调用".into(), "使用你自己的 API Key".into())]);
    }

    #[test]
    fn result_actions_follow_the_shortcut_contract() {
        // 回车复制译文、⌘↵ 替换选中、⌘⇧C 复制原文、⌘R 朗读：顺序和快捷键是对用户的承诺。
        let fake = |_: &ModelConfig, _: &str, _: &str| Ok("Hi".to_string());
        let d = translate(
            &model("o", "Qwen", ModelKind::Ollama),
            "auto",
            "auto",
            "你好",
            &history(),
            &fake,
        );
        assert_eq!(
            titles(&d),
            ["复制译文", "替换选中文字", "复制原文", "朗读译文"]
        );
        let shortcuts: Vec<_> = d.actions.iter().map(|a| a.shortcut.as_deref()).collect();
        assert_eq!(shortcuts, [None, Some("⌘↵"), Some("⌘⇧C"), Some("⌘R")]);
        // 朗读不能把面板关掉，否则用户看不到正在读的译文。
        assert!(d.actions[3].action.keeps_open());
        assert_eq!(d.metadata[0].1, "本机 Ollama");
    }

    #[test]
    fn failures_explain_and_always_offer_settings() {
        // 出错时只丢一串报错，用户不知道下一步：每种错误都要有人话标题、建议，以及「打开设置」。
        let m = model("a", "Claude", ModelKind::Anthropic);
        let fail = |kind| {
            move |_: &ModelConfig, _: &str, _: &str| -> Result<String> {
                Err(models::ModelError::new(kind, "服务端原话").into())
            }
        };
        let auth = translate(&m, "auto", "auto", "hi", &history(), &fail(FailureKind::Auth));
        assert_eq!(auth.title, "API Key 无效");
        // 配置类错误：回车直接去设置。
        assert_eq!(titles(&auth), ["打开设置", "重试"]);
        assert!(matches!(&auth.blocks[1], Block::Code(raw) if raw.contains("服务端原话")));
        let setup = translate(&m, "auto", "auto", "hi", &history(), &fail(FailureKind::Setup));
        assert_eq!(titles(&setup)[0], "打开设置");
        for kind in [
            FailureKind::Timeout,
            FailureKind::RateLimited,
            FailureKind::Unreachable,
            FailureKind::Server,
        ] {
            // 网络、限流类错误：回车重试，设置仍然可达。
            let d = translate(&m, "auto", "auto", "hi", &history(), &fail(kind));
            assert_eq!(titles(&d), ["重试", "打开设置"], "{kind:?}");
            assert!(d.actions[0].action.is_rerun());
            assert_eq!(
                d.actions[1].action.run().unwrap(),
                Outcome::OpenSettingsPage(Page::Models)
            );
        }
        let timeout = translate(&m, "auto", "auto", "hi", &history(), &fail(FailureKind::Timeout));
        assert_eq!(timeout.title, "网络超时");
        let limited = translate(
            &m,
            "auto",
            "auto",
            "hi",
            &history(),
            &fail(FailureKind::RateLimited),
        );
        assert_eq!(limited.title, "请求太频繁");
        // 不是模型报的错（如钥匙串以外的意外）也要有出路。
        let other = |_: &ModelConfig, _: &str, _: &str| -> Result<String> {
            Err(anyhow::anyhow!("boom"))
        };
        let d = translate(&m, "auto", "auto", "hi", &history(), &other);
        assert!(titles(&d).contains(&"打开设置"));
    }

    #[test]
    fn repeated_text_comes_from_history_without_calling_the_model() {
        // 输入即译会反复触发同一句（↑ 调历史、删一个字再打回来）：每次都调模型既慢又花用户的钱。
        let calls = Cell::new(0);
        let fake = |_: &ModelConfig, _: &str, user: &str| {
            calls.set(calls.get() + 1);
            Ok(format!("EN({user})"))
        };
        let h = history();
        let m = model("a", "Claude", ModelKind::Anthropic);
        translate(&m, "auto", "auto", "第一句", &h, &fake);
        translate(&m, "auto", "auto", "第二句", &h, &fake);
        let again = translate(&m, "auto", "auto", "第一句", &h, &fake);
        assert_eq!(calls.get(), 2, "历史里有的不应再调模型");
        assert!(matches!(&again.blocks[1], Block::Caption(c) if c.ends_with("来自历史记录")));
        // 换了目标语言是另一次翻译。
        translate(&m, "auto", "ja", "第一句", &h, &fake);
        assert_eq!(calls.get(), 3);
        // ↑ 调出的历史：最近的在前，同一原文只出现一次。
        let provider = Translate {
            history: h,
            ..Translate::default()
        };
        assert_eq!(provider.recall(), ["第一句", "第二句"]);
    }

    #[test]
    fn failures_are_not_cached() {
        // 失败不能进历史：否则网络恢复后再输入同一句，看到的还是错误。
        let h = history();
        let m = model("a", "Claude", ModelKind::Anthropic);
        let fail = |_: &ModelConfig, _: &str, _: &str| -> Result<String> {
            Err(models::ModelError::new(FailureKind::Timeout, "slow").into())
        };
        translate(&m, "auto", "auto", "你好", &h, &fail);
        assert!(h.lock().unwrap().is_empty());
    }

    #[test]
    fn translates_through_model_with_mock_server() {
        // 端到端（mock Ollama，不出网、不读钥匙串）：原文原样作为 user 消息发出、目标语言写进提示。
        let (url, rx) = crate::models::client::tests::mock(
            200,
            r#"{"message":{"role":"assistant","content":"  Hello, world \n"},"done":true}"#,
        );
        let mut m = model("o", "Qwen3 8B", ModelKind::Ollama);
        m.base_url = url;
        let d = translate(&m, "auto", "auto", "你好，世界", &history(), &models::complete);
        assert_eq!(d.title, "译文 · 英文");
        assert!(matches!(&d.blocks[0], Block::Result { text, .. } if text == "Hello, world"));
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
    fn picks_a_natural_voice_for_the_language() {
        // 用英文声音读中文会读成乱码；系统里排在前面的是 Albert、Bad News 这类搞怪声音，不能挑它们。
        let listing = "\
Albert              en_US    # Hello! My name is Albert.
Bad News            en_US    # Hello! My name is Bad News.
Eddy (Chinese (China mainland)) zh_CN    # 你好！我叫Eddy。
Samantha            en_US    # Hello! My name is Samantha.
Tingting (Chinese (China mainland)) zh_CN    # 你好，我叫婷婷。
Kyoko               ja_JP    # こんにちは! 私の名前はKyokoです。
Meijia (Chinese (Taiwan)) zh_TW    # 你好，我叫美佳。
";
        let voices = parse_voices(listing);
        assert_eq!(voices[0], ("Albert".into(), "en_US".into()));
        assert_eq!(
            voices[2],
            ("Eddy (Chinese (China mainland))".into(), "zh_CN".into())
        );
        assert_eq!(
            pick_voice(&voices, "zh-Hans"),
            Some("Tingting (Chinese (China mainland))")
        );
        assert_eq!(pick_voice(&voices, "en"), Some("Samantha"));
        assert_eq!(pick_voice(&voices, "ja"), Some("Kyoko"));
        assert_eq!(pick_voice(&voices, "zh-Hant"), Some("Meijia (Chinese (Taiwan))"));
        // 没有首选声音时退到同语言的普通声音，不挑搞怪声音。
        let fallback = parse_voices("Albert  en_US  # x\nFred  en_US  # x\n");
        assert_eq!(pick_voice(&fallback, "en"), Some("Fred"));
        assert_eq!(pick_voice(&voices, "ko"), None);
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
            let fields = form_item(&config, "", "selected", true, &history())
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

            let fake = |_: &ModelConfig, _: &str, _: &str| Ok("Hi".to_string());
            let d = translate(
                &model("a", "Claude Haiku 5.5", ModelKind::Anthropic),
                "zh-Hans",
                "en",
                "你好",
                &history(),
                &fake,
            );
            assert_eq!(d.title, "Translation · English");
            assert!(matches!(
                &d.blocks[1],
                Block::Caption(c) if c.starts_with("Claude Haiku 5.5 · Simplified Chinese → English · ")
            ));
            assert_eq!(d.metadata, vec![("Usage".into(), "Your own API key".into())]);
            let fail = |_: &ModelConfig, _: &str, _: &str| -> Result<String> {
                Err(models::ModelError::new(FailureKind::Auth, "x").into())
            };
            let e = translate(
                &model("a", "Claude", ModelKind::Anthropic),
                "auto",
                "auto",
                "hi",
                &history(),
                &fail,
            );
            assert_eq!(e.title, "Invalid API key");
            assert_eq!(titles(&e), ["Open Settings", "Try Again"]);
        });
    }

    #[test]
    fn icons_exist_in_bundled_assets() {
        // 图标名拼错时界面只画一块空白，不会报错。
        use gpui_kit::AssetSource as _;
        for icon in [ICON, ERROR_ICON] {
            let Icon::Symbol { name, .. } = icon else {
                unreachable!()
            };
            let path = format!("icons/{name}.svg");
            assert!(
                gpui_kit::assets::AllAssets.load(&path).unwrap().is_some(),
                "缺少图标 {path}"
            );
        }
    }
}
