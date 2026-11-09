//! 设置窗口「模型」页（设计稿 SettingsModels）：已接入列表、编辑区、用途。
//!
//! - 文本字段改动立即写 config.json；API Key 在失焦 / 回车 / 测试连接时写钥匙串（避免每个按键写一次）。
//! - 测试连接只在用户点按钮时发一个极小请求；Ollama 在线状态在打开本页和测试时探测。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::config::{self, ModelConfig, ModelKind};
use crate::i18n::{tr, trf};
use crate::style::{self, Palette};

const KINDS: [ModelKind; 3] = [
    ModelKind::Anthropic,
    ModelKind::OpenAiCompatible,
    ModelKind::Ollama,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Kind,
    TranslateModel,
    Target,
}

#[derive(Clone)]
enum Status {
    Idle,
    Running,
    /// (中文, 英文)：渲染时按当前语言取，切换语言后跟着变。
    Ok((&'static str, &'static str)),
    Err(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextField {
    Name,
    BaseUrl,
    Model,
}

pub struct ModelsPane {
    /// 编辑区对应的模型 id。
    selected: Option<String>,
    /// 输入框里当前装的是哪个模型的值。
    loaded: Option<String>,
    name: Entity<InputState>,
    base_url: Entity<InputState>,
    api_key: Entity<InputState>,
    model: Entity<InputState>,
    key_dirty: bool,
    status: Status,
    /// Ollama 模型 id → 是否在线。
    online: HashMap<String, bool>,
    menu: Option<Menu>,
    /// 输入框占位文字按哪一代语言设置的（`i18n::generation`）；None 表示还没设置。
    placeholders_for: Option<u64>,
    _subscriptions: Vec<Subscription>,
}

impl ModelsPane {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 随语言变的占位文字在 render 里设置（`sync_placeholders`）。
        let name = cx.new(|cx| InputState::new(window, cx));
        let base_url = cx.new(|cx| InputState::new(window, cx).placeholder("https://…"));
        let api_key = cx.new(|cx| InputState::new(window, cx).masked(true));
        let model = cx.new(|cx| InputState::new(window, cx));

        let mut subscriptions = Vec::new();
        for (state, field) in [
            (&name, TextField::Name),
            (&base_url, TextField::BaseUrl),
            (&model, TextField::Model),
        ] {
            let state_for_read = state.clone();
            subscriptions.push(cx.subscribe_in(
                state,
                window,
                move |this, _, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        let value = state_for_read.read(cx).value().to_string();
                        this.on_text_changed(field, value, cx);
                    }
                },
            ));
        }
        subscriptions.push(cx.subscribe_in(
            &api_key,
            window,
            |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => this.key_dirty = true,
                InputEvent::Blur | InputEvent::PressEnter { .. } => this.save_key(cx),
                InputEvent::Focus => {}
            },
        ));

        let selected = config::get().models.first().map(|model| model.id.clone());
        let mut pane = Self {
            selected,
            loaded: None,
            name,
            base_url,
            api_key,
            model,
            key_dirty: false,
            status: Status::Idle,
            online: HashMap::new(),
            menu: None,
            placeholders_for: None,
            _subscriptions: subscriptions,
        };
        pane.load_selected(window, cx);
        pane.check_ollama(cx);
        pane
    }

    /// 首次渲染、切换语言后，按当前语言重设输入框的占位文字（InputState 不会自己跟着变）。
    fn sync_placeholders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let generation = crate::i18n::generation();
        if self.placeholders_for == Some(generation) {
            return;
        }
        self.placeholders_for = Some(generation);
        let texts = [
            (&self.name, tr("如 DeepSeek", "e.g. DeepSeek")),
            (
                &self.api_key,
                tr("只存在本机钥匙串", "Stored only in your Keychain"),
            ),
            (&self.model, tr("如 deepseek-chat", "e.g. deepseek-chat")),
        ];
        for (state, text) in texts {
            state.update(cx, |state, cx| state.set_placeholder(text, window, cx));
        }
    }

    fn selected_model(&self) -> Option<ModelConfig> {
        let id = self.selected.as_deref()?;
        config::get()
            .models
            .iter()
            .find(|model| model.id == id)
            .cloned()
    }

    /// 选中项变了：把它的值装进输入框（`set_value` 不触发 Change，不会回写配置）。
    fn load_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loaded == self.selected {
            return;
        }
        let model = self.selected_model();
        let key = match &model {
            Some(model) if crate::models::needs_key(model.kind) => {
                match crate::models::api_key(model) {
                    Ok(key) => key.unwrap_or_default(),
                    Err(err) => {
                        self.status = Status::Err(format!("{err:#}"));
                        String::new()
                    }
                }
            }
            _ => String::new(),
        };
        let values = [
            (&self.name, model.as_ref().map(|m| m.name.clone())),
            (&self.base_url, model.as_ref().map(|m| m.base_url.clone())),
            (&self.model, model.as_ref().map(|m| m.model.clone())),
            (&self.api_key, Some(key)),
        ];
        for (state, value) in values {
            let value = value.unwrap_or_default();
            state.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        self.key_dirty = false;
        self.loaded = self.selected.clone();
    }

    /// 修改选中模型并落盘；失败显示在状态行，不静默。
    fn update_selected(&mut self, f: impl FnOnce(&mut ModelConfig), cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if let Err(err) = config::update(|config| {
            if let Some(model) = config.models.iter_mut().find(|model| model.id == id) {
                f(model);
            }
        }) {
            self.status = Status::Err(save_failed(&err));
        }
        cx.notify();
    }

    fn on_text_changed(&mut self, field: TextField, value: String, cx: &mut Context<Self>) {
        if self.loaded != self.selected {
            return;
        }
        self.update_selected(
            |model| match field {
                TextField::Name => model.name = value,
                TextField::BaseUrl => model.base_url = value,
                TextField::Model => model.model = value,
            },
            cx,
        );
    }

    fn save_key(&mut self, cx: &mut Context<Self>) {
        if !self.key_dirty {
            return;
        }
        let Some(model) = self.selected_model() else {
            return;
        };
        let key = self.api_key.read(cx).value().to_string();
        self.status = match crate::models::set_api_key(&model, &key) {
            Ok(()) if key.trim().is_empty() => {
                Status::Ok(("已从钥匙串删除 Key", "Key removed from Keychain"))
            }
            Ok(()) => Status::Ok(("Key 已存入钥匙串", "Key saved to Keychain")),
            Err(err) => Status::Err(format!("{err:#}")),
        };
        self.key_dirty = false;
        cx.notify();
    }

    fn select(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.save_key(cx);
        self.selected = Some(id);
        self.status = Status::Idle;
        self.menu = None;
        self.load_selected(window, cx);
        cx.notify();
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_key(cx);
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let id = format!("m{millis:x}");
        let kind = ModelKind::OpenAiCompatible;
        let model = ModelConfig {
            id: id.clone(),
            name: tr("新模型", "New Model").into(),
            kind,
            base_url: crate::models::default_base_url(kind).into(),
            model: String::new(),
        };
        if let Err(err) = config::update(|config| config.models.push(model)) {
            self.status = Status::Err(save_failed(&err));
            cx.notify();
            return;
        }
        self.select(id, window, cx);
    }

    /// 移除所选模型：同时删钥匙串里的 Key；翻译若指向它则改回默认。
    fn remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.selected_model() else {
            return;
        };
        if let Err(err) = crate::models::delete_api_key(&model) {
            // Key 删不掉就不移除模型，否则 Key 成了找不到主人的孤儿条目。
            self.status = Status::Err(format!("{err:#}"));
            cx.notify();
            return;
        }
        let id = model.id.clone();
        let result = config::update(|config| {
            config.models.retain(|model| model.id != id);
            if config.translate.model_id.as_deref() == Some(id.as_str()) {
                config.translate.model_id = None;
            }
        });
        self.status = match result {
            Ok(()) => Status::Idle,
            Err(err) => Status::Err(save_failed(&err)),
        };
        self.online.remove(&model.id);
        self.key_dirty = false;
        self.selected = config::get().models.first().map(|model| model.id.clone());
        self.load_selected(window, cx);
        cx.notify();
    }

    fn set_kind(&mut self, kind: ModelKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.selected_model() else {
            return;
        };
        if model.kind == kind {
            return;
        }
        // Base URL 还是旧接口的默认值（或空）时，跟着换成新接口的默认值。
        let url = model.base_url.trim().trim_end_matches('/');
        let reset_url = url.is_empty()
            || KINDS
                .iter()
                .any(|k| crate::models::default_base_url(*k) == url);
        let new_url = crate::models::default_base_url(kind).to_string();
        self.update_selected(
            |model| {
                model.kind = kind;
                if reset_url {
                    model.base_url = new_url.clone();
                }
            },
            cx,
        );
        if reset_url {
            self.base_url
                .update(cx, |state, cx| state.set_value(new_url, window, cx));
        }
        self.status = Status::Idle;
        self.check_ollama(cx);
    }

    fn test(&mut self, cx: &mut Context<Self>) {
        self.save_key(cx);
        let Some(model) = self.selected_model() else {
            return;
        };
        if matches!(self.status, Status::Running) {
            return;
        }
        self.status = Status::Running;
        cx.notify();
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let id = model.id.clone();
            let kind = model.kind;
            let result = cx
                .background_spawn(async move { crate::models::test_connection(&model, None) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if kind == ModelKind::Ollama {
                    this.online.insert(id.clone(), result.is_ok());
                }
                if this.selected.as_deref() == Some(id.as_str()) {
                    this.status = match result {
                        Ok(()) if kind == ModelKind::Ollama => {
                            Status::Ok(("连接正常", "Connected"))
                        }
                        Ok(()) => Status::Ok((
                            "连接正常 · Key 已存入钥匙串",
                            "Connected · Key saved to Keychain",
                        )),
                        Err(err) => Status::Err(format!("{err:#}")),
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 后台探测所有 Ollama 模型是否在线（各 2 秒超时）。
    fn check_ollama(&mut self, cx: &mut Context<Self>) {
        let targets: Vec<(String, String)> = config::get()
            .models
            .iter()
            .filter(|model| model.kind == ModelKind::Ollama)
            .map(|model| (model.id.clone(), model.base_url.clone()))
            .collect();
        if targets.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let results = cx
                .background_spawn(async move {
                    targets
                        .into_iter()
                        .map(|(id, url)| (id, crate::models::ollama_online(&url)))
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.online.extend(results);
                cx.notify();
            });
        })
        .detach();
    }

    fn set_translate_model(&mut self, id: String, cx: &mut Context<Self>) {
        if let Err(err) = config::update(|config| config.translate.model_id = Some(id)) {
            self.status = Status::Err(save_failed(&err));
        }
        self.menu = None;
        cx.notify();
    }

    fn set_target(&mut self, code: String, cx: &mut Context<Self>) {
        if let Err(err) = config::update(|config| config.translate.target = code) {
            self.status = Status::Err(save_failed(&err));
        }
        self.menu = None;
        cx.notify();
    }

    fn toggle_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        self.menu = if self.menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
        cx.notify();
    }
}

fn save_failed(err: &anyhow::Error) -> String {
    trf!("保存设置失败：{err:#}", "Couldn't save settings: {err:#}")
}

fn subtitle(model: &ModelConfig) -> String {
    match model.kind {
        ModelKind::Ollama => {
            let url = model.base_url.trim();
            let url = if url.is_empty() {
                crate::models::default_base_url(ModelKind::Ollama)
            } else {
                url
            };
            let host = url
                .trim_start_matches("http://")
                .trim_start_matches("https://")
                .trim_end_matches('/');
            if crate::models::is_local(model) {
                trf!("Ollama · 本机 {host}", "Ollama · {host} (local)")
            } else {
                format!("Ollama · {host}")
            }
        }
        kind => format!("{} · {}", crate::models::kind_label(kind), model.model),
    }
}

/// 列表左侧的字母块：Anthropic 用 A、Ollama 用 O，兼容接口用名字首字母。
fn letter(model: &ModelConfig) -> (String, Hsla) {
    match model.kind {
        ModelKind::Anthropic => ("A".into(), rgb(0xc96442).into()),
        ModelKind::Ollama => ("O".into(), rgb(0x3a3a3c).into()),
        ModelKind::OpenAiCompatible => (
            model
                .name
                .chars()
                .next()
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_else(|| "?".into()),
            rgb(0x4d6bfe).into(),
        ),
    }
}

fn kind_short(kind: ModelKind) -> &'static str {
    match kind {
        ModelKind::Anthropic => "Anthropic",
        ModelKind::OpenAiCompatible => tr("OpenAI 兼容", "OpenAI-Compatible"),
        ModelKind::Ollama => "Ollama",
    }
}

fn group(pal: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .mb(px(14.))
        .rounded(px(12.))
        .bg(pal.group)
        .border_1()
        .border_color(pal.separator)
        .overflow_hidden()
}

fn heading(text: impl Into<SharedString>, pal: &Palette) -> Div {
    div()
        .ml(px(4.))
        .mb(px(6.))
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(pal.fg)
        .child(text.into())
}

fn row(first: bool, pal: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .min_h(px(40.))
        .px(px(14.))
        .py(px(4.))
        .when(!first, |row| row.border_t_1().border_color(pal.separator))
}

fn chip(text: &str, bg: Hsla, fg: Hsla, dot: Option<Hsla>) -> Div {
    div()
        .ml_auto()
        .flex()
        .items_center()
        .gap(px(5.))
        .h(px(20.))
        .px(px(8.))
        .rounded(px(10.))
        .bg(bg)
        .text_color(fg)
        .text_size(px(11.5))
        .font_weight(FontWeight::SEMIBOLD)
        .when_some(dot, |chip, dot| {
            chip.child(div().size(px(6.)).rounded_full().bg(dot))
        })
        .child(text.to_string())
}

/// 系统设置风格的弹出按钮。
fn pop_button(id: &'static str, label: String, pal: &Palette) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(24.))
        .pl(px(10.))
        .pr(px(6.))
        .rounded(px(6.))
        .bg(pal.control)
        .border_1()
        .border_color(pal.separator)
        .text_color(pal.fg)
        .cursor_pointer()
        .child(label)
        .child(
            gpui_kit::component::Icon::empty()
                .path("icons/chevrons-up-down.svg")
                .text_color(pal.key_fg)
                .size(px(12.)),
        )
}

/// 文本输入框（设计稿 `.fld`：宽 300、高 24）。
fn field(state: &Entity<InputState>, mono: bool, pal: &Palette) -> Div {
    div()
        .ml_auto()
        .w(px(300.))
        .h(px(24.))
        .px(px(8.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .bg(pal.control)
        .border_1()
        .border_color(pal.separator)
        .text_size(px(13.))
        .when(mono, |field| field.font_family("Menlo"))
        .child(Input::new(state).appearance(false))
}

impl ModelsPane {
    /// 弹出按钮 + 选项菜单。`on_pick` 收到选中的值。
    #[allow(clippy::too_many_arguments)]
    fn dropdown(
        &self,
        id: &'static str,
        menu: Menu,
        current: String,
        options: Vec<(String, String)>,
        selected_value: &str,
        on_pick: fn(&mut Self, String, &mut Window, &mut Context<Self>),
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let button = pop_button(id, current, pal)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_menu(menu, cx)));
        let mut container = div().ml_auto().relative().child(button);
        if self.menu == Some(menu) {
            let mut list = div()
                .id(SharedString::from(format!("{id}-menu")))
                .flex()
                .flex_col()
                .min_w(px(200.))
                .p(px(5.))
                .rounded(px(8.))
                .bg(pal.window)
                .border_1()
                .border_color(pal.separator)
                .shadow_lg()
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.menu = None;
                    cx.notify();
                }));
            for (ix, (label, value)) in options.into_iter().enumerate() {
                let on = value == selected_value;
                list = list.child(
                    div()
                        .id(SharedString::from(format!("{id}-opt-{ix}")))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(24.))
                        .px(px(8.))
                        .rounded(px(5.))
                        .text_color(pal.fg)
                        .hover(|style| style.bg(pal.selection).text_color(pal.selection_fg))
                        .child(div().w(px(12.)).child(if on { "✓" } else { "" }))
                        .child(label)
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.menu = None;
                            on_pick(this, value.clone(), window, cx);
                            cx.notify();
                        })),
                );
            }
            container =
                container.child(div().absolute().right_0().top(px(28.)).child(
                    deferred(anchored().anchor(Anchor::TopRight).child(list)).with_priority(1),
                ));
        }
        container
    }

    fn render_list(&self, pal: &Palette, cx: &mut Context<Self>) -> Div {
        let config = config::get();
        let default_id = crate::models::translate_model(&config).map(|model| model.id.clone());
        let mut list = group(pal);
        if config.models.is_empty() {
            list = list.child(
                row(true, pal)
                    .min_h(px(46.))
                    .text_color(pal.muted)
                    .child(tr(
                        "还没有接入模型。点下方 + 添加 Anthropic、OpenAI 兼容接口或本机 Ollama。",
                        "No models yet. Click + below to add Anthropic, an OpenAI-compatible API, \
                         or a local Ollama model.",
                    )),
            );
        }
        for (ix, model) in config.models.iter().enumerate() {
            let (letter, color) = letter(model);
            let on = self.selected.as_deref() == Some(model.id.as_str());
            let chip = if default_id.as_deref() == Some(model.id.as_str()) {
                Some(chip(
                    tr("默认", "Default"),
                    pal.accent.opacity(0.12),
                    pal.accent,
                    None,
                ))
            } else if model.kind == ModelKind::Ollama {
                match self.online.get(&model.id) {
                    Some(true) => {
                        let (bg, fg) = style::tone_colors(crate::provider::Tone::Done, pal.dark);
                        Some(chip(
                            tr("在线", "Online"),
                            bg,
                            fg,
                            Some(style::tint(crate::provider::Tint::Green)),
                        ))
                    }
                    Some(false) => {
                        let (bg, fg) = style::tone_colors(crate::provider::Tone::Neutral, pal.dark);
                        Some(chip(tr("离线", "Offline"), bg, fg, Some(pal.muted)))
                    }
                    None => None,
                }
            } else {
                None
            };
            let id = model.id.clone();
            list = list.child(
                row(ix == 0, pal)
                    .id(SharedString::from(format!("model-{}", model.id)))
                    .min_h(px(46.))
                    .when(on, |row| row.bg(pal.accent.opacity(0.1)))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(28.))
                            .rounded(px(7.))
                            .bg(color)
                            .text_color(white())
                            .text_size(px(13.))
                            .font_weight(FontWeight::BOLD)
                            .child(letter),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .min_w_0()
                            .child(
                                div()
                                    .text_color(pal.fg)
                                    .truncate()
                                    .child(model.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(pal.muted)
                                    .truncate()
                                    .child(subtitle(model)),
                            ),
                    )
                    .children(chip)
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.select(id.clone(), window, cx)
                    })),
            );
        }
        let has_selection = self.selected.is_some();
        list.child(
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .h(px(28.))
                .px(px(8.))
                .border_t_1()
                .border_color(pal.separator)
                .child(
                    div()
                        .id("model-add")
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(26.))
                        .h(px(22.))
                        .rounded(px(5.))
                        .text_size(px(16.))
                        .text_color(pal.key_fg)
                        .cursor_pointer()
                        .hover(|style| style.bg(pal.key_bg))
                        .child("+")
                        .on_click(
                            cx.listener(|this, _: &ClickEvent, window, cx| this.add(window, cx)),
                        ),
                )
                .child(
                    div()
                        .id("model-remove")
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(26.))
                        .h(px(22.))
                        .rounded(px(5.))
                        .text_size(px(16.))
                        .text_color(if has_selection {
                            pal.key_fg
                        } else {
                            pal.muted.opacity(0.5)
                        })
                        .when(has_selection, |button| {
                            button
                                .cursor_pointer()
                                .hover(|style| style.bg(pal.key_bg))
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.remove(window, cx)
                                }))
                        })
                        .child("−"),
                ),
        )
    }

    fn render_editor(&self, model: &ModelConfig, pal: &Palette, cx: &mut Context<Self>) -> Div {
        let kinds = KINDS
            .iter()
            .map(|kind| (kind_short(*kind).to_string(), kind_value(*kind).to_string()))
            .collect();
        let kind_menu = self.dropdown(
            "model-kind",
            Menu::Kind,
            kind_short(model.kind).into(),
            kinds,
            kind_value(model.kind),
            |this, value, window, cx| {
                if let Some(kind) = KINDS.iter().find(|kind| kind_value(**kind) == value) {
                    this.set_kind(*kind, window, cx);
                }
            },
            pal,
            cx,
        );
        let (status_text, status_color) = match &self.status {
            Status::Idle => (String::new(), pal.muted),
            Status::Running => (tr("正在连接…", "Connecting…").to_string(), pal.muted),
            Status::Ok((zh, en)) => (
                format!("✓ {}", tr(zh, en)),
                style::tone_colors(crate::provider::Tone::Done, pal.dark).1,
            ),
            Status::Err(text) => (text.clone(), pal.danger),
        };
        let running = matches!(self.status, Status::Running);
        let label = |text: &'static str| div().flex_none().text_color(pal.fg).child(text);
        group(pal)
            .child(
                row(true, pal)
                    .child(label(tr("名称", "Name")))
                    .child(field(&self.name, false, pal)),
            )
            .child(
                row(false, pal)
                    .child(label(tr("接口类型", "API Type")))
                    .child(kind_menu),
            )
            .child(
                row(false, pal)
                    .child(label("Base URL"))
                    .child(field(&self.base_url, true, pal)),
            )
            .when(crate::models::needs_key(model.kind), |group| {
                group.child(row(false, pal).child(label("API Key")).child(field(
                    &self.api_key,
                    false,
                    pal,
                )))
            })
            .child(
                row(false, pal)
                    .child(label(tr("模型名", "Model")))
                    .child(field(&self.model, true, pal)),
            )
            .child(
                row(false, pal)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .text_color(status_color)
                            .child(status_text),
                    )
                    .child(
                        pop_button_plain(
                            "model-test",
                            if running {
                                tr("测试中…", "Testing…")
                            } else {
                                tr("测试连接", "Test Connection")
                            },
                            pal,
                        )
                        .when(!running, |button| {
                            button
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.test(cx)))
                        }),
                    ),
            )
    }

    fn render_usage(&self, pal: &Palette, cx: &mut Context<Self>) -> Div {
        let config = config::get();
        let current = crate::models::translate_model(&config);
        let models: Vec<(String, String)> = config
            .models
            .iter()
            .map(|model| (model.name.clone(), model.id.clone()))
            .collect();
        let model_label = current
            .map(|model| model.name.clone())
            .unwrap_or_else(|| tr("未接入模型", "No Models").into());
        let model_value = current.map(|model| model.id.clone()).unwrap_or_default();
        let mut targets = vec![(
            tr("中英互译（自动）", "Auto (Chinese ↔ English)").to_string(),
            "auto".to_string(),
        )];
        targets.extend(
            crate::models::LANGUAGES
                .iter()
                .map(|(code, zh, en)| (tr(zh, en).to_string(), code.to_string())),
        );
        let target = config.translate.target.clone();
        let target_label = targets
            .iter()
            .find(|(_, code)| *code == target)
            .map(|(label, _)| label.clone())
            .unwrap_or_else(|| target.clone());
        let model_menu = self.dropdown(
            "translate-model",
            Menu::TranslateModel,
            model_label,
            models,
            &model_value,
            |this, value, _, cx| this.set_translate_model(value, cx),
            pal,
            cx,
        );
        let target_menu = self.dropdown(
            "translate-target",
            Menu::Target,
            target_label,
            targets,
            &target,
            |this, value, _, cx| this.set_target(value, cx),
            pal,
            cx,
        );
        group(pal)
            .child(
                row(true, pal)
                    .child(tr("翻译", "Translate"))
                    .child(model_menu),
            )
            .child(
                row(false, pal)
                    .child(tr("翻译默认目标语言", "Default Target Language"))
                    .child(target_menu),
            )
    }
}

fn kind_value(kind: ModelKind) -> &'static str {
    match kind {
        ModelKind::Anthropic => "anthropic",
        ModelKind::OpenAiCompatible => "openai",
        ModelKind::Ollama => "ollama",
    }
}

fn pop_button_plain(id: &'static str, label: &'static str, pal: &Palette) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(24.))
        .px(px(10.))
        .rounded(px(6.))
        .bg(pal.control)
        .border_1()
        .border_color(pal.separator)
        .text_color(pal.fg)
        .cursor_pointer()
        .child(label)
}

impl Render for ModelsPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 选中的模型可能被别处删掉（如设置窗口别的页面），对齐一下。
        let config = config::get();
        if self
            .selected
            .as_deref()
            .is_some_and(|id| !config.models.iter().any(|model| model.id == id))
        {
            self.selected = config.models.first().map(|model| model.id.clone());
        }
        self.load_selected(window, cx);
        self.sync_placeholders(window, cx);

        let pal = style::palette(window);
        let selected = self.selected_model();
        div()
            .id("models-pane")
            .size_full()
            .overflow_y_scroll()
            .px(px(28.))
            .pt(px(16.))
            .pb(px(20.))
            .text_size(px(13.))
            .text_color(pal.fg)
            .child(
                div()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .mb(px(6.))
                    .text_size(px(15.))
                    .font_weight(FontWeight::BOLD)
                    .child(tr("模型", "Models")),
            )
            .child(
                div()
                    .mb(px(14.))
                    .text_size(px(12.5))
                    .line_height(px(18.))
                    .text_color(pal.muted)
                    .child(tr(
                        "Helmp 不自带模型，也不经过任何中转。接入你自己的 API Key 或本机模型，\
                         用于翻译等功能；Key 只存在本机钥匙串里。",
                        "Helmp doesn't ship with a model or route requests through any relay. \
                         Connect your own API key or a local model to power features like \
                         translation. Keys are stored only in your Mac's Keychain.",
                    )),
            )
            .child(heading(tr("已接入", "Your Models"), &pal))
            .child(self.render_list(&pal, cx))
            .when_some(selected, |pane, model| {
                pane.child(heading(model.name.clone(), &pal))
                    .child(self.render_editor(&model, &pal, cx))
            })
            .child(heading(tr("用途", "Usage"), &pal))
            .child(self.render_usage(&pal, cx))
    }
}
