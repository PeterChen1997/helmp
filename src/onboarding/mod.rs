//! 首次启动权限引导窗口（设计稿 Onboarding）。
//!
//! 三项权限各显示真实状态：辅助功能（`AXIsProcessTrusted`）、录屏（`CGPreflightScreenCaptureAccess`）、
//! 自动化（无法静默查询，只说明「首次跳转时系统会询问」）。只查询，绝不调用会弹系统授权框的 API；
//! 「前往授权」打开系统设置的对应面板。窗口获得焦点时重新查询（用户从系统设置授权回来）。
//! 「继续」「跳过」都把 `onboarding_done` 设为 true 并关窗。
//!
//! 辅助功能、录屏两项点「前往授权」时，另外在系统设置里浮出拖拽卡片（`guide`），
//! 把 Helmp 图标拖进权限列表即可完成添加。

mod guide;
mod native;
mod placement;

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::provider::Tint;
use crate::settings::{brand_icon, open_transparent_window, sync_component_theme};
use crate::style::{self, Palette};

const WINDOW_SIZE: Size<Pixels> = size(px(640.), px(580.));

/// 一项权限的当前状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Granted,
    NotGranted,
    /// 系统不提供静默查询（自动化）。
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Accessibility,
    ScreenRecording,
    Automation,
}

impl Permission {
    pub const ALL: [Permission; 3] = [
        Permission::Accessibility,
        Permission::ScreenRecording,
        Permission::Automation,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Permission::Accessibility => "辅助功能",
            Permission::ScreenRecording => "录屏与系统录音",
            Permission::Automation => "自动化",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Permission::Accessibility => "选中剪贴板记录后自动粘贴到前台应用",
            Permission::ScreenRecording => "区域截图与识别文字",
            Permission::Automation => {
                "跳转到 iTerm 或终端里正在运行的 Agent 会话；首次跳转时系统会询问"
            }
        }
    }

    /// 需要把 App 拖进系统设置列表的权限（自动化由系统在首次使用时询问，不需要拖）。
    pub fn needs_drag(self) -> bool {
        matches!(
            self,
            Permission::Accessibility | Permission::ScreenRecording
        )
    }

    fn icon(self) -> (&'static str, Tint) {
        match self {
            Permission::Accessibility => ("accessibility", Tint::Blue),
            Permission::ScreenRecording => ("scan", Tint::Indigo),
            Permission::Automation => ("terminal", Tint::Graphite),
        }
    }

    /// 「系统设置 › 隐私与安全性」里对应面板的 URL。
    pub fn settings_url(self) -> &'static str {
        match self {
            Permission::Accessibility => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            Permission::ScreenRecording => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
            }
            Permission::Automation => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation"
            }
        }
    }

    /// 只读查询，不弹框。
    pub fn status(self) -> Status {
        match self {
            // SAFETY: 无参数的只读查询。
            Permission::Accessibility => {
                if crate::permissions::accessibility_trusted() {
                    Status::Granted
                } else {
                    Status::NotGranted
                }
            }
            Permission::ScreenRecording => {
                // 实时判定：系统的预检接口在 App 重启前不会变，授权后会一直显示「前往授权」。
                if crate::permissions::screen_recording_granted() {
                    Status::Granted
                } else {
                    Status::NotGranted
                }
            }
            Permission::Automation => Status::Unknown,
        }
    }
}

/// 右侧显示什么：已授权的绿色对勾，或者一个按钮（主按钮 / 次按钮）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trailing {
    Granted,
    Button { label: &'static str, primary: bool },
}

pub fn trailing(status: Status) -> Trailing {
    match status {
        Status::Granted => Trailing::Granted,
        Status::NotGranted => Trailing::Button {
            label: "前往授权",
            primary: true,
        },
        // 查不到状态：不催用户，给一个次要按钮，想提前看看也可以。
        Status::Unknown => Trailing::Button {
            label: "打开设置",
            primary: false,
        },
    }
}

struct OnboardingWindow(AnyWindowHandle);

impl Global for OnboardingWindow {}

/// 配置里 `onboarding_done` 为 false 时显示引导窗口。
pub fn maybe_show(cx: &mut App) {
    if crate::config::get().onboarding_done {
        return;
    }
    show(cx);
    // 自检用：HELMP_ONBOARDING_GUIDE=accessibility|screen 直接走一次「前往授权」（配合 HELMP_TRACE 看轮询）。
    if let Some(permission) = std::env::var("HELMP_ONBOARDING_GUIDE")
        .ok()
        .and_then(|name| match name.as_str() {
            "accessibility" => Some(Permission::Accessibility),
            "screen" => Some(Permission::ScreenRecording),
            _ => None,
        })
        && let Some(handle) = cx.try_global::<OnboardingWindow>().map(|global| global.0)
    {
        let _ = handle.update(cx, |_, window, cx| {
            let onboarding = window
                .root::<base::Root>()
                .flatten()
                .and_then(|root| root.read(cx).view().clone().downcast::<Onboarding>().ok());
            if let Some(onboarding) = onboarding {
                cx.open_url(permission.settings_url());
                guide::open(permission, onboarding.downgrade(), cx);
            }
        });
    }
}

/// 显示（或前置）引导窗口。
pub fn show(cx: &mut App) {
    cx.activate(true);
    if let Some(handle) = cx.try_global::<OnboardingWindow>().map(|global| global.0)
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("欢迎使用 Helmp".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(16.), px(16.))),
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            WINDOW_SIZE,
            cx,
        ))),
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        is_movable: true,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Blurred,
        ..Default::default()
    };
    match open_transparent_window(options, cx, |window, cx| {
        cx.new(|cx| Onboarding::new(window, cx))
    }) {
        Ok(handle) => {
            cx.set_global(OnboardingWindow(handle));
            let _ = handle.update(cx, |_, window, cx| {
                sync_component_theme(window, cx);
                window.activate_window();
            });
        }
        Err(err) => eprintln!("helmp: 打开引导窗口失败：{err:#}"),
    }
}

pub struct Onboarding {
    statuses: [Status; 3],
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
    _refresh: Task<()>,
}

impl Onboarding {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::dock::track_window(cx);
        let subscriptions = vec![
            // 用户去系统设置授权后切回来：重新查询状态。
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.refresh(cx);
                }
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                sync_component_theme(window, cx);
            }),
        ];
        // 用户可能在系统设置里授权而不切回来：每秒查一次（只读查询，开销很小）。
        let refresh = cx.spawn(async move |this, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        let statuses = Permission::ALL.map(Permission::status);
        crate::trace::log(format!("引导页权限状态（初始）：{statuses:?}"));
        Self {
            statuses,
            error: None,
            _subscriptions: subscriptions,
            _refresh: refresh,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let statuses = Permission::ALL.map(Permission::status);
        if statuses != self.statuses {
            crate::trace::log(format!("引导页权限状态：{statuses:?}"));
            // 刚在系统设置里授权完：把引导窗口拉回前台，用户不用自己切回来。
            let newly_granted = statuses
                .iter()
                .zip(&self.statuses)
                .any(|(now, before)| *now == Status::Granted && *before != Status::Granted);
            if newly_granted {
                cx.activate(true);
            }
            self.statuses = statuses;
            cx.notify();
        }
    }

    /// 「继续」「跳过」：记下已完成并关窗。写盘失败时不关窗，把原因显示出来。
    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match crate::config::update(|c| c.onboarding_done = true) {
            Ok(()) => {
                guide::close(cx);
                window.remove_window();
            }
            Err(err) => {
                eprintln!("helmp: 保存引导状态失败：{err:#}");
                self.error = Some(format!("保存失败：{err:#}"));
                cx.notify();
            }
        }
    }

    fn render_row(&self, permission: Permission, pal: &Palette, cx: &mut Context<Self>) -> Div {
        let ix = Permission::ALL
            .iter()
            .position(|p| *p == permission)
            .unwrap_or_default();
        let status = self.statuses.get(ix).copied().unwrap_or(Status::Unknown);
        let (icon, tint) = permission.icon();
        let trailing: AnyElement = match trailing(status) {
            Trailing::Granted => div()
                .flex()
                .items_center()
                .gap(px(5.))
                .text_color(granted_color(pal))
                .font_weight(FontWeight::SEMIBOLD)
                .child(
                    gpui_kit::component::Icon::empty()
                        .path("icons/circle-check.svg")
                        .text_color(granted_color(pal))
                        .size(px(15.)),
                )
                .child("已授权")
                .into_any_element(),
            Trailing::Button { label, primary } => {
                let url = permission.settings_url();
                div()
                    .id(SharedString::from(format!("grant-{}", permission.title())))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(28.))
                    .px(px(14.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .when(primary, |el| {
                        el.bg(pal.accent)
                            .text_color(pal.selection_fg)
                            .hover(|el| el.opacity(0.9))
                    })
                    .when(!primary, |el| {
                        el.bg(pal.key_bg)
                            .text_color(pal.fg)
                            .hover(|el| el.bg(pal.separator))
                    })
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        cx.open_url(url);
                        if permission.needs_drag() {
                            guide::open(permission, cx.entity().downgrade(), cx);
                        }
                    }))
                    .child(label)
                    .into_any_element()
            }
        };
        div()
            .flex()
            .items_center()
            .gap(px(14.))
            .min_h(px(64.))
            .px(px(16.))
            .py(px(10.))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(36.))
                    .rounded(px(9.))
                    .bg(style::tint(tint))
                    .child(
                        gpui_kit::component::Icon::empty()
                            .path(SharedString::from(format!("icons/{icon}.svg")))
                            .text_color(gpui_kit::white())
                            .size(px(20.)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(pal.fg)
                            .child(permission.title()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .text_color(pal.muted)
                            .child(permission.help()),
                    ),
            )
            .child(div().flex_none().child(trailing))
    }
}

fn granted_color(pal: &Palette) -> Hsla {
    style::tone_colors(crate::provider::Tone::Done, pal.dark).1
}

impl Render for Onboarding {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = style::palette(window);

        let mut group = div()
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(pal.group)
            .border_1()
            .border_color(pal.separator)
            .overflow_hidden();
        for (ix, permission) in Permission::ALL.into_iter().enumerate() {
            group = group.child(
                self.render_row(permission, &pal, cx)
                    .when(ix > 0, |row| row.border_t_1().border_color(pal.separator)),
            );
        }

        let mut footer = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .mt_auto()
            .px(px(24.))
            .pt(px(18.))
            .pb(px(20.));
        footer = match &self.error {
            Some(err) => footer.child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(pal.danger)
                    .child(err.clone()),
            ),
            None => footer.child(div().flex_1()),
        };
        footer = footer
            .child(
                div()
                    .id("onboarding-skip")
                    .flex()
                    .items_center()
                    .h(px(28.))
                    .px(px(14.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .text_color(pal.accent)
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|el| el.bg(pal.key_bg))
                    .on_click(
                        cx.listener(|this, _: &ClickEvent, window, cx| this.finish(window, cx)),
                    )
                    .child("跳过"),
            )
            .child(
                div()
                    .id("onboarding-continue")
                    .flex()
                    .items_center()
                    .h(px(28.))
                    .px(px(14.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .bg(pal.accent)
                    .text_color(pal.selection_fg)
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|el| el.opacity(0.9))
                    .on_click(
                        cx.listener(|this, _: &ClickEvent, window, cx| this.finish(window, cx)),
                    )
                    .child("继续"),
            );

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(pal.window)
            .text_color(pal.fg)
            .text_size(px(13.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .px(px(40.))
                    // 顶部 26 px 给红绿灯。
                    .pt(px(38.))
                    .pb(px(22.))
                    .child(brand_icon(px(72.)))
                    .child(
                        div()
                            .mt(px(6.))
                            .text_size(px(22.))
                            .font_weight(FontWeight::BOLD)
                            .child("授予权限"),
                    )
                    .child(
                        div()
                            .max_w(px(440.))
                            .text_center()
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .text_color(pal.muted)
                            .child(
                                "Helmp 只在需要时请求权限。不授权也能搜索应用、查看 Agent 会话和剪贴板历史，\
                                 下面三项用于更完整的体验。",
                            ),
                    ),
            )
            .child(div().px(px(32.)).child(group))
            .child(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::{Permission, Status, Trailing, trailing};

    #[test]
    fn status_maps_to_the_right_affordance() {
        // 已授权不该再出现按钮；查不到状态（自动化）不该用醒目的主按钮催用户。
        assert_eq!(trailing(Status::Granted), Trailing::Granted);
        assert_eq!(
            trailing(Status::NotGranted),
            Trailing::Button {
                label: "前往授权",
                primary: true
            }
        );
        assert!(matches!(
            trailing(Status::Unknown),
            Trailing::Button { primary: false, .. }
        ));
    }

    #[test]
    fn every_permission_opens_its_own_privacy_pane() {
        // 打开错面板，用户在系统设置里找不到 Helmp 该勾的地方。
        let urls: Vec<_> = Permission::ALL.iter().map(|p| p.settings_url()).collect();
        assert!(urls.iter().all(|url| {
            url.starts_with("x-apple.systempreferences:com.apple.preference.security?Privacy_")
        }));
        assert!(urls[0].ends_with("Accessibility"));
        assert!(urls[1].ends_with("ScreenCapture"));
        assert!(urls[2].ends_with("Automation"));
    }

    #[test]
    fn automation_is_never_queried() {
        // 查询自动化权限的唯一办法会弹系统授权框；引导页绝不能触发它。
        assert_eq!(Permission::Automation.status(), Status::Unknown);
    }
}
