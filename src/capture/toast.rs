//! 「已复制」「已保存」提示：截图界面关掉后用户要知道刚才那一下成功了。
//! 不接收鼠标、不抢焦点，出现在选区中央，一会儿自己消失。
//!
//! 录屏存好后的提示（[`show_actions`]）多带几个按钮（复制文件、在访达中显示），可以点，停留时间长一些。

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::*;

use super::geometry::{self, Rect};
use super::{Shot, log, native, offscreen};

const SHOW_FOR: Duration = Duration::from_millis(1400);
/// 带按钮的提示要给人时间去点。
const SHOW_ACTIONS_FOR: Duration = Duration::from_secs(6);
const HEIGHT: f64 = 40.0;
const ACTIONS_HEIGHT: f64 = 44.0;

/// 提示上的一个按钮：点了执行 `run`，然后提示消失。
pub struct ToastAction {
    pub id: &'static str,
    pub label: &'static str,
    pub run: Rc<dyn Fn(&mut App)>,
}

/// 提示出现在哪：选区（本屏坐标）、所在屏（AppKit 全局坐标）、可见区域（本屏坐标）。
#[derive(Clone, Copy)]
pub struct Placement {
    pub rect: Rect,
    pub screen: Rect,
    pub visible: Rect,
}

impl From<&Shot> for Placement {
    fn from(shot: &Shot) -> Self {
        Self {
            rect: shot.rect,
            screen: shot.screen,
            visible: shot.visible,
        }
    }
}

struct Toast {
    text: SharedString,
}

struct ActionToast {
    text: SharedString,
    icon: &'static str,
    actions: Vec<ToastAction>,
}

impl Render for ActionToast {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let handle = window.window_handle();
        div()
            .size_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(14.))
            .pr(px(6.))
            .rounded(px(10.))
            .bg(black().opacity(0.82))
            .text_color(white())
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .child(
                gpui_kit::component::Icon::empty()
                    .path(SharedString::from(format!("icons/{}.svg", self.icon)))
                    .text_color(white())
                    .size(px(16.)),
            )
            .child(div().flex_1().truncate().child(self.text.clone()))
            .children(self.actions.iter().map(|action| {
                let run = action.run.clone();
                div()
                    .id(action.id)
                    .flex_shrink_0()
                    .px(px(10.))
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .bg(white().opacity(0.16))
                    .hover(|style| style.bg(white().opacity(0.28)))
                    .cursor(CursorStyle::PointingHand)
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation();
                        let run = run.clone();
                        // 关掉提示窗口要等这次事件处理完。
                        cx.defer(move |cx| {
                            run(cx);
                            let _ = handle.update(cx, |_, window, _| window.remove_window());
                        });
                    })
                    .child(action.label)
            }))
    }
}

impl Render for Toast {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .px(px(16.))
            .rounded(px(10.))
            .bg(black().opacity(0.78))
            .text_color(white())
            .text_size(px(14.))
            .font_weight(FontWeight::MEDIUM)
            .child(
                gpui_kit::component::Icon::empty()
                    .path("icons/check.svg")
                    .text_color(white())
                    .size(px(16.)),
            )
            .child(div().truncate().child(self.text.clone()))
    }
}

/// 提示条的宽度：按字数估算，限制在 160–480 pt。
fn width(text: &str) -> f64 {
    let units: f64 = text
        .chars()
        .map(|c| if c.is_ascii() { 7.5 } else { 14.0 })
        .sum();
    (units + 64.0).clamp(160.0, 480.0)
}

/// 提示条在屏幕上的位置（本屏坐标）：选区中央，整条留在可见区域内。
fn place(text: &str, shot: &Shot) -> Rect {
    centered(width(text), HEIGHT, &Placement::from(shot))
}

fn centered(w: f64, h: f64, at: &Placement) -> Rect {
    let center = (at.rect.x + at.rect.w / 2.0, at.rect.y + at.rect.h / 2.0);
    let v = at.visible;
    Rect::new(
        (center.0 - w / 2.0).clamp(v.x, (v.right() - w).max(v.x)),
        (center.1 - h / 2.0).clamp(v.y, (v.bottom() - h).max(v.y)),
        w,
        h,
    )
}

/// 带按钮的提示：文字估宽 + 每个按钮按字数估宽，最宽 640 pt。
fn actions_width(text: &str, actions: &[ToastAction]) -> f64 {
    let buttons: f64 = actions.iter().map(|action| width(action.label) - 64.0 + 28.0).sum();
    (width(text) + buttons).min(640.0)
}

/// 带按钮的提示（录屏存好后：复制文件、在访达中显示）。可以点，不抢焦点，几秒后自己消失。
pub fn show_actions(
    text: &str,
    icon: &'static str,
    at: Placement,
    actions: Vec<ToastAction>,
    cx: &mut App,
) {
    log(cx, format!("toast {text}"));
    let rect = centered(actions_width(text, &actions), ACTIONS_HEIGHT, &at);
    let frame = geometry::to_appkit(at.screen, rect);
    let offscreen = offscreen();
    let options = popup_options(frame);
    let text = SharedString::from(text.to_string());
    let handle = match crate::settings::open_transparent_window(options, cx, |_, cx| {
        cx.new(|_| ActionToast {
            text,
            icon,
            actions,
        })
    }) {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("helmp: 打开提示失败：{err:#}");
            return;
        }
    };
    let _ = handle.update(cx, |_, window, _| {
        // 钉图那一类：能点、不激活 Helmp、Helmp 隐藏时也留着。
        native::setup(window, native::Kind::Pin, frame, offscreen);
        native::present(window, false, offscreen);
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        cx.background_executor().timer(SHOW_ACTIONS_FOR).await;
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    })
    .detach();
}

fn popup_options(frame: Rect) -> WindowOptions {
    WindowOptions {
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
    }
}

pub fn show(text: &str, shot: &Shot, cx: &mut App) {
    log(cx, format!("toast {text}"));
    let frame = geometry::to_appkit(shot.screen, place(text, shot));
    let offscreen = offscreen();
    let options = popup_options(frame);
    let text = SharedString::from(text.to_string());
    let handle = match crate::settings::open_transparent_window(options, cx, |_, cx| {
        cx.new(|_| Toast { text })
    }) {
        Ok(handle) => handle,
        Err(err) => {
            // 提示开不出来不影响已经完成的复制 / 保存，只记下来。
            eprintln!("helmp: 打开提示失败：{err:#}");
            return;
        }
    };
    let _ = handle.update(cx, |_, window, _| {
        native::setup(window, native::Kind::Toast, frame, offscreen);
        native::present(window, false, offscreen);
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        cx.background_executor().timer(SHOW_FOR).await;
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    })
    .detach();
}

#[cfg(test)]
mod tests {
    // gpui_kit::* 里也有一个 `test` 属性宏，经 `super::*` 引入后会遮住标准库的。
    use core::prelude::v1::test;

    use super::*;
    use std::sync::Arc;

    #[test]
    fn toast_sits_in_the_selection_and_stays_visible() {
        // 提示出现在用户刚才看着的地方（选区中央）；选区贴边时提示整条仍在可见区域里。
        let shot = |rect| Shot {
            png: Arc::new(Vec::new()),
            pixels: (1, 1),
            rect,
            screen: Rect::new(0.0, 0.0, 1512.0, 982.0),
            visible: Rect::new(0.0, 37.0, 1512.0, 880.0),
        };
        let middle = place(
            "已复制到剪贴板",
            &shot(Rect::new(400.0, 300.0, 400.0, 200.0)),
        );
        assert_eq!(
            (middle.x + middle.w / 2.0, middle.y + middle.h / 2.0),
            (600.0, 400.0)
        );
        let corner = place("已复制到剪贴板", &shot(Rect::new(0.0, 0.0, 20.0, 20.0)));
        assert_eq!((corner.x, corner.y), (0.0, 37.0));
        assert!(width("已保存 Helmp 截图 2026-11-11 12.00.00.png") > width("已复制"));
        assert_eq!(width(&"长".repeat(100)), 480.0);
    }
}
