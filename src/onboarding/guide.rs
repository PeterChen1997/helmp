//! 授权浮窗：点「前往授权」后，在系统设置窗口内底部浮出一张卡片，
//! 把 Helmp 图标直接拖进权限列表即完成添加（参考 ChatGPT / Friendly Voice 的做法）。
//!
//! - 非激活面板（GPUI PopUp = NonactivatingPanel 的 NSPanel），跨 Space 可见，不抢系统设置的焦点。
//! - 每 0.5 秒读一次系统设置窗口（`native::read_settings_state`）：跟随它的位置；它弹密码框、
//!   用户切到别的 App、它在别的 Space 时让开；用户关掉系统设置时浮窗跟着关。
//! - 每次轮询顺带查权限：授权后显示「已授权」，引导窗口里该项同步刷新，1.5 秒后自动关闭。
//! - 拖拽是真正的系统文件拖拽（`native::DragTileView`），内容是 Helmp.app；
//!   从 target/ 直接运行时没有 .app，卡片置灰并说明原因。

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use objc2::rc::Retained;

use super::native::{self, DragTileView};
use super::placement::{self, Phase, Rect, Tick};
use super::{Onboarding, Permission, Status};
use crate::style::{self, Palette};

const POLL: Duration = Duration::from_millis(500);
/// 系统设置点开后要一会儿才出窗口；先等它，避免浮窗先出现在屏幕底部再跳上去。
const SETTINGS_WAIT_TICKS: u32 = 6;
const CLOSE_AFTER_GRANT: Duration = Duration::from_millis(1500);

// 面板内布局（点，左上角为原点）。拖拽卡片的位置 GPUI 与原生拖拽视图共用。
const PAD: f64 = 16.;
const BACK_SIZE: f64 = 32.;
const CONTENT_X: f64 = PAD + BACK_SIZE + 12.;
const TEXT_TOP: f64 = 12.;
const TEXT_HEIGHT: f64 = 36.;
const TILE: Rect = Rect {
    x: CONTENT_X,
    y: 54.,
    width: placement::GUIDE_WIDTH - CONTENT_X - PAD,
    height: 46.,
};

struct GuideWindow {
    handle: AnyWindowHandle,
    panel: Entity<GuidePanel>,
}

impl Global for GuideWindow {}

/// 打开（或切换到）某项权限的授权浮窗。只用于需要把 App 加进列表的两项权限。
pub fn open(permission: Permission, onboarding: WeakEntity<Onboarding>, cx: &mut App) {
    if let Some(existing) = cx.try_global::<GuideWindow>() {
        let (handle, panel) = (existing.handle, existing.panel.clone());
        let switched = handle.update(cx, |_, window, cx| {
            panel.update(cx, |panel, cx| panel.restart(permission, window, cx));
        });
        if switched.is_ok() {
            return;
        }
    }

    let fallback = native::work_area_for(None)
        .map(|area| placement::guide_bounds(None, area))
        .unwrap_or(Rect::new(
            0.,
            0.,
            placement::GUIDE_WIDTH,
            placement::GUIDE_HEIGHT,
        ));
    let options = WindowOptions {
        kind: WindowKind::PopUp,
        titlebar: None,
        focus: false,
        show: false,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(fallback.x as f32), px(fallback.y as f32)),
            size(px(fallback.width as f32), px(fallback.height as f32)),
        ))),
        ..Default::default()
    };
    let mut built = None;
    let result = crate::settings::open_transparent_window(options, cx, |window, cx| {
        let panel = cx.new(|cx| GuidePanel::new(permission, onboarding, window, cx));
        built = Some(panel.clone());
        panel
    });
    let (handle, Some(panel)) = (
        match result {
            Ok(handle) => handle,
            Err(err) => {
                eprintln!("helmp: 打开授权浮窗失败：{err:#}");
                return;
            }
        },
        built,
    ) else {
        eprintln!("helmp: 授权浮窗没有创建内容视图");
        return;
    };
    let configured = handle.update(cx, |_, window, _| {
        // 圆角毛玻璃与启动器面板一致。
        crate::panel::install_glass(window);
        match crate::panel::ns_window(window) {
            Some(ns_window) => native::configure_panel(&ns_window),
            None => eprintln!("helmp: 授权浮窗取不到 NSWindow，跨 Space 显示不可用"),
        }
    });
    if let Err(err) = configured {
        eprintln!("helmp: 授权浮窗初始化失败：{err:#}");
        return;
    }
    cx.set_global(GuideWindow { handle, panel });
}

/// 关闭授权浮窗（引导完成、跳过时调用）。
pub fn close(cx: &mut App) {
    if let Some(existing) = cx.try_global::<GuideWindow>() {
        let handle = existing.handle;
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

pub struct GuidePanel {
    permission: Permission,
    granted: bool,
    bundle: Option<PathBuf>,
    onboarding: WeakEntity<Onboarding>,
    saw_settings: bool,
    waits: u32,
    shown: bool,
    last_frame: Option<Rect>,
    drag_tile: Option<Retained<DragTileView>>,
    _poll: Task<()>,
    _close: Option<Task<()>>,
}

impl GuidePanel {
    fn new(
        permission: Permission,
        onboarding: WeakEntity<Onboarding>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this
                    .update_in(cx, |this, window, cx| this.tick(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            permission,
            granted: permission.status() == Status::Granted,
            bundle: native::bundle_path(),
            onboarding,
            saw_settings: false,
            waits: 0,
            shown: false,
            last_frame: None,
            drag_tile: None,
            _poll: poll,
            _close: None,
        }
    }

    /// 浮窗还开着时用户点了另一项的「前往授权」：换内容，重新等系统设置跳到新面板。
    fn restart(&mut self, permission: Permission, _window: &mut Window, cx: &mut Context<Self>) {
        self.permission = permission;
        self.granted = permission.status() == Status::Granted;
        self._close = None;
        cx.notify();
    }

    fn phase(&self) -> Phase {
        placement::phase(self.granted, self.bundle.is_some())
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_drag_tile(window);

        if !self.granted && self.permission.status() == Status::Granted {
            self.granted = true;
            if let Some(tile) = &self.drag_tile {
                tile.setHidden(true);
            }
            let _ = self
                .onboarding
                .update(cx, |onboarding, cx| onboarding.refresh(cx));
            self._close = Some(cx.spawn_in(window, async move |_, cx| {
                cx.background_executor().timer(CLOSE_AFTER_GRANT).await;
                let _ = cx.update(|window, _| window.remove_window());
            }));
            cx.notify();
        }

        // 读不到是「不知道」，不是「已关闭」：什么都不做，等下一次。
        let Some(state) = native::read_settings_state() else {
            return;
        };
        if state.settings.is_some() {
            self.saw_settings = true;
        } else if !self.saw_settings && self.waits < SETTINGS_WAIT_TICKS {
            self.waits += 1;
            return;
        }
        let Some(ns_window) = crate::panel::ns_window(window) else {
            return;
        };
        let decision = placement::tick(&state, self.saw_settings);
        crate::trace::log(format!(
            "guide tick：{state:?} saw={} granted={} drag_tile={} → {decision:?}",
            self.saw_settings,
            self.granted,
            self.drag_tile.is_some(),
        ));
        match decision {
            Tick::Close => window.remove_window(),
            Tick::Hide => {
                if self.shown {
                    ns_window.orderOut(None);
                    self.shown = false;
                }
            }
            Tick::Show(settings) => {
                if let Some(area) = native::work_area_for(settings) {
                    let frame = placement::guide_bounds(settings, area);
                    if self.last_frame != Some(frame) {
                        crate::trace::log(format!("guide frame {frame:?}"));
                        native::set_frame(&ns_window, frame);
                        self.last_frame = Some(frame);
                    }
                }
                if !self.shown {
                    ns_window.orderFrontRegardless();
                    self.shown = true;
                }
            }
        }
    }

    /// 拖拽源在第一次轮询时装上（此时 GPUI 视图已经在窗口里）。没有 .app 时不装。
    fn ensure_drag_tile(&mut self, window: &Window) {
        if self.drag_tile.is_some() || self.granted {
            return;
        }
        let Some(bundle) = &self.bundle else {
            return;
        };
        self.drag_tile = native::install_drag_tile(window, TILE, bundle);
        if self.drag_tile.is_none() {
            eprintln!("helmp: 授权浮窗装不上拖拽源，只能在系统设置里用 + 手动添加");
        }
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.remove_window();
        super::show(cx);
    }

    fn instruction(&self) -> String {
        let name = self.permission.title();
        match (self.phase(), self.permission) {
            (Phase::Granted, _) => format!("已授权「{name}」"),
            (_, Permission::ScreenRecording) => {
                format!("把 Helmp 拖进「{name}」列表并打开开关；之后需重新打开 Helmp 才生效")
            }
            _ => format!("把 Helmp 拖进「{name}」列表，然后打开它的开关"),
        }
    }

    fn render_tile(&self, pal: &Palette) -> AnyElement {
        let phase = self.phase();
        let (title, note) = match phase {
            Phase::NoBundle => (
                "无法拖拽：当前不是从 Helmp.app 运行",
                "请在列表下方点 + 手动选择",
            ),
            _ => ("Helmp", "或通过 + 选择应用"),
        };
        div()
            .absolute()
            .left(px(TILE.x as f32))
            .top(px(TILE.y as f32))
            .w(px(TILE.width as f32))
            .h(px(TILE.height as f32))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .rounded(px(10.))
            .bg(pal.card)
            .border_1()
            .border_color(pal.separator)
            .when(phase == Phase::Drag, |el| el.cursor(CursorStyle::OpenHand))
            .when(phase == Phase::NoBundle, |el| el.opacity(0.6))
            .child(crate::settings::brand_icon(px(28.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(pal.fg)
                    .child(title),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(pal.muted)
                    .child(note),
            )
            .into_any_element()
    }
}

impl Render for GuidePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = style::palette(window);
        let phase = self.phase();
        let granted_fg = style::tone_colors(crate::provider::Tone::Done, pal.dark).1;

        let mut text = div()
            .absolute()
            .left(px(CONTENT_X as f32))
            .right(px(PAD as f32))
            .top(px(TEXT_TOP as f32))
            .h(px(TEXT_HEIGHT as f32))
            .flex()
            .items_center()
            .gap(px(8.));
        text = match phase {
            Phase::Granted => text.child(
                gpui_kit::component::Icon::empty()
                    .path("icons/circle-check.svg")
                    .text_color(granted_fg)
                    .size(px(18.)),
            ),
            _ => text.child(
                div()
                    .text_size(px(24.))
                    .line_height(px(24.))
                    .text_color(pal.accent)
                    .child("↑"),
            ),
        };
        text = text.child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(13.))
                .line_height(px(17.))
                .when(phase == Phase::Granted, |el| {
                    el.text_color(granted_fg).font_weight(FontWeight::SEMIBOLD)
                })
                .when(phase != Phase::Granted, |el| el.text_color(pal.fg))
                .child(self.instruction()),
        );

        div()
            .id("permission-guide")
            .relative()
            .size_full()
            .bg(pal.panel)
            .text_color(pal.fg)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.back(window, cx);
                }
            }))
            .child(
                div()
                    .id("guide-back")
                    .absolute()
                    .left(px(PAD as f32))
                    .top(px(((placement::GUIDE_HEIGHT - BACK_SIZE) / 2.) as f32))
                    .size(px(BACK_SIZE as f32))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(pal.key_bg)
                    .cursor_pointer()
                    .hover(|el| el.bg(pal.separator))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.back(window, cx)))
                    .child(
                        gpui_kit::component::Icon::empty()
                            .path("icons/arrow-left.svg")
                            .text_color(pal.fg)
                            .size(px(16.)),
                    ),
            )
            .child(text)
            .when(phase != Phase::Granted, |el| {
                el.child(self.render_tile(&pal))
            })
    }
}

// 原生拖拽视图按 TILE 叠在 GPUI 画的卡片上：跑出面板或压住说明文字，用户就拖不到 / 看不到说明。
const _: () = {
    assert!(TILE.x >= CONTENT_X && TILE.x + TILE.width <= placement::GUIDE_WIDTH - PAD + 0.01);
    assert!(TILE.y >= TEXT_TOP + TEXT_HEIGHT);
    assert!(TILE.y + TILE.height <= placement::GUIDE_HEIGHT - 12.);
};
