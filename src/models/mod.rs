//! 用户自带的模型（设计稿 11「设置 · 模型」）：Anthropic、OpenAI 兼容接口、Ollama。
//!
//! 对外：
//! - `complete(model, system, user)`：阻塞调用一次模型（30 秒超时），在后台线程用。
//! - `test_connection`：设置页「测试连接」发的极小请求。
//! - `api_key` / `set_api_key` / `delete_api_key`：钥匙串里的 Key。
//! - `capture_selection()`：取词，前台应用选中的文字（AX，必要时模拟 ⌘C 并还原剪贴板）。
//! - `settings_pane`：设置窗口「模型」页。

pub(crate) mod client;
mod keychain;
mod pane;
mod selection;

use anyhow::Result;
use gpui_kit::*;

use crate::config::{Config, ModelConfig, ModelKind};
use crate::i18n::tr;

pub use client::{
    FailureKind, ModelError, default_base_url, failure_kind, needs_key, ollama_online,
};
pub use keychain::{api_key, delete_api_key, set_api_key};
pub use selection::capture as capture_selection;

/// 一次普通调用最多生成的 token 数（翻译一段文字绰绰有余）。
const MAX_TOKENS: u32 = 4096;

/// 用模型配置里的 Key（钥匙串）调用一次。阻塞，别在界面线程调。
pub fn complete(model: &ModelConfig, system: &str, user: &str) -> Result<String> {
    let key = if needs_key(model.kind) {
        // 钥匙串读不出来也是「配置问题」：界面要引导去设置里重新填 Key。
        api_key(model).map_err(|err| ModelError::new(FailureKind::Setup, format!("{err:#}")))?
    } else {
        None
    };
    client::send(
        model,
        key.as_deref(),
        &client::Request {
            system,
            user,
            max_tokens: MAX_TOKENS,
        },
    )
}

/// 「测试连接」：只生成 1 个 token（OpenAI 兼容接口不限，但提示要求只回一个词）。
/// `key` 为 None 时读钥匙串。
pub fn test_connection(model: &ModelConfig, key: Option<&str>) -> Result<()> {
    let stored = match key {
        Some(_) => None,
        None if needs_key(model.kind) => api_key(model)?,
        None => None,
    };
    client::send(
        model,
        key.or(stored.as_deref()),
        &client::Request {
            system: "Reply with the single word OK.",
            user: "ping",
            max_tokens: 1,
        },
    )
    .map(|_| ())
}

/// 翻译用的模型：`translate.model_id` 指定的，找不到就用第一个。
pub fn translate_model(config: &Config) -> Option<&ModelConfig> {
    config
        .translate
        .model_id
        .as_deref()
        .and_then(|id| config.models.iter().find(|model| model.id == id))
        .or_else(|| config.models.first())
}

pub fn kind_label(kind: ModelKind) -> &'static str {
    match kind {
        ModelKind::Anthropic => "Anthropic",
        ModelKind::OpenAiCompatible => tr("OpenAI 兼容接口", "OpenAI-Compatible API"),
        ModelKind::Ollama => "Ollama",
    }
}

/// 是否是本机模型（本机 Ollama 不花钱、不出网）。
pub fn is_local(model: &ModelConfig) -> bool {
    model.kind == ModelKind::Ollama && {
        let url = model.base_url.trim();
        url.is_empty()
            || ["localhost", "127.0.0.1", "[::1]"]
                .iter()
                .any(|host| url.contains(host))
    }
}

/// 可选的目标语言：(代码, 中文名, 英文名)。英文名既写进提示词，也是英文界面的显示名。`auto` 不在其中。
pub const LANGUAGES: &[(&str, &str, &str)] = &[
    ("en", "英文", "English"),
    ("zh-Hans", "简体中文", "Simplified Chinese"),
    ("zh-Hant", "繁体中文", "Traditional Chinese"),
    ("ja", "日文", "Japanese"),
    ("ko", "韩文", "Korean"),
    ("fr", "法文", "French"),
    ("de", "德文", "German"),
    ("es", "西班牙文", "Spanish"),
    ("ru", "俄文", "Russian"),
];

/// 语言代码对应的显示名（按界面语言）。
pub fn language_label(code: &str) -> Option<&'static str> {
    LANGUAGES
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, zh, en)| tr(zh, en))
}

/// 语言代码对应的英文名（写进提示词）。
pub fn language_english(code: &str) -> Option<&'static str> {
    LANGUAGES
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, _, english)| *english)
}

/// 设置窗口「模型」页的视图。设置窗口创建一次后复用。
// 设置窗口由另一个子任务实现，合并后才有调用方；在此之前别让整页代码报 dead_code。
#[allow(dead_code)]
pub fn settings_pane(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| pane::ModelsPane::new(window, cx)).into()
}

#[cfg(test)]
mod tests {
    // 不用 `super::*`：那会带进 gpui_kit 的 `test` 宏，盖掉标准的 `#[test]`。
    use super::{ModelKind, kind_label, language_english, language_label};
    use crate::i18n::{Lang, with_lang};

    #[test]
    fn language_names_follow_interface_but_prompt_names_do_not() {
        // 目标语言菜单、翻译结果标题给用户看，要随界面语言；
        // 写进提示词的英文名是给模型看的，换界面语言不能改变模型收到的指令。
        assert_eq!(language_label("ja"), Some("日文"));
        with_lang(Lang::En, || {
            assert_eq!(language_label("ja"), Some("Japanese"));
            assert_eq!(language_label("zh-Hans"), Some("Simplified Chinese"));
            assert_eq!(
                kind_label(ModelKind::OpenAiCompatible),
                "OpenAI-Compatible API"
            );
            assert_eq!(language_english("ja"), Some("Japanese"));
        });
        assert_eq!(language_english("ja"), Some("Japanese"));
    }
}
