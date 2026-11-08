//! 用户配置：`<data_dir>/config.json`。设置窗口读写它，各模块按需读取。
//!
//! - `get()` 返回当前配置快照（廉价的 `Arc` 克隆）。
//! - `update(|c| ...)` 修改、落盘，并通知所有订阅者。
//! - 字段都有默认值（`#[serde(default)]`），旧版本的文件缺字段也能读。
//! - API Key 不在这里：存在钥匙串，见 `ModelConfig::keychain_account`。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub general: General,
    /// 按 provider id 的启用、别名与快捷键。缺省视为启用。
    pub extensions: BTreeMap<String, ExtensionConfig>,
    pub models: Vec<ModelConfig>,
    pub translate: TranslateConfig,
    pub clipboard: ClipboardConfig,
    pub screenshot: ScreenshotConfig,
    pub appearance: Appearance,
    /// 额外的脚本目录（默认目录 `<data_dir>/scripts` 总是包含）。
    pub script_dirs: Vec<String>,
    /// 首次启动的权限引导是否已完成或跳过。
    pub onboarding_done: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: General::default(),
            extensions: BTreeMap::new(),
            models: Vec::new(),
            translate: TranslateConfig::default(),
            clipboard: ClipboardConfig::default(),
            screenshot: ScreenshotConfig::default(),
            appearance: Appearance::System,
            script_dirs: Vec::new(),
            onboarding_done: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct General {
    /// 唤起快捷键，格式同 GPUI keystroke：`alt-space`、`cmd-shift-space`。
    pub hotkey: String,
    pub launch_at_login: bool,
    pub show_menubar_icon: bool,
    /// 隐藏超过这么多秒再唤起时清空输入；0 = 每次清空。
    pub reset_after_secs: u64,
    pub switch_input_source: bool,
    pub panel_screen: PanelScreen,
    pub auto_check_updates: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            hotkey: "alt-space".into(),
            launch_at_login: false,
            show_menubar_icon: true,
            reset_after_secs: 90,
            switch_input_source: true,
            panel_screen: PanelScreen::Mouse,
            auto_check_updates: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PanelScreen {
    /// 鼠标所在的屏幕。
    Mouse,
    /// 主屏幕。
    Primary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ExtensionConfig {
    pub enabled: bool,
    pub alias: Option<String>,
    pub hotkey: Option<String>,
}

impl Default for ExtensionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            alias: None,
            hotkey: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    Anthropic,
    /// OpenAI Chat Completions 兼容接口（OpenAI、DeepSeek、各类网关）。
    OpenAiCompatible,
    Ollama,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelConfig {
    /// 稳定 id，也是钥匙串里的账户名后缀。
    pub id: String,
    /// 显示名，如「Claude Haiku 5.5」。
    pub name: String,
    pub kind: ModelKind,
    pub base_url: String,
    /// 接口里的模型名，如 `claude-haiku-5-5`、`deepseek-chat`。
    pub model: String,
}

impl ModelConfig {
    /// 钥匙串条目：服务名固定为 `KEYCHAIN_SERVICE`，账户名为 `model:<id>`。
    pub fn keychain_account(&self) -> String {
        format!("model:{}", self.id)
    }
}

/// 钥匙串服务名，与 bundle id 一致。
pub const KEYCHAIN_SERVICE: &str = "dev.helmp.launcher";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TranslateConfig {
    /// 用哪个模型翻译（`ModelConfig::id`）；None = 第一个模型。
    pub model_id: Option<String>,
    /// `auto`（中英互译）或目标语言代码，如 `en`、`zh-Hans`、`ja`。
    pub target: String,
}

impl Default for TranslateConfig {
    fn default() -> Self {
        Self {
            model_id: None,
            target: "auto".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ClipboardConfig {
    pub enabled: bool,
    /// 保留天数；None = 永久。
    pub retention_days: Option<u32>,
    pub record_images: bool,
    pub skip_concealed: bool,
    /// 不记录这些应用复制的内容（bundle id）。
    pub ignore_apps: Vec<String>,
    pub auto_paste: bool,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: None,
            record_images: true,
            skip_concealed: true,
            ignore_apps: vec!["com.apple.Passwords".into()],
            auto_paste: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ScreenshotConfig {
    /// 「截图并保存」的目录；None = `<data_dir>/screenshots`。
    pub save_dir: Option<String>,
    pub ocr_languages: Vec<String>,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            save_dir: None,
            ocr_languages: vec!["zh-Hans".into(), "en-US".into()],
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    System,
    Light,
    Dark,
}

type Listener = Box<dyn Fn(&Config) + Send + Sync>;

struct Store {
    current: RwLock<Arc<Config>>,
    listeners: Mutex<Vec<Listener>>,
}

fn store() -> &'static Store {
    static STORE: OnceLock<Store> = OnceLock::new();
    STORE.get_or_init(|| {
        let config = match load() {
            Ok(config) => config,
            Err(err) => {
                // 配置坏了不能让整个 App 起不来：用默认值，并大声说明。
                eprintln!("helmp: 读取配置失败，使用默认配置：{err:#}");
                Config::default()
            }
        };
        Store {
            current: RwLock::new(Arc::new(config)),
            listeners: Mutex::new(Vec::new()),
        }
    })
}

fn path() -> Result<PathBuf> {
    Ok(crate::paths::data_dir()?.join("config.json"))
}

fn load() -> Result<Config> {
    let path = path()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).with_context(|| format!("解析 {}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(err) => Err(err).with_context(|| format!("读取 {}", path.display())),
    }
}

/// 当前配置快照。
pub fn get() -> Arc<Config> {
    Arc::clone(&store().current.read().expect("config lock poisoned"))
}

/// 修改配置并落盘；成功后通知订阅者。写盘失败时内存里的配置不变。
pub fn update(f: impl FnOnce(&mut Config)) -> Result<()> {
    let store = store();
    let next = {
        let current = store.current.read().expect("config lock poisoned");
        let mut next = Config::clone(&current);
        f(&mut next);
        next
    };
    let path = path()?;
    let text = serde_json::to_string_pretty(&next)?;
    // 先写临时文件再改名，避免写到一半断电留下坏文件。
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("写入 {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("保存 {}", path.display()))?;
    let next = Arc::new(next);
    *store.current.write().expect("config lock poisoned") = Arc::clone(&next);
    for listener in store
        .listeners
        .lock()
        .expect("config listeners poisoned")
        .iter()
    {
        listener(&next);
    }
    Ok(())
}

/// 配置变化时回调（在调用 `update` 的线程上执行）。
pub fn subscribe(listener: impl Fn(&Config) + Send + Sync + 'static) {
    store()
        .listeners
        .lock()
        .expect("config listeners poisoned")
        .push(Box::new(listener));
}

/// 某个扩展是否启用（未配置视为启用）。
pub fn extension_enabled(provider_id: &str) -> bool {
    get()
        .extensions
        .get(provider_id)
        .is_none_or(|ext| ext.enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_files_missing_fields_still_load() {
        // 升级后旧的 config.json 会缺新字段；读不出来就等于把用户设置全丢了。
        let config: Config = serde_json::from_str(r#"{"general":{"hotkey":"cmd-space"}}"#).unwrap();
        assert_eq!(config.general.hotkey, "cmd-space");
        assert_eq!(config.general.reset_after_secs, 90);
        assert!(config.clipboard.skip_concealed);
    }

    #[test]
    fn round_trips_through_json() {
        let mut config = Config::default();
        config.models.push(ModelConfig {
            id: "m1".into(),
            name: "DeepSeek".into(),
            kind: ModelKind::OpenAiCompatible,
            base_url: "https://api.deepseek.com/v1".into(),
            model: "deepseek-chat".into(),
        });
        let text = serde_json::to_string(&config).unwrap();
        assert_eq!(serde_json::from_str::<Config>(&text).unwrap(), config);
    }
}
