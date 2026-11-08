//! 设置窗口的纯逻辑：页面目录、各控件选项与配置字段之间的映射。不碰界面，便于单测。

use std::path::Path;

use crate::config::{Appearance, Config, PanelScreen};
use crate::provider::Tint;

// ---------------------------------------------------------------- 页面

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Extensions,
    Models,
    Agents,
    Clipboard,
    Screenshot,
    Appearance,
    About,
}

impl Page {
    pub const ALL: [Page; 8] = [
        Page::General,
        Page::Extensions,
        Page::Models,
        Page::Agents,
        Page::Clipboard,
        Page::Screenshot,
        Page::Appearance,
        Page::About,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::General => "通用",
            Page::Extensions => "扩展",
            Page::Models => "模型",
            Page::Agents => "Agent",
            Page::Clipboard => "剪贴板",
            Page::Screenshot => "截图",
            Page::Appearance => "外观",
            Page::About => "关于",
        }
    }

    /// Lucide 图标名与底块颜色（设计稿侧栏）。
    pub fn icon(self) -> (&'static str, Tint) {
        match self {
            Page::General => ("settings", Tint::Gray),
            Page::Extensions => ("puzzle", Tint::Blue),
            Page::Models => ("cpu", Tint::Teal),
            Page::Agents => ("sparkles", Tint::Purple),
            Page::Clipboard => ("clipboard", Tint::Orange),
            Page::Screenshot => ("scan", Tint::Indigo),
            Page::Appearance => ("contrast", Tint::Graphite),
            Page::About => ("info", Tint::Gray),
        }
    }

    /// 侧栏搜索除了页面名，还按页内设置项匹配（搜「快捷键」能找到「通用」）。
    fn keywords(self) -> &'static [&'static str] {
        match self {
            Page::General => &[
                "快捷键",
                "热键",
                "唤起",
                "开机",
                "登录",
                "启动",
                "菜单栏",
                "清空",
                "输入法",
                "屏幕",
                "更新",
                "hotkey",
            ],
            Page::Extensions => &["插件", "别名", "脚本", "raycast", "启用"],
            Page::Models => &[
                "api", "key", "翻译", "claude", "openai", "ollama", "deepseek",
            ],
            Page::Agents => &["claude", "codex", "会话"],
            Page::Clipboard => &["历史", "粘贴", "隐私", "密码", "保留"],
            Page::Screenshot => &["ocr", "文字识别", "语言", "保存", "目录"],
            Page::Appearance => &["深色", "浅色", "主题", "暗色", "dark", "light"],
            Page::About => &["版本", "数据", "目录", "helmp"],
        }
    }

    pub fn matches(self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        self.title().to_lowercase().contains(&query)
            || self
                .keywords()
                .iter()
                .any(|kw| kw.contains(query.as_str()) || query.contains(kw))
    }
}

// ---------------------------------------------------------------- 通用

/// 「永不」清空：隐藏多久都保留上次输入。启动器按 `Duration::from_secs` 比较，u64::MAX 永远达不到。
pub const RESET_NEVER: u64 = u64::MAX;

/// 「重新打开时清空输入」的选项：(秒, 显示名)。
pub const RESET_OPTIONS: [(u64, &str); 5] = [
    (0, "每次都清空"),
    (30, "隐藏 30 秒后"),
    (90, "隐藏 90 秒后"),
    (180, "隐藏 3 分钟后"),
    (RESET_NEVER, "永不"),
];

pub fn reset_label(secs: u64) -> String {
    RESET_OPTIONS
        .iter()
        .find(|(value, _)| *value == secs)
        .map(|(_, label)| label.to_string())
        // 手改 config.json 得到的其他值也要如实显示，而不是假装是某个选项。
        .unwrap_or_else(|| format!("隐藏 {secs} 秒后"))
}

pub const PANEL_SCREEN_OPTIONS: [(PanelScreen, &str); 2] = [
    (PanelScreen::Mouse, "鼠标所在的屏幕"),
    (PanelScreen::Primary, "主屏幕"),
];

pub fn panel_screen_label(screen: PanelScreen) -> &'static str {
    PANEL_SCREEN_OPTIONS
        .iter()
        .find(|(value, _)| *value == screen)
        .map(|(_, label)| *label)
        .unwrap_or("鼠标所在的屏幕")
}

// ---------------------------------------------------------------- 剪贴板

/// 保留时长：(天数, 显示名)，None = 永久。
pub const RETENTION_OPTIONS: [(Option<u32>, &str); 5] = [
    (Some(1), "1 天"),
    (Some(7), "7 天"),
    (Some(30), "30 天"),
    (Some(90), "90 天"),
    (None, "永久"),
];

pub fn retention_label(days: Option<u32>) -> String {
    RETENTION_OPTIONS
        .iter()
        .find(|(value, _)| *value == days)
        .map(|(_, label)| label.to_string())
        .unwrap_or_else(|| match days {
            Some(days) => format!("{days} 天"),
            None => "永久".into(),
        })
}

/// 加入忽略列表；已存在则不重复。返回是否有变化。
pub fn add_ignored_app(config: &mut Config, bundle_id: &str) -> bool {
    let bundle_id = bundle_id.trim();
    if bundle_id.is_empty()
        || config
            .clipboard
            .ignore_apps
            .iter()
            .any(|id| id == bundle_id)
    {
        return false;
    }
    config.clipboard.ignore_apps.push(bundle_id.to_string());
    true
}

/// 从 `.app` 的 Info.plist 读 bundle id。
pub fn bundle_id_of(app: &Path) -> anyhow::Result<String> {
    use anyhow::Context as _;
    let plist_path = app.join("Contents/Info.plist");
    let value = plist::Value::from_file(&plist_path)
        .with_context(|| format!("读取 {}", plist_path.display()))?;
    value
        .as_dictionary()
        .and_then(|dict| dict.get("CFBundleIdentifier"))
        .and_then(|id| id.as_string())
        .map(str::to_string)
        .with_context(|| format!("{} 没有 CFBundleIdentifier", plist_path.display()))
}

// ---------------------------------------------------------------- 外观

pub const APPEARANCE_OPTIONS: [(Appearance, &str); 3] = [
    (Appearance::System, "跟随系统"),
    (Appearance::Light, "浅色"),
    (Appearance::Dark, "深色"),
];

// ---------------------------------------------------------------- 扩展

/// 扩展表格里一行的展示信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionMeta {
    pub name: &'static str,
    /// 「内置」或「脚本」。
    pub kind: &'static str,
    pub icon: &'static str,
    pub tint: Tint,
    pub note: Option<&'static str>,
}

pub fn extension_meta(provider_id: &str) -> ExtensionMeta {
    let (name, kind, icon, tint, note) = match provider_id {
        "apps" => ("应用搜索", "内置", "app-window", Tint::Blue, None),
        "agents" => ("Agent 会话", "内置", "sparkles", Tint::Purple, None),
        "clipboard" => ("剪贴板历史", "内置", "clipboard", Tint::Orange, None),
        "screenshot" => ("截图与文字识别", "内置", "scan-text", Tint::Indigo, None),
        "scripts" => ("脚本命令", "脚本", "square-terminal", Tint::Graphite, None),
        "translate" => (
            "翻译",
            "内置",
            "languages",
            Tint::Teal,
            Some("使用「模型」里配置的模型"),
        ),
        "builtin" => ("系统命令", "内置", "settings", Tint::Gray, None),
        // 新插件还没登记显示名：显示 id，至少能开关。
        _ => ("", "内置", "puzzle", Tint::Gray, None),
    };
    ExtensionMeta {
        name,
        kind,
        icon,
        tint,
        note,
    }
}

/// 插件声明的默认前缀（如 `"cb "`）去掉空格后作为别名的占位提示。
pub fn default_alias(prefixes: &[String]) -> Option<String> {
    prefixes
        .iter()
        .map(|prefix| prefix.trim())
        .find(|prefix| !prefix.is_empty())
        .map(str::to_string)
}

/// 别名输入框的值 → 配置：去空白，空串 = 不设别名（用插件默认）。
pub fn normalize_alias(text: &str) -> Option<String> {
    let alias: String = text.split_whitespace().collect();
    (!alias.is_empty()).then_some(alias)
}

pub fn set_extension_enabled(config: &mut Config, provider_id: &str, enabled: bool) {
    config
        .extensions
        .entry(provider_id.to_string())
        .or_default()
        .enabled = enabled;
}

pub fn set_extension_alias(config: &mut Config, provider_id: &str, alias: Option<String>) {
    config
        .extensions
        .entry(provider_id.to_string())
        .or_default()
        .alias = alias;
}

pub fn set_extension_hotkey(config: &mut Config, provider_id: &str, hotkey: Option<String>) {
    config
        .extensions
        .entry(provider_id.to_string())
        .or_default()
        .hotkey = hotkey;
}

/// 加入脚本目录；默认目录或已存在的目录不重复加入。返回是否有变化。
pub fn add_script_dir(config: &mut Config, dir: &str, default_dir: &str) -> bool {
    let dir = dir.trim_end_matches('/');
    if dir.is_empty()
        || dir == default_dir.trim_end_matches('/')
        || config
            .script_dirs
            .iter()
            .any(|existing| existing.trim_end_matches('/') == dir)
    {
        return false;
    }
    config.script_dirs.push(dir.to_string());
    true
}

pub fn remove_script_dir(config: &mut Config, dir: &str) -> bool {
    let before = config.script_dirs.len();
    config.script_dirs.retain(|existing| existing != dir);
    config.script_dirs.len() != before
}

// ---------------------------------------------------------------- Agent

/// Agent 来源开关存在 `extensions` 里，键为 `agents.<来源>`（见报告：需要 agents 插件读取）。
pub const AGENT_SOURCES: [(&str, &str, &str); 2] = [
    (
        "agents.claude",
        "Claude Code",
        "读取 ~/.claude 下的会话记录与运行状态",
    ),
    (
        "agents.codex",
        "Codex",
        "读取 ~/.codex 下的会话记录与运行状态",
    ),
];

// ---------------------------------------------------------------- 截图

/// Vision 支持的常用识别语言：(代码, 显示名)。
pub const OCR_LANGUAGES: [(&str, &str); 6] = [
    ("zh-Hans", "简体中文"),
    ("zh-Hant", "繁体中文"),
    ("en-US", "英语"),
    ("ja-JP", "日语"),
    ("ko-KR", "韩语"),
    ("fr-FR", "法语"),
];

/// 开关某个识别语言。至少保留一个：全关掉 Vision 会退回默认语言，和界面显示不符。
/// 返回是否有变化。
pub fn toggle_ocr_language(config: &mut Config, code: &str, on: bool) -> bool {
    let languages = &mut config.screenshot.ocr_languages;
    let present = languages.iter().any(|lang| lang == code);
    match (on, present) {
        (true, false) => {
            languages.push(code.to_string());
            true
        }
        (false, true) if languages.len() > 1 => {
            languages.retain(|lang| lang != code);
            true
        }
        _ => false,
    }
}

// ---------------------------------------------------------------- 通用工具

/// 把家目录前缀显示成 `~`。
pub fn abbreviate_home(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return path.to_string();
    }
    if path == home {
        return "~".into();
    }
    match path.strip_prefix(home) {
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_search_finds_pages_by_settings_inside() {
        // 用户记得的是设置项名字而不是它在哪一页，搜不到就只能逐页翻。
        assert!(Page::General.matches("快捷键"));
        assert!(Page::Screenshot.matches("OCR"));
        assert!(Page::Appearance.matches("深色"));
        assert!(!Page::About.matches("快捷键"));
        assert!(Page::ALL.iter().all(|page| page.matches("  ")));
    }

    #[test]
    fn reset_options_map_to_config_seconds() {
        // 启动器按秒数判断是否清空输入，选项和秒数对不上就会和界面描述的行为不一致。
        assert_eq!(reset_label(90), "隐藏 90 秒后");
        assert_eq!(reset_label(0), "每次都清空");
        assert_eq!(reset_label(RESET_NEVER), "永不");
        assert_eq!(reset_label(45), "隐藏 45 秒后");
        // 「永不」必须真的永远达不到，启动器用 Duration::from_secs 比较隐藏时长。
        assert!(
            std::time::Duration::from_secs(RESET_NEVER)
                > std::time::Duration::from_secs(86400 * 365 * 100)
        );
    }

    #[test]
    fn retention_none_means_forever() {
        // None 是「永久」：显示成别的值会让用户以为历史会被删。
        assert_eq!(retention_label(None), "永久");
        assert_eq!(retention_label(Some(7)), "7 天");
        assert_eq!(retention_label(Some(14)), "14 天");
    }

    #[test]
    fn extension_switches_write_per_provider_config() {
        // 关掉一个插件只能影响它自己；未配置的插件仍按启用处理。
        let mut config = Config::default();
        set_extension_enabled(&mut config, "clipboard", false);
        set_extension_alias(&mut config, "clipboard", normalize_alias(" c b "));
        set_extension_hotkey(&mut config, "agents", Some("alt-a".into()));
        let clipboard = &config.extensions["clipboard"];
        assert!(!clipboard.enabled);
        assert_eq!(clipboard.alias.as_deref(), Some("cb"));
        let agents = &config.extensions["agents"];
        assert!(agents.enabled, "只设快捷键不应顺带关掉插件");
        assert_eq!(agents.hotkey.as_deref(), Some("alt-a"));
        assert_eq!(normalize_alias("   "), None);
    }

    #[test]
    fn default_alias_comes_from_scope_prefix() {
        // 别名列显示插件默认前缀，用户才知道不改也能用 `cb ` 进入。
        assert_eq!(default_alias(&["cb ".into()]).as_deref(), Some("cb"));
        assert_eq!(
            default_alias(&[" ".into(), "ag ".into()]).as_deref(),
            Some("ag")
        );
        assert_eq!(default_alias(&[]), None);
    }

    #[test]
    fn script_dirs_dedupe_and_keep_default_implicit() {
        // 默认目录总是加载，再写进 script_dirs 会让同一批脚本出现两次。
        let mut config = Config::default();
        let default = "/data/scripts";
        assert!(!add_script_dir(&mut config, "/data/scripts/", default));
        assert!(add_script_dir(&mut config, "/x/commands/", default));
        assert!(!add_script_dir(&mut config, "/x/commands", default));
        assert_eq!(config.script_dirs, ["/x/commands"]);
        assert!(remove_script_dir(&mut config, "/x/commands"));
        assert!(config.script_dirs.is_empty());
    }

    #[test]
    fn ocr_languages_keep_at_least_one() {
        // 全部关掉时 Vision 会悄悄回到默认语言，界面却显示「都没选」。
        let mut config = Config::default();
        assert!(toggle_ocr_language(&mut config, "ja-JP", true));
        assert!(!toggle_ocr_language(&mut config, "ja-JP", true));
        assert!(toggle_ocr_language(&mut config, "zh-Hans", false));
        assert!(toggle_ocr_language(&mut config, "en-US", false));
        assert!(!toggle_ocr_language(&mut config, "ja-JP", false));
        assert_eq!(config.screenshot.ocr_languages, ["ja-JP"]);
    }

    #[test]
    fn ignored_apps_are_unique() {
        let mut config = Config::default();
        assert!(!add_ignored_app(&mut config, "com.apple.Passwords"));
        assert!(add_ignored_app(&mut config, "com.agilebits.onepassword7"));
        assert_eq!(config.clipboard.ignore_apps.len(), 2);
    }

    #[test]
    fn reads_bundle_id_from_a_real_app() {
        // 「不记录以下应用」存的是 bundle id，剪贴板插件按它比对来源 App。
        let id = bundle_id_of(Path::new("/System/Applications/Calculator.app")).unwrap();
        assert_eq!(id, "com.apple.calculator");
        assert!(bundle_id_of(Path::new("/nonexistent.app")).is_err());
    }

    #[test]
    fn abbreviates_home_only_on_path_boundaries() {
        assert_eq!(abbreviate_home("/Users/a/code", "/Users/a"), "~/code");
        assert_eq!(abbreviate_home("/Users/a", "/Users/a/"), "~");
        // /Users/ab 不在 /Users/a 下面。
        assert_eq!(abbreviate_home("/Users/ab/x", "/Users/a"), "/Users/ab/x");
    }

    #[test]
    fn known_providers_have_display_names() {
        // 扩展表格里显示 provider id（如 `builtin`）对用户没有意义。
        for id in [
            "apps",
            "agents",
            "clipboard",
            "screenshot",
            "scripts",
            "translate",
            "builtin",
        ] {
            assert!(!extension_meta(id).name.is_empty(), "{id}");
        }
        assert_eq!(extension_meta("scripts").kind, "脚本");
    }
}
