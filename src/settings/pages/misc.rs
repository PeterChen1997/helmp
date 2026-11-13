//! Agent、截图、外观、关于（设计稿没有单独的页面，沿用分组表单的样式）。

use gpui_kit::component::Disableable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::display_path;
use crate::config::{self, Appearance};
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
                        "「Agent 会话」插件已在「扩展」页关闭，以上来源都不会显示。",
                        "The Agent Sessions extension is turned off in Extensions, so none of these sources are shown.",
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
        let current = config::get().appearance;
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
        div()
            .flex()
            .flex_col()
            .child(widgets::group(
                vec![
                    div()
                        .px(px(14.))
                        .py(px(16.))
                        .child(tiles)
                        .into_any_element(),
                ],
                pal,
            ))
            .child(widgets::help_text(
                tr(
                    "影响启动器面板、设置窗口与引导窗口。跟随系统时随「系统设置 › 外观」自动切换。",
                    "Applies to the launcher panel, Settings and the welcome window. Auto follows System Settings › Appearance.",
                ),
                pal,
            ))
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
