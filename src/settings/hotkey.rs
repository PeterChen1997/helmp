//! 快捷键录制框：点一下进入录制，按下组合键写回配置。
//!
//! 配置里的格式同 GPUI keystroke：小写、`-` 连接，修饰键固定按 `cmd-ctrl-alt-shift` 排在前面，
//! 如 `alt-space`、`cmd-shift-space`。界面上按 macOS 习惯显示成 ⌃⌥⇧⌘ 键帽。
//!
//! 录制框不直接写配置：它发出 `HotkeyEvent`，由设置窗口写盘（失败时在窗口里报错）。
//! 冲突判定见 `crate::hotkey::conflict`：冲突的组合不会发出事件，录制框停在录制态等用户重按。

use std::rc::Rc;
use std::time::Duration;

use global_hotkey::hotkey::{HotKey, Modifiers as HotKeyModifiers};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::*;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSEventType};

use crate::config;
use crate::hotkey::Command;
use crate::hotkey::conflict::{self, Conflict, SystemShortcut};
use crate::i18n::{tr, trf};
use crate::style::Palette;

/// 把一次按键转成配置字符串。只按了修饰键、或者没有 ⌘ / ⌃ / ⌥ 时返回 None：
/// 单独的字母或 ⇧+字母做全局热键会吞掉正常打字。F1–F20 例外，可以不带修饰键。
pub fn keystroke_to_hotkey(modifiers: &Modifiers, key: &str) -> Option<String> {
    let key = key.to_ascii_lowercase();
    if key.is_empty() || is_modifier_key(&key) {
        return None;
    }
    let strong = modifiers.platform || modifiers.control || modifiers.alt;
    if !strong && !is_function_key(&key) {
        return None;
    }
    let mut parts = Vec::new();
    if modifiers.platform {
        parts.push("cmd");
    }
    if modifiers.control {
        parts.push("ctrl");
    }
    if modifiers.alt {
        parts.push("alt");
    }
    if modifiers.shift {
        parts.push("shift");
    }
    parts.push(&key);
    Some(parts.join("-"))
}

/// 正在处理的按键的物理键（Carbon 虚拟键码）和修饰键。
///
/// 不用 GPUI 的 `Keystroke`：按住 ⇧ 时它把 ⇧⌘3 报成 `cmd-#`（字符、且去掉了 ⇧），
/// 而全局热键按物理键注册。录制在 key down 回调里同步进行，`currentEvent` 就是这次按键。
fn current_press() -> Option<(u16, Modifiers)> {
    let mtm = MainThreadMarker::new()?;
    let event = NSApplication::sharedApplication(mtm).currentEvent()?;
    if event.r#type() != NSEventType::KeyDown {
        return None;
    }
    let flags = event.modifierFlags();
    Some((
        event.keyCode(),
        Modifiers {
            platform: flags.contains(NSEventModifierFlags::Command),
            control: flags.contains(NSEventModifierFlags::Control),
            alt: flags.contains(NSEventModifierFlags::Option),
            shift: flags.contains(NSEventModifierFlags::Shift),
            function: false,
        },
    ))
}

/// 物理键 + 修饰键 → 组合。表外的键（全局热键注册不了）、没有 ⌘ / ⌃ / ⌥ 的普通键
/// （F1–F20 除外；单独的字母或 ⇧+字母做全局热键会吞掉正常打字）不能用。
fn press_to_hotkey(modifiers: &Modifiers, vk: u16) -> Result<HotKey, Rejection> {
    let code = crate::hotkey::code_from_carbon(vk).ok_or(Rejection::Unsupported)?;
    let strong = modifiers.platform || modifiers.control || modifiers.alt;
    if !strong && !crate::hotkey::is_function_key(code) {
        return Err(Rejection::NeedsModifier);
    }
    let mut mods = HotKeyModifiers::empty();
    for (on, modifier) in [
        (modifiers.platform, HotKeyModifiers::META),
        (modifiers.control, HotKeyModifiers::CONTROL),
        (modifiers.alt, HotKeyModifiers::ALT),
        (modifiers.shift, HotKeyModifiers::SHIFT),
    ] {
        if on {
            mods |= modifier;
        }
    }
    Ok(HotKey::new(Some(mods), code))
}

/// 解析后的快捷键。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedHotkey {
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: String,
}

/// 解析配置字符串；接受常见别名（`option`、`command`、`control`），顺序不限。
/// 格式不对（空、只有修饰键、未知修饰键）返回 None。
pub fn parse_hotkey(text: &str) -> Option<ParsedHotkey> {
    let text = text.trim().to_ascii_lowercase();
    if text.is_empty() {
        return None;
    }
    // 键本身可能就是 `-`（如 `cmd--`）：最后一段为空时，键是 `-`。
    let (mods, key) = match text.strip_suffix("--") {
        Some(rest) => (rest, "-".to_string()),
        None => match text.rsplit_once('-') {
            Some((mods, key)) => (mods, key.to_string()),
            None => ("", text.clone()),
        },
    };
    if key.is_empty() || is_modifier_key(&key) {
        return None;
    }
    let mut parsed = ParsedHotkey {
        key,
        ..Default::default()
    };
    for part in mods.split('-').filter(|part| !part.is_empty()) {
        match part {
            "cmd" | "command" | "super" | "platform" | "meta" => parsed.cmd = true,
            "ctrl" | "control" => parsed.ctrl = true,
            "alt" | "option" | "opt" => parsed.alt = true,
            "shift" => parsed.shift = true,
            _ => return None,
        }
    }
    Some(parsed)
}

/// 配置字符串 → 键帽，按 macOS 顺序 ⌃⌥⇧⌘。解析失败时原样显示，让用户看得到坏值。
pub fn hotkey_keycaps(text: &str) -> Vec<String> {
    let Some(parsed) = parse_hotkey(text) else {
        return vec![text.to_string()];
    };
    let mut caps = Vec::new();
    if parsed.ctrl {
        caps.push("⌃".to_string());
    }
    if parsed.alt {
        caps.push("⌥".to_string());
    }
    if parsed.shift {
        caps.push("⇧".to_string());
    }
    if parsed.cmd {
        caps.push("⌘".to_string());
    }
    caps.push(key_label(&parsed.key));
    caps
}

/// 正在按住的修饰键（录制时实时显示）。
fn modifier_caps(modifiers: &Modifiers) -> Vec<String> {
    let mut caps = Vec::new();
    for (on, cap) in [
        (modifiers.control, "⌃"),
        (modifiers.alt, "⌥"),
        (modifiers.shift, "⇧"),
        (modifiers.platform, "⌘"),
    ] {
        if on {
            caps.push(cap.to_string());
        }
    }
    caps
}

fn key_label(key: &str) -> String {
    match key {
        "space" => "Space".into(),
        "enter" => "↵".into(),
        "tab" => "⇥".into(),
        "escape" => "Esc".into(),
        "backspace" => "⌫".into(),
        "delete" => "⌦".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "pageup" => "PgUp".into(),
        "pagedown" => "PgDn".into(),
        other => other.to_uppercase(),
    }
}

fn is_modifier_key(key: &str) -> bool {
    matches!(
        key,
        "cmd"
            | "command"
            | "super"
            | "platform"
            | "ctrl"
            | "control"
            | "alt"
            | "option"
            | "shift"
            | "fn"
            | "function"
    )
}

fn is_function_key(key: &str) -> bool {
    key.strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=20).contains(&n))
}

pub enum HotkeyEvent {
    /// 录到新的快捷键（已通过冲突检查）；None = 用户清除了快捷键。
    Changed(Option<String>),
    /// 恢复默认（已检查默认组合没有冲突）。
    Reset,
}

/// 录制框下方的一行提示，由所在页面决定放在哪（通用页放在控件下方，扩展表格放在整行下方）。
pub enum Note {
    /// 刚才按下的组合不能用，录制框仍在等待（红字）。
    Rejected(String),
    /// 已保存的快捷键没能生效（警示图标 + 原因）。
    Inactive(String),
}

/// 刚才按下的组合为什么没保存。文案在渲染时按当前语言取。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Rejection {
    /// 没有 ⌘ / ⌃ / ⌥（F1–F20 除外）。
    NeedsModifier,
    /// 表外的按键（全局热键注册不了）。
    Unsupported,
    /// 唤起快捷键不能清除。
    CannotClear,
    /// 和别的命令 / 系统 / 其他 App 冲突。`resetting`：点「恢复默认」时发现默认组合冲突。
    Conflict {
        glyphs: String,
        conflict: Conflict,
        resetting: bool,
    },
}

/// 录制框。值通过 `read` 每帧读取（通常读 `crate::config`），保证显示的就是落盘的值。
///
/// - 点击开始录制：显示「请按下快捷键…」（闪烁），按住修饰键时实时显示。
/// - Esc 取消；Backspace / Delete 清除（允许清除时）；失焦也算取消。
/// - 按下的组合与 Helmp 其他命令、系统快捷键、其他 App 冲突时不保存，停在录制态，用 [`Note`] 指名原因。
/// - 当前值不是默认值时，旁边有「恢复默认」小按钮。
pub struct HotkeyRecorder {
    focus: FocusHandle,
    command: Command,
    read: Rc<dyn Fn() -> Option<String>>,
    recording: bool,
    held: Modifiers,
    rejection: Option<Rejection>,
    /// 开始录制时读到的系统快捷键（用户可能刚在系统设置里改过，每次录制重读）。
    system: Vec<SystemShortcut>,
    allow_clear: bool,
    compact: bool,
    _blur: Subscription,
}

impl EventEmitter<HotkeyEvent> for HotkeyRecorder {}

impl HotkeyRecorder {
    pub fn new(
        command: Command,
        read: impl Fn() -> Option<String> + 'static,
        allow_clear: bool,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let blur = cx.on_blur(&focus, window, |this: &mut Self, _, cx| {
            // 点到别处 = 取消录制；必须恢复全局热键，否则 Helmp 会一直没有快捷键。
            this.end_recording();
            this.rejection = None;
            cx.notify();
        });
        Self {
            focus,
            command,
            read: Rc::new(read),
            recording: false,
            held: Modifiers::default(),
            rejection: None,
            system: Vec::new(),
            allow_clear,
            compact,
            _blur: blur,
        }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.recording {
            crate::hotkey::suspend();
            self.recording = true;
        }
        self.system = conflict::load_system_shortcuts();
        self.rejection = None;
        self.held = Modifiers::default();
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// 退出录制态并恢复全局热键（只恢复自己挂起的那一次）。
    fn end_recording(&mut self) {
        if self.recording {
            crate::hotkey::resume();
            self.recording = false;
        }
        self.held = Modifiers::default();
    }

    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end_recording();
        self.rejection = None;
        window.blur(cx);
        cx.notify();
    }

    fn reject(&mut self, rejection: Rejection, cx: &mut Context<Self>) {
        self.rejection = Some(rejection);
        self.held = Modifiers::default();
        cx.notify();
    }

    fn check(&self, hotkey: HotKey, system: &[SystemShortcut]) -> Result<(), Conflict> {
        conflict::check(
            hotkey,
            self.command,
            &crate::hotkey::assigned(&config::get()),
            system,
            conflict::probe,
        )
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.recording {
            return;
        }
        cx.stop_propagation();
        let keystroke = &event.keystroke;
        let plain = !keystroke.modifiers.modified();
        match keystroke.key.as_str() {
            "escape" if plain => return self.finish(window, cx),
            "backspace" | "delete" if plain => {
                if !self.allow_clear {
                    return self.reject(Rejection::CannotClear, cx);
                }
                cx.emit(HotkeyEvent::Changed(None));
                return self.finish(window, cx);
            }
            _ => {}
        }
        let pressed = match current_press() {
            Some((vk, modifiers)) => press_to_hotkey(&modifiers, vk),
            // 拿不到原生事件（理论上不会）：退回 GPUI 的按键名，⇧ + 数字 / 符号会认不出来。
            None => keystroke_to_hotkey(&keystroke.modifiers, &keystroke.key)
                .ok_or(Rejection::NeedsModifier)
                .and_then(|spec| crate::hotkey::parse(&spec).map_err(|_| Rejection::Unsupported)),
        };
        let hotkey = match pressed {
            Ok(hotkey) => hotkey,
            Err(rejection) => return self.reject(rejection, cx),
        };
        let Some(spec) = crate::hotkey::to_spec(&hotkey) else {
            return self.reject(Rejection::Unsupported, cx);
        };
        match self.check(hotkey, &self.system) {
            Ok(()) => {
                let unchanged = (self.read)().and_then(|value| crate::hotkey::parse(&value).ok())
                    == Some(hotkey);
                if !unchanged {
                    cx.emit(HotkeyEvent::Changed(Some(spec)));
                }
                self.finish(window, cx);
            }
            Err(conflict) => self.reject(
                Rejection::Conflict {
                    glyphs: crate::hotkey::glyphs(&hotkey),
                    conflict,
                    resetting: false,
                },
                cx,
            ),
        }
    }

    fn on_modifiers(
        &mut self,
        event: &ModifiersChangedEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.recording {
            self.held = event.modifiers;
            cx.notify();
        }
    }

    /// 「恢复默认」：默认组合同样要过冲突检查（可能已被用户分给了别的命令）。
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hotkey) = crate::hotkey::default_spec(self.command)
            .and_then(|spec| crate::hotkey::parse(spec).ok())
        else {
            return;
        };
        let system = conflict::load_system_shortcuts();
        match self.check(hotkey, &system) {
            Ok(()) => {
                cx.emit(HotkeyEvent::Reset);
                self.finish(window, cx);
            }
            Err(conflict) => {
                self.end_recording();
                self.reject(
                    Rejection::Conflict {
                        glyphs: crate::hotkey::glyphs(&hotkey),
                        conflict,
                        resetting: true,
                    },
                    cx,
                );
            }
        }
    }

    /// 当前值不是默认值时才显示「恢复默认」。
    fn differs_from_default(&self, value: Option<&str>) -> bool {
        let Some(default) = crate::hotkey::default_spec(self.command) else {
            return false;
        };
        value.and_then(|value| crate::hotkey::parse(value).ok())
            != crate::hotkey::parse(default).ok()
    }

    /// 录制框下方要显示的提示：刚被拒绝的原因优先，其次是已保存的快捷键没生效的原因。
    pub fn note(&self) -> Option<Note> {
        if let Some(rejection) = &self.rejection {
            let text = match rejection {
                Rejection::NeedsModifier => tr(
                    "需要包含 ⌘、⌃ 或 ⌥（F1–F20 除外），请重新按下",
                    "Include ⌘, ⌃ or ⌥ (F1–F20 work alone). Try again.",
                )
                .to_string(),
                Rejection::Unsupported => tr(
                    "不支持这个按键，请换一个组合",
                    "This key isn’t supported. Try another combination.",
                )
                .to_string(),
                Rejection::CannotClear => tr(
                    "唤起快捷键不能清除；请按下新的组合，或按 Esc 取消",
                    "The launcher shortcut can’t be cleared. Press a new one, or Esc to cancel.",
                )
                .to_string(),
                Rejection::Conflict {
                    glyphs,
                    conflict,
                    resetting: false,
                } => trf!(
                    "{glyphs} {}，请换一个",
                    "{glyphs}: {}. Try another.",
                    conflict.message()
                ),
                Rejection::Conflict {
                    glyphs,
                    conflict,
                    resetting: true,
                } => trf!(
                    "无法恢复默认：{glyphs} {}",
                    "Can’t restore the default: {glyphs}: {}",
                    conflict.message()
                ),
            };
            return Some(Note::Rejected(text));
        }
        if self.recording {
            return None;
        }
        crate::hotkey::issue(self.command).map(|issue| Note::Inactive(issue.message()))
    }

    pub fn render_with(&mut self, pal: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let value = (self.read)();
        let inactive = !self.recording && crate::hotkey::issue(self.command).is_some();

        let mut field = div()
            .id("hotkey-recorder")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_modifiers_changed(cx.listener(Self::on_modifiers))
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                if this.recording {
                    this.finish(window, cx);
                } else {
                    this.start(window, cx);
                }
            }))
            .cursor_pointer()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .h(px(if self.compact { 24. } else { 26. }))
            .pl(px(if self.compact { 6. } else { 10. }))
            .pr(px(6.))
            .rounded(px(7.))
            .bg(pal.control)
            .border_1()
            .border_color(if self.recording {
                pal.accent
            } else if inactive || self.rejection.is_some() {
                pal.danger
            } else {
                pal.separator
            });

        if self.recording {
            let held = modifier_caps(&self.held);
            let waiting = if !held.is_empty() {
                "…"
            } else if self.compact {
                tr("按下…", "Press…")
            } else {
                tr("请按下快捷键…", "Press a shortcut…")
            };
            for cap in held {
                field = field.child(keycap(cap, pal));
            }
            field = field.child(
                div()
                    .text_size(px(12.))
                    .text_color(pal.accent)
                    .whitespace_nowrap()
                    .child(waiting)
                    .with_animation(
                        "hotkey-waiting",
                        Animation::new(Duration::from_millis(1100))
                            .repeat()
                            .with_easing(pulsating_between(0.3, 1.0)),
                        |el, delta| el.opacity(delta),
                    ),
            );
        } else {
            match value.as_deref() {
                Some(value) => {
                    for cap in hotkey_keycaps(value) {
                        field = field.child(keycap(cap, pal));
                    }
                }
                None => {
                    field = field.child(
                        div()
                            .text_size(px(12.))
                            .text_color(pal.muted)
                            .whitespace_nowrap()
                            .child(if self.compact {
                                tr("未设置", "None")
                            } else {
                                tr("未设置 · 点按录制", "Not set · Click to record")
                            }),
                    )
                }
            }
        }
        if !self.recording && self.allow_clear && value.is_some() {
            field = field.child(
                div()
                    .id("hotkey-clear")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(16.))
                    .ml(px(2.))
                    .rounded_full()
                    .bg(pal.muted.opacity(0.45))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.rejection = None;
                        cx.emit(HotkeyEvent::Changed(None));
                        cx.notify();
                    }))
                    .tooltip(|window, cx| {
                        Tooltip::new(tr("清除快捷键", "Clear shortcut")).build(window, cx)
                    })
                    .child(
                        gpui_kit::component::Icon::empty()
                            .path("icons/x.svg")
                            .text_color(gpui_kit::white())
                            .size(px(10.)),
                    ),
            );
        }

        let mut control = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .child(field);
        if !self.recording && self.differs_from_default(value.as_deref()) {
            let default = crate::hotkey::default_spec(self.command)
                .and_then(|spec| crate::hotkey::parse(spec).ok())
                .map(|hotkey| crate::hotkey::glyphs(&hotkey))
                .unwrap_or_default();
            control = control.child(
                div()
                    .id("hotkey-reset")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(20.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(|el| el.bg(pal.key_bg))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.reset(window, cx);
                    }))
                    .tooltip(move |window, cx| {
                        Tooltip::new(trf!("恢复默认（{default}）", "Restore default ({default})"))
                            .build(window, cx)
                    })
                    .child(
                        gpui_kit::component::Icon::empty()
                            .path("icons/undo-2.svg")
                            .text_color(pal.muted)
                            .size(px(13.)),
                    ),
            );
        }
        control.into_any_element()
    }
}

/// 录制中途窗口被关掉时也要恢复全局热键，否则 Helmp 会一直没有快捷键。
impl Drop for HotkeyRecorder {
    fn drop(&mut self) {
        self.end_recording();
    }
}

impl Render for HotkeyRecorder {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = crate::style::palette(window);
        self.render_with(&pal, cx)
    }
}

/// 录制框下方的一行提示。
pub fn note_line(note: &Note, pal: &Palette) -> Div {
    let (text, icon) = match note {
        Note::Rejected(text) => (text.clone(), false),
        Note::Inactive(text) => (text.clone(), true),
    };
    let mut line = div()
        .flex()
        .items_start()
        .gap(px(4.))
        .text_size(px(11.5))
        .line_height(px(16.))
        .text_color(pal.danger);
    if icon {
        line = line.child(
            div().flex_none().pt(px(2.)).child(
                gpui_kit::component::Icon::empty()
                    .path("icons/triangle-alert.svg")
                    .text_color(pal.danger)
                    .size(px(12.)),
            ),
        );
    }
    line.child(div().min_w_0().child(text))
}

/// 录制框 + 下方提示，给「左标签、右控件」的行用（通用页的唤起快捷键）。
pub fn with_note(recorder: &Entity<HotkeyRecorder>, pal: &Palette, cx: &App) -> Div {
    let mut column = div()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(4.))
        .child(recorder.clone());
    if let Some(note) = recorder.read(cx).note() {
        column = column.child(note_line(&note, pal).max_w(px(360.)));
    }
    column
}

/// 键帽，与设计稿 `.k` 一致。
pub fn keycap(text: impl Into<SharedString>, pal: &Palette) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .min_w(px(20.))
        .h(px(20.))
        .px(px(5.))
        .rounded(px(5.))
        .bg(pal.key_bg)
        .text_color(pal.key_fg)
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

#[cfg(test)]
mod tests {
    use super::{Rejection, hotkey_keycaps, keystroke_to_hotkey, parse_hotkey, press_to_hotkey};
    use gpui_kit::Modifiers;

    fn mods(cmd: bool, ctrl: bool, alt: bool, shift: bool) -> Modifiers {
        Modifiers {
            platform: cmd,
            control: ctrl,
            alt,
            shift,
            function: false,
        }
    }

    #[test]
    fn records_canonical_config_strings() {
        // 菜单栏子任务按这个字符串重新注册全局热键，格式不稳定就会注册失败或注册错键。
        assert_eq!(
            keystroke_to_hotkey(&mods(false, false, true, false), "space").as_deref(),
            Some("alt-space")
        );
        assert_eq!(
            keystroke_to_hotkey(&mods(true, false, false, true), "space").as_deref(),
            Some("cmd-shift-space")
        );
        assert_eq!(
            keystroke_to_hotkey(&mods(true, true, true, true), "K").as_deref(),
            Some("cmd-ctrl-alt-shift-k")
        );
    }

    #[test]
    fn rejects_hotkeys_that_would_swallow_typing() {
        // 单独的字母、⇧+字母做全局热键，用户在任何 App 里都打不出这个字。
        assert_eq!(
            keystroke_to_hotkey(&mods(false, false, false, false), "a"),
            None
        );
        assert_eq!(
            keystroke_to_hotkey(&mods(false, false, false, true), "a"),
            None
        );
        // 只按修饰键时还没录完，不能写盘。
        assert_eq!(
            keystroke_to_hotkey(&mods(true, false, false, false), "cmd"),
            None
        );
        // 功能键本身不会产生字符，可以单独用。
        assert_eq!(
            keystroke_to_hotkey(&mods(false, false, false, false), "f5").as_deref(),
            Some("f5")
        );
    }

    #[test]
    fn recorded_strings_round_trip_through_parser() {
        // 录制写下的字符串必须能被读回同样的组合，否则设置页显示的和实际注册的不是一个键。
        for (m, key) in [
            (mods(false, false, true, false), "space"),
            (mods(true, false, false, true), "v"),
            (mods(true, true, false, false), "-"),
        ] {
            let text = keystroke_to_hotkey(&m, key).unwrap();
            let parsed = parse_hotkey(&text).unwrap();
            assert_eq!(
                (parsed.cmd, parsed.ctrl, parsed.alt, parsed.shift),
                (m.platform, m.control, m.alt, m.shift),
                "{text}"
            );
            assert_eq!(parsed.key, key);
        }
    }

    #[test]
    fn parser_accepts_aliases_and_rejects_garbage() {
        // config.json 允许手改：常见写法要认，坏值要识别出来而不是当成某个键。
        let parsed = parse_hotkey("Option-Command-J").unwrap();
        assert!(parsed.alt && parsed.cmd && !parsed.shift);
        assert_eq!(parsed.key, "j");
        assert_eq!(parse_hotkey(""), None);
        assert_eq!(parse_hotkey("cmd-shift"), None);
        assert_eq!(parse_hotkey("hyper-k"), None);
    }

    #[test]
    fn shows_keycaps_in_macos_order() {
        // 设计稿与系统菜单都按 ⌃⌥⇧⌘ 排列，配置里的顺序不同也要显示一致。
        assert_eq!(hotkey_keycaps("alt-space"), ["⌥", "Space"]);
        assert_eq!(hotkey_keycaps("cmd-shift-v"), ["⇧", "⌘", "V"]);
        assert_eq!(hotkey_keycaps("shift-cmd-2"), ["⇧", "⌘", "2"]);
        // 坏值原样显示，用户能看到并重新录制。
        assert_eq!(hotkey_keycaps("cmd-"), ["cmd-"]);
    }

    #[test]
    fn records_physical_keys_so_shifted_digits_work() {
        // GPUI 把 ⇧⌘3 报成 `cmd-#`：按字符录的话截屏组合根本录不出来、也就提示不了「已被系统使用」，
        // 默认的 ⇧⌘2 也录不回去。按物理键（kVK_ANSI_3 = 0x14）录才对。
        let shift_cmd_3 = press_to_hotkey(&mods(true, false, false, true), 0x14).unwrap();
        assert_eq!(
            crate::hotkey::to_spec(&shift_cmd_3).as_deref(),
            Some("cmd-shift-3")
        );
        assert_eq!(
            crate::hotkey::glyphs(
                &press_to_hotkey(&mods(false, false, true, false), 0x31).unwrap()
            ),
            "⌥Space"
        );
    }

    #[test]
    fn physical_press_rules_match_typing_safety() {
        // 不带 ⌘ / ⌃ / ⌥ 的字母（含 ⇧+字母）会吞掉打字，必须拒绝并说明要加修饰键。
        assert_eq!(
            press_to_hotkey(&mods(false, false, false, false), 0x00),
            Err(Rejection::NeedsModifier)
        );
        assert_eq!(
            press_to_hotkey(&mods(false, false, false, true), 0x00),
            Err(Rejection::NeedsModifier)
        );
        // F5（0x60）、F20（0x5A）单独可用。
        assert!(press_to_hotkey(&mods(false, false, false, false), 0x60).is_ok());
        assert!(press_to_hotkey(&mods(false, false, false, false), 0x5A).is_ok());
        // 表外的键（如小键盘 0x52）注册不了，要明说不支持而不是假装录上。
        assert_eq!(
            press_to_hotkey(&mods(true, false, false, false), 0x52),
            Err(Rejection::Unsupported)
        );
    }
}
