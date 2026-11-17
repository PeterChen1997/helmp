//! 截图与钉图共用的图标按钮。标注工具及其选项只出现在截图遮罩里。

use std::rc::Rc;

use gpui_kit::component::{
    Sizable,
    input::{Input, InputState},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::annotation::{BLOCK_SIZES, COLORS, Editor, Tool, WIDTHS};
use super::record::Audio;
use super::{Act, Tab};
use crate::i18n::{tr, trf};
use crate::style::Palette;

const BUTTON: f32 = 28.;
const GAP: f32 = 2.;
const PAD: f32 = 4.;
const DIVIDER: f32 = 1.;
const OPTIONS_GAP: f32 = 4.;
const ROW_HEIGHT: f32 = BUTTON + PAD * 2. + 2.;
/// 「截图 | 录屏」切换里每一格的宽度（按界面语言，英文字更长）。
fn tab_width() -> f32 {
    if crate::i18n::is_en() { 56. } else { 44. }
}
/// 录屏工具栏上「系统声音」「麦克风」两个开关和「开始录制」按钮的宽度。
fn record_widths() -> (f32, f32, f32) {
    if crate::i18n::is_en() {
        (118., 104., 136.)
    } else {
        (96., 82., 96.)
    }
}
const RED: u32 = 0xFF3B30;

pub const SAVE: ActionButton = (Act::Save, "download", "保存", "Save", "⌘S");
pub const RECOGNIZE: ActionButton = (
    Act::Recognize,
    "scan-text",
    "识别文字",
    "Recognize Text",
    "",
);
pub const PIN: ActionButton = (Act::Pin, "pin", "钉在屏幕上", "Pin to Screen", "");
pub const COPY: ActionButton = (Act::Copy, "copy", "复制", "Copy", "⌘C");
/// 关闭不在 [`ACTIONS`] 里：它固定在工具栏最左边，用分隔线和其他按钮隔开。
const CLOSE: ActionButton = (Act::Close, "x", "关闭", "Close", "Esc");

/// 遮罩工具栏右侧的操作，复制（主操作）在最右。
pub const ACTIONS: [ActionButton; 4] = [SAVE, RECOGNIZE, PIN, COPY];

const TOOLS: [(Option<Tool>, &str, &str, &str, &str); 8] = [
    (None, "move", "调整选区", "Adjust Selection", "v"),
    (
        Some(Tool::Rectangle),
        "square",
        "矩形框选",
        "Rectangle",
        "r",
    ),
    (Some(Tool::Ellipse), "circle", "椭圆", "Ellipse", "o"),
    (Some(Tool::Line), "minus", "直线", "Line", "l"),
    (Some(Tool::Arrow), "arrow-up-right", "箭头", "Arrow", "a"),
    (Some(Tool::Pen), "pencil", "画笔", "Pen", "p"),
    (
        Some(Tool::Highlighter),
        "highlighter",
        "荧光笔",
        "Highlighter",
        "h",
    ),
    (
        Some(Tool::Mosaic),
        "grid-3x3",
        "框选马赛克",
        "Rectangular Mosaic",
        "m",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    Select(Option<Tool>),
    Undo,
    Redo,
    Color(u32),
    Width(usize),
    BlockSize(usize),
    StepSize(isize),
    /// 切换「截图 | 录屏」。
    Tab(Tab),
    /// 开关「录系统声音」「录麦克风」（记进配置）。
    SystemAudio,
    Microphone,
}

pub type ActionButton = (Act, &'static str, &'static str, &'static str, &'static str);
pub type OnPress = Rc<dyn Fn(Act, &mut Window, &mut App)>;
pub type OnEdit = Rc<dyn Fn(Edit, &mut Window, &mut App)>;

pub fn key_tool(key: &str) -> Option<Edit> {
    match key {
        "1" => return Some(Edit::StepSize(-1)),
        "2" => return Some(Edit::StepSize(1)),
        _ => {}
    }
    TOOLS
        .iter()
        .find(|entry| entry.4 == key)
        .map(|entry| Edit::Select(entry.0))
}

pub fn size(tab: Tab) -> (f64, f64) {
    let segment = 2. * tab_width() + 2. * SEGMENT_PAD;
    let width = match tab {
        // 关闭 | 截图/录屏 | 工具 | 撤销 重做 | 操作：四条分隔线。
        Tab::Shot => {
            let buttons = (1 + TOOLS.len() + 2 + ACTIONS.len()) as f32;
            let dividers = 4.;
            buttons * BUTTON
                + segment
                + (buttons + 1. + dividers - 1.) * GAP
                + dividers * DIVIDER
                + 2. * (PAD + 1.)
        }
        // 关闭 | 截图/录屏 | 系统声音 麦克风 | 开始录制：三条分隔线。
        Tab::Record => {
            let (system, microphone, start) = record_widths();
            let items = 5.;
            let dividers = 3.;
            BUTTON
                + segment
                + system
                + microphone
                + start
                + (items + dividers - 1.) * GAP
                + dividers * DIVIDER
                + 2. * (PAD + 1.)
        }
    };
    (f64::from(width), f64::from(ROW_HEIGHT))
}

const SEGMENT_PAD: f32 = 2.;

/// 「截图 | 录屏」分段切换。
fn tab_switch(tab: Tab, pal: &Palette, on_edit: &OnEdit) -> Div {
    let item = |target: Tab, zh: &str, en: &str| {
        let on = target == tab;
        let on_edit = on_edit.clone();
        div()
            .id(SharedString::from(format!("capture-tab-{target:?}")))
            .w(px(tab_width()))
            .h(px(BUTTON - 2. * SEGMENT_PAD))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .text_size(px(12.))
            .when(on, |d| d.bg(pal.window).shadow_sm().text_color(pal.fg))
            .when(!on, |d| {
                d.text_color(pal.fg.opacity(0.6))
                    .cursor(CursorStyle::PointingHand)
                    .hover(|style| style.text_color(pal.fg))
            })
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                on_edit(Edit::Tab(target), window, cx);
            })
            .child(tr(zh, en).to_string())
    };
    div()
        .flex()
        .flex_shrink_0()
        .p(px(SEGMENT_PAD))
        .rounded(px(6.))
        .bg(pal.key_bg)
        .child(item(Tab::Shot, "截图", "Capture"))
        .child(item(Tab::Record, "录屏", "Record"))
}

/// 声音开关（图标 + 文字）。开着时是强调色描边，关着是灰的划线图标。
fn audio_toggle(
    edit: Edit,
    on: bool,
    width: f32,
    pal: &Palette,
    on_edit: &OnEdit,
) -> Stateful<Div> {
    let (name, zh, en) = match (edit, on) {
        (Edit::Microphone, true) => ("mic", "麦克风", "Microphone"),
        (Edit::Microphone, false) => ("mic-off", "麦克风", "Microphone"),
        (_, true) => ("volume-2", "系统声音", "System Audio"),
        (_, false) => ("volume-x", "系统声音", "System Audio"),
    };
    let tip = match (edit, on) {
        (Edit::Microphone, true) => tr("录制麦克风：开", "Record microphone: on"),
        (Edit::Microphone, false) => tr("录制麦克风：关", "Record microphone: off"),
        (_, true) => tr("录制系统声音：开", "Record system audio: on"),
        (_, false) => tr("录制系统声音：关", "Record system audio: off"),
    };
    let color = if on { pal.accent } else { pal.fg.opacity(0.55) };
    let on_edit = on_edit.clone();
    div()
        .id(SharedString::from(format!("capture-{edit:?}")))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .gap(px(5.))
        .w(px(width))
        .h(px(BUTTON))
        .rounded(px(5.))
        .cursor(CursorStyle::PointingHand)
        .when(on, |d| d.bg(pal.key_bg).border_1().border_color(pal.accent))
        .when(!on, |d| d.hover(|style| style.bg(pal.key_bg)))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            on_edit(edit, window, cx);
        })
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
        })
        .child(icon(name, color))
        .child(
            div()
                .text_size(px(12.))
                .text_color(if on { pal.accent } else { pal.fg.opacity(0.7) })
                .child(tr(zh, en).to_string()),
        )
}

/// 「区域录屏」快捷键的框选提示条上的声音开关（深色底，白字）。
pub fn hint_audio_toggles(audio: Audio, on_edit: OnEdit) -> Vec<Stateful<Div>> {
    [
        (Edit::SystemAudio, audio.system),
        (Edit::Microphone, audio.microphone),
    ]
    .into_iter()
    .map(|(edit, on)| {
        let (name, label) = match (edit, on) {
            (Edit::Microphone, true) => ("mic", tr("麦克风", "Microphone")),
            (Edit::Microphone, false) => ("mic-off", tr("麦克风", "Microphone")),
            (_, true) => ("volume-2", tr("系统声音", "System Audio")),
            (_, false) => ("volume-x", tr("系统声音", "System Audio")),
        };
        let color = if on { white() } else { white().opacity(0.5) };
        let on_edit = on_edit.clone();
        div()
            .id(SharedString::from(format!("hint-{edit:?}")))
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(8.))
            .h(px(22.))
            .rounded(px(11.))
            .cursor(CursorStyle::PointingHand)
            .when(on, |d| d.bg(white().opacity(0.18)))
            .hover(|style| style.bg(white().opacity(0.26)))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                on_edit(edit, window, cx);
            })
            .child(icon(name, color).size(px(14.)))
            .child(div().text_color(color).child(label.to_string()))
    })
    .collect()
}

/// 录屏工具栏右边的红色「开始录制」。
fn start_button(width: f32, on_press: &OnPress) -> Stateful<Div> {
    let on_press = on_press.clone();
    div()
        .id("capture-start-recording")
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .w(px(width))
        .h(px(BUTTON))
        .rounded(px(BUTTON / 2.))
        .bg(rgb(RED))
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.opacity(0.88))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            on_press(Act::Record, window, cx);
        })
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tooltip("开始录制", "Start Recording", "↵"))
                .build(window, cx)
        })
        .child(div().size(px(9.)).rounded_full().bg(white()))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(white())
                .child(tr("开始录制", "Start Recording").to_string()),
        )
}

/// 选项贴在主工具栏旁，空间不足时改放上方。
pub fn options_offset(top: f64, bottom: f64) -> Pixels {
    let offset = ROW_HEIGHT + OPTIONS_GAP;
    if top + f64::from(offset + ROW_HEIGHT) <= bottom {
        px(offset)
    } else {
        px((-top as f32).max(-offset))
    }
}

fn row(pal: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(GAP))
        .p(px(PAD))
        .rounded(px(8.))
        .bg(pal.window)
        .border_1()
        .border_color(pal.separator)
        .shadow_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
}

fn button(
    id: String,
    tip: String,
    selected: bool,
    enabled: bool,
    pal: &Palette,
    on_press: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .justify_center()
        .size(px(BUTTON))
        .flex_shrink_0()
        .rounded(px(5.))
        .when(selected, |d| {
            d.bg(pal.key_bg).border_1().border_color(pal.accent)
        })
        .when(!enabled, |d| d.opacity(0.42))
        .when(enabled, |d| {
            d.cursor(CursorStyle::PointingHand)
                .hover(|style| style.bg(pal.key_bg))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    on_press(window, cx);
                })
        })
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
        })
}

fn icon(name: &str, color: Hsla) -> gpui_kit::component::Icon {
    gpui_kit::component::Icon::empty()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .text_color(color)
        .size(px(17.))
}

fn tooltip(zh: &str, en: &str, keys: &str) -> String {
    let label = tr(zh, en);
    if keys.is_empty() {
        label.to_string()
    } else {
        trf!("{label}（{keys}）", "{label} ({keys})")
    }
}

fn action_buttons(
    buttons: &[ActionButton],
    primary: Act,
    pal: &Palette,
    on_press: &OnPress,
) -> Vec<Stateful<Div>> {
    buttons
        .iter()
        .map(|&(action, name, zh, en, keys)| {
            let is_primary = action == primary;
            let keys = if is_primary {
                if action == Act::Copy {
                    "↵ / ⌘C"
                } else {
                    "↵ / ⌘S"
                }
            } else {
                keys
            };
            let on_press = on_press.clone();
            button(
                format!("capture-{action:?}"),
                tooltip(zh, en, keys),
                false,
                true,
                pal,
                move |window, cx| on_press(action, window, cx),
            )
            .child(icon(name, if is_primary { pal.accent } else { pal.fg }))
        })
        .collect()
}

/// 关闭在最左，后面一条分隔线，再接 `buttons`。
pub fn button_row(buttons: &[ActionButton], primary: Act, pal: &Palette, on_press: OnPress) -> Div {
    row(pal)
        .children(action_buttons(&[CLOSE], primary, pal, &on_press))
        .child(divider(pal))
        .children(action_buttons(buttons, primary, pal, &on_press))
}

fn divider(pal: &Palette) -> Div {
    div()
        .w(px(DIVIDER))
        .h(px(18.))
        .flex_shrink_0()
        .bg(pal.separator)
}

fn edit_button(
    name: &str,
    tip: String,
    edit: Edit,
    selected: bool,
    enabled: bool,
    pal: &Palette,
    on_edit: &OnEdit,
) -> Stateful<Div> {
    let on_edit = on_edit.clone();
    button(
        format!("edit-{name}"),
        tip,
        selected,
        enabled,
        pal,
        move |window, cx| on_edit(edit, window, cx),
    )
}

fn options(
    editor: &Editor,
    tool: Tool,
    hex: &Entity<InputState>,
    pal: &Palette,
    on_edit: &OnEdit,
) -> Div {
    let mut options = row(pal);
    if tool != Tool::Mosaic {
        for (color, zh, en) in COLORS {
            options = options.child(
                edit_button(
                    en,
                    tr(zh, en).to_string(),
                    Edit::Color(color),
                    color == editor.color(),
                    true,
                    pal,
                    on_edit,
                )
                .child(
                    div()
                        .size(px(15.))
                        .rounded_full()
                        .bg(rgb(color))
                        .border_1()
                        .border_color(pal.separator),
                ),
            );
        }
        options = options
            .child(
                div()
                    .w(px(66.))
                    .flex_shrink_0()
                    .child(Input::new(hex).xsmall()),
            )
            .child(divider(pal));
    }
    let sizes = if tool == Tool::Mosaic {
        BLOCK_SIZES
    } else {
        WIDTHS
    };
    for (index, width) in sizes.into_iter().enumerate() {
        let (edit, selected, tip) = match tool {
            Tool::Mosaic => (
                Edit::BlockSize(index),
                index == editor.block_size,
                trf!("色块 {width} 点", "{width} pt blocks"),
            ),
            Tool::Highlighter => {
                let width = width * 4.;
                (
                    Edit::Width(index),
                    index == editor.width,
                    trf!("笔宽 {width} 点", "{width} pt stroke"),
                )
            }
            _ => (
                Edit::Width(index),
                index == editor.width,
                trf!("笔宽 {width} 点", "{width} pt stroke"),
            ),
        };
        let name = format!("size-{index}");
        let sample = if tool == Tool::Mosaic {
            div()
                .size(px(5. + index as f32 * 4.))
                .border_1()
                .border_color(pal.fg)
        } else {
            div()
                .w(px(16.))
                .h(px(width as f32))
                .rounded_full()
                .bg(pal.fg)
        };
        options = options
            .child(edit_button(&name, tip, edit, selected, true, pal, on_edit).child(sample));
    }
    options
}

#[allow(clippy::too_many_arguments)]
pub fn editor_toolbar(
    editor: &Editor,
    tab: Tab,
    audio: Audio,
    primary: Act,
    hex: &Entity<InputState>,
    options_top: Pixels,
    pal: &Palette,
    on_edit: OnEdit,
    on_press: OnPress,
) -> Div {
    if tab == Tab::Record {
        let (system, microphone, start) = record_widths();
        return row(pal)
            .children(action_buttons(&[CLOSE], primary, pal, &on_press))
            .child(divider(pal))
            .child(tab_switch(tab, pal, &on_edit))
            .child(divider(pal))
            .child(audio_toggle(Edit::SystemAudio, audio.system, system, pal, &on_edit))
            .child(audio_toggle(Edit::Microphone, audio.microphone, microphone, pal, &on_edit))
            .child(divider(pal))
            .child(start_button(start, &on_press));
    }
    let tools = TOOLS.map(|(tool, name, zh, en, key)| {
        let selected = tool == editor.tool;
        edit_button(
            name,
            tooltip(zh, en, &key.to_ascii_uppercase()),
            Edit::Select(tool),
            selected,
            true,
            pal,
            &on_edit,
        )
        .child(icon(name, if selected { pal.accent } else { pal.fg }))
    });
    let undo = edit_button(
        "undo",
        tooltip("撤销", "Undo", "⌘Z"),
        Edit::Undo,
        false,
        editor.can_undo(),
        pal,
        &on_edit,
    )
    .child(icon("undo-2", pal.fg));
    let redo = edit_button(
        "redo",
        tooltip("重做", "Redo", "⇧⌘Z"),
        Edit::Redo,
        false,
        editor.can_redo(),
        pal,
        &on_edit,
    )
    .child(icon("redo-2", pal.fg));
    div()
        .relative()
        .child(
            row(pal)
                .children(action_buttons(&[CLOSE], primary, pal, &on_press))
                .child(divider(pal))
                .child(tab_switch(tab, pal, &on_edit))
                .child(divider(pal))
                .children(tools)
                .child(divider(pal))
                .child(undo)
                .child(redo)
                .child(divider(pal))
                .children(action_buttons(&ACTIONS, primary, pal, &on_press)),
        )
        .children(editor.tool.map(|tool| {
            div()
                .absolute()
                .right_0()
                .top(options_top)
                .child(options(editor, tool, hex, pal, &on_edit))
        }))
}
