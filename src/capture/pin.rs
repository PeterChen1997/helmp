//! 钉图：选区原地变成一张置顶的无边框小窗，只用 AppKit 的窗口阴影。
//! 图片的位置、大小和选区完全一样，可以钉很多张。
//!
//! 拖动窗口任意处移动它；鼠标移上去时右下角出现工具栏（保存、识别文字、关闭、复制）；
//! Esc、⌘W 或双击关闭；获得焦点后 ⌘C 复制、⌘S 保存。点它不会激活 Helmp，不打断用户在别的 App 里的输入。
//!
//! 在钉图上滚动滚轮缩放（往上滚放大、往下滚缩小；触控板双指滚动按距离连续缩放，捏合也行），
//! 以光标为中心：光标下的那一点不动。范围 [`MIN_ZOOM`]–[`MAX_ZOOM`]，缩放时中间短暂显示百分比。
//! 图片一直是原始分辨率的那张（NSImageView 随窗口缩放），放大不会糊成缩略图。

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::*;

use super::geometry::{self, Rect};
use super::toolbar::{ActionButton, button_row};
use super::{Act, Shot, log, native, offscreen, state};
use crate::i18n::tr;
use crate::style;

/// 滚轮缩放的范围（相对钉上时的大小）：10%–800%。再小只剩几个点、找不回来；再大一块屏也放不下
/// （Retina 截图到 200% 已是一个截图像素对一个物理像素，800% 足够看清单个像素）。
pub const MIN_ZOOM: f64 = 0.1;
pub const MAX_ZOOM: f64 = 8.0;
/// 缩小时最短边至少这么多点：再小就点不中、拖不动了（小截图的 10% 可能只有一两个点）。
const MIN_SIDE: f64 = 16.0;
/// 滚轮每格（按行滚动）的缩放倍数。
const WHEEL_STEP: f64 = 1.1;
/// 触控板等精确滚动：每滚动 1 点的缩放指数，滚 100 点约放大 1.35 倍。
const PIXEL_RATE: f64 = 0.003;
/// 百分比提示显示多久。
const ZOOM_LABEL_FOR: Duration = Duration::from_millis(900);

pub struct Pin {
    shot: Shot,
    focus: FocusHandle,
    hovered: bool,
    /// 钉上时的大小（点），缩放以它为 100%。
    base: (f64, f64),
    /// 当前缩放倍数。
    pub zoom: f64,
    /// 百分比提示：每缩放一次加一，提示只在最后一次缩放后的 [`ZOOM_LABEL_FOR`] 内显示。
    zoom_flash: u64,
    zoom_label: bool,
}

/// 钉图的工具栏按钮（遮罩工具栏去掉「钉图」）。
const BUTTONS: [ActionButton; 4] = [
    super::toolbar::ACTIONS[0],
    super::toolbar::ACTIONS[2],
    super::toolbar::ACTIONS[3],
    super::toolbar::ACTIONS[4],
];

/// 钉图窗口的位置：就是选区原来的位置（AppKit 全局坐标）。
pub fn frame(shot: &Shot) -> Rect {
    geometry::to_appkit(shot.screen, shot.rect)
}

/// 一次滚动的缩放倍数（> 1 放大）：滚轮按格、每格 ×1.1；触控板按滚动距离连续变化（不会一跳一跳）。
/// 往上滚（delta.y > 0）放大、往下滚缩小。单次限制在 0.5–2 倍，猛地一甩也不至于跳得太远。
pub fn zoom_factor(delta: ScrollDelta) -> f64 {
    let factor = match delta {
        ScrollDelta::Lines(lines) => WHEEL_STEP.powf(f64::from(lines.y)),
        ScrollDelta::Pixels(pixels) => (f64::from(pixels.y) * PIXEL_RATE).exp(),
    };
    factor.clamp(0.5, 2.0)
}

/// 把缩放倍数限制在 [`MIN_ZOOM`]–[`MAX_ZOOM`]，并且缩小后最短边不小于 [`MIN_SIDE`] 点
/// （本来就比它小的钉图不再缩小，最多保持原样）。
pub fn clamp_zoom(zoom: f64, base: (f64, f64)) -> f64 {
    let short = base.0.min(base.1).max(1.0);
    let min = MIN_ZOOM.max(MIN_SIDE / short).min(1.0);
    zoom.clamp(min, MAX_ZOOM)
}

/// 缩放后的窗口位置（AppKit 全局坐标，y 向上）。`size` 是新大小，`anchor` 是光标在窗口里的位置
/// （左上角为原点，点）：缩放前后光标下的是图上同一点。取整到整点，图片不会因为半点偏移发虚。
pub fn zoomed_frame(frame: Rect, size: (f64, f64), anchor: (f64, f64)) -> Rect {
    let fx = (anchor.0 / frame.w).clamp(0.0, 1.0);
    let fy = (anchor.1 / frame.h).clamp(0.0, 1.0);
    let cursor_x = frame.x + fx * frame.w;
    // 光标的全局 y：AppKit 的 y 从下往上，窗口顶边是 frame.y + frame.h。
    let cursor_y = frame.y + frame.h - fy * frame.h;
    let (w, h) = (size.0.round().max(1.0), size.1.round().max(1.0));
    Rect::new(
        (cursor_x - fx * w).round(),
        (cursor_y - (1.0 - fy) * h).round(),
        w,
        h,
    )
}

/// 「150%」。
pub fn zoom_label(zoom: f64) -> String {
    format!("{}%", (zoom * 100.0).round())
}

pub fn open(shot: Shot, cx: &mut App) {
    let frame = frame(&shot);
    let offscreen = offscreen();
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
            point(px(0.), px(0.)),
            size(px(frame.w as f32), px(frame.h as f32)),
        ))),
        ..Default::default()
    };
    let png = shot.png.clone();
    let mut view = None;
    let opened = crate::settings::open_transparent_window(options, cx, |window, cx| {
        let entity = cx.new(|cx| Pin::new(shot, window, cx));
        view = Some(entity.clone());
        entity
    });
    let (handle, view) = match (opened, view) {
        (Ok(handle), Some(view)) => (handle, view),
        (Ok(_), None) => return,
        (Err(err), _) => {
            super::fail(
                err.context(tr("钉图失败", "Couldn't pin the screenshot")),
                cx,
            );
            return;
        }
    };
    let _ = handle.update(cx, |_, window, _| {
        native::setup(window, native::Kind::Pin, frame, offscreen);
        native::underlay_png(window, &png, (frame.w, frame.h));
        native::present(window, false, offscreen);
    });
    state(cx).pins.push((handle, view));
    log(cx, format!("pinned {}x{}", frame.w, frame.h));
}

impl Pin {
    fn new(shot: Shot, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            base: (shot.rect.w, shot.rect.h),
            shot,
            focus,
            hovered: false,
            zoom: 1.0,
            zoom_flash: 0,
            zoom_label: false,
        }
    }

    /// 钉图此刻在屏幕上的位置换算成「所在屏 + 本屏坐标」，复制 / 保存 / 识别的提示和结果窗口据此摆放。
    fn current(&self, window: &Window) -> Shot {
        let mut shot = self.shot.clone();
        if let (Some(now), Some((screen, visible))) =
            (native::frame(window), native::screen_of(window))
        {
            shot.screen = screen;
            shot.rect = geometry::from_appkit(screen, now);
            shot.visible = geometry::from_appkit(screen, visible);
        }
        shot
    }

    /// 自检用：「300x90@150%」（窗口此刻的点尺寸和缩放百分比）。GPUI 的视口没跟上窗口尺寸时
    /// （工具栏、悬停和滚动区域还停在旧尺寸）在后面注明，测试据此发现。
    pub fn summary(&self, window: &Window) -> String {
        let (w, h) = native::frame(window).map_or((0.0, 0.0), |frame| (frame.w, frame.h));
        let mut text = format!("{w}x{h}@{}", zoom_label(self.zoom));
        let viewport = window.viewport_size();
        let (vw, vh) = (f64::from(viewport.width), f64::from(viewport.height));
        if (vw - w).abs() > 0.5 || (vh - h).abs() > 0.5 {
            text.push_str(&format!(" gpui={vw}x{vh}"));
        }
        text
    }

    /// 以 `anchor`（窗口坐标，左上角为原点）为中心把缩放倍数乘上 `factor`。
    fn zoom_by(
        &mut self,
        factor: f64,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let zoom = clamp_zoom(self.zoom * factor, self.base);
        if (zoom - self.zoom).abs() > f64::EPSILON
            && let Some(frame) = native::frame(window)
        {
            let size = (self.base.0 * zoom, self.base.1 * zoom);
            let anchor = (f64::from(anchor.x), f64::from(anchor.y));
            native::set_frame_later(window, zoomed_frame(frame, size, anchor), cx);
            self.zoom = zoom;
        }
        // 到了上下限也提示一下当前百分比：用户知道不是没反应，而是到头了。
        self.zoom_flash += 1;
        self.zoom_label = true;
        cx.notify();
        let flash = self.zoom_flash;
        cx.spawn(async move |pin, cx: &mut AsyncApp| {
            cx.background_executor().timer(ZOOM_LABEL_FOR).await;
            let _ = pin.update(cx, |pin, cx| {
                if pin.zoom_flash == flash {
                    pin.zoom_label = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn on_scroll(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.zoom_by(zoom_factor(event.delta), event.position, window, cx);
    }

    fn on_pinch(&mut self, event: &PinchEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let factor = (1.0 + f64::from(event.delta)).clamp(0.5, 2.0);
        self.zoom_by(factor, event.position, window, cx);
    }

    fn run(&mut self, action: Act, window: &mut Window, cx: &mut Context<Self>) {
        let shot = self.current(window);
        let handle = window.window_handle();
        cx.defer(move |cx| match action {
            Act::Copy => {
                super::copy(&shot, cx);
            }
            Act::Save => {
                super::remember_return_app(false, cx);
                super::save(shot, false, cx);
            }
            Act::Recognize => {
                super::remember_return_app(false, cx);
                super::result::open(shot, cx);
            }
            Act::Close | Act::Pin => close(handle, cx),
        });
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let command = keystroke.modifiers.platform;
        let action = match keystroke.key.as_str() {
            _ if super::is_close_key(keystroke) => Act::Close,
            "c" if command => Act::Copy,
            "s" if command => Act::Save,
            _ => return,
        };
        cx.stop_propagation();
        self.run(action, window, cx);
    }

    fn on_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        if event.click_count >= 2 {
            self.run(Act::Close, window, cx);
        } else if !offscreen() {
            // 交给系统拖动窗口：跟手、能拖到别的屏上。
            window.start_window_move();
        }
    }
}

fn close(handle: AnyWindowHandle, cx: &mut App) {
    state(cx).pins.retain(|(pin, _)| *pin != handle);
    let _ = handle.update(cx, |_, window, _| window.remove_window());
    log(cx, "unpinned".into());
}

impl Render for Pin {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = style::palette(window);
        let entity = cx.entity();
        let toolbar = button_row(
            &BUTTONS,
            Act::Copy,
            &pal,
            Rc::new(move |action, window, cx| {
                entity.update(cx, |pin, cx| pin.run(action, window, cx));
            }),
        );
        let mut root = div()
            .id("capture-pin")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .cursor(CursorStyle::OpenHand)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = *hovered;
                cx.notify();
            }))
            .on_key_down(cx.listener(Self::on_key))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_pinch(cx.listener(Self::on_pinch))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down));
        if self.hovered {
            root = root.child(div().absolute().right(px(6.)).bottom(px(6.)).child(toolbar));
        }
        if self.zoom_label {
            root = root.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .px(px(10.))
                            .py(px(4.))
                            .rounded(px(8.))
                            .bg(black().opacity(0.72))
                            .text_color(white())
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(zoom_label(self.zoom)),
                    ),
            );
        }
        root
    }
}

#[cfg(test)]
mod tests {
    // gpui_kit::* 里也有一个 `test` 属性宏，经 `super::*` 引入后会遮住标准库的。
    use core::prelude::v1::test;

    use super::*;
    use std::sync::Arc;

    #[test]
    fn pin_frame_is_the_selection_frame() {
        // 钉图要原地「浮起来」：位置和大小都和选区一样，用户才知道钉住的是哪块。
        let shot = Shot {
            png: Arc::new(Vec::new()),
            pixels: (600, 300),
            rect: Rect::new(100.0, 200.0, 300.0, 150.0),
            screen: Rect::new(0.0, 0.0, 1512.0, 982.0),
            visible: Rect::new(0.0, 37.0, 1512.0, 880.0),
        };
        assert_eq!(frame(&shot), Rect::new(100.0, 632.0, 300.0, 150.0));
        assert_eq!(geometry::from_appkit(shot.screen, frame(&shot)), shot.rect);
    }

    /// 窗口 `frame` 里 `anchor` 处（左上角为原点）的全局坐标（AppKit，y 向上）。
    fn global(frame: Rect, anchor: (f64, f64)) -> (f64, f64) {
        (frame.x + anchor.0, frame.y + frame.h - anchor.1)
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor_fixed() {
        // 以光标为中心缩放：用户盯着的那个字缩放前后都还在光标下面，不用再去找。
        let frame = Rect::new(100.0, 500.0, 300.0, 150.0);
        let anchor = (60.0, 30.0); // 图上 (20%, 20%) 处
        let cursor = global(frame, anchor);
        let zoomed = zoomed_frame(frame, (600.0, 300.0), anchor);
        assert_eq!((zoomed.w, zoomed.h), (600.0, 300.0));
        // 新窗口里图上同一点（20%, 20%）的全局位置仍是光标处。
        let same = global(zoomed, (0.2 * zoomed.w, 0.2 * zoomed.h));
        assert!(
            (same.0 - cursor.0).abs() <= 0.5 && (same.1 - cursor.1).abs() <= 0.5,
            "{same:?} vs {cursor:?}"
        );
        // 缩回原大小就回到原位。
        let back = zoomed_frame(zoomed, (300.0, 150.0), (0.2 * zoomed.w, 0.2 * zoomed.h));
        assert_eq!(back, frame);
        // 光标在左上角：左边和顶边不动。
        let corner = zoomed_frame(frame, (450.0, 225.0), (0.0, 0.0));
        assert_eq!(
            (corner.x, corner.y + corner.h),
            (frame.x, frame.y + frame.h)
        );
    }

    #[test]
    fn zoom_is_clamped_to_sensible_bounds() {
        // 范围 10%–800%：滚过头不会缩成看不见的一点，也不会放到一块屏都装不下还在长。
        let base = (600.0, 400.0);
        assert_eq!(clamp_zoom(20.0, base), MAX_ZOOM);
        assert_eq!(clamp_zoom(0.01, base), MIN_ZOOM);
        assert_eq!(clamp_zoom(1.5, base), 1.5);
        // 小截图缩小时最短边不小于 16 点：40 点高的钉图最多缩到 40%。
        assert!((clamp_zoom(0.1, (200.0, 40.0)) - 0.4).abs() < 1e-9);
        // 本来就比 16 点还小的：不再缩小，但还能放大。
        assert_eq!(clamp_zoom(0.5, (10.0, 8.0)), 1.0);
        assert_eq!(clamp_zoom(3.0, (10.0, 8.0)), 3.0);
    }

    #[test]
    fn wheel_up_zooms_in_and_trackpad_is_proportional() {
        // 滚轮往上（delta.y > 0）放大、往下缩小，每格 10%，上一格再下一格回到原样。
        let up = zoom_factor(ScrollDelta::Lines(point(0.0, 1.0)));
        let down = zoom_factor(ScrollDelta::Lines(point(0.0, -1.0)));
        assert!((up - 1.1).abs() < 1e-9, "{up}");
        assert!((up * down - 1.0).abs() < 1e-9, "{up} {down}");
        // 触控板：滚得越多缩放越多，分几次滚和一次滚完结果一样（连续、不跳）。
        let small = zoom_factor(ScrollDelta::Pixels(point(px(0.), px(10.))));
        let big = zoom_factor(ScrollDelta::Pixels(point(px(0.), px(40.))));
        assert!(small > 1.0 && big > small);
        assert!((small.powi(4) - big).abs() < 1e-9);
        // 一次甩很远也最多翻倍 / 减半。
        assert_eq!(
            zoom_factor(ScrollDelta::Pixels(point(px(0.), px(5000.)))),
            2.0
        );
        assert_eq!(zoom_factor(ScrollDelta::Lines(point(0.0, -40.0))), 0.5);
        // 横向滚动不缩放。
        assert_eq!(zoom_factor(ScrollDelta::Lines(point(3.0, 0.0))), 1.0);
        assert_eq!(zoom_label(1.5), "150%");
    }
}
