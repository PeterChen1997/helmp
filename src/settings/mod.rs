//! 设置窗口（设计稿 09–12：通用、扩展、模型、Agent、剪贴板、截图、外观、关于）。
//!
//! 标准窗口 840 × 640，透明标题栏保留红绿灯；左侧毛玻璃侧栏（搜索 + 8 个页面），右侧分组表单。
//! 每个控件直接读写 `crate::config`：渲染时读当前快照，改动立即 `config::update` 落盘；
//! 写盘失败时在页面底部显示原因，配置保持原值。

mod hotkey;
mod model;
mod pages;
pub(crate) mod system;
mod widgets;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::config::{self, Config};
use crate::style::{self, Palette};

use hotkey::{HotkeyEvent, HotkeyRecorder};
use model::Page;

const WINDOW_SIZE: Size<Pixels> = size(px(840.), px(640.));
const SIDEBAR_WIDTH: Pixels = px(220.);

/// 唯一的设置窗口。
struct SettingsWindow(AnyWindowHandle);

impl Global for SettingsWindow {}

/// 打开（或前置）设置窗口。菜单栏「设置…」与面板里的 ⌘, 调用它。
pub fn open(cx: &mut App) {
    // App 是 Accessory 策略：不激活整个 App，窗口只会出现在别的 App 后面。
    cx.activate(true);
    if let Some(handle) = cx.try_global::<SettingsWindow>().map(|global| global.0)
        && handle
            .update(cx, |_, window, cx| {
                sync_component_theme(window, cx);
                window.activate_window();
            })
            .is_ok()
    {
        return;
    }

    let bounds = Bounds::centered(None, WINDOW_SIZE, cx);
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Helmp 设置".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(20.), px(20.))),
        }),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(720.), px(520.))),
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        is_movable: true,
        is_resizable: true,
        is_minimizable: true,
        // 毛玻璃由系统提供；内容区画 94% 的底色，侧栏画 70%，侧栏就透出模糊的桌面。
        window_background: WindowBackgroundAppearance::Blurred,
        ..Default::default()
    };
    match open_transparent_window(options, cx, |window, cx| {
        cx.new(|cx| SettingsView::new(window, cx))
    }) {
        Ok(handle) => {
            cx.set_global(SettingsWindow(handle));
            let _ = handle.update(cx, |_, window, cx| {
                sync_component_theme(window, cx);
                window.activate_window();
            });
        }
        Err(err) => eprintln!("helmp: 打开设置窗口失败：{err:#}"),
    }
}

/// 同 `gpui_kit::open_window`，但 Root 底色透明：Root 默认铺主题底色（不透明），会盖住毛玻璃。
pub(crate) fn open_transparent_window<V: Render>(
    options: WindowOptions,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> anyhow::Result<AnyWindowHandle> {
    let window = cx.open_window(options, |window, cx| {
        let view = build(window, cx);
        cx.new(|cx| base::Root::new(view, window, cx).bg(transparent_black()))
    })?;
    Ok(window.into())
}

/// 让 GPUI Component 的按钮、菜单、对话框跟随 `style::palette` 的明暗。
/// 组件主题是全局的：只在模式真的不同时切换，避免反复刷新所有窗口。
pub(crate) fn sync_component_theme(window: &mut Window, cx: &mut App) {
    let mode = if style::palette(window).dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    if cx.theme().mode != mode {
        Theme::change(mode, Some(window), cx);
    }
}

/// 品牌图标（1024 px 原图缩放，Retina 下也清晰）。关于页与引导窗口共用。
pub(crate) fn brand_icon(size: Pixels) -> Img {
    static BYTES: &[u8] = include_bytes!("../../resources/AppIcon-1024.png");
    img(std::sync::Arc::new(Image::from_bytes(
        ImageFormat::Png,
        BYTES.to_vec(),
    )))
    .flex_none()
    .size(size)
}

/// 扩展表格里的一行。
struct ExtensionRow {
    id: &'static str,
    /// 插件声明的范围前缀（去掉空格），别名为空时作为占位提示。
    default_alias: Option<String>,
    alias: Entity<InputState>,
    hotkey: Entity<HotkeyRecorder>,
}

pub struct SettingsView {
    page: Page,
    search: Entity<InputState>,
    search_query: String,
    hotkey: Entity<HotkeyRecorder>,
    extensions: Vec<ExtensionRow>,
    models: AnyView,
    /// 扩展页脚本目录列表里选中的一项（用于「−」）。
    selected_script_dir: Option<String>,
    /// 剪贴板页忽略列表里选中的 bundle id。
    selected_ignored_app: Option<String>,
    /// 最近一次保存失败的原因。
    error: Option<String>,
    /// 「开机时自动启动」注册失败的原因（例如未打包运行）。
    login_error: Option<String>,
    /// 登录项的系统状态；未打包运行时为 None（查不到）。
    login_status: Option<system::LoginItemStatus>,
    /// bundle id → 已安装 App 的路径（忽略列表显示名字和图标用），按需查询后缓存。
    app_paths: std::collections::HashMap<String, Option<std::path::PathBuf>>,
    /// 已安排一次稍后重绘，等后台生成的 App 图标。
    icon_refresh_pending: bool,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索"));
        let hotkey = cx.new(|cx| {
            HotkeyRecorder::new(
                || Some(config::get().general.hotkey.clone()).filter(|hotkey| !hotkey.is_empty()),
                false,
                false,
                window,
                cx,
            )
        });

        let mut subscriptions = vec![
            cx.subscribe_in(
                &search,
                window,
                |this, search, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        this.search_query = search.read(cx).value().to_string();
                        if !this.page.matches(&this.search_query)
                            && let Some(first) = Page::ALL
                                .into_iter()
                                .find(|page| page.matches(&this.search_query))
                        {
                            this.page = first;
                        }
                        cx.notify();
                    }
                },
            ),
            cx.subscribe(&hotkey, |this, _, event: &HotkeyEvent, cx| {
                let HotkeyEvent::Changed(Some(hotkey)) = event else {
                    // 唤起快捷键不能清空，录制框也不提供清除按钮。
                    return;
                };
                let hotkey = hotkey.clone();
                this.save(cx, move |config| config.general.hotkey = hotkey);
            }),
            // 跟随系统外观时，系统切换明暗要同步组件主题。
            cx.observe_window_appearance(window, |_, window, cx| {
                sync_component_theme(window, cx);
            }),
        ];

        let extensions = Self::extension_rows(window, cx, &mut subscriptions);
        let models = crate::models::settings_pane(window, cx);

        // 自检 / 截图用：HELMP_SETTINGS_PAGE=<页面名> 直接打开某一页。
        let page = std::env::var("HELMP_SETTINGS_PAGE")
            .ok()
            .and_then(|name| Page::ALL.into_iter().find(|page| page.title() == name))
            .unwrap_or(Page::General);

        Self {
            page,
            search,
            search_query: String::new(),
            hotkey,
            extensions,
            models,
            selected_script_dir: None,
            selected_ignored_app: None,
            error: None,
            login_error: None,
            login_status: system::running_from_bundle().then(system::login_item_status),
            app_paths: Default::default(),
            icon_refresh_pending: false,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    /// 插件列表来自启动器的引擎；启动器还没创建（理论上不会）时表格为空并打印原因。
    fn extension_rows(
        window: &mut Window,
        cx: &mut Context<Self>,
        subscriptions: &mut Vec<Subscription>,
    ) -> Vec<ExtensionRow> {
        let Some(global) = cx.try_global::<crate::engine::EngineGlobal>() else {
            eprintln!("helmp: 设置窗口找不到搜索引擎，扩展列表为空");
            return Vec::new();
        };
        let engine = global.0.clone();
        let scopes = engine.scopes();
        let mut rows = Vec::new();
        for id in engine.provider_ids() {
            let default_alias = scopes
                .iter()
                .find(|scope| scope.id == id)
                .and_then(|scope| model::default_alias(&scope.prefixes));
            let current = config::get()
                .extensions
                .get(id)
                .and_then(|ext| ext.alias.clone())
                .unwrap_or_default();
            let placeholder = default_alias.clone().unwrap_or_else(|| "—".into());
            let alias = cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                state.set_value(current, window, cx);
                state
            });
            subscriptions.push(cx.subscribe_in(
                &alias,
                window,
                move |this, alias, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        let value = model::normalize_alias(&alias.read(cx).value());
                        let unchanged = config::get()
                            .extensions
                            .get(id)
                            .and_then(|e| e.alias.clone())
                            == value;
                        if !unchanged {
                            this.save(cx, move |config| {
                                model::set_extension_alias(config, id, value)
                            });
                        }
                    }
                },
            ));
            let hotkey = cx.new(|cx| {
                HotkeyRecorder::new(
                    move || {
                        config::get()
                            .extensions
                            .get(id)
                            .and_then(|e| e.hotkey.clone())
                    },
                    true,
                    true,
                    window,
                    cx,
                )
            });
            subscriptions.push(
                cx.subscribe(&hotkey, move |this, _, event: &HotkeyEvent, cx| {
                    let HotkeyEvent::Changed(hotkey) = event;
                    let hotkey = hotkey.clone();
                    this.save(cx, move |config| {
                        model::set_extension_hotkey(config, id, hotkey)
                    });
                }),
            );
            rows.push(ExtensionRow {
                id,
                default_alias,
                alias,
                hotkey,
            });
        }
        rows
    }

    /// 修改配置并落盘；失败时记下原因显示在页面底部（不吞掉）。
    fn save(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Config)) {
        match config::update(f) {
            Ok(()) => self.error = None,
            Err(err) => {
                eprintln!("helmp: 保存设置失败：{err:#}");
                self.error = Some(format!("保存设置失败：{err:#}"));
            }
        }
        cx.notify();
    }

    /// 弹出菜单项用的回调：在菜单的 `App` 上下文里回到本视图保存。
    fn setter(
        &self,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Config) + 'static,
    ) -> widgets::MenuHandler {
        let this = cx.entity().downgrade();
        let f = std::rc::Rc::new(f);
        std::rc::Rc::new(move |_, cx| {
            let f = f.clone();
            if let Err(err) = this.update(cx, |this, cx| this.save(cx, |config| f(config))) {
                eprintln!("helmp: 设置窗口已关闭，未保存：{err:#}");
            }
        })
    }

    fn select_page(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.page != page {
            self.page = page;
            self.scroll.set_offset(point(px(0.), px(0.)));
            cx.notify();
        }
    }

    fn render_sidebar(&self, pal: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let mut nav = div().flex().flex_col().gap(px(1.));
        for page in Page::ALL {
            if !page.matches(&self.search_query) {
                continue;
            }
            let on = page == self.page;
            let (icon, tint) = page.icon();
            nav = nav.child(
                div()
                    .id(SharedString::from(format!("nav-{}", page.title())))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .h(px(30.))
                    .px(px(8.))
                    .rounded(px(8.))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(on, |el| el.bg(pal.selection).text_color(pal.selection_fg))
                    .when(!on, |el| {
                        el.text_color(pal.fg).hover(|el| el.bg(pal.key_bg))
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.select_page(page, cx);
                    }))
                    .child(widgets::tinted_icon(icon, tint, px(20.)))
                    .child(page.title()),
            );
        }

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(SIDEBAR_WIDTH)
            .m(px(8.))
            .mr(px(0.))
            .px(px(8.))
            // 顶部留出红绿灯的位置（标题栏透明，红绿灯画在侧栏里）。
            .pt(px(40.))
            .pb(px(10.))
            .rounded(px(14.))
            .bg(pal.sidebar)
            .border_1()
            .border_color(pal.separator)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(28.))
                    .mx(px(2.))
                    .mb(px(10.))
                    .px(px(8.))
                    .rounded(px(7.))
                    .bg(pal.key_bg)
                    .text_size(px(13.))
                    .child(
                        gpui_kit::component::Icon::empty()
                            .path("icons/search.svg")
                            .text_color(pal.muted)
                            .size(px(13.)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.search).appearance(false)),
                    ),
            )
            .child(nav)
    }

    fn render_page(
        &mut self,
        pal: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.page {
            Page::General => self.render_general(pal, cx),
            Page::Extensions => self.render_extensions(pal, window, cx),
            Page::Models => self.models.clone().into_any_element(),
            Page::Agents => self.render_agents(pal, cx),
            Page::Clipboard => self.render_clipboard(pal, window, cx),
            Page::Screenshot => self.render_screenshot(pal, cx),
            Page::Appearance => self.render_appearance(pal, cx),
            Page::About => self.render_about(pal, cx),
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = style::palette(window);
        let page = self.render_page(&pal, window, cx);
        // 模型页由「翻译与模型」子任务整页提供，不套标题。
        let titled = self.page != Page::Models;

        let mut content = div()
            .id("settings-content")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(px(28.))
            .pt(px(16.))
            .pb(px(20.));
        if titled {
            content = content.child(widgets::page_title(self.page.title(), &pal));
        }
        content = content.child(page);
        if let Some(error) = &self.error {
            content = content.child(
                div()
                    .mt(px(8.))
                    .child(widgets::error_text(error.clone(), &pal)),
            );
        }

        div()
            .id("settings")
            .flex()
            .size_full()
            .bg(pal.window)
            .text_color(pal.fg)
            .text_size(px(13.))
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, cx| {
                // 没有菜单栏菜单，⌘W 关窗要自己处理。
                let keystroke = &event.keystroke;
                if keystroke.modifiers.platform && keystroke.key == "w" {
                    cx.stop_propagation();
                    window.remove_window();
                }
            }))
            .child(self.render_sidebar(&pal, cx))
            .child(content)
    }
}
