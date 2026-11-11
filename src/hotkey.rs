//! 全局快捷键：`global-hotkey`（Carbon `RegisterEventHotKey`，免辅助功能权限）。
//!
//! - 唤起面板：`config.general.hotkey`（默认 ⌥Space）。
//! - 范围快捷键：`config.extensions[<slot>].hotkey`，未配置用 [`EXTENSION_SHORTCUTS`] 的默认值，
//!   配置成空字符串表示不绑定；扩展被停用时也不绑定。`slot` 一般就是插件 id；一个插件有第二条快捷键时
//!   另起一个名字（截图：`screenshot` 是「截图识别文字」，`screenshot-capture` 是「截图」）。
//! - 配置变化（设置窗口改快捷键）时增量重新注册：新组合注册失败就保留旧的，
//!   原因记成 [`Issue`]，设置窗口在对应行显示（同时写 stderr）。
//! - 录制前的冲突判定（Helmp 自己的命令 / 系统快捷键 / 保留组合 / 其他 App）见 [`conflict`]。
//!
//! 事件在 Carbon 回调线程产生，这里用前台任务定时轮询 receiver，把命令投回 GPUI 主线程
//! （Loungy 的做法）。菜单栏、面板通过 [`active`] / [`hint`] 读取真实生效的快捷键来显示。

pub mod conflict;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::*;

use crate::config::Config;
use crate::i18n::{tr, trf};
use crate::launcher::{Launcher, LauncherHandle};

/// 唤起面板的默认快捷键，与 `config::General::default()` 一致。
pub const DEFAULT_TOGGLE: &str = "alt-space";

const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// 注册失败的绑定多少个轮询周期重试一次（50 ms × 60 = 3 秒）。
const RETRY_EVERY: u32 = 60;

/// 快捷键（或菜单项）触发的命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Command {
    /// 唤起 / 收起面板。
    Toggle,
    /// 唤起面板并进入某个插件范围；已经在这个范围里时收起。
    Scope(&'static str),
    /// 不开面板，直接执行某一项（如「截图识别文字」）。用 `engine.find(id, query)` 取项。
    RunItem {
        id: &'static str,
        query: &'static str,
    },
}

/// 各插件的快捷键默认值。
///
/// 设计稿上翻译是 ⌘⇧T、Agent 是 ⌥A：前者会抢浏览器「重新打开标签页」，后者会抢输入字符 å，
/// 所以默认改成 ⌥⇧T 和 ⌃⌥A。
pub struct ExtensionShortcut {
    /// 所属插件：插件停用时这条快捷键也不注册。
    pub provider: &'static str,
    /// 配置里存这条快捷键的键（`config.extensions[slot].hotkey`），也是设置、自检里认它的名字。
    pub slot: &'static str,
    pub label: &'static str,
    pub label_en: &'static str,
    pub default: &'static str,
    pub command: Command,
}

pub const EXTENSION_SHORTCUTS: [ExtensionShortcut; 5] = [
    ExtensionShortcut {
        provider: "clipboard",
        slot: "clipboard",
        label: "剪贴板历史",
        label_en: "Clipboard History",
        default: "cmd-shift-v",
        command: Command::Scope("clipboard"),
    },
    // 「截图」：冻结屏幕后框选。⇧⌘1 不和系统的 ⇧⌘3 / 4 / 5 截图冲突，与「截图识别文字」⇧⌘2 挨着好记。
    ExtensionShortcut {
        provider: "screenshot",
        slot: "screenshot-capture",
        label: "截图",
        label_en: "Screenshot",
        default: "cmd-shift-1",
        command: Command::RunItem {
            id: "screenshot:clipboard",
            query: "截图",
        },
    },
    ExtensionShortcut {
        provider: "screenshot",
        slot: "screenshot",
        label: "截图识别文字",
        label_en: "Recognize Text in Screenshot",
        default: "cmd-shift-2",
        command: Command::RunItem {
            id: "screenshot:ocr-screen",
            query: "截图识别文字",
        },
    },
    ExtensionShortcut {
        provider: "translate",
        slot: "translate",
        label: "翻译选中文字",
        label_en: "Translate Selection",
        default: "alt-shift-t",
        command: Command::Scope("translate"),
    },
    ExtensionShortcut {
        provider: "agents",
        slot: "agents",
        label: "Agent 会话",
        label_en: "Agent Sessions",
        default: "ctrl-alt-a",
        command: Command::Scope("agents"),
    },
];

// ======================= 字符串 ⇄ HotKey =======================

/// (配置里的写法, 物理键, 显示符号, NSMenuItem 的 keyEquivalent 字符, Carbon 虚拟键码 kVK_*)。
///
/// 虚拟键码用来对照系统快捷键（`com.apple.symbolichotkeys.plist` 里存的就是它）和探测 Carbon 注册。
const KEYS: &[(&str, Code, &str, char, u16)] = &[
    ("a", Code::KeyA, "A", 'a', 0x00),
    ("b", Code::KeyB, "B", 'b', 0x0B),
    ("c", Code::KeyC, "C", 'c', 0x08),
    ("d", Code::KeyD, "D", 'd', 0x02),
    ("e", Code::KeyE, "E", 'e', 0x0E),
    ("f", Code::KeyF, "F", 'f', 0x03),
    ("g", Code::KeyG, "G", 'g', 0x05),
    ("h", Code::KeyH, "H", 'h', 0x04),
    ("i", Code::KeyI, "I", 'i', 0x22),
    ("j", Code::KeyJ, "J", 'j', 0x26),
    ("k", Code::KeyK, "K", 'k', 0x28),
    ("l", Code::KeyL, "L", 'l', 0x25),
    ("m", Code::KeyM, "M", 'm', 0x2E),
    ("n", Code::KeyN, "N", 'n', 0x2D),
    ("o", Code::KeyO, "O", 'o', 0x1F),
    ("p", Code::KeyP, "P", 'p', 0x23),
    ("q", Code::KeyQ, "Q", 'q', 0x0C),
    ("r", Code::KeyR, "R", 'r', 0x0F),
    ("s", Code::KeyS, "S", 's', 0x01),
    ("t", Code::KeyT, "T", 't', 0x11),
    ("u", Code::KeyU, "U", 'u', 0x20),
    ("v", Code::KeyV, "V", 'v', 0x09),
    ("w", Code::KeyW, "W", 'w', 0x0D),
    ("x", Code::KeyX, "X", 'x', 0x07),
    ("y", Code::KeyY, "Y", 'y', 0x10),
    ("z", Code::KeyZ, "Z", 'z', 0x06),
    ("0", Code::Digit0, "0", '0', 0x1D),
    ("1", Code::Digit1, "1", '1', 0x12),
    ("2", Code::Digit2, "2", '2', 0x13),
    ("3", Code::Digit3, "3", '3', 0x14),
    ("4", Code::Digit4, "4", '4', 0x15),
    ("5", Code::Digit5, "5", '5', 0x17),
    ("6", Code::Digit6, "6", '6', 0x16),
    ("7", Code::Digit7, "7", '7', 0x1A),
    ("8", Code::Digit8, "8", '8', 0x1C),
    ("9", Code::Digit9, "9", '9', 0x19),
    ("-", Code::Minus, "-", '-', 0x1B),
    ("=", Code::Equal, "=", '=', 0x18),
    ("[", Code::BracketLeft, "[", '[', 0x21),
    ("]", Code::BracketRight, "]", ']', 0x1E),
    ("\\", Code::Backslash, "\\", '\\', 0x2A),
    (";", Code::Semicolon, ";", ';', 0x29),
    ("'", Code::Quote, "'", '\'', 0x27),
    (",", Code::Comma, ",", ',', 0x2B),
    (".", Code::Period, ".", '.', 0x2F),
    ("/", Code::Slash, "/", '/', 0x2C),
    ("`", Code::Backquote, "`", '`', 0x32),
    ("space", Code::Space, "Space", ' ', 0x31),
    ("enter", Code::Enter, "↵", '\r', 0x24),
    ("tab", Code::Tab, "⇥", '\t', 0x30),
    ("escape", Code::Escape, "⎋", '\u{1b}', 0x35),
    ("backspace", Code::Backspace, "⌫", '\u{8}', 0x33),
    ("delete", Code::Delete, "⌦", '\u{f728}', 0x75),
    ("up", Code::ArrowUp, "↑", '\u{f700}', 0x7E),
    ("down", Code::ArrowDown, "↓", '\u{f701}', 0x7D),
    ("left", Code::ArrowLeft, "←", '\u{f702}', 0x7B),
    ("right", Code::ArrowRight, "→", '\u{f703}', 0x7C),
    ("home", Code::Home, "↖", '\u{f729}', 0x73),
    ("end", Code::End, "↘", '\u{f72b}', 0x77),
    ("pageup", Code::PageUp, "⇞", '\u{f72c}', 0x74),
    ("pagedown", Code::PageDown, "⇟", '\u{f72d}', 0x79),
    ("f1", Code::F1, "F1", '\u{f704}', 0x7A),
    ("f2", Code::F2, "F2", '\u{f705}', 0x78),
    ("f3", Code::F3, "F3", '\u{f706}', 0x63),
    ("f4", Code::F4, "F4", '\u{f707}', 0x76),
    ("f5", Code::F5, "F5", '\u{f708}', 0x60),
    ("f6", Code::F6, "F6", '\u{f709}', 0x61),
    ("f7", Code::F7, "F7", '\u{f70a}', 0x62),
    ("f8", Code::F8, "F8", '\u{f70b}', 0x64),
    ("f9", Code::F9, "F9", '\u{f70c}', 0x65),
    ("f10", Code::F10, "F10", '\u{f70d}', 0x6D),
    ("f11", Code::F11, "F11", '\u{f70e}', 0x67),
    ("f12", Code::F12, "F12", '\u{f70f}', 0x6F),
    ("f13", Code::F13, "F13", '\u{f710}', 0x69),
    ("f14", Code::F14, "F14", '\u{f711}', 0x6B),
    ("f15", Code::F15, "F15", '\u{f712}', 0x71),
    ("f16", Code::F16, "F16", '\u{f713}', 0x6A),
    ("f17", Code::F17, "F17", '\u{f714}', 0x40),
    ("f18", Code::F18, "F18", '\u{f715}', 0x4F),
    ("f19", Code::F19, "F19", '\u{f716}', 0x50),
    ("f20", Code::F20, "F20", '\u{f717}', 0x5A),
];

/// 修饰键的 macOS 显示顺序 ⌃⌥⇧⌘。
const MODIFIER_GLYPHS: [(Modifiers, &str); 4] = [
    (Modifiers::CONTROL, "⌃"),
    (Modifiers::ALT, "⌥"),
    (Modifiers::SHIFT, "⇧"),
    (Modifiers::META, "⌘"),
];

/// 解析配置里的快捷键（GPUI keystroke 写法：`alt-space`、`cmd-shift-v`、`cmd--`）。
pub fn parse(spec: &str) -> Result<HotKey> {
    let spec = spec.trim().to_lowercase();
    if spec.is_empty() {
        bail!("快捷键为空");
    }
    // 键本身是「-」时写作 `cmd--`，不能简单按「-」切分。
    let (mods_part, key) = if spec == "-" {
        ("", "-")
    } else if let Some(prefix) = spec.strip_suffix("--") {
        (prefix, "-")
    } else {
        match spec.rsplit_once('-') {
            Some((mods, key)) => (mods, key),
            None => ("", spec.as_str()),
        }
    };
    let mut mods = Modifiers::empty();
    for token in mods_part.split('-').filter(|token| !token.is_empty()) {
        let modifier = match token {
            "cmd" | "command" | "super" | "meta" => Modifiers::META,
            "ctrl" | "control" => Modifiers::CONTROL,
            "alt" | "option" | "opt" => Modifiers::ALT,
            "shift" => Modifiers::SHIFT,
            other => bail!("「{spec}」里的「{other}」不是修饰键（可用 cmd、ctrl、alt、shift）"),
        };
        if mods.contains(modifier) {
            bail!("「{spec}」里重复写了修饰键「{token}」");
        }
        mods |= modifier;
    }
    let code = KEYS
        .iter()
        .find(|(token, ..)| *token == key)
        .map(|(_, code, ..)| *code)
        .with_context(|| format!("「{spec}」里的「{key}」不是支持的按键"))?;
    if mods.is_empty() && !is_function_key(code) {
        // 不带修饰键的全局快捷键会吞掉所有 App 里的这个键，等于把键盘弄坏。
        // F1–F20 例外：它们不产生字符，单独做快捷键很常见。
        bail!("「{spec}」没有修饰键，全局快捷键至少要带一个 cmd / ctrl / alt / shift");
    }
    Ok(HotKey::new(Some(mods), code))
}

/// F1–F20：不产生字符，可以不带修饰键单独做全局快捷键。
pub fn is_function_key(code: Code) -> bool {
    KEYS.iter()
        .find(|(_, c, ..)| *c == code)
        .is_some_and(|(token, ..)| {
            token
                .strip_prefix('f')
                .and_then(|n| n.parse::<u8>().ok())
                .is_some_and(|n| (1..=20).contains(&n))
        })
}

/// Carbon 虚拟键码（kVK_*）。表外的键返回 None。
pub fn carbon_keycode(code: Code) -> Option<u16> {
    KEYS.iter()
        .find(|(_, c, ..)| *c == code)
        .map(|(.., vk)| *vk)
}

/// Carbon 虚拟键码 → 物理键，[`carbon_keycode`] 的逆运算。
pub fn code_from_carbon(vk: u16) -> Option<Code> {
    KEYS.iter()
        .find(|(.., v)| *v == vk)
        .map(|(_, code, ..)| *code)
}

/// `HotKey` → 配置写法，[`parse`] 的逆运算。表外的键返回 None。
/// 设置窗口录制快捷键后写回配置用。
pub fn to_spec(hotkey: &HotKey) -> Option<String> {
    let (token, ..) = KEYS.iter().find(|(_, code, ..)| *code == hotkey.key)?;
    // 与 GPUI `Keystroke::unparse` 的顺序一致：ctrl-alt-cmd-shift，设置窗口写回去的字符串不会和它打架。
    let mut parts: Vec<&str> = [
        (Modifiers::CONTROL, "ctrl"),
        (Modifiers::ALT, "alt"),
        (Modifiers::META, "cmd"),
        (Modifiers::SHIFT, "shift"),
    ]
    .iter()
    .filter(|(modifier, _)| hotkey.mods.contains(*modifier))
    .map(|(_, name)| *name)
    .collect();
    parts.push(token);
    Some(parts.join("-"))
}

/// 显示用的符号串，如 `⌥Space`、`⌘⇧V`（可直接交给 `provider::keycaps` 拆键帽）。
pub fn glyphs(hotkey: &HotKey) -> String {
    let mut out: String = MODIFIER_GLYPHS
        .iter()
        .filter(|(modifier, _)| hotkey.mods.contains(*modifier))
        .map(|(_, glyph)| *glyph)
        .collect();
    match KEYS.iter().find(|(_, code, ..)| *code == hotkey.key) {
        Some((_, _, glyph, ..)) => out.push_str(glyph),
        None => out.push_str(&hotkey.key.to_string()),
    }
    out
}

/// NSMenuItem 的 keyEquivalent 字符（菜单右侧显示快捷键用）。
pub fn key_equivalent(hotkey: &HotKey) -> Option<char> {
    KEYS.iter()
        .find(|(_, code, ..)| *code == hotkey.key)
        .map(|(_, _, _, ch, _)| *ch)
}

// ======================= 期望的绑定 =======================

/// 一条期望的绑定。`spec = None` 表示不绑定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub command: Command,
    pub label: &'static str,
    pub spec: Option<String>,
}

/// 每条命令分到的组合，不管扩展是否启用。顺序即优先级：组合冲突时排在前面的保留。
///
/// 录制时按它查冲突：停用的扩展仍然占着自己的组合，否则重新启用时会和别的命令撞上。
pub fn assigned(config: &Config) -> Vec<Wanted> {
    let mut out = vec![Wanted {
        command: Command::Toggle,
        label: "唤起 Helmp",
        spec: Some(config.general.hotkey.clone()),
    }];
    for shortcut in &EXTENSION_SHORTCUTS {
        let ext = config.extensions.get(shortcut.slot);
        let spec = match ext.and_then(|ext| ext.hotkey.as_deref()) {
            // 用户清除了快捷键（写成空字符串）：不绑定。
            Some(spec) if spec.trim().is_empty() => None,
            Some(spec) => Some(spec.to_string()),
            // 没配置过：用默认值。
            None => Some(shortcut.default.to_string()),
        };
        out.push(Wanted {
            command: shortcut.command,
            label: shortcut.label,
            spec,
        });
    }
    out
}

/// 按配置算出所有命令真正要注册的绑定：[`assigned`] 去掉停用的扩展。
pub fn wanted(config: &Config) -> Vec<Wanted> {
    let mut out = assigned(config);
    for want in &mut out {
        let disabled = EXTENSION_SHORTCUTS
            .iter()
            .find(|shortcut| shortcut.command == want.command)
            .and_then(|shortcut| config.extensions.get(shortcut.provider))
            .is_some_and(|ext| !ext.enabled);
        if disabled {
            want.spec = None;
        }
    }
    out
}

/// 某条命令在配置里分到的组合（设置窗口显示用）。None = 未设置。
pub fn assigned_spec(config: &Config, command: Command) -> Option<String> {
    assigned(config)
        .into_iter()
        .find(|want| want.command == command)
        .and_then(|want| want.spec)
        .filter(|spec| !spec.trim().is_empty())
}

/// 某条命令的出厂默认组合。
pub fn default_spec(command: Command) -> Option<&'static str> {
    match command {
        Command::Toggle => Some(DEFAULT_TOGGLE),
        _ => EXTENSION_SHORTCUTS
            .iter()
            .find(|shortcut| shortcut.command == command)
            .map(|shortcut| shortcut.default),
    }
}

/// 某个快捷键位的命令（扩展表格里哪些行能录快捷键；插件 id 就是它的主快捷键位）。
pub fn extension_command(slot: &str) -> Option<Command> {
    EXTENSION_SHORTCUTS
        .iter()
        .find(|shortcut| shortcut.slot == slot)
        .map(|shortcut| shortcut.command)
}

/// 命令的显示名（按当前语言），冲突提示里指名用。
pub fn command_label(command: Command) -> &'static str {
    match command {
        Command::Toggle => tr("唤起 Helmp", "Open Helmp"),
        _ => EXTENSION_SHORTCUTS
            .iter()
            .find(|shortcut| shortcut.command == command)
            .map_or(tr("Helmp 命令", "a Helmp command"), |shortcut| {
                tr(shortcut.label, shortcut.label_en)
            }),
    }
}

// ======================= 注册表 =======================

/// 能注册 / 注销热键的东西。抽出来是为了在测试里模拟「被系统占用」的组合。
pub trait Registrar {
    fn register(&self, hotkey: HotKey) -> Result<()>;
    fn unregister(&self, hotkey: HotKey) -> Result<()>;
}

impl Registrar for GlobalHotKeyManager {
    fn register(&self, hotkey: HotKey) -> Result<()> {
        GlobalHotKeyManager::register(self, hotkey).map_err(anyhow::Error::from)
    }

    fn unregister(&self, hotkey: HotKey) -> Result<()> {
        GlobalHotKeyManager::unregister(self, hotkey).map_err(anyhow::Error::from)
    }
}

/// 某条命令没能按配置生效的原因。设置窗口在对应行显示，文案按当前语言现取。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Issue {
    /// 配置里的写法解析不了（多半是手改 config.json 改坏了）。
    Invalid { spec: String },
    /// 和优先级更高的命令要了同一个组合。
    Duplicate { wanted: HotKey, owner: Command },
    /// 向系统注册失败（被系统或其他 App 占用）。`kept` 是退回去继续生效的旧组合。
    Unavailable {
        wanted: HotKey,
        kept: Option<HotKey>,
    },
}

impl Issue {
    pub fn message(&self) -> String {
        match self {
            Issue::Invalid { spec } => trf!(
                "配置里的快捷键「{spec}」无效，未生效",
                "The shortcut “{spec}” in the config file is invalid and isn’t active"
            ),
            Issue::Duplicate { wanted, owner } => trf!(
                "{} 已被「{}」使用，未生效",
                "{} is already used by “{}” and isn’t active",
                glyphs(wanted),
                command_label(*owner)
            ),
            Issue::Unavailable {
                wanted,
                kept: Some(kept),
            } => trf!(
                "{} {}。在那之前仍用 {}",
                "{}: {}. Until then, {} still works",
                glyphs(wanted),
                conflict::other_app_message(),
                glyphs(kept)
            ),
            Issue::Unavailable { wanted, kept: None } => trf!(
                "{} {}。在那之前没有生效的快捷键",
                "{}: {}. Until then, no shortcut is active",
                glyphs(wanted),
                conflict::other_app_message()
            ),
        }
    }
}

/// 当前真实注册着的绑定，以及没能按配置生效的命令。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Registry {
    bound: BTreeMap<Command, HotKey>,
    issues: BTreeMap<Command, Issue>,
}

impl Registry {
    pub fn get(&self, command: Command) -> Option<HotKey> {
        self.bound.get(&command).copied()
    }

    pub fn issue(&self, command: Command) -> Option<&Issue> {
        self.issues.get(&command)
    }

    /// 是否有绑定因注册失败（被占用）而没生效，需要定期重试。
    pub fn has_unavailable(&self) -> bool {
        self.issues
            .values()
            .any(|issue| matches!(issue, Issue::Unavailable { .. }))
    }

    pub fn command_for(&self, hotkey_id: u32) -> Option<Command> {
        self.bound
            .iter()
            .find(|(_, hotkey)| hotkey.id() == hotkey_id)
            .map(|(command, _)| *command)
    }

    /// 把注册状态调整到 `wanted`，返回需要告诉用户的问题（每条一句，写 stderr 用）；
    /// 同样的问题按命令记进 `issues`，给设置窗口显示。
    ///
    /// - 解析失败 / 注册失败：保留这条命令原来的组合。
    /// - 两条命令想要同一个组合：排在前面的拿到，后面的报冲突、保留原来的（原来的也冲突就不绑定）。
    pub fn apply(&mut self, registrar: &impl Registrar, wanted: &[Wanted]) -> Vec<String> {
        let mut problems = Vec::new();
        for want in wanted {
            self.issues.remove(&want.command);
        }

        // 1. 解析，解析失败保留旧的。
        let mut targets: Vec<(Command, &'static str, Option<HotKey>)> = wanted
            .iter()
            .map(|want| {
                let current = self.get(want.command);
                let target = match &want.spec {
                    None => None,
                    Some(spec) => match parse(spec) {
                        Ok(hotkey) => Some(hotkey),
                        Err(err) => {
                            problems.push(format!("「{}」的快捷键无效：{err:#}", want.label));
                            self.issues
                                .insert(want.command, Issue::Invalid { spec: spec.clone() });
                            current
                        }
                    },
                };
                (want.command, want.label, target)
            })
            .collect();

        // 2. 冲突：按优先级认领组合。
        let mut claimed: Vec<(HotKey, Command, &'static str)> = Vec::new();
        for i in 0..targets.len() {
            let (command, label, Some(target)) = targets[i] else {
                continue;
            };
            if let Some((_, owner, owner_label)) =
                claimed.iter().find(|(hotkey, ..)| *hotkey == target)
            {
                problems.push(format!(
                    "「{label}」的快捷键 {} 与「{owner_label}」冲突，未生效",
                    glyphs(&target)
                ));
                self.issues.insert(
                    command,
                    Issue::Duplicate {
                        wanted: target,
                        owner: *owner,
                    },
                );
                let fallback = self.get(command).filter(|current| {
                    *current != target
                        && !claimed.iter().any(|(hotkey, ..)| hotkey == current)
                        && !targets
                            .iter()
                            .any(|(other, _, t)| *other != command && *t == Some(*current))
                });
                targets[i].2 = fallback;
                if let Some(fallback) = fallback {
                    claimed.push((fallback, command, label));
                }
            } else {
                claimed.push((target, command, label));
            }
        }

        // 3. 先注销所有要变的旧组合，再注册新组合（这样两条命令互换组合也能成功）。
        let changed: Vec<_> = targets
            .iter()
            .filter(|(command, _, target)| self.get(*command) != *target)
            .copied()
            .collect();
        let mut previous = BTreeMap::new();
        for (command, label, _) in &changed {
            if let Some(old) = self.bound.remove(command) {
                if let Err(err) = registrar.unregister(old) {
                    problems.push(format!("注销「{label}」的旧快捷键失败：{err:#}"));
                }
                previous.insert(*command, old);
            }
        }
        for (command, label, target) in &changed {
            let Some(target) = target else {
                continue;
            };
            match registrar.register(*target) {
                Ok(()) => {
                    self.bound.insert(*command, *target);
                }
                Err(err) => {
                    let old = previous.get(command).copied();
                    let kept = old.filter(|old| {
                        !self.bound.values().any(|bound| bound == old)
                            && registrar.register(*old).is_ok()
                    });
                    self.issues.insert(
                        *command,
                        Issue::Unavailable {
                            wanted: *target,
                            kept,
                        },
                    );
                    match kept {
                        Some(old) => {
                            self.bound.insert(*command, old);
                            problems.push(format!(
                                "「{label}」的快捷键 {} 注册失败（可能被系统或其他 App 占用），继续使用 {}：{err:#}",
                                glyphs(target),
                                glyphs(&old)
                            ));
                        }
                        None => problems.push(format!(
                            "「{label}」的快捷键 {} 注册失败（可能被系统或其他 App 占用），目前没有快捷键：{err:#}",
                            glyphs(target)
                        )),
                    }
                }
            }
        }
        problems
    }
}

// ======================= 对外状态 =======================

static ACTIVE: Mutex<Option<Registry>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// 某条命令当前真实生效的快捷键。
pub fn active(command: Command) -> Option<HotKey> {
    ACTIVE
        .lock()
        .expect("hotkey registry poisoned")
        .as_ref()
        .and_then(|registry| registry.get(command))
}

/// 某条命令没能按配置生效的原因（设置窗口显示警示用）。
pub fn issue(command: Command) -> Option<Issue> {
    ACTIVE
        .lock()
        .expect("hotkey registry poisoned")
        .as_ref()
        .and_then(|registry| registry.issue(command).cloned())
}

/// 这个组合现在是否由 Helmp 自己注册着（探测「被其他 App 占用」前排除自己）。
pub fn is_bound_by_helmp(hotkey: HotKey) -> bool {
    ACTIVE
        .lock()
        .expect("hotkey registry poisoned")
        .as_ref()
        .is_some_and(|registry| registry.bound.values().any(|bound| *bound == hotkey))
}

/// 面板 / 首页结果右侧显示的快捷键：只有真正生效的才显示，冲突未生效、被清除、录制中都不显示。
pub fn hint(command: Command) -> Option<String> {
    hint_in(
        ACTIVE.lock().expect("hotkey registry poisoned").as_ref(),
        command,
    )
}

fn hint_in(registry: Option<&Registry>, command: Command) -> Option<String> {
    registry
        .and_then(|registry| registry.get(command))
        .map(|hotkey| glyphs(&hotkey))
}

/// 某个扩展的快捷键提示，做成结果右侧的键帽（provider 用，见 [`hint`]）。没有生效的绑定时为空。
pub fn hint_keys(provider: &str) -> Vec<crate::provider::Accessory> {
    extension_command(provider)
        .and_then(hint)
        .map(|glyphs| crate::provider::Accessory::keys(&glyphs))
        .into_iter()
        .collect()
}

/// 图形自检打印用：每个快捷键位真实生效的组合（`-` 表示没有），没生效的附上原因。
/// 形如 `toggle=⌥Space clipboard=⇧⌘V screenshot-capture=⇧⌘1 screenshot=⇧⌘2 …`。
pub fn describe_active() -> String {
    let guard = ACTIVE.lock().expect("hotkey registry poisoned");
    let registry = guard.as_ref();
    std::iter::once(("toggle", Command::Toggle))
        .chain(EXTENSION_SHORTCUTS.iter().map(|s| (s.slot, s.command)))
        .map(|(slot, command)| {
            let bound = hint_in(registry, command).unwrap_or_else(|| "-".into());
            match registry.and_then(|registry| registry.issue(command)) {
                Some(issue) => format!("{slot}={bound}({issue:?})"),
                None => format!("{slot}={bound}"),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 「直接执行某一项」的快捷键（[`Command::RunItem`]）在那一项上的键帽：截图、截图识别文字这样
/// 同一个插件有两条快捷键时，各自的命令显示各自的组合，不能都显示插件主快捷键位的那一个。
pub fn item_hint_keys(item_id: &str) -> Vec<crate::provider::Accessory> {
    item_command(item_id)
        .and_then(hint)
        .map(|glyphs| crate::provider::Accessory::keys(&glyphs))
        .into_iter()
        .collect()
}

/// 直接执行 `item_id` 这一项的快捷键命令（没有就是 None）。
pub fn item_command(item_id: &str) -> Option<Command> {
    EXTENSION_SHORTCUTS
        .iter()
        .map(|shortcut| shortcut.command)
        .find(|command| matches!(command, Command::RunItem { id, .. } if *id == item_id))
}

/// 某个插件的全部快捷键位，按 [`EXTENSION_SHORTCUTS`] 的顺序（设置里逐条列出录制框）。
pub fn shortcuts_of(provider: &str) -> impl Iterator<Item = &'static ExtensionShortcut> + '_ {
    EXTENSION_SHORTCUTS
        .iter()
        .filter(move |shortcut| shortcut.provider == provider)
}

/// 每次绑定或问题变化加一，菜单栏、设置窗口据此刷新。
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

fn publish(registry: &Registry) {
    let mut active = ACTIVE.lock().expect("hotkey registry poisoned");
    if active.as_ref() != Some(registry) {
        *active = Some(registry.clone());
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

fn report(problems: Vec<String>) {
    for problem in problems {
        eprintln!("helmp: {problem}");
    }
}

/// 注册全部快捷键并启动轮询任务。`manager` 归轮询任务所有、活到进程结束，
/// 否则 Carbon 侧的 handler 会被卸载。唤起快捷键一个都注册不上时返回错误。
pub fn register_toggle(
    cx: &mut App,
    _window: AnyWindowHandle,
    _launcher: Entity<Launcher>,
) -> Result<()> {
    let manager = GlobalHotKeyManager::new().context("GlobalHotKeyManager::new")?;
    let mut registry = Registry::default();
    let wanted_now = wanted(&crate::config::get());
    report(registry.apply(&manager, &wanted_now));

    if registry.get(Command::Toggle).is_none() {
        // 配置里的唤起快捷键用不了：退回默认值，免得整个 App 没有入口。
        let toggle_issue = registry.issue(Command::Toggle).cloned();
        let fallback: Vec<Wanted> = wanted_now
            .iter()
            .cloned()
            .map(|mut want| {
                if want.command == Command::Toggle {
                    want.spec = Some(DEFAULT_TOGGLE.into());
                }
                want
            })
            .collect();
        report(registry.apply(&manager, &fallback));
        match registry.get(Command::Toggle) {
            Some(hotkey) => eprintln!("helmp: 暂时改用默认唤起快捷键 {}", glyphs(&hotkey)),
            None => bail!("唤起快捷键注册失败（默认的 {DEFAULT_TOGGLE} 也不行）"),
        }
        // 默认值顶上了，但用户配的那个仍然没生效：设置里要继续提示，并说明现在用的是哪个。
        if let Some(mut issue) = toggle_issue {
            if let Issue::Unavailable { kept, .. } = &mut issue {
                *kept = registry.get(Command::Toggle);
            }
            registry.issues.insert(Command::Toggle, issue);
        }
    }
    publish(&registry);

    let (changed_tx, changed_rx) = mpsc::channel::<()>();
    let _ = RECHECK.set(std::sync::Mutex::new(changed_tx.clone()));
    crate::config::subscribe(move |_| {
        // 接收端在轮询任务里，进程结束前一直存在；发送失败只可能发生在退出途中。
        let _ = changed_tx.send(());
    });

    cx.spawn(async move |cx: &mut AsyncApp| {
        let receiver = GlobalHotKeyEvent::receiver();
        let mut retry_ticks = 0u32;
        loop {
            while let Ok(event) = receiver.try_recv() {
                if event.state != HotKeyState::Pressed {
                    continue;
                }
                match registry.command_for(event.id) {
                    Some(command) => {
                        crate::trace::log(format!("hotkey {command:?}"));
                        cx.update(|cx| execute(command, cx));
                    }
                    None => crate::trace::log(format!("hotkey id {} 没有对应命令", event.id)),
                }
            }
            // 有绑定因被其他应用占用而没生效：每 3 秒重试一次，对方退出或换键后自动接管。
            retry_ticks += 1;
            let retry_due = retry_ticks >= RETRY_EVERY
                && SUSPENDED.load(Ordering::Relaxed) == 0
                && registry.has_unavailable();
            if retry_due {
                retry_ticks = 0;
            }
            if changed_rx.try_recv().is_ok() || retry_due {
                while changed_rx.try_recv().is_ok() {}
                let suspended = SUSPENDED.load(Ordering::Relaxed) > 0;
                let wanted_now = wanted_while(suspended, &crate::config::get());
                // 挂起只是临时让出组合，不代表问题解决了：设置窗口里别的行的警示保持不变。
                let issues = suspended.then(|| registry.issues.clone());
                report(registry.apply(&manager, &wanted_now));
                if let Some(issues) = issues {
                    registry.issues = issues;
                }
                publish(&registry);
            }
            cx.background_executor().timer(POLL_INTERVAL).await;
        }
    })
    .detach();

    Ok(())
}

/// 挂起时每条命令都要求「不绑定」，`apply` 才会真的注销（只传空列表的话什么都不会变）。
///
/// 设置窗口录制快捷键期间注销全部全局热键：Carbon 热键会在系统层吞掉按键，
/// 不注销的话用户按下当前的唤起键（如 ⌥Space）录到的不是组合，而是面板被唤起。
fn wanted_while(suspended: bool, config: &Config) -> Vec<Wanted> {
    let mut out = wanted(config);
    if suspended {
        for want in &mut out {
            want.spec = None;
        }
    }
    out
}

/// 正在录制的录制框个数。用计数而不是开关：从一个录制框直接点到另一个时，
/// 前一个失焦（恢复）和后一个开始（挂起）的先后不固定，开关会被后到的「恢复」打开。
static SUSPENDED: AtomicUsize = AtomicUsize::new(0);
static RECHECK: std::sync::OnceLock<std::sync::Mutex<mpsc::Sender<()>>> =
    std::sync::OnceLock::new();

/// 开始录制：挂起全部全局热键，并让系统快捷键（⌘Space、⇧⌘3 等）在 Helmp 前台时暂不生效，
/// 录制框才收得到这些按键、能提示「已被系统使用」。必须和 [`resume`] 成对调用（主线程）。
pub fn suspend() {
    if SUSPENDED.fetch_add(1, Ordering::Relaxed) == 0 {
        crate::trace::log("hotkeys suspended");
        conflict::pause_system_hotkeys();
        recheck();
    }
}

/// 结束录制：恢复全局热键与系统快捷键。
pub fn resume() {
    let previous = SUSPENDED
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
        .unwrap_or(0);
    if previous == 1 {
        crate::trace::log("hotkeys resumed");
        conflict::restore_system_hotkeys();
        recheck();
    }
}

fn recheck() {
    if let Some(tx) = RECHECK.get() {
        let _ = tx.lock().expect("hotkey recheck poisoned").send(());
    }
}

// ======================= 执行命令 =======================

/// 执行一条命令。快捷键和菜单栏共用。
pub fn execute(command: Command, cx: &mut App) {
    match command {
        Command::Toggle => {
            LauncherHandle::update(cx, |launcher, window, cx| launcher.toggle(window, cx))
        }
        Command::Scope(scope) => LauncherHandle::update(cx, |launcher, window, cx| {
            if launcher.is_visible() && launcher.scope_id() == Some(scope) {
                launcher.hide(window, cx);
            } else {
                launcher.open_scope(scope, window, cx);
            }
        }),
        Command::RunItem { id, query } => {
            let Some(engine) = engine(cx) else {
                return;
            };
            cx.spawn(async move |cx: &mut AsyncApp| {
                let item = cx
                    .background_executor()
                    .spawn(async move { engine.find(id, query, None) })
                    .await;
                match item {
                    Some(item) => cx.update(|cx| run_action(item, cx)),
                    None => eprintln!("helmp: 找不到「{id}」（扩展可能被停用）"),
                }
            })
            .detach();
        }
    }
}

/// 启动器的引擎（菜单栏、快捷键查询用，结果与面板一致）。
pub fn engine(cx: &App) -> Option<crate::engine::Engine> {
    match cx.try_global::<crate::engine::EngineGlobal>() {
        Some(global) => Some(global.0.clone()),
        None => {
            eprintln!("helmp: 启动器还没创建");
            None
        }
    }
}

/// 不经过面板直接执行一个动作（后台线程），再按结果处理。
///
/// 面板没有公开「执行任意项并展示结果」的接口，所以 `Message` / `Show` 只能写到 stderr，
/// 见报告里请求的 `Launcher::run_item`。
/// 执行一个结果项（菜单栏点击、范围快捷键）：交给启动器，结果需要展示时会打开面板。
pub fn run_action(item: crate::provider::Item, cx: &mut App) {
    crate::launcher::LauncherHandle::update(cx, |launcher, window, cx| {
        launcher.run_item(item, window, cx)
    });
}

#[cfg(test)]
mod tests {
    // gpui_kit::* 里也有一个 `test` 属性宏，经 `super::*` 引入后会遮住标准库的。
    use core::prelude::v1::test;

    use std::cell::RefCell;

    use super::*;
    use crate::config::ExtensionConfig;

    #[test]
    fn parses_config_strings() {
        // 配置文件和设置窗口用 GPUI 的写法；解析错了，用户设的快捷键就注册成别的组合。
        assert_eq!(
            parse("alt-space").unwrap(),
            HotKey::new(Some(Modifiers::ALT), Code::Space)
        );
        assert_eq!(
            parse("cmd-shift-v").unwrap(),
            HotKey::new(Some(Modifiers::META | Modifiers::SHIFT), Code::KeyV)
        );
        assert_eq!(
            parse("Ctrl-Option-A").unwrap(),
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyA)
        );
        // 键本身是「-」。
        assert_eq!(
            parse("cmd--").unwrap(),
            HotKey::new(Some(Modifiers::META), Code::Minus)
        );
        assert_eq!(
            parse("cmd-shift-2").unwrap(),
            HotKey::new(Some(Modifiers::META | Modifiers::SHIFT), Code::Digit2)
        );
    }

    #[test]
    fn rejects_bad_strings_with_a_reason() {
        // 设置窗口要把原因展示给用户，不能只说「失败」。
        for bad in ["", "space", "cmd-", "hyper-a", "cmd-cmd-a", "cmd-nosuchkey"] {
            assert!(parse(bad).is_err(), "{bad:?} 不应该能解析");
        }
        let err = format!("{:#}", parse("space").unwrap_err());
        assert!(err.contains("修饰键"), "{err}");
    }

    #[test]
    fn hotkey_round_trips_to_config_string() {
        // 设置窗口录制到的 HotKey 要写回配置；写回去再读出来必须是同一个组合。
        for spec in [
            "alt-space",
            "cmd-shift-v",
            "ctrl-alt-a",
            "alt-shift-t",
            "cmd-shift-2",
            "cmd--",
            "ctrl-alt-cmd-shift-f12",
            "cmd-up",
        ] {
            let hotkey = parse(spec).unwrap();
            assert_eq!(to_spec(&hotkey).as_deref(), Some(spec));
            assert_eq!(parse(&to_spec(&hotkey).unwrap()).unwrap(), hotkey);
        }
    }

    #[test]
    fn displays_mac_glyphs_in_system_order() {
        // 菜单和面板里显示的快捷键要和系统菜单一样按 ⌃⌥⇧⌘ 排，用户才认得。
        assert_eq!(glyphs(&parse("alt-space").unwrap()), "⌥Space");
        assert_eq!(glyphs(&parse("cmd-shift-v").unwrap()), "⇧⌘V");
        assert_eq!(glyphs(&parse("cmd-alt-ctrl-a").unwrap()), "⌃⌥⌘A");
        assert_eq!(key_equivalent(&parse("cmd-shift-v").unwrap()), Some('v'));
        assert_eq!(key_equivalent(&parse("alt-space").unwrap()), Some(' '));
    }

    #[test]
    fn defaults_avoid_stealing_common_keys() {
        // ⌘⇧T 是浏览器「重新打开标签页」，⌥A 会吞掉输入字符 å；默认值绝不能是它们。
        let wanted = wanted(&Config::default());
        let specs: Vec<_> = wanted.iter().filter_map(|w| w.spec.clone()).collect();
        assert_eq!(
            specs,
            [
                "alt-space",
                "cmd-shift-v",
                "cmd-shift-1",
                "cmd-shift-2",
                "alt-shift-t",
                "ctrl-alt-a"
            ]
        );
        for spec in &specs {
            let hotkey = parse(spec).unwrap();
            assert_ne!(hotkey, parse("cmd-shift-t").unwrap());
            assert_ne!(hotkey, parse("alt-a").unwrap());
        }
    }

    #[test]
    fn disabled_or_cleared_extensions_are_not_bound() {
        // 停用的扩展还占着全局组合，会让用户在别的 App 里按不出这个键。
        let mut config = Config::default();
        config.extensions.insert(
            "clipboard".into(),
            ExtensionConfig {
                enabled: false,
                ..ExtensionConfig::default()
            },
        );
        config.extensions.insert(
            "translate".into(),
            ExtensionConfig {
                hotkey: Some(String::new()),
                ..ExtensionConfig::default()
            },
        );
        config.extensions.insert(
            "agents".into(),
            ExtensionConfig {
                hotkey: Some("cmd-alt-a".into()),
                ..ExtensionConfig::default()
            },
        );
        let wanted = wanted(&config);
        let spec = |command| {
            wanted
                .iter()
                .find(|w| w.command == command)
                .unwrap()
                .spec
                .clone()
        };
        assert_eq!(spec(Command::Scope("clipboard")), None);
        assert_eq!(spec(Command::Scope("translate")), None);
        assert_eq!(spec(Command::Scope("agents")).as_deref(), Some("cmd-alt-a"));
    }

    /// 模拟 Carbon：`taken` 里的组合被别人占着，注册必失败。
    #[derive(Default)]
    struct Fake {
        taken: Vec<HotKey>,
        registered: RefCell<Vec<HotKey>>,
    }

    impl Registrar for Fake {
        fn register(&self, hotkey: HotKey) -> Result<()> {
            let mut registered = self.registered.borrow_mut();
            if self.taken.contains(&hotkey) || registered.contains(&hotkey) {
                bail!("RegisterEventHotKey failed");
            }
            registered.push(hotkey);
            Ok(())
        }

        fn unregister(&self, hotkey: HotKey) -> Result<()> {
            self.registered.borrow_mut().retain(|h| *h != hotkey);
            Ok(())
        }
    }

    fn want(command: Command, spec: &str) -> Wanted {
        Wanted {
            command,
            label: "测试",
            spec: Some(spec.into()),
        }
    }

    #[test]
    fn duplicate_combination_is_reported_and_first_wins() {
        // 两条命令绑同一个组合，按下时只会触发其中一个；必须报出来而不是悄悄覆盖。
        let fake = Fake::default();
        let mut registry = Registry::default();
        let problems = registry.apply(
            &fake,
            &[
                want(Command::Toggle, "cmd-shift-v"),
                want(Command::Scope("clipboard"), "cmd-shift-v"),
            ],
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems.join("").contains("冲突"), "{problems:?}");
        assert_eq!(
            registry.get(Command::Toggle),
            Some(parse("cmd-shift-v").unwrap())
        );
        assert_eq!(registry.get(Command::Scope("clipboard")), None);
        assert_eq!(fake.registered.borrow().len(), 1);
    }

    #[test]
    fn failed_registration_keeps_the_old_hotkey() {
        // 用户在设置里改成一个被系统占用的组合时，不能连原来能用的快捷键也丢了。
        let fake = Fake {
            taken: vec![parse("cmd-space").unwrap()],
            ..Fake::default()
        };
        let mut registry = Registry::default();
        assert!(
            registry
                .apply(&fake, &[want(Command::Toggle, "alt-space")])
                .is_empty()
        );

        let problems = registry.apply(&fake, &[want(Command::Toggle, "cmd-space")]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems.join("").contains("继续使用"), "{problems:?}");
        assert_eq!(
            registry.get(Command::Toggle),
            Some(parse("alt-space").unwrap())
        );
        assert_eq!(*fake.registered.borrow(), [parse("alt-space").unwrap()]);
    }

    #[test]
    fn invalid_string_keeps_the_old_hotkey() {
        // 配置文件手改坏了，也要保留当前能用的组合。
        let fake = Fake::default();
        let mut registry = Registry::default();
        registry.apply(&fake, &[want(Command::Toggle, "alt-space")]);
        let problems = registry.apply(&fake, &[want(Command::Toggle, "alt-nosuchkey")]);
        assert_eq!(problems.len(), 1);
        assert_eq!(
            registry.get(Command::Toggle),
            Some(parse("alt-space").unwrap())
        );
    }

    #[test]
    fn swapping_two_combinations_succeeds() {
        // 先注销再注册：两条命令互换组合时，不能因为「对方还占着」而失败。
        let fake = Fake::default();
        let mut registry = Registry::default();
        registry.apply(
            &fake,
            &[
                want(Command::Toggle, "alt-space"),
                want(Command::Scope("clipboard"), "cmd-shift-v"),
            ],
        );
        let problems = registry.apply(
            &fake,
            &[
                want(Command::Toggle, "cmd-shift-v"),
                want(Command::Scope("clipboard"), "alt-space"),
            ],
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            registry.get(Command::Toggle),
            Some(parse("cmd-shift-v").unwrap())
        );
        let id = parse("alt-space").unwrap().id();
        assert_eq!(registry.command_for(id), Some(Command::Scope("clipboard")));
    }

    #[test]
    fn unbinding_releases_the_combination() {
        // 停用扩展后，组合要还给系统，别的 App 才能用。
        let fake = Fake::default();
        let mut registry = Registry::default();
        registry.apply(&fake, &[want(Command::Scope("clipboard"), "cmd-shift-v")]);
        let problems = registry.apply(
            &fake,
            &[Wanted {
                command: Command::Scope("clipboard"),
                label: "剪贴板历史",
                spec: None,
            }],
        );
        assert!(problems.is_empty());
        assert!(fake.registered.borrow().is_empty());
        assert_eq!(registry.get(Command::Scope("clipboard")), None);
    }

    #[test]
    fn function_keys_work_without_modifiers() {
        // F1–F20 不产生字符，单独做全局快捷键很常见（HapiGo 用户就有 ⌃⌥⇧⌘F20）；
        // 录制框允许 F5，解析这边不放行的话，录下来的值永远注册不上。
        assert_eq!(
            parse("f5").unwrap(),
            HotKey::new(Some(Modifiers::empty()), Code::F5)
        );
        assert_eq!(parse("f20").unwrap().key, Code::F20);
        assert_eq!(
            to_spec(&parse("ctrl-f13").unwrap()).as_deref(),
            Some("ctrl-f13")
        );
        // 字母、空格仍然必须带修饰键。
        assert!(parse("q").is_err());
        assert!(parse("space").is_err());
    }

    #[test]
    fn carbon_keycodes_round_trip() {
        // 系统快捷键 plist 存的是虚拟键码；对照表错一位，⇧⌘3 就会被认成别的组合而漏报。
        assert_eq!(carbon_keycode(Code::Space), Some(0x31));
        assert_eq!(carbon_keycode(Code::Digit3), Some(0x14));
        assert_eq!(code_from_carbon(0x14), Some(Code::Digit3));
        for (_, code, .., vk) in KEYS {
            assert_eq!(code_from_carbon(*vk), Some(*code), "{code:?}");
        }
    }

    #[test]
    fn cleared_hotkey_is_never_triggered() {
        // 用户清除了剪贴板历史的快捷键：注册表里不能还留着 ⇧⌘V，否则按下去照样打开剪贴板。
        let fake = Fake::default();
        let mut registry = Registry::default();
        registry.apply(&fake, &wanted(&Config::default()));
        let shift_cmd_v = parse("cmd-shift-v").unwrap();
        assert_eq!(
            registry.command_for(shift_cmd_v.id()),
            Some(Command::Scope("clipboard"))
        );

        let mut config = Config::default();
        config.extensions.insert(
            "clipboard".into(),
            ExtensionConfig {
                hotkey: Some(String::new()),
                ..ExtensionConfig::default()
            },
        );
        registry.apply(&fake, &wanted(&config));
        assert_eq!(registry.command_for(shift_cmd_v.id()), None);
        assert!(!fake.registered.borrow().contains(&shift_cmd_v));
        assert_eq!(hint_in(Some(&registry), Command::Scope("clipboard")), None);
    }

    #[test]
    fn failures_are_recorded_per_command_for_the_settings_window() {
        // 只写 stderr 的话用户永远看不到：注册失败、重复、写坏都要记到对应命令上。
        let fake = Fake {
            taken: vec![parse("cmd-shift-2").unwrap()],
            ..Fake::default()
        };
        let mut registry = Registry::default();
        registry.apply(
            &fake,
            &[
                want(Command::Toggle, "alt-space"),
                want(Command::Scope("clipboard"), "alt-space"),
                want(Command::Scope("translate"), "alt-nosuchkey"),
                want(
                    Command::RunItem {
                        id: "screenshot:ocr-screen",
                        query: "截图识别文字",
                    },
                    "cmd-shift-2",
                ),
            ],
        );
        assert_eq!(registry.issue(Command::Toggle), None);
        assert_eq!(
            registry.issue(Command::Scope("clipboard")),
            Some(&Issue::Duplicate {
                wanted: parse("alt-space").unwrap(),
                owner: Command::Toggle,
            })
        );
        assert!(matches!(
            registry.issue(Command::Scope("translate")),
            Some(Issue::Invalid { .. })
        ));
        let ocr = Command::RunItem {
            id: "screenshot:ocr-screen",
            query: "截图识别文字",
        };
        assert_eq!(
            registry.issue(ocr),
            Some(&Issue::Unavailable {
                wanted: parse("cmd-shift-2").unwrap(),
                kept: None,
            })
        );
        let message = registry.issue(ocr).unwrap().message();
        assert!(
            message.contains("⇧⌘2") && message.contains("占用"),
            "{message}"
        );
        let message = registry
            .issue(Command::Scope("clipboard"))
            .unwrap()
            .message();
        assert!(message.contains("唤起 Helmp"), "{message}");

        // 用户改成能用的组合后，警示要消失。
        registry.apply(
            &fake,
            &[
                want(Command::Toggle, "alt-space"),
                want(Command::Scope("clipboard"), "cmd-shift-v"),
            ],
        );
        assert_eq!(registry.issue(Command::Scope("clipboard")), None);
    }

    #[test]
    fn hints_show_only_what_actually_works() {
        // 菜单和面板里显示的快捷键必须按得出来：冲突没生效的不能显示，生效的按 ⌃⌥⇧⌘ 顺序显示。
        let fake = Fake::default();
        let mut registry = Registry::default();
        registry.apply(
            &fake,
            &[
                want(Command::Toggle, "cmd-shift-v"),
                want(Command::Scope("clipboard"), "cmd-shift-v"),
            ],
        );
        assert_eq!(
            hint_in(Some(&registry), Command::Toggle).as_deref(),
            Some("⇧⌘V")
        );
        assert_eq!(hint_in(Some(&registry), Command::Scope("clipboard")), None);
        // 热键还没注册（命令行模式、单测）时什么都不显示。
        assert_eq!(hint_in(None, Command::Toggle), None);
    }

    #[test]
    fn screenshot_has_its_own_configurable_shortcut() {
        // 「截图」和「截图识别文字」是同一个插件的两条快捷键：各自存、各自清，互不影响；
        // 插件停用时两条都不注册。
        let capture = extension_command("screenshot-capture").unwrap();
        let ocr = extension_command("screenshot").unwrap();
        assert_eq!(
            capture,
            Command::RunItem {
                id: "screenshot:clipboard",
                query: "截图"
            }
        );
        assert_ne!(capture, ocr);
        assert_eq!(default_spec(capture), Some("cmd-shift-1"));
        let config = Config::default();
        assert_eq!(assigned_spec(&config, capture).as_deref(), Some("cmd-shift-1"));
        assert_eq!(assigned_spec(&config, ocr).as_deref(), Some("cmd-shift-2"));

        let mut config = Config::default();
        config.extensions.insert(
            "screenshot-capture".into(),
            ExtensionConfig {
                hotkey: Some("ctrl-alt-s".into()),
                ..ExtensionConfig::default()
            },
        );
        assert_eq!(assigned_spec(&config, capture).as_deref(), Some("ctrl-alt-s"));
        assert_eq!(assigned_spec(&config, ocr).as_deref(), Some("cmd-shift-2"));

        config.extensions.insert(
            "screenshot".into(),
            ExtensionConfig {
                enabled: false,
                ..ExtensionConfig::default()
            },
        );
        let wanted = wanted(&config);
        for command in [capture, ocr] {
            let spec = &wanted.iter().find(|w| w.command == command).unwrap().spec;
            assert_eq!(spec, &None, "{command:?}");
        }
    }

    #[test]
    fn screenshot_and_ocr_are_both_registered_and_hinted_on_their_own_commands() {
        // 用户看到的「截图快捷键只有一个」：两条都要真的注册上，结果右侧的键帽也要各显示各的，
        // 不能两条命令都显示插件主键位（截图识别文字）的那个组合，或者「截图」那一项干脆没有键帽。
        let fake = Fake::default();
        let mut registry = Registry::default();
        let problems = registry.apply(&fake, &wanted(&Config::default()));
        assert!(problems.is_empty(), "{problems:?}");
        let capture = item_command("screenshot:clipboard").unwrap();
        let ocr = item_command("screenshot:ocr-screen").unwrap();
        assert_eq!(Some(capture), extension_command("screenshot-capture"));
        assert_eq!(Some(ocr), extension_command("screenshot"));
        assert_eq!(registry.issue(capture), None);
        assert_eq!(registry.issue(ocr), None);
        assert_eq!(hint_in(Some(&registry), capture).as_deref(), Some("⇧⌘1"));
        assert_eq!(hint_in(Some(&registry), ocr).as_deref(), Some("⇧⌘2"));
        // 不是由快捷键直接执行的项没有键帽。
        assert_eq!(item_command("screenshot:save"), None);

        // 设置里逐条列出：截图插件名下就是这两条，「截图」在前。
        let slots: Vec<&str> = shortcuts_of("screenshot").map(|s| s.slot).collect();
        assert_eq!(slots, ["screenshot-capture", "screenshot"]);
        let slots: Vec<&str> = shortcuts_of("clipboard").map(|s| s.slot).collect();
        assert_eq!(slots, ["clipboard"]);
    }

    #[test]
    fn suspending_really_unregisters_everything() {
        // 录制时要把全部组合让出来：只传空列表给 apply 什么都不会注销，
        // 用户在录制框里按当前的唤起键（⌥Space）就会把面板叫出来，而不是录到组合。
        let fake = Fake::default();
        let mut registry = Registry::default();
        let config = Config::default();
        registry.apply(&fake, &wanted_while(false, &config));
        assert_eq!(
            fake.registered.borrow().len(),
            EXTENSION_SHORTCUTS.len() + 1
        );
        registry.apply(&fake, &wanted_while(true, &config));
        assert!(fake.registered.borrow().is_empty());
        assert_eq!(registry.get(Command::Toggle), None);
        registry.apply(&fake, &wanted_while(false, &config));
        assert_eq!(
            registry.get(Command::Toggle),
            Some(parse("alt-space").unwrap())
        );
    }

    #[test]
    fn assigned_keeps_disabled_extensions_but_wanted_does_not() {
        // 停用扩展不注册（还给别的 App），但它分到的组合仍算它的：录制别的命令时要指名冲突。
        let mut config = Config::default();
        config.extensions.insert(
            "agents".into(),
            ExtensionConfig {
                enabled: false,
                ..ExtensionConfig::default()
            },
        );
        assert_eq!(
            assigned_spec(&config, Command::Scope("agents")).as_deref(),
            Some("ctrl-alt-a")
        );
        let wanted = wanted(&config);
        assert_eq!(
            wanted
                .iter()
                .find(|w| w.command == Command::Scope("agents"))
                .unwrap()
                .spec,
            None
        );
        assert_eq!(default_spec(Command::Scope("agents")), Some("ctrl-alt-a"));
        assert_eq!(default_spec(Command::Toggle), Some(DEFAULT_TOGGLE));
    }
}
