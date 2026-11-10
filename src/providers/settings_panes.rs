//! 系统设置面板：每个面板一条结果（Wi‑Fi、蓝牙、键盘、隐私与安全性下的各项权限……），
//! 回车用 `x-apple.systempreferences:<扩展 id>[?锚点]` 直接打开那一页。
//!
//! - 面板 id 取自 macOS 27 的 `/System/Library/ExtensionKit/Extensions/*.appex`
//!   （`EXExtensionPointIdentifier == com.apple.Settings.extension.ui` 且
//!   `allowsXAppleSystemPreferencesURLScheme` 为真），macOS 13 起这套 id 不变；
//!   单测 `panes_exist_on_this_system` 在 macOS 27 上逐个核对本机确有这个扩展。
//! - 隐私子页的锚点（`Privacy_Accessibility` 等）取自隐私扩展二进制里的字符串；
//!   `Privacy_AllFiles`（完全磁盘访问）、`Privacy_ListenEvent`（输入监控）不在字符串里（可能是拼出来的），
//!   沿用 macOS 13–15 的通行写法，27 上未逐个点开实测。
//! - 匹配看中英关键词，中文名自带拼音 / 首字母（`ly` → 蓝牙）。

use std::path::PathBuf;

use crate::i18n::tr;
use crate::matcher;
use crate::provider::{Action, Icon, Item, NamedAction, Provider};

/// 一个面板：界面文字存 (中文, 英文)，匹配只看 `keywords`（含中英名）。
pub struct Pane {
    pub key: &'static str,
    pub title: (&'static str, &'static str),
    /// 所在位置（隐私子页写「隐私与安全性」），None = 顶层面板。
    pub parent: Option<(&'static str, &'static str)>,
    pub keywords: &'static [&'static str],
    /// 设置扩展的 bundle id。
    pub extension: &'static str,
    /// URL 锚点（`?` 之后的部分）。
    pub anchor: Option<&'static str>,
}

impl Pane {
    pub fn url(&self) -> String {
        match self.anchor {
            Some(anchor) => format!("x-apple.systempreferences:{}?{anchor}", self.extension),
            None => format!("x-apple.systempreferences:{}", self.extension),
        }
    }
}

const PRIVACY: &str = "com.apple.settings.PrivacySecurity.extension";
const PRIVACY_NAME: (&str, &str) = ("隐私与安全性", "Privacy & Security");
const GENERAL_NAME: (&str, &str) = ("通用", "General");

const fn pane(
    key: &'static str,
    title: (&'static str, &'static str),
    keywords: &'static [&'static str],
    extension: &'static str,
) -> Pane {
    Pane {
        key,
        title,
        parent: None,
        keywords,
        extension,
        anchor: None,
    }
}

/// 「通用」下的子面板：系统设置里点进「通用」才看得到，所以副标题写出路径。
const fn general(
    key: &'static str,
    title: (&'static str, &'static str),
    keywords: &'static [&'static str],
    extension: &'static str,
) -> Pane {
    Pane {
        parent: Some(GENERAL_NAME),
        ..pane(key, title, keywords, extension)
    }
}

const fn privacy(
    key: &'static str,
    title: (&'static str, &'static str),
    keywords: &'static [&'static str],
    anchor: &'static str,
) -> Pane {
    Pane {
        key,
        title,
        parent: Some(PRIVACY_NAME),
        keywords,
        extension: PRIVACY,
        anchor: Some(anchor),
    }
}

pub static PANES: &[Pane] = &[
    pane(
        "wifi",
        ("Wi‑Fi", "Wi‑Fi"),
        &["wifi", "wi-fi", "无线", "无线局域网", "wlan"],
        "com.apple.wifi-settings-extension",
    ),
    pane(
        "bluetooth",
        ("蓝牙", "Bluetooth"),
        &["蓝牙", "bluetooth"],
        "com.apple.BluetoothSettings",
    ),
    pane(
        "network",
        ("网络", "Network"),
        &[
            "网络",
            "network",
            "以太网",
            "ethernet",
            "代理",
            "proxy",
            "dns",
        ],
        "com.apple.Network-Settings.extension",
    ),
    pane(
        "vpn",
        ("VPN", "VPN"),
        &["vpn"],
        "com.apple.NetworkExtensionSettingsUI.NESettingsUIExtension",
    ),
    pane(
        "notifications",
        ("通知", "Notifications"),
        &["通知", "notifications"],
        "com.apple.Notifications-Settings.extension",
    ),
    pane(
        "sound",
        ("声音", "Sound"),
        &["声音", "音量", "sound", "audio", "volume"],
        "com.apple.Sound-Settings.extension",
    ),
    pane(
        "focus",
        ("专注模式", "Focus"),
        &["专注模式", "勿扰", "focus", "do not disturb"],
        "com.apple.Focus-Settings.extension",
    ),
    pane(
        "screen-time",
        ("屏幕使用时间", "Screen Time"),
        &["屏幕使用时间", "screen time"],
        "com.apple.Screen-Time-Settings.extension",
    ),
    pane(
        "general",
        GENERAL_NAME,
        &["通用", "general"],
        "com.apple.systempreferences.GeneralSettings",
    ),
    general(
        "about",
        ("关于本机", "About"),
        &[
            "关于本机",
            "关于",
            "about",
            "about this mac",
            "序列号",
            "serial number",
        ],
        "com.apple.SystemProfiler.AboutExtension",
    ),
    general(
        "software-update",
        ("软件更新", "Software Update"),
        &["软件更新", "系统更新", "software update", "update"],
        "com.apple.Software-Update-Settings.extension",
    ),
    general(
        "storage",
        ("储存空间", "Storage"),
        &["储存空间", "存储空间", "磁盘空间", "storage", "disk space"],
        "com.apple.settings.Storage",
    ),
    general(
        "airdrop",
        ("隔空投送与接力", "AirDrop & Handoff"),
        &["隔空投送", "接力", "airdrop", "handoff"],
        "com.apple.AirDrop-Handoff-Settings.extension",
    ),
    general(
        "login-items",
        ("登录项与扩展", "Login Items & Extensions"),
        &["登录项", "开机启动", "扩展", "login items", "extensions"],
        "com.apple.LoginItems-Settings.extension",
    ),
    general(
        "language",
        ("语言与地区", "Language & Region"),
        &["语言与地区", "语言", "地区", "language", "region"],
        "com.apple.Localization-Settings.extension",
    ),
    general(
        "date-time",
        ("日期与时间", "Date & Time"),
        &["日期与时间", "时间", "时区", "date & time", "time zone"],
        "com.apple.Date-Time-Settings.extension",
    ),
    general(
        "sharing",
        ("共享", "Sharing"),
        &["共享", "文件共享", "远程登录", "sharing", "remote login"],
        "com.apple.Sharing-Settings.extension",
    ),
    general(
        "time-machine",
        ("时间机器", "Time Machine"),
        &["时间机器", "备份", "time machine", "backup"],
        "com.apple.Time-Machine-Settings.extension",
    ),
    general(
        "transfer-reset",
        ("传输或还原", "Transfer or Reset"),
        &["传输或还原", "抹掉", "transfer or reset", "erase"],
        "com.apple.Transfer-Reset-Settings.extension",
    ),
    general(
        "startup-disk",
        ("启动磁盘", "Startup Disk"),
        &["启动磁盘", "startup disk"],
        "com.apple.Startup-Disk-Settings.extension",
    ),
    general(
        "profiles",
        ("设备管理", "Device Management"),
        &["设备管理", "描述文件", "device management", "profiles"],
        "com.apple.Profiles-Settings.extension",
    ),
    pane(
        "appearance",
        ("外观", "Appearance"),
        &["外观", "深色", "浅色", "appearance", "dark", "light"],
        "com.apple.Appearance-Settings.extension",
    ),
    pane(
        "accessibility",
        ("辅助功能", "Accessibility"),
        &["辅助功能", "无障碍", "accessibility"],
        "com.apple.Accessibility-Settings.extension",
    ),
    pane(
        "control-center",
        ("控制中心", "Control Center"),
        &["控制中心", "菜单栏", "control center", "menu bar"],
        "com.apple.ControlCenter-Settings.extension",
    ),
    pane(
        "siri",
        ("Apple 智能与 Siri", "Apple Intelligence & Siri"),
        &["siri", "apple 智能", "apple intelligence", "听写"],
        "com.apple.Siri-Settings.extension",
    ),
    pane(
        "spotlight",
        ("聚焦", "Spotlight"),
        &["聚焦", "spotlight"],
        "com.apple.Spotlight-Settings.extension",
    ),
    pane(
        "privacy",
        PRIVACY_NAME,
        &[
            "隐私与安全性",
            "隐私",
            "安全性",
            "privacy",
            "security",
            "filevault",
        ],
        PRIVACY,
    ),
    privacy(
        "privacy-accessibility",
        ("辅助功能权限", "Accessibility Access"),
        &["辅助功能权限", "辅助功能", "accessibility"],
        "Privacy_Accessibility",
    ),
    privacy(
        "privacy-screen-recording",
        ("屏幕与系统录音权限", "Screen & System Audio Recording"),
        &[
            "屏幕录制",
            "录屏权限",
            "屏幕录制权限",
            "screen recording",
            "screen capture",
        ],
        "Privacy_ScreenCapture",
    ),
    privacy(
        "privacy-full-disk",
        ("完全磁盘访问权限", "Full Disk Access"),
        &[
            "完全磁盘访问",
            "完全磁盘访问权限",
            "磁盘访问",
            "full disk access",
        ],
        "Privacy_AllFiles",
    ),
    privacy(
        "privacy-automation",
        ("自动化权限", "Automation"),
        &["自动化", "自动化权限", "automation", "apple events"],
        "Privacy_Automation",
    ),
    privacy(
        "privacy-input-monitoring",
        ("输入监控权限", "Input Monitoring"),
        &["输入监控", "键盘监控", "input monitoring"],
        "Privacy_ListenEvent",
    ),
    privacy(
        "privacy-microphone",
        ("麦克风权限", "Microphone"),
        &["麦克风", "麦克风权限", "microphone"],
        "Privacy_Microphone",
    ),
    privacy(
        "privacy-camera",
        ("摄像头权限", "Camera"),
        &["摄像头", "相机权限", "摄像头权限", "camera"],
        "Privacy_Camera",
    ),
    privacy(
        "privacy-location",
        ("定位服务", "Location Services"),
        &["定位服务", "定位", "位置", "location services", "location"],
        "Privacy_LocationServices",
    ),
    privacy(
        "privacy-files-folders",
        ("文件与文件夹权限", "Files & Folders"),
        &["文件与文件夹", "files & folders", "files and folders"],
        "Privacy_FilesAndFolders",
    ),
    privacy(
        "privacy-photos",
        ("照片权限", "Photos"),
        &["照片权限", "photos access"],
        "Privacy_Photos",
    ),
    privacy(
        "privacy-calendars",
        ("日历权限", "Calendars"),
        &["日历权限", "calendars access"],
        "Privacy_Calendars",
    ),
    privacy(
        "privacy-developer-tools",
        ("开发者工具权限", "Developer Tools"),
        &["开发者工具", "developer tools"],
        "Privacy_DevTools",
    ),
    pane(
        "desktop-dock",
        ("桌面与程序坞", "Desktop & Dock"),
        &[
            "桌面与程序坞",
            "程序坞",
            "台前调度",
            "调度中心",
            "desktop & dock",
            "dock",
            "stage manager",
            "mission control",
            "hot corners",
            "触发角",
        ],
        "com.apple.Desktop-Settings.extension",
    ),
    pane(
        "displays",
        ("显示器", "Displays"),
        &[
            "显示器",
            "分辨率",
            "夜览",
            "displays",
            "resolution",
            "night shift",
        ],
        "com.apple.Displays-Settings.extension",
    ),
    pane(
        "wallpaper",
        ("墙纸", "Wallpaper"),
        &["墙纸", "壁纸", "桌面背景", "wallpaper"],
        "com.apple.Wallpaper-Settings.extension",
    ),
    pane(
        "battery",
        ("电池", "Battery"),
        &["电池", "节能", "电源", "battery", "energy", "power"],
        "com.apple.Battery-Settings.extension",
    ),
    pane(
        "lock-screen",
        ("锁定屏幕", "Lock Screen"),
        &["锁定屏幕", "屏幕保护程序", "lock screen", "screen saver"],
        "com.apple.Lock-Screen-Settings.extension",
    ),
    pane(
        "touch-id",
        ("触控 ID 与密码", "Touch ID & Password"),
        &["触控 id", "指纹", "登录密码", "touch id", "password"],
        "com.apple.Touch-ID-Settings.extension",
    ),
    pane(
        "users",
        ("用户与群组", "Users & Groups"),
        &["用户与群组", "用户", "users & groups", "users"],
        "com.apple.Users-Groups-Settings.extension",
    ),
    pane(
        "passwords",
        ("密码", "Passwords"),
        &["密码", "passwords"],
        "com.apple.Passwords-Settings.extension",
    ),
    pane(
        "internet-accounts",
        ("互联网账户", "Internet Accounts"),
        &["互联网账户", "邮件账户", "internet accounts"],
        "com.apple.Internet-Accounts-Settings.extension",
    ),
    pane(
        "game-center",
        ("游戏中心", "Game Center"),
        &["游戏中心", "game center"],
        "com.apple.Game-Center-Settings.extension",
    ),
    pane(
        "wallet",
        ("钱包与 Apple Pay", "Wallet & Apple Pay"),
        &["钱包", "apple pay", "wallet"],
        "com.apple.WalletSettingsExtension",
    ),
    pane(
        "keyboard",
        ("键盘", "Keyboard"),
        &[
            "键盘",
            "输入法",
            "快捷键",
            "keyboard",
            "input sources",
            "shortcuts",
        ],
        "com.apple.Keyboard-Settings.extension",
    ),
    pane(
        "mouse",
        ("鼠标", "Mouse"),
        &["鼠标", "mouse"],
        "com.apple.Mouse-Settings.extension",
    ),
    pane(
        "trackpad",
        ("触控板", "Trackpad"),
        &["触控板", "trackpad"],
        "com.apple.Trackpad-Settings.extension",
    ),
    pane(
        "printers",
        ("打印机与扫描仪", "Printers & Scanners"),
        &["打印机与扫描仪", "打印机", "扫描仪", "printers", "scanners"],
        "com.apple.Print-Scan-Settings.extension",
    ),
    pane(
        "family",
        ("家人", "Family"),
        &["家人", "家人共享", "family", "family sharing"],
        "com.apple.Family-Settings.extension",
    ),
    pane(
        "apple-account",
        ("Apple 账户", "Apple Account"),
        &["apple 账户", "apple id", "icloud", "apple account"],
        "com.apple.systempreferences.AppleIDSettings",
    ),
];

const SYSTEM_SETTINGS_APP: &str = "/System/Applications/System Settings.app";

pub struct SettingsPanes;

fn badge() -> &'static str {
    tr("系统设置", "System Settings")
}

impl Provider for SettingsPanes {
    fn id(&self) -> &'static str {
        "settings-panes"
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        PANES
            .iter()
            .filter_map(|pane| {
                let score = pane
                    .keywords
                    .iter()
                    .chain([&pane.title.0, &pane.title.1])
                    .filter_map(|keyword| matcher::score(query, keyword))
                    .max()?;
                Some(item(pane, score))
            })
            .collect()
    }
}

fn item(pane: &Pane, score: u32) -> Item {
    let url = pane.url();
    let root = tr("系统设置", "System Settings");
    let subtitle = match pane.parent {
        Some(parent) => format!("{root} › {}", tr(parent.0, parent.1)),
        None => root.to_string(),
    };
    Item {
        id: format!("settings-panes:{}", pane.key),
        title: tr(pane.title.0, pane.title.1).into(),
        subtitle,
        badge: badge().into(),
        // 同分时让同名应用（如「系统设置」App 本身）排前面。
        score: score.saturating_sub(1),
        icon: Icon::File(PathBuf::from(SYSTEM_SETTINGS_APP)),
        action: Action::open(url.clone()),
        action_title: tr("打开设置", "Open Settings").into(),
        actions: vec![NamedAction::new(
            tr("复制链接", "Copy Link"),
            Some("⌘⇧C"),
            Action::copy(url),
        )],
        ..Item::blank()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn find(query: &str, key: &str) -> Option<Item> {
        let id = format!("settings-panes:{key}");
        SettingsPanes
            .query(query)
            .into_iter()
            .find(|item| item.id == id)
    }

    #[test]
    fn every_url_is_well_formed_and_unique() {
        // 拼错一个字符，系统设置只会打开首页、不报错，用户以为功能坏了。
        let mut keys = HashSet::new();
        let mut urls = HashSet::new();
        for pane in PANES {
            assert!(keys.insert(pane.key), "重复的 key {}", pane.key);
            let url = pane.url();
            assert!(urls.insert(url.clone()), "重复的 URL {url}");
            let rest = url.strip_prefix("x-apple.systempreferences:").unwrap();
            let (id, anchor) = rest.split_once('?').unwrap_or((rest, ""));
            assert!(
                id.split('.').count() >= 3
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-.".contains(c)),
                "{url}"
            );
            assert!(
                anchor
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "{url}"
            );
            assert_eq!(
                pane.anchor.is_some(),
                pane.extension == PRIVACY && pane.key != "privacy"
            );
            assert!(!pane.keywords.is_empty(), "{}", pane.key);
        }
    }

    /// 本机 macOS 主版本号（读 SystemVersion.plist）。
    fn macos_major() -> Option<u32> {
        let value =
            plist::Value::from_file("/System/Library/CoreServices/SystemVersion.plist").ok()?;
        let version = value.as_dictionary()?.get("ProductVersion")?.as_string()?;
        version.split('.').next()?.parse().ok()
    }

    /// 本机所有允许 `x-apple.systempreferences:` 打开的设置扩展 id。
    fn installed_panes() -> HashSet<String> {
        let dirs = [
            "/System/Library/ExtensionKit/Extensions",
            "/System/Library/CoreServices/Spotlight.app/Contents/Extensions",
            "/System/Cryptexes/App/System/Library/CoreServices",
        ];
        let mut ids = HashSet::new();
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let info = entry.path().join("Contents/Info.plist");
                let Ok(value) = plist::Value::from_file(&info) else {
                    continue;
                };
                let Some(dict) = value.as_dictionary() else {
                    continue;
                };
                let attrs = dict
                    .get("EXAppExtensionAttributes")
                    .and_then(plist::Value::as_dictionary);
                let point = attrs
                    .and_then(|a| a.get("EXExtensionPointIdentifier"))
                    .and_then(plist::Value::as_string);
                let allows = attrs
                    .and_then(|a| a.get("SettingsExtensionAttributes"))
                    .and_then(plist::Value::as_dictionary)
                    .and_then(|s| s.get("allowsXAppleSystemPreferencesURLScheme"))
                    .and_then(plist::Value::as_boolean);
                if point == Some("com.apple.Settings.extension.ui")
                    && allows == Some(true)
                    && let Some(id) = dict
                        .get("CFBundleIdentifier")
                        .and_then(plist::Value::as_string)
                {
                    ids.insert(id.to_string());
                }
            }
        }
        ids
    }

    #[test]
    fn panes_exist_on_this_system() {
        // 面板 id 每个 macOS 大版本都可能改名（13、14、26 都改过）。表是按 macOS 27 核对的：
        // 在 27 上逐个确认本机真有这个设置扩展、且允许用 URL 打开；其他版本只打印提示。
        let installed = installed_panes();
        let missing: Vec<_> = PANES
            .iter()
            .map(|pane| pane.extension)
            .filter(|id| !installed.contains(*id))
            .collect();
        match macos_major() {
            Some(27) => assert!(missing.is_empty(), "本机没有这些设置扩展：{missing:?}"),
            other => {
                eprintln!("macOS {other:?}：面板表按 27 核对，本机缺少 {missing:?}（不判失败）")
            }
        }
    }

    #[test]
    fn found_by_chinese_english_and_pinyin() {
        // 中文用户打拼音或首字母，英文用户打英文，都要找得到。
        for (query, key) in [
            ("蓝牙", "bluetooth"),
            ("ly", "bluetooth"),
            ("lanya", "bluetooth"),
            ("bluetooth", "bluetooth"),
            ("键盘", "keyboard"),
            ("jp", "keyboard"),
            ("keyboard", "keyboard"),
            ("录屏权限", "privacy-screen-recording"),
            ("screen recording", "privacy-screen-recording"),
            ("wqcpfw", "privacy-full-disk"),
            ("full disk", "privacy-full-disk"),
            ("wifi", "wifi"),
            ("台前调度", "desktop-dock"),
        ] {
            assert!(find(query, key).is_some(), "{query:?} 应找到 {key}");
        }
    }

    #[test]
    fn opens_the_pane_url_and_shows_where_it_lives() {
        // 回车直接打开那一页；隐私子页副标题要写出路径，用户才知道去哪儿找。
        let item = find("辅助功能权限", "privacy-accessibility").unwrap();
        assert_eq!(item.subtitle, "系统设置 › 隐私与安全性");
        assert_eq!(item.actions[0].title, "复制链接");
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let item = find("bluetooth", "bluetooth").unwrap();
            assert_eq!(item.title, "Bluetooth");
            assert_eq!(item.subtitle, "System Settings");
        });
    }

    #[test]
    fn nothing_on_the_home_page() {
        assert!(SettingsPanes.query("").is_empty());
    }
}
