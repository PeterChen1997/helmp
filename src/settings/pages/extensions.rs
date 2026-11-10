//! 扩展（设计稿 SettingsExtensions）：插件表格 + 脚本目录。

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{default_script_dir, display_path};
use crate::config::{self, Config};
use crate::i18n::{tr, trf};
use crate::providers::websearch;
use crate::settings::SettingsView;
use crate::settings::hotkey;
use crate::settings::model;
use crate::settings::system;
use crate::settings::widgets;
use crate::style::Palette;

// 表格列宽，照设计稿 `grid-template-columns: 1fr 64px 64px 96px 46px`；
// 快捷键列放录制框（键帽 + 清除 + 恢复默认），比设计稿宽。
const COL_KIND: Pixels = px(48.);
const COL_ALIAS: Pixels = px(64.);
const COL_HOTKEY: Pixels = px(140.);
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
                header_cell(tr("名称", "Name")),
                header_cell(tr("类型", "Type")),
                header_cell(tr("别名", "Alias")),
                header_cell(tr("快捷键", "Shortcut")),
                header_cell(tr("启用", "On")),
            )
            .min_h(px(30.))
            .into_any_element(),
        ];
        if self.extensions.is_empty() {
            rows.push(
                div()
                    .px(px(14.))
                    .py(px(12.))
                    .child(widgets::error_text(
                        tr(
                            "没有取到插件列表（启动器未创建）",
                            "Couldn't load extensions (the launcher isn't ready).",
                        ),
                        pal,
                    ))
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
            let hotkey_cell = match &row.hotkey {
                Some(recorder) => recorder.clone().into_any_element(),
                // 这个扩展没有全局快捷键命令：不给录制框，录了也不会生效。
                None => widgets::help_text("—", pal).into_any_element(),
            };
            // 录制被拒的原因 / 已保存却没生效的原因：表格列太窄，放在整行下方。
            let note = row
                .hotkey
                .as_ref()
                .and_then(|recorder| recorder.read(cx).note());
            let line = cells(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(widgets::tinted_icon(meta.icon, meta.tint, px(24.)))
                    .child(title)
                    .into_any_element(),
                widgets::help_text(meta.kind, pal).into_any_element(),
                alias.into_any_element(),
                hotkey_cell,
                widgets::switch(
                    SharedString::from(format!("ext-enabled-{id}")),
                    enabled,
                    &trf!("启用{name}", "Enable {name}"),
                    pal,
                    cx.listener(move |this, on: &bool, _, cx| {
                        let on = *on;
                        this.save(cx, |c| model::set_extension_enabled(c, id, on));
                    }),
                )
                .into_any_element(),
            );
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .child(line)
                    .when_some(note, |el, note| {
                        // 与名称文字左对齐（14 边距 + 24 图标 + 10 间距）。
                        el.child(
                            div()
                                .pl(px(48.))
                                .pr(px(14.))
                                .pb(px(8.))
                                .child(hotkey::note_line(&note, pal)),
                        )
                    })
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
                    tr(
                        "别名留空时使用插件默认前缀（灰色）；输入别名加空格即可进入该插件",
                        "Leave an alias empty to use the default prefix (in gray). Type an alias and a space to jump into that extension.",
                    ),
                    pal,
                )))
            })
            .child(self.render_web_search(&config.web_search.engines(), pal, cx))
            .child(self.render_system_commands(config.system_commands.confirm_dangerous, pal, cx))
            .child(self.render_files(config.files.in_root_search, pal, cx))
            .child(widgets::group_header(tr("脚本目录", "Script Folders"), pal))
            .child(self.render_script_dirs(&config.script_dirs, pal, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(widgets::push_button(
                        "import-raycast",
                        tr("从 Raycast 导入脚本…", "Import from Raycast…"),
                        false,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.pick_path(true, tr("导入", "Import"), cx, |this, dir, cx| {
                                this.add_script_dir(dir, cx)
                            });
                        }),
                    ))
                    .child(widgets::help_text(
                        tr(
                            "兼容 Raycast Script Commands 的头注释格式",
                            "Works with Raycast Script Commands metadata comments.",
                        ),
                        pal,
                    )),
            )
            .into_any_element()
    }

    /// 网页搜索引擎：开关 + 上下移动调整顺序（顺序就是结果里的顺序）；
    /// 自定义引擎可编辑、删除，内置引擎只能关闭。下方是添加 / 编辑自定义引擎的表单。
    fn render_web_search(
        &mut self,
        engines: &[config::WebSearchEngine],
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let last = engines.len().saturating_sub(1);
        let mut rows: Vec<AnyElement> = engines
            .iter()
            .enumerate()
            .map(|(index, engine)| {
                let name = websearch::engine_name(engine);
                let (icon, tint) = websearch::engine_icon(&engine.id);
                let move_button = |up: bool| {
                    let id = engine.id.clone();
                    let disabled = if up { index == 0 } else { index == last };
                    widgets::push_button(
                        SharedString::from(format!(
                            "web-search-{}-{}",
                            if up { "up" } else { "down" },
                            engine.id
                        )),
                        if up { "↑" } else { "↓" },
                        disabled,
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let id = id.clone();
                            this.save(cx, move |c| {
                                model::move_web_search(c, &id, up);
                            });
                        }),
                    )
                };
                let mut controls = div().flex().items_center().gap(px(6.));
                if !engine.builtin {
                    let (id, name, url) =
                        (engine.id.clone(), engine.name.clone(), engine.url.clone());
                    controls = controls.child(widgets::push_button(
                        SharedString::from(format!("web-search-edit-{}", engine.id)),
                        tr("编辑", "Edit"),
                        false,
                        cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.edit_web_engine(
                                Some((id.clone(), name.clone(), url.clone())),
                                window,
                                cx,
                            );
                        }),
                    ));
                    let id = engine.id.clone();
                    controls = controls.child(widgets::danger_button(
                        SharedString::from(format!("web-search-remove-{}", engine.id)),
                        tr("删除", "Delete"),
                        false,
                        pal,
                        cx.listener(move |this, _: &ClickEvent, window, cx| {
                            let id = id.clone();
                            if this.web_engine_editing.as_deref() == Some(id.as_str()) {
                                this.edit_web_engine(None, window, cx);
                            }
                            this.save(cx, move |c| {
                                model::remove_custom_engine(c, &id);
                            });
                        }),
                    ));
                }
                let id = engine.id.clone();
                widgets::row(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(widgets::tinted_icon(icon, tint, px(24.)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(1.))
                                .min_w_0()
                                .child(widgets::label(name.clone(), pal))
                                .child(widgets::help_text(engine.url.clone(), pal).truncate()),
                        ),
                    controls
                        .child(move_button(true))
                        .child(move_button(false))
                        .child(widgets::switch(
                            SharedString::from(format!("web-search-enabled-{}", engine.id)),
                            engine.enabled,
                            &trf!("在 {name} 搜索", "Search {name}"),
                            pal,
                            cx.listener(move |this, on: &bool, _, cx| {
                                let (id, on) = (id.clone(), *on);
                                this.save(cx, move |c| {
                                    model::set_web_search_enabled(c, &id, on);
                                });
                            }),
                        )),
                )
                .into_any_element()
            })
            .collect();

        // 添加 / 编辑自定义引擎。
        let editing = self.web_engine_editing.is_some();
        let field = |state: &Entity<InputState>| {
            div()
                .flex()
                .items_center()
                .h(px(26.))
                .px(px(6.))
                .rounded(px(6.))
                .bg(pal.control)
                .border_1()
                .border_color(pal.separator)
                .text_size(px(12.))
                .child(Input::new(state).appearance(false))
        };
        let mut buttons = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(widgets::push_button(
                "web-search-submit",
                if editing {
                    tr("保存", "Save")
                } else {
                    tr("添加", "Add")
                },
                false,
                cx.listener(|this, _: &ClickEvent, window, cx| this.submit_web_engine(window, cx)),
            ));
        if editing {
            buttons = buttons.child(widgets::push_button(
                "web-search-cancel-edit",
                tr("取消", "Cancel"),
                false,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.edit_web_engine(None, window, cx)
                }),
            ));
        }
        if let Some(err) = self.web_engine_error {
            buttons = buttons.child(widgets::error_text(err.message(), pal));
        }
        rows.push(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .px(px(14.))
                .py(px(10.))
                .child(widgets::label(
                    if editing {
                        tr("编辑自定义引擎", "Edit Custom Engine")
                    } else {
                        tr("添加自定义引擎", "Add Custom Engine")
                    },
                    pal,
                ))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            div()
                                .w(px(150.))
                                .flex_none()
                                .child(field(&self.web_engine_name)),
                        )
                        .child(div().flex_1().min_w_0().child(field(&self.web_engine_url))),
                )
                .child(buttons)
                .into_any_element(),
        );

        div()
            .flex()
            .flex_col()
            .child(widgets::group_header(tr("网页搜索", "Web Search"), pal))
            .child(widgets::group(rows, pal).mb(px(6.)))
            .child(div().mb(px(18.)).ml(px(4.)).child(widgets::help_text(
                tr(
                    "查询非空时列在结果末尾，没有本地结果时排在最前；从上到下就是结果里的顺序。网址里的 {query} 会换成你输入的内容；内置引擎只能关闭，不能删除",
                    "Listed after local results, or first when nothing local matches, in this order. {query} in the URL is replaced with what you type. Built-in engines can be turned off but not deleted.",
                ),
                pal,
            )))
    }

    /// 开始编辑某个自定义引擎（把它的名称、网址填进表单），或传 None 回到「添加」并清空表单。
    fn edit_web_engine(
        &mut self,
        engine: Option<(String, String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (id, name, url) = match engine {
            Some((id, name, url)) => (Some(id), name, url),
            None => (None, String::new(), String::new()),
        };
        self.web_engine_editing = id;
        self.web_engine_error = None;
        self.web_engine_name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.web_engine_url
            .update(cx, |input, cx| input.set_value(url, window, cx));
        cx.notify();
    }

    /// 添加或保存自定义引擎。名称为空或网址没有 {query} 时就地提示，不保存。
    fn submit_web_engine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.web_engine_name.read(cx).value().to_string();
        let url = self.web_engine_url.read(cx).value().to_string();
        let editing = self.web_engine_editing.clone();
        let apply = move |c: &mut Config| match &editing {
            Some(id) => model::update_custom_engine(c, id, &name, &url).map(|_| ()),
            None => model::add_custom_engine(c, &name, &url).map(|_| ()),
        };
        // 先在副本上试一遍：填写有误时不落盘，只显示原因。
        let mut probe = Config::clone(&config::get());
        if let Err(err) = apply(&mut probe) {
            self.web_engine_error = Some(err);
            cx.notify();
            return;
        }
        self.save(cx, move |c| {
            if let Err(err) = apply(c) {
                // 上面刚用同样的输入校验过，走到这里说明配置在这一瞬间被别处改了。
                eprintln!("helmp: 保存自定义搜索引擎失败：{}", err.message());
            }
        });
        self.edit_web_engine(None, window, cx);
    }

    /// 系统命令：危险命令的二次确认开关。
    fn render_system_commands(
        &mut self,
        confirm: bool,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .child(widgets::group_header(tr("系统命令", "System Commands"), pal))
            .child(widgets::group(
                vec![
                    widgets::row(
                        widgets::label_with_help(
                            tr("危险命令需要确认", "Confirm dangerous commands"),
                            tr(
                                "清空废纸篓、重启、关机、注销、退出所有应用：在查询末尾加 ! 后回车才执行",
                                "Empty Trash, Restart, Shut Down, Log Out, Quit All Apps run only after you add ! to the end of the query",
                            ),
                            pal,
                        ),
                        widgets::switch(
                            "system-confirm-dangerous",
                            confirm,
                            tr("危险命令需要确认", "Confirm dangerous commands"),
                            pal,
                            cx.listener(|this, on: &bool, _, cx| {
                                let on = *on;
                                this.save(cx, move |c| c.system_commands.confirm_dangerous = on);
                            }),
                        ),
                    )
                    .into_any_element(),
                ],
                pal,
            ))
    }

    /// 文件搜索：搜索范围、排除目录（逗号分隔，失焦或回车保存），是否混入根搜索。
    fn render_files(
        &mut self,
        in_root: bool,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let field = |input: &Entity<InputState>| {
            div()
                .flex()
                .items_center()
                .w(px(260.))
                .h(px(26.))
                .px(px(6.))
                .rounded(px(6.))
                .bg(pal.control)
                .border_1()
                .border_color(pal.separator)
                .font_family("Menlo")
                .text_size(px(12.))
                .child(Input::new(input).appearance(false))
        };
        div()
            .flex()
            .flex_col()
            .child(widgets::group_header(tr("文件搜索", "File Search"), pal))
            .child(widgets::group(
                vec![
                    widgets::row(
                        widgets::label_with_help(
                            tr("搜索范围", "Search in"),
                            tr(
                                "目录，逗号分隔；~ 表示家目录",
                                "Folders, comma-separated; ~ is your home folder",
                            ),
                            pal,
                        ),
                        field(&self.files_roots),
                    )
                    .into_any_element(),
                    widgets::row(
                        widgets::label_with_help(
                            tr("排除", "Exclude"),
                            tr(
                                "目录名（任意层级都排除）或路径，逗号分隔",
                                "Folder names (excluded at any depth) or paths, comma-separated",
                            ),
                            pal,
                        ),
                        field(&self.files_excluded),
                    )
                    .into_any_element(),
                    widgets::row(
                        widgets::label_with_help(
                            tr("在全局搜索中显示文件", "Show files in the main search"),
                            tr(
                                "关闭后只在「文件」范围（输入 f 加空格）里搜索文件",
                                "When off, files are searched only in the Files scope (type f and a space)",
                            ),
                            pal,
                        ),
                        widgets::switch(
                            "files-in-root-search",
                            in_root,
                            tr("在全局搜索中显示文件", "Show files in the main search"),
                            pal,
                            cx.listener(|this, on: &bool, _, cx| {
                                let on = *on;
                                this.save(cx, move |c| c.files.in_root_search = on);
                            }),
                        ),
                    )
                    .into_any_element(),
                ],
                pal,
            ))
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
                    trf!(
                        "默认脚本目录不可用：{err:#}",
                        "Default script folder is unavailable: {err:#}"
                    ),
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
                    this.pick_path(true, tr("添加", "Add"), cx, |this, dir, cx| {
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
                self.error = Some(trf!(
                    "默认脚本目录不可用：{err:#}",
                    "Default script folder is unavailable: {err:#}"
                ));
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
        Some(1) => tr("1 个", "1 script").to_string(),
        Some(n) => trf!("{n} 个", "{n} scripts"),
        None => tr("目录不存在", "Folder not found").to_string(),
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
        .when(!removable, |el| {
            el.child(widgets::help_text(tr("默认", "Default"), pal))
        })
        .child(widgets::help_text(count, pal));
    if let Some(on_click) = on_click {
        row = row.cursor_pointer().on_click(on_click);
    }
    row.into_any_element()
}
