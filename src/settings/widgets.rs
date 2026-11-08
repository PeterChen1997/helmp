//! 设置页共用的分组表单控件，尺寸照设计稿（`.grp` / `.r` / `.gh` / `.sw` / `.pop`）。
//! 颜色一律取 `style::Palette`，浅色 / 深色都能看。

use std::rc::Rc;

use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::provider::Tint;
use crate::style::{self, Palette};

/// 弹出菜单项被选中时执行。
pub type MenuHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// 弹出菜单里的一项：(显示名, 是否当前值, 选中时执行)。
pub type MenuChoice = (String, bool, MenuHandler);

pub fn page_title(text: &str, pal: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .h(px(28.))
        .mb(px(18.))
        .text_size(px(15.))
        .font_weight(FontWeight::BOLD)
        .text_color(pal.fg)
        .child(text.to_string())
}

pub fn group_header(text: &str, pal: &Palette) -> Div {
    div()
        .mb(px(6.))
        .ml(px(4.))
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(pal.fg)
        .child(text.to_string())
}

/// 圆角分组，行与行之间画分隔线。
pub fn group(rows: Vec<AnyElement>, pal: &Palette) -> Div {
    let mut grp = div()
        .flex()
        .flex_col()
        .mb(px(18.))
        .rounded(px(12.))
        .bg(pal.group)
        .border_1()
        .border_color(pal.separator)
        .overflow_hidden();
    for (ix, row) in rows.into_iter().enumerate() {
        grp = grp.child(
            div()
                .when(ix > 0, |row| row.border_t_1().border_color(pal.separator))
                .child(row),
        );
    }
    grp
}

/// 一行：左边标签，右边控件。
pub fn row(label: impl IntoElement, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .min_h(px(44.))
        .px(px(14.))
        .py(px(6.))
        .child(div().flex_1().min_w_0().child(label))
        .child(div().flex_none().child(control))
}

pub fn label(text: impl Into<SharedString>, pal: &Palette) -> Div {
    div()
        .text_size(px(13.))
        .text_color(pal.fg)
        .child(text.into())
}

/// 标签 + 下面一行灰色说明。
pub fn label_with_help(
    text: impl Into<SharedString>,
    help: impl Into<SharedString>,
    pal: &Palette,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(label(text, pal))
        .child(help_text(help, pal))
}

pub fn help_text(text: impl Into<SharedString>, pal: &Palette) -> Div {
    div()
        .text_size(px(12.))
        .line_height(px(17.))
        .text_color(pal.muted)
        .child(text.into())
}

pub fn error_text(text: impl Into<SharedString>, pal: &Palette) -> Div {
    div()
        .text_size(px(12.))
        .line_height(px(17.))
        .text_color(pal.danger)
        .child(text.into())
}

pub fn switch(
    id: impl Into<ElementId>,
    checked: bool,
    accessibility_label: &str,
    pal: &Palette,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> Switch {
    Switch::new(id)
        .checked(checked)
        .color(pal.accent)
        .accessibility_label(accessibility_label.to_string())
        .on_change(on_change)
}

/// 弹出菜单按钮（设计稿 `.pop`）：显示当前值，点开列出选项，当前值打勾。
pub fn popup(
    id: impl Into<ElementId>,
    current: impl Into<SharedString>,
    choices: Vec<MenuChoice>,
) -> impl IntoElement {
    let choices = Rc::new(choices);
    Button::new(id)
        .outline()
        .small()
        .label(current)
        .dropdown_caret(true)
        .dropdown_menu(move |mut menu, _, _| {
            for (label, checked, handler) in choices.iter() {
                let handler = handler.clone();
                menu = menu.item(
                    PopupMenuItem::new(label.clone())
                        .checked(*checked)
                        .on_click(move |_, window, cx| handler(window, cx)),
                );
            }
            menu
        })
}

/// 普通按钮（设计稿 `.pb`）。
pub fn push_button(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    disabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    Button::new(id)
        .outline()
        .small()
        .label(text)
        .disabled(disabled)
        .on_click(on_click)
}

/// 彩色底块上的白色 Lucide 图标（侧栏、扩展表格）。
pub fn tinted_icon(name: &str, tint: Tint, size: Pixels) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(size)
        .rounded(size * 0.25)
        .bg(style::tint(tint))
        .child(
            gpui_kit::component::Icon::empty()
                .path(SharedString::from(format!("icons/{name}.svg")))
                .text_color(gpui_kit::white())
                .size(size * 0.62),
        )
}

/// 列表下方的 + / − 工具条（脚本目录、忽略的应用）。
pub fn plus_minus_bar(
    id: &'static str,
    can_remove: bool,
    pal: &Palette,
    on_add: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_remove: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let icon_button = |suffix: &'static str, icon: &'static str, enabled: bool| {
        div()
            .id(SharedString::from(format!("{id}-{suffix}")))
            .flex()
            .items_center()
            .justify_center()
            .w(px(26.))
            .h(px(22.))
            .rounded(px(5.))
            .when(enabled, |el| {
                el.cursor_pointer().hover(|el| el.bg(pal.key_bg))
            })
            .child(
                gpui_kit::component::Icon::empty()
                    .path(SharedString::from(format!("icons/{icon}.svg")))
                    .text_color(if enabled {
                        pal.key_fg
                    } else {
                        pal.muted.opacity(0.5)
                    })
                    .size(px(14.)),
            )
    };
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .h(px(30.))
        .px(px(8.))
        .child(icon_button("add", "plus", true).on_click(on_add))
        .child(
            icon_button("remove", "minus", can_remove).on_click(move |event, window, cx| {
                if can_remove {
                    on_remove(event, window, cx);
                }
            }),
        )
}

/// 危险操作按钮的文字色。Button 的 danger 变体是实心红底，设计稿是白底红字。
pub fn danger_button(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    disabled: bool,
    pal: &Palette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    push_button(id, text, disabled, on_click).text_color(pal.danger)
}
