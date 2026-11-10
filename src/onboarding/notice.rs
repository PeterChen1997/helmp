//! 「Helmp 已在运行」卡片：引导完成后再手动打开 Helmp（或在运行中再次打开）时显示，
//! 告诉用户确实打开成功了、怎么唤起，而不是又把人带回授权页。
//!
//! 一张液态玻璃小卡片，屏幕居中偏上，8 秒后自动消失；还有权限没给时多一行入口。

use std::time::Duration;

use gpui_kit::*;

use super::{Permission, Status};
use crate::i18n::{tr, trf};
use crate::style;

const WIDTH: f32 = 380.;
const HEIGHT: f32 = 150.;
const AUTO_CLOSE: Duration = Duration::from_secs(8);

struct NoticeWindow(AnyWindowHandle);

impl Global for NoticeWindow {}

/// 显示（或重新显示）卡片。
pub fn show(cx: &mut App) {
    if let Some(existing) = cx.try_global::<NoticeWindow>() {
        let handle = existing.0;
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
    let mut bounds = Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx);
    // 居中偏上，和面板唤起的位置大致对齐。
    bounds.origin.y = (bounds.origin.y - px(140.)).max(px(80.));
    let options = WindowOptions {
        kind: WindowKind::PopUp,
        titlebar: None,
        focus: false,
        show: true,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        ..Default::default()
    };
    match crate::settings::open_transparent_window(options, cx, |window, cx| {
        cx.new(|cx| Notice::new(window, cx))
    }) {
        Ok(handle) => {
            let _ = handle.update(cx, |_, window, _| crate::panel::install_glass(window));
            cx.set_global(NoticeWindow(handle));
            crate::trace::log("显示「Helmp 已在运行」卡片");
        }
        Err(err) => eprintln!("helmp: 打开「已在运行」卡片失败：{err:#}"),
    }
}

struct Notice {
    missing: usize,
    _close: Task<()>,
}

impl Notice {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let missing = Permission::ALL
            .into_iter()
            .filter(|permission| permission.status() == Status::NotGranted)
            .count();
        let close = cx.spawn_in(window, async move |_, cx| {
            cx.background_executor().timer(AUTO_CLOSE).await;
            let _ = cx.update(|window, _| window.remove_window());
        });
        // 排查 / 自检：HELMP_SELF_TEST_NOTICE_OPEN 时 1 秒后自动点「打开 Helmp」。
        if std::env::var_os("HELMP_SELF_TEST_NOTICE_OPEN").is_some() {
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let _ = this.update_in(cx, |this, window, cx| this.open_panel(window, cx));
            })
            .detach();
        }
        Self {
            missing,
            _close: close,
        }
    }

    fn open_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.remove_window();
        cx.defer(|cx| {
            crate::launcher::LauncherHandle::update(cx, |launcher, window, cx| {
                launcher.show(window, cx)
            })
        });
    }
}

fn button(id: &'static str, label: &'static str, primary: bool, pal: &style::Palette) -> Stateful<Div> {
    let base = div()
        .id(id)
        .flex()
        .items_center()
        .h(px(28.))
        .px(px(14.))
        .rounded(px(8.))
        .cursor_pointer()
        .font_weight(FontWeight::MEDIUM)
        .child(label);
    if primary {
        base.bg(pal.accent)
            .text_color(pal.selection_fg)
            .hover(|el| el.opacity(0.9))
    } else {
        base.text_color(pal.fg).hover(|el| el.bg(pal.key_bg))
    }
}

impl Render for Notice {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = style::palette(window);
        let hint = match super::toggle_glyphs() {
            Some(keys) => trf!("按 {keys} 随时唤起", "Press {keys} to open it anytime"),
            None => tr(
                "还没设唤起快捷键，可以在 设置 › 通用 里录一个",
                "No shortcut yet; record one in Settings › General",
            )
            .to_string(),
        };
        let mut text = div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .min_w_0()
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(tr("Helmp 已在运行", "Helmp is running")),
            )
            .child(div().text_color(pal.muted).child(hint));
        if self.missing > 0 {
            let missing = self.missing;
            text = text.child(
                div()
                    .id("notice-permissions")
                    .text_size(px(12.))
                    .text_color(pal.accent)
                    .cursor_pointer()
                    .on_click(cx.listener(|_, _: &ClickEvent, window, cx| {
                        window.remove_window();
                        cx.defer(super::show);
                    }))
                    .child(trf!(
                        "还有 {missing} 项权限未授予，去看看",
                        "{missing} permission(s) not granted yet; review"
                    )),
            );
        }
        div()
            .flex()
            .flex_col()
            .justify_between()
            .size_full()
            .px(px(20.))
            .py(px(16.))
            .rounded(px(crate::panel::CORNER_RADIUS as f32))
            .bg(pal.panel)
            .text_color(pal.fg)
            .text_size(px(13.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .child(crate::settings::brand_icon(px(48.)))
                    .child(text),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        button("notice-close", tr("关闭", "Close"), false, &pal).on_click(
                            cx.listener(|_, _: &ClickEvent, window, _| window.remove_window()),
                        ),
                    )
                    .child(
                        button("notice-open", tr("打开 Helmp", "Open Helmp"), true, &pal)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.open_panel(window, cx)
                            })),
                    ),
            )
    }
}
