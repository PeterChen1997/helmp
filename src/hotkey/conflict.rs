//! 录制快捷键时的冲突判定（HapiGo 的做法：冲突就不保存，指名说出被谁占用）。
//!
//! 按顺序查：
//! 1. Helmp 自己的其他命令（含停用扩展分到的组合）；
//! 2. 系统快捷键：「键盘 › 键盘快捷键」里启用的项，读
//!    `~/Library/Preferences/com.apple.symbolichotkeys.plist`（只读；用户没改过的项不会写进去，按系统默认补上）；
//! 3. macOS 固定占用、设置里改不了的组合（⌘Tab、⌥⌘Esc…）；
//! 4. 各 App 通用的菜单快捷键（⌘Q、⌘W、⌘C…）：全局注册后所有 App 里的这个快捷键都会失效；
//! 5. 实际向 Carbon `RegisterEventHotKey` 独占注册一次探测是否被其他 App 占用，随即注销。
//!
//! [`check`] 是纯函数（系统快捷键列表、探测结果由调用方传入），测试覆盖它；
//! [`load_system_shortcuts`] / [`probe`] 是它在真实系统上的两个输入。

use std::ffi::c_void;
use std::sync::Mutex;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};

use super::{
    Command, Wanted, carbon_keycode, code_from_carbon, command_label, is_function_key, parse,
};
use crate::i18n::{tr, trf};

/// 中英文名称。
pub type Name = (&'static str, &'static str);

/// 一个系统占用的组合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemShortcut {
    pub hotkey: HotKey,
    /// None：plist 里启用了但不认识的条目，只能说「已被系统使用」。
    pub name: Option<Name>,
}

/// 录到的组合不能用的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    /// 已分给 Helmp 的另一条命令。
    Command(Command),
    /// 系统快捷键（键盘设置里启用的，或 macOS 固定占用的）。
    System(Option<Name>),
    /// 各 App 通用的菜单快捷键。
    AppStandard(Name),
    /// 独占注册失败（eventHotKeyExistsErr）：别的 App 已经注册了它。
    OtherApp,
}

impl Conflict {
    /// 显示在录制框下方的红字（不含组合本身）。
    pub fn message(&self) -> String {
        match self {
            Conflict::Command(command) => trf!(
                "已被「{}」使用",
                "Already used by “{}”",
                command_label(*command)
            ),
            Conflict::System(Some((zh, en))) => {
                trf!("已被系统使用（{}）", "Used by macOS ({})", tr(zh, en))
            }
            Conflict::System(None) => tr("已被系统使用", "Used by macOS").to_string(),
            Conflict::AppStandard((zh, en)) => trf!(
                "会和应用内快捷键冲突（{}）",
                "Conflicts with a standard app shortcut ({})",
                tr(zh, en)
            ),
            Conflict::OtherApp => {
                tr("已被其他应用占用", "Already taken by another app").to_string()
            }
        }
    }
}

/// 向 Carbon 探测的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Free,
    Taken,
    /// 探测本身出错（不是「被占用」）：不拦用户，交给真正注册时再报。
    Unknown,
}

/// 判定 `command` 能否用 `candidate`。
///
/// `assigned` 是各命令当前分到的组合（`hotkey::assigned`），`system` 是系统占用的组合
/// （[`load_system_shortcuts`]），`probe` 只在前面都没冲突时才调用（它要真的注册一次）。
pub fn check(
    candidate: HotKey,
    command: Command,
    assigned: &[Wanted],
    system: &[SystemShortcut],
    probe: impl FnOnce(HotKey) -> Probe,
) -> Result<(), Conflict> {
    let spec_of = |want: &Wanted| want.spec.as_deref().and_then(|spec| parse(spec).ok());
    // 重新录了同一个组合：什么都没变，不算冲突。
    if assigned
        .iter()
        .any(|want| want.command == command && spec_of(want) == Some(candidate))
    {
        return Ok(());
    }
    if let Some(owner) = assigned
        .iter()
        .find(|want| want.command != command && spec_of(want) == Some(candidate))
    {
        return Err(Conflict::Command(owner.command));
    }
    if let Some(shortcut) = system.iter().find(|shortcut| shortcut.hotkey == candidate) {
        return Err(Conflict::System(shortcut.name));
    }
    if let Some((_, name)) = FIXED_SYSTEM
        .iter()
        .find(|(spec, _)| parse(spec).ok() == Some(candidate))
    {
        return Err(Conflict::System(Some(*name)));
    }
    if let Some((_, name)) = APP_STANDARD
        .iter()
        .find(|(spec, _)| parse(spec).ok() == Some(candidate))
    {
        return Err(Conflict::AppStandard(*name));
    }
    match probe(candidate) {
        Probe::Taken => Err(Conflict::OtherApp),
        Probe::Free | Probe::Unknown => Ok(()),
    }
}

// ======================= 静态表 =======================

const SCREENSHOT: Name = ("截屏", "Screenshot");
const INPUT_SOURCE: Name = ("切换输入法", "Switch input source");
const KEYBOARD_NAV: Name = ("键盘导航", "Keyboard navigation");
const MISSION_CONTROL: Name = ("调度中心", "Mission Control");
const APP_WINDOWS: Name = ("应用程序窗口", "Application windows");
const SPACES: Name = ("切换桌面", "Switch Spaces");
const SHOW_DESKTOP: Name = ("显示桌面", "Show Desktop");
const ZOOM: Name = ("缩放与显示辅助", "Zoom and display accessibility");
const BRIGHTNESS: Name = ("显示器亮度", "Display brightness");

/// macOS 固定占用、在系统设置里改不了的组合。
const FIXED_SYSTEM: &[(&str, Name)] = &[
    ("cmd-tab", ("切换应用", "Switch apps")),
    ("cmd-shift-tab", ("切换应用", "Switch apps")),
    ("alt-cmd-escape", ("强制退出", "Force Quit")),
    ("ctrl-cmd-q", ("锁定屏幕", "Lock Screen")),
    ("cmd-shift-q", ("退出登录", "Log Out")),
];

/// 各 App 通用的菜单快捷键（多为单修饰键 ⌘ 组合）。
const APP_STANDARD: &[(&str, Name)] = &[
    ("cmd-q", ("⌘Q 退出", "⌘Q Quit")),
    ("cmd-w", ("⌘W 关闭窗口", "⌘W Close Window")),
    ("cmd-h", ("⌘H 隐藏", "⌘H Hide")),
    ("alt-cmd-h", ("⌥⌘H 隐藏其他", "⌥⌘H Hide Others")),
    ("cmd-m", ("⌘M 最小化", "⌘M Minimize")),
    ("cmd-c", ("⌘C 拷贝", "⌘C Copy")),
    ("cmd-v", ("⌘V 粘贴", "⌘V Paste")),
    ("cmd-x", ("⌘X 剪切", "⌘X Cut")),
    ("cmd-z", ("⌘Z 撤销", "⌘Z Undo")),
    ("cmd-shift-z", ("⇧⌘Z 重做", "⇧⌘Z Redo")),
    ("cmd-a", ("⌘A 全选", "⌘A Select All")),
    ("cmd-s", ("⌘S 存储", "⌘S Save")),
    ("cmd-n", ("⌘N 新建", "⌘N New")),
    ("cmd-o", ("⌘O 打开", "⌘O Open")),
    ("cmd-p", ("⌘P 打印", "⌘P Print")),
    ("cmd-f", ("⌘F 查找", "⌘F Find")),
    ("cmd-t", ("⌘T 新建标签页", "⌘T New Tab")),
    ("cmd-,", ("⌘, 设置", "⌘, Settings")),
    ("ctrl-cmd-f", ("⌃⌘F 全屏", "⌃⌘F Full Screen")),
    (
        "ctrl-cmd-space",
        ("⌃⌘Space 表情与符号", "⌃⌘Space Emoji & Symbols"),
    ),
];

/// 默认启用的系统快捷键：(plist 里的 id, 默认组合)。用户没改过的项不会写进 plist，按这里补。
const SYMBOLIC_DEFAULTS: &[(u64, &str)] = &[
    (7, "ctrl-f2"),
    (8, "ctrl-f3"),
    (9, "ctrl-f4"),
    (10, "ctrl-f5"),
    (11, "ctrl-f6"),
    (12, "ctrl-f1"),
    (13, "ctrl-f7"),
    (27, "cmd-`"),
    (28, "cmd-shift-3"),
    (29, "ctrl-cmd-shift-3"),
    (30, "cmd-shift-4"),
    (31, "ctrl-cmd-shift-4"),
    (32, "ctrl-up"),
    (33, "ctrl-down"),
    (36, "f11"),
    (52, "alt-cmd-d"),
    (57, "ctrl-f8"),
    (59, "cmd-f5"),
    (60, "ctrl-space"),
    (61, "ctrl-alt-space"),
    (64, "cmd-space"),
    (65, "alt-cmd-space"),
    (79, "ctrl-left"),
    (81, "ctrl-right"),
    (98, "cmd-shift-/"),
    (162, "alt-cmd-f5"),
    (184, "cmd-shift-5"),
];

/// plist 里各 id 的名称（系统设置「键盘快捷键」里的叫法）。
fn symbolic_name(id: u64) -> Option<Name> {
    Some(match id {
        7..=13 | 57 => KEYBOARD_NAV,
        15..=26 => ZOOM,
        27 => ("将焦点移到下一个窗口", "Move focus to next window"),
        28..=31 | 184 => SCREENSHOT,
        32 | 34 => MISSION_CONTROL,
        33 | 35 => APP_WINDOWS,
        36 | 37 => SHOW_DESKTOP,
        52 => ("程序坞自动隐藏", "Dock hiding"),
        53..=56 => BRIGHTNESS,
        59 => ("旁白", "VoiceOver"),
        60 | 61 => INPUT_SOURCE,
        64 => ("聚焦搜索", "Spotlight"),
        65 => ("聚焦搜索：访达搜索窗口", "Spotlight: Finder search window"),
        79..=82 | 118..=133 => SPACES,
        98 => ("显示帮助菜单", "Show Help menu"),
        162 => ("辅助功能快捷键", "Accessibility Shortcuts"),
        _ => return None,
    })
}

// ======================= 系统快捷键（plist） =======================

const NS_SHIFT: u64 = 1 << 17;
const NS_CONTROL: u64 = 1 << 18;
const NS_OPTION: u64 = 1 << 19;
const NS_COMMAND: u64 = 1 << 20;
const NS_FUNCTION: u64 = 1 << 23;

/// 读当前用户启用的系统快捷键（只读）。文件不存在（从没改过任何系统快捷键）时全部按默认。
pub fn load_system_shortcuts() -> Vec<SystemShortcut> {
    let root = crate::paths::home_dir().ok().and_then(|home| {
        let path = home.join("Library/Preferences/com.apple.symbolichotkeys.plist");
        match plist::Value::from_file(&path) {
            Ok(value) => Some(value),
            Err(err) => {
                if path.exists() {
                    eprintln!(
                        "helmp: 读取系统快捷键 {} 失败，按系统默认判断：{err:#}",
                        path.display()
                    );
                }
                None
            }
        }
    });
    system_shortcuts(root.as_ref())
}

/// 从 symbolichotkeys.plist 的内容算出系统占用的组合。`root = None` 表示没有这个文件。
///
/// 每项形如 `{ enabled = 1; value = { parameters = (字符, 虚拟键码, 修饰键标志); type = standard; } }`。
pub fn system_shortcuts(root: Option<&plist::Value>) -> Vec<SystemShortcut> {
    let entries = root
        .and_then(plist::Value::as_dictionary)
        .and_then(|root| root.get("AppleSymbolicHotKeys"))
        .and_then(plist::Value::as_dictionary);
    let mut out = Vec::new();
    let mut seen = Vec::new();
    if let Some(entries) = entries {
        for (key, entry) in entries {
            let Ok(id) = key.parse::<u64>() else {
                continue;
            };
            seen.push(id);
            let Some(entry) = entry.as_dictionary() else {
                continue;
            };
            let enabled = entry.get("enabled").is_some_and(|value| {
                value
                    .as_boolean()
                    .or_else(|| value.as_signed_integer().map(|n| n != 0))
                    .unwrap_or(false)
            });
            if !enabled {
                continue;
            }
            let parameters = entry
                .get("value")
                .and_then(plist::Value::as_dictionary)
                .and_then(|value| value.get("parameters"))
                .and_then(plist::Value::as_array);
            let hotkey = match parameters {
                Some(parameters) => decode(parameters),
                // 启用了但没写组合：用这一项的默认组合。
                None => default_of(id),
            };
            if let Some(hotkey) = hotkey {
                out.push(SystemShortcut {
                    hotkey,
                    name: symbolic_name(id),
                });
            }
        }
    }
    for (id, spec) in SYMBOLIC_DEFAULTS {
        if !seen.contains(id)
            && let Ok(hotkey) = parse(spec)
        {
            out.push(SystemShortcut {
                hotkey,
                name: symbolic_name(*id),
            });
        }
    }
    out
}

fn default_of(id: u64) -> Option<HotKey> {
    SYMBOLIC_DEFAULTS
        .iter()
        .find(|(default_id, _)| *default_id == id)
        .and_then(|(_, spec)| parse(spec).ok())
}

/// `(字符, 虚拟键码, 修饰键标志)` → 组合。没有按键（65535）、Carbon 注册不了的组合返回 None。
fn decode(parameters: &[plist::Value]) -> Option<HotKey> {
    let integer = |ix: usize| {
        parameters.get(ix).and_then(|value| {
            value.as_unsigned_integer().or_else(|| {
                value
                    .as_signed_integer()
                    .and_then(|n| u64::try_from(n).ok())
            })
        })
    };
    let vk = u16::try_from(integer(1)?).ok()?;
    let flags = integer(2)?;
    let code = code_from_carbon(vk)?;
    let mut mods = Modifiers::empty();
    for (flag, modifier) in [
        (NS_SHIFT, Modifiers::SHIFT),
        (NS_CONTROL, Modifiers::CONTROL),
        (NS_OPTION, Modifiers::ALT),
        (NS_COMMAND, Modifiers::META),
    ] {
        if flags & flag != 0 {
            mods |= modifier;
        }
    }
    // F 键、方向键等本身就带 fn 标志；fn + 字母（如「快速备忘录」fn Q）全局热键注册不了，不会撞。
    if flags & NS_FUNCTION != 0 && !fn_native(code) {
        return None;
    }
    if mods.is_empty() && !is_function_key(code) {
        return None;
    }
    Some(HotKey::new(Some(mods), code))
}

/// 系统给这些键自动加 fn 标志。
fn fn_native(code: Code) -> bool {
    is_function_key(code)
        || matches!(
            code,
            Code::ArrowUp
                | Code::ArrowDown
                | Code::ArrowLeft
                | Code::ArrowRight
                | Code::Home
                | Code::End
                | Code::PageUp
                | Code::PageDown
                | Code::Delete
        )
}

// ======================= Carbon =======================

/// `eventHotKeyExistsErr`：组合已被注册（其他 App 独占，或本进程已注册）。
const EVENT_HOT_KEY_EXISTS_ERR: i32 = -9878;
/// `kEventHotKeyExclusive`：独占注册，别人独占过就失败——正好用来探测。
/// 实测值是 1（写成 `1 << 1` 会得到 paramErr -50）。
///
/// 局限：别的 App 以普通方式（非独占）注册的组合，独占注册照样成功，探测不出来；
/// 这种情况真正注册时通常也会成功，按下时由哪边响应取决于系统，Helmp 无从得知。
const EVENT_HOT_KEY_EXCLUSIVE: u32 = 1;
/// `kHIHotKeyModeAllDisabled`：暂停全部系统快捷键（录制期间、仅 Helmp 在前台时）。
const HOT_KEY_MODE_ALL_DISABLED: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn RegisterEventHotKey(
        code: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: *mut c_void,
        options: u32,
        out: *mut *mut c_void,
    ) -> i32;
    fn UnregisterEventHotKey(hotkey: *mut c_void) -> i32;
    fn PushSymbolicHotKeyMode(options: u32) -> *mut c_void;
    fn PopSymbolicHotKeyMode(token: *mut c_void);
}

fn carbon_modifiers(mods: Modifiers) -> u32 {
    // cmdKey / shiftKey / optionKey / controlKey，同 global-hotkey 的换算。
    let mut out = 0;
    if mods.contains(Modifiers::META) {
        out |= 256;
    }
    if mods.contains(Modifiers::SHIFT) {
        out |= 512;
    }
    if mods.contains(Modifiers::ALT) {
        out |= 2048;
    }
    if mods.contains(Modifiers::CONTROL) {
        out |= 4096;
    }
    out
}

/// 独占注册一次再立即注销，判断组合是否被其他 App 占用。主线程调用。
///
/// Helmp 自己注册着的组合直接算空闲（录制时全局热键是挂起的，这只是防挂起还没生效的那一瞬；
/// 和 Helmp 其他命令的冲突由 [`check`] 前面的步骤指名报出）。
pub fn probe(hotkey: HotKey) -> Probe {
    if super::is_bound_by_helmp(hotkey) {
        return Probe::Free;
    }
    let Some(vk) = carbon_keycode(hotkey.key) else {
        return Probe::Unknown;
    };
    let id = EventHotKeyId {
        signature: u32::from_be_bytes(*b"hlpP"),
        id: 0,
    };
    let mut registered: *mut c_void = std::ptr::null_mut();
    // SAFETY: 参数都是值类型；`registered` 是有效的输出指针，成功时拿到的引用立即交还给 Unregister。
    let status = unsafe {
        RegisterEventHotKey(
            u32::from(vk),
            carbon_modifiers(hotkey.mods),
            id,
            GetApplicationEventTarget(),
            EVENT_HOT_KEY_EXCLUSIVE,
            &mut registered,
        )
    };
    match status {
        0 => {
            // SAFETY: `registered` 是上面注册成功返回的引用，只注销这一次。
            let status = unsafe { UnregisterEventHotKey(registered) };
            if status != 0 {
                eprintln!("helmp: 探测快捷键后注销失败（OSStatus {status}）");
            }
            Probe::Free
        }
        EVENT_HOT_KEY_EXISTS_ERR => Probe::Taken,
        other => {
            crate::trace::log(format!("探测快捷键失败 OSStatus {other}"));
            Probe::Unknown
        }
    }
}

/// `PushSymbolicHotKeyMode` 返回的令牌（指针当整数存），0 = 没有暂停。
static SYMBOLIC_MODE: Mutex<usize> = Mutex::new(0);

/// 录制期间暂停系统快捷键（只在 Helmp 是前台 App 时生效、进程退出自动恢复，不改系统设置），
/// 否则按 ⌘Space、⇧⌘3 会直接触发聚焦搜索 / 截屏，录制框根本收不到。主线程调用。
pub fn pause_system_hotkeys() {
    let mut token = SYMBOLIC_MODE.lock().expect("symbolic hotkey mode poisoned");
    if *token == 0 {
        // SAFETY: 无参数前置条件；返回的令牌只在 `restore_system_hotkeys` 里交还一次。
        *token = unsafe { PushSymbolicHotKeyMode(HOT_KEY_MODE_ALL_DISABLED) } as usize;
    }
}

/// 恢复 [`pause_system_hotkeys`] 暂停的系统快捷键。
pub fn restore_system_hotkeys() {
    let mut token = SYMBOLIC_MODE.lock().expect("symbolic hotkey mode poisoned");
    if *token != 0 {
        // SAFETY: 令牌来自 PushSymbolicHotKeyMode，且只交还这一次（随后清零）。
        unsafe { PopSymbolicHotKeyMode(*token as *mut c_void) };
        *token = 0;
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;
    use crate::config::{Config, ExtensionConfig};
    use crate::hotkey::{DEFAULT_TOGGLE, EXTENSION_SHORTCUTS, assigned, glyphs};

    fn key(spec: &str) -> HotKey {
        parse(spec).unwrap()
    }

    fn free(_: HotKey) -> Probe {
        Probe::Free
    }

    /// 只有一个系统条目的 plist：`{ AppleSymbolicHotKeys = { id = { enabled; value.parameters } } }`。
    fn plist_with(entries: &[(u64, bool, Option<[u64; 3]>)]) -> plist::Value {
        let mut hotkeys = plist::Dictionary::new();
        for (id, enabled, parameters) in entries {
            let mut entry = plist::Dictionary::new();
            entry.insert("enabled".into(), plist::Value::Boolean(*enabled));
            if let Some(parameters) = parameters {
                let mut value = plist::Dictionary::new();
                value.insert(
                    "parameters".into(),
                    plist::Value::Array(
                        parameters
                            .iter()
                            .map(|n| plist::Value::Integer((*n).into()))
                            .collect(),
                    ),
                );
                value.insert("type".into(), plist::Value::String("standard".into()));
                entry.insert("value".into(), plist::Value::Dictionary(value));
            }
            hotkeys.insert(id.to_string(), plist::Value::Dictionary(entry));
        }
        let mut root = plist::Dictionary::new();
        root.insert(
            "AppleSymbolicHotKeys".into(),
            plist::Value::Dictionary(hotkeys),
        );
        plist::Value::Dictionary(root)
    }

    fn check_default(
        candidate: &str,
        command: Command,
        system: &[SystemShortcut],
    ) -> Result<(), Conflict> {
        check(
            key(candidate),
            command,
            &assigned(&Config::default()),
            system,
            free,
        )
    }

    #[test]
    fn existing_command_is_named_instead_of_silently_overwritten() {
        // 把唤起键录成剪贴板历史正在用的 ⇧⌘V：静默保存会让剪贴板快捷键悄悄失效，必须指名拦下。
        let err = check_default("cmd-shift-v", Command::Toggle, &[]).unwrap_err();
        assert_eq!(err, Conflict::Command(Command::Scope("clipboard")));
        assert!(err.message().contains("剪贴板历史"), "{}", err.message());
    }

    #[test]
    fn disabled_extension_keeps_its_combination() {
        // 停用的扩展重新启用时要拿回自己的组合；录制时放行就会在启用那一刻撞车。
        let mut config = Config::default();
        config.extensions.insert(
            "agents".into(),
            ExtensionConfig {
                enabled: false,
                ..ExtensionConfig::default()
            },
        );
        let err = check(
            key("ctrl-alt-a"),
            Command::Toggle,
            &assigned(&config),
            &[],
            free,
        )
        .unwrap_err();
        assert_eq!(err, Conflict::Command(Command::Scope("agents")));
    }

    #[test]
    fn cleared_command_releases_its_combination() {
        // 清除了快捷键的命令不再占用组合，别的命令可以拿去用。
        let mut config = Config::default();
        config.extensions.insert(
            "clipboard".into(),
            ExtensionConfig {
                hotkey: Some(String::new()),
                ..ExtensionConfig::default()
            },
        );
        assert_eq!(
            check(
                key("cmd-shift-v"),
                Command::Toggle,
                &assigned(&config),
                &[],
                free
            ),
            Ok(())
        );
    }

    #[test]
    fn recording_the_same_combination_again_is_not_a_conflict() {
        // 用户点开录制又按了一遍原来的组合：不能报「已被自己使用」，也不该去探测（自己正占着它）。
        let result = check(
            key("cmd-shift-v"),
            Command::Scope("clipboard"),
            &assigned(&Config::default()),
            &[],
            |_| panic!("没有变化时不该探测"),
        );
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn spotlight_cmd_space_must_be_reported_by_default() {
        // 没改过系统快捷键的用户 plist 里没有聚焦搜索这一项，但 ⌘Space 照样被系统占着。
        let system = system_shortcuts(None);
        let err = check_default("cmd-space", Command::Toggle, &system).unwrap_err();
        assert_eq!(err, Conflict::System(Some(("聚焦搜索", "Spotlight"))));
        assert!(err.message().contains("聚焦搜索"), "{}", err.message());
        let err = check_default("alt-cmd-space", Command::Toggle, &system).unwrap_err();
        assert!(matches!(err, Conflict::System(Some(_))));
    }

    #[test]
    fn spotlight_disabled_by_user_frees_cmd_space() {
        // 关掉聚焦搜索的人（HapiGo / Raycast 用户常这么做）就是想把 ⌘Space 给启动器，不能误拦。
        let root = plist_with(&[(64, false, Some([65535, 49, 1_048_576]))]);
        let system = system_shortcuts(Some(&root));
        assert_eq!(check_default("cmd-space", Command::Toggle, &system), Ok(()));
    }

    #[test]
    fn user_remapped_system_shortcut_is_read_from_plist() {
        // 用户把聚焦搜索改成了 ⌃⌥Space：要按实际设置拦 ⌃⌥Space，原来的 ⌘Space 则放行。
        let root = plist_with(&[
            (64, true, Some([32, 49, 262_144 | 524_288])),
            (61, false, None),
        ]);
        let system = system_shortcuts(Some(&root));
        assert_eq!(
            check_default("ctrl-alt-space", Command::Toggle, &system),
            Err(Conflict::System(Some(("聚焦搜索", "Spotlight"))))
        );
        assert_eq!(check_default("cmd-space", Command::Toggle, &system), Ok(()));
    }

    #[test]
    fn input_source_switching_must_be_reported() {
        // ⌃Space / ⌃⌥Space 是中文用户切换输入法的命根子，被启动器抢走就打不了字。
        let system = system_shortcuts(None);
        for spec in ["ctrl-space", "ctrl-alt-space"] {
            let err = check_default(spec, Command::Toggle, &system).unwrap_err();
            assert_eq!(err, Conflict::System(Some(INPUT_SOURCE)), "{spec}");
            assert!(err.message().contains("切换输入法"));
        }
    }

    #[test]
    fn screenshot_and_mission_control_must_be_reported() {
        let system = system_shortcuts(None);
        for spec in [
            "cmd-shift-3",
            "cmd-shift-4",
            "cmd-shift-5",
            "ctrl-cmd-shift-4",
        ] {
            assert_eq!(
                check_default(spec, Command::Toggle, &system),
                Err(Conflict::System(Some(SCREENSHOT))),
                "{spec}"
            );
        }
        assert_eq!(
            check_default("ctrl-up", Command::Toggle, &system),
            Err(Conflict::System(Some(MISSION_CONTROL)))
        );
        // F11 显示桌面：不带修饰键的 F 键也会被系统占着。
        assert_eq!(
            check_default("f11", Command::Toggle, &system),
            Err(Conflict::System(Some(SHOW_DESKTOP)))
        );
    }

    #[test]
    fn unknown_enabled_system_entry_still_blocks() {
        // 不认识名字的系统快捷键也是系统在用：说不出名字也要拦，不能放过。
        let root = plist_with(&[(222, true, Some([100, 2, 262_144]))]);
        let system = system_shortcuts(Some(&root));
        let err = check_default("ctrl-d", Command::Toggle, &system).unwrap_err();
        assert_eq!(err, Conflict::System(None));
        assert_eq!(err.message(), "已被系统使用");
    }

    #[test]
    fn fn_letter_and_keyless_entries_are_ignored() {
        // 「快速备忘录」是 fn+Q：全局热键注册不了 fn，不能把它当成占用了「Q」或别的组合。
        // 没有按键（65535）的条目（如启动台）也不占用任何组合。
        let root = plist_with(&[
            (190, true, Some([113, 12, 8_388_608])),
            (160, true, Some([65535, 65535, 0])),
        ]);
        let system = system_shortcuts(Some(&root));
        assert!(
            system
                .iter()
                .all(|shortcut| shortcut.hotkey.key != Code::KeyQ),
            "{system:?}"
        );
    }

    #[test]
    fn standard_app_shortcuts_are_refused() {
        // 全局注册 ⌘Q / ⌘W 后，所有 App 里的「退出」「关闭窗口」都会失效。
        for (spec, part) in [("cmd-q", "退出"), ("cmd-w", "关闭窗口"), ("cmd-c", "拷贝")] {
            let err = check_default(spec, Command::Toggle, &[]).unwrap_err();
            assert!(matches!(err, Conflict::AppStandard(_)), "{spec}");
            assert!(err.message().contains("应用内快捷键"), "{}", err.message());
            assert!(err.message().contains(part), "{}", err.message());
        }
        // ⌘Tab 这类系统固定组合不在 plist 里，也要拦。
        assert!(matches!(
            check_default("cmd-tab", Command::Toggle, &[]),
            Err(Conflict::System(Some(_)))
        ));
    }

    #[test]
    fn taken_by_another_app_is_reported_but_probe_errors_are_not() {
        // 别的 App 独占了组合：保存了也注册不上，要当场说清楚。
        assert_eq!(
            check(
                key("ctrl-alt-cmd-k"),
                Command::Toggle,
                &assigned(&Config::default()),
                &[],
                |_| Probe::Taken
            ),
            Err(Conflict::OtherApp)
        );
        // 探测本身出错不等于被占用：不能因此挡住用户。
        assert_eq!(
            check(
                key("ctrl-alt-cmd-k"),
                Command::Toggle,
                &assigned(&Config::default()),
                &[],
                |_| Probe::Unknown
            ),
            Ok(())
        );
    }

    #[test]
    fn defaults_do_not_collide_with_system_or_standard_shortcuts() {
        // 新用户一装好就在设置里看到冲突警示是不可接受的：出厂默认必须全部通过检查。
        let system = system_shortcuts(None);
        let assigned = assigned(&Config::default());
        let mut defaults = vec![(Command::Toggle, DEFAULT_TOGGLE)];
        defaults.extend(EXTENSION_SHORTCUTS.iter().map(|s| (s.command, s.default)));
        for (command, spec) in defaults {
            let others: Vec<Wanted> = assigned
                .iter()
                .filter(|want| want.command != command)
                .cloned()
                .collect();
            assert_eq!(
                check(key(spec), command, &others, &system, free),
                Ok(()),
                "{spec}"
            );
        }
    }

    #[test]
    fn static_tables_are_valid_specs() {
        // 表里写错一个字，那一项就永远匹配不上，等于没拦。
        for (spec, _) in FIXED_SYSTEM.iter().chain(APP_STANDARD) {
            assert!(parse(spec).is_ok(), "{spec}");
        }
        for (_, spec) in SYMBOLIC_DEFAULTS {
            assert!(parse(spec).is_ok(), "{spec}");
        }
    }

    #[test]
    fn probe_unregisters_after_itself() {
        // 探测只是试一下：试完不注销，Helmp 自己就占着这个组合，用户真要用时反而注册失败。
        // 连续探测两次：第一次没注销的话，第二次会撞上本进程自己的注册而报「已占用」。
        // （没有窗口服务器的环境里两次都可能是 Unknown，那也说明没有残留。）
        let rare = key("ctrl-alt-cmd-shift-f19");
        let first = probe(rare);
        assert_ne!(first, Probe::Taken, "{} 不该被占用", glyphs(&rare));
        assert_eq!(probe(rare), first, "{} 第一次探测没有注销", glyphs(&rare));
    }
}
