//! 匿名使用统计（Umami）与崩溃 / 错误报告（Sentry）。
//!
//! - 设置 › 通用 › 「匿名使用统计与崩溃报告」关掉后两者都不发：每次发送前现读配置，改了立即生效。
//! - 只发枚举、版本号、布尔：功能名是内置 provider 的 id（插件统一记作 `plugins`），
//!   不发查询词、文件名、剪贴板或译文内容；错误里的家目录换成 `~`。
//! - 开发构建默认不发；`HELMP_TELEMETRY=1` 强制打开（environment = development），`=0` 强制关闭。
//!   图形自检（`HELMP_SELF_TEST`）与演练模式（`HELMP_DRY_RUN`，e2e 用）不发。
//! - Sentry DSN 构建期从环境变量 `SENTRY_DSN_HELMP` 注入，没有就不初始化 Sentry（Umami 照常）。

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const UMAMI_URL: &str = "https://a.hweb.peterchen97.cn/api/send";
/// Umami 后台的 website「Helmp」（落地页用同一个）。
const UMAMI_WEBSITE: &str = "202fc4e3-a6c6-46d8-8517-faf1f867822f";
const SENTRY_DSN: Option<&str> = option_env!("SENTRY_DSN_HELMP");
const TIMEOUT: Duration = Duration::from_secs(10);

/// 记「功能使用」时认的 provider id：结果项 id 的前缀（`<provider>:…`）不在表里就记作 `other`，
/// 保证上报的只会是这些枚举值。新增内置 provider 时补上（单测会提醒）。
const PROVIDERS: [&str; 25] = [
    "builtin",
    "calculator",
    "system",
    "apps",
    "screenshot",
    "clipboard",
    "scripts",
    "plugins",
    "mcp",
    "agents",
    "translate",
    "processes",
    "window",
    "snippets",
    "notes",
    "websearch",
    "files",
    "settings-panes",
    "toggles",
    "bookmarks",
    "dictionary",
    "time",
    "devtools",
    "emoji",
    "color",
];

pub fn release() -> String {
    format!("helmp@{}", env!("CARGO_PKG_VERSION"))
}

/// 这次运行允许上报时的 environment；构建类型或环境变量不允许时为 None。
fn environment() -> Option<&'static str> {
    let dev = cfg!(debug_assertions);
    let env = |name| std::env::var(name).ok();
    match env("HELMP_TELEMETRY").as_deref() {
        Some("0") => return None,
        Some("1") => return Some(if dev { "development" } else { "production" }),
        _ => {}
    }
    if dev || env("HELMP_SELF_TEST").is_some() || crate::effects::dry_run() {
        return None;
    }
    Some("production")
}

/// 现在能不能发：构建 / 环境允许，且用户没在设置里关掉。
pub fn enabled() -> bool {
    environment().is_some() && crate::config::get().general.telemetry
}

/// 初始化 Sentry（未捕获 panic 自动上报）。返回的 guard 要活到进程结束，drop 时把队列发完。
pub fn init() -> Option<sentry::ClientInitGuard> {
    let environment = environment()?;
    let dsn = SENTRY_DSN.filter(|dsn| !dsn.is_empty())?;
    let mut options = sentry::ClientOptions::default();
    options.release = Some(release().into());
    options.environment = Some(environment.into());
    options.send_default_pii = false;
    options.before_send = Some(Arc::new(|event| {
        crate::config::get()
            .general
            .telemetry
            .then(|| scrub(event, &home()))
    }));
    Some(sentry::init((dsn, options)))
}

/// 把一个处理过的错误报给 Sentry。`place` 是固定的位置名（如 `history.save`），作为标题与标签。
pub fn report(place: &'static str, err: &anyhow::Error) {
    let mut event = sentry::event_from_error::<dyn std::error::Error>(err.as_ref());
    event.message = Some(format!("{place}: {err:#}"));
    event.tags.insert("place".into(), place.into());
    // 标题用位置名（anyhow 的类型名都是 `Error`，看不出是哪里）；附上调用处的堆栈
    // （发版时上传了 dSYM，Sentry 端符号化）。
    if let Some(exception) = event.exception.values.last_mut() {
        exception.ty = place.into();
        exception.stacktrace = sentry::integrations::backtrace::current_stacktrace();
    }
    sentry::capture_event(event);
}

/// 等待队列里的事件发出（马上就要退出时调用）。
pub fn flush() {
    if let Some(client) = sentry::Hub::current().client() {
        client.flush(Some(Duration::from_secs(3)));
    }
}

/// 图形界面启动：记一次 `launch`；版本号比上次运行时新，再记一次 `update_installed`。
pub fn launched() {
    let previous = match state::swap_version(env!("CARGO_PKG_VERSION")) {
        Ok(previous) => previous,
        Err(err) => {
            eprintln!("helmp: 读写统计状态失败：{err:#}");
            report("telemetry.state", &err);
            None
        }
    };
    track("launch", json!({}));
    if let Some(from) = previous.filter(|from| from != env!("CARGO_PKG_VERSION")) {
        track("update_installed", json!({ "from": from }));
    }
}

/// 用户执行了一个结果项：按它所属的 provider 记 `use_<provider>`。
pub fn used(provider: &str) {
    track(&format!("use_{}", known_provider(provider)), json!({}));
}

/// 便签只上报固定操作枚举，不接收正文或便签标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteEvent {
    Created,
    Docked,
    Deleted,
}

impl NoteEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Created => "note_create",
            Self::Docked => "note_dock",
            Self::Deleted => "note_delete",
        }
    }
}

pub fn note(event: NoteEvent) {
    track(event.name(), json!({}));
}

/// 拖把手改了便签大小：只发宽度档位（`s` ≤ 300 / `m` ≤ 460 / `l`），不发正文。
pub fn note_resized(width: f64) {
    track("note_resized", json!({ "size": note_size_bucket(width) }));
}

/// 编辑完一张便签、正文里用到了 Markdown：只发用到的语法类别（如 `bold+task`），不发正文。
pub fn note_markdown_used(kinds: &[&'static str]) {
    track("note_markdown_used", json!({ "kinds": kinds.join("+") }));
}

fn note_size_bucket(width: f64) -> &'static str {
    if width <= 300.0 {
        "s"
    } else if width <= 460.0 {
        "m"
    } else {
        "l"
    }
}

/// 从剪贴板历史写回一条文本：`mode` 为 `paste` / `copy` / `plain_text`，`formats` 是一起写回的富文本种类
/// （来自固定表的短名，如 `rtf+html`；没有为 `plain`）。只发这两个枚举，不发内容。
pub fn clipboard_paste(mode: &'static str, formats: &[&'static str]) {
    let formats = if formats.is_empty() {
        "plain".to_string()
    } else {
        formats.join("+")
    };
    track(
        "clipboard_paste",
        json!({ "mode": mode, "formats": formats }),
    );
}

/// 从插件中心或自定义插件源装好 / 更新了一个插件：只发源的类型和是否更新，不发插件 id 和地址。
pub fn plugin_installed(custom: bool, update: bool) {
    track(
        "plugin_install",
        json!({ "source": if custom { "custom" } else { "public" }, "update": update }),
    );
}

/// 一次插件更新检查有了结果（有插件更新了或失败了；什么都没变时不发）。`trigger` 是 `scheduled` / `manual`。
/// 不发插件 id、版本和地址。
pub fn plugin_autoupdate(trigger: &'static str, updated: bool, needs_consent: bool, failed: bool) {
    if !updated && !failed {
        return;
    }
    track(
        "plugin_autoupdate",
        json!({ "trigger": trigger, "updated": updated, "needs_consent": needs_consent, "failed": failed }),
    );
}

/// 卸载了一个从插件中心或插件源装的插件：源的类型、是否一并删了数据目录。
pub fn plugin_uninstalled(custom: bool, delete_data: bool) {
    track(
        "plugin_uninstall",
        json!({ "source": if custom { "custom" } else { "public" }, "delete_data": delete_data }),
    );
}

/// 搜索框显示了插件提供的寄语（每天最多一次）：轮换方式、是不是按日期指定的那条。不发插件 id 和文案。
pub fn plugin_greeting_shown(rotation: crate::providers::plugins::manifest::Rotation, dated: bool) {
    use crate::providers::plugins::manifest::Rotation;
    let rotation = match rotation {
        Rotation::Open => "open",
        Rotation::Daily => "daily",
    };
    track(
        "plugin_greeting_shown",
        json!({ "rotation": rotation, "dated": dated }),
    );
}

/// 用户在设置 › 插件里打开 / 关掉某个插件的搜索框寄语。不发插件 id。
pub fn plugin_greeting_toggled(enabled: bool) {
    track("plugin_greeting_toggle", json!({ "enabled": enabled }));
}

/// 插件定时任务跑了一次（每种触发方式、成败组合每天最多一次）：触发方式（interval / daily / date）、
/// 是否成功、是否发了通知、是否超时被杀。不发插件 id、任务名和脚本输出。
pub fn plugin_schedule_run(trigger: &'static str, ok: bool, notified: bool, timed_out: bool) {
    track(
        "plugin_schedule_run",
        json!({ "trigger": trigger, "ok": ok, "notified": notified, "timed_out": timed_out }),
    );
}

/// 用户在设置 › 插件里单独打开 / 停用某个定时任务。不发插件 id 和任务名。
pub fn plugin_schedule_toggled(enabled: bool) {
    track("plugin_schedule_toggle", json!({ "enabled": enabled }));
}

/// 第一次要发通知时向系统请求授权，用户的选择。
pub fn plugin_notification_permission(granted: bool) {
    track("plugin_notification_permission", json!({ "granted": granted }));
}

/// 插件主题生效（每天最多一次）：是不是按日期自动换上的、有没有背景图。不发插件 id 和主题名。
pub fn plugin_theme_applied(dated: bool, image: bool) {
    track(
        "plugin_theme_applied",
        json!({ "dated": dated, "image": image }),
    );
}

/// 用户在「设置 › 外观」里选了主题：`plugin` = 插件提供的，否则是默认配色。不发插件 id。
pub fn plugin_theme_selected(plugin: bool) {
    track("plugin_theme_select", json!({ "plugin": plugin }));
}

/// 打开 / 关掉「按日期自动切换主题」。
pub fn plugin_theme_by_date(enabled: bool) {
    track("plugin_theme_by_date", json!({ "enabled": enabled }));
}

/// 点了「今天不用」（`skip`）或「今天也用」。
pub fn plugin_theme_skip_today(skip: bool) {
    track("plugin_theme_skip_today", json!({ "skip": skip }));
}

/// 插件源的增删：`via` 是入口（`settings` / `cli`），`auth` 是否带鉴权。不发名字和地址。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceEvent {
    Added { via: &'static str, auth: bool },
    Removed,
    TokenChanged,
}

pub fn plugin_source(event: SourceEvent) {
    match event {
        SourceEvent::Added { via, auth } => {
            track("plugin_source_add", json!({ "via": via, "auth": auth }))
        }
        SourceEvent::Removed => track("plugin_source_remove", json!({})),
        SourceEvent::TokenChanged => track("plugin_source_token", json!({})),
    }
}

fn known_provider(provider: &str) -> &'static str {
    PROVIDERS
        .iter()
        .find(|id| **id == provider)
        .copied()
        .unwrap_or("other")
}

/// 发一个 Umami 自定义事件（后台线程，失败只记 stderr，不影响使用）。`data` 只放枚举 / 数字 / 布尔。
fn track(name: &str, mut data: Value) {
    if !enabled() {
        return;
    }
    if let Value::Object(map) = &mut data {
        map.insert("version".into(), env!("CARGO_PKG_VERSION").into());
    }
    let body = json!({
        "type": "event",
        "payload": {
            "website": UMAMI_WEBSITE,
            "hostname": "app.helmp",
            "url": "/app",
            "title": "Helmp",
            "language": if crate::i18n::is_en() { "en" } else { "zh-CN" },
            "screen": "",
            "name": name,
            "data": data,
            "id": state::install_id(),
        }
    });
    let name = name.to_string();
    std::thread::spawn(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .build()
            .into();
        if let Err(err) = agent
            .post(UMAMI_URL)
            .header("User-Agent", &user_agent())
            .send_json(&body)
        {
            eprintln!("helmp: 统计事件 {name} 发送失败：{err}");
        }
    });
}

/// Umami 把不像浏览器的 User-Agent 当机器人丢掉，并从这里解析出操作系统。
fn user_agent() -> String {
    let macos = crate::updates::macos_version()
        .unwrap_or_else(|| "10.15".into())
        .replace('.', "_");
    format!(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X {macos}) Helmp/{}",
        env!("CARGO_PKG_VERSION")
    )
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// 去掉事件里可能带出用户信息的部分：主机名、用户；文本和路径里的家目录换成 `~`。
fn scrub(mut event: sentry::protocol::Event<'static>, home: &str) -> sentry::protocol::Event<'static> {
    let clean = |text: &mut String| {
        if home.len() > 1 && text.contains(home) {
            *text = text.replace(home, "~");
        }
    };
    event.server_name = None;
    event.user = None;
    if let Some(message) = &mut event.message {
        clean(message);
    }
    for exception in &mut event.exception.values {
        if let Some(value) = &mut exception.value {
            clean(value);
        }
        for frame in exception
            .stacktrace
            .iter_mut()
            .flat_map(|stacktrace| stacktrace.frames.iter_mut())
        {
            if let Some(path) = &mut frame.abs_path {
                clean(path);
            }
        }
    }
    for breadcrumb in &mut event.breadcrumbs.values {
        if let Some(message) = &mut breadcrumb.message {
            clean(message);
        }
    }
    event
}

/// 数据目录里的 `telemetry.json`：匿名安装 ID（随机 UUID，与设备、账号无关）与上次运行的版本号。
mod state {
    use super::*;
    use anyhow::{Context as _, Result};
    use std::sync::{Mutex, OnceLock};

    #[derive(Default, Clone, Serialize, Deserialize)]
    struct State {
        install_id: String,
        last_version: Option<String>,
    }

    fn path() -> Result<std::path::PathBuf> {
        Ok(crate::paths::data_dir()?.join("telemetry.json"))
    }

    fn cached() -> &'static Mutex<Option<State>> {
        static STATE: OnceLock<Mutex<Option<State>>> = OnceLock::new();
        STATE.get_or_init(Default::default)
    }

    fn load() -> Result<State> {
        let path = path()?;
        let mut state: State = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(err) => return Err(err).with_context(|| format!("读取 {}", path.display())),
        };
        if state.install_id.is_empty() {
            state.install_id = uuid::Uuid::new_v4().to_string();
            save(&state)?;
        }
        Ok(state)
    }

    fn save(state: &State) -> Result<()> {
        let path = path()?;
        std::fs::write(&path, serde_json::to_string(state)?)
            .with_context(|| format!("写入 {}", path.display()))
    }

    fn with<R>(f: impl FnOnce(&mut State) -> Result<R>) -> Result<R> {
        let mut cached = cached().lock().expect("telemetry state poisoned");
        if cached.is_none() {
            *cached = Some(load()?);
        }
        f(cached.as_mut().expect("loaded above"))
    }

    /// 记下本次运行的版本，返回上次运行的版本（第一次运行为 None）。
    pub fn swap_version(current: &str) -> Result<Option<String>> {
        with(|state| {
            let previous = state.last_version.replace(current.to_string());
            if previous.as_deref() != Some(current) {
                save(state)?;
            }
            Ok(previous)
        })
    }

    /// 读不到状态文件时用一个本次运行内固定的临时 ID，统计照常发。
    pub fn install_id() -> String {
        static FALLBACK: OnceLock<String> = OnceLock::new();
        with(|state| Ok(state.install_id.clone()))
            .unwrap_or_else(|_| FALLBACK.get_or_init(|| uuid::Uuid::new_v4().to_string()).clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_table_matches_registered_providers() {
        // 表里缺了某个 provider，它的使用就全记成 `other`，后台看不出这个功能有没有人用。
        let registered: Vec<&str> = crate::providers::all().iter().map(|p| p.id()).collect();
        let mut table = PROVIDERS.to_vec();
        let mut registered_sorted = registered.clone();
        table.sort_unstable();
        registered_sorted.sort_unstable();
        assert_eq!(table, registered_sorted);
    }

    #[test]
    fn only_known_provider_ids_leave_the_machine() {
        // 上报的名字只能是表里的枚举：别的字符串（误传进来的路径、内容）一律变成 `other`。
        assert_eq!(known_provider("clipboard"), "clipboard");
        assert_eq!(known_provider("settings-panes"), "settings-panes");
        assert_eq!(known_provider("notes"), "notes");
        assert_eq!(known_provider("/Users/alice/secret.txt"), "other");
        assert_eq!(known_provider("我的密码是 123"), "other");
        assert_eq!(known_provider(""), "other");
    }

    #[test]
    fn note_events_map_to_fixed_names() {
        // 事件名必须稳定，且接口不接受正文，防止便签私密内容进入统计。
        assert_eq!(NoteEvent::Created.name(), "note_create");
        assert_eq!(NoteEvent::Docked.name(), "note_dock");
        assert_eq!(NoteEvent::Deleted.name(), "note_delete");
        // 缩放事件只带宽度档位：默认 280 算小号，拉到上限 640 算大号。
        assert_eq!(note_size_bucket(280.0), "s");
        assert_eq!(note_size_bucket(400.0), "m");
        assert_eq!(note_size_bucket(640.0), "l");
    }

    #[test]
    fn scrub_hides_home_directory_and_host() {
        // 错误信息里常带文件路径，路径里有用户名；主机名常是「某某的 MacBook」。
        let mut event = sentry::protocol::Event {
            message: Some("写入 /Users/alice/Library/Application Support/Helmp/history.json".into()),
            server_name: Some("Alices-MacBook".into()),
            ..Default::default()
        };
        event.exception.values.push(sentry::protocol::Exception {
            value: Some("/Users/alice/x: Permission denied".into()),
            ..Default::default()
        });
        let event = scrub(event, "/Users/alice");
        assert_eq!(
            event.message.as_deref(),
            Some("写入 ~/Library/Application Support/Helmp/history.json")
        );
        assert_eq!(event.server_name, None);
        assert_eq!(
            event.exception.values[0].value.as_deref(),
            Some("~/x: Permission denied")
        );
    }
}
