//! 选区的状态机（纯逻辑，单测覆盖）：悬停 → 拖拽框选 → 选好后可拖控制点调整、在里面拖动平移。
//!
//! 遮罩视图只把鼠标事件（显示器内的点坐标）交给它，按返回的 [`Effect`] 决定关窗、复制或识别。

use super::geometry::{self, Handle, Hit, Point, Rect};

/// 按下后移动不到这么多就松开，算单击（选中鼠标下的窗口），不算框选。
const CLICK_SLOP: f64 = 4.0;
/// 控制点的抓取半径（pt）：画出来的控制点是 8 pt 见方，抓取范围再宽一点。
pub const GRAB: f64 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum State {
    /// 还没选。`hover` 是鼠标下的窗口，单击就选中它。
    Idle { hover: Option<Rect> },
    /// 正在框选。
    Dragging {
        anchor: Point,
        current: Point,
        hover: Option<Rect>,
    },
    /// 选好了；`edit` 是正在进行的调整（拖控制点 / 平移）。
    Selected { rect: Rect, edit: Option<Edit> },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edit {
    pub kind: EditKind,
    pub start: Rect,
    pub from: Point,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Move,
    Resize(Handle),
}

/// 一次鼠标操作之后界面要做的事。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effect {
    None,
    /// 刚选好一个区域（松开鼠标）。识别文字模式此刻直接开始识别。
    Selected(Rect),
    /// 选区里双击：等同回车（复制并关闭）。
    Confirm,
    /// 没有选区时右键：关闭截图。
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Selector {
    pub state: State,
    /// 本屏范围（显示器内坐标，即 0,0,宽,高）。所有点先夹进这里。
    pub bounds: Rect,
}

impl Selector {
    pub fn new(bounds: Rect) -> Self {
        Self {
            state: State::Idle { hover: None },
            bounds,
        }
    }

    /// 当前要画出来的选区：选好的，或者正在拖出来的。
    pub fn selection(&self) -> Option<Rect> {
        match self.state {
            State::Selected { rect, .. } => Some(rect),
            State::Dragging {
                anchor, current, ..
            } => {
                let rect = geometry::normalize(anchor, current);
                (rect.w >= 1.0 && rect.h >= 1.0).then_some(rect)
            }
            State::Idle { .. } => None,
        }
    }

    /// 还没开始框选时鼠标下的窗口（悬停高亮）。
    pub fn hover(&self) -> Option<Rect> {
        match self.state {
            State::Idle { hover } => hover,
            State::Dragging {
                anchor,
                current,
                hover,
            } => {
                // 按下后还没拖开时仍高亮窗口：松开就选中它。
                let moved = (anchor.x - current.x)
                    .abs()
                    .max((anchor.y - current.y).abs());
                (moved < CLICK_SLOP).then_some(hover).flatten()
            }
            State::Selected { .. } => None,
        }
    }

    /// 鼠标移动（没按键）：更新悬停的窗口。
    pub fn hover_at(&mut self, p: Point, windows: &[Rect]) {
        if let State::Idle { hover } = &mut self.state {
            *hover = geometry::window_under(p, windows, self.bounds);
        }
    }

    /// 左键按下。`clicks` 是连击次数（双击为 2）。
    pub fn press(&mut self, p: Point, clicks: usize, windows: &[Rect]) -> Effect {
        let p = geometry::clamp_point(p, self.bounds);
        if let State::Selected { rect, edit } = &mut self.state {
            match geometry::hit_test(*rect, p, GRAB) {
                Hit::Inside if clicks >= 2 => return Effect::Confirm,
                Hit::Inside => {
                    *edit = Some(Edit {
                        kind: EditKind::Move,
                        start: *rect,
                        from: p,
                    });
                    return Effect::None;
                }
                Hit::Handle(handle) => {
                    *edit = Some(Edit {
                        kind: EditKind::Resize(handle),
                        start: *rect,
                        from: p,
                    });
                    return Effect::None;
                }
                // 选区外按下：丢掉旧选区，重新框选（和 Snipaste 一样）。
                Hit::Outside => {}
            }
        }
        self.state = State::Dragging {
            anchor: p,
            current: p,
            hover: geometry::window_under(p, windows, self.bounds),
        };
        Effect::None
    }

    /// 按着左键移动。
    pub fn drag(&mut self, p: Point) {
        let p = geometry::clamp_point(p, self.bounds);
        match &mut self.state {
            State::Dragging { current, .. } => *current = p,
            State::Selected {
                rect,
                edit: Some(edit),
            } => {
                let (dx, dy) = (p.x - edit.from.x, p.y - edit.from.y);
                *rect = match edit.kind {
                    EditKind::Move => geometry::translate(edit.start, dx, dy, self.bounds),
                    EditKind::Resize(handle) => {
                        geometry::resize(edit.start, handle, dx, dy, self.bounds)
                    }
                };
            }
            _ => {}
        }
    }

    /// 左键松开。
    pub fn release(&mut self, p: Point) -> Effect {
        let p = geometry::clamp_point(p, self.bounds);
        match self.state {
            State::Dragging { anchor, hover, .. } => {
                let moved = (anchor.x - p.x).abs().max((anchor.y - p.y).abs());
                let rect = if moved < CLICK_SLOP {
                    // 单击：选中鼠标下的窗口；什么窗口都没有就回到悬停状态。
                    hover
                } else {
                    Some(geometry::normalize(anchor, p))
                };
                match rect {
                    Some(rect) => {
                        self.state = State::Selected { rect, edit: None };
                        Effect::Selected(rect)
                    }
                    None => {
                        self.state = State::Idle { hover: None };
                        Effect::None
                    }
                }
            }
            State::Selected {
                rect,
                edit: Some(_),
            } => {
                self.state = State::Selected { rect, edit: None };
                Effect::None
            }
            _ => Effect::None,
        }
    }

    /// 没收到松开事件（在另一块屏上松开的）：按最后拖到的位置补一次松开。
    pub fn finish(&mut self) -> Effect {
        match self.state {
            State::Dragging { current, .. } => self.release(current),
            State::Selected { edit: Some(_), .. } => self.release(Point::default()),
            _ => Effect::None,
        }
    }

    /// 右键：有选区（或正在框选）时取消选区，没有时关闭截图。
    pub fn right_click(&mut self) -> Effect {
        match self.state {
            State::Idle { .. } => Effect::Close,
            _ => {
                self.state = State::Idle { hover: None };
                Effect::None
            }
        }
    }

    /// 鼠标在选区上的哪里（决定光标形状）。
    pub fn hit(&self, p: Point) -> Hit {
        match self.state {
            State::Selected { rect, .. } => geometry::hit_test(rect, p, GRAB),
            _ => Hit::Outside,
        }
    }

    /// 是否正在按着鼠标（框选或调整中）。另一块屏上收到「没按键的移动」时据此补一次松开。
    pub fn pressing(&self) -> bool {
        matches!(
            self.state,
            State::Dragging { .. } | State::Selected { edit: Some(_), .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect::new(0.0, 0.0, 1000.0, 800.0);

    fn select(from: Point, to: Point) -> (Selector, Effect) {
        let mut s = Selector::new(SCREEN);
        s.press(from, 1, &[]);
        s.drag(to);
        let effect = s.release(to);
        (s, effect)
    }

    #[test]
    fn dragging_selects_and_reports_once_released() {
        // 识别文字模式靠「松开时报告选区」立刻开始识别；拖的过程中不能提前报告，否则会识别半截选区。
        let mut s = Selector::new(SCREEN);
        assert_eq!(s.press(Point::new(300.0, 200.0), 1, &[]), Effect::None);
        s.drag(Point::new(100.0, 100.0));
        assert_eq!(s.selection(), Some(Rect::new(100.0, 100.0, 200.0, 100.0)));
        let effect = s.release(Point::new(100.0, 100.0));
        assert_eq!(
            effect,
            Effect::Selected(Rect::new(100.0, 100.0, 200.0, 100.0))
        );
        assert!(!s.pressing());
    }

    #[test]
    fn selection_stays_and_can_be_moved_or_resized() {
        // 松开后选区留在原地：拖控制点改大小、在里面拖动平移，都不会重新框选。
        let (mut s, _) = select(Point::new(100.0, 100.0), Point::new(300.0, 200.0));
        s.press(Point::new(300.0, 200.0), 1, &[]);
        s.drag(Point::new(350.0, 260.0));
        s.release(Point::new(350.0, 260.0));
        assert_eq!(s.selection(), Some(Rect::new(100.0, 100.0, 250.0, 160.0)));
        s.press(Point::new(200.0, 150.0), 1, &[]);
        s.drag(Point::new(210.0, 140.0));
        assert_eq!(s.release(Point::new(210.0, 140.0)), Effect::None);
        assert_eq!(s.selection(), Some(Rect::new(110.0, 90.0, 250.0, 160.0)));
    }

    #[test]
    fn clicking_selects_the_window_under_the_mouse() {
        // 不拖、直接单击：选中鼠标下的窗口；桌面上（没窗口）单击则什么也不选。
        let window = Rect::new(50.0, 60.0, 400.0, 300.0);
        let mut s = Selector::new(SCREEN);
        s.hover_at(Point::new(100.0, 100.0), &[window]);
        assert_eq!(s.hover(), Some(window));
        s.press(Point::new(100.0, 100.0), 1, &[window]);
        assert_eq!(s.hover(), Some(window), "按下还没拖开时仍然高亮窗口");
        assert_eq!(
            s.release(Point::new(101.0, 101.0)),
            Effect::Selected(window)
        );
        let mut empty = Selector::new(SCREEN);
        empty.press(Point::new(900.0, 700.0), 1, &[window]);
        assert_eq!(empty.release(Point::new(900.0, 700.0)), Effect::None);
        assert_eq!(empty.selection(), None);
    }

    #[test]
    fn double_click_inside_confirms() {
        // 选区里双击 = 回车（复制并关闭）；选区外双击只是重新框选，不能误复制。
        let (mut s, _) = select(Point::new(100.0, 100.0), Point::new(300.0, 200.0));
        assert_eq!(s.press(Point::new(200.0, 150.0), 2, &[]), Effect::Confirm);
        let (mut s, _) = select(Point::new(100.0, 100.0), Point::new(300.0, 200.0));
        assert_eq!(s.press(Point::new(600.0, 600.0), 2, &[]), Effect::None);
        assert!(matches!(s.state, State::Dragging { .. }));
    }

    #[test]
    fn right_click_cancels_selection_then_closes() {
        // 右键第一次只取消选区（框错了重来），没有选区时再右键才关闭截图。
        let (mut s, _) = select(Point::new(100.0, 100.0), Point::new(300.0, 200.0));
        assert_eq!(s.right_click(), Effect::None);
        assert_eq!(s.selection(), None);
        assert_eq!(s.right_click(), Effect::Close);
    }

    #[test]
    fn pressing_outside_starts_a_new_selection() {
        let (mut s, _) = select(Point::new(100.0, 100.0), Point::new(300.0, 200.0));
        s.press(Point::new(500.0, 500.0), 1, &[]);
        s.drag(Point::new(600.0, 650.0));
        s.release(Point::new(600.0, 650.0));
        assert_eq!(s.selection(), Some(Rect::new(500.0, 500.0, 100.0, 150.0)));
    }

    #[test]
    fn a_missed_release_still_finishes_the_drag() {
        // 拖到另一块屏上松开时，本屏的遮罩收不到松开事件；之后任何一块屏收到「没按键的移动」就补一次，
        // 否则选区会一直跟着鼠标走、工具栏永远不出来。
        let mut s = Selector::new(SCREEN);
        s.press(Point::new(100.0, 100.0), 1, &[]);
        s.drag(Point::new(400.0, 300.0));
        assert!(s.pressing());
        assert_eq!(
            s.finish(),
            Effect::Selected(Rect::new(100.0, 100.0, 300.0, 200.0))
        );
        assert!(!s.pressing());
        assert_eq!(s.finish(), Effect::None);
    }

    #[test]
    fn points_outside_the_screen_are_clamped() {
        // 拖到另一块屏上松开：选区贴着本屏边缘，不跨屏。
        let (s, effect) = select(Point::new(900.0, 700.0), Point::new(1400.0, 1200.0));
        assert_eq!(
            effect,
            Effect::Selected(Rect::new(900.0, 700.0, 100.0, 100.0))
        );
        assert_eq!(s.selection(), Some(Rect::new(900.0, 700.0, 100.0, 100.0)));
    }
}
