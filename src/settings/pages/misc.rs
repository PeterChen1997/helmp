//! Agent、截图、外观、关于（设计稿没有单独的页面，沿用分组表单的样式）。

use gpui_kit::component::Disableable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::display_path;
use crate::config::{self, Appearance};
use crate::i18n::{tr, trf};
use crate::settings::model::{self, AGENT_SOURCES, APPEARANCE_OPTIONS, OCR_LANGUAGES};
use crate::settings::widgets;
use crate::settings::{SettingsView, sync_component_theme};
use crate::style::{self, Palette};

impl SettingsView {
    pub(in crate::settings) fn render_agents(
        &mut self,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut rows = Vec::new();
        for (key, name, help_zh, help_en) in AGENT_SOURCES {
            rows.push(
                widgets::row(
                    widgets::label_with_help(name, tr(help_zh, help_en), pal),
                    widgets::switch(
                        key,
                        config::extension_enabled(key),
                        name,
                        pal,
                        cx.listener(move |this, on: &bool, _, cx| {
                            let on = *on;
                            this.save(cx, |c| model::set_extension_enabled(c, key, on));
                        }),
                    ),
                )
                .into_any_element(),
            );
        }
        let plugin_off = !config::extension_enabled("agents");

        div()
            .flex()
            .flex_col()
            .child(
                div().mb(px(16.)).child(widgets::help_text(
                    tr(
                        "Helmp 读取本机 Claude Code 与 Codex 的会话记录，在面板里列出正在运行和等你处理的会话，\
                         回车跳转到对应的终端或 App。只在本机读取，不上传任何内容。",
                        "Helmp reads Claude Code and Codex session history on this Mac and lists sessions that \
                         are running or waiting for you. Press Return to jump to the terminal or app. \
                         Everything stays on this Mac; nothing is uploaded.",
                    ),
                    pal,
                )),
            )
            .child(widgets::group_header(tr("会话来源", "Session Sources"), pal))
            .child(widgets::group(rows, pal))
            .when(plugin_off, |el| {
                el.child(widgets::error_text(
                    tr(
                        "「Agent 会话」插件已在「扩展」页关闭，以上来源都不会显示。",
                        "The Agent Sessions extension is turned off in Extensions, so none of these sources are shown.",
                    ),
                    pal,
                ))
            })
            .child(widgets::help_text(
                tr(
                    "跳转到 iTerm 或终端里的会话需要「自动化」权限，首次跳转时系统会询问。",
                    "Jumping to a session in iTerm or Terminal needs Automation permission. macOS asks the first time.",
                ),
                pal,
            ))
            .into_any_element()
    }

    pub(in crate::settings) fn render_screenshot(
        &mut self,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let config = config::get();
        let shot = &config.screenshot;
        let (dir_text, custom) = match &shot.save_dir {
            Some(dir) => (display_path(std::path::Path::new(dir)), true),
            None => match crate::paths::data_dir() {
                Ok(dir) => (display_path(&dir.join("screenshots")), false),
                Err(err) => (
                    trf!(
                        "数据目录不可用：{err:#}",
                        "Data folder is unavailable: {err:#}"
                    ),
                    false,
                ),
            },
        };

        let mut languages = Vec::new();
        for (code, name_zh, name_en) in OCR_LANGUAGES {
            let name = tr(name_zh, name_en);
            let on = shot.ocr_languages.iter().any(|lang| lang == code);
            let last = on && shot.ocr_languages.len() == 1;
            languages.push(
                widgets::row(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(widgets::label(name, pal))
                        .child(widgets::help_text(code, pal)),
                    widgets::switch(
                        SharedString::from(format!("ocr-{code}")),
                        on,
                        name,
                        pal,
                        cx.listener(move |this, on: &bool, _, cx| {
                            let on = *on;
                            this.save(cx, |c| {
                                model::toggle_ocr_language(c, code, on);
                            });
                        }),
                    )
                    // 最后一种语言不能关：Vision 没有语言时会退回默认，与界面不符。
                    .disabled(last)
                    .when(last, |switch| {
                        switch.tooltip(tr(
                            "至少保留一种识别语言",
                            "Keep at least one recognition language",
                        ))
                    }),
                )
                .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .child(widgets::group(
                vec![
                    widgets::row(
                        widgets::label_with_help(
                            tr("「截图并保存」的位置", "“Screenshot and Save” folder"),
                            dir_text,
                            pal,
                        ),
                        div()
                            .flex()
                            .gap(px(8.))
                            .when(custom, |el| {
                                el.child(widgets::push_button(
                                    "shot-dir-reset",
                                    tr("恢复默认", "Restore Default"),
                                    false,
                                    cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.save(cx, |c| c.screenshot.save_dir = None);
                                    }),
                                ))
                            })
                            .child(widgets::push_button(
                                "shot-dir-pick",
                                tr("选择…", "Choose…"),
                                false,
                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.pick_path(
                                        true,
                                        tr("选择", "Choose"),
                                        cx,
                                        |this, dir, cx| {
                                            let dir = dir.to_string_lossy().into_owned();
                                            this.save(cx, |c| c.screenshot.save_dir = Some(dir));
                                        },
                                    );
                                }),
                            )),
                    )
                    .into_any_element(),
                ],
                pal,
            ))
            .child(widgets::group_header(
                tr("文字识别语言", "Text Recognition Languages"),
                pal,
            ))
            .child(widgets::group(languages, pal).mb(px(6.)))
            .child(div().ml(px(4.)).child(widgets::help_text(
                tr(
                    "按勾选顺序优先识别，至少保留一种。",
                    "Languages are tried in the order you turned them on. Keep at least one.",
                ),
                pal,
            )))
            .into_any_element()
    }

    pub(in crate::settings) fn render_appearance(
        &mut self,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = config::get().appearance;
        let mut tiles = div().flex().gap(px(16.));
        for (appearance, label_zh, label_en) in APPEARANCE_OPTIONS {
            let label = tr(label_zh, label_en);
            let selected = appearance == current;
            let preview = match appearance {
                Appearance::Light => div().bg(style::light().window),
                Appearance::Dark => div().bg(style::dark().window),
                Appearance::System => div()
                    .flex()
                    .child(div().flex_1().h_full().bg(style::light().window))
                    .child(div().flex_1().h_full().bg(style::dark().window)),
            };
            tiles = tiles.child(
                div()
                    .id(SharedString::from(format!("appearance-{appearance:?}")))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.save(cx, |c| c.appearance = appearance);
                        // 启动器与组件主题都读这个设置：同步组件主题并重绘所有窗口。
                        sync_component_theme(window, cx);
                        cx.refresh_windows();
                    }))
                    .child(
                        preview
                            .w(px(120.))
                            .h(px(76.))
                            .rounded(px(10.))
                            .overflow_hidden()
                            .border_2()
                            .border_color(if selected { pal.accent } else { pal.separator }),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(if selected { pal.accent } else { pal.fg })
                            .when(selected, |el| el.font_weight(FontWeight::SEMIBOLD))
                            .child(label),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .child(widgets::group(
                vec![
                    div()
                        .px(px(14.))
                        .py(px(16.))
                        .child(tiles)
                        .into_any_element(),
                ],
                pal,
            ))
            .child(widgets::help_text(
                tr(
                    "影响启动器面板、设置窗口与引导窗口。跟随系统时随「系统设置 › 外观」自动切换。",
                    "Applies to the launcher panel, Settings and the welcome window. Auto follows System Settings › Appearance.",
                ),
                pal,
            ))
            .into_any_element()
    }

    pub(in crate::settings) fn render_about(
        &mut self,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let data_dir = crate::paths::data_dir();
        let dir_row = match &data_dir {
            Ok(dir) => {
                let dir = dir.clone();
                widgets::row(
                    widgets::label_with_help(
                        tr("数据目录", "Data folder"),
                        display_path(&dir),
                        pal,
                    ),
                    widgets::push_button(
                        "reveal-data-dir",
                        tr("在访达中显示", "Show in Finder"),
                        false,
                        move |_, _, cx| {
                            cx.reveal_path(&dir);
                        },
                    ),
                )
            }
            Err(err) => widgets::row(
                widgets::label(tr("数据目录", "Data folder"), pal),
                widgets::error_text(format!("{err:#}"), pal),
            ),
        };
        let _ = cx;

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .py(px(18.))
                    .child(crate::settings::brand_icon(px(96.)))
                    .child(
                        div()
                            .mt(px(6.))
                            .text_size(px(22.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(pal.fg)
                            .child("Helmp"),
                    )
                    .child(widgets::help_text(
                        trf!("版本 {}", "Version {}", env!("CARGO_PKG_VERSION")),
                        pal,
                    ))
                    .child(
                        div()
                            .mt(px(6.))
                            .max_w(px(420.))
                            .text_center()
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .text_color(pal.fg)
                            .child(tr(
                                "读起来像 help me。⌥Space 唤起，一个面板里搜索应用、Agent 会话、\
                                 剪贴板历史、截图识字、翻译和脚本命令。",
                                "Sounds like “help me.” Press ⌥Space to search apps, agent sessions, \
                                 clipboard history, screen text, translations and script commands \
                                 from a single panel.",
                            )),
                    ),
            )
            .child(widgets::group(vec![dir_row.into_any_element()], pal))
            .into_any_element()
    }
}
