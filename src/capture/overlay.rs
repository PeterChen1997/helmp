//! 一块屏的冻结遮罩：冻结的画面由原生 NSImageView 垫在下面（见 `native::underlay_file`），
//! 这里画选区外的压暗、选区边框与控制点、尺寸标签、工具栏和操作提示，并把鼠标 / 按键交给选区状态机。

use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;

use super::annotation::{BLOCK_SIZES, Tool, WIDTHS};
use super::geometry::{self, Handle, Hit, Rect};
use super::selector::{Effect, Selector, State};
use super::toolbar::{self, Edit};
use super::{Act, Mode, Primary, Session, act, is_close_key};
use crate::i18n::tr;
use crate::style::{self, Palette};

/// 选区外的压暗程度。
const DIM: f32 = 0.5;
/// 控制点边长（pt）。
const HANDLE: f32 = 8.;
/// 工具栏与选区的间距（pt）。
const TOOLBAR_GAP: f64 = 6.;

pub struct Overlay {
    session: Entity<Session>,
    index: usize,
    focus: FocusHandle,
    /// 鼠标的最新位置（本屏坐标），决定光标形状。
    mouse: geometry::Point,
    hex: Entity<InputState>,
    _observe: Subscription,
    _hex_input: Subscription,
}

impl Overlay {
    pub fn new(
        session: Entity<Session>,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        // 别的屏上的操作（在那里开始框选会清掉本屏的选区）也要让本屏重画。
        let observe = cx.observe(&session, |_, _, cx| cx.notify());
        let hex = cx.new(|cx| InputState::new(window, cx).placeholder("#RRGGBB"));
        let hex_input = cx.subscribe_in(
            &hex,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let value = input.read(cx).value();
                    let hex = value.trim().trim_start_matches('#');
                    if hex.len() == 6
                        && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
                        && let Ok(color) = u32::from_str_radix(hex, 16)
                    {
                        this.edit(Edit::Color(color), cx);
                    }
                }
                InputEvent::PressEnter { .. } => this.focus.focus(window, cx),
                _ => {}
            },
        );
        Self {
            session,
            index,
            focus,
            mouse: geometry::Point::default(),
            hex,
            _observe: observe,
            _hex_input: hex_input,
        }
    }

    fn primary(&self, cx: &App) -> Act {
        match self.session.read(cx).mode {
            Mode::Capture(Primary::Copy) => Act::Copy,
            Mode::Capture(Primary::Save) => Act::Save,
            Mode::Ocr => Act::Recognize,
        }
    }

    /// 选区状态机报告的结果 → 截图操作（推迟到本次事件处理之后：操作会关掉这个窗口）。
    fn apply(&self, effect: Effect, cx: &mut Context<Self>) {
        let action = match effect {
            Effect::Selected(_) if self.session.read(cx).mode == Mode::Ocr => Act::Recognize,
            Effect::Confirm => self.primary(cx),
            Effect::Close => Act::Close,
            Effect::None | Effect::Selected(_) => return,
        };
        cx.defer(move |cx| act(action, cx));
    }

    fn edit(&self, edit: Edit, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| {
            if session.busy
                || session.mode == Mode::Ocr
                || !session.selected().is_some_and(|(ix, _)| ix == self.index)
            {
                return;
            }
            match edit {
                Edit::Select(tool) => session.select_tool(self.index, tool, cx),
                Edit::Undo => session.editor.undo(),
                Edit::Redo => session.editor.redo(),
                Edit::Color(color) => session.editor.color = Some(color),
                Edit::Width(width) => session.editor.width = width,
                Edit::BlockSize(size) => session.editor.block_size = size,
                Edit::StepSize(step) => match session.editor.tool {
                    Some(Tool::Mosaic) => {
                        let size = &mut session.editor.block_size;
                        *size = size.saturating_add_signed(step).min(BLOCK_SIZES.len() - 1);
                    }
                    Some(_) => {
                        let width = &mut session.editor.width;
                        *width = width.saturating_add_signed(step).min(WIDTHS.len() - 1);
                    }
                    None => {}
                },
            }
            cx.notify();
        });
    }

    fn toolbar_edit(&self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        self.edit(edit, cx);
        if matches!(edit, Edit::Select(_) | Edit::Color(_)) {
            let color = self.session.read(cx).editor.color();
            let value = format!("#{color:06X}");
            self.hex
                .update(cx, |input, cx| input.set_value(value, window, cx));
        }
    }

    fn on_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        let p = local(event.position);
        self.mouse = p;
        let index = self.index;
        let effect = self.session.update(cx, |session, cx| {
            if session.busy {
                return Effect::None;
            }
            if let Some((screen, selection)) = session.selected() {
                if screen == index
                    && let Some(tool) = session.editor.tool
                {
                    if selection.contains(p) {
                        session.editor.begin(tool, p);
                        cx.notify();
                    }
                    return Effect::None;
                }
                if screen != index || session.selectors[index].hit(p) == Hit::Outside {
                    session.editor.clear();
                }
            }
            // 选区只在一块屏上。
            for (ix, selector) in session.selectors.iter_mut().enumerate() {
                if ix != index {
                    *selector = Selector::new(selector.bounds);
                }
            }
            let effect = session.selectors[index].press(
                p,
                event.click_count,
                &session.screens[index].screen.windows,
            );
            cx.notify();
            effect
        });
        self.apply(effect, cx);
    }

    fn on_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let p = local(event.position);
        self.mouse = p;
        let index = self.index;
        let effect = self.session.update(cx, |session, cx| {
            if session.busy {
                return Effect::None;
            }
            let effect = if session.editor.drawing() {
                if event.pressed_button != Some(MouseButton::Left) {
                    // 跨屏松开时，结束留在原屏上的笔迹。
                    session.editor.finish();
                } else if let Some((ix, rect)) = session.selected()
                    && ix == index
                {
                    session.editor.drag(p, event.modifiers.shift, rect);
                }
                Effect::None
            } else if event.pressed_button == Some(MouseButton::Left) {
                session.selectors[index].drag(p);
                Effect::None
            } else if let Some(pressing) = session.selectors.iter_mut().find(|s| s.pressing()) {
                // 在另一块屏上松开了鼠标，按下的那块屏没收到松开：补上。
                pressing.finish()
            } else {
                if session.editor.tool.is_some()
                    && matches!(session.selectors[index].state, State::Selected { .. })
                {
                    return Effect::None;
                }
                session.selectors[index].hover_at(p, &session.screens[index].screen.windows);
                Effect::None
            };
            cx.notify();
            effect
        });
        self.apply(effect, cx);
    }

    fn on_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let p = local(event.position);
        let index = self.index;
        let effect = self.session.update(cx, |session, cx| {
            if session.busy {
                return Effect::None;
            }
            let effect = if session.editor.drawing() {
                if let Some((ix, rect)) = session.selected()
                    && ix == index
                {
                    session.editor.drag(p, event.modifiers.shift, rect);
                }
                session.editor.finish();
                Effect::None
            } else {
                session.selectors[index].release(p)
            };
            cx.notify();
            effect
        });
        self.apply(effect, cx);
    }

    fn on_right(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let effect = self.session.update(cx, |session, cx| {
            if session.busy {
                return Effect::None;
            }
            if session.editor.cancel() || session.editor.tool.take().is_some() {
                cx.notify();
                return Effect::None;
            }
            session.editor.clear();
            // 哪块屏上有选区就取消哪块的；都没有时右键关闭截图。
            let effect = match session
                .selectors
                .iter_mut()
                .find(|s| !matches!(s.state, State::Idle { .. }))
            {
                Some(selector) => selector.right_click(),
                None => Effect::Close,
            };
            cx.notify();
            effect
        });
        self.apply(effect, cx);
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // 在 HEX 颜色框里打字时按键归输入框，但 Esc / ⌘W 照样关掉截图界面。
        if self.hex.focus_handle(cx).is_focused(window) && !is_close_key(&event.keystroke) {
            return;
        }
        let keystroke = &event.keystroke;
        let command = keystroke.modifiers.platform;
        let edit = match keystroke.key.as_str() {
            "z" if command && keystroke.modifiers.shift => Some(Edit::Redo),
            "z" if command => Some(Edit::Undo),
            key if !keystroke.modifiers.modified() => toolbar::key_tool(key),
            _ => None,
        };
        if let Some(edit) = edit {
            cx.stop_propagation();
            self.toolbar_edit(edit, window, cx);
            return;
        }
        let action = match keystroke.key.as_str() {
            _ if is_close_key(keystroke) => Act::Close,
            "enter" => self.primary(cx),
            "c" if command => Act::Copy,
            "s" if command => Act::Save,
            _ => return,
        };
        cx.stop_propagation();
        cx.defer(move |cx| act(action, cx));
    }
}

fn local(position: Point<Pixels>) -> geometry::Point {
    geometry::Point::new(f64::from(position.x), f64::from(position.y))
}

fn at(rect: Rect) -> Div {
    div()
        .absolute()
        .left(px(rect.x as f32))
        .top(px(rect.y as f32))
        .w(px(rect.w as f32))
        .h(px(rect.h as f32))
}

fn cursor_for(hit: Hit, pressing: bool) -> CursorStyle {
    match hit {
        Hit::Handle(Handle::TopLeft | Handle::BottomRight) => CursorStyle::ResizeUpLeftDownRight,
        Hit::Handle(Handle::TopRight | Handle::BottomLeft) => CursorStyle::ResizeUpRightDownLeft,
        Hit::Handle(Handle::Top | Handle::Bottom) => CursorStyle::ResizeUpDown,
        Hit::Handle(Handle::Left | Handle::Right) => CursorStyle::ResizeLeftRight,
        Hit::Inside if pressing => CursorStyle::ClosedHand,
        Hit::Inside => CursorStyle::OpenHand,
        Hit::Outside => CursorStyle::Crosshair,
    }
}

/// 选区外的压暗：没有选区时整屏压暗，有选区（或悬停的窗口）时在它周围拼四块。
fn dimming(hole: Option<Rect>, bounds: Rect) -> Vec<Div> {
    let shade = |rect: Rect| at(rect).bg(black().opacity(DIM));
    let Some(hole) = hole else {
        return vec![shade(bounds)];
    };
    vec![
        shade(Rect::new(0.0, 0.0, bounds.w, hole.y)),
        shade(Rect::new(
            0.0,
            hole.bottom(),
            bounds.w,
            bounds.h - hole.bottom(),
        )),
        shade(Rect::new(0.0, hole.y, hole.x, hole.h)),
        shade(Rect::new(
            hole.right(),
            hole.y,
            bounds.w - hole.right(),
            hole.h,
        )),
    ]
}

/// 选区左上角上方的尺寸标签（像素，和存出来的图片一致）；上面放不下就放进选区里。
fn size_label(sel: Rect, scale: f64, pixels: (u32, u32)) -> Option<Div> {
    let crop = geometry::pixel_crop(sel, scale, pixels)?;
    let top = if sel.y >= 26.0 {
        sel.y - 24.0
    } else {
        sel.y + 4.0
    };
    Some(
        div()
            .absolute()
            .left(px((sel.x + if sel.y >= 26.0 { 0.0 } else { 4.0 }) as f32))
            .top(px(top as f32))
            .px(px(6.))
            .h(px(20.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .bg(black().opacity(0.72))
            .text_color(white())
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM)
            .child(format!("{} × {} px", crop.w, crop.h)),
    )
}

fn handles(sel: Rect, pal: &Palette) -> Vec<Div> {
    Handle::ALL
        .iter()
        .map(|handle| {
            let c = handle.center(sel);
            let half = f64::from(HANDLE) / 2.0;
            at(Rect::new(
                c.x - half,
                c.y - half,
                f64::from(HANDLE),
                f64::from(HANDLE),
            ))
            .rounded(px(2.))
            .bg(white())
            .border_1()
            .border_color(pal.accent)
        })
        .collect()
}

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = style::palette(window);
        let session = self.session.read(cx);
        let frozen = &session.screens[self.index];
        let selector = &session.selectors[self.index];
        let bounds = frozen.bounds();
        let selection = selector.selection();
        let hover = selector.hover();
        let selected = matches!(selector.state, State::Selected { .. });
        let pressing = selector.pressing();
        let hole = selection.or(hover);
        let annotating = selected && session.editor.tool.is_some();
        let cursor = if annotating {
            CursorStyle::Crosshair
        } else {
            cursor_for(selector.hit(self.mouse), pressing)
        };
        let ocr = session.mode == Mode::Ocr;
        let busy = session.busy;
        let primary = self.primary(cx);

        let mut root = div()
            .id("capture-overlay")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .cursor(cursor)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right))
            .on_mouse_move(cx.listener(Self::on_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_up))
            // 拖出窗口（到另一块屏）后松开：事件仍送给按下的这个窗口，但落在根元素外面。
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_up))
            .children(dimming(hole, bounds));

        if let Some(sel) = selection.filter(|_| selected && !ocr) {
            let marks = session.editor.preview();
            if !marks.is_empty() {
                root = root.child(super::preview::layer(
                    sel,
                    marks,
                    session.mosaic(self.index).cloned(),
                ));
            }
        }
        if let Some(rect) = hole {
            // 悬停的窗口用粗一点的框提示「单击选中它」；选区用细框加控制点。
            let border = at(rect).border_color(pal.accent);
            root = root.child(if selection.is_some() {
                border.border_1()
            } else {
                border.border_2()
            });
        }
        if let Some(sel) = selection {
            root = root.children(size_label(sel, frozen.scale, frozen.pixels));
            if selected && !pressing && !ocr && !annotating {
                root = root.children(handles(sel, &pal));
            }
            if selected && !pressing && !ocr && !busy {
                let origin = geometry::toolbar_origin(sel, toolbar::size(), bounds, TOOLBAR_GAP);
                let entity = cx.entity();
                root = root.child(
                    div()
                        .absolute()
                        .left(px(origin.x as f32))
                        .top(px(origin.y as f32))
                        .child(toolbar::editor_toolbar(
                            &session.editor,
                            primary,
                            &self.hex,
                            toolbar::options_offset(origin.y, bounds.bottom()),
                            &pal,
                            Rc::new(move |edit, window, cx| {
                                entity.update(cx, |overlay, cx| {
                                    overlay.toolbar_edit(edit, window, cx)
                                })
                            }),
                            Rc::new(|action, _, cx| cx.defer(move |cx| act(action, cx))),
                        )),
                );
            }
        } else if !pressing {
            let hint = if ocr {
                tr(
                    "框选要识别的文字 · 单击选中窗口 · Esc 退出",
                    "Select the text to recognize · Click a window to pick it · Esc to quit",
                )
            } else {
                tr(
                    "拖动框选 · 单击选中窗口 · Esc 退出",
                    "Drag to select · Click a window to pick it · Esc to quit",
                )
            };
            root = root.child(
                div()
                    .absolute()
                    .top(px(48.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .px(px(14.))
                            .py(px(7.))
                            .rounded(px(16.))
                            .bg(black().opacity(0.66))
                            .text_color(white())
                            .text_size(px(13.))
                            .child(hint),
                    ),
            );
        }
        root
    }
}
