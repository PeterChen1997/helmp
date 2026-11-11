//! 截图与钉图共用的图标按钮。标注工具及其选项只出现在截图遮罩里。

use std::rc::Rc;

use gpui_kit::component::{
    Sizable,
    input::{Input, InputState},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::Act;
use super::annotation::{BLOCK_SIZES, COLORS, Editor, Tool, WIDTHS};
use crate::i18n::{tr, trf};
use crate::style::Palette;

const BUTTON: f32 = 28.;
const GAP: f32 = 2.;
const PAD: f32 = 4.;
const DIVIDER: f32 = 1.;
const OPTIONS_GAP: f32 = 4.;
const ROW_HEIGHT: f32 = BUTTON + PAD * 2. + 2.;

pub const ACTIONS: [ActionButton; 5] = [
    (Act::Save, "download", "保存", "Save", "⌘S"),
    (Act::Pin, "pin", "钉在屏幕上", "Pin to Screen", ""),
    (
        Act::Recognize,
        "scan-text",
        "识别文字",
        "Recognize Text",
        "",
    ),
    (Act::Close, "x", "关闭", "Close", "Esc"),
    (Act::Copy, "copy", "复制", "Copy", "⌘C"),
];

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

#[derive(Clone, Copy)]
pub enum Edit {
    Select(Option<Tool>),
    Undo,
    Redo,
    Color(u32),
    Width(usize),
    BlockSize(usize),
    StepSize(isize),
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

pub fn size() -> (f64, f64) {
    let buttons = (TOOLS.len() + 2 + ACTIONS.len()) as f32;
    let width = buttons * BUTTON + (buttons + 1.) * GAP + 2. * (PAD + DIVIDER + 1.);
    (f64::from(width), f64::from(ROW_HEIGHT))
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

pub fn button_row(buttons: &[ActionButton], primary: Act, pal: &Palette, on_press: OnPress) -> Div {
    row(pal).children(action_buttons(buttons, primary, pal, &on_press))
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

pub fn editor_toolbar(
    editor: &Editor,
    primary: Act,
    hex: &Entity<InputState>,
    options_top: Pixels,
    pal: &Palette,
    on_edit: OnEdit,
    on_press: OnPress,
) -> Div {
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
