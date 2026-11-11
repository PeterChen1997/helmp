//! 「已复制」「已保存」提示：截图界面关掉后用户要知道刚才那一下成功了。
//! 不接收鼠标、不抢焦点，出现在选区中央，一会儿自己消失。

use std::time::Duration;

use gpui_kit::*;

use super::geometry::{self, Rect};
use super::{Shot, log, native, offscreen};

const SHOW_FOR: Duration = Duration::from_millis(1400);
const HEIGHT: f64 = 40.0;

struct Toast {
    text: SharedString,
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
    let w = width(text);
    let center = (
        shot.rect.x + shot.rect.w / 2.0,
        shot.rect.y + shot.rect.h / 2.0,
    );
    let v = shot.visible;
    Rect::new(
        (center.0 - w / 2.0).clamp(v.x, (v.right() - w).max(v.x)),
        (center.1 - HEIGHT / 2.0).clamp(v.y, (v.bottom() - HEIGHT).max(v.y)),
        w,
        HEIGHT,
    )
}

pub fn show(text: &str, shot: &Shot, cx: &mut App) {
    log(cx, format!("toast {text}"));
    let frame = geometry::to_appkit(shot.screen, place(text, shot));
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
