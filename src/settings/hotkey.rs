//! 快捷键录制框：点一下进入录制，按下组合键写回配置。
//!
//! 配置里的格式同 GPUI keystroke：小写、`-` 连接，修饰键固定按 `cmd-ctrl-alt-shift` 排在前面，
//! 如 `alt-space`、`cmd-shift-space`。界面上按 macOS 习惯显示成 ⌃⌥⇧⌘ 键帽。
//!
//! 录制框不直接写配置：它发出 `HotkeyEvent::Changed`，由设置窗口写盘（失败时在窗口里报错）。

use std::rc::Rc;

use gpui_kit::*;

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
    /// 录到新的快捷键；None = 用户清除了快捷键。
    Changed(Option<String>),
}

/// 录制框。值通过 `read` 每帧读取（通常读 `crate::config`），保证显示的就是落盘的值。
pub struct HotkeyRecorder {
    focus: FocusHandle,
    read: Rc<dyn Fn() -> Option<String>>,
    recording: bool,
    held: Modifiers,
    /// 按了不能用的组合时给出的提示。
    hint: Option<&'static str>,
    allow_clear: bool,
    compact: bool,
    _blur: Subscription,
}

impl EventEmitter<HotkeyEvent> for HotkeyRecorder {}

impl HotkeyRecorder {
    pub fn new(
        read: impl Fn() -> Option<String> + 'static,
        allow_clear: bool,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let blur = cx.on_blur(&focus, window, |this: &mut Self, _, cx| {
            this.recording = false;
            this.hint = None;
            this.held = Modifiers::default();
            cx.notify();
        });
        Self {
            focus,
            read: Rc::new(read),
            recording: false,
            held: Modifiers::default(),
            hint: None,
            allow_clear,
            compact,
            _blur: blur,
        }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::hotkey::set_suspended(true);
        self.recording = true;
        self.hint = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::hotkey::set_suspended(false);
        self.recording = false;
        self.hint = None;
        self.held = Modifiers::default();
        window.blur(cx);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.recording {
            return;
        }
        cx.stop_propagation();
        let keystroke = &event.keystroke;
        let plain = !keystroke.modifiers.modified();
        match keystroke.key.as_str() {
            "escape" if plain => return self.stop(window, cx),
            "backspace" | "delete" if plain && self.allow_clear => {
                cx.emit(HotkeyEvent::Changed(None));
                return self.stop(window, cx);
            }
            _ => {}
        }
        match keystroke_to_hotkey(&keystroke.modifiers, &keystroke.key) {
            Some(hotkey) => {
                cx.emit(HotkeyEvent::Changed(Some(hotkey)));
                self.stop(window, cx);
            }
            None => {
                self.hint = Some("需要包含 ⌘、⌃ 或 ⌥");
                cx.notify();
            }
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

    pub fn render_with(&mut self, pal: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let value = (self.read)();
        let caps = if self.recording {
            modifier_caps(&self.held)
        } else {
            value.as_deref().map(hotkey_keycaps).unwrap_or_default()
        };
        let placeholder = if self.recording {
            self.hint.unwrap_or(if self.compact {
                "按下…"
            } else {
                "按下快捷键…"
            })
        } else if self.compact {
            "—"
        } else {
            "点按以录制"
        };

        let mut field = div()
            .id("hotkey-recorder")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_modifiers_changed(cx.listener(Self::on_modifiers))
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                if this.recording {
                    this.stop(window, cx);
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
            } else {
                pal.separator
            });
        if caps.is_empty() || (self.recording && self.hint.is_some()) {
            field = field.child(
                div()
                    .text_size(px(12.))
                    .text_color(if self.hint.is_some() {
                        pal.danger
                    } else {
                        pal.muted
                    })
                    .whitespace_nowrap()
                    .child(placeholder),
            );
        }
        if !(self.recording && self.hint.is_some()) {
            for cap in caps {
                field = field.child(keycap(cap, pal));
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
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        cx.emit(HotkeyEvent::Changed(None));
                        cx.notify();
                    }))
                    .child(
                        gpui_kit::component::Icon::empty()
                            .path("icons/x.svg")
                            .text_color(gpui_kit::white())
                            .size(px(10.)),
                    ),
            );
        }
        field.into_any_element()
    }
}

/// 录制中途窗口被关掉时也要恢复全局热键，否则 Helmp 会一直没有快捷键。
impl Drop for HotkeyRecorder {
    fn drop(&mut self) {
        if self.recording {
            crate::hotkey::set_suspended(false);
        }
    }
}

impl Render for HotkeyRecorder {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = crate::style::palette(window);
        self.render_with(&pal, cx)
    }
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
    use super::{hotkey_keycaps, keystroke_to_hotkey, parse_hotkey};
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
}
