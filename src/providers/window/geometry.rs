//! 窗口管理的几何计算（纯函数）：目标位置只由「屏幕可见区域 + 命令 + 当前窗口」决定。
//!
//! 坐标一律用辅助功能（AX / CoreGraphics）的全局坐标：原点在主屏（带菜单栏的屏）左上角，y 向下，
//! 单位是点。`NSScreen` 给的是 AppKit 坐标（原点在主屏左下，y 向上），用 [`cocoa_to_ax`] 转换。
//! 结果取整到整点：两个半屏正好拼满可见区域，不留 1 px 缝也不重叠。

#[derive(Debug, Clone, Copy, PartialEq)]
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

    fn center(self) -> (f64, f64) {
        (self.x + self.w / 2., self.y + self.h / 2.)
    }

    fn contains_point(self, (px, py): (f64, f64)) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    fn overlap(self, other: Rect) -> f64 {
        let w = (self.x + self.w).min(other.x + other.w) - self.x.max(other.x);
        let h = (self.y + self.h).min(other.y + other.h) - self.y.max(other.y);
        w.max(0.) * h.max(0.)
    }

    fn rounded(self) -> Rect {
        Rect::new(self.x.round(), self.y.round(), self.w.round(), self.h.round())
    }

    /// 两个矩形在取整误差内相同（窗口有最小尺寸、App 自己会微调几个点）。
    pub fn close_to(self, other: Rect) -> bool {
        [
            (self.x, other.x),
            (self.y, other.y),
            (self.w, other.w),
            (self.h, other.h),
        ]
        .iter()
        .all(|(a, b)| (a - b).abs() <= 2.)
    }
}

impl std::fmt::Display for Rect {
    /// dry-run 输出：`x,y w×h`（整数）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{},{} {}x{}",
            self.x.round(),
            self.y.round(),
            self.w.round(),
            self.h.round()
        )
    }
}

/// 一块屏幕：整块区域与去掉菜单栏、Dock 后的可见区域。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    pub frame: Rect,
    pub visible: Rect,
}

/// 按可见区域摆放的命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    FirstThird,
    CenterThird,
    LastThird,
    FirstTwoThirds,
    LastTwoThirds,
    Maximize,
    /// 可见区域的 90%，居中。
    AlmostMaximize,
    /// 保持大小（超出可见区域时缩小），居中。
    Center,
}

/// `start` 开始、长 `len` 的一段等分成 `parts` 份，取第 `from..to` 份。边界取整，相邻两段首尾相接。
fn slice(start: f64, len: f64, parts: u32, from: u32, to: u32) -> (f64, f64) {
    let edge = |k: u32| (start + len * f64::from(k) / f64::from(parts)).round();
    let a = edge(from);
    (a, edge(to) - a)
}

/// 命令在这块可见区域里的目标位置。
pub fn target_frame(layout: Layout, visible: Rect, current: Rect) -> Rect {
    let v = visible.rounded();
    let cols = |parts, from, to| slice(v.x, v.w, parts, from, to);
    let rows = |parts, from, to| slice(v.y, v.h, parts, from, to);
    let with = |(x, w): (f64, f64), (y, h): (f64, f64)| Rect::new(x, y, w, h);
    let full_w = (v.x, v.w);
    let full_h = (v.y, v.h);
    match layout {
        Layout::LeftHalf => with(cols(2, 0, 1), full_h),
        Layout::RightHalf => with(cols(2, 1, 2), full_h),
        Layout::TopHalf => with(full_w, rows(2, 0, 1)),
        Layout::BottomHalf => with(full_w, rows(2, 1, 2)),
        Layout::TopLeft => with(cols(2, 0, 1), rows(2, 0, 1)),
        Layout::TopRight => with(cols(2, 1, 2), rows(2, 0, 1)),
        Layout::BottomLeft => with(cols(2, 0, 1), rows(2, 1, 2)),
        Layout::BottomRight => with(cols(2, 1, 2), rows(2, 1, 2)),
        Layout::FirstThird => with(cols(3, 0, 1), full_h),
        Layout::CenterThird => with(cols(3, 1, 2), full_h),
        Layout::LastThird => with(cols(3, 2, 3), full_h),
        Layout::FirstTwoThirds => with(cols(3, 0, 2), full_h),
        Layout::LastTwoThirds => with(cols(3, 1, 3), full_h),
        Layout::Maximize => v,
        Layout::AlmostMaximize => centered(v, (v.w * 0.9).round(), (v.h * 0.9).round()),
        Layout::Center => centered(v, current.w.round().min(v.w), current.h.round().min(v.h)),
    }
}

fn centered(v: Rect, w: f64, h: f64) -> Rect {
    Rect::new(
        (v.x + (v.w - w) / 2.).round(),
        (v.y + (v.h - h) / 2.).round(),
        w,
        h,
    )
}

/// 窗口在哪块屏上：先看窗口中心落在哪块屏里，都不在（窗口拖到屏幕外）时取重叠面积最大的，
/// 完全不重叠时算主屏。
pub fn screen_index(window: Rect, screens: &[Screen]) -> Option<usize> {
    if screens.is_empty() {
        return None;
    }
    let center = window.center();
    if let Some(index) = screens
        .iter()
        .position(|screen| screen.frame.contains_point(center))
    {
        return Some(index);
    }
    let (index, area) = screens
        .iter()
        .enumerate()
        .map(|(index, screen)| (index, screen.frame.overlap(window)))
        .fold((0, 0.), |best, next| if next.1 > best.1 { next } else { best });
    Some(if area > 0. { index } else { 0 })
}

/// 把窗口从一块屏的可见区域搬到另一块：按比例保持相对位置和大小（左半屏到了新屏还是左半屏），
/// 再收进新屏的可见区域里。
pub fn move_to_screen(current: Rect, from: Rect, to: Rect) -> Rect {
    let sx = to.w / from.w.max(1.);
    let sy = to.h / from.h.max(1.);
    let w = (current.w * sx).round().min(to.w.round());
    let h = (current.h * sy).round().min(to.h.round());
    let x = (to.x + (current.x - from.x) * sx).round();
    let y = (to.y + (current.y - from.y) * sy).round();
    let x = x.clamp(to.x.round(), (to.x + to.w).round() - w);
    let y = y.clamp(to.y.round(), (to.y + to.h).round() - h);
    Rect::new(x, y, w, h)
}

/// AppKit 坐标（原点在主屏左下，y 向上）→ AX 坐标（原点在主屏左上，y 向下）。
/// `primary_height` 是主屏（`NSScreen.screens[0]`）的高度。
pub fn cocoa_to_ax(rect: Rect, primary_height: f64) -> Rect {
    Rect::new(rect.x, primary_height - (rect.y + rect.h), rect.w, rect.h)
}

/// 「恢复」记下的位置：(第一次被 Helmp 摆放之前的位置, Helmp 最后摆放到的位置)。
///
/// 连续摆放（左半屏 → 最大化 → 居中）时，恢复应回到最初的位置，而不是上一步：所以只有当窗口
/// 已经不在 Helmp 摆的地方（用户手动拖过）时，才把当前位置记成新的「原位」。
pub fn remember(previous: Option<(Rect, Rect)>, current: Rect, target: Rect) -> (Rect, Rect) {
    match previous {
        Some((before, applied)) if applied.close_to(current) => (before, target),
        _ => (current, target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 13 寸 MacBook：菜单栏 25，Dock 在底部 75。
    const LAPTOP: Screen = Screen {
        frame: Rect::new(0., 0., 1440., 900.),
        visible: Rect::new(0., 25., 1440., 800.),
    };
    /// 主屏右边的外接 4K（逻辑 2560×1440），顶边比主屏高 300，Dock 不在它上面，菜单栏 25。
    const EXTERNAL: Screen = Screen {
        frame: Rect::new(1440., -300., 2560., 1440.),
        visible: Rect::new(1440., -275., 2560., 1415.),
    };
    const WINDOW: Rect = Rect::new(100., 100., 800., 600.);

    #[test]
    fn halves_and_quarters_tile_the_visible_area() {
        // 半屏 / 四分之一必须躲开菜单栏和 Dock，并且拼起来正好铺满：差 1 px 就会露缝或叠边。
        let v = LAPTOP.visible;
        assert_eq!(target_frame(Layout::LeftHalf, v, WINDOW), Rect::new(0., 25., 720., 800.));
        assert_eq!(target_frame(Layout::RightHalf, v, WINDOW), Rect::new(720., 25., 720., 800.));
        assert_eq!(target_frame(Layout::TopHalf, v, WINDOW), Rect::new(0., 25., 1440., 400.));
        assert_eq!(target_frame(Layout::BottomHalf, v, WINDOW), Rect::new(0., 425., 1440., 400.));
        assert_eq!(target_frame(Layout::TopLeft, v, WINDOW), Rect::new(0., 25., 720., 400.));
        assert_eq!(target_frame(Layout::BottomRight, v, WINDOW), Rect::new(720., 425., 720., 400.));
        assert_eq!(target_frame(Layout::Maximize, v, WINDOW), v);
    }

    #[test]
    fn odd_sizes_split_without_gaps() {
        // 奇数宽度（1441、Dock 在左侧时的可见区域）：左右两半的边界要对上，总宽不变。
        let v = Rect::new(65., 25., 1441., 875.);
        let left = target_frame(Layout::LeftHalf, v, WINDOW);
        let right = target_frame(Layout::RightHalf, v, WINDOW);
        assert_eq!(left.x, 65.);
        assert_eq!(left.x + left.w, right.x);
        assert_eq!(left.w + right.w, 1441.);
        let thirds = [Layout::FirstThird, Layout::CenterThird, Layout::LastThird]
            .map(|l| target_frame(l, v, WINDOW));
        assert_eq!(thirds[0].x + thirds[0].w, thirds[1].x);
        assert_eq!(thirds[1].x + thirds[1].w, thirds[2].x);
        assert_eq!(thirds[2].x + thirds[2].w, 65. + 1441.);
        let two = target_frame(Layout::FirstTwoThirds, v, WINDOW);
        assert_eq!(two.x + two.w, thirds[1].x + thirds[1].w);
        let top = target_frame(Layout::TopHalf, v, WINDOW);
        let bottom = target_frame(Layout::BottomHalf, v, WINDOW);
        assert_eq!(top.h + bottom.h, 875.);
    }

    #[test]
    fn center_keeps_size_and_shrinks_oversized_windows() {
        // 居中不应改变窗口大小；比可见区域还大的窗口要缩进来，否则标题栏会跑到菜单栏下面。
        let v = LAPTOP.visible;
        assert_eq!(target_frame(Layout::Center, v, WINDOW), Rect::new(320., 125., 800., 600.));
        let huge = Rect::new(0., 0., 3000., 2000.);
        assert_eq!(target_frame(Layout::Center, v, huge), v);
        assert_eq!(
            target_frame(Layout::AlmostMaximize, v, WINDOW),
            Rect::new(72., 65., 1296., 720.)
        );
    }

    #[test]
    fn layouts_on_a_secondary_display_with_negative_coordinates() {
        // 外接屏在主屏上方时 y 是负数：计算要以那块屏的可见区域为准，不能按主屏算。
        let v = EXTERNAL.visible;
        assert_eq!(
            target_frame(Layout::RightHalf, v, WINDOW),
            Rect::new(2720., -275., 1280., 1415.)
        );
        assert_eq!(
            target_frame(Layout::BottomLeft, v, WINDOW),
            Rect::new(1440., 433., 1280., 707.)
        );
    }

    #[test]
    fn finds_the_screen_a_window_is_on() {
        let screens = [LAPTOP, EXTERNAL];
        assert_eq!(screen_index(WINDOW, &screens), Some(0));
        assert_eq!(screen_index(Rect::new(2000., 0., 800., 600.), &screens), Some(1));
        // 中心在两块屏之外（拖出了屏幕顶部）：按重叠面积。
        assert_eq!(screen_index(Rect::new(1500., -1000., 800., 900.), &screens), Some(1));
        assert_eq!(screen_index(Rect::new(-5000., -5000., 10., 10.), &screens), Some(0));
        assert_eq!(screen_index(WINDOW, &[]), None);
    }

    #[test]
    fn moving_to_next_display_keeps_relative_layout() {
        // 左半屏的窗口挪到外接屏，还应该是左半屏；比新屏还大时收进可见区域。
        let left = target_frame(Layout::LeftHalf, LAPTOP.visible, WINDOW);
        assert_eq!(
            move_to_screen(left, LAPTOP.visible, EXTERNAL.visible),
            target_frame(Layout::LeftHalf, EXTERNAL.visible, WINDOW)
        );
        let back = move_to_screen(
            Rect::new(3000., 500., 2560., 1415.),
            EXTERNAL.visible,
            LAPTOP.visible,
        );
        assert_eq!(back, Rect::new(0., 25., 1440., 800.));
    }

    #[test]
    fn appkit_coordinates_flip_to_top_left_origin() {
        // NSScreen 的可见区域从左下角算：菜单栏在顶上 25 点，转换后 y 应该是 25。
        assert_eq!(
            cocoa_to_ax(Rect::new(0., 75., 1440., 800.), 900.),
            LAPTOP.visible
        );
        // 主屏上方的外接屏在 AppKit 里 y > 主屏高度，AX 里是负数。
        assert_eq!(
            cocoa_to_ax(Rect::new(1440., -240., 2560., 1440.), 900.),
            EXTERNAL.frame
        );
    }

    #[test]
    fn restore_returns_to_the_position_before_the_first_move() {
        // 左半屏 → 最大化 → 恢复，应回到最初的位置；用户中途手动拖过窗口，就以拖过后的位置为原位。
        let half = Rect::new(0., 25., 720., 800.);
        let full = LAPTOP.visible;
        let first = remember(None, WINDOW, half);
        assert_eq!(first, (WINDOW, half));
        let second = remember(Some(first), half, full);
        assert_eq!(second, (WINDOW, full));
        let dragged = Rect::new(500., 300., 700., 500.);
        assert_eq!(remember(Some(second), dragged, half), (dragged, half));
    }
}
