//! 屏幕枚举与坐标系转换。
//!
//! - `store::Frame`、GPUI 窗口及 `store::{snap_edge, dock_layout}` 统一使用 Top-Left 全局坐标：
//!   原点在主屏（`NSScreen.screens[0]`）左上角，y 向下，单位 pt。
//! - AppKit（`NSWindow::frame`、`NSScreen::frame` / `visibleFrame`、`NSEvent::mouseLocation`）
//!   使用 Bottom-Left 全局坐标：原点在主屏左下角，y 向上。

use std::ffi::c_void;

use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_foundation::NSRect;

use super::store::{Edge, Frame, TAB_W};
use crate::permissions::{double, object_for};

pub const DEFAULT_CARD_W: f64 = 280.0;
pub const DEFAULT_CARD_H: f64 = 200.0;
pub const MIN_CARD_W: f64 = 220.0;
pub const MIN_CARD_H: f64 = 140.0;

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    pub id: u32,
    pub uuid: String,
    /// Top-Left 全局坐标。
    pub frame: Frame,
    /// Top-Left 全局坐标（去掉菜单栏与 Dock）。
    pub visible: Frame,
}

#[repr(C)]
struct CfUuidBytes {
    bytes: [u8; 16],
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFUUIDGetUUIDBytes(uuid: *const c_void) -> CfUuidBytes;
    fn CFRelease(cf: *const c_void);
}

fn display_uuid(display_id: u32) -> String {
    // SAFETY: `CGDisplayCreateUUIDFromDisplayID` 返回新建的 CFUUID（或 null），使用后必须 CFRelease。
    unsafe {
        let cf_uuid = CGDisplayCreateUUIDFromDisplayID(display_id);
        if cf_uuid.is_null() {
            return format!("display-{display_id}");
        }
        let raw = CFUUIDGetUUIDBytes(cf_uuid);
        CFRelease(cf_uuid);
        uuid::Uuid::from_bytes(raw.bytes).to_string()
    }
}

/// 主屏高度（`NSScreen.screens[0]`，AppKit 与 Top-Left 坐标互转基准）。
pub fn primary_height(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm)
        .firstObject()
        .map_or(0.0, |s| s.frame().size.height)
}

/// 枚举当前所有屏幕（第一块为主屏）。坐标为 Top-Left 全局坐标。
pub fn all_screens(mtm: MainThreadMarker) -> Vec<DisplayInfo> {
    let screens = NSScreen::screens(mtm);
    let primary_h = screens
        .firstObject()
        .map_or(0.0, |s| s.frame().size.height);
    screens
        .iter()
        .filter_map(|screen| {
            let desc = screen.deviceDescription();
            let number = object_for(&desc, "NSScreenNumber")?;
            let id = double(&number)? as u32;
            Some(DisplayInfo {
                id,
                uuid: display_uuid(id),
                frame: cocoa_to_tl(from_ns_rect(screen.frame()), primary_h),
                visible: cocoa_to_tl(from_ns_rect(screen.visibleFrame()), primary_h),
            })
        })
        .collect()
}

fn from_ns_rect(rect: NSRect) -> Frame {
    Frame {
        x: rect.origin.x,
        y: rect.origin.y,
        w: rect.size.width,
        h: rect.size.height,
    }
}

/// AppKit 全局坐标（主屏左下原点、y 向上）→ Top-Left 全局坐标（主屏左上原点、y 向下）。
pub fn cocoa_to_tl(rect: Frame, primary_height: f64) -> Frame {
    Frame {
        x: rect.x,
        y: primary_height - (rect.y + rect.h),
        w: rect.w,
        h: rect.h,
    }
}

/// Top-Left 全局坐标 → AppKit 全局坐标。
pub fn tl_to_cocoa(rect: Frame, primary_height: f64) -> Frame {
    Frame {
        x: rect.x,
        y: primary_height - (rect.y + rect.h),
        w: rect.w,
        h: rect.h,
    }
}

pub fn point_cocoa_to_tl(x: f64, y: f64, primary_height: f64) -> (f64, f64) {
    (x, primary_height - y)
}

/// 优先按持久化 UUID 找屏幕；找不到（外接屏拔掉等）回退主屏 `screens.first()`。
pub fn resolve_display<'a>(screens: &'a [DisplayInfo], uuid: &str) -> Option<&'a DisplayInfo> {
    screens
        .iter()
        .find(|screen| screen.uuid == uuid)
        .or_else(|| screens.first())
}

/// 矩形主要落在哪块屏：先看中心点，再看重叠面积最大，全不重叠则回退主屏。
pub fn screen_for_frame(screens: &[DisplayInfo], frame: Frame) -> Option<&DisplayInfo> {
    if screens.is_empty() {
        return None;
    }
    let cx = frame.x + frame.w / 2.0;
    let cy = frame.y + frame.h / 2.0;
    if let Some(hit) = screens.iter().find(|screen| {
        let f = screen.frame;
        cx >= f.x && cx <= f.x + f.w && cy >= f.y && cy <= f.y + f.h
    }) {
        return Some(hit);
    }
    screens
        .iter()
        .max_by(|a, b| {
            overlap(a.frame, frame)
                .partial_cmp(&overlap(b.frame, frame))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .filter(|screen| overlap(screen.frame, frame) > 0.0)
        .or_else(|| screens.first())
}

fn overlap(a: Frame, b: Frame) -> f64 {
    let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    w.max(0.0) * h.max(0.0)
}

pub fn card_size(saved: Option<(f64, f64)>, visible: Frame) -> (f64, f64) {
    let (w, h) = saved.unwrap_or((DEFAULT_CARD_W, DEFAULT_CARD_H));
    let w = if w.is_finite() && w >= MIN_CARD_W {
        w
    } else {
        DEFAULT_CARD_W
    };
    let h = if h.is_finite() && h >= MIN_CARD_H {
        h
    } else {
        DEFAULT_CARD_H
    };
    let max_w = (visible.w - 16.0).max(TAB_W + 40.0);
    let max_h = (visible.h - 16.0).max(MIN_CARD_H);
    (w.min(max_w).round(), h.min(max_h).round())
}

/// 保证自由浮窗完整留在当前屏幕可见区域内（至少不会飞到屏幕外找不回来）。
pub fn clamp_floating(frame: Frame, visible: Frame) -> Frame {
    let (w, h) = card_size(Some((frame.w, frame.h)), visible);
    let max_x = (visible.x + visible.w - w).max(visible.x);
    let max_y = (visible.y + visible.h - h).max(visible.y);
    let x = if frame.x.is_finite() {
        frame.x.clamp(visible.x, max_x)
    } else {
        visible.x + (visible.w - w) / 2.0
    };
    let y = if frame.y.is_finite() {
        frame.y.clamp(visible.y, max_y)
    } else {
        visible.y + (visible.h - h) / 2.0
    };
    Frame {
        x: x.round(),
        y: y.round(),
        w,
        h,
    }
}

/// 停靠耳标在悬停或编辑展开时对应的卡片 Frame：
/// - 贴着屏幕内侧就地展开，绝不跨过屏幕边界挡到相邻显示器。
/// - 垂直方向尽量以耳标中心对齐，同时限制在 `visible` 上下界内。
pub fn expanded_frame_for_docked(
    tab: Frame,
    saved_size: Option<(f64, f64)>,
    edge: Edge,
    visible: Frame,
) -> Frame {
    let (w, h) = card_size(saved_size, visible);
    let x = match edge {
        Edge::Left => visible.x,
        Edge::Right => visible.x + visible.w - w,
    };
    let tab_center_y = tab.y + tab.h / 2.0;
    let max_y = (visible.y + visible.h - h).max(visible.y);
    let y = (tab_center_y - h / 2.0).clamp(visible.y, max_y);
    Frame {
        x: x.round(),
        y: y.round(),
        w,
        h,
    }
}

/// 在屏幕上放一张默认自由卡片时的位置（主屏靠右上方或居中偏右）。
pub fn default_floating_frame(visible: Frame) -> Frame {
    let (w, h) = card_size(None, visible);
    Frame {
        x: (visible.x + (visible.w - w) / 2.0).round(),
        y: (visible.y + (visible.h - h) / 3.0).round(),
        w,
        h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn displays() -> Vec<DisplayInfo> {
        vec![
            DisplayInfo {
                id: 1,
                uuid: "primary".into(),
                frame: Frame {
                    x: 0.0,
                    y: 0.0,
                    w: 1512.0,
                    h: 982.0,
                },
                visible: Frame {
                    x: 0.0,
                    y: 37.0,
                    w: 1512.0,
                    h: 880.0,
                },
            },
            // 外接屏在右侧且顶边比主屏高 300pt
            DisplayInfo {
                id: 2,
                uuid: "external".into(),
                frame: Frame {
                    x: 1512.0,
                    y: -300.0,
                    w: 2560.0,
                    h: 1440.0,
                },
                visible: Frame {
                    x: 1512.0,
                    y: -275.0,
                    w: 2560.0,
                    h: 1415.0,
                },
            },
        ]
    }

    #[test]
    fn cocoa_and_top_left_round_trip_across_screens() {
        // 多屏负坐标换算出错会让便签跳到屏幕外或上下颠倒。
        let primary_h = 982.0;
        let samples = [
            Frame {
                x: 0.0,
                y: 37.0,
                w: 280.0,
                h: 200.0,
            },
            Frame {
                x: 1512.0,
                y: -275.0,
                w: 24.0,
                h: 72.0,
            },
        ];
        for sample in samples {
            let cocoa = tl_to_cocoa(sample, primary_h);
            let back = cocoa_to_tl(cocoa, primary_h);
            assert_eq!(back, sample);
        }
    }

    #[test]
    fn missing_display_falls_back_to_primary() {
        // 拔掉外接屏重启后，原来停靠在外接屏的便签必须回到主屏而不是消失。
        let list = displays();
        assert_eq!(resolve_display(&list, "external").unwrap().uuid, "external");
        assert_eq!(resolve_display(&list, "gone").unwrap().uuid, "primary");
    }

    #[test]
    fn expanded_dock_card_stays_inside_visible_and_never_crosses_edge() {
        // 靠右停靠时只能向当前屏幕左侧展开；多屏拼接处绝不能露到相邻显示器里。
        let list = displays();
        let primary = &list[0];
        let tab_right = Frame {
            x: primary.visible.x + primary.visible.w - TAB_W,
            y: primary.visible.y + primary.visible.h - 80.0,
            w: TAB_W,
            h: 72.0,
        };
        let card = expanded_frame_for_docked(tab_right, None, Edge::Right, primary.visible);
        assert_eq!(card.x + card.w, primary.visible.x + primary.visible.w);
        assert!(card.x >= primary.visible.x);
        assert!(card.y >= primary.visible.y);
        assert!(card.y + card.h <= primary.visible.y + primary.visible.h);

        let external = &list[1];
        let tab_left = Frame {
            x: external.visible.x,
            y: external.visible.y,
            w: TAB_W,
            h: 72.0,
        };
        let card_ext = expanded_frame_for_docked(tab_left, None, Edge::Left, external.visible);
        assert_eq!(card_ext.x, external.visible.x);
        assert_eq!(card_ext.y, external.visible.y);
        assert!(card_ext.y + card_ext.h <= external.visible.y + external.visible.h);
    }

    #[test]
    fn screen_for_frame_picks_owning_display_even_when_partially_off_edge() {
        // 拖着浮窗跨越屏幕边缘时，应选中心所在屏幕或重叠面积最大的屏幕来判定吸边。
        let list = displays();
        let on_external = Frame {
            x: 1500.0,
            y: 100.0,
            w: 280.0,
            h: 200.0,
        };
        assert_eq!(
            screen_for_frame(&list, on_external).unwrap().uuid,
            "external"
        );
    }
}
