//! 用户自带的模型（设计稿 11「设置 · 模型」）：Anthropic、OpenAI 兼容接口、Ollama。
//!
//! 对外：
//! - `complete(model, system, user)`：阻塞调用一次模型（30 秒超时），在后台线程用。
//! - `test_connection`：设置页「测试连接」发的极小请求。
//! - `api_key` / `set_api_key` / `delete_api_key`：钥匙串里的 Key。
//! - `selected_text()`：前台应用选中的文字（AX；无权限返回 None）。
//! - `settings_pane`：设置窗口「模型」页。

pub(crate) mod client;
mod keychain;
mod pane;
mod selection;

use anyhow::Result;
use gpui_kit::*;

use crate::config::{Config, ModelConfig, ModelKind};

pub use client::{default_base_url, needs_key, ollama_online};
pub use keychain::{api_key, delete_api_key, set_api_key};
pub use selection::selected_text;

/// 一次普通调用最多生成的 token 数（翻译一段文字绰绰有余）。
const MAX_TOKENS: u32 = 4096;

/// 用模型配置里的 Key（钥匙串）调用一次。阻塞，别在界面线程调。
pub fn complete(model: &ModelConfig, system: &str, user: &str) -> Result<String> {
    let key = if needs_key(model.kind) {
        api_key(model)?
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
        ModelKind::OpenAiCompatible => "OpenAI 兼容接口",
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

/// 可选的目标语言：(代码, 中文名, 给模型看的英文名)。`auto` 不在其中。
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

/// 语言代码对应的中文名。
pub fn language_label(code: &str) -> Option<&'static str> {
    LANGUAGES
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, label, _)| *label)
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
