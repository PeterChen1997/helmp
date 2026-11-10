//! 设置 › 插件：内置功能、自带的清单插件、用户插件放在一页管理。
//!
//! - 每个插件：启用开关、版本 / 作者 / 来源、权限、出错原因；点开一行看详情并填偏好设置。
//! - 用户插件第一次启用（或清单权限变了）先列出权限，用户点「允许并启用」才生效。
//! - secret 偏好设置用掩码输入框，失焦或回车时写进钥匙串，界面上从不回显已存的值。
//! - 顶部：打开插件文件夹、重新加载、新建插件（生成模板并在访达中显示）、开发者文档。
//!
//! 插件列表每次渲染现取 `registry::current()`，目录轮询热重载后这里自然跟着变。

use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::config::{self, Config};
use crate::i18n::{tr, trf};
use crate::provider::Action;
use crate::providers::plugins::manifest::{PrefKind, Preference};
use crate::providers::plugins::registry::{self, Entry, Source, State};
use crate::providers::plugins::{self, scaffold, secrets};
use crate::style::{self, Palette};

use super::model;
use super::widgets;

const DOCS_EN: &str = "https://github.com/PeterChen1997/helmp/blob/HEAD/docs/plugins.md";
const DOCS_ZH: &str = "https://github.com/PeterChen1997/helmp/blob/HEAD/docs/plugins.zh-CN.md";

/// 选中插件的一个文本类偏好设置输入框。
struct PrefInput {
    id: String,
    kind: PrefKind,
    state: Entity<InputState>,
    /// secret：钥匙串里是否已经存了值（只用于提示，不显示值）。
    stored: bool,
}

pub struct PluginsPane {
    /// 展开的插件 id。
    selected: Option<String>,
    /// 等待用户确认权限的插件 id。
    consent: Option<String>,
    /// 当前输入框属于哪个插件（切换插件时重建）。
    inputs_for: Option<String>,
    inputs: Vec<PrefInput>,
    /// 最近一次操作的结果（成功提示或错误）。
    status: Option<Result<String, String>>,
    _subscriptions: Vec<Subscription>,
}

pub fn new(window: &mut Window, cx: &mut App) -> AnyView {
    let _ = window;
    cx.new(|_| PluginsPane {
        selected: None,
        consent: None,
        inputs_for: None,
        inputs: Vec::new(),
        status: None,
        _subscriptions: Vec::new(),
    })
    .into()
}

fn entry(id: &str) -> Option<Entry> {
    registry::current()
        .iter()
        .find(|entry| entry.id == id)
        .cloned()
}

impl PluginsPane {
    fn save(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Config)) {
        if let Err(err) = config::update(f) {
            eprintln!("helmp: 保存插件设置失败：{err:#}");
            self.status = Some(Err(trf!(
                "保存设置失败：{err:#}",
                "Couldn't save settings: {err:#}"
            )));
        }
        cx.notify();
    }

    fn select(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = id;
        self.consent = None;
        self.status = None;
        self.load_inputs(window, cx);
        cx.notify();
    }

    /// 为选中插件的 text / secret 偏好设置建输入框。secret 只查「是否已存」，不把值放进输入框。
    fn load_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inputs_for == self.selected {
            return;
        }
        self.inputs_for = self.selected.clone();
        self.inputs.clear();
        self._subscriptions.clear();
        let Some(plugin) = self
            .selected
            .as_deref()
            .and_then(entry)
            .and_then(|entry| entry.plugin().cloned())
        else {
            return;
        };
        let config = config::get();
        for pref in &plugin.manifest.preferences {
            if !matches!(pref.kind, PrefKind::Text | PrefKind::Secret) {
                continue;
            }
            let (value, stored) = match pref.kind {
                PrefKind::Secret => {
                    let stored = match secrets::get(plugin.id(), &pref.id) {
                        Ok(value) => value.is_some_and(|v| !v.is_empty()),
                        Err(err) => {
                            self.status = Some(Err(format!("{err:#}")));
                            false
                        }
                    };
                    (String::new(), stored)
                }
                _ => (
                    config
                        .plugins
                        .get(plugin.id())
                        .and_then(|settings| settings.prefs.get(&pref.id))
                        .cloned()
                        .unwrap_or_default(),
                    false,
                ),
            };
            let placeholder = placeholder(pref, stored);
            let secret = pref.kind == PrefKind::Secret;
            let state = cx.new(|cx| {
                let mut state = InputState::new(window, cx)
                    .masked(secret)
                    .placeholder(placeholder);
                state.set_value(value, window, cx);
                state
            });
            let (plugin_id, pref_id) = (plugin.id().to_string(), pref.id.clone());
            self._subscriptions.push(cx.subscribe_in(
                &state,
                window,
                move |this, state, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        let value = state.read(cx).value().to_string();
                        this.save_pref(&plugin_id, &pref_id, secret, value, window, cx);
                    }
                },
            ));
            self.inputs.push(PrefInput {
                id: pref.id.clone(),
                kind: pref.kind,
                state,
                stored,
            });
        }
    }

    fn save_pref(
        &mut self,
        plugin_id: &str,
        pref_id: &str,
        secret: bool,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if secret {
            // 空输入框失焦不代表要删除已存的密钥（输入框本来就不回显），删除走「清除」按钮。
            if value.trim().is_empty() {
                return;
            }
            match secrets::set(plugin_id, pref_id, &value) {
                Ok(()) => {
                    self.status = Some(Ok(tr("已存入钥匙串", "Saved to the Keychain").into()));
                    if let Some(input) = self.inputs.iter_mut().find(|input| input.id == pref_id) {
                        input.stored = true;
                        input.state.update(cx, |state, cx| {
                            state.set_value(String::new(), window, cx);
                            state.set_placeholder(
                                tr(
                                    "已存入钥匙串（输入新值可替换）",
                                    "Saved in the Keychain (type to replace)",
                                ),
                                window,
                                cx,
                            );
                        });
                    }
                }
                Err(err) => self.status = Some(Err(format!("{err:#}"))),
            }
            cx.notify();
            return;
        }
        let current = config::get()
            .plugins
            .get(plugin_id)
            .and_then(|settings| settings.prefs.get(pref_id))
            .cloned()
            .unwrap_or_default();
        if current == value.trim() {
            return;
        }
        let (plugin_id, pref_id, value) = (
            plugin_id.to_string(),
            pref_id.to_string(),
            value.trim().to_string(),
        );
        self.save(cx, move |config| {
            set_pref(config, &plugin_id, &pref_id, value)
        });
    }

    fn clear_secret(
        &mut self,
        plugin_id: &str,
        pref_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match secrets::set(plugin_id, pref_id, "") {
            Ok(()) => {
                self.status = Some(Ok(tr("已从钥匙串删除", "Removed from the Keychain").into()));
                if let Some(input) = self.inputs.iter_mut().find(|input| input.id == pref_id) {
                    input.stored = false;
                    input.state.update(cx, |state, cx| {
                        state.set_placeholder(tr("未设置", "Not set"), window, cx)
                    });
                }
            }
            Err(err) => self.status = Some(Err(format!("{err:#}"))),
        }
        cx.notify();
    }

    fn set_enabled(&mut self, entry: &Entry, on: bool, cx: &mut Context<Self>) {
        // 用户插件开启前先确认权限；自带插件直接切换。
        if on && entry.source == Source::User && self.consent.as_deref() != Some(entry.id.as_str())
        {
            self.consent = Some(entry.id.clone());
            self.selected = Some(entry.id.clone());
            cx.notify();
            return;
        }
        self.consent = None;
        let entry = entry.clone();
        self.save(cx, move |config| registry::set_enabled(config, &entry, on));
    }

    fn toolbar(&self, pal: &Palette, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .mb(px(18.))
            .child(widgets::push_button(
                "plugins-open-folder",
                tr("打开插件文件夹", "Open Plugins Folder"),
                false,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    let result = registry::user_dir()
                        .and_then(|dir| Action::open(dir.to_string_lossy()).run());
                    if let Err(err) = result {
                        this.status = Some(Err(format!("{err:#}")));
                        cx.notify();
                    }
                }),
            ))
            .child(widgets::push_button(
                "plugins-reload",
                tr("重新加载", "Reload"),
                false,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    let entries = registry::reload();
                    let broken = entries.iter().filter(|entry| entry.loaded.is_err()).count();
                    this.status = Some(Ok(trf!(
                        "已重新加载 {} 个插件，{broken} 个出错",
                        "Reloaded {} plugins, {broken} with errors",
                        entries.len()
                    )));
                    this.inputs_for = None;
                    this.load_inputs(window, cx);
                    cx.notify();
                }),
            ))
            .child(widgets::push_button(
                "plugins-new",
                tr("新建插件", "New Plugin"),
                false,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    let created = registry::user_dir().and_then(|root| scaffold::create(&root));
                    match created {
                        Ok(dir) => {
                            if let Err(err) = Action::reveal(dir.join("plugin.json").to_string_lossy()).run() {
                                eprintln!("helmp: 在访达中显示新插件失败：{err:#}");
                            }
                            registry::reload();
                            let id = dir
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned());
                            this.select(id, window, cx);
                            this.status = Some(Ok(trf!(
                                "已在 {} 生成模板插件：改好 plugin.json 后在这里启用",
                                "Created a template plugin in {}. Edit plugin.json, then turn it on here.",
                                super::pages::display_path(&dir)
                            )));
                        }
                        Err(err) => this.status = Some(Err(format!("{err:#}"))),
                    }
                    cx.notify();
                }),
            ))
            .child(widgets::push_button(
                "plugins-docs",
                tr("开发者文档", "Developer Docs"),
                false,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    let url = if crate::i18n::is_en() { DOCS_EN } else { DOCS_ZH };
                    if let Err(err) = Action::open(url).run() {
                        this.status = Some(Err(format!("{err:#}")));
                        cx.notify();
                    }
                }),
            ))
            .when_some(self.status.clone(), |el, status| match status {
                Ok(text) => el.child(widgets::help_text(text, pal)),
                Err(text) => el.child(widgets::error_text(text, pal)),
            })
    }

    /// 内置功能（Rust 写的 provider）：只能开关，详细设置在「扩展」等页面。
    fn builtin_group(&self, pal: &Palette, cx: &mut Context<Self>) -> Div {
        let config = config::get();
        let ids = cx
            .try_global::<crate::engine::EngineGlobal>()
            .map(|global| global.0.provider_ids())
            .unwrap_or_default();
        let rows = ids
            .into_iter()
            .map(|id| {
                let meta = model::extension_meta(id);
                let name = if meta.name.is_empty() { id } else { meta.name };
                let enabled = config.extensions.get(id).is_none_or(|ext| ext.enabled);
                widgets::row(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(widgets::tinted_icon(meta.icon, meta.tint, px(24.)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .child(widgets::label(name, pal))
                                .child(widgets::help_text(meta.kind, pal)),
                        ),
                    widgets::switch(
                        SharedString::from(format!("plugins-builtin-{id}")),
                        enabled,
                        &trf!("启用{name}", "Enable {name}"),
                        pal,
                        cx.listener(move |this, on: &bool, _, cx| {
                            let on = *on;
                            this.save(cx, |c| model::set_extension_enabled(c, id, on));
                        }),
                    ),
                )
                .into_any_element()
            })
            .collect();
        widgets::group(rows, pal)
    }

    fn plugin_row(
        &self,
        entry: &Entry,
        config: &Config,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = entry.state(config);
        let open = self.selected.as_deref() == Some(entry.id.as_str());
        let (name, meta, icon) = match entry.plugin() {
            Some(plugin) => {
                let meta = format!(
                    "v{} · {} · {}",
                    plugin.manifest.version,
                    plugin.manifest.author,
                    entry.source.label()
                );
                (
                    plugin.manifest.name.get().to_string(),
                    meta,
                    plugins::icon_for(plugin, None, None),
                )
            }
            // 加载失败：副标题直接写出错原因（第一条），不用点开才知道哪里坏了。
            None => (
                entry.id.clone(),
                match &entry.loaded {
                    Err(errors) if !errors.is_empty() => {
                        format!("{} · {}", entry.source.label(), errors.join(tr("；", "; ")))
                    }
                    _ => entry.source.label().to_string(),
                },
                crate::provider::Icon::Symbol {
                    name: "triangle-alert",
                    tint: crate::provider::Tint::Red,
                },
            ),
        };
        let icon = match icon {
            crate::provider::Icon::Symbol { name, tint } => {
                widgets::tinted_icon(name, tint, px(24.))
            }
            // png 图标：设置页只显示通用符号（列表里会显示真图）。
            _ => widgets::tinted_icon("puzzle", crate::provider::Tint::Gray, px(24.)),
        };
        let tag = match state {
            State::Enabled => None,
            State::Disabled => Some((state.label(), pal.muted)),
            State::NeedsConsent | State::Broken => Some((state.label(), pal.danger)),
        };
        let id = entry.id.clone();
        let control: AnyElement = if state == State::Broken {
            // 名字后面已经标了「出错」，右侧不再重复；留空位与其他行的开关对齐。
            div().w(px(46.)).into_any_element()
        } else {
            let entry_for_switch = entry.clone();
            widgets::switch(
                SharedString::from(format!("plugin-enabled-{}", entry.id)),
                state == State::Enabled,
                &trf!("启用{name}", "Enable {name}"),
                pal,
                cx.listener(move |this, on: &bool, _, cx| {
                    // 开关在可点击的整行里：不拦住的话，这次点击还会冒泡到行上「收起详情」，
                    // 刚弹出的权限确认卡片立刻被清掉，用户插件就怎么也开不了。
                    cx.stop_propagation();
                    this.set_enabled(&entry_for_switch, *on, cx)
                }),
            )
            .into_any_element()
        };
        let line = div()
            .id(SharedString::from(format!("plugin-row-{}", entry.id)))
            .flex()
            .items_center()
            .gap(px(12.))
            .min_h(px(48.))
            .px(px(14.))
            .py(px(6.))
            .cursor_pointer()
            .when(open, |el| el.bg(pal.accent.opacity(0.08)))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                let next = (this.selected.as_deref() != Some(id.as_str())).then(|| id.clone());
                this.select(next, window, cx);
            }))
            .child(icon)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(widgets::label(name, pal).truncate())
                            .when_some(tag, |el, (text, color)| {
                                el.child(div().text_size(px(11.)).text_color(color).child(text))
                            }),
                    )
                    .child(widgets::help_text(meta, pal).truncate()),
            )
            .child(div().flex_none().child(control));
        let mut column = div().flex().flex_col().child(line);
        if self.consent.as_deref() == Some(entry.id.as_str()) {
            column = column.child(self.consent_card(entry, pal, cx));
        }
        if open {
            column = column.child(self.details(entry, pal, cx));
        }
        column.into_any_element()
    }

    fn consent_card(&self, entry: &Entry, pal: &Palette, cx: &mut Context<Self>) -> Div {
        let lines = entry
            .plugin()
            .map(|plugin| plugin.permissions.describe())
            .unwrap_or_default();
        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .mx(px(14.))
            .mb(px(10.))
            .p(px(12.))
            .rounded(px(8.))
            .border_1()
            .border_color(pal.danger.opacity(0.5))
            .child(widgets::label(
                tr(
                    "启用前请确认这个插件可以做的事：",
                    "Before turning it on, review what this plugin can do:",
                ),
                pal,
            ));
        if lines.is_empty() {
            card = card.child(widgets::help_text(
                tr(
                    "没有额外权限（只在 Helmp 里显示内容）",
                    "No extra permissions",
                ),
                pal,
            ));
        }
        for line in lines {
            card = card.child(widgets::help_text(format!("• {line}"), pal));
        }
        let allow = entry.clone();
        card.child(
            div()
                .flex()
                .gap(px(8.))
                .mt(px(4.))
                .child(widgets::push_button(
                    SharedString::from(format!("plugin-consent-{}", entry.id)),
                    tr("允许并启用", "Allow and Turn On"),
                    false,
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.consent = None;
                        let entry = allow.clone();
                        this.save(cx, move |config| {
                            registry::set_enabled(config, &entry, true)
                        });
                    }),
                ))
                .child(widgets::push_button(
                    SharedString::from(format!("plugin-consent-cancel-{}", entry.id)),
                    tr("取消", "Cancel"),
                    false,
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.consent = None;
                        cx.notify();
                    }),
                )),
        )
    }

    fn details(&self, entry: &Entry, pal: &Palette, cx: &mut Context<Self>) -> Div {
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .pl(px(50.))
            .pr(px(14.))
            .pb(px(12.));
        let plugin = match &entry.loaded {
            Err(errors) => {
                body = body.child(widgets::label(tr("无法加载：", "Couldn't load:"), pal));
                for error in errors {
                    body = body.child(widgets::error_text(error.clone(), pal));
                }
                if let Some(dir) = &entry.dir {
                    body = body.child(widgets::help_text(super::pages::display_path(dir), pal));
                }
                return body;
            }
            Ok(plugin) => Arc::clone(plugin),
        };
        let manifest = &plugin.manifest;
        if let Some(description) = &manifest.description {
            body = body.child(widgets::help_text(description.get().to_string(), pal));
        }
        if let Some(dir) = &entry.dir {
            body = body.child(widgets::help_text(super::pages::display_path(dir), pal));
        }
        if let Some(homepage) = manifest.homepage.clone() {
            body = body.child(div().flex().child(widgets::push_button(
                SharedString::from(format!("plugin-homepage-{}", entry.id)),
                tr("打开主页", "Open Homepage"),
                false,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Err(err) = Action::open(homepage.clone()).run() {
                        this.status = Some(Err(format!("{err:#}")));
                        cx.notify();
                    }
                }),
            )));
        }
        // 命令：标题 + 触发词。
        body = body.child(widgets::label(tr("命令", "Commands"), pal).mt(px(4.)));
        for command in &manifest.commands {
            let trigger = command.keywords.first().cloned().unwrap_or_default();
            let text = if trigger.is_empty() {
                command.title.get().to_string()
            } else {
                format!("{} · {trigger}", command.title.get())
            };
            body = body.child(widgets::help_text(text, pal));
        }
        body = body.child(widgets::label(tr("权限", "Permissions"), pal).mt(px(4.)));
        let lines = plugin.permissions.describe();
        if lines.is_empty() {
            body = body.child(widgets::help_text(tr("无", "None"), pal));
        }
        for line in lines {
            body = body.child(widgets::help_text(format!("• {line}"), pal));
        }
        if !manifest.preferences.is_empty() {
            body = body.child(widgets::label(tr("偏好设置", "Preferences"), pal).mt(px(4.)));
            for pref in &manifest.preferences {
                body = body.child(self.pref_row(&plugin, pref, pal, cx));
            }
        }
        body
    }

    fn pref_row(
        &self,
        plugin: &Arc<registry::Plugin>,
        pref: &Preference,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let title = if pref.required {
            format!("{} *", pref.title.get())
        } else {
            pref.title.get().to_string()
        };
        let mut label = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(widgets::label(title, pal));
        if let Some(description) = &pref.description {
            label = label.child(widgets::help_text(description.get().to_string(), pal));
        }
        let plugin_id = plugin.id().to_string();
        let pref_id = pref.id.clone();
        let current = config::get()
            .plugins
            .get(plugin.id())
            .and_then(|settings| settings.prefs.get(&pref.id))
            .cloned()
            .or_else(|| pref.default.clone())
            .unwrap_or_default();
        let control: AnyElement = match pref.kind {
            PrefKind::Text | PrefKind::Secret => {
                let Some(input) = self.inputs.iter().find(|input| input.id == pref.id) else {
                    return label;
                };
                let field = div()
                    .flex()
                    .items_center()
                    .h(px(26.))
                    .w(px(320.))
                    .px(px(6.))
                    .rounded(px(6.))
                    .bg(pal.control)
                    .border_1()
                    .border_color(pal.separator)
                    .text_size(px(12.))
                    .child(Input::new(&input.state).appearance(false));
                let mut controls = div().flex().items_center().gap(px(6.)).child(field);
                if input.kind == PrefKind::Secret && input.stored {
                    let (plugin_id, pref_id) = (plugin_id.clone(), pref_id.clone());
                    controls = controls.child(widgets::danger_button(
                        SharedString::from(format!("plugin-secret-clear-{plugin_id}-{pref_id}")),
                        tr("清除", "Clear"),
                        false,
                        pal,
                        cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.clear_secret(&plugin_id, &pref_id, window, cx)
                        }),
                    ));
                }
                controls.into_any_element()
            }
            PrefKind::Checkbox => widgets::switch(
                SharedString::from(format!("plugin-pref-{plugin_id}-{pref_id}")),
                current == "true",
                pref.title.get(),
                pal,
                cx.listener(move |this, on: &bool, _, cx| {
                    let (plugin_id, pref_id, on) = (plugin_id.clone(), pref_id.clone(), *on);
                    this.save(cx, move |config| {
                        set_pref(config, &plugin_id, &pref_id, on.to_string())
                    });
                }),
            )
            .into_any_element(),
            PrefKind::Select => {
                let shown = pref
                    .options
                    .iter()
                    .find(|option| option.value == current)
                    .map(|option| option.title.get().to_string())
                    .unwrap_or_else(|| tr("未选择", "Not set").to_string());
                let this = cx.entity().downgrade();
                let choices = pref
                    .options
                    .iter()
                    .map(|option| {
                        let (plugin_id, pref_id, value) =
                            (plugin_id.clone(), pref_id.clone(), option.value.clone());
                        let this = this.clone();
                        let handler: widgets::MenuHandler = std::rc::Rc::new(move |_, cx| {
                            let (plugin_id, pref_id, value) =
                                (plugin_id.clone(), pref_id.clone(), value.clone());
                            let _ = this.update(cx, |this, cx| {
                                this.save(cx, move |config| {
                                    set_pref(config, &plugin_id, &pref_id, value)
                                })
                            });
                        });
                        (
                            option.title.get().to_string(),
                            option.value == current,
                            handler,
                        )
                    })
                    .collect();
                widgets::popup(
                    SharedString::from(format!("plugin-pref-{plugin_id}-{pref_id}")),
                    shown,
                    choices,
                )
                .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .py(px(4.))
            .child(label)
            .child(control)
    }
}

/// 非密钥偏好设置写进配置；空值等于恢复清单默认值。
fn set_pref(config: &mut Config, plugin_id: &str, pref_id: &str, value: String) {
    let settings = config.plugins.entry(plugin_id.to_string()).or_default();
    if value.is_empty() {
        settings.prefs.remove(pref_id);
    } else {
        settings.prefs.insert(pref_id.to_string(), value);
    }
}

fn placeholder(pref: &Preference, stored: bool) -> String {
    if pref.kind == PrefKind::Secret && stored {
        return tr(
            "已存入钥匙串（输入新值可替换）",
            "Saved in the Keychain (type to replace)",
        )
        .into();
    }
    pref.placeholder
        .as_ref()
        .map(|text| text.get().to_string())
        .or_else(|| pref.default.clone())
        .unwrap_or_default()
}

impl Render for PluginsPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 选中的插件可能被删掉或重命名（热重载），对齐一下。
        if self
            .selected
            .as_deref()
            .is_some_and(|id| entry(id).is_none())
        {
            self.selected = None;
        }
        self.load_inputs(window, cx);
        let pal = style::palette(window);
        let config = config::get();
        let entries = registry::current();
        let (bundled, user): (Vec<&Entry>, Vec<&Entry>) = entries
            .iter()
            .partition(|entry| entry.source == Source::Bundled);
        let mut rows = Vec::new();
        for entry in &bundled {
            rows.push(self.plugin_row(entry, &config, &pal, cx));
        }
        let mut user_rows = Vec::new();
        for entry in &user {
            user_rows.push(self.plugin_row(entry, &config, &pal, cx));
        }
        if user_rows.is_empty() {
            user_rows.push(
                div()
                    .px(px(14.))
                    .py(px(12.))
                    .child(widgets::help_text(
                        tr(
                            "还没有安装插件。把插件文件夹放进插件文件夹，或点「新建插件」从模板开始。",
                            "No plugins installed yet. Drop a plugin folder into the plugins folder, or click New Plugin to start from a template.",
                        ),
                        &pal,
                    ))
                    .into_any_element(),
            );
        }
        let extension_off = !config::extension_enabled(plugins::PROVIDER_ID);
        div()
            .flex()
            .flex_col()
            .child(self.toolbar(&pal, cx))
            .when(extension_off, |el| {
                el.child(div().mb(px(12.)).child(widgets::error_text(
                    tr(
                        "「清单插件」扩展已在下方内置功能里关闭，所有插件都不会出现在搜索里",
                        "The Manifest Plugins extension is turned off, so no plugin shows up in search",
                    ),
                    &pal,
                )))
            })
            .child(widgets::group_header(tr("自带插件", "Bundled Plugins"), &pal))
            .child(widgets::group(rows, &pal))
            .child(widgets::group_header(tr("我的插件", "My Plugins"), &pal))
            .child(widgets::group(user_rows, &pal).mb(px(6.)))
            .child(div().mb(px(18.)).ml(px(4.)).child(widgets::help_text(
                tr(
                    "用户插件默认关闭：开启前会列出它要的权限。密钥类设置只存钥匙串。",
                    "Your plugins start turned off; you'll see their permissions before turning them on. Secrets are kept in the Keychain only.",
                ),
                &pal,
            )))
            .child(widgets::group_header(tr("内置功能", "Built-in Features"), &pal))
            .child(self.builtin_group(&pal, cx))
    }
}
