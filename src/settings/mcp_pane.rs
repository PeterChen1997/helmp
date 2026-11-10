//! 设置 › MCP：管理 MCP 服务器（它们的工具会变成启动器命令）。
//!
//! - 列表：每个服务器的状态（运行中 / 已停止 / 出错 + 最近的错误）、工具数、启用开关；
//!   点开看命令、工具、协议版本，「编辑」「重启」「查看日志」「删除」（删除要再点一次确认）。
//!   插件带的服务器只读，随插件启用 / 停用。
//! - 添加：手动填写（命令 / 参数 / 环境变量，或地址 / 请求头）；「从 Claude Code / Codex / Cursor 导入」
//!   （只读那几个文件，列出清单勾选）；「粘贴 JSON」；「精选」一键添加（先列出要运行的完整命令和要填的值）。
//! - 名字像密钥的环境变量 / 请求头（TOKEN、KEY、SECRET、Authorization…）保存时自动挪进钥匙串，
//!   config.json 只记键名；界面上从不回显已存的值。
//!
//! 状态每 2 秒刷新一次（服务器在后台启动、空闲关闭、崩溃）。

use std::collections::BTreeMap;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::config;
use crate::i18n::{tr, trf};
use crate::provider::{Action, Tint};
use crate::providers::mcp::import::{self, Found};
use crate::providers::mcp::manager::{self, Status};
use crate::providers::mcp::servers::{self, Origin, ServerConfig, Spec, Transport};
use crate::providers::mcp::{catalog, schema};
use crate::style::{self, Palette};

use super::widgets;

const DOCS_EN: &str = "https://github.com/PeterChen1997/helmp/blob/HEAD/docs/mcp-client.md";
const DOCS_ZH: &str = "https://github.com/PeterChen1997/helmp/blob/HEAD/docs/mcp-client.zh-CN.md";

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Kind {
    #[default]
    Stdio,
    Http,
}

/// 手动添加 / 编辑的表单。
struct EditForm {
    /// 编辑前的名字；None = 新增。
    original: Option<String>,
    kind: Kind,
    resident: bool,
    name: Entity<InputState>,
    command: Entity<InputState>,
    args: Entity<InputState>,
    cwd: Entity<InputState>,
    url: Entity<InputState>,
    /// 环境变量（KEY=VALUE）或请求头（Name: value），每行一个。
    pairs: Entity<TextareaState>,
    /// 已存进钥匙串的键（编辑时显示，可清除）。
    stored: Vec<String>,
    cleared: Vec<String>,
}

/// 正在展开的「添加」方式。
enum Adding {
    None,
    Manual,
    Import {
        project: Entity<InputState>,
        found: Vec<(Found, bool)>,
        scanned: bool,
    },
    Paste(Entity<TextareaState>),
    Catalog {
        id: &'static str,
        value: Entity<InputState>,
    },
}

pub struct McpPane {
    selected: Option<String>,
    adding: Adding,
    form: Option<EditForm>,
    confirm_delete: Option<String>,
    status: Option<Result<String, String>>,
    /// 正在后台重启 / 读工具的服务器。
    busy: Vec<String>,
}

pub fn new(window: &mut Window, cx: &mut App) -> AnyView {
    let _ = window;
    let pane = cx.new(|_| McpPane {
        selected: None,
        adding: Adding::None,
        form: None,
        confirm_delete: None,
        status: None,
        busy: Vec::new(),
    });
    // 状态在后台变（启动、空闲关闭、崩溃）：定时重绘。
    let weak = pane.downgrade();
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            if weak.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        }
    })
    .detach();
    pane.into()
}

fn line_input(window: &mut Window, cx: &mut App, placeholder: &str, value: &str) -> Entity<InputState> {
    let (placeholder, value) = (placeholder.to_string(), value.to_string());
    cx.new(|cx| {
        let mut state = InputState::new(window, cx).placeholder(placeholder);
        state.set_value(value, window, cx);
        state
    })
}

fn area_input(window: &mut Window, cx: &mut App, placeholder: &str, value: &str) -> Entity<TextareaState> {
    let (placeholder, value) = (placeholder.to_string(), value.to_string());
    cx.new(|cx| {
        let mut state = TextareaState::new(window, cx).placeholder(placeholder);
        state.set_value(value, window, cx);
        state
    })
}

/// 带边框的单行输入框。
fn field(state: &Entity<InputState>, width: f32, pal: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .h(px(26.))
        .w(px(width))
        .px(px(6.))
        .rounded(px(6.))
        .bg(pal.control)
        .border_1()
        .border_color(pal.separator)
        .text_size(px(12.))
        .child(Input::new(state).appearance(false))
}

fn area(state: &Entity<TextareaState>, height: f32, pal: &Palette) -> Div {
    div()
        .h(px(height))
        .w_full()
        .px(px(2.))
        .rounded(px(6.))
        .bg(pal.control)
        .border_1()
        .border_color(pal.separator)
        .text_size(px(12.))
        .child(Textarea::new(state).appearance(false).h_full())
}

fn labeled(label: &str, control: impl IntoElement, pal: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(div().w(px(110.)).flex_none().child(widgets::help_text(label.to_string(), pal)))
        .child(control)
}

fn card(pal: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .mb(px(14.))
        .p(px(14.))
        .rounded(px(10.))
        .bg(pal.group)
        .border_1()
        .border_color(pal.separator)
}

fn code(text: String, pal: &Palette) -> Div {
    div()
        .px(px(8.))
        .py(px(5.))
        .rounded(px(6.))
        .bg(pal.key_bg)
        .text_size(px(12.))
        .font_family("Menlo")
        .text_color(pal.fg)
        .child(text)
}

impl McpPane {
    fn report(&mut self, result: anyhow::Result<String>, cx: &mut Context<Self>) {
        self.status = Some(result.map_err(|err| format!("{err:#}")));
        cx.notify();
    }

    // ------------------------------------------------------------ 编辑表单

    fn open_form(&mut self, original: Option<&ServerConfig>, window: &mut Window, cx: &mut Context<Self>) {
        let (kind, command, args, cwd, url, pairs) = match original.map(|s| &s.transport) {
            Some(Transport::Stdio { command, args, env, cwd }) => (
                Kind::Stdio,
                command.clone(),
                args.iter().map(|a| servers::shell_quote(a)).collect::<Vec<_>>().join(" "),
                cwd.clone().unwrap_or_default(),
                String::new(),
                servers::format_pairs(env, "="),
            ),
            Some(Transport::Http { url, headers }) => (
                Kind::Http,
                String::new(),
                String::new(),
                String::new(),
                url.clone(),
                servers::format_pairs(headers, ": "),
            ),
            None => Default::default(),
        };
        self.form = Some(EditForm {
            original: original.map(|s| s.name.clone()),
            kind,
            resident: original.is_some_and(|s| s.resident),
            name: line_input(window, cx, tr("如 github", "e.g. github"), original.map_or("", |s| s.name.as_str())),
            command: line_input(window, cx, "npx", &command),
            args: line_input(window, cx, "-y @modelcontextprotocol/server-memory", &args),
            cwd: line_input(window, cx, tr("可选，缺省为主目录", "Optional; defaults to your home folder"), &cwd),
            url: line_input(window, cx, "https://example.com/mcp", &url),
            pairs: area_input(window, cx, &pairs_placeholder(kind), &pairs),
            stored: original.map(|s| s.secrets.clone()).unwrap_or_default(),
            cleared: Vec::new(),
        });
        self.status = None;
        cx.notify();
    }

    fn save_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.form else { return };
        let read = |state: &Entity<InputState>| state.read(cx).value().trim().to_string();
        let pairs_text = form.pairs.read(cx).value().to_string();
        let built: Result<ServerConfig, String> = (|| {
            let transport = match form.kind {
                Kind::Stdio => Transport::Stdio {
                    command: read(&form.command),
                    args: servers::split_args(&read(&form.args))?,
                    env: servers::parse_pairs(&pairs_text, '=')?,
                    cwd: Some(read(&form.cwd)).filter(|cwd| !cwd.is_empty()),
                },
                Kind::Http => Transport::Http {
                    url: read(&form.url),
                    headers: servers::parse_pairs(&pairs_text, ':')?,
                },
            };
            let previous = form
                .original
                .as_deref()
                .and_then(|name| config::get().mcp.servers.iter().find(|s| s.name == name).cloned());
            Ok(ServerConfig {
                name: read(&form.name),
                enabled: previous.as_ref().is_none_or(|p| p.enabled),
                resident: form.resident,
                transport,
                secrets: form.stored.clone(),
                timeout_secs: previous.and_then(|p| p.timeout_secs),
            })
        })();
        let server = match built {
            Ok(server) => server,
            Err(err) => {
                self.status = Some(Err(err));
                cx.notify();
                return;
            }
        };
        let original = form.original.clone();
        let cleared = form.cleared.clone();
        match servers::upsert(original.as_deref(), server, &cleared) {
            Ok(name) => {
                self.form = None;
                self.adding = Adding::None;
                self.selected = Some(name.clone());
                self.report(Ok(trf!("已保存「{name}」", "Saved “{name}”")), cx);
            }
            Err(err) => self.report(Err(err), cx),
        }
    }

    fn render_form(&self, pal: &Palette, cx: &mut Context<Self>) -> Div {
        let Some(form) = &self.form else { return div() };
        let title = match &form.original {
            Some(name) => trf!("编辑「{name}」", "Edit “{name}”"),
            None => tr("添加服务器", "Add Server").to_string(),
        };
        let kind_choice = |kind: Kind, label: &str| -> widgets::MenuChoice {
            let this = cx.entity().downgrade();
            (
                label.to_string(),
                form.kind == kind,
                std::rc::Rc::new(move |window: &mut Window, cx: &mut App| {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(form) = &mut this.form {
                            form.kind = kind;
                            form.pairs.update(cx, |state, cx| {
                                state.set_placeholder(pairs_placeholder(kind), window, cx)
                            });
                        }
                        cx.notify();
                    });
                }),
            )
        };
        let kind_label = match form.kind {
            Kind::Stdio => tr("本地命令（stdio）", "Local command (stdio)"),
            Kind::Http => tr("远程地址（HTTP）", "Remote URL (HTTP)"),
        };
        let mut body = card(pal)
            .child(widgets::label(title, pal))
            .child(labeled(tr("名字", "Name"), field(&form.name, 260., pal), pal))
            .child(labeled(
                tr("类型", "Type"),
                widgets::popup(
                    "mcp-form-kind",
                    kind_label,
                    vec![
                        kind_choice(Kind::Stdio, tr("本地命令（stdio）", "Local command (stdio)")),
                        kind_choice(Kind::Http, tr("远程地址（HTTP）", "Remote URL (HTTP)")),
                    ],
                ),
                pal,
            ));
        body = match form.kind {
            Kind::Stdio => body
                .child(labeled(tr("命令", "Command"), field(&form.command, 260., pal), pal))
                .child(labeled(tr("参数", "Arguments"), field(&form.args, 380., pal), pal))
                .child(labeled(tr("工作目录", "Working folder"), field(&form.cwd, 380., pal), pal))
                .child(widgets::help_text(
                    tr("环境变量，每行一个 KEY=VALUE：", "Environment, one KEY=VALUE per line:"),
                    pal,
                )),
            Kind::Http => body
                .child(labeled(tr("地址", "URL"), field(&form.url, 380., pal), pal))
                .child(widgets::help_text(
                    tr("请求头，每行一个 Name: value：", "Headers, one Name: value per line:"),
                    pal,
                )),
        };
        body = body.child(area(&form.pairs, 72., pal)).child(widgets::help_text(
            tr(
                "名字含 TOKEN / KEY / SECRET / PASSWORD / AUTH 的值（以及 Authorization 请求头）保存时自动存进钥匙串，不写进配置文件。",
                "Values whose names contain TOKEN / KEY / SECRET / PASSWORD / AUTH (and the Authorization header) go to the Keychain, never the config file.",
            ),
            pal,
        ));
        let shown: Vec<&String> = form
            .stored
            .iter()
            .filter(|key| !form.cleared.contains(key))
            .collect();
        for key in shown {
            let key_for_clear = key.clone();
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(widgets::help_text(
                        trf!("{key}：已存入钥匙串（在上面重新填写可替换）", "{key}: saved in the Keychain (enter it above to replace)"),
                        pal,
                    ))
                    .child(widgets::danger_button(
                        SharedString::from(format!("mcp-clear-{key}")),
                        tr("清除", "Clear"),
                        false,
                        pal,
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            if let Some(form) = &mut this.form {
                                form.cleared.push(key_for_clear.clone());
                            }
                            cx.notify();
                        }),
                    )),
            );
        }
        let resident = form.resident;
        body.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(widgets::switch(
                    "mcp-form-resident",
                    resident,
                    tr("常驻", "Keep running"),
                    pal,
                    cx.listener(|this, on: &bool, _, cx| {
                        if let Some(form) = &mut this.form {
                            form.resident = *on;
                        }
                        cx.notify();
                    }),
                ))
                .child(widgets::help_text(
                    tr(
                        "常驻：打开面板时就启动、不因空闲停止（第一次调用更快，但一直占内存）",
                        "Keep running: start with the panel and never stop for idleness (faster first call, uses memory)",
                    ),
                    pal,
                )),
        )
        .child(
            div()
                .flex()
                .gap(px(8.))
                .child(widgets::push_button(
                    "mcp-form-save",
                    tr("保存", "Save"),
                    false,
                    cx.listener(|this, _: &ClickEvent, _, cx| this.save_form(cx)),
                ))
                .child(widgets::push_button(
                    "mcp-form-cancel",
                    tr("取消", "Cancel"),
                    false,
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.form = None;
                        if matches!(this.adding, Adding::Manual) {
                            this.adding = Adding::None;
                        }
                        cx.notify();
                    }),
                )),
        )
    }

    // ------------------------------------------------------------ 添加：导入 / 粘贴 / 精选

    fn scan(&mut self, cx: &mut Context<Self>) {
        let Adding::Import { project, .. } = &self.adding else { return };
        let project = project.read(cx).value().trim().to_string();
        let project = (!project.is_empty()).then(|| expand(&project));
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let found = cx
                .background_spawn(async move { import::scan_all(project.as_deref().map(std::path::Path::new)) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let existing: Vec<String> = config::get().mcp.servers.iter().map(|s| s.name.clone()).collect();
                if let Adding::Import { found: list, scanned, .. } = &mut this.adding {
                    let mut seen = std::collections::BTreeSet::new();
                    *list = found
                        .into_iter()
                        .map(|found| {
                            let pick = found.server.is_ok()
                                && !existing.contains(&found.name)
                                && seen.insert(found.name.clone());
                            (found, pick)
                        })
                        .collect();
                    *scanned = true;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn import_picked(&mut self, cx: &mut Context<Self>) {
        let Adding::Import { found, .. } = &self.adding else { return };
        let picked: Vec<Found> = found.iter().filter(|(_, on)| *on).map(|(f, _)| f.clone()).collect();
        let (mut added, mut problems) = (Vec::new(), Vec::new());
        for found in picked {
            match found.server.clone().map_err(anyhow::Error::msg).and_then(|s| servers::upsert(None, s, &[])) {
                Ok(name) => {
                    added.push(name);
                    if let Some(note) = found.note {
                        problems.push(format!("{}: {note}", found.name));
                    }
                }
                Err(err) => problems.push(format!("{}: {err:#}", found.name)),
            }
        }
        self.adding = Adding::None;
        let mut text = trf!("已导入 {} 个服务器", "Imported {} servers", added.len());
        if !problems.is_empty() {
            text.push_str(tr("。注意：", ". Note: "));
            text.push_str(&problems.join(tr("；", "; ")));
        }
        self.report(Ok(text), cx);
    }

    fn add_pasted(&mut self, cx: &mut Context<Self>) {
        let Adding::Paste(state) = &self.adding else { return };
        let text = state.read(cx).value().to_string();
        match import::parse_pasted(&text) {
            Err(err) => self.report(Err(anyhow::anyhow!(err)), cx),
            Ok(found) => {
                let mut added = Vec::new();
                let mut problems = Vec::new();
                for found in found {
                    match found.server.map_err(anyhow::Error::msg).and_then(|s| servers::upsert(None, s, &[])) {
                        Ok(name) => added.push(name),
                        Err(err) => problems.push(format!("{}: {err:#}", found.name)),
                    }
                }
                if problems.is_empty() {
                    self.adding = Adding::None;
                    self.report(Ok(trf!("已添加：{}", "Added: {}", added.join(", "))), cx);
                } else {
                    self.report(Err(anyhow::anyhow!(problems.join("; "))), cx);
                }
            }
        }
    }

    fn add_catalog(&mut self, cx: &mut Context<Self>) {
        let Adding::Catalog { id, value } = &self.adding else { return };
        let Some(entry) = catalog::find(id) else { return };
        let mut values = BTreeMap::new();
        if let Some(need) = entry.needs.first() {
            values.insert(need.key.to_string(), value.read(cx).value().trim().to_string());
        }
        let result = entry
            .server(&values)
            .map_err(|missing| anyhow::anyhow!(trf!("还没填：{missing}", "Missing: {missing}")))
            .and_then(|server| servers::upsert(None, server, &[]));
        match result {
            Ok(name) => {
                self.adding = Adding::None;
                self.selected = Some(name.clone());
                self.report(
                    Ok(trf!(
                        "已添加「{name}」：第一次用到时启动（npx / uvx 第一次运行要下载，可能要等一会儿）",
                        "Added “{name}”. It starts on first use (npx / uvx download on the first run, which can take a moment)."
                    )),
                    cx,
                );
            }
            Err(err) => self.report(Err(err), cx),
        }
    }

    fn toolbar(&self, pal: &Palette, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .mb(px(14.))
            .child(widgets::push_button(
                "mcp-add-manual",
                tr("添加服务器", "Add Server"),
                false,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.adding = Adding::Manual;
                    this.open_form(None, window, cx);
                }),
            ))
            .child(widgets::push_button(
                "mcp-add-import",
                tr("从 Claude Code / Codex / Cursor 导入", "Import from Claude Code / Codex / Cursor"),
                false,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.form = None;
                    this.adding = Adding::Import {
                        project: line_input(window, cx, tr("可选：项目目录（读它的 .mcp.json）", "Optional: a project folder (reads its .mcp.json)"), ""),
                        found: Vec::new(),
                        scanned: false,
                    };
                    this.scan(cx);
                }),
            ))
            .child(widgets::push_button(
                "mcp-add-paste",
                tr("粘贴 JSON", "Paste JSON"),
                false,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.form = None;
                    this.adding = Adding::Paste(area_input(
                        window,
                        cx,
                        "{\"mcpServers\": {\"name\": {\"command\": \"npx\", \"args\": [\"-y\", \"…\"]}}}",
                        "",
                    ));
                    cx.notify();
                }),
            ))
            .child(widgets::push_button(
                "mcp-docs",
                tr("文档", "Docs"),
                false,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    let url = if crate::i18n::is_en() { DOCS_EN } else { DOCS_ZH };
                    if let Err(err) = Action::open(url).run() {
                        this.report(Err(err), cx);
                    }
                }),
            ))
            .when_some(self.status.clone(), |el, status| match status {
                Ok(text) => el.child(widgets::help_text(text, pal)),
                Err(text) => el.child(widgets::error_text(text, pal)),
            })
    }

    fn render_adding(&self, pal: &Palette, cx: &mut Context<Self>) -> Option<Div> {
        match &self.adding {
            Adding::None | Adding::Catalog { .. } => None,
            Adding::Manual => Some(self.render_form(pal, cx)),
            Adding::Paste(state) => Some(
                card(pal)
                    .child(widgets::label(tr("粘贴 JSON", "Paste JSON"), pal))
                    .child(widgets::help_text(
                        tr(
                            "支持各家文档里常见的 {\"mcpServers\": {…}}，也可以直接 {\"名字\": {…}}。",
                            "Accepts the common {\"mcpServers\": {…}} from server READMEs, or {\"name\": {…}}.",
                        ),
                        pal,
                    ))
                    .child(area(state, 120., pal))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(widgets::push_button(
                                "mcp-paste-add",
                                tr("添加", "Add"),
                                false,
                                cx.listener(|this, _: &ClickEvent, _, cx| this.add_pasted(cx)),
                            ))
                            .child(cancel_button("mcp-paste-cancel", cx)),
                    ),
            ),
            Adding::Import { project, found, scanned } => {
                let mut body = card(pal)
                    .child(widgets::label(tr("从其他客户端导入", "Import from other clients"), pal))
                    .child(widgets::help_text(
                        tr(
                            "只读取 ~/.claude.json、~/.codex/config.toml、~/.cursor/mcp.json，不会改动它们。",
                            "Only reads ~/.claude.json, ~/.codex/config.toml and ~/.cursor/mcp.json; never changes them.",
                        ),
                        pal,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(field(project, 320., pal))
                            .child(widgets::push_button(
                                "mcp-import-rescan",
                                tr("重新扫描", "Scan Again"),
                                false,
                                cx.listener(|this, _: &ClickEvent, _, cx| this.scan(cx)),
                            )),
                    );
                if !scanned {
                    body = body.child(widgets::help_text(tr("正在扫描…", "Scanning…"), pal));
                } else if found.is_empty() {
                    body = body.child(widgets::help_text(
                        tr("没有找到 MCP 服务器配置。", "No MCP server configurations found."),
                        pal,
                    ));
                }
                let existing: Vec<String> = config::get().mcp.servers.iter().map(|s| s.name.clone()).collect();
                for (index, (item, on)) in found.iter().enumerate() {
                    let (detail, can) = match &item.server {
                        Err(reason) => (reason.clone(), false),
                        Ok(_) if existing.contains(&item.name) => (tr("已存在同名服务器", "A server with this name exists").into(), false),
                        Ok(server) => (
                            format!("{} · {}", server.transport.summary(), super::pages::display_path(&item.file)),
                            true,
                        ),
                    };
                    let mut line = div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(widgets::label(format!("{} · {}", item.name, item.source.label()), pal))
                                .child(widgets::help_text(detail, pal).truncate()),
                        );
                    if can {
                        line = line.child(widgets::switch(
                            SharedString::from(format!("mcp-import-{index}")),
                            *on,
                            &trf!("导入 {}", "Import {}", item.name),
                            pal,
                            cx.listener(move |this, value: &bool, _, cx| {
                                if let Adding::Import { found, .. } = &mut this.adding
                                    && let Some(entry) = found.get_mut(index)
                                {
                                    entry.1 = *value;
                                }
                                cx.notify();
                            }),
                        ));
                    }
                    body = body.child(line);
                }
                let count = found.iter().filter(|(_, on)| *on).count();
                Some(
                    body.child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(widgets::push_button(
                                "mcp-import-go",
                                trf!("导入所选（{count}）", "Import Selected ({count})"),
                                count == 0,
                                cx.listener(|this, _: &ClickEvent, _, cx| this.import_picked(cx)),
                            ))
                            .child(cancel_button("mcp-import-cancel", cx)),
                    ),
                )
            }
        }
    }

    fn catalog_group(&self, pal: &Palette, cx: &mut Context<Self>) -> Div {
        let existing: Vec<String> = config::get().mcp.servers.iter().map(|s| s.name.clone()).collect();
        let rows = catalog::ENTRIES
            .iter()
            .map(|entry| {
                let added = existing.iter().any(|name| name == entry.id);
                let open = matches!(&self.adding, Adding::Catalog { id, .. } if *id == entry.id);
                let id = entry.id;
                let line = widgets::row(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(widgets::tinted_icon("plug", Tint::Teal, px(24.)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .child(widgets::label(entry.title, pal))
                                .child(widgets::help_text(entry.summary(), pal)),
                        ),
                    widgets::push_button(
                        SharedString::from(format!("mcp-catalog-{id}")),
                        if added { tr("已添加", "Added") } else { tr("添加", "Add") },
                        added,
                        cx.listener(move |this, _: &ClickEvent, window, cx| {
                            let placeholder = catalog::find(id)
                                .and_then(|e| e.needs.first())
                                .map(|need| tr(need.label_zh, need.label_en))
                                .unwrap_or_default();
                            this.form = None;
                            this.adding = Adding::Catalog {
                                id,
                                value: line_input(window, cx, placeholder, ""),
                            };
                            cx.notify();
                        }),
                    ),
                );
                let mut column = div().flex().flex_col().child(line);
                if open && let Adding::Catalog { value, .. } = &self.adding {
                    column = column.child(self.catalog_card(entry, value, pal, cx));
                }
                column.into_any_element()
            })
            .collect();
        widgets::group(rows, pal)
    }

    fn catalog_card(
        &self,
        entry: &catalog::Entry,
        value: &Entity<InputState>,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let typed = value.read(cx).value().trim().to_string();
        let shown = if entry.needs.first().is_some_and(|need| need.key == "arg") {
            typed.clone()
        } else {
            String::new()
        };
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .pl(px(48.))
            .pr(px(14.))
            .pb(px(12.))
            .child(widgets::help_text(
                if entry.url.is_some() {
                    tr("将连接：", "Will connect to:")
                } else {
                    tr("将运行（以你的用户身份）：", "Will run (as you):")
                },
                pal,
            ))
            .child(code(entry.command_line(&shown), pal));
        if !entry.requires.is_empty() {
            body = body.child(widgets::help_text(
                trf!("需要先安装：{}", "Requires: {}", entry.requires),
                pal,
            ));
        }
        for need in entry.needs {
            let label = tr(need.label_zh, need.label_en);
            let label = if need.required { format!("{label} *") } else { label.to_string() };
            body = body.child(labeled(&label, field(value, 320., pal), pal));
            if need.key.starts_with("env:") || need.key.starts_with("header:") {
                body = body.child(widgets::help_text(
                    tr("会存进钥匙串。", "Saved to the Keychain."),
                    pal,
                ));
            }
        }
        let homepage = entry.homepage;
        body.child(
            div()
                .flex()
                .gap(px(8.))
                .child(widgets::push_button(
                    "mcp-catalog-confirm",
                    tr("确认添加", "Add"),
                    false,
                    cx.listener(|this, _: &ClickEvent, _, cx| this.add_catalog(cx)),
                ))
                .child(widgets::push_button(
                    "mcp-catalog-home",
                    tr("项目主页", "Homepage"),
                    false,
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Err(err) = Action::open(homepage).run() {
                            this.report(Err(err), cx);
                        }
                    }),
                ))
                .child(cancel_button("mcp-catalog-cancel", cx)),
        )
    }

    // ------------------------------------------------------------ 服务器列表

    fn restart(&mut self, spec: Spec, cx: &mut Context<Self>) {
        let key = spec.key.clone();
        if self.busy.contains(&key) {
            return;
        }
        self.busy.push(key.clone());
        cx.notify();
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let result = cx
                .background_spawn(async move {
                    let manager = manager::global();
                    let result = manager.restart(&spec);
                    // 非常驻的读完工具就停，和懒启动一致。
                    if result.is_ok() && !spec.resident {
                        manager.stop(&spec.key);
                    }
                    result
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy.retain(|k| *k != key);
                let message = match result {
                    Ok(()) => Ok(trf!("「{key}」已重启并读取了工具列表", "Restarted “{key}” and read its tools")),
                    Err(err) if manager::is_dry_run_blocked(&err) => Ok(format!("[dry-run] mcp-start {key}")),
                    Err(err) => Err(err),
                };
                this.report(message, cx);
            });
        })
        .detach();
    }

    fn server_row(&self, spec: &Spec, pal: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let manager = manager::global();
        let info = manager.info(&spec.key);
        let tools = manager.cached_tools(spec);
        let busy = self.busy.contains(&spec.key);
        let open = self.selected.as_deref() == Some(spec.key.as_str());
        let user = matches!(spec.origin, Origin::User);
        let (tag, color) = if !spec.enabled {
            (tr("已停用", "Disabled").to_string(), pal.muted)
        } else if busy {
            (tr("启动中", "Starting").to_string(), pal.accent)
        } else {
            let color = match info.status {
                Status::Running | Status::Starting => pal.accent,
                Status::Error(_) => pal.danger,
                Status::Stopped => pal.muted,
            };
            (info.status.label().to_string(), color)
        };
        let count = match &tools {
            Some(tools) => trf!("{} 个工具", "{} tools", tools.len()),
            None => tr("还没读取工具", "Tools not read yet").to_string(),
        };
        // 我的服务器不用再写来源；插件带的写明是哪个插件。
        let meta = if user {
            format!("{count} · {}", spec.transport.summary())
        } else {
            format!("{count} · {} · {}", spec.transport.summary(), spec.origin_label())
        };
        let control: AnyElement = if user {
            let name = spec.key.clone();
            widgets::switch(
                SharedString::from(format!("mcp-enabled-{}", spec.key)),
                spec.enabled,
                &trf!("启用{}", "Enable {}", spec.name),
                pal,
                cx.listener(move |this, on: &bool, _, cx| {
                    cx.stop_propagation();
                    let result = servers::set_flags(&name, Some(*on), None).map(|()| String::new());
                    if let Err(err) = result {
                        this.report(Err(err), cx);
                    }
                    cx.notify();
                }),
            )
            .into_any_element()
        } else {
            widgets::help_text(tr("随插件", "With plugin"), pal).into_any_element()
        };
        let key = spec.key.clone();
        let icon = if matches!(info.status, Status::Error(_)) {
            widgets::tinted_icon("triangle-alert", Tint::Red, px(24.))
        } else {
            widgets::tinted_icon("plug", Tint::Teal, px(24.))
        };
        let line = div()
            .id(SharedString::from(format!("mcp-row-{}", spec.key)))
            .flex()
            .items_center()
            .gap(px(12.))
            .min_h(px(48.))
            .px(px(14.))
            .py(px(6.))
            .cursor_pointer()
            .when(open, |el| el.bg(pal.accent.opacity(0.08)))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.selected = (this.selected.as_deref() != Some(key.as_str())).then(|| key.clone());
                this.confirm_delete = None;
                cx.notify();
            }))
            .child(icon)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(widgets::label(spec.name.clone(), pal).truncate())
                            .child(div().text_size(px(11.)).text_color(color).child(tag)),
                    )
                    .child(widgets::help_text(meta, pal).truncate()),
            )
            .child(div().flex_none().child(control));
        let mut column = div().flex().flex_col().child(line);
        if open {
            column = column.child(self.details(spec, &info, tools.as_deref(), pal, cx));
        }
        column.into_any_element()
    }

    fn details(
        &self,
        spec: &Spec,
        info: &manager::Info,
        tools: Option<&[schema::Tool]>,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .pl(px(50.))
            .pr(px(14.))
            .pb(px(12.))
            .child(code(spec.transport.summary(), pal));
        if let Some(error) = &info.last_error {
            body = body.child(widgets::error_text(trf!("最近的错误：{error}", "Last error: {error}"), pal));
        }
        if let Some(protocol) = &info.protocol {
            body = body.child(widgets::help_text(trf!("协议版本 {protocol}", "Protocol {protocol}"), pal));
        }
        if let Some(tools) = tools {
            let names: Vec<String> = tools
                .iter()
                .map(|tool| {
                    if schema::needs_confirmation(&tool.annotations) {
                        format!("{} ⚠︎", tool.display_title())
                    } else {
                        tool.display_title().to_string()
                    }
                })
                .collect();
            body = body.child(widgets::help_text(
                trf!("工具：{}", "Tools: {}", names.join(tr("、", ", "))),
                pal,
            ));
        }
        if !spec.secrets.is_empty() {
            body = body.child(widgets::help_text(
                trf!("钥匙串里的密钥：{}", "Keys in the Keychain: {}", spec.secrets.join(", ")),
                pal,
            ));
        }
        let mut buttons = div().flex().flex_wrap().gap(px(8.)).mt(px(4.));
        let user_server = config::get().mcp.servers.iter().find(|s| s.name == spec.key).cloned();
        if let Some(server) = user_server.clone() {
            buttons = buttons.child(widgets::push_button(
                SharedString::from(format!("mcp-edit-{}", spec.key)),
                tr("编辑", "Edit"),
                false,
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.adding = Adding::None;
                    this.open_form(Some(&server), window, cx);
                }),
            ));
        }
        let restart_spec = spec.clone();
        buttons = buttons.child(widgets::push_button(
            SharedString::from(format!("mcp-restart-{}", spec.key)),
            tr("重启", "Restart"),
            self.busy.contains(&spec.key) || !spec.enabled,
            cx.listener(move |this, _: &ClickEvent, _, cx| this.restart(restart_spec.clone(), cx)),
        ));
        let key = spec.key.clone();
        buttons = buttons.child(widgets::push_button(
            SharedString::from(format!("mcp-log-{}", spec.key)),
            tr("查看日志", "View Log"),
            false,
            cx.listener(move |this, _: &ClickEvent, _, cx| {
                let result = manager::log_path(&key).and_then(|path| {
                    anyhow::ensure!(
                        path.is_file(),
                        tr("还没有日志：服务器还没启动过", "No log yet: the server hasn't started")
                    );
                    Action::open(path.display().to_string()).run().map(|_| String::new())
                });
                if let Err(err) = result {
                    this.report(Err(err), cx);
                }
            }),
        ));
        if user_server.is_some() {
            let name = spec.key.clone();
            let confirming = self.confirm_delete.as_deref() == Some(spec.key.as_str());
            buttons = buttons.child(widgets::danger_button(
                SharedString::from(format!("mcp-delete-{}", spec.key)),
                if confirming {
                    tr("再点一次确认删除", "Click again to delete")
                } else {
                    tr("删除", "Delete")
                },
                false,
                pal,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if this.confirm_delete.as_deref() != Some(name.as_str()) {
                        this.confirm_delete = Some(name.clone());
                        cx.notify();
                        return;
                    }
                    this.confirm_delete = None;
                    this.selected = None;
                    let result = servers::remove(&name)
                        .map(|()| trf!("已删除「{name}」（钥匙串里的密钥一并删除）", "Deleted “{name}” (and its Keychain entries)"));
                    this.report(result, cx);
                }),
            ));
            let resident = spec.resident;
            let name = spec.key.clone();
            buttons = buttons.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(widgets::switch(
                        SharedString::from(format!("mcp-resident-{}", spec.key)),
                        resident,
                        tr("常驻", "Keep running"),
                        pal,
                        cx.listener(move |this, on: &bool, _, cx| {
                            if let Err(err) = servers::set_flags(&name, None, Some(*on)) {
                                this.report(Err(err), cx);
                            }
                            cx.notify();
                        }),
                    ))
                    .child(widgets::help_text(tr("常驻", "Keep running"), pal)),
            );
        }
        body.child(buttons)
    }
}

fn cancel_button(id: &'static str, cx: &mut Context<McpPane>) -> gpui_kit::component::button::Button {
    widgets::push_button(
        id,
        tr("取消", "Cancel"),
        false,
        cx.listener(|this, _: &ClickEvent, _, cx| {
            this.adding = Adding::None;
            cx.notify();
        }),
    )
}

fn pairs_placeholder(kind: Kind) -> String {
    match kind {
        Kind::Stdio => "GITHUB_PERSONAL_ACCESS_TOKEN=ghp_…".into(),
        Kind::Http => "Authorization: Bearer …".into(),
    }
}

fn expand(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => crate::paths::home_dir()
            .map(|home| home.join(rest).display().to_string())
            .unwrap_or_else(|_| path.to_string()),
        None => path.to_string(),
    }
}

impl Render for McpPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = style::palette(window);
        let config = config::get();
        let specs = servers::all(&config);
        if self
            .selected
            .as_deref()
            .is_some_and(|key| !specs.iter().any(|spec| spec.key == key))
        {
            self.selected = None;
        }
        let (mine, bundled): (Vec<&Spec>, Vec<&Spec>) =
            specs.iter().partition(|spec| matches!(spec.origin, Origin::User));
        let mut rows: Vec<AnyElement> = mine.iter().map(|spec| self.server_row(spec, &pal, cx)).collect();
        if rows.is_empty() {
            rows.push(
                div()
                    .px(px(14.))
                    .py(px(12.))
                    .child(widgets::help_text(
                        tr(
                            "还没有 MCP 服务器。从 Claude Code / Codex / Cursor 导入、在下面的「精选」里一键添加，或粘贴 JSON。",
                            "No MCP servers yet. Import from Claude Code / Codex / Cursor, add a featured one below, or paste JSON.",
                        ),
                        &pal,
                    ))
                    .into_any_element(),
            );
        }
        let editing_existing = self
            .form
            .as_ref()
            .is_some_and(|form| form.original.is_some());
        let extension_off = !config::extension_enabled(crate::providers::mcp::PROVIDER_ID);
        let mut page = div()
            .flex()
            .flex_col()
            .child(widgets::help_text(
                tr(
                    "MCP 服务器提供的工具会变成命令：在启动器里搜工具名，填参数，回车运行。",
                    "Tools from MCP servers become commands: search a tool's name in the launcher, fill in the form, press Return.",
                ),
                &pal,
            ).mb(px(10.)))
            .child(self.toolbar(&pal, cx))
            .when(extension_off, |el| {
                el.child(div().mb(px(12.)).child(widgets::error_text(
                    tr(
                        "「MCP 工具」扩展已在 设置 › 插件 › 内置功能 里关闭，工具不会出现在搜索里",
                        "The MCP Tools extension is turned off in Settings › Plugins, so no tool shows up in search",
                    ),
                    &pal,
                )))
            });
        if let Some(adding) = self.render_adding(&pal, cx) {
            page = page.child(adding);
        }
        if editing_existing {
            page = page.child(self.render_form(&pal, cx));
        }
        page = page
            .child(widgets::group_header(tr("我的服务器", "My Servers"), &pal))
            .child(widgets::group(rows, &pal));
        if !bundled.is_empty() {
            let rows = bundled.iter().map(|spec| self.server_row(spec, &pal, cx)).collect();
            page = page
                .child(widgets::group_header(tr("插件带的服务器", "From Plugins"), &pal))
                .child(widgets::group(rows, &pal));
        }
        page.child(widgets::group_header(tr("精选", "Featured"), &pal))
            .child(self.catalog_group(&pal, cx))
            .child(div().mb(px(18.)).ml(px(4.)).child(widgets::help_text(
                tr(
                    "服务器第一次用到时才启动，空闲 10 分钟后停止；崩溃后按 2、4、8…60 秒退避重启。日志在数据目录的 mcp/logs 里。",
                    "Servers start on first use and stop after 10 idle minutes; after a crash they restart with a 2, 4, 8…60 s backoff. Logs live in mcp/logs in the data folder.",
                ),
                &pal,
            )))
    }
}
