//! 通用（设计稿 SettingsGeneral）。

use gpui_kit::*;

use crate::config::{self, Language, PanelScreen};
use crate::i18n::tr;
use crate::settings::SettingsView;
use crate::settings::model::{self, LANGUAGE_OPTIONS, PANEL_SCREEN_OPTIONS, RESET_OPTIONS};
use crate::settings::system::{self, LoginItemStatus};
use crate::settings::widgets::{self, MenuChoice};
use crate::style::Palette;

impl SettingsView {
    pub(in crate::settings) fn render_general(
        &mut self,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let config = config::get();
        let general = &config.general;

        let launch = {
            let mut text = div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(widgets::label(tr("开机时自动启动", "Launch at login"), pal));
            if let Some(err) = &self.login_error {
                text = text.child(widgets::error_text(err.clone(), pal));
            } else if !system::running_from_bundle() {
                text = text.child(widgets::help_text(
                    tr(
                        "需要从 Helmp.app 运行才能注册登录项（当前直接运行了二进制）",
                        "Run Helmp.app to add it as a login item (this is the bare binary).",
                    ),
                    pal,
                ));
            } else if self.login_status == Some(LoginItemStatus::RequiresApproval) {
                text = text.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(widgets::help_text(
                            tr(
                                "需要在系统设置的登录项中允许 Helmp",
                                "Allow Helmp in Login Items in System Settings.",
                            ),
                            pal,
                        ))
                        .child(widgets::push_button(
                            "open-login-items",
                            tr("打开登录项设置", "Open Login Items"),
                            false,
                            |_, _, _| system::open_login_items_settings(),
                        )),
                );
            }
            widgets::row(
                text,
                widgets::switch(
                    "launch-at-login",
                    general.launch_at_login,
                    tr("开机时自动启动", "Launch at login"),
                    pal,
                    cx.listener(|this, on: &bool, _, cx| this.set_launch_at_login(*on, cx)),
                ),
            )
        };

        let reset_choices: Vec<MenuChoice> = RESET_OPTIONS
            .iter()
            .map(|(secs, zh, en)| {
                let secs = *secs;
                (
                    tr(zh, en).to_string(),
                    general.reset_after_secs == secs,
                    self.setter(cx, move |c| c.general.reset_after_secs = secs),
                )
            })
            .collect();
        let screen_choices: Vec<MenuChoice> = PANEL_SCREEN_OPTIONS
            .iter()
            .map(|(screen, zh, en)| {
                let screen: PanelScreen = *screen;
                (
                    tr(zh, en).to_string(),
                    general.panel_screen == screen,
                    self.setter(cx, move |c| c.general.panel_screen = screen),
                )
            })
            .collect();
        // 切换后 `i18n` 经配置订阅更新全局语言，save 里的 notify 让本窗口立即按新语言重绘。
        let language_choices: Vec<MenuChoice> = LANGUAGE_OPTIONS
            .iter()
            .map(|(language, zh, en)| {
                let language: Language = *language;
                (
                    tr(zh, en).to_string(),
                    general.language == language,
                    self.setter(cx, move |c| c.general.language = language),
                )
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .child(widgets::group(
                vec![
                    widgets::row(
                        widgets::label(tr("唤起快捷键", "Keyboard shortcut"), pal),
                        self.hotkey.clone(),
                    )
                    .into_any_element(),
                    launch.into_any_element(),
                    widgets::row(
                        widgets::label(tr("在菜单栏显示图标", "Show in menu bar"), pal),
                        widgets::switch(
                            "show-menubar-icon",
                            general.show_menubar_icon,
                            tr("在菜单栏显示图标", "Show in menu bar"),
                            pal,
                            cx.listener(|this, on: &bool, _, cx| {
                                let on = *on;
                                this.save(cx, |c| c.general.show_menubar_icon = on);
                            }),
                        ),
                    )
                    .into_any_element(),
                    // 中文界面的标签带上 "Language"：不认识中文的人误切过来也能找到这一行切回去。
                    widgets::row(
                        widgets::label(tr("语言 / Language", "Language"), pal),
                        widgets::popup(
                            "language",
                            model::language_label(general.language),
                            language_choices,
                        ),
                    )
                    .into_any_element(),
                ],
                pal,
            ))
            .child(widgets::group_header(tr("面板", "Panel"), pal))
            .child(widgets::group(
                vec![
                    widgets::row(
                        widgets::label_with_help(
                            tr("重新打开时清空输入", "Clear search after"),
                            tr(
                                "在这段时间内重新唤起，会保留上次的输入并全选",
                                "Reopen within this time to keep your last search, selected.",
                            ),
                            pal,
                        ),
                        widgets::popup(
                            "reset-after",
                            model::reset_label(general.reset_after_secs),
                            reset_choices,
                        ),
                    )
                    .into_any_element(),
                    widgets::row(
                        widgets::label_with_help(
                            tr("唤起时切换到英文输入法", "Switch to ABC input source when opened"),
                            tr(
                                "中文输入法下，拼音缩写会进入组字，按键到不了搜索框",
                                "With a Chinese or other IME, keystrokes go to composition instead of the search field.",
                            ),
                            pal,
                        ),
                        widgets::switch(
                            "switch-input-source",
                            general.switch_input_source,
                            tr("唤起时切换到英文输入法", "Switch to ABC input source when opened"),
                            pal,
                            cx.listener(|this, on: &bool, _, cx| {
                                let on = *on;
                                this.save(cx, |c| c.general.switch_input_source = on);
                            }),
                        ),
                    )
                    .into_any_element(),
                    widgets::row(
                        widgets::label(tr("面板出现在", "Show panel on"), pal),
                        widgets::popup(
                            "panel-screen",
                            model::panel_screen_label(general.panel_screen),
                            screen_choices,
                        ),
                    )
                    .into_any_element(),
                ],
                pal,
            ))
            .child(widgets::group_header(tr("更新", "Updates"), pal))
            .child(widgets::group(
                vec![
                    widgets::row(
                        widgets::label(
                            tr("自动检查更新", "Automatically check for updates"),
                            pal,
                        ),
                        widgets::switch(
                            "auto-check-updates",
                            general.auto_check_updates,
                            tr("自动检查更新", "Automatically check for updates"),
                            pal,
                            cx.listener(|this, on: &bool, _, cx| {
                                let on = *on;
                                this.save(cx, |c| c.general.auto_check_updates = on);
                            }),
                        ),
                    )
                    .into_any_element(),
                    widgets::row(
                        div()
                            .text_size(px(13.))
                            .text_color(pal.muted)
                            .child(format!("Helmp {}", env!("CARGO_PKG_VERSION"))),
                        // 还没有更新服务：按钮禁用，而不是点了假装「已是最新版本」。
                        widgets::push_button(
                            "check-update",
                            tr("立即检查", "Check Now"),
                            true,
                            |_, _, _| {},
                        )
                        .tooltip(tr("更新服务尚未接入", "Update service isn't available yet")),
                    )
                    .into_any_element(),
                ],
                pal,
            ))
            .into_any_element()
    }

    /// 先向系统注册 / 注销登录项，成功后才写配置：配置里的开关必须反映真实状态。
    fn set_launch_at_login(&mut self, on: bool, cx: &mut Context<Self>) {
        match system::set_launch_at_login(on) {
            Ok(()) => {
                self.login_error = None;
                self.save(cx, |c| c.general.launch_at_login = on);
            }
            Err(err) => {
                eprintln!("helmp: 设置开机自动启动失败：{err:#}");
                self.login_error = Some(format!("{err:#}"));
            }
        }
        if system::running_from_bundle() {
            self.login_status = Some(system::login_item_status());
        }
        cx.notify();
    }
}
