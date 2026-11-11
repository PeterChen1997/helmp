//! 截图界面的几何计算：全部是纯函数，单测覆盖。
//!
//! 坐标约定：没有特别说明时都是「显示器内的点坐标」——原点在这块屏幕的左上角，y 向下，
//! 单位是点（pt，不是像素），和 GPUI 窗口里的鼠标坐标一致（遮罩窗口铺满整块屏幕）。
//! AppKit 的全局坐标（原点在主屏左下角，y 向上）只在摆放原生窗口时用，见 [`to_appkit`]。

/// 点。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// 矩形：左上角 + 宽高（宽高不为负）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    /// 含左上边、不含右下边，和像素网格一致：相邻两个窗口的公共边只算左边那个的。
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    /// 两个矩形的交集；不相交时为 None。
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > x && bottom > y).then(|| Rect::new(x, y, right - x, bottom - y))
    }
}

/// 拖拽的起点和终点 → 选区。从任意方向拖（往左上、往右下……）得到的都是同一个正向矩形。
pub fn normalize(a: Point, b: Point) -> Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    Rect::new(x, y, (a.x - b.x).abs(), (a.y - b.y).abs())
}

/// 把点限制在 `bounds` 里（鼠标拖出屏幕边时选区贴边，而不是越界）。
pub fn clamp_point(p: Point, bounds: Rect) -> Point {
    Point::new(
        p.x.clamp(bounds.x, bounds.right()),
        p.y.clamp(bounds.y, bounds.bottom()),
    )
}

/// 选区边框上的八个控制点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle {
    pub const ALL: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    /// 控制点中心在选区上的位置。
    pub fn center(self, sel: Rect) -> Point {
        let (cx, cy) = (sel.x + sel.w / 2.0, sel.y + sel.h / 2.0);
        match self {
            Handle::TopLeft => Point::new(sel.x, sel.y),
            Handle::Top => Point::new(cx, sel.y),
            Handle::TopRight => Point::new(sel.right(), sel.y),
            Handle::Right => Point::new(sel.right(), cy),
            Handle::BottomRight => Point::new(sel.right(), sel.bottom()),
            Handle::Bottom => Point::new(cx, sel.bottom()),
            Handle::BottomLeft => Point::new(sel.x, sel.bottom()),
            Handle::Left => Point::new(sel.x, cy),
        }
    }

    fn moves_left(self) -> bool {
        matches!(self, Handle::TopLeft | Handle::Left | Handle::BottomLeft)
    }

    fn moves_right(self) -> bool {
        matches!(self, Handle::TopRight | Handle::Right | Handle::BottomRight)
    }

    fn moves_top(self) -> bool {
        matches!(self, Handle::TopLeft | Handle::Top | Handle::TopRight)
    }

    fn moves_bottom(self) -> bool {
        matches!(
            self,
            Handle::BottomLeft | Handle::Bottom | Handle::BottomRight
        )
    }
}

/// 鼠标按在选区的哪里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Handle(Handle),
    Inside,
    Outside,
}

/// 按下的位置落在哪：先看控制点（离角 / 边在 `grab` 以内），再看选区内部。
///
/// 角优先于边、边优先于内部：选区很小时四个角挤在一起，先认角才能把它拉大；
/// 边框外侧 `grab` 以内也算抓到边，用户不必精确点在 1 pt 的线上。
pub fn hit_test(sel: Rect, p: Point, grab: f64) -> Hit {
    let near = |a: f64, b: f64| (a - b).abs() <= grab;
    let within_x = p.x >= sel.x - grab && p.x <= sel.right() + grab;
    let within_y = p.y >= sel.y - grab && p.y <= sel.bottom() + grab;
    let (left, right) = (near(p.x, sel.x), near(p.x, sel.right()));
    let (top, bottom) = (near(p.y, sel.y), near(p.y, sel.bottom()));
    let handle = match (left, right, top, bottom) {
        (true, _, true, _) => Some(Handle::TopLeft),
        (_, true, true, _) => Some(Handle::TopRight),
        (true, _, _, true) => Some(Handle::BottomLeft),
        (_, true, _, true) => Some(Handle::BottomRight),
        (_, _, true, _) if within_x => Some(Handle::Top),
        (_, _, _, true) if within_x => Some(Handle::Bottom),
        (true, ..) if within_y => Some(Handle::Left),
        (_, true, ..) if within_y => Some(Handle::Right),
        _ => None,
    };
    match handle {
        Some(handle) => Hit::Handle(handle),
        None if sel.contains(p) => Hit::Inside,
        None => Hit::Outside,
    }
}

/// 拖控制点调整大小：被拖的边跟着鼠标走（限制在屏幕内），拖过对边时选区翻转而不是变成负宽度。
/// 宽高至少 1 pt，免得选区消失后再也抓不住。
pub fn resize(start: Rect, handle: Handle, dx: f64, dy: f64, bounds: Rect) -> Rect {
    let clamp_x = |x: f64| x.clamp(bounds.x, bounds.right());
    let clamp_y = |y: f64| y.clamp(bounds.y, bounds.bottom());
    let mut left = start.x;
    let mut right = start.right();
    let mut top = start.y;
    let mut bottom = start.bottom();
    if handle.moves_left() {
        left = clamp_x(left + dx);
    }
    if handle.moves_right() {
        right = clamp_x(right + dx);
    }
    if handle.moves_top() {
        top = clamp_y(top + dy);
    }
    if handle.moves_bottom() {
        bottom = clamp_y(bottom + dy);
    }
    let mut rect = normalize(Point::new(left, top), Point::new(right, bottom));
    if rect.w < 1.0 {
        rect.w = 1.0;
        rect.x = rect.x.min(bounds.right() - 1.0);
    }
    if rect.h < 1.0 {
        rect.h = 1.0;
        rect.y = rect.y.min(bounds.bottom() - 1.0);
    }
    rect
}

/// 在选区内拖动：整体平移，大小不变，碰到屏幕边就停（不会被挤小）。
pub fn translate(start: Rect, dx: f64, dy: f64, bounds: Rect) -> Rect {
    let x = (start.x + dx).clamp(bounds.x, (bounds.right() - start.w).max(bounds.x));
    let y = (start.y + dy).clamp(bounds.y, (bounds.bottom() - start.h).max(bounds.y));
    Rect::new(x, y, start.w, start.h)
}

/// 工具栏的左上角：默认在选区外的右下方（右边对齐选区右边）；下面放不下就翻到选区上方；
/// 上下都放不下（选区几乎占满整屏）就放进选区里面的右下角。水平方向始终留在屏幕内。
pub fn toolbar_origin(sel: Rect, size: (f64, f64), bounds: Rect, gap: f64) -> Point {
    let (w, h) = size;
    let x = (sel.right() - w).clamp(bounds.x, (bounds.right() - w).max(bounds.x));
    let below = sel.bottom() + gap;
    if below + h <= bounds.bottom() {
        return Point::new(x, below);
    }
    let above = sel.y - gap - h;
    if above >= bounds.y {
        return Point::new(x, above);
    }
    let inside = (sel.bottom() - gap - h).clamp(bounds.y, (bounds.bottom() - h).max(bounds.y));
    Point::new(x, inside)
}

/// 选区换算成截图里的像素矩形（Retina 屏 1 pt = 2 px）。各边四舍五入到最近的像素并限制在图内，
/// 这样相邻的两次截图不会多一列或少一列。宽或高为 0 时返回 None。
pub fn pixel_crop(sel: Rect, scale: f64, image: (u32, u32)) -> Option<PixelRect> {
    let edge = |v: f64, max: u32| (v * scale).round().clamp(0.0, f64::from(max)) as u32;
    let (x0, x1) = (edge(sel.x, image.0), edge(sel.right(), image.0));
    let (y0, y1) = (edge(sel.y, image.1), edge(sel.bottom(), image.1));
    (x1 > x0 && y1 > y0).then(|| PixelRect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

/// 截图里的像素矩形（原点左上）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// 识别结果窗口的左上角：优先放在选区右侧（顶边对齐），放不下依次试左侧、下方、上方；
/// 都放不下就压在选区上。最后整体限制在 `bounds`（屏幕可见区域）内，保证整个窗口看得见。
pub fn beside(sel: Rect, size: (f64, f64), bounds: Rect, gap: f64) -> Point {
    let (w, h) = size;
    let fits = |p: Point| {
        p.x >= bounds.x
            && p.y >= bounds.y
            && p.x + w <= bounds.right()
            && p.y + h <= bounds.bottom()
    };
    let candidates = [
        Point::new(sel.right() + gap, sel.y),
        Point::new(sel.x - gap - w, sel.y),
        Point::new(sel.x, sel.bottom() + gap),
        Point::new(sel.x, sel.y - gap - h),
    ];
    // 侧边放置时竖直方向可以挪：选区贴着屏幕底边时，窗口往上提一点仍然算「在右侧」。
    let slide = |p: Point| {
        Point::new(
            p.x,
            p.y.clamp(bounds.y, (bounds.bottom() - h).max(bounds.y)),
        )
    };
    let chosen = candidates
        .iter()
        .enumerate()
        .map(|(ix, p)| if ix < 2 { slide(*p) } else { *p })
        .find(|p| fits(*p))
        .unwrap_or(Point::new(sel.x, sel.y));
    Point::new(
        chosen.x.clamp(bounds.x, (bounds.right() - w).max(bounds.x)),
        chosen
            .y
            .clamp(bounds.y, (bounds.bottom() - h).max(bounds.y)),
    )
}

/// 显示器内的矩形 → AppKit 全局坐标的矩形（`display` 是这块屏的 NSScreen.frame，原点左下、y 向上）。
/// 钉图窗口就靠它落在选区原来的位置上。
pub fn to_appkit(display: Rect, local: Rect) -> Rect {
    Rect::new(
        display.x + local.x,
        display.y + display.h - local.y - local.h,
        local.w,
        local.h,
    )
}

/// AppKit 全局坐标的矩形 → 显示器内的矩形，[`to_appkit`] 的逆运算（如把 visibleFrame 换算成本屏坐标）。
pub fn from_appkit(display: Rect, global: Rect) -> Rect {
    Rect::new(
        global.x - display.x,
        display.y + display.h - global.y - global.h,
        global.w,
        global.h,
    )
}

/// CoreGraphics 全局坐标（原点在主屏左上、y 向下，`CGWindowListCopyWindowInfo` 和 `CGDisplayBounds` 用它）
/// 的矩形 → 某块屏内的矩形。`display` 是这块屏的 `CGDisplayBounds`。
pub fn cg_to_local(display: Rect, global: Rect) -> Rect {
    Rect::new(
        global.x - display.x,
        global.y - display.y,
        global.w,
        global.h,
    )
}

/// 鼠标下最上面的那个窗口（`windows` 从前到后排列），裁到屏幕内。悬停高亮、单击选中窗口用。
pub fn window_under(p: Point, windows: &[Rect], bounds: Rect) -> Option<Rect> {
    windows
        .iter()
        .find(|window| window.contains(p))
        .and_then(|window| window.intersect(&bounds))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect::new(0.0, 0.0, 1512.0, 982.0);

    #[test]
    fn drag_in_any_direction_gives_the_same_selection() {
        // 用户可能从右下往左上拖，也可能从左下往右上拖：选区必须一样，否则反向拖出来的是负宽度、什么也截不到。
        let expected = Rect::new(100.0, 50.0, 200.0, 80.0);
        let corners = [
            (Point::new(100.0, 50.0), Point::new(300.0, 130.0)),
            (Point::new(300.0, 130.0), Point::new(100.0, 50.0)),
            (Point::new(300.0, 50.0), Point::new(100.0, 130.0)),
            (Point::new(100.0, 130.0), Point::new(300.0, 50.0)),
        ];
        for (a, b) in corners {
            assert_eq!(normalize(a, b), expected, "{a:?} → {b:?}");
        }
        // 拖出屏幕边：鼠标坐标先被夹回屏内，选区贴边。
        let out = clamp_point(Point::new(-40.0, 2000.0), SCREEN);
        assert_eq!(out, Point::new(0.0, 982.0));
    }

    #[test]
    fn hit_test_prefers_corners_then_edges_then_inside() {
        let sel = Rect::new(100.0, 100.0, 200.0, 100.0);
        // 角：边框内外各 grab 以内都算。
        assert_eq!(
            hit_test(sel, Point::new(97.0, 103.0), 6.0),
            Hit::Handle(Handle::TopLeft)
        );
        assert_eq!(
            hit_test(sel, Point::new(305.0, 205.0), 6.0),
            Hit::Handle(Handle::BottomRight)
        );
        // 边：沿边任意位置都能抓。
        assert_eq!(
            hit_test(sel, Point::new(180.0, 98.0), 6.0),
            Hit::Handle(Handle::Top)
        );
        assert_eq!(
            hit_test(sel, Point::new(302.0, 150.0), 6.0),
            Hit::Handle(Handle::Right)
        );
        assert_eq!(
            hit_test(sel, Point::new(150.0, 201.0), 6.0),
            Hit::Handle(Handle::Bottom)
        );
        assert_eq!(
            hit_test(sel, Point::new(99.0, 150.0), 6.0),
            Hit::Handle(Handle::Left)
        );
        // 里面拖 = 移动；外面 = 不归选区管。
        assert_eq!(hit_test(sel, Point::new(200.0, 150.0), 6.0), Hit::Inside);
        assert_eq!(hit_test(sel, Point::new(50.0, 150.0), 6.0), Hit::Outside);
        // 边的延长线上、离选区很远的点不算抓到边。
        assert_eq!(hit_test(sel, Point::new(500.0, 100.0), 6.0), Hit::Outside);
        // 很小的选区：四个角挤在一起时先认角，才能把它拉大。
        let tiny = Rect::new(10.0, 10.0, 4.0, 4.0);
        assert_eq!(
            hit_test(tiny, Point::new(12.0, 12.0), 6.0),
            Hit::Handle(Handle::TopLeft)
        );
    }

    #[test]
    fn resize_follows_the_handle_and_stays_on_screen() {
        let sel = Rect::new(100.0, 100.0, 200.0, 100.0);
        // 右下角往右下拉：只动右边和下边。
        assert_eq!(
            resize(sel, Handle::BottomRight, 50.0, 20.0, SCREEN),
            Rect::new(100.0, 100.0, 250.0, 120.0)
        );
        // 上边往上拉：只动上边，左右不变。
        assert_eq!(
            resize(sel, Handle::Top, 30.0, -40.0, SCREEN),
            Rect::new(100.0, 60.0, 200.0, 140.0)
        );
        // 拉出屏幕：被拖的边停在屏幕边上，不会越界（越界的部分截不到）。
        assert_eq!(
            resize(sel, Handle::TopLeft, -500.0, -500.0, SCREEN),
            Rect::new(0.0, 0.0, 300.0, 200.0)
        );
        // 拖过对边：选区翻转成正向矩形，而不是负宽度。
        assert_eq!(
            resize(sel, Handle::Right, -260.0, 0.0, SCREEN),
            Rect::new(40.0, 100.0, 60.0, 100.0)
        );
        // 正好拖到和对边重合：至少留 1 pt，选区不消失。
        assert_eq!(resize(sel, Handle::Bottom, 0.0, -100.0, SCREEN).h, 1.0);
    }

    #[test]
    fn moving_keeps_size_and_stops_at_screen_edges() {
        // 平移碰到屏幕边时停住，选区不能被压小（压小就截不到用户框好的内容了）。
        let sel = Rect::new(100.0, 100.0, 200.0, 100.0);
        assert_eq!(
            translate(sel, 10.0, -20.0, SCREEN),
            Rect::new(110.0, 80.0, 200.0, 100.0)
        );
        assert_eq!(
            translate(sel, -999.0, 9999.0, SCREEN),
            Rect::new(0.0, 882.0, 200.0, 100.0)
        );
        assert_eq!(
            translate(sel, 9999.0, -999.0, SCREEN),
            Rect::new(1312.0, 0.0, 200.0, 100.0)
        );
    }

    #[test]
    fn toolbar_sits_below_right_and_flips_when_there_is_no_room() {
        let size = (240.0, 36.0);
        // 平常：选区外、右下方，右边与选区对齐。
        let sel = Rect::new(400.0, 200.0, 300.0, 200.0);
        assert_eq!(
            toolbar_origin(sel, size, SCREEN, 6.0),
            Point::new(460.0, 406.0)
        );
        // 选区贴着屏幕底：翻到选区上方。
        let low = Rect::new(400.0, 800.0, 300.0, 170.0);
        assert_eq!(
            toolbar_origin(low, size, SCREEN, 6.0),
            Point::new(460.0, 758.0)
        );
        // 选区几乎占满整屏：上下都没地方，放进选区里面的右下角，仍然完整可见。
        let full = Rect::new(0.0, 10.0, 1512.0, 965.0);
        let inside = toolbar_origin(full, size, SCREEN, 6.0);
        assert_eq!(inside, Point::new(1272.0, 933.0));
        assert!(inside.y + size.1 <= SCREEN.bottom());
        // 选区很窄、贴着左边：工具栏不能伸出屏幕左边。
        let narrow = Rect::new(0.0, 100.0, 40.0, 40.0);
        assert_eq!(toolbar_origin(narrow, size, SCREEN, 6.0).x, 0.0);
    }

    #[test]
    fn crop_converts_points_to_pixels_with_backing_scale() {
        // Retina 屏：1 pt = 2 px，截出来的图必须是选区里的全部像素（保持清晰），不能按点数缩小。
        let sel = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert_eq!(
            pixel_crop(sel, 2.0, (3024, 1964)),
            Some(PixelRect {
                x: 20,
                y: 40,
                w: 200,
                h: 100
            })
        );
        assert_eq!(
            pixel_crop(sel, 1.0, (1920, 1080)),
            Some(PixelRect {
                x: 10,
                y: 20,
                w: 100,
                h: 50
            })
        );
        // 非整数缩放（如 1.5 倍的外接屏）：各边就近取整，不越出图片。
        assert_eq!(
            pixel_crop(Rect::new(0.5, 0.0, 10.0, 10.0), 1.5, (100, 100)),
            Some(PixelRect {
                x: 1,
                y: 0,
                w: 15,
                h: 15
            })
        );
        let edge = pixel_crop(Rect::new(1500.0, 970.0, 50.0, 50.0), 2.0, (3024, 1964)).unwrap();
        assert_eq!((edge.x + edge.w, edge.y + edge.h), (3024, 1964));
        // 空选区不产出图片。
        assert_eq!(
            pixel_crop(Rect::new(5.0, 5.0, 0.1, 10.0), 2.0, (100, 100)),
            None
        );
    }

    #[test]
    fn result_window_stays_next_to_the_selection_and_on_screen() {
        let visible = Rect::new(0.0, 37.0, 1512.0, 880.0);
        let size = (380.0, 300.0);
        // 右边有地方：放右边，顶边对齐选区。
        let sel = Rect::new(100.0, 200.0, 300.0, 100.0);
        assert_eq!(beside(sel, size, visible, 12.0), Point::new(412.0, 200.0));
        // 选区贴着右边：放左边。
        let right = Rect::new(1200.0, 200.0, 300.0, 100.0);
        assert_eq!(beside(right, size, visible, 12.0), Point::new(808.0, 200.0));
        // 选区靠近底部：仍在右侧，但往上提，整个窗口留在可见区域内。
        let low = Rect::new(100.0, 850.0, 300.0, 60.0);
        let p = beside(low, size, visible, 12.0);
        assert_eq!(p.x, 412.0);
        assert_eq!(p.y + size.1, visible.bottom());
        // 选区占满整屏：左右上下都放不下，压在选区上，但不出屏幕、不盖菜单栏。
        let full = Rect::new(0.0, 0.0, 1512.0, 982.0);
        let p = beside(full, size, visible, 12.0);
        assert!(p.x >= visible.x && p.y >= visible.y);
        assert!(p.x + size.0 <= visible.right() && p.y + size.1 <= visible.bottom());
    }

    #[test]
    fn pin_lands_exactly_where_the_selection_was() {
        // 钉图要原地「浮起来」：AppKit 原点在左下，换算错一次就会上下颠倒地跳到别处。
        let main = Rect::new(0.0, 0.0, 1512.0, 982.0);
        let sel = Rect::new(100.0, 200.0, 300.0, 150.0);
        let frame = to_appkit(main, sel);
        assert_eq!(frame, Rect::new(100.0, 632.0, 300.0, 150.0));
        assert_eq!(from_appkit(main, frame), sel);
        // 副屏在主屏右侧、底边更低（原点为负）。
        let side = Rect::new(1512.0, -300.0, 2560.0, 1440.0);
        let frame = to_appkit(side, sel);
        assert_eq!(frame, Rect::new(1612.0, 790.0, 300.0, 150.0));
        assert_eq!(from_appkit(side, frame), sel);
    }

    #[test]
    fn hover_picks_the_frontmost_window_under_the_mouse() {
        // 窗口列表从前到后：重叠处取最前面的那个；伸出屏幕的部分裁掉（截不到屏幕外）。
        let display = Rect::new(1512.0, 0.0, 1920.0, 1080.0);
        let windows = [
            cg_to_local(display, Rect::new(1612.0, 100.0, 400.0, 300.0)),
            cg_to_local(display, Rect::new(1512.0, 0.0, 1920.0, 1080.0)),
            cg_to_local(display, Rect::new(3300.0, 500.0, 400.0, 300.0)),
        ];
        let bounds = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(
            window_under(Point::new(150.0, 150.0), &windows, bounds),
            Some(Rect::new(100.0, 100.0, 400.0, 300.0))
        );
        assert_eq!(
            window_under(Point::new(1000.0, 900.0), &windows, bounds),
            Some(bounds)
        );
        let clipped = window_under(Point::new(1850.0, 600.0), &windows[2..], bounds).unwrap();
        assert_eq!(clipped, Rect::new(1788.0, 500.0, 132.0, 300.0));
        assert_eq!(
            window_under(Point::new(5.0, 5.0), &windows[..1], bounds),
            None
        );
    }
}
