//! 用户配置：`<data_dir>/config.json`。设置窗口读写它，各模块按需读取。
//!
//! - `get()` 返回当前配置快照（廉价的 `Arc` 克隆）。
//! - `update(|c| ...)` 修改、落盘，并通知所有订阅者。
//! - 字段都有默认值（`#[serde(default)]`），旧版本的文件缺字段也能读。
//! - API Key 不在这里：存在钥匙串，见 `ModelConfig::keychain_account`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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
    /// 网页搜索兜底：引擎列表的顺序就是结果里的顺序。
    #[serde(default)]
    pub web_search: WebSearchConfig,
    #[serde(default)]
    pub system_commands: SystemCommandsConfig,
    /// 文件搜索：搜索范围、排除目录、是否混入根搜索。
    #[serde(default)]
    pub files: FilesConfig,
    #[serde(default)]
    pub devtools: DevToolsConfig,
    pub appearance: Appearance,
    /// 额外的脚本目录（默认目录 `<data_dir>/scripts` 总是包含）。
    pub script_dirs: Vec<String>,
    /// 清单插件（`plugin.json`）按插件 id 的启用状态、授权与非密钥偏好设置。密钥只在钥匙串。
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginSettings>,
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
            web_search: WebSearchConfig::default(),
            system_commands: SystemCommandsConfig::default(),
            files: FilesConfig::default(),
            devtools: DevToolsConfig::default(),
            appearance: Appearance::System,
            script_dirs: Vec::new(),
            plugins: BTreeMap::new(),
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
    pub panel_screen: PanelScreen,
    pub auto_check_updates: bool,
    /// 界面语言；旧配置没有这个字段时跟随系统。
    pub language: Language,
}

impl Default for General {
    fn default() -> Self {
        Self {
            hotkey: "alt-space".into(),
            launch_at_login: false,
            show_menubar_icon: true,
            reset_after_secs: 90,
            panel_screen: PanelScreen::Mouse,
            auto_check_updates: true,
            language: Language::System,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    /// 跟随系统首选语言（见 `i18n::resolve`）。
    #[default]
    System,
    ZhHans,
    En,
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

/// 一个清单插件的用户设置（见 `providers::plugins`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PluginSettings {
    /// None = 按来源的默认值：自带插件默认启用，用户插件默认停用（要先确认权限）。
    pub enabled: Option<bool>,
    /// 用户启用时确认过的权限（`Permissions::keys`）。清单的权限变了就要重新确认。
    pub consent: Vec<String>,
    /// 非密钥偏好设置的值（text / select / checkbox）。secret 类型永远不进这里。
    pub prefs: BTreeMap<String, String>,
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

/// 内置网页搜索引擎：(id, 网址模板, 默认是否启用)，顺序即默认顺序。
/// 以后新增的引擎默认关闭，会自动出现在老用户列表的末尾。显示名随界面语言，见 `providers::websearch::engine_name`。
pub const BUILTIN_SEARCH_ENGINES: &[(&str, &str, bool)] = &[
    ("google", "https://www.google.com/search?q={query}", true),
    ("baidu", "https://www.baidu.com/s?wd={query}", true),
    ("github", "https://github.com/search?q={query}", true),
    ("bing", "https://www.bing.com/search?q={query}", false),
    (
        "bing-translate",
        "https://www.bing.com/translator?text={query}",
        false,
    ),
];

/// 网页搜索设置：只存用户的改动（内置引擎的开关、整体顺序）和自定义引擎，内置引擎本身在代码里。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(from = "RawWebSearchConfig")]
pub struct WebSearchConfig {
    /// 用户排过的顺序（引擎 id，内置与自定义混排）；不在里面的引擎按默认顺序排在后面。
    pub order: Vec<String>,
    /// 内置引擎的开关（id → 是否启用）；没写的用默认值。
    pub builtin: BTreeMap<String, bool>,
    pub custom: Vec<CustomSearchEngine>,
}

/// 用户自己加的引擎。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CustomSearchEngine {
    /// 稳定 id（`custom-<n>`），也是结果 id 的后缀。
    pub id: String,
    pub name: String,
    /// 网址模板，`{query}` 换成 URL 编码后的查询。
    pub url: String,
    pub enabled: bool,
}

impl Default for CustomSearchEngine {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            url: String::new(),
            enabled: true,
        }
    }
}

/// 读配置时的中间形态：兼容旧格式（一整份 `engines` 列表）。
#[derive(Deserialize, Default)]
#[serde(default)]
struct RawWebSearchConfig {
    order: Vec<String>,
    builtin: BTreeMap<String, bool>,
    custom: Vec<CustomSearchEngine>,
    /// 旧格式：内置与自定义混在一起、带网址的完整列表。
    engines: Option<Vec<CustomSearchEngine>>,
}

impl From<RawWebSearchConfig> for WebSearchConfig {
    fn from(raw: RawWebSearchConfig) -> Self {
        let mut config = Self {
            order: raw.order,
            builtin: raw.builtin,
            custom: raw.custom,
        };
        // 旧格式迁移：顺序照搬，内置引擎只留开关（网址以代码为准），其余当自定义引擎。
        if let Some(engines) = raw.engines
            && config == Self::default()
        {
            for engine in engines {
                config.order.push(engine.id.clone());
                if is_builtin_search_engine(&engine.id) {
                    config.builtin.insert(engine.id, engine.enabled);
                } else {
                    config.custom.push(engine);
                }
            }
        }
        config
    }
}

pub fn is_builtin_search_engine(id: &str) -> bool {
    BUILTIN_SEARCH_ENGINES
        .iter()
        .any(|(builtin, _, _)| *builtin == id)
}

/// 合并了用户改动后的一个引擎（不落盘）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchEngine {
    /// 稳定 id，也是结果 id 的后缀（`websearch:<id>`）。
    pub id: String,
    /// 自定义引擎的名字；内置引擎为空，显示名按界面语言取。
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub builtin: bool,
}

impl WebSearchConfig {
    /// 所有引擎，按用户的顺序；没排过的（含新增的内置引擎）按默认顺序排在后面。
    pub fn engines(&self) -> Vec<WebSearchEngine> {
        self.resolve(BUILTIN_SEARCH_ENGINES)
    }

    fn resolve(&self, catalog: &[(&str, &str, bool)]) -> Vec<WebSearchEngine> {
        let builtin = catalog.iter().map(|(id, url, enabled)| WebSearchEngine {
            id: id.to_string(),
            name: String::new(),
            url: url.to_string(),
            enabled: self.builtin.get(*id).copied().unwrap_or(*enabled),
            builtin: true,
        });
        let custom = self.custom.iter().map(|engine| WebSearchEngine {
            id: engine.id.clone(),
            name: engine.name.clone(),
            url: engine.url.clone(),
            enabled: engine.enabled,
            builtin: false,
        });
        let mut all: Vec<_> = builtin.chain(custom).collect();
        // 稳定排序：不在 order 里的保持「内置在前、自定义在后」的默认顺序。
        all.sort_by_key(|engine| {
            self.order
                .iter()
                .position(|id| *id == engine.id)
                .unwrap_or(usize::MAX)
        });
        all
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SystemCommandsConfig {
    /// 清空废纸篓、重启、关机、注销、退出所有应用：先出确认页，确认后才执行（查询末尾加 ` !` 可跳过）。
    pub confirm_dangerous: bool,
}

impl Default for SystemCommandsConfig {
    fn default() -> Self {
        Self {
            confirm_dangerous: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct FilesConfig {
    /// 搜索范围（目录，`~` 开头表示家目录）。
    pub roots: Vec<String>,
    /// 排除的目录：不含 `/` 的是目录名（任意层级都排除，如 `node_modules`），含 `/` 的是路径前缀。
    pub excluded: Vec<String>,
    /// 根搜索里是否混入文件结果（查询至少 2 个字符）。
    pub in_root_search: bool,
}

impl Default for FilesConfig {
    fn default() -> Self {
        Self {
            roots: vec!["~".into()],
            excluded: ["node_modules", ".git", "Library", "target"]
                .map(str::to_string)
                .to_vec(),
            in_root_search: true,
        }
    }
}

/// 开发者小工具。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DevToolsConfig {
    /// 「公网 IP」只在回车时请求这个地址（返回纯文本 IP）。空 = https://api.ipify.org。
    pub public_ip_url: String,
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
                match path().and_then(|path| back_up_unreadable(&path)) {
                    Ok(backup) => eprintln!("helmp: 原配置已备份到 {}", backup.display()),
                    Err(err) => eprintln!("helmp: 备份读不出的配置失败：{err:#}"),
                }
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

/// 读不出的配置先复制一份：之后任何一次 `update`（比如引导页点「继续」）都会用默认配置覆盖原文件，
/// 手改配置时少了一个逗号，不该让模型、快捷键等全部设置跟着丢掉。
fn back_up_unreadable(path: &Path) -> Result<PathBuf> {
    // 固定文件名：配置一直坏着时每次启动（包括每次 CLI 调用）都会走到这里，不能越积越多。
    let backup = path.with_extension("json.unreadable");
    std::fs::copy(path, &backup)
        .with_context(|| format!("复制 {} 到 {}", path.display(), backup.display()))?;
    Ok(backup)
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
    std::fs::write(&tmp, text)
        .with_context(|| crate::i18n::trf!("写入 {}", "Couldn't write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| crate::i18n::trf!("保存 {}", "Couldn't save {}", path.display()))?;
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
    fn unreadable_config_is_kept_before_defaults_overwrite_it() {
        // 解析失败时用默认配置启动，下一次保存就会覆盖原文件；先备份，用户手改坏的配置才找得回来。
        let dir = std::env::temp_dir().join(format!("helmp-config-backup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let broken = r#"{"models":[{"id":"m1","name":"我的模型",}]"#;
        std::fs::write(&path, broken).unwrap();
        let backup = back_up_unreadable(&path).unwrap();
        std::fs::write(&path, "{}").unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), broken);
        assert_eq!(backup.parent(), Some(dir.as_path()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn old_files_missing_fields_still_load() {
        // 升级后旧的 config.json 会缺新字段；读不出来就等于把用户设置全丢了。
        let config: Config = serde_json::from_str(r#"{"general":{"hotkey":"cmd-space"}}"#).unwrap();
        assert_eq!(config.general.hotkey, "cmd-space");
        assert_eq!(config.general.reset_after_secs, 90);
        assert!(config.clipboard.skip_concealed);
        // 加语言设置之前的配置：跟随系统，而不是读失败。
        assert_eq!(config.general.language, Language::System);
        // 加网页搜索与系统命令之前的配置：默认引擎照常出现，危险命令仍要确认。
        let enabled: Vec<_> = config
            .web_search
            .engines()
            .iter()
            .filter(|engine| engine.enabled)
            .map(|engine| engine.id.clone())
            .collect();
        assert_eq!(enabled, ["google", "baidu", "github"]);
        assert!(config.system_commands.confirm_dangerous);
    }

    #[test]
    fn language_is_saved_with_stable_names() {
        // config.json 里的取值是对外的格式，改名会让用户手选的语言在升级后丢失。
        let mut config = Config::default();
        config.general.language = Language::ZhHans;
        let text = serde_json::to_string(&config).unwrap();
        assert!(text.contains(r#""language":"zh_hans""#), "{text}");
        let parsed: Config = serde_json::from_str(r#"{"general":{"language":"en"}}"#).unwrap();
        assert_eq!(parsed.general.language, Language::En);
    }

    #[test]
    fn migrates_the_old_engine_list() {
        // 旧格式把整份引擎列表连网址一起存着：升级后用户调过的顺序、开关和自定义引擎都不能丢。
        let config: Config = serde_json::from_str(
            r#"{"web_search":{"engines":[
                {"id":"github","url":"https://github.com/search?q={query}","enabled":true},
                {"id":"google","url":"https://www.google.com/search?q={query}","enabled":false},
                {"id":"zhihu","name":"知乎","url":"https://www.zhihu.com/search?q={query}","enabled":true},
                {"id":"baidu","url":"https://www.baidu.com/s?wd={query}","enabled":true}
            ]}}"#,
        )
        .unwrap();
        let engines = config.web_search.engines();
        let summary: Vec<_> = engines
            .iter()
            .map(|e| (e.id.as_str(), e.enabled, e.builtin))
            .collect();
        assert_eq!(
            summary,
            [
                ("github", true, true),
                ("google", false, true),
                ("zhihu", true, false),
                ("baidu", true, true),
                // 旧列表里没有的内置引擎补在末尾，用默认开关。
                ("bing", false, true),
                ("bing-translate", false, true),
            ]
        );
        assert_eq!(engines[2].name, "知乎");
        // 迁移后按新格式保存，再读回来一样。
        let text = serde_json::to_string(&config).unwrap();
        assert!(!text.contains(r#""engines""#), "{text}");
        let again: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(again.web_search.engines(), engines);
    }

    #[test]
    fn new_builtin_engines_show_up_at_the_end_for_existing_users() {
        // 只存用户的改动，所以以后新增的内置引擎会自动出现在老用户列表末尾（默认关闭），不会被吞掉。
        let config = WebSearchConfig {
            order: vec!["github".into(), "custom-1".into(), "google".into()],
            builtin: BTreeMap::from([("baidu".to_string(), false)]),
            custom: vec![CustomSearchEngine {
                id: "custom-1".into(),
                name: "知乎".into(),
                url: "https://www.zhihu.com/search?q={query}".into(),
                enabled: true,
            }],
        };
        let mut catalog = BUILTIN_SEARCH_ENGINES.to_vec();
        catalog.push(("duckduckgo", "https://duckduckgo.com/?q={query}", false));
        let summary: Vec<_> = config
            .resolve(&catalog)
            .into_iter()
            .map(|e| (e.id, e.enabled))
            .collect();
        let expected = [
            ("github", true),
            ("custom-1", true),
            ("google", true),
            ("baidu", false),
            ("bing", false),
            ("bing-translate", false),
            ("duckduckgo", false),
        ]
        .map(|(id, on)| (id.to_string(), on));
        assert_eq!(summary, expected);
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
