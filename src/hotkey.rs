//! 全局快捷键：`global-hotkey`（Carbon `RegisterEventHotKey`，免辅助功能权限）。
//!
//! - 唤起面板：`config.general.hotkey`（默认 ⌥Space）。
//! - 范围快捷键：`config.extensions[<provider id>].hotkey`，未配置用 [`EXTENSION_SHORTCUTS`] 的默认值，
//!   配置成空字符串表示不绑定；扩展被停用时也不绑定。
//! - 配置变化（设置窗口改快捷键）时增量重新注册：新组合注册失败就保留旧的，并在 stderr 说明。
//!
//! 事件在 Carbon 回调线程产生，这里用前台任务定时轮询 receiver，把命令投回 GPUI 主线程
//! （Loungy 的做法）。菜单栏通过 [`active`] 读取真实生效的快捷键来显示。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::*;

use crate::config::Config;
use crate::launcher::{Launcher, LauncherHandle};
use crate::provider::{Action, Outcome};

/// 唤起面板的默认快捷键，与 `config::General::default()` 一致。
pub const DEFAULT_TOGGLE: &str = "alt-space";

const POLL_INTERVAL: Duration = Duration::from_millis(50);

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
    pub provider: &'static str,
    pub label: &'static str,
    pub default: &'static str,
    pub command: Command,
}

pub const EXTENSION_SHORTCUTS: [ExtensionShortcut; 4] = [
    ExtensionShortcut {
        provider: "clipboard",
        label: "剪贴板历史",
        default: "cmd-shift-v",
        command: Command::Scope("clipboard"),
    },
    ExtensionShortcut {
        provider: "screenshot",
        label: "截图识别文字",
        default: "cmd-shift-2",
        command: Command::RunItem {
            id: "screenshot:ocr-screen",
            query: "截图识别文字",
        },
    },
    ExtensionShortcut {
        provider: "translate",
        label: "翻译选中文字",
        default: "alt-shift-t",
        command: Command::Scope("translate"),
    },
    ExtensionShortcut {
        provider: "agents",
        label: "Agent 会话",
        default: "ctrl-alt-a",
        command: Command::Scope("agents"),
    },
];

// ======================= 字符串 ⇄ HotKey =======================

/// (配置里的写法, 物理键, 显示符号, NSMenuItem 的 keyEquivalent 字符)
const KEYS: &[(&str, Code, &str, char)] = &[
    ("a", Code::KeyA, "A", 'a'),
    ("b", Code::KeyB, "B", 'b'),
    ("c", Code::KeyC, "C", 'c'),
    ("d", Code::KeyD, "D", 'd'),
    ("e", Code::KeyE, "E", 'e'),
    ("f", Code::KeyF, "F", 'f'),
    ("g", Code::KeyG, "G", 'g'),
    ("h", Code::KeyH, "H", 'h'),
    ("i", Code::KeyI, "I", 'i'),
    ("j", Code::KeyJ, "J", 'j'),
    ("k", Code::KeyK, "K", 'k'),
    ("l", Code::KeyL, "L", 'l'),
    ("m", Code::KeyM, "M", 'm'),
    ("n", Code::KeyN, "N", 'n'),
    ("o", Code::KeyO, "O", 'o'),
    ("p", Code::KeyP, "P", 'p'),
    ("q", Code::KeyQ, "Q", 'q'),
    ("r", Code::KeyR, "R", 'r'),
    ("s", Code::KeyS, "S", 's'),
    ("t", Code::KeyT, "T", 't'),
    ("u", Code::KeyU, "U", 'u'),
    ("v", Code::KeyV, "V", 'v'),
    ("w", Code::KeyW, "W", 'w'),
    ("x", Code::KeyX, "X", 'x'),
    ("y", Code::KeyY, "Y", 'y'),
    ("z", Code::KeyZ, "Z", 'z'),
    ("0", Code::Digit0, "0", '0'),
    ("1", Code::Digit1, "1", '1'),
    ("2", Code::Digit2, "2", '2'),
    ("3", Code::Digit3, "3", '3'),
    ("4", Code::Digit4, "4", '4'),
    ("5", Code::Digit5, "5", '5'),
    ("6", Code::Digit6, "6", '6'),
    ("7", Code::Digit7, "7", '7'),
    ("8", Code::Digit8, "8", '8'),
    ("9", Code::Digit9, "9", '9'),
    ("-", Code::Minus, "-", '-'),
    ("=", Code::Equal, "=", '='),
    ("[", Code::BracketLeft, "[", '['),
    ("]", Code::BracketRight, "]", ']'),
    ("\\", Code::Backslash, "\\", '\\'),
    (";", Code::Semicolon, ";", ';'),
    ("'", Code::Quote, "'", '\''),
    (",", Code::Comma, ",", ','),
    (".", Code::Period, ".", '.'),
    ("/", Code::Slash, "/", '/'),
    ("`", Code::Backquote, "`", '`'),
    ("space", Code::Space, "Space", ' '),
    ("enter", Code::Enter, "↵", '\r'),
    ("tab", Code::Tab, "⇥", '\t'),
    ("escape", Code::Escape, "⎋", '\u{1b}'),
    ("backspace", Code::Backspace, "⌫", '\u{8}'),
    ("delete", Code::Delete, "⌦", '\u{f728}'),
    ("up", Code::ArrowUp, "↑", '\u{f700}'),
    ("down", Code::ArrowDown, "↓", '\u{f701}'),
    ("left", Code::ArrowLeft, "←", '\u{f702}'),
    ("right", Code::ArrowRight, "→", '\u{f703}'),
    ("home", Code::Home, "↖", '\u{f729}'),
    ("end", Code::End, "↘", '\u{f72b}'),
    ("pageup", Code::PageUp, "⇞", '\u{f72c}'),
    ("pagedown", Code::PageDown, "⇟", '\u{f72d}'),
    ("f1", Code::F1, "F1", '\u{f704}'),
    ("f2", Code::F2, "F2", '\u{f705}'),
    ("f3", Code::F3, "F3", '\u{f706}'),
    ("f4", Code::F4, "F4", '\u{f707}'),
    ("f5", Code::F5, "F5", '\u{f708}'),
    ("f6", Code::F6, "F6", '\u{f709}'),
    ("f7", Code::F7, "F7", '\u{f70a}'),
    ("f8", Code::F8, "F8", '\u{f70b}'),
    ("f9", Code::F9, "F9", '\u{f70c}'),
    ("f10", Code::F10, "F10", '\u{f70d}'),
    ("f11", Code::F11, "F11", '\u{f70e}'),
    ("f12", Code::F12, "F12", '\u{f70f}'),
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
    if mods.is_empty() {
        // 不带修饰键的全局快捷键会吞掉所有 App 里的这个键，等于把键盘弄坏。
        bail!("「{spec}」没有修饰键，全局快捷键至少要带一个 cmd / ctrl / alt / shift");
    }
    Ok(HotKey::new(Some(mods), code))
}

/// `HotKey` → 配置写法，[`parse`] 的逆运算。表外的键返回 None。
/// 给设置窗口录制快捷键后写回配置用；在它接上之前只有测试调用。
#[allow(dead_code)]
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
        Some((_, _, glyph, _)) => out.push_str(glyph),
        None => out.push_str(&hotkey.key.to_string()),
    }
    out
}

/// NSMenuItem 的 keyEquivalent 字符（菜单右侧显示快捷键用）。
pub fn key_equivalent(hotkey: &HotKey) -> Option<char> {
    KEYS.iter()
        .find(|(_, code, ..)| *code == hotkey.key)
        .map(|(.., ch)| *ch)
}

// ======================= 期望的绑定 =======================

/// 一条期望的绑定。`spec = None` 表示不绑定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub command: Command,
    pub label: &'static str,
    pub spec: Option<String>,
}

/// 按配置算出所有命令的期望绑定。顺序即优先级：组合冲突时排在前面的保留。
pub fn wanted(config: &Config) -> Vec<Wanted> {
    let mut out = vec![Wanted {
        command: Command::Toggle,
        label: "唤起 Helmp",
        spec: Some(config.general.hotkey.clone()),
    }];
    for shortcut in &EXTENSION_SHORTCUTS {
        let ext = config.extensions.get(shortcut.provider);
        let enabled = ext.is_none_or(|ext| ext.enabled);
        let spec = match ext.and_then(|ext| ext.hotkey.as_deref()) {
            _ if !enabled => None,
            // 用户清空了快捷键：不绑定。
            Some(spec) if spec.trim().is_empty() => None,
            Some(spec) => Some(spec.to_string()),
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

/// 当前真实注册着的绑定。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Registry {
    bound: BTreeMap<Command, HotKey>,
}

impl Registry {
    pub fn get(&self, command: Command) -> Option<HotKey> {
        self.bound.get(&command).copied()
    }

    pub fn command_for(&self, hotkey_id: u32) -> Option<Command> {
        self.bound
            .iter()
            .find(|(_, hotkey)| hotkey.id() == hotkey_id)
            .map(|(command, _)| *command)
    }

    /// 把注册状态调整到 `wanted`，返回需要告诉用户的问题（每条一句）。
    ///
    /// - 解析失败 / 注册失败：保留这条命令原来的组合。
    /// - 两条命令想要同一个组合：排在前面的拿到，后面的报冲突、保留原来的（原来的也冲突就不绑定）。
    pub fn apply(&mut self, registrar: &impl Registrar, wanted: &[Wanted]) -> Vec<String> {
        let mut problems = Vec::new();

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
                            current
                        }
                    },
                };
                (want.command, want.label, target)
            })
            .collect();

        // 2. 冲突：按优先级认领组合。
        let mut claimed: Vec<(HotKey, &'static str)> = Vec::new();
        for i in 0..targets.len() {
            let (command, label, Some(target)) = targets[i] else {
                continue;
            };
            if let Some((_, owner)) = claimed.iter().find(|(hotkey, _)| *hotkey == target) {
                problems.push(format!(
                    "「{label}」的快捷键 {} 与「{owner}」冲突，未生效",
                    glyphs(&target)
                ));
                let fallback = self.get(command).filter(|current| {
                    *current != target
                        && !claimed.iter().any(|(hotkey, _)| hotkey == current)
                        && !targets
                            .iter()
                            .any(|(other, _, t)| *other != command && *t == Some(*current))
                });
                targets[i].2 = fallback;
                if let Some(fallback) = fallback {
                    claimed.push((fallback, label));
                }
            } else {
                claimed.push((target, label));
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

/// 每次绑定变化加一，菜单栏据此决定是否重建菜单。
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
    }
    publish(&registry);

    let (changed_tx, changed_rx) = mpsc::channel::<()>();
    crate::config::subscribe(move |_| {
        // 接收端在轮询任务里，进程结束前一直存在；发送失败只可能发生在退出途中。
        let _ = changed_tx.send(());
    });

    cx.spawn(async move |cx: &mut AsyncApp| {
        let receiver = GlobalHotKeyEvent::receiver();
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
            if changed_rx.try_recv().is_ok() {
                while changed_rx.try_recv().is_ok() {}
                let wanted_now = wanted(&crate::config::get());
                report(registry.apply(&manager, &wanted_now));
                publish(&registry);
            }
            cx.background_executor().timer(POLL_INTERVAL).await;
        }
    })
    .detach();

    Ok(())
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
                    Some(item) => cx.update(|cx| run_action(item.title, item.action, cx)),
                    None => eprintln!("helmp: 找不到「{id}」（扩展可能被停用）"),
                }
            })
            .detach();
        }
    }
}

/// 启动器的引擎（菜单栏、快捷键查询用，结果与面板一致）。
pub fn engine(cx: &App) -> Option<crate::engine::Engine> {
    match cx.try_global::<LauncherHandle>() {
        Some(handle) => Some(handle.launcher.read(cx).engine()),
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
pub fn run_action(title: String, action: Action, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let outcome = cx
            .background_executor()
            .spawn(async move { action.run() })
            .await;
        cx.update(|cx| match outcome {
            Ok(Outcome::Done) => {}
            Ok(Outcome::EnterScope(scope)) => execute(Command::Scope(scope), cx),
            Ok(Outcome::OpenSettings) => crate::settings::open(cx),
            Ok(Outcome::Message(message)) => {
                eprintln!("helmp: 「{title}」的结果（面板无法展示）：{message}")
            }
            Ok(Outcome::Show(detail)) => {
                eprintln!("helmp: 「{title}」的结果（面板无法展示）：{}", detail.title)
            }
            Err(err) => eprintln!("helmp: 「{title}」执行失败：{err:#}"),
        });
    })
    .detach();
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
}
