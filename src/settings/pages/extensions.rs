//! 扩展（设计稿 SettingsExtensions）：插件表格 + 脚本目录。

use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{default_script_dir, display_path};
use crate::config;
use crate::settings::SettingsView;
use crate::settings::model;
use crate::settings::system;
use crate::settings::widgets;
use crate::style::Palette;

// 表格列宽，照设计稿 `grid-template-columns: 1fr 64px 64px 96px 46px`；
// 快捷键列放录制框，比设计稿略宽。
const COL_KIND: Pixels = px(48.);
const COL_ALIAS: Pixels = px(64.);
const COL_HOTKEY: Pixels = px(118.);
const COL_SWITCH: Pixels = px(46.);

impl SettingsView {
    pub(in crate::settings) fn render_extensions(
        &mut self,
        pal: &Palette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let config = config::get();

        let cells = |name: AnyElement,
                     kind: AnyElement,
                     alias: AnyElement,
                     hotkey: AnyElement,
                     enabled: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .min_h(px(44.))
                .px(px(14.))
                .py(px(4.))
                .child(div().flex_1().min_w_0().child(name))
                .child(div().flex_none().w(COL_KIND).child(kind))
                .child(div().flex_none().w(COL_ALIAS).child(alias))
                .child(div().flex_none().w(COL_HOTKEY).child(hotkey))
                .child(div().flex_none().w(COL_SWITCH).child(enabled))
        };
        let header_cell = |text: &str| {
            div()
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(pal.muted)
                .child(text.to_string())
                .into_any_element()
        };

        let mut rows = vec![
            cells(
                header_cell("名称"),
                header_cell("类型"),
                header_cell("别名"),
                header_cell("快捷键"),
                header_cell("启用"),
            )
            .min_h(px(30.))
            .into_any_element(),
        ];
        if self.extensions.is_empty() {
            rows.push(
                div()
                    .px(px(14.))
                    .py(px(12.))
                    .child(widgets::error_text("没有取到插件列表（启动器未创建）", pal))
                    .into_any_element(),
            );
        }
        for row in &self.extensions {
            let id = row.id;
            let meta = model::extension_meta(id);
            let enabled = config.extensions.get(id).is_none_or(|ext| ext.enabled);
            let name = if meta.name.is_empty() { id } else { meta.name };
            let mut title = div().flex().flex_col().gap(px(1.)).min_w_0().child(
                div()
                    .text_size(px(13.))
                    .text_color(pal.fg)
                    .truncate()
                    .child(name),
            );
            if let Some(note) = meta.note {
                title = title.child(widgets::help_text(note, pal).truncate());
            }
            let alias = div()
                .flex()
                .items_center()
                .h(px(24.))
                .px(px(6.))
                .rounded(px(6.))
                .bg(pal.control)
                .border_1()
                .border_color(pal.separator)
                .font_family("Menlo")
                .text_size(px(12.))
                .child(Input::new(&row.alias).appearance(false));
            rows.push(
                cells(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(widgets::tinted_icon(meta.icon, meta.tint, px(24.)))
                        .child(title)
                        .into_any_element(),
                    widgets::help_text(meta.kind, pal).into_any_element(),
                    alias.into_any_element(),
                    row.hotkey.clone().into_any_element(),
                    widgets::switch(
                        SharedString::from(format!("ext-enabled-{id}")),
                        enabled,
                        &format!("启用{name}"),
                        pal,
                        cx.listener(move |this, on: &bool, _, cx| {
                            let on = *on;
                            this.save(cx, |c| model::set_extension_enabled(c, id, on));
                        }),
                    )
                    .into_any_element(),
                )
                .into_any_element(),
            );
        }
        // 别名为空时显示的是插件默认前缀（占位提示），这里说明一下。
        let has_default_alias = self
            .extensions
            .iter()
            .any(|row| row.default_alias.is_some());

        div()
            .flex()
            .flex_col()
            .child(widgets::group(rows, pal).mb(px(if has_default_alias { 6. } else { 18. })))
            .when(has_default_alias, |el| {
                el.child(div().mb(px(18.)).ml(px(4.)).child(widgets::help_text(
                    "别名留空时使用插件默认前缀（灰色）；输入别名加空格即可进入该插件",
                    pal,
                )))
            })
            .child(widgets::group_header("脚本目录", pal))
            .child(self.render_script_dirs(&config.script_dirs, pal, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(widgets::push_button(
                        "import-raycast",
                        "从 Raycast 导入脚本…",
                        false,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.pick_path(true, "导入", cx, |this, dir, cx| {
                                this.add_script_dir(dir, cx)
                            });
                        }),
                    ))
                    .child(widgets::help_text(
                        "兼容 Raycast Script Commands 的头注释格式",
                        pal,
                    )),
            )
            .into_any_element()
    }

    fn render_script_dirs(
        &mut self,
        dirs: &[String],
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let mut list = div().flex().flex_col();
        match default_script_dir() {
            Ok(dir) => {
                list = list.child(script_dir_row(
                    &dir.to_string_lossy(),
                    false,
                    false,
                    pal,
                    None,
                ))
            }
            Err(err) => {
                list = list.child(div().px(px(14.)).py(px(8.)).child(widgets::error_text(
                    format!("默认脚本目录不可用：{err:#}"),
                    pal,
                )))
            }
        }
        for dir in dirs {
            let selected = self.selected_script_dir.as_deref() == Some(dir.as_str());
            let dir_owned = dir.clone();
            let on_click = cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.selected_script_dir = Some(dir_owned.clone());
                cx.notify();
            });
            list = list.child(script_dir_row(
                dir,
                true,
                selected,
                pal,
                Some(Box::new(on_click)),
            ));
        }
        let can_remove = self
            .selected_script_dir
            .as_ref()
            .is_some_and(|selected| dirs.contains(selected));

        div()
            .flex()
            .flex_col()
            .mb(px(18.))
            .rounded(px(12.))
            .bg(pal.group)
            .border_1()
            .border_color(pal.separator)
            .overflow_hidden()
            .child(list)
            .child(div().child(widgets::plus_minus_bar(
                "script-dir",
                can_remove,
                pal,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.pick_path(true, "添加", cx, |this, dir, cx| {
                        this.add_script_dir(dir, cx)
                    });
                }),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(dir) = this.selected_script_dir.take() {
                        this.save(cx, |c| {
                            model::remove_script_dir(c, &dir);
                        });
                    }
                }),
            )))
    }

    fn add_script_dir(&mut self, dir: std::path::PathBuf, cx: &mut Context<Self>) {
        let default = match default_script_dir() {
            Ok(default) => default.to_string_lossy().into_owned(),
            Err(err) => {
                self.error = Some(format!("默认脚本目录不可用：{err:#}"));
                cx.notify();
                return;
            }
        };
        let dir = dir.to_string_lossy().into_owned();
        self.selected_script_dir = Some(dir.clone());
        self.save(cx, |c| {
            model::add_script_dir(c, &dir, &default);
        });
    }
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

fn script_dir_row(
    dir: &str,
    removable: bool,
    selected: bool,
    pal: &Palette,
    on_click: Option<ClickHandler>,
) -> AnyElement {
    let path = std::path::Path::new(dir);
    let count = match system::count_scripts(path) {
        Some(n) => format!("{n} 个"),
        None => "目录不存在".into(),
    };
    let mut row = div()
        .id(SharedString::from(format!("script-dir-{dir}")))
        .flex()
        .items_center()
        .gap(px(10.))
        .min_h(px(36.))
        .px(px(14.))
        .border_b_1()
        .border_color(pal.separator)
        .when(selected, |el| el.bg(pal.accent.opacity(0.12)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family("Menlo")
                .text_size(px(12.))
                .text_color(pal.fg)
                .child(display_path(path)),
        )
        .when(!removable, |el| el.child(widgets::help_text("默认", pal)))
        .child(widgets::help_text(count, pal));
    if let Some(on_click) = on_click {
        row = row.cursor_pointer().on_click(on_click);
    }
    row.into_any_element()
}
