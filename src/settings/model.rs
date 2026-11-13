//! 设置窗口的纯逻辑：页面目录、各控件选项与配置字段之间的映射。不碰界面，便于单测。

use std::path::Path;

use crate::config::{
    Appearance, Config, CustomSearchEngine, Language, PanelScreen, is_builtin_search_engine,
};
use crate::i18n::{tr, trf};
use crate::provider::Tint;

// ---------------------------------------------------------------- 页面

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Extensions,
    Plugins,
    Mcp,
    Models,
    Agents,
    Clipboard,
    Snippets,
    Screenshot,
    Appearance,
    About,
}

impl Page {
    pub const ALL: [Page; 11] = [
        Page::General,
        Page::Extensions,
        Page::Plugins,
        Page::Mcp,
        Page::Models,
        Page::Agents,
        Page::Clipboard,
        Page::Snippets,
        Page::Screenshot,
        Page::Appearance,
        Page::About,
    ];

    /// 当前语言的页面名。
    pub fn title(self) -> &'static str {
        let (zh, en) = self.titles();
        tr(zh, en)
    }

    /// (中文名, 英文名)。侧栏搜索与 `HELMP_SETTINGS_PAGE` 两种都认，与界面语言无关。
    fn titles(self) -> (&'static str, &'static str) {
        match self {
            Page::General => ("通用", "General"),
            Page::Extensions => ("扩展", "Extensions"),
            Page::Plugins => ("插件", "Plugins"),
            Page::Mcp => ("MCP", "MCP"),
            Page::Models => ("模型", "Models"),
            Page::Agents => ("Agent", "Agents"),
            Page::Clipboard => ("剪贴板", "Clipboard"),
            Page::Snippets => ("片段", "Snippets"),
            Page::Screenshot => ("截图", "Screenshot"),
            Page::Appearance => ("外观", "Appearance"),
            Page::About => ("关于", "About"),
        }
    }

    /// 按页面名（中文或英文，英文不分大小写）找页面。自检 / 截图用的 `HELMP_SETTINGS_PAGE`。
    pub fn from_name(name: &str) -> Option<Page> {
        let name = name.trim();
        Page::ALL.into_iter().find(|page| {
            let (zh, en) = page.titles();
            name == zh || name.eq_ignore_ascii_case(en)
        })
    }

    /// Lucide 图标名与底块颜色（设计稿侧栏）。
    pub fn icon(self) -> (&'static str, Tint) {
        match self {
            Page::General => ("settings", Tint::Gray),
            Page::Extensions => ("puzzle", Tint::Blue),
            Page::Plugins => ("blocks", Tint::Indigo),
            Page::Mcp => ("plug", Tint::Teal),
            Page::Models => ("cpu", Tint::Teal),
            Page::Agents => ("sparkles", Tint::Purple),
            Page::Clipboard => ("clipboard", Tint::Orange),
            Page::Snippets => ("text-quote", Tint::Purple),
            Page::Screenshot => ("scan", Tint::Indigo),
            Page::Appearance => ("contrast", Tint::Graphite),
            Page::About => ("info", Tint::Gray),
        }
    }

    /// 侧栏搜索除了页面名，还按页内设置项匹配（搜「快捷键」能找到「通用」）。
    /// 中英文关键词都放在这里，与界面语言无关：英文界面下搜中文、中文界面下搜英文都能找到。
    /// 英文关键词一律小写（查询会先转小写）。
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
                "语言",
                "hotkey",
                "shortcut",
                "keyboard",
                "login",
                "launch",
                "menu bar",
                "input source",
                "screen",
                "update",
                "language",
            ],
            Page::Extensions => &[
                "插件",
                "别名",
                "脚本",
                "raycast",
                "启用",
                "extension",
                "plugin",
                "alias",
                "script",
                "enable",
            ],
            Page::Plugins => &[
                "插件",
                "清单",
                "权限",
                "flomo",
                "obsidian",
                "plugin",
                "manifest",
                "permission",
                "webhook",
            ],
            Page::Mcp => &[
                "mcp",
                "服务器",
                "工具",
                "导入",
                "server",
                "tool",
                "import",
                "claude",
                "cursor",
                "codex",
            ],
            Page::Models => &[
                "api",
                "key",
                "翻译",
                "claude",
                "openai",
                "ollama",
                "deepseek",
                "model",
                "translate",
                "translation",
            ],
            Page::Agents => &["claude", "codex", "会话", "agent", "session"],
            Page::Clipboard => &[
                "历史",
                "粘贴",
                "隐私",
                "密码",
                "保留",
                "clipboard",
                "history",
                "paste",
                "privacy",
                "password",
                "retention",
            ],
            Page::Snippets => &[
                "文本片段",
                "常用语",
                "模板",
                "导入",
                "snippet",
                "template",
                "import",
                "raycast",
                "alfred",
            ],
            Page::Screenshot => &[
                "ocr",
                "文字识别",
                "截图识别文字",
                "快捷键",
                "shortcut",
                "语言",
                "保存",
                "目录",
                "历史",
                "截图历史",
                "screenshot",
                "text recognition",
                "language",
                "save",
                "folder",
                "history",
            ],
            Page::Appearance => &[
                "深色",
                "浅色",
                "主题",
                "暗色",
                "dark",
                "light",
                "theme",
                "appearance",
            ],
            Page::About => &["版本", "数据", "目录", "helmp", "version", "data", "folder"],
        }
    }

    pub fn matches(self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let (zh, en) = self.titles();
        zh.to_lowercase().contains(&query)
            || en.to_lowercase().contains(&query)
            || self
                .keywords()
                .iter()
                .any(|kw| kw.contains(query.as_str()) || query.contains(kw))
    }
}

// ---------------------------------------------------------------- 通用

/// 「永不」清空：隐藏多久都保留上次输入。启动器按 `Duration::from_secs` 比较，u64::MAX 永远达不到。
pub const RESET_NEVER: u64 = u64::MAX;

/// 「重新打开时清空输入」的选项：(秒, 中文名, 英文名)。显示时用 `tr` 取当前语言。
pub const RESET_OPTIONS: [(u64, &str, &str); 5] = [
    (0, "每次都清空", "Immediately"),
    (30, "隐藏 30 秒后", "After 30 seconds"),
    (90, "隐藏 90 秒后", "After 90 seconds"),
    (180, "隐藏 3 分钟后", "After 3 minutes"),
    (RESET_NEVER, "永不", "Never"),
];

pub fn reset_label(secs: u64) -> String {
    RESET_OPTIONS
        .iter()
        .find(|(value, _, _)| *value == secs)
        .map(|(_, zh, en)| tr(zh, en).to_string())
        // 手改 config.json 得到的其他值也要如实显示，而不是假装是某个选项。
        .unwrap_or_else(|| trf!("隐藏 {secs} 秒后", "After {secs} seconds"))
}

/// 「面板出现在」的选项：(值, 中文名, 英文名)。
pub const PANEL_SCREEN_OPTIONS: [(PanelScreen, &str, &str); 2] = [
    (PanelScreen::Mouse, "鼠标所在的屏幕", "Screen with pointer"),
    (PanelScreen::Primary, "主屏幕", "Main screen"),
];

pub fn panel_screen_label(screen: PanelScreen) -> &'static str {
    PANEL_SCREEN_OPTIONS
        .iter()
        .find(|(value, _, _)| *value == screen)
        .map(|(_, zh, en)| tr(zh, en))
        .unwrap_or_else(|| tr("鼠标所在的屏幕", "Screen with pointer"))
}

/// 界面语言的选项：(值, 中文界面的显示名, 英文界面的显示名)。
/// 具体语言始终用各自语言的原名，看不懂当前界面的人也认得出自己的语言。
pub const LANGUAGE_OPTIONS: [(Language, &str, &str); 3] = [
    (Language::System, "跟随系统", "System Default"),
    (Language::ZhHans, "简体中文", "简体中文"),
    (Language::En, "English", "English"),
];

pub fn language_label(language: Language) -> &'static str {
    LANGUAGE_OPTIONS
        .iter()
        .find(|(value, _, _)| *value == language)
        .map(|(_, zh, en)| tr(zh, en))
        .unwrap_or_else(|| tr("跟随系统", "System Default"))
}

// ---------------------------------------------------------------- 剪贴板

/// 按时间清理的保留时长：(天数, 中文名, 英文名)。
pub const RETENTION_OPTIONS: [(Option<u32>, &str, &str); 5] = [
    (
        Some(crate::config::ClipboardConfig::MIN_RETENTION_DAYS),
        "3 天",
        "3 days",
    ),
    (Some(7), "7 天", "7 days"),
    (Some(30), "30 天", "30 days"),
    (Some(90), "90 天", "90 days"),
    (None, "不按时间清理", "No time limit"),
];

pub fn retention_label(days: Option<u32>) -> String {
    RETENTION_OPTIONS
        .iter()
        .find(|(value, _, _)| *value == days)
        .map(|(_, zh, en)| tr(zh, en).to_string())
        .unwrap_or_else(|| match days {
            Some(days) => trf!("{days} 天", "{days} days"),
            None => tr("不按时间清理", "No time limit").to_string(),
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
        .with_context(|| trf!("读取 {}", "Couldn't read {}", plist_path.display()))?;
    value
        .as_dictionary()
        .and_then(|dict| dict.get("CFBundleIdentifier"))
        .and_then(|id| id.as_string())
        .map(str::to_string)
        .with_context(|| {
            trf!(
                "{} 没有 CFBundleIdentifier",
                "{} has no CFBundleIdentifier",
                plist_path.display()
            )
        })
}

// ---------------------------------------------------------------- 外观

/// (值, 中文名, 英文名)；英文与 macOS「外观」一致。
pub const APPEARANCE_OPTIONS: [(Appearance, &str, &str); 3] = [
    (Appearance::System, "跟随系统", "Auto"),
    (Appearance::Light, "浅色", "Light"),
    (Appearance::Dark, "深色", "Dark"),
];

// ---------------------------------------------------------------- 扩展

/// 扩展表格里一行的展示信息（按调用时的语言取文字，每次渲染现取，不要存起来）。
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
    let builtin = tr("内置", "Built-in");
    let (name, kind, icon, tint, note) = match provider_id {
        "apps" => (
            tr("应用搜索", "Applications"),
            builtin,
            "app-window",
            Tint::Blue,
            None,
        ),
        "agents" => (
            tr("Agent 会话", "Agent Sessions"),
            builtin,
            "sparkles",
            Tint::Purple,
            None,
        ),
        "clipboard" => (
            tr("剪贴板历史", "Clipboard History"),
            builtin,
            "clipboard",
            Tint::Orange,
            None,
        ),
        "screenshot" => (
            tr("截图与文字识别", "Screenshot & OCR"),
            builtin,
            "scan-text",
            Tint::Indigo,
            None,
        ),
        "scripts" => (
            tr("脚本命令", "Script Commands"),
            tr("脚本", "Script"),
            "square-terminal",
            Tint::Graphite,
            None,
        ),
        "translate" => (
            tr("翻译", "Translate"),
            builtin,
            "languages",
            Tint::Teal,
            Some(tr(
                "使用「模型」里配置的模型",
                "Uses the model set up in Models",
            )),
        ),
        // 退出 Helmp、打开设置。「系统命令」名字给了真正的系统命令插件，这里改叫 Helmp 命令免得两行同名。
        "builtin" => (
            tr("Helmp 命令", "Helmp Commands"),
            builtin,
            "settings",
            Tint::Gray,
            None,
        ),
        "system" => (
            tr("系统命令", "System Commands"),
            builtin,
            "power",
            Tint::Red,
            Some(tr(
                "锁屏、睡眠、关机、清空废纸篓等",
                "Lock, sleep, shut down, empty Trash, and more",
            )),
        ),
        "calculator" => (
            tr("计算器", "Calculator"),
            builtin,
            "calculator",
            Tint::Orange,
            None,
        ),
        "processes" => (
            tr("进程与端口", "Processes & Ports"),
            builtin,
            "activity",
            Tint::Red,
            None,
        ),
        "window" => (
            tr("窗口管理", "Window Management"),
            builtin,
            "layout-grid",
            Tint::Blue,
            Some(tr("需要辅助功能权限", "Needs Accessibility permission")),
        ),
        "snippets" => (
            tr("文本片段", "Snippets"),
            builtin,
            "text-quote",
            Tint::Purple,
            None,
        ),
        "mcp" => (
            tr("MCP 工具", "MCP Tools"),
            tr("MCP", "MCP"),
            "plug",
            Tint::Teal,
            Some(tr(
                "把 MCP 服务器的工具变成命令，在「MCP」页添加服务器",
                "Turns MCP server tools into commands; add servers on the MCP page",
            )),
        ),
        "plugins" => (
            tr("清单插件", "Manifest Plugins"),
            tr("插件", "Plugin"),
            "blocks",
            Tint::Indigo,
            Some(tr(
                "flomo、Obsidian、快捷搜索和你装的插件，在「插件」页管理",
                "flomo, Obsidian, Quick Search and your own plugins; manage them on the Plugins page",
            )),
        ),
        "websearch" => (
            tr("网页搜索", "Web Search"),
            builtin,
            "globe",
            Tint::Blue,
            Some(tr(
                "没有本地结果时排在最前，引擎在下方调整",
                "Shown first when nothing local matches; set engines below",
            )),
        ),
        "files" => (
            tr("文件搜索", "File Search"),
            builtin,
            "file-search",
            Tint::Blue,
            Some(tr(
                "用 Spotlight 按文件名搜索，范围与排除目录在下方设置",
                "Searches file names with Spotlight; set folders below",
            )),
        ),
        "settings-panes" => (
            tr("系统设置面板", "System Settings Panes"),
            builtin,
            "settings-2",
            Tint::Gray,
            None,
        ),
        "toggles" => (
            tr("系统开关", "System Toggles"),
            builtin,
            "sliders-horizontal",
            Tint::Orange,
            Some(tr(
                "防休眠、音量、显示隐藏文件、弹出磁盘",
                "Keep awake, volume, hidden files, eject disks",
            )),
        ),
        "bookmarks" => (
            tr("浏览器书签", "Browser Bookmarks"),
            builtin,
            "bookmark",
            Tint::Orange,
            Some(tr(
                "Chrome、Edge、Brave、Arc、Safari（需完全磁盘访问）",
                "Chrome, Edge, Brave, Arc, Safari (needs Full Disk Access)",
            )),
        ),
        "dictionary" => (
            tr("词典", "Dictionary"),
            builtin,
            "book-a",
            Tint::Red,
            Some(tr(
                "使用 macOS 自带词典，离线查词",
                "Uses the built-in macOS dictionaries offline",
            )),
        ),
        "time" => (
            tr("时间与时区", "Time & Time Zones"),
            builtin,
            "clock",
            Tint::Blue,
            Some(tr(
                "时间戳、时区换算、相对时间",
                "Timestamps, time zone conversion, relative dates",
            )),
        ),
        "devtools" => (
            tr("开发工具", "Dev Tools"),
            builtin,
            "wrench",
            Tint::Graphite,
            Some(tr(
                "UUID、密码、哈希、编码、JSON、JWT、IP",
                "UUID, passwords, hashes, encoding, JSON, JWT, IP",
            )),
        ),
        "emoji" => (
            tr("Emoji 与符号", "Emoji & Symbols"),
            builtin,
            "face-grinning",
            Tint::Orange,
            None,
        ),
        "color" => (
            tr("取色器", "Color Picker"),
            builtin,
            "pipette",
            Tint::Purple,
            Some(tr(
                "取屏幕颜色，转换 HEX / RGB / HSL",
                "Pick screen colors; convert HEX / RGB / HSL",
            )),
        ),
        // 新插件还没登记显示名：显示 id，至少能开关。
        _ => ("", builtin, "puzzle", Tint::Gray, None),
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

/// 清除扩展的快捷键：写成空字符串（`None` 表示「用默认值」，清除后就又绑回默认了）。
pub fn clear_extension_hotkey(config: &mut Config, provider_id: &str) {
    set_extension_hotkey(config, provider_id, Some(String::new()));
}

/// 恢复扩展的默认快捷键：不再记自定义值，以后默认值变了也跟着变。
pub fn reset_extension_hotkey(config: &mut Config, provider_id: &str) {
    set_extension_hotkey(config, provider_id, None);
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

/// 网页搜索引擎的开关：内置引擎记在覆盖表里，自定义引擎改自己的字段。返回是否找到这个引擎。
pub fn set_web_search_enabled(config: &mut Config, id: &str, enabled: bool) -> bool {
    let web = &mut config.web_search;
    if is_builtin_search_engine(id) {
        web.builtin.insert(id.to_string(), enabled);
        return true;
    }
    match web.custom.iter_mut().find(|e| e.id == id) {
        Some(engine) => {
            engine.enabled = enabled;
            true
        }
        None => false,
    }
}

/// 把引擎上移（`up`）或下移一位；已在顶端 / 底端或找不到时不动，返回是否有变化。
/// 移动后把当前完整顺序写进 `order`。
pub fn move_web_search(config: &mut Config, id: &str, up: bool) -> bool {
    let mut ids: Vec<String> = config
        .web_search
        .engines()
        .into_iter()
        .map(|e| e.id)
        .collect();
    let Some(index) = ids.iter().position(|e| e == id) else {
        return false;
    };
    let target = if up {
        index.checked_sub(1)
    } else {
        Some(index + 1).filter(|&next| next < ids.len())
    };
    let Some(target) = target else {
        return false;
    };
    ids.swap(index, target);
    config.web_search.order = ids;
    true
}

/// 自定义引擎填写有误的原因。文案在显示时按语言取。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineError {
    EmptyName,
    MissingQuery,
}

impl EngineError {
    pub fn message(self) -> &'static str {
        match self {
            EngineError::EmptyName => tr("请填写名称", "Enter a name"),
            EngineError::MissingQuery => tr(
                "网址模板必须包含 {query}，搜索时会换成你输入的内容",
                "The URL template must contain {query}; it’s replaced with what you type",
            ),
        }
    }
}

fn validate_engine(name: &str, url: &str) -> Result<(String, String), EngineError> {
    let (name, url) = (name.trim(), url.trim());
    if name.is_empty() {
        return Err(EngineError::EmptyName);
    }
    if !url.contains("{query}") {
        return Err(EngineError::MissingQuery);
    }
    Ok((name.to_string(), url.to_string()))
}

/// 添加自定义引擎（默认启用，排在末尾），返回它的 id。填写有误时不改配置。
pub fn add_custom_engine(
    config: &mut Config,
    name: &str,
    url: &str,
) -> Result<String, EngineError> {
    let (name, url) = validate_engine(name, url)?;
    let web = &mut config.web_search;
    let id = (1..)
        .map(|n| format!("custom-{n}"))
        .find(|id| !web.custom.iter().any(|e| &e.id == id) && !web.order.contains(id))
        .unwrap_or_default();
    web.custom.push(CustomSearchEngine {
        id: id.clone(),
        name,
        url,
        enabled: true,
    });
    Ok(id)
}

/// 修改自定义引擎的名称与网址。填写有误时不改配置；找不到（含内置引擎）返回 `Ok(false)`。
pub fn update_custom_engine(
    config: &mut Config,
    id: &str,
    name: &str,
    url: &str,
) -> Result<bool, EngineError> {
    let (name, url) = validate_engine(name, url)?;
    match config.web_search.custom.iter_mut().find(|e| e.id == id) {
        Some(engine) => {
            engine.name = name;
            engine.url = url;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// 删除自定义引擎；内置引擎删不掉（只能关闭），返回是否删除了。
pub fn remove_custom_engine(config: &mut Config, id: &str) -> bool {
    let web = &mut config.web_search;
    let before = web.custom.len();
    web.custom.retain(|e| e.id != id);
    web.order.retain(|e| e != id);
    web.custom.len() != before
}

// ---------------------------------------------------------------- Agent

/// Agent 来源开关存在 `extensions` 里，键为 `agents.<来源>`（见报告：需要 agents 插件读取）。
/// (键, 名称, 中文说明, 英文说明)。
pub const AGENT_SOURCES: [(&str, &str, &str, &str); 2] = [
    (
        "agents.claude",
        "Claude Code",
        "读取 ~/.claude 下的会话记录与运行状态",
        "Reads session history and status from ~/.claude",
    ),
    (
        "agents.codex",
        "Codex",
        "读取 ~/.codex 下的会话记录与运行状态",
        "Reads session history and status from ~/.codex",
    ),
];

// ---------------------------------------------------------------- 截图

/// Vision 支持的常用识别语言：(代码, 中文名, 英文名)。默认只开中英文，其余按需打开。
pub const OCR_LANGUAGES: [(&str, &str, &str); 11] = [
    ("zh-Hans", "简体中文", "Chinese, Simplified"),
    ("zh-Hant", "繁体中文", "Chinese, Traditional"),
    ("en-US", "英语", "English"),
    ("ja-JP", "日语", "Japanese"),
    ("ko-KR", "韩语", "Korean"),
    ("fr-FR", "法语", "French"),
    ("de-DE", "德语", "German"),
    ("es-ES", "西班牙语", "Spanish"),
    ("it-IT", "意大利语", "Italian"),
    ("pt-BR", "葡萄牙语", "Portuguese"),
    ("ru-RU", "俄语", "Russian"),
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

// ---------------------------------------------------------------- 文件搜索

/// 逗号（中英文）或换行分隔的列表 → 去空白、去空项。
pub fn parse_list(text: &str) -> Vec<String> {
    text.split([',', '，', '\n'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn format_list(items: &[String]) -> String {
    items.join(", ")
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
        assert!(Page::Screenshot.matches("截图历史"));
        assert!(Page::Appearance.matches("深色"));
        assert!(!Page::About.matches("快捷键"));
        assert!(Page::ALL.iter().all(|page| page.matches("  ")));
    }

    #[test]
    fn sidebar_search_works_in_either_language() {
        // 界面语言和用户脑子里的词不一定一致：英文界面下搜「快捷键」、中文界面下搜 "hotkey" /
        // "clipboard" 都得找到对应页面，否则切换语言后就找不到设置项了。
        use crate::i18n::{Lang, with_lang};
        for lang in [Lang::Zh, Lang::En] {
            with_lang(lang, || {
                assert!(Page::General.matches("快捷键"), "{lang:?}");
                assert!(Page::General.matches("hotkey"), "{lang:?}");
                assert!(Page::General.matches("Language"), "{lang:?}");
                assert!(Page::General.matches("语言"), "{lang:?}");
                assert!(Page::Clipboard.matches("clipboard"), "{lang:?}");
                assert!(Page::Clipboard.matches("剪贴板"), "{lang:?}");
                assert!(Page::Appearance.matches("外观"), "{lang:?}");
                assert!(Page::Appearance.matches("Appearance"), "{lang:?}");
                assert!(!Page::About.matches("hotkey"), "{lang:?}");
            });
        }
    }

    #[test]
    fn page_titles_follow_language_and_env_name_accepts_both() {
        // 侧栏与页标题随语言变；HELMP_SETTINGS_PAGE 是脚本里写死的，不能因为界面语言不同就打不开。
        use crate::i18n::{Lang, with_lang};
        with_lang(Lang::En, || {
            assert_eq!(Page::General.title(), "General");
            assert_eq!(Page::Agents.title(), "Agents");
            assert_eq!(Page::from_name("剪贴板"), Some(Page::Clipboard));
        });
        with_lang(Lang::Zh, || {
            assert_eq!(Page::General.title(), "通用");
            assert_eq!(Page::from_name("Clipboard"), Some(Page::Clipboard));
            assert_eq!(Page::from_name("about"), Some(Page::About));
            assert_eq!(Page::from_name("关于"), Some(Page::About));
        });
        assert_eq!(Page::from_name("不存在"), None);
        // 每一页的两个名字都能反查回自己，新加页面忘了登记会在这里暴露。
        for page in Page::ALL {
            let (zh, en) = page.titles();
            assert_eq!(Page::from_name(zh), Some(page));
            assert_eq!(Page::from_name(en), Some(page));
        }
    }

    #[test]
    fn language_options_cover_every_setting() {
        // 语言菜单少一项，用户就选不回那个语言；写入的值必须就是选中项对应的枚举。
        for language in [Language::System, Language::ZhHans, Language::En] {
            assert_eq!(
                LANGUAGE_OPTIONS
                    .iter()
                    .filter(|(value, _, _)| *value == language)
                    .count(),
                1,
                "{language:?}"
            );
        }
    }

    #[test]
    fn language_labels_keep_native_names() {
        // 具体语言用原名显示：误切到看不懂的界面时，用户仍能认出「简体中文」/ "English" 切回来。
        use crate::i18n::{Lang, with_lang};
        with_lang(Lang::En, || {
            assert_eq!(language_label(Language::System), "System Default");
            assert_eq!(language_label(Language::ZhHans), "简体中文");
            assert_eq!(language_label(Language::En), "English");
        });
        with_lang(Lang::Zh, || {
            assert_eq!(language_label(Language::System), "跟随系统");
            assert_eq!(language_label(Language::ZhHans), "简体中文");
            assert_eq!(language_label(Language::En), "English");
        });
    }

    #[test]
    fn option_labels_switch_to_english() {
        // 下拉菜单显示名随语言变，但选项背后的值不变（配置里存的仍是秒数 / 天数）。
        use crate::i18n::{Lang, with_lang};
        with_lang(Lang::En, || {
            assert_eq!(reset_label(90), "After 90 seconds");
            assert_eq!(reset_label(RESET_NEVER), "Never");
            assert_eq!(reset_label(45), "After 45 seconds");
            assert_eq!(retention_label(None), "No time limit");
            assert_eq!(retention_label(Some(14)), "14 days");
            assert_eq!(panel_screen_label(PanelScreen::Primary), "Main screen");
            assert_eq!(extension_meta("scripts").kind, "Script");
            assert_eq!(extension_meta("clipboard").name, "Clipboard History");
        });
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
    fn retention_options_start_at_three_days() {
        assert_eq!(retention_label(None), "不按时间清理");
        assert_eq!(RETENTION_OPTIONS[0].0, Some(3));
        assert_eq!(retention_label(Some(3)), "3 天");
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
    fn web_search_order_moves_one_step_and_stops_at_the_ends() {
        // 设置里用上下按钮调整顺序：越界时不能绕到另一头，也不能动到别的引擎。
        let mut config = Config::default();
        let order = |c: &Config| -> Vec<String> {
            c.web_search.engines().into_iter().map(|e| e.id).collect()
        };
        assert!(!move_web_search(&mut config, "google", true));
        assert!(move_web_search(&mut config, "github", true));
        assert_eq!(order(&config)[..3], ["google", "github", "baidu"]);
        let last = order(&config).last().cloned().unwrap();
        assert!(!move_web_search(&mut config, &last, false));
        assert!(!move_web_search(&mut config, "nope", false));
        assert!(set_web_search_enabled(&mut config, "bing", true));
        let enabled = |c: &Config, id: &str| {
            c.web_search
                .engines()
                .iter()
                .any(|e| e.id == id && e.enabled)
        };
        assert!(enabled(&config, "bing"));
        assert!(!set_web_search_enabled(&mut config, "nope", true));
    }

    #[test]
    fn custom_engines_need_a_name_and_a_query_slot() {
        // 模板里没有 {query} 的引擎永远搜不到用户输入的内容，不能悄悄存下来。
        let mut config = Config::default();
        assert_eq!(
            add_custom_engine(&mut config, "知乎", "https://www.zhihu.com/"),
            Err(EngineError::MissingQuery)
        );
        assert_eq!(
            add_custom_engine(&mut config, "  ", "https://x/?q={query}"),
            Err(EngineError::EmptyName)
        );
        assert!(config.web_search.custom.is_empty(), "填写有误时不能保存");

        let id = add_custom_engine(
            &mut config,
            " 知乎 ",
            " https://www.zhihu.com/search?q={query} ",
        )
        .unwrap();
        let second =
            add_custom_engine(&mut config, "V2EX", "https://www.v2ex.com/?q={query}").unwrap();
        assert_ne!(id, second, "id 必须唯一，否则开关和删除会作用到别的引擎");
        let added = config
            .web_search
            .engines()
            .into_iter()
            .find(|e| e.id == id)
            .unwrap();
        assert_eq!(
            (added.name.as_str(), added.enabled, added.builtin),
            ("知乎", true, false)
        );
        assert_eq!(added.url, "https://www.zhihu.com/search?q={query}");

        // 编辑同样要校验，校验失败时保持原样。
        assert_eq!(
            update_custom_engine(&mut config, &id, "知乎", "https://www.zhihu.com/"),
            Err(EngineError::MissingQuery)
        );
        assert_eq!(
            update_custom_engine(
                &mut config,
                &id,
                "知乎问答",
                "https://zhihu.com/s?q={query}"
            ),
            Ok(true)
        );
        assert_eq!(config.web_search.custom[0].name, "知乎问答");
        assert_eq!(
            update_custom_engine(&mut config, "google", "G", "https://g/?q={query}"),
            Ok(false),
            "内置引擎不能改名改网址"
        );

        // 内置引擎删不掉，只能关；自定义引擎删除后连同排序一起消失。
        assert!(!remove_custom_engine(&mut config, "google"));
        assert!(config.web_search.engines().iter().any(|e| e.id == "google"));
        assert!(move_web_search(&mut config, &id, true));
        assert!(remove_custom_engine(&mut config, &id));
        assert!(!config.web_search.order.contains(&id));
        assert!(config.web_search.engines().iter().all(|e| e.id != id));
    }

    #[test]
    fn cleared_extension_hotkey_is_not_triggered_anymore() {
        // 用户在设置里清除了剪贴板历史的快捷键：之后按 ⇧⌘V 不能再打开它。
        // 写成 None 会悄悄退回默认的 ⇧⌘V，等于没清除。
        use crate::hotkey::{Command, wanted};
        let clipboard = Command::Scope("clipboard");
        let spec_of = |config: &Config| {
            wanted(config)
                .into_iter()
                .find(|want| want.command == clipboard)
                .and_then(|want| want.spec)
        };

        let mut config = Config::default();
        set_extension_hotkey(&mut config, "clipboard", Some("ctrl-alt-v".into()));
        clear_extension_hotkey(&mut config, "clipboard");
        assert_eq!(spec_of(&config), None);

        // 恢复默认则回到出厂组合。
        reset_extension_hotkey(&mut config, "clipboard");
        assert_eq!(spec_of(&config).as_deref(), Some("cmd-shift-v"));
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
    fn file_search_lists_round_trip() {
        // 范围和排除目录在一个输入框里用逗号分隔；中文逗号、多余空格不能变成奇怪的目录名。
        let list = parse_list(" ~ ,~/Work，node_modules,, target ");
        assert_eq!(list, ["~", "~/Work", "node_modules", "target"]);
        assert_eq!(format_list(&list), "~, ~/Work, node_modules, target");
        assert!(parse_list("  ").is_empty());
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
            "system",
            "calculator",
            "websearch",
            "files",
            "settings-panes",
            "toggles",
            "bookmarks",
            "dictionary",
            "time",
            "devtools",
            "emoji",
            "color",
        ] {
            assert!(!extension_meta(id).name.is_empty(), "{id}");
        }
        // 两个插件同名，用户分不清该开关哪一个。
        assert_ne!(
            extension_meta("builtin").name,
            extension_meta("system").name
        );
        assert_eq!(extension_meta("scripts").kind, "脚本");
    }
}
