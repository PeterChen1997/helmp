//! 钉图：选区原地变成一张置顶的无边框小窗（位置、大小和选区完全一样），可以钉很多张。
//!
//! 拖动窗口任意处移动它；鼠标移上去时右下角出现工具栏（复制、保存、识别文字、关闭）；
//! Esc 或双击关闭；获得焦点后 ⌘C 复制、⌘S 保存。点它不会激活 Helmp，不打断用户在别的 App 里的输入。

use std::rc::Rc;

use gpui_kit::*;

use super::geometry::{self, Rect};
use super::overlay::button_row;
use super::{Act, Shot, log, native, offscreen, state};
use crate::i18n::tr;
use crate::style;

pub struct Pin {
    shot: Shot,
    focus: FocusHandle,
    hovered: bool,
}

/// 钉图的工具栏按钮（遮罩工具栏去掉「钉图」）。
const BUTTONS: [(Act, &str, &str, &str, &str); 4] = [
    (Act::Copy, "copy", "复制", "Copy", "⌘C"),
    (Act::Save, "download", "保存", "Save", "⌘S"),
    (
        Act::Recognize,
        "scan-text",
        "识别文字",
        "Recognize Text",
        "",
    ),
    (Act::Close, "x", "关闭", "Close", "Esc"),
];

/// 钉图窗口的位置：就是选区原来的位置（AppKit 全局坐标）。
pub fn frame(shot: &Shot) -> Rect {
    geometry::to_appkit(shot.screen, shot.rect)
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
    let handle = match crate::settings::open_transparent_window(options, cx, |window, cx| {
        cx.new(|cx| Pin::new(shot, window, cx))
    }) {
        Ok(handle) => handle,
        Err(err) => {
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
    state(cx).pins.push(handle);
    log(cx, format!("pinned {}x{}", frame.w, frame.h));
}

impl Pin {
    fn new(shot: Shot, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            shot,
            focus,
            hovered: false,
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

    fn run(&mut self, action: Act, window: &mut Window, cx: &mut Context<Self>) {
        let shot = self.current(window);
        let handle = window.window_handle();
        cx.defer(move |cx| match action {
            Act::Copy => super::copy(&shot, cx),
            Act::Save => super::save(shot, false, cx),
            Act::Recognize => super::result::open(shot, cx),
            Act::Close | Act::Pin => close(handle, cx),
        });
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let command = keystroke.modifiers.platform;
        let action = match keystroke.key.as_str() {
            "escape" => Act::Close,
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
    state(cx).pins.retain(|pin| *pin != handle);
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
            .border_1()
            .border_color(if self.hovered {
                pal.accent
            } else {
                pal.separator
            })
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = *hovered;
                cx.notify();
            }))
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down));
        if self.hovered {
            root = root.child(div().absolute().right(px(6.)).bottom(px(6.)).child(toolbar));
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
}
