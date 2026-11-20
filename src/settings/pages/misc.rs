//! Agent、截图、外观、关于（设计稿没有单独的页面，沿用分组表单的样式）。

use gpui_kit::component::Disableable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::display_path;
use crate::config::{self, Appearance};
use crate::providers::plugins::{self, greeting, theme};
use crate::i18n::{tr, trf};
use crate::settings::model::{self, AGENT_SOURCES, APPEARANCE_OPTIONS, OCR_LANGUAGES};
use crate::settings::widgets;
use crate::settings::{SettingsView, sync_component_theme};
use crate::style::{self, Palette};

/// 关于页「联系作者」。
const AUTHOR_NAME: &str = "Peter Chen";
const AUTHOR_BLOG: &str = "https://blog.peterchen97.cn";
const AUTHOR_BLOG_LABEL: &str = "blog.peterchen97.cn";

/// 最近一次「连接」的结果（成功的说明或错误）。
static MCP_CONNECT_STATUS: std::sync::Mutex<Option<Result<String, String>>> =
    std::sync::Mutex::new(None);

impl SettingsView {
    pub(in crate::settings) fn render_agents(
        &mut self,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut rows = Vec::new();
        for (key, name, help_zh, help_en) in AGENT_SOURCES {
            rows.push(
                widgets::row(
                    widgets::label_with_help(name, tr(help_zh, help_en), pal),
                    widgets::switch(
                        key,
                        config::extension_enabled(key),
                        name,
                        pal,
                        cx.listener(move |this, on: &bool, _, cx| {
                            let on = *on;
                            this.save(cx, |c| model::set_extension_enabled(c, key, on));
                        }),
                    ),
                )
                .into_any_element(),
            );
        }
        let plugin_off = !config::extension_enabled("agents");

        div()
            .flex()
            .flex_col()
            .child(
                div().mb(px(16.)).child(widgets::help_text(
                    tr(
                        "Helmp 读取本机 Claude Code 与 Codex 的会话记录，在面板里列出正在运行和等你处理的会话，\
                         回车跳转到对应的终端或 App。只在本机读取，不上传任何内容。",
                        "Helmp reads Claude Code and Codex session history on this Mac and lists sessions that \
                         are running or waiting for you. Press Return to jump to the terminal or app. \
                         Everything stays on this Mac; nothing is uploaded.",
                    ),
                    pal,
                )),
            )
            .child(widgets::group_header(tr("会话来源", "Session Sources"), pal))
            .child(widgets::group(rows, pal))
            .when(plugin_off, |el| {
                el.child(widgets::error_text(
                    tr(
                        "「Agent 会话」已在 设置 › 插件 › 内置功能 里关闭，以上来源都不会显示。",
                        "The Agent Sessions extension is turned off in Settings › Plugins › Built-in Features, so none of these sources are shown.",
                    ),
                    pal,
                ))
            })
            .child(widgets::help_text(
                tr(
                    "跳转到 iTerm 或终端里的会话需要「自动化」权限，首次跳转时系统会询问。",
                    "Jumping to a session in iTerm or Terminal needs Automation permission. macOS asks the first time.",
                ),
                pal,
            ))
            .child(div().mt(px(20.)).child(self.render_mcp_connect(pal, cx)))
            .into_any_element()
    }

    /// 「连接到 coding agent」：把 `helmp mcp` 登记到 Claude Code / Codex / Cursor（见 src/mcp/connect.rs）。
    /// 只在用户点按钮时写 agent 的配置；状态每次渲染现读（配置文件很小，`~/.claude.json` 也只读不写）。
    fn render_mcp_connect(&mut self, pal: &Palette, cx: &mut Context<Self>) -> AnyElement {
        use crate::mcp::connect::{self, Agent, Status};
        let mut rows = Vec::new();
        for agent in Agent::ALL {
            let status = connect::status(agent).unwrap_or(Status::NotConnected);
            let path = agent
                .config_path()
                .map(|path| display_path(&path))
                .unwrap_or_default();
            let button = match status {
                Status::Connected => tr("已连接", "Connected"),
                Status::Outdated(_) => tr("重新连接", "Reconnect"),
                Status::NotConnected => tr("连接", "Connect"),
            };
            rows.push(
                widgets::row(
                    widgets::label_with_help(
                        agent.name(),
                        format!("{} · {path}", status.label()),
                        pal,
                    ),
                    widgets::push_button(
                        SharedString::from(format!("mcp-connect-{}", agent.slug())),
                        button,
                        status == Status::Connected,
                        cx.listener(move |_, _: &ClickEvent, _, cx| {
                            cx.spawn(async move |this, cx: &mut AsyncApp| {
                                let result = cx
                                    .background_spawn(async move { connect::install(agent) })
                                    .await;
                                let message = match result {
                                    Ok(installed) => Ok(connect::describe(agent, &installed)),
                                    Err(err) => Err(format!("{err:#}")),
                                };
                                *MCP_CONNECT_STATUS
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(message);
                                let _ = this.update(cx, |_, cx| cx.notify());
                            })
                            .detach();
                        }),
                    ),
                )
                .into_any_element(),
            );
        }
        let last = MCP_CONNECT_STATUS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        div()
            .flex()
            .flex_col()
            .child(widgets::group_header(tr("连接到 coding agent", "Connect Coding Agents"), pal))
            .child(widgets::group(rows, pal))
            .child(widgets::help_text(
                tr(
                    "连接后，Claude Code / Codex / Cursor 可以按你的需求直接做出 Helmp 插件，也能搜索和使用 Helmp。\
                     有副作用的操作会先在面板里问你；新插件要你在「插件」页启用后才生效；上传到插件中心前一定先问你。",
                    "Once connected, Claude Code, Codex or Cursor can build Helmp plugins for what you ask and search \
                     or use Helmp. Anything with side effects asks you in the panel first; new plugins only work after \
                     you enable them in Plugins; nothing is published to the plugin center without asking you.",
                ),
                pal,
            ))
            .when_some(last, |el, last| match last {
                Ok(text) => el.child(widgets::help_text(text, pal)),
                Err(text) => el.child(widgets::error_text(text, pal)),
            })
            .into_any_element()
    }

    pub(in crate::settings) fn render_screenshot(
        &mut self,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let config = config::get();
        let shot = &config.screenshot;
        let (dir_text, custom) = match &shot.save_dir {
            Some(dir) => (display_path(std::path::Path::new(dir)), true),
            None => match crate::paths::data_dir() {
                Ok(dir) => (display_path(&dir.join("screenshots")), false),
                Err(err) => (
                    trf!(
                        "数据目录不可用：{err:#}",
                        "Data folder is unavailable: {err:#}"
                    ),
                    false,
                ),
            },
        };

        let mut languages = Vec::new();
        for (code, name_zh, name_en) in OCR_LANGUAGES {
            let name = tr(name_zh, name_en);
            let on = shot.ocr_languages.iter().any(|lang| lang == code);
            let last = on && shot.ocr_languages.len() == 1;
            languages.push(
                widgets::row(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(widgets::label(name, pal))
                        .child(widgets::help_text(code, pal)),
                    widgets::switch(
                        SharedString::from(format!("ocr-{code}")),
                        on,
                        name,
                        pal,
                        cx.listener(move |this, on: &bool, _, cx| {
                            let on = *on;
                            this.save(cx, |c| {
                                model::toggle_ocr_language(c, code, on);
                            });
                        }),
                    )
                    // 最后一种语言不能关：Vision 没有语言时会退回默认，与界面不符。
                    .disabled(last)
                    .when(last, |switch| {
                        switch.tooltip(tr(
                            "至少保留一种识别语言",
                            "Keep at least one recognition language",
                        ))
                    }),
                )
                .into_any_element(),
            );
        }

        // 「截图」「截图识别文字」是两条独立的快捷键，并排列出、各自录制；录成对方正在用的组合会被指名拦下。
        let mut rows = Vec::new();
        for shortcut in &self.screenshot_hotkeys {
            let (zh, en) = shortcut.label;
            let help = if shortcut.slot == "screenshot-capture" {
                tr(
                    "冻结屏幕后框选，再复制、保存、钉图或识别文字",
                    "Freeze the screen, select an area, then copy, save, pin or recognize text",
                )
            } else {
                tr(
                    "冻结屏幕后框选，松开鼠标立即识别文字并复制",
                    "Freeze the screen, select an area; its text is recognized and copied when you release the mouse",
                )
            };
            rows.push(
                widgets::row(
                    widgets::label_with_help(
                        trf!("「{}」快捷键", "“{}” shortcut", tr(zh, en)),
                        help,
                        pal,
                    ),
                    crate::settings::hotkey::with_note(&shortcut.recorder, pal, cx),
                )
                .into_any_element(),
            );
        }
        div()
            .flex()
            .flex_col()
            .child(widgets::group(
                rows.into_iter().chain([
                    widgets::row(
                        widgets::label_with_help(
                            tr("截图默认保存位置", "Default folder for saved screenshots"),
                            dir_text,
                            pal,
                        ),
                        div()
                            .flex()
                            .gap(px(8.))
                            .when(custom, |el| {
                                el.child(widgets::push_button(
                                    "shot-dir-reset",
                                    tr("恢复默认", "Restore Default"),
                                    false,
                                    cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.save(cx, |c| c.screenshot.save_dir = None);
                                    }),
                                ))
                            })
                            .child(widgets::push_button(
                                "shot-dir-pick",
                                tr("选择…", "Choose…"),
                                false,
                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.pick_path(
                                        true,
                                        tr("选择", "Choose"),
                                        cx,
                                        |this, dir, cx| {
                                            let dir = dir.to_string_lossy().into_owned();
                                            this.save(cx, |c| c.screenshot.save_dir = Some(dir));
                                        },
                                    );
                                }),
                            )),
                    )
                    .into_any_element(),
                    // 截图历史：只留最近 10 张；关掉时不再记录，并删掉已经记下的。
                    widgets::row(
                        widgets::label_with_help(
                            tr("保留截图历史", "Keep screenshot history"),
                            tr(
                                "记下最近 10 张截图，可在「截图历史」里再钉图、复制、识别文字。关闭后不再记录，并删除已保存的历史。",
                                "Keeps your last 10 screenshots so you can pin, copy or recognize them again from Screenshot History. Turning it off stops recording and deletes the saved history.",
                            ),
                            pal,
                        ),
                        widgets::switch(
                            "shot-history",
                            shot.history,
                            tr("保留截图历史", "Keep screenshot history"),
                            pal,
                            cx.listener(|this, on: &bool, _, cx| {
                                let on = *on;
                                this.save(cx, |c| c.screenshot.history = on);
                                if !on
                                    && let Err(err) = crate::capture::history::dir()
                                        .and_then(|dir| crate::capture::history::clear(&dir))
                                {
                                    eprintln!("helmp: 清空截图历史失败：{err:#}");
                                }
                            }),
                        ),
                    )
                    .into_any_element(),
                ]).collect(),
                pal,
            ))
            .child(widgets::group_header(
                tr("文字识别语言", "Text Recognition Languages"),
                pal,
            ))
            .child(widgets::group(languages, pal).mb(px(6.)))
            .child(div().ml(px(4.)).child(widgets::help_text(
                tr(
                    "按勾选顺序优先识别，至少保留一种。",
                    "Languages are tried in the order you turned them on. Keep at least one.",
                ),
                pal,
            )))
            .into_any_element()
    }

    pub(in crate::settings) fn render_appearance(
        &mut self,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let config = config::get();
        let current = config.appearance;
        let mut tiles = div().flex().gap(px(16.));
        for (appearance, label_zh, label_en) in APPEARANCE_OPTIONS {
            let label = tr(label_zh, label_en);
            let selected = appearance == current;
            let preview = match appearance {
                Appearance::Light => div().bg(style::light().window),
                Appearance::Dark => div().bg(style::dark().window),
                Appearance::System => div()
                    .flex()
                    .child(div().flex_1().h_full().bg(style::light().window))
                    .child(div().flex_1().h_full().bg(style::dark().window)),
            };
            tiles = tiles.child(
                div()
                    .id(SharedString::from(format!("appearance-{appearance:?}")))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.save(cx, |c| c.appearance = appearance);
                        // 启动器与组件主题都读这个设置：同步组件主题并重绘所有窗口。
                        sync_component_theme(window, cx);
                        cx.refresh_windows();
                    }))
                    .child(
                        preview
                            .w(px(120.))
                            .h(px(76.))
                            .rounded(px(10.))
                            .overflow_hidden()
                            .border_2()
                            .border_color(if selected { pal.accent } else { pal.separator }),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(if selected { pal.accent } else { pal.fg })
                            .when(selected, |el| el.font_weight(FontWeight::SEMIBOLD))
                            .child(label),
                    ),
            );
        }

        let themes = theme::available(&config, &plugins::registry::current());
        let today = greeting::local_date(chrono::Utc::now(), &chrono::Local);
        let selected = theme::selected_available(&config, &themes);
        let active = theme::resolve(&config, &themes, today);
        let selected_name = selected
            .as_ref()
            .and_then(|key| themes.iter().find(|theme| theme.key == *key))
            .map_or_else(|| tr("默认", "Default").to_string(), |theme| theme.name.clone());

        let mut page = div().flex().flex_col();
        if config.theme.by_date
            && let Some((dated, until)) = theme::dated_today(&themes, today)
        {
            page = page.child(self.theme_date_banner(
                dated,
                until,
                &selected_name,
                theme::skipped_today(&config, today),
                today,
                pal,
                cx,
            ));
        }
        page = page
            .child(widgets::group_header(tr("明暗", "Light and Dark"), pal))
            .child(widgets::group(
                vec![div().px(px(14.)).py(px(16.)).child(tiles).into_any_element()],
                pal,
            ))
            .child(widgets::group_header(
                tr(
                    "主题 · 只换配色，明暗仍按上面的设置",
                    "Theme · changes colors only; light and dark follow the setting above",
                ),
                pal,
            ));

        let live = active.as_ref().filter(|active| active.dated).map(|active| active.theme.key.clone());
        let mut cards = div().flex().flex_wrap().gap(px(10.)).p(px(10.));
        cards = cards.child(self.theme_card(
            "default".into(),
            tr("默认", "Default").into(),
            tr("Helmp 内置", "Built into Helmp").into(),
            (style::light(), style::dark()),
            (None, None),
            selected.is_none(),
            false,
            None,
            pal,
            cx,
        ));
        for theme in &themes {
            let mut subtitle = theme.plugin_name.clone();
            if let Some(range) = theme.dates.first() {
                subtitle = format!("{subtitle} · {}", range.describe());
            }
            cards = cards.child(self.theme_card(
                theme.key.clone().into(),
                theme.name.clone().into(),
                subtitle.into(),
                (
                    style::themed(style::light(), &theme.light),
                    style::themed(style::dark(), &theme.dark),
                ),
                (theme.image_light.clone(), theme.image_dark.clone()),
                selected.as_deref() == Some(theme.key.as_str()),
                live.as_deref() == Some(theme.key.as_str()),
                Some(theme.key.clone()),
                pal,
                cx,
            ));
        }
        page = page.child(widgets::group(vec![cards.into_any_element()], pal));

        if themes.iter().any(|theme| !theme.dates.is_empty()) {
            page = page.child(widgets::group(
                vec![
                    widgets::row(
                        widgets::label_with_help(
                            tr("按日期自动切换主题", "Switch themes by date"),
                            tr(
                                "到日期自动换上带日期的主题，过后回到你选的主题",
                                "Dated themes switch on by themselves on their days, then hand back to your choice",
                            ),
                            pal,
                        ),
                        widgets::switch(
                            "theme-by-date",
                            config.theme.by_date,
                            tr("按日期自动切换主题", "Switch themes by date"),
                            pal,
                            cx.listener(|this, on: &bool, window, cx| {
                                let on = *on;
                                this.save(cx, |c| c.theme.by_date = on);
                                crate::telemetry::plugin_theme_by_date(on);
                                sync_component_theme(window, cx);
                                cx.refresh_windows();
                            }),
                        ),
                    )
                    .into_any_element(),
                ],
                pal,
            ));
        }
        page.child(widgets::help_text(
            if themes.is_empty() {
                tr(
                    "影响启动器面板、设置窗口与引导窗口。跟随系统时随「系统设置 › 外观」自动切换。安装提供主题的插件后，可以在这里选择。",
                    "Applies to the launcher panel, Settings and the welcome window. Auto follows System Settings › Appearance. Themes from plugins you install show up here.",
                )
            } else {
                tr(
                    "影响启动器面板、设置窗口与引导窗口。主题来自已启用的插件，停用或卸载插件后回到默认配色。",
                    "Applies to the launcher panel, Settings and the welcome window. Themes come from enabled plugins; disabling or removing the plugin returns to the default colors.",
                )
            },
            pal,
        ))
        .into_any_element()
    }

    /// 日期主题今天生效时，页面顶部的说明条：「今天不用」只管今天，点错了可以「今天也用」。
    #[allow(clippy::too_many_arguments)]
    fn theme_date_banner(
        &self,
        dated: &theme::Available,
        until: chrono::NaiveDate,
        selected_name: &str,
        skipped: bool,
        today: chrono::NaiveDate,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let back = until
            .succ_opt()
            .map(|day| day.format("%m-%d").to_string())
            .unwrap_or_default();
        let (title, detail, button) = if skipped {
            (
                trf!("今天不用「{}」", "Not using \u{201c}{}\u{201d} today", dated.name),
                tr("明天起照常按日期换上。", "It switches on again by date tomorrow.").to_string(),
                tr("今天也用", "Use it today"),
            )
        } else {
            (
                trf!(
                    "今天在「{}」的生效日期内，已自动换上",
                    "\u{201c}{}\u{201d} is on today because of its dates",
                    dated.name
                ),
                trf!(
                    "{back} 起恢复你选的「{selected_name}」。",
                    "Your choice (\u{201c}{selected_name}\u{201d}) comes back on {back}."
                ),
                tr("今天不用", "Not today"),
            )
        };
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .mb(px(18.))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(10.))
            .bg(pal.accent.opacity(0.12))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(pal.fg)
                            .child(title),
                    )
                    .child(widgets::help_text(detail, pal)),
            )
            .child(widgets::push_button(
                "theme-skip-today",
                button,
                false,
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    let skip = !skipped;
                    this.save(cx, |c| {
                        c.theme.skip_date = skip.then(|| today.to_string());
                    });
                    crate::telemetry::plugin_theme_skip_today(skip);
                    sync_component_theme(window, cx);
                    cx.refresh_windows();
                }),
            ))
            .into_any_element()
    }

    /// 一张主题卡片：缩略图左浅右深（用主题配色画的迷你面板），下面是名字和来源。
    #[allow(clippy::too_many_arguments)]
    fn theme_card(
        &self,
        id: SharedString,
        name: SharedString,
        subtitle: SharedString,
        (light, dark): (Palette, Palette),
        (image_light, image_dark): (Option<std::path::PathBuf>, Option<std::path::PathBuf>),
        selected: bool,
        live: bool,
        key: Option<String>,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(SharedString::from(format!("theme-{id}")))
            .relative()
            .flex()
            .flex_col()
            .gap(px(4.))
            .w(px(166.))
            .p(px(6.))
            .rounded(px(10.))
            .border_2()
            .border_color(if selected { pal.accent } else { transparent_black() })
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                let key = key.clone();
                crate::telemetry::plugin_theme_selected(key.is_some());
                this.save(cx, |c| c.theme.selected = key);
                sync_component_theme(window, cx);
                cx.refresh_windows();
            }))
            .child(
                div()
                    .flex()
                    .h(px(84.))
                    .rounded(px(7.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(pal.separator)
                    .child(theme_thumb_half(&light, image_light))
                    .child(theme_thumb_half(&dark, image_dark)),
            )
            .child(
                div()
                    .mt(px(2.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(pal.fg)
                    .truncate()
                    .child(name),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .line_height(px(15.))
                    .text_color(pal.muted)
                    .child(subtitle),
            )
            .when(live, |card| {
                card.child(
                    div()
                        .absolute()
                        .top(px(10.))
                        .right(px(10.))
                        .px(px(6.))
                        .rounded(px(5.))
                        .bg(gpui_kit::rgba(0x000000a6))
                        .text_size(px(10.5))
                        .text_color(gpui_kit::rgba(0xffffffff))
                        .child(tr("生效中", "On now")),
                )
            })
            .into_any_element()
    }
    pub(in crate::settings) fn render_about(
        &mut self,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let data_dir = crate::paths::data_dir();
        let dir_row = match &data_dir {
            Ok(dir) => {
                let dir = dir.clone();
                widgets::row(
                    widgets::label_with_help(
                        tr("数据目录", "Data folder"),
                        display_path(&dir),
                        pal,
                    ),
                    widgets::push_button(
                        "reveal-data-dir",
                        tr("在访达中显示", "Show in Finder"),
                        false,
                        move |_, _, cx| {
                            cx.reveal_path(&dir);
                        },
                    ),
                )
            }
            Err(err) => widgets::row(
                widgets::label(tr("数据目录", "Data folder"), pal),
                widgets::error_text(format!("{err:#}"), pal),
            ),
        };
        let _ = cx;
        let author_row = widgets::row(
            widgets::label_with_help(AUTHOR_NAME, AUTHOR_BLOG_LABEL, pal),
            widgets::push_button(
                "open-author-blog",
                tr("访问博客", "Visit Blog"),
                false,
                |_, _, cx| cx.open_url(AUTHOR_BLOG),
            ),
        );

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .py(px(18.))
                    .child(crate::settings::brand_icon(px(96.)))
                    .child(
                        div()
                            .mt(px(6.))
                            .text_size(px(22.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(pal.fg)
                            .child("Helmp"),
                    )
                    .child(widgets::help_text(
                        trf!("版本 {}", "Version {}", env!("CARGO_PKG_VERSION")),
                        pal,
                    ))
                    .child(
                        div()
                            .mt(px(6.))
                            .max_w(px(420.))
                            .text_center()
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .text_color(pal.fg)
                            .child(tr(
                                "读起来像 help me。⌥Space 唤起，一个面板里搜索应用、Agent 会话、\
                                 剪贴板历史、截图识字、翻译和脚本命令。",
                                "Sounds like “help me.” Press ⌥Space to search apps, agent sessions, \
                                 clipboard history, screen text, translations and script commands \
                                 from a single panel.",
                            )),
                    ),
            )
            .child(widgets::group_header(tr("更新", "Updates"), pal))
            .child(widgets::group(update_rows(pal), pal))
            .child(widgets::group_header(tr("联系作者", "Contact the Author"), pal))
            .child(widgets::group(vec![author_row.into_any_element()], pal))
            .child(widgets::group(vec![dir_row.into_any_element()], pal))
            .into_any_element()
    }
}

/// 关于页的「更新」分组：状态行 + 按钮、更新说明、安装后的说明。
/// 状态在 `crate::updates` 里，设置窗口按它的代数轮询重绘（见 `watch_hotkey_registry`）。
/// 更新说明：小标题加粗，列表项前面放圆点并悬挂缩进，行内 Markdown 已在 `updates::notes` 去掉。
fn release_notes(markdown: &str, pal: &Palette) -> Vec<AnyElement> {
    use crate::updates::notes::{Block, blocks};
    blocks(markdown)
        .into_iter()
        .map(|block| match block {
            Block::Heading(title) => widgets::help_text(title, pal)
                .pt(px(6.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(pal.fg)
                .into_any_element(),
            Block::Bullet(text) => div()
                .flex()
                .gap(px(6.))
                .child(widgets::help_text("•", pal).flex_none())
                .child(widgets::help_text(text, pal).flex_1().min_w_0())
                .into_any_element(),
            Block::Paragraph(text) => widgets::help_text(text, pal).into_any_element(),
        })
        .collect()
}

fn update_rows(pal: &Palette) -> Vec<AnyElement> {
    use crate::updates::{self, Busy, install::Location};

    let state = updates::snapshot();
    let busy = state.busy != Busy::Idle;
    let status = state.status_line();
    let failed = state.check_error.is_some() || state.install_error.is_some();
    let available = state.available().filter(|_| state.unsupported_macos().is_none());

    let (help, button) = match available {
        Some(manifest) => {
            let (help, label) = match updates::install::current_location() {
                Location::Replace { .. } => (
                    tr(
                        "下载后校验签名，再退出 Helmp 替换为新版本并重新打开。",
                        "Helmp verifies the download's signature, then quits, replaces itself and reopens.",
                    )
                    .to_string(),
                    tr("下载并安装", "Download and Install"),
                ),
                Location::Manual { reason } => (
                    trf!(
                        "{reason}，所以改为下载 dmg，由你拖进「应用程序」。",
                        "{reason}, so Helmp downloads the dmg for you to drag into Applications."
                    ),
                    tr("下载 dmg", "Download dmg"),
                ),
            };
            let size = manifest
                .asset(updates::manifest::AssetKind::Zip)
                .map(|asset| format!("{:.1} MB", asset.size as f64 / 1_000_000.0));
            let help = match size {
                Some(size) => format!("{help} {}", trf!("大小约 {size}。", "About {size}.")),
                None => help,
            };
            (
                help,
                widgets::push_button("update-install", label, busy, |_, _, _| {
                    updates::install_in_background();
                }),
            )
        }
        None => (
            trf!("当前版本 {}", "Current version {}", env!("CARGO_PKG_VERSION")),
            widgets::push_button(
                "update-check",
                tr("检查更新", "Check for Updates"),
                busy,
                |_, _, _| updates::check_in_background(true),
            ),
        ),
    };
    let status_label = div()
        .flex()
        .flex_col()
        .flex_1()
        .gap(px(2.))
        .child(match failed {
            true => widgets::error_text(status, pal),
            false => widgets::label(status, pal),
        })
        .child(widgets::help_text(help, pal));
    let mut rows = vec![widgets::row(status_label, button).into_any_element()];

    if let Some(notes) = state.available().and_then(|manifest| manifest.notes()) {
        rows.push(
            div()
                .px(px(14.))
                .py(px(10.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(widgets::label(tr("更新说明", "What's New"), pal))
                .children(release_notes(notes, pal))
                .into_any_element(),
        );
    }
    if let Some(notice) = state.notice {
        rows.push(
            div()
                .px(px(14.))
                .py(px(10.))
                .child(widgets::help_text(notice, pal))
                .into_any_element(),
        );
    }
    rows
}

/// 主题缩略图的一半：用这套配色画一个迷你面板（搜索栏、选中行、两行文字），有背景图就铺在搜索栏里。
fn theme_thumb_half(pal: &Palette, image: Option<std::path::PathBuf>) -> Div {
    let line = |color: Hsla, width: f32| div().h(px(3.)).w(relative(width)).rounded(px(2.)).bg(color);
    let base = Hsla { a: 1., ..pal.panel };
    let mut bar = div()
        .relative()
        .flex()
        .items_center()
        .h(px(20.))
        .px(px(7.))
        .border_b_1()
        .border_color(pal.separator);
    if let Some(path) = image {
        bar = bar.child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .overflow_hidden()
                .child(img(path).size_full().object_fit(ObjectFit::Cover))
                .child(div().absolute().top_0().left_0().size_full().bg(Hsla { a: 0.55, ..base })),
        );
    }
    div()
        .flex_1()
        .h_full()
        .flex()
        .flex_col()
        .bg(base)
        .child(bar.child(line(pal.muted, 0.6)))
        .child(
            div()
                .flex()
                .items_center()
                .mx(px(4.))
                .mt(px(6.))
                .h(px(14.))
                .px(px(4.))
                .rounded(px(3.))
                .bg(pal.selection)
                .child(line(pal.selection_fg, 0.55)),
        )
        .child(div().flex().flex_col().gap(px(3.)).mx(px(8.)).mt(px(6.)).child(line(pal.fg, 0.5)).child(line(pal.muted, 0.7)))
        .child(div().flex().flex_col().gap(px(3.)).mx(px(8.)).mt(px(6.)).child(line(pal.fg, 0.4)).child(line(pal.muted, 0.6)))
}
