//! 剪贴板（设计稿 SettingsClipboard），绑定 `clipboard.*`。

use std::time::Duration;

use gpui_kit::component::WindowExt as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::config;
use crate::settings::SettingsView;
use crate::settings::model::{self, RETENTION_OPTIONS};
use crate::settings::system;
use crate::settings::widgets::{self, MenuChoice};
use crate::style::Palette;

/// 清空全部剪贴板历史的实现。剪贴板插件目前没有对外的清空接口，按钮保持禁用；
/// 接口就绪后把这里换成 `Some(crate::providers::clipboard::clear_all)` 即可（见报告）。
const CLEAR_HISTORY: Option<fn() -> anyhow::Result<()>> = Some(clear_history);

fn clear_history() -> anyhow::Result<()> {
    crate::providers::clipboard::clear_history().map(|_| ())
}

/// 「共 3,482 条 · 占用 46 MB」。读不到时不显示，原因打到 stderr。
fn stats_label() -> Option<String> {
    match crate::providers::clipboard::history_stats() {
        Ok((count, bytes)) => Some(format!(
            "共 {} 条 · 占用 {}",
            thousands(count),
            human_size(bytes)
        )),
        Err(err) => {
            eprintln!("helmp: 读取剪贴板统计失败：{err:#}");
            None
        }
    }
}

fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn human_size(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= MB {
        format!("{:.0} MB", bytes as f64 / MB)
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

impl SettingsView {
    pub(in crate::settings) fn render_clipboard(
        &mut self,
        pal: &Palette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let config = config::get();
        let clip = &config.clipboard;

        let retention: Vec<MenuChoice> = RETENTION_OPTIONS
            .iter()
            .map(|(days, label)| {
                let days = *days;
                (
                    label.to_string(),
                    clip.retention_days == days,
                    self.setter(cx, move |c| c.clipboard.retention_days = days),
                )
            })
            .collect();

        let toggle = |id: &'static str,
                      checked: bool,
                      label: &str,
                      set: fn(&mut config::Config, bool),
                      cx: &mut Context<Self>| {
            widgets::switch(
                id,
                checked,
                label,
                pal,
                cx.listener(move |this, on: &bool, _, cx| {
                    let on = *on;
                    this.save(cx, |c| set(c, on));
                }),
            )
        };

        div()
            .flex()
            .flex_col()
            .child(widgets::group(
                vec![
                    widgets::row(
                        widgets::label("记录剪贴板历史", pal),
                        toggle(
                            "clip-enabled",
                            clip.enabled,
                            "记录剪贴板历史",
                            |c, on| c.clipboard.enabled = on,
                            cx,
                        ),
                    )
                    .into_any_element(),
                    widgets::row(
                        widgets::label("保留时长", pal),
                        widgets::popup(
                            "clip-retention",
                            model::retention_label(clip.retention_days),
                            retention,
                        ),
                    )
                    .into_any_element(),
                    widgets::row(
                        widgets::label("记录图片和文件", pal),
                        toggle(
                            "clip-images",
                            clip.record_images,
                            "记录图片和文件",
                            |c, on| c.clipboard.record_images = on,
                            cx,
                        ),
                    )
                    .into_any_element(),
                ],
                pal,
            ))
            .child(widgets::group_header("隐私", pal))
            .child(widgets::group(
                vec![
                    widgets::row(
                        widgets::label_with_help(
                            "跳过标记为隐藏的内容",
                            "密码 App、1Password 等标记为隐藏或临时的内容，不会写入磁盘",
                            pal,
                        ),
                        toggle(
                            "clip-concealed",
                            clip.skip_concealed,
                            "跳过标记为隐藏的内容",
                            |c, on| c.clipboard.skip_concealed = on,
                            cx,
                        ),
                    )
                    .into_any_element(),
                    self.render_ignored_apps(&clip.ignore_apps, pal, cx),
                ],
                pal,
            ))
            .child(widgets::group_header("粘贴", pal))
            .child(widgets::group(
                vec![
                    widgets::row(
                        widgets::label_with_help(
                            "选中后自动粘贴到前台应用",
                            "需要在「隐私与安全性 › 辅助功能」中允许 Helmp；未授权时只复制",
                            pal,
                        ),
                        toggle(
                            "clip-auto-paste",
                            clip.auto_paste,
                            "选中后自动粘贴到前台应用",
                            |c, on| c.clipboard.auto_paste = on,
                            cx,
                        ),
                    )
                    .into_any_element(),
                ],
                pal,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(widgets::danger_button(
                        "clip-clear",
                        "清空全部历史…",
                        CLEAR_HISTORY.is_none(),
                        pal,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.confirm_clear_history(window, cx)
                        }),
                    ))
                    .when(CLEAR_HISTORY.is_none(), |el| {
                        el.child(widgets::help_text("剪贴板插件尚未提供清空接口", pal))
                    })
                    .when_some(stats_label(), |el, label| {
                        el.child(widgets::help_text(label, pal))
                    }),
            )
            .into_any_element()
    }

    /// 「不记录以下应用复制的内容」：App 图标 + 名字，下方 + / −。
    fn render_ignored_apps(
        &mut self,
        apps: &[String],
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut list = div()
            .flex()
            .flex_col()
            .rounded(px(8.))
            .bg(pal.control)
            .border_1()
            .border_color(pal.separator)
            .overflow_hidden();
        let mut icon_pending = false;
        for bundle_id in apps {
            let app_path = self
                .app_paths
                .entry(bundle_id.clone())
                .or_insert_with(|| system::app_path_for_bundle_id(bundle_id))
                .clone();
            let name = app_path
                .as_ref()
                .and_then(|path| path.file_stem())
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| bundle_id.clone());
            let icon = match app_path.as_deref().map(crate::icons::png_for) {
                Some(Some(png)) => img(png).flex_none().size(px(20.)).into_any_element(),
                other => {
                    // 图标在后台生成：稍后重绘一次。找不到 App 时画灰色占位。
                    icon_pending |= other.is_some();
                    div()
                        .flex_none()
                        .size(px(18.))
                        .m(px(1.))
                        .rounded(px(4.))
                        .bg(pal.key_bg)
                        .into_any_element()
                }
            };
            let selected = self.selected_ignored_app.as_deref() == Some(bundle_id.as_str());
            let id = bundle_id.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("ignored-{bundle_id}")))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(32.))
                    .px(px(10.))
                    .border_b_1()
                    .border_color(pal.separator)
                    .cursor_pointer()
                    .when(selected, |el| el.bg(pal.accent.opacity(0.12)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.selected_ignored_app = Some(id.clone());
                        cx.notify();
                    }))
                    .child(icon)
                    .child(div().text_color(pal.fg).child(name))
                    .when(app_path.is_none(), |el| {
                        el.child(widgets::help_text(format!("{bundle_id}（未安装）"), pal))
                    }),
            );
        }
        if icon_pending && !self.icon_refresh_pending {
            self.icon_refresh_pending = true;
            cx.spawn(async move |this, cx: &mut AsyncApp| {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.icon_refresh_pending = false;
                    cx.notify();
                });
            })
            .detach();
        }

        let can_remove = self
            .selected_ignored_app
            .as_ref()
            .is_some_and(|selected| apps.contains(selected));
        list = list.child(widgets::plus_minus_bar(
            "ignored-app",
            can_remove,
            pal,
            cx.listener(|this, _: &ClickEvent, _, cx| {
                this.pick_path(
                    false,
                    "不记录",
                    cx,
                    |this, app, cx| match model::bundle_id_of(&app) {
                        Ok(bundle_id) => {
                            this.selected_ignored_app = Some(bundle_id.clone());
                            this.save(cx, |c| {
                                model::add_ignored_app(c, &bundle_id);
                            });
                        }
                        Err(err) => {
                            this.error = Some(format!("无法识别所选应用：{err:#}"));
                            cx.notify();
                        }
                    },
                );
            }),
            cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Some(bundle_id) = this.selected_ignored_app.take() {
                    this.save(cx, |c| {
                        c.clipboard.ignore_apps.retain(|id| *id != bundle_id)
                    });
                }
            }),
        ));

        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(14.))
            .py(px(10.))
            .child(widgets::label("不记录以下应用复制的内容", pal))
            .child(list)
            .into_any_element()
    }

    /// 二次确认后清空（GPUI Kit 的 AlertDialog）。
    fn confirm_clear_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(clear) = CLEAR_HISTORY else {
            return;
        };
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title("清空全部剪贴板历史？")
                .description("所有记录（包括图片）都会被删除，无法恢复。")
                .show_cancel(true)
                .ok_text("清空")
                .cancel_text("取消")
                .on_ok(move |_, _, cx| {
                    let result = clear();
                    let _ = this.update(cx, |this, cx| {
                        if let Err(err) = result {
                            eprintln!("helmp: 清空剪贴板历史失败：{err:#}");
                            this.error = Some(format!("清空剪贴板历史失败：{err:#}"));
                        }
                        cx.notify();
                    });
                    true
                })
        });
    }
}
