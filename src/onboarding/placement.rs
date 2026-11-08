//! 授权浮窗的纯逻辑：系统设置窗口状态的判定、浮窗锚定位置、每次轮询该做什么。
//! 不碰 AppKit，便于单测。坐标一律用 CoreGraphics 的全局坐标（主屏左上角为原点，y 向下）。

/// 浮窗尺寸（与 Electron 参考实现一致）。
pub const GUIDE_WIDTH: f64 = 560.;
pub const GUIDE_HEIGHT: f64 = 124.;
/// 锚定在系统设置窗口内部，离它底边这么远。
pub const BOTTOM_INSET: f64 = 20.;
/// 退到屏幕底部时离屏幕底边的距离；锚定位置跑出屏幕时也按它夹住。
pub const SCREEN_MARGIN: f64 = 24.;
/// 小于这个尺寸的窗口是阴影或辅助层，不是对话框。
const MIN_DIALOG_SIDE: f64 = 100.;
/// 普通窗口与模态面板所在的层；锁屏遮罩远高于此，不能当成授权框。
const MAX_DIALOG_LAYER: i64 = 8;
/// 单独进程画的授权框（不同 macOS 版本不同）。
const AUTH_OWNERS: [&str; 3] = ["SecurityAgent", "loginwindow", "UserNotificationCenter"];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn area(&self) -> f64 {
        self.width * self.height
    }

    pub fn contains_center_of(&self, other: &Rect) -> bool {
        let (cx, cy) = (other.x + other.width / 2., other.y + other.height / 2.);
        cx >= self.x && cx < self.x + self.width && cy >= self.y && cy < self.y + self.height
    }
}

/// CGWindowList 里的一个窗口（自前向后排列）。
#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    pub owner_pid: i32,
    pub owner_name: String,
    pub layer: i64,
    pub bounds: Rect,
    /// 属于系统设置进程（按 bundle id 判断，不按本地化的进程名）。
    pub is_settings: bool,
}

impl WindowInfo {
    fn dialog_sized(&self) -> bool {
        self.bounds.width >= MIN_DIALOG_SIDE && self.bounds.height >= MIN_DIALOG_SIDE
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frontmost {
    Settings,
    /// Helmp 自己的窗口（引导窗口）。
    SelfApp,
    Other,
    Unknown,
}

/// 系统设置窗口此刻的状态。
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsState {
    /// 系统设置主窗口（当前 Space 上最大的那个）的位置；不在屏幕上时为 None。
    pub settings: Option<Rect>,
    /// 系统设置前面压着授权框（输密码）或它自己的 sheet。
    pub auth_prompt: bool,
    pub frontmost: Frontmost,
    pub settings_running: bool,
}

/// 从自前向后的窗口列表得出系统设置的状态（移植自参考实现 macos-window-bounds.swift）。
pub fn settings_state(
    windows: &[WindowInfo],
    self_pid: i32,
    settings_running: bool,
) -> SettingsState {
    let settings_windows: Vec<(usize, &WindowInfo)> = windows
        .iter()
        .enumerate()
        .filter(|(_, w)| w.layer == 0 && w.dialog_sized() && w.is_settings)
        .collect();
    // 最大的是设置窗口本身；取最前面的会锚到 sheet（包括密码框）上。
    let main = settings_windows
        .iter()
        .max_by(|a, b| a.1.bounds.area().total_cmp(&b.1.bounds.area()));
    let sheet_in_front =
        main.is_some_and(|(main_ix, _)| settings_windows.iter().any(|(ix, _)| ix < main_ix));
    let separate_auth = windows.iter().any(|w| {
        AUTH_OWNERS.contains(&w.owner_name.as_str())
            && w.dialog_sized()
            && (0..=MAX_DIALOG_LAYER).contains(&w.layer)
    });
    let frontmost = windows
        .iter()
        .find(|w| w.layer == 0 && w.dialog_sized())
        .map(|w| {
            if w.is_settings {
                Frontmost::Settings
            } else if self_pid != 0 && w.owner_pid == self_pid {
                Frontmost::SelfApp
            } else {
                Frontmost::Other
            }
        })
        .unwrap_or(Frontmost::Unknown);
    SettingsState {
        settings: main.map(|(_, w)| w.bounds),
        auth_prompt: sheet_in_front || separate_auth,
        frontmost,
        settings_running,
    }
}

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max.max(min))
}

/// 浮窗位置：系统设置窗口内底部居中；还没出现时退到工作区底部居中。始终夹在工作区内。
pub fn guide_bounds(settings: Option<Rect>, work_area: Rect) -> Rect {
    let width = GUIDE_WIDTH.min(work_area.width);
    let height = GUIDE_HEIGHT.min(work_area.height);
    let lowest_top = work_area.y + work_area.height - height - SCREEN_MARGIN;
    let (x, y) = match settings {
        Some(s) => (
            s.x + (s.width - width) / 2.,
            s.y + s.height - height - BOTTOM_INSET,
        ),
        None => (work_area.x + (work_area.width - width) / 2., lowest_top),
    };
    Rect::new(
        clamp(x, work_area.x, work_area.x + work_area.width - width).round(),
        clamp(y, work_area.y, lowest_top).round(),
        width,
        height,
    )
}

/// 让开：系统设置在问密码、用户切到了别的 App、或系统设置在别的 Space 上。
pub fn should_step_aside(state: &SettingsState) -> bool {
    state.auth_prompt
        || state.frontmost == Frontmost::Other
        || (state.settings.is_none() && state.settings_running)
}

/// 一次轮询的结论。
#[derive(Debug, Clone, PartialEq)]
pub enum Tick {
    /// 用户关掉了系统设置：浮窗跟着关。
    Close,
    /// 让开（隐藏），保持轮询。
    Hide,
    /// 显示在系统设置里；None = 系统设置还没出现，用屏幕底部的后备位置。
    Show(Option<Rect>),
}

/// `saw_settings`：本次引导里是否见过系统设置窗口。没见过之前它关着不算「被关掉」，
/// 只是还没打开完。
pub fn tick(state: &SettingsState, saw_settings: bool) -> Tick {
    if state.settings.is_none() && saw_settings && !state.settings_running {
        return Tick::Close;
    }
    if should_step_aside(state) {
        return Tick::Hide;
    }
    Tick::Show(state.settings)
}

/// 浮窗里显示什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 等用户把图标拖进列表。
    Drag,
    /// 从 target/ 直接运行，没有 .app 可拖。
    NoBundle,
    /// 已授权：显示完成并稍后自动关闭。
    Granted,
}

pub fn phase(granted: bool, bundled: bool) -> Phase {
    match (granted, bundled) {
        (true, _) => Phase::Granted,
        (false, true) => Phase::Drag,
        (false, false) => Phase::NoBundle,
    }
}

/// AppKit 坐标（主屏左下角为原点，y 向上）与 CG 坐标互转只差一次翻转，两个方向公式相同。
pub fn flip_y(rect: Rect, primary_height: f64) -> Rect {
    Rect::new(
        rect.x,
        primary_height - rect.y - rect.height,
        rect.width,
        rect.height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(pid: i32, owner: &str, layer: i64, bounds: Rect, is_settings: bool) -> WindowInfo {
        WindowInfo {
            owner_pid: pid,
            owner_name: owner.into(),
            layer,
            bounds,
            is_settings,
        }
    }

    const SCREEN: Rect = Rect {
        x: 0.,
        y: 25.,
        width: 1440.,
        height: 875.,
    };

    #[test]
    fn anchors_inside_the_settings_window_bottom() {
        // 浮窗要压在系统设置的权限列表下方，用户才知道往哪里拖。
        let settings = Rect::new(200., 100., 720., 800.);
        let b = guide_bounds(Some(settings), Rect::new(0., 25., 1440., 2000.));
        assert_eq!(b.x, 200. + (720. - 560.) / 2.);
        assert_eq!(b.y, 100. + 800. - 124. - BOTTOM_INSET);
        assert_eq!((b.width, b.height), (560., 124.));
    }

    #[test]
    fn falls_back_to_screen_bottom_and_stays_on_screen() {
        // 系统设置还没出现时放在屏幕底部；系统设置窗口靠边时浮窗也不能跑出屏幕。
        let b = guide_bounds(None, SCREEN);
        assert_eq!(b.x, (1440. - 560.) / 2.);
        assert_eq!(b.y, 25. + 875. - 124. - SCREEN_MARGIN);
        let off = guide_bounds(Some(Rect::new(1300., 500., 400., 600.)), SCREEN);
        assert_eq!(off.x, 1440. - 560.);
        assert!(off.y + off.height <= SCREEN.y + SCREEN.height);
    }

    #[test]
    fn largest_settings_window_is_the_anchor_and_sheets_count_as_prompts() {
        // 密码框是系统设置自己的 sheet：锚到它上面浮窗会跳，而且此时应该让开。
        let main = Rect::new(100., 100., 700., 800.);
        let sheet = Rect::new(300., 200., 300., 200.);
        let windows = [
            win(9, "System Settings", 0, sheet, true),
            win(9, "System Settings", 0, main, true),
        ];
        let state = settings_state(&windows, 1, true);
        assert_eq!(state.settings, Some(main));
        assert!(state.auth_prompt);
        assert_eq!(state.frontmost, Frontmost::Settings);
        assert!(should_step_aside(&state));
    }

    #[test]
    fn frontmost_app_and_separate_auth_windows() {
        let settings = win(9, "系统设置", 0, Rect::new(100., 100., 700., 800.), true);
        let mine = win(1, "helmp", 0, Rect::new(0., 0., 640., 580.), false);
        let other = win(5, "Safari", 0, Rect::new(0., 0., 900., 700.), false);
        let tiny = win(5, "Safari", 0, Rect::new(0., 0., 20., 20.), false);

        let state = settings_state(&[tiny.clone(), mine.clone(), settings.clone()], 1, true);
        assert_eq!(state.frontmost, Frontmost::SelfApp, "太小的窗口不算前台");
        assert!(!should_step_aside(&state));

        let state = settings_state(&[other, settings.clone()], 1, true);
        assert_eq!(state.frontmost, Frontmost::Other);
        assert!(should_step_aside(&state), "用户切到别的 App 时浮窗不能挡着");

        let agent = win(3, "SecurityAgent", 0, Rect::new(0., 0., 400., 300.), false);
        assert!(settings_state(&[agent, settings.clone()], 1, true).auth_prompt);
        // 锁屏遮罩的层级远高于对话框，不能当成授权框。
        let shield = win(
            3,
            "loginwindow",
            2000,
            Rect::new(0., 0., 1440., 900.),
            false,
        );
        assert!(!settings_state(&[shield, settings], 1, true).auth_prompt);
    }

    #[test]
    fn closes_only_after_settings_was_seen_and_quit() {
        // 刚点「前往授权」时系统设置还没起来；这时关掉浮窗，用户就看不到它了。
        let gone = SettingsState {
            settings: None,
            auth_prompt: false,
            frontmost: Frontmost::Unknown,
            settings_running: false,
        };
        assert_eq!(tick(&gone, false), Tick::Show(None));
        assert_eq!(tick(&gone, true), Tick::Close);
        // 系统设置还在运行但不在当前 Space：让开而不是关闭。
        let elsewhere = SettingsState {
            settings_running: true,
            ..gone
        };
        assert_eq!(tick(&elsewhere, true), Tick::Hide);
        let here = SettingsState {
            settings: Some(Rect::new(0., 0., 700., 800.)),
            frontmost: Frontmost::Settings,
            ..elsewhere
        };
        assert_eq!(tick(&here, true), Tick::Show(here.settings));
    }

    #[test]
    fn phase_follows_grant_and_bundle() {
        // 没有 .app 时拖进去的不是 Helmp，系统设置会拒绝：必须禁用拖拽并说明原因。
        assert_eq!(phase(false, true), Phase::Drag);
        assert_eq!(phase(false, false), Phase::NoBundle);
        assert_eq!(phase(true, false), Phase::Granted);
    }

    #[test]
    fn flips_between_appkit_and_cg_coordinates() {
        let cg = Rect::new(10., 20., 560., 124.);
        let appkit = flip_y(cg, 900.);
        assert_eq!(appkit.y, 900. - 20. - 124.);
        assert_eq!(flip_y(appkit, 900.), cg);
    }
}
