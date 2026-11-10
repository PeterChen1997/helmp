//! 设置 › 片段：片段列表、编辑表单（名称 / 关键词 / 标签 / 内容）、从 Raycast / Alfred 导入。
//!
//! 片段存在 `snippets.json`（见 `providers::snippets`），不进 config.json：这页自己读写，
//! 不经过 `SettingsView::save`。点列表里的一项把它装进表单，改完点「保存」；「+」清空表单新建；
//! 「−」先出一行确认再删除。从启动器里按 ⌘E「编辑」会打开这一页并选中那个片段。

use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::i18n::{tr, trf};
use crate::providers::snippets::{self, Snippet};
use crate::settings::widgets;
use crate::style::{self, Palette};

pub(in crate::settings) fn snippets_pane(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| SnippetsPane::new(window, cx)).into()
}

pub struct SnippetsPane {
    snippets: Vec<Snippet>,
    /// 表单里正在编辑的片段 id；None = 新建。
    editing: Option<String>,
    name: Entity<InputState>,
    keyword: Entity<InputState>,
    tags: Entity<InputState>,
    content: Entity<TextareaState>,
    /// 等待确认删除的片段 id。
    confirm_delete: Option<String>,
    error: Option<String>,
    notice: Option<String>,
    /// 输入框占位文字按哪一代语言设置的（`i18n::generation`）。
    placeholders_for: Option<u64>,
    /// 上次读到的片段文件修改时间：启动器里新建 / 删除后，本页重绘时跟着刷新。
    loaded_at: Option<std::time::SystemTime>,
}

impl SnippetsPane {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut pane = Self {
            snippets: Vec::new(),
            editing: None,
            name: cx.new(|cx| InputState::new(window, cx)),
            keyword: cx.new(|cx| InputState::new(window, cx)),
            tags: cx.new(|cx| InputState::new(window, cx)),
            content: cx.new(|cx| TextareaState::new(window, cx)),
            confirm_delete: None,
            error: None,
            notice: None,
            placeholders_for: None,
            loaded_at: None,
        };
        pane.reload();
        pane
    }

    /// 重新读文件。文件坏了要显示原因，不能当成空列表（否则「保存」会覆盖掉用户的片段）。
    fn reload(&mut self) {
        self.loaded_at = snippets::modified();
        match snippets::load() {
            Ok(list) => {
                self.snippets = list;
                self.snippets.sort_by(|a, b| a.name.cmp(&b.name));
            }
            Err(err) => self.error = Some(format!("{err:#}")),
        }
    }

    fn sync_placeholders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let generation = crate::i18n::generation();
        if self.placeholders_for == Some(generation) {
            return;
        }
        self.placeholders_for = Some(generation);
        let lines = [
            (&self.name, tr("如 邮件签名", "e.g. Email Signature")),
            (&self.keyword, tr("可选，如 qm", "Optional, e.g. sig")),
            (&self.tags, tr("可选，逗号分隔", "Optional, comma-separated")),
        ];
        for (state, text) in lines {
            state.update(cx, |state, cx| state.set_placeholder(text, window, cx));
        }
        self.content.update(cx, |state, cx| {
            state.set_placeholder(tr("要粘贴的文字", "Text to paste"), window, cx)
        });
    }

    /// 把一个片段装进表单（None = 清空，准备新建）。
    fn load_into_form(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let snippet = id
            .as_ref()
            .and_then(|id| self.snippets.iter().find(|s| &s.id == id))
            .cloned();
        self.editing = snippet.as_ref().map(|s| s.id.clone());
        let snippet = snippet.unwrap_or_else(snippets::new_snippet);
        self.name
            .update(cx, |s, cx| s.set_value(snippet.name.clone(), window, cx));
        self.keyword
            .update(cx, |s, cx| s.set_value(snippet.keyword.clone(), window, cx));
        self.tags
            .update(cx, |s, cx| s.set_value(snippet.tags.join(", "), window, cx));
        self.content
            .update(cx, |s, cx| s.set_value(snippet.content.clone(), window, cx));
        self.error = None;
        self.confirm_delete = None;
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snippet = Snippet {
            id: self
                .editing
                .clone()
                .unwrap_or_else(|| snippets::new_snippet().id),
            name: self.name.read(cx).value().trim().to_string(),
            keyword: self.keyword.read(cx).value().trim().to_string(),
            content: self.content.read(cx).value().to_string(),
            tags: snippets::parse_tags(&self.tags.read(cx).value()),
        };
        let id = snippet.id.clone();
        let result = snippets::update(|list| {
            snippets::validate(list, &snippet)?;
            match list.iter_mut().find(|s| s.id == snippet.id) {
                Some(existing) => *existing = snippet.clone(),
                None => list.push(snippet.clone()),
            }
            Ok(())
        });
        match result {
            Ok(()) => {
                self.error = None;
                self.notice = Some(trf!(
                    "已保存「{}」",
                    "Saved “{}”",
                    snippet.name
                ));
                self.reload();
                self.load_into_form(Some(id), window, cx);
            }
            Err(err) => {
                self.error = Some(format!("{err:#}"));
                cx.notify();
            }
        }
    }

    fn delete(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let result = snippets::update(|list| {
            list.retain(|s| s.id != id);
            Ok(())
        });
        match result {
            Ok(()) => {
                self.notice = None;
                self.reload();
                self.load_into_form(None, window, cx);
            }
            Err(err) => {
                self.error = Some(format!("{err:#}"));
                cx.notify();
            }
        }
    }

    fn import(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some(tr("导入", "Import").into()),
        });
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let picked = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) => None,
                Ok(Err(err)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.error = Some(trf!(
                            "打开选择面板失败：{err:#}",
                            "Couldn't open the file picker: {err:#}"
                        ));
                        cx.notify();
                    });
                    None
                }
                Err(_) => None,
            };
            if let Some(path) = picked {
                let _ = this.update(cx, |this, cx| {
                    let result = snippets::import(&path);
                    this.notice = Some(snippets::import_message(&result));
                    this.error = None;
                    this.reload();
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn render_list(&self, pal: &Palette, cx: &mut Context<Self>) -> Div {
        let mut rows: Vec<AnyElement> = Vec::new();
        for snippet in &self.snippets {
            let on = self.editing.as_deref() == Some(snippet.id.as_str());
            let id = snippet.id.clone();
            rows.push(
                div()
                    .id(SharedString::from(format!("snippet-{}", snippet.id)))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(34.))
                    .px(px(14.))
                    .cursor_pointer()
                    .when(on, |el| el.bg(pal.selection).text_color(pal.selection_fg))
                    .when(!on, |el| el.hover(|el| el.bg(pal.key_bg)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.load_into_form(Some(id.clone()), window, cx);
                    }))
                    .child(div().flex_1().min_w_0().truncate().child(snippet.name.clone()))
                    .when(!snippet.keyword.is_empty(), |el| {
                        el.child(
                            div()
                                .flex_none()
                                .text_size(px(12.))
                                .text_color(if on { pal.selection_fg } else { pal.muted })
                                .child(snippet.keyword.clone()),
                        )
                    })
                    .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .px(px(14.))
                    .py(px(12.))
                    .child(widgets::help_text(
                        tr(
                            "还没有片段。在下面填写后点「保存」，或从 Raycast / Alfred 导入。",
                            "No snippets yet. Fill in the form below and click Save, or import from Raycast / Alfred.",
                        ),
                        pal,
                    ))
                    .into_any_element(),
            );
        }
        let selected = self.editing.clone();
        rows.push(
            widgets::plus_minus_bar(
                "snippets",
                selected.is_some(),
                pal,
                cx.listener(|this, _: &ClickEvent, window, cx| this.load_into_form(None, window, cx)),
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.confirm_delete = selected.clone();
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        if let Some(id) = self.confirm_delete.clone() {
            let name = self
                .snippets
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .child(widgets::label(trf!("删除「{name}」？无法恢复。", "Delete “{name}”? This can’t be undone."), pal)),
                    )
                    .child(widgets::danger_button(
                        "snippet-delete-confirm",
                        tr("删除", "Delete"),
                        false,
                        pal,
                        cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.delete(id.clone(), window, cx)
                        }),
                    ))
                    .child(widgets::push_button(
                        "snippet-delete-cancel",
                        tr("取消", "Cancel"),
                        false,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.confirm_delete = None;
                            cx.notify();
                        }),
                    ))
                    .into_any_element(),
            );
        }
        widgets::group(rows, pal)
    }

    fn render_form(&self, pal: &Palette, cx: &mut Context<Self>) -> Div {
        let field = |state: &Entity<InputState>| {
            div()
                .w(px(300.))
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
        let content = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(14.))
            .py(px(10.))
            .child(widgets::label(tr("内容", "Content"), pal))
            .child(
                div()
                    .h(px(140.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .bg(pal.control)
                    .border_1()
                    .border_color(pal.separator)
                    .text_size(px(12.))
                    .child(Textarea::new(&self.content).appearance(false).h_full()),
            )
            .child(widgets::help_text(
                trf!(
                    "预览：{}",
                    "Preview: {}",
                    snippets::preview(&self.content.read(cx).value(), &self.snippets)
                ),
                pal,
            ));
        let mut buttons = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(14.))
            .py(px(10.))
            .child(widgets::push_button(
                "snippet-save",
                tr("保存", "Save"),
                false,
                cx.listener(|this, _: &ClickEvent, window, cx| this.save(window, cx)),
            ));
        if self.editing.is_some() {
            buttons = buttons.child(widgets::push_button(
                "snippet-new",
                tr("新建", "New"),
                false,
                cx.listener(|this, _: &ClickEvent, window, cx| this.load_into_form(None, window, cx)),
            ));
        }
        if let Some(err) = &self.error {
            buttons = buttons.child(widgets::error_text(err.clone(), pal));
        } else if let Some(notice) = &self.notice {
            buttons = buttons.child(widgets::help_text(notice.clone(), pal));
        }
        widgets::group(
            vec![
                widgets::row(widgets::label(tr("名称", "Name"), pal), field(&self.name)).into_any_element(),
                widgets::row(
                    widgets::label_with_help(
                        tr("关键词", "Keyword"),
                        tr("在「片段」里输入它直接定位", "Type it in Snippets to jump straight to this one"),
                        pal,
                    ),
                    field(&self.keyword),
                )
                .into_any_element(),
                widgets::row(widgets::label(tr("标签", "Tags"), pal), field(&self.tags)).into_any_element(),
                content.into_any_element(),
                buttons.into_any_element(),
            ],
            pal,
        )
    }
}

impl Render for SnippetsPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_placeholders(window, cx);
        if snippets::modified() != self.loaded_at {
            self.reload();
        }
        // 启动器里按了「编辑」：打开本页后选中那个片段。
        if let Some(id) = snippets::take_pending_edit() {
            self.reload();
            cx.defer_in(window, move |this, window, cx| {
                this.load_into_form(Some(id), window, cx)
            });
        }
        let pal = style::palette(window);
        let editing_name = self
            .editing
            .as_ref()
            .and_then(|id| self.snippets.iter().find(|s| &s.id == id))
            .map(|s| s.name.clone());
        div()
            .flex()
            .flex_col()
            .child(div().mb(px(16.)).child(widgets::help_text(
                tr(
                    "常用文字存成片段，在启动器里输入「sn 」或「片段 」搜索，回车粘贴到正在用的 App。",
                    "Save text you use often. Type “sn ” in Helmp to search, then press Return to paste into the app you’re using.",
                ),
                &pal,
            )))
            .child(widgets::group_header(tr("全部片段", "All Snippets"), &pal))
            .child(self.render_list(&pal, cx))
            .child(widgets::group_header(
                &match editing_name {
                    Some(name) => trf!("编辑「{name}」", "Edit “{name}”"),
                    None => tr("新建片段", "New Snippet").to_string(),
                },
                &pal,
            ))
            .child(self.render_form(&pal, cx))
            .child(widgets::group_header(tr("导入", "Import"), &pal))
            .child(widgets::group(
                vec![
                    widgets::row(
                        widgets::label_with_help(
                            tr("从 Raycast 或 Alfred 导入", "Import from Raycast or Alfred"),
                            tr(
                                "Raycast：Snippets › Export 得到的 JSON；Alfred：.alfredsnippets 文件。重复的会跳过。",
                                "Raycast: the JSON from Snippets › Export. Alfred: an .alfredsnippets file. Duplicates are skipped.",
                            ),
                            &pal,
                        ),
                        widgets::push_button(
                            "snippet-import",
                            tr("选择文件…", "Choose File…"),
                            false,
                            cx.listener(|this, _: &ClickEvent, _, cx| this.import(cx)),
                        ),
                    )
                    .into_any_element(),
                ],
                &pal,
            ))
            .child(widgets::help_text(
                tr(
                    "占位符：{date}、{time}、{datetime}、{date format=\"yyyy-MM-dd\"}、{clipboard}、{uuid}、{cursor}（粘贴后光标停在这里）、\
                     {argument name=\"收件人\"}（粘贴前先填写）、{snippet name=\"签名\"}；可加修饰符，如 {clipboard | trim | uppercase}。\
                     输入关键词自动展开即将支持。",
                    "Placeholders: {date}, {time}, {datetime}, {date format=\"yyyy-MM-dd\"}, {clipboard}, {uuid}, {cursor} (where the caret ends up), \
                     {argument name=\"Recipient\"} (asked before pasting), {snippet name=\"Signature\"}; add modifiers like {clipboard | trim | uppercase}. \
                     Expanding keywords as you type is coming soon.",
                ),
                &pal,
            ))
    }
}
