//! 标注保存在屏幕点坐标里。调整截图边界时，笔迹仍贴在原来的内容上。

use std::sync::Arc;

use super::geometry::{self, Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Rectangle,
    Ellipse,
    Line,
    Arrow,
    Pen,
    Highlighter,
    Mosaic,
}

pub const COLORS: [(u32, &str, &str); 8] = [
    (0xff453a, "红色", "Red"),
    (0xff9f0a, "橙色", "Orange"),
    (0xffd60a, "黄色", "Yellow"),
    (0x30d158, "绿色", "Green"),
    (0x0a84ff, "蓝色", "Blue"),
    (0xbf5af2, "紫色", "Purple"),
    (0xffffff, "白色", "White"),
    (0x1c1c1e, "黑色", "Black"),
];

pub const WIDTHS: [f64; 4] = [2., 4., 7., 12.];
pub const BLOCK_SIZES: [f64; 4] = [8., 14., 22., 36.];

#[derive(Clone)]
pub struct Mark {
    tool: Tool,
    color: u32,
    size: f64,
    start: Point,
    end: Point,
    points: Vec<Point>,
}

impl Mark {
    fn freehand(&self) -> bool {
        matches!(self.tool, Tool::Pen | Tool::Highlighter)
    }

    fn update(&mut self, point: Point, constrained: bool, bounds: Rect) {
        if self.freehand() {
            if self.points.last().copied() != Some(point) {
                self.points.push(point);
            }
            self.end = point;
        } else if constrained {
            let dx = point.x - self.start.x;
            let dy = point.y - self.start.y;
            let (dx, dy) = match self.tool {
                Tool::Rectangle | Tool::Ellipse | Tool::Mosaic => {
                    let available = |v: f64, from: f64, low: f64, high: f64| {
                        if v < 0. { from - low } else { high - from }
                    };
                    let side = dx
                        .abs()
                        .max(dy.abs())
                        .min(available(dx, self.start.x, bounds.x, bounds.right()))
                        .min(available(dy, self.start.y, bounds.y, bounds.bottom()));
                    (side.copysign(dx), side.copysign(dy))
                }
                _ => {
                    let step = std::f64::consts::FRAC_PI_4;
                    let angle = (dy.atan2(dx) / step).round() * step;
                    let length = dx.hypot(dy);
                    (length * angle.cos(), length * angle.sin())
                }
            };
            self.end = Point::new(self.start.x + dx, self.start.y + dy);
        } else {
            self.end = point;
        }
    }

    fn visible(&self) -> bool {
        match self.tool {
            Tool::Pen | Tool::Highlighter => true,
            Tool::Line | Tool::Arrow => self.start != self.end,
            _ => {
                let rect = geometry::normalize(self.start, self.end);
                rect.w >= 1. && rect.h >= 1.
            }
        }
    }

    pub fn drawing(&self) -> Option<Drawing<'_>> {
        if !self.visible() {
            return None;
        }
        let rect = geometry::normalize(self.start, self.end);
        let shape = match self.tool {
            Tool::Rectangle => Shape::Rectangle(rect),
            Tool::Ellipse => Shape::Ellipse(rect),
            Tool::Line => Shape::Line([self.start, self.end]),
            Tool::Arrow => {
                let dx = self.end.x - self.start.x;
                let dy = self.end.y - self.start.y;
                let length = dx.hypot(dy);
                let head = (self.size * 3. + 7.).min(length * 0.7);
                let x = self.end.x - dx / length * head;
                let y = self.end.y - dy / length * head;
                let (ox, oy) = (dy / length * head * 0.45, -dx / length * head * 0.45);
                Shape::Arrow {
                    shaft: [self.start, Point::new(x, y)],
                    head: [
                        self.end,
                        Point::new(x + ox, y + oy),
                        Point::new(x - ox, y - oy),
                    ],
                }
            }
            Tool::Pen | Tool::Highlighter => Shape::Freehand(&self.points),
            Tool::Mosaic => return Some(Drawing::Mosaic(rect, self.size)),
        };
        let (width, opacity) = if self.tool == Tool::Highlighter {
            (self.size * 4., 0.36)
        } else {
            (self.size, 1.)
        };
        Some(Drawing::Ink(shape, self.color, width, opacity))
    }
}

pub enum Shape<'a> {
    Rectangle(Rect),
    Ellipse(Rect),
    Line([Point; 2]),
    Arrow { shaft: [Point; 2], head: [Point; 3] },
    Freehand(&'a [Point]),
}

pub enum Drawing<'a> {
    Ink(Shape<'a>, u32, f64, f64),
    Mosaic(Rect, f64),
}

#[derive(Default)]
pub struct Editor {
    pub tool: Option<Tool>,
    pub color: Option<u32>,
    pub width: usize,
    pub block_size: usize,
    done: Vec<Arc<Mark>>,
    undone: Vec<Arc<Mark>>,
    draft: Option<Mark>,
}

impl Editor {
    pub fn clear(&mut self) {
        self.tool = None;
        self.done.clear();
        self.undone.clear();
        self.draft = None;
    }

    pub fn color(&self) -> u32 {
        self.color
            .unwrap_or(if self.tool == Some(Tool::Highlighter) {
                COLORS[2].0
            } else {
                COLORS[0].0
            })
    }

    pub fn begin(&mut self, tool: Tool, point: Point) {
        self.draft = Some(Mark {
            tool,
            color: self.color(),
            size: if tool == Tool::Mosaic {
                BLOCK_SIZES[self.block_size]
            } else {
                WIDTHS[self.width]
            },
            start: point,
            end: point,
            points: vec![point],
        });
    }

    pub fn drag(&mut self, point: Point, constrained: bool, bounds: Rect) {
        if let Some(mark) = &mut self.draft {
            mark.update(geometry::clamp_point(point, bounds), constrained, bounds);
        }
    }

    pub fn drawing(&self) -> bool {
        self.draft.is_some()
    }

    pub fn finish(&mut self) {
        if let Some(mark) = self.draft.take().filter(Mark::visible) {
            self.done.push(Arc::new(mark));
            self.undone.clear();
        }
    }

    pub fn cancel(&mut self) -> bool {
        self.draft.take().is_some()
    }

    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    pub fn undo(&mut self) {
        if !self.cancel()
            && let Some(mark) = self.done.pop()
        {
            self.undone.push(mark);
        }
    }

    pub fn redo(&mut self) {
        if !self.drawing()
            && let Some(mark) = self.undone.pop()
        {
            self.done.push(mark);
        }
    }

    pub fn marks(&self) -> Vec<Arc<Mark>> {
        self.done.clone()
    }

    pub fn preview(&self) -> Vec<Arc<Mark>> {
        let mut marks = self.marks();
        marks.extend(self.draft.clone().map(Arc::new));
        marks
    }
}
