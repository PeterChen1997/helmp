//! 执行清单里的动作：open / http / script / copy / paste，以及列表命令的脚本。
//!
//! 安全约定：
//! - 所有改动用户环境的动作先过 `effects::dry`（演练模式只回报不执行）。
//! - http 只发往清单推导出的主机（运行时按渲染后的地址再查一次），不跟随重定向，避免被跳到别的主机。
//!   演练模式下只有发往本机（127.0.0.1 / localhost）的请求真的发出（端到端测试的假服务），其余只回报。
//!   测试模式（见 `secrets::test_mode`）额外允许本机地址，flomo 这类锁定主机名的插件才能指向假服务。
//! - 脚本按 argv 传参，从不经过 shell；路径必须在插件目录里（解析符号链接后再查一次）。
//! - secret 偏好设置的值在输出、错误信息、演练回报里一律替换成 `••••`。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use serde_json::Value;

use super::manifest::{
    ActionSpec, HttpSpec, ListSpec, Method, PrefKind, ScriptOutput, ScriptSpec, host_of,
};
use super::registry::Plugin;
use super::secrets;
use super::template::{self, Context, Template};
use crate::i18n::{tr, trf};
use crate::provider::{Action, Block, Detail, Icon, Outcome};
use crate::providers::scripts::runner;

/// 脚本动作的默认超时（与脚本插件一致）。
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(30);
/// http 默认超时。
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// 列表脚本默认超时：每次输入都可能跑，要短。
const LIST_TIMEOUT: Duration = Duration::from_millis(1000);
/// 列表最多显示的项数。
const MAX_LIST_ITEMS: usize = 200;
const MASK: &str = "••••";

/// 一次执行的输入。
pub struct Input<'a> {
    pub plugin: &'a Arc<Plugin>,
    /// 命令标题（当前语言），提示和错误信息里用。
    pub title: &'a str,
    pub icon: Icon,
    pub query: &'a str,
    pub form: &'a BTreeMap<String, String>,
    /// 列表项（列表命令的动作）。
    pub item: Option<&'a BTreeMap<String, String>>,
}

/// 偏好设置的当前值：secret 从钥匙串取，其余从配置取，没填用清单默认值。
pub fn pref_value(plugin: &Plugin, key: &str) -> Result<String> {
    let Some(pref) = plugin.manifest.preference(key) else {
        return Ok(String::new());
    };
    if pref.kind == PrefKind::Secret {
        return Ok(secrets::get(plugin.id(), key)?.unwrap_or_default());
    }
    let config = crate::config::get();
    Ok(config
        .plugins
        .get(plugin.id())
        .and_then(|settings| settings.prefs.get(key))
        .cloned()
        .or_else(|| pref.default.clone())
        .unwrap_or_default())
}

/// 不读钥匙串的取值（查询时渲染表单初值、预览地址用；secret 一律当空）。
pub fn plain_pref_value(plugin: &Plugin, key: &str) -> String {
    match plugin.manifest.preference(key) {
        Some(pref) if pref.kind != PrefKind::Secret => pref_value(plugin, key).unwrap_or_default(),
        _ => String::new(),
    }
}

fn read_clipboard() -> Result<String> {
    if crate::effects::dry_run() {
        // 演练模式不读用户的剪贴板。
        return Ok("[clipboard]".into());
    }
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    let pasteboard = NSPasteboard::generalPasteboard();
    // SAFETY: AppKit 导出的常量 NSString，进程内始终有效。
    let ty = unsafe { NSPasteboardTypeString };
    Ok(pasteboard
        .stringForType(ty)
        .map(|text| text.to_string())
        .unwrap_or_default())
}

/// 把输出里出现的密钥换成 `••••`。太短的值不替换（会把正常文字打得到处是码）。
fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for secret in secrets.iter().filter(|secret| secret.chars().count() >= 4) {
        out = out.replace(secret.as_str(), MASK);
    }
    out
}

fn redact_outcome(outcome: Outcome, secrets: &[String]) -> Outcome {
    match outcome {
        Outcome::Message(text) => Outcome::Message(redact(&text, secrets)),
        Outcome::Show(mut detail) => {
            detail.title = redact(&detail.title, secrets);
            for block in &mut detail.blocks {
                match block {
                    Block::Text(text) | Block::Code(text) | Block::Caption(text) => {
                        *text = redact(text, secrets)
                    }
                    Block::Result { text, .. }
                    | Block::Quote { text, .. }
                    | Block::Markdown { text, .. } => *text = redact(text, secrets),
                    Block::Image(_) => {}
                }
            }
            Outcome::Show(detail)
        }
        other => other,
    }
}

/// 执行一个动作。用到的密钥会记下来，结果和错误里出现的都打码。
pub fn run(input: &Input, action: &ActionSpec) -> Result<Outcome> {
    let used = RefCell::new(Vec::new());
    let pref = |key: &str| -> Result<String> {
        let value = pref_value(input.plugin, key)?;
        if input
            .plugin
            .manifest
            .preference(key)
            .is_some_and(|p| p.kind == PrefKind::Secret)
            && !value.is_empty()
        {
            used.borrow_mut().push(value.clone());
        }
        Ok(value)
    };
    let clipboard = read_clipboard;
    let context = Context {
        query: input.query,
        form: input.form,
        pref: &pref,
        clipboard: &clipboard,
        item: input.item,
        response: None,
        now: template::now(),
    };
    let result = match action {
        ActionSpec::Open(spec) => {
            let url = render(&spec.url, &context)?;
            ensure!(
                !url.trim().is_empty(),
                trf!(
                    "「{}」要打开的地址是空的",
                    "“{}” has nothing to open",
                    input.title
                )
            );
            Action::open(url.trim()).run()
        }
        ActionSpec::Copy(spec) => Action::copy(render(&spec.text, &context)?).run(),
        ActionSpec::Paste(spec) => paste(render(&spec.text, &context)?),
        ActionSpec::Http(spec) => http(input, spec, &context),
        ActionSpec::Script(spec) => script(input, spec, &context),
    };
    let secrets = used.into_inner();
    match result {
        Ok(outcome) => Ok(redact_outcome(outcome, &secrets)),
        Err(err) => Err(anyhow!(redact(&format!("{err:#}"), &secrets))),
    }
}

fn render(text: &str, context: &Context) -> Result<String> {
    Template::parse(text)?.render(context)
}

/// 复制并向前台 App 发 ⌘V；没有辅助功能权限时只复制并说明。
fn paste(text: String) -> Result<Outcome> {
    if let Some(outcome) = crate::effects::dry("paste", &text) {
        return Ok(outcome);
    }
    Action::copy(text).run()?;
    if !crate::providers::clipboard::macos::accessibility_trusted() {
        return Ok(Outcome::Message(
            tr(
                "已复制。没有辅助功能权限，无法自动粘贴，请手动粘贴（⌘V）。",
                "Copied. Helmp can’t paste without Accessibility permission, so paste it yourself (⌘V).",
            )
            .into(),
        ));
    }
    std::thread::sleep(Duration::from_millis(150));
    crate::providers::clipboard::macos::send_paste()?;
    Ok(Outcome::Done)
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

/// 渲染 JSON 正文：字符串叶子按模板渲染，结构与非字符串值原样保留（引号、换行由序列化转义）。
fn render_json(value: &Value, context: &Context) -> Result<Value> {
    Ok(match value {
        Value::String(text) => Value::String(render(text, context)?),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| render_json(item, context))
                .collect::<Result<_>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| Ok((key.clone(), render_json(item, context)?)))
                .collect::<Result<_>>()?,
        ),
        other => other.clone(),
    })
}

fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

fn http(input: &Input, spec: &HttpSpec, context: &Context) -> Result<Outcome> {
    let title = input.title;
    let url = render(&spec.url, context)?.trim().to_string();
    let Some((scheme, rest)) = url.split_once("://") else {
        bail!(trf!(
            "「{title}」的地址不是 http(s) 链接，检查插件设置",
            "“{title}”: the URL isn't an http(s) link. Check the plugin settings."
        ));
    };
    ensure!(
        scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http"),
        trf!(
            "「{title}」的地址不是 http(s) 链接，检查插件设置",
            "“{title}”: the URL isn't an http(s) link. Check the plugin settings."
        )
    );
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host_of(authority).to_ascii_lowercase();
    let loopback = is_loopback(&host);
    let allowed = input.plugin.permissions.allows_host(&host) || (loopback && secrets::test_mode());
    ensure!(
        allowed,
        trf!(
            "「{title}」要访问 {host}，不在插件的网络权限里，已拦截",
            "“{title}” tried to reach {host}, which isn't in the plugin's network permissions. Blocked."
        )
    );

    let mut headers = Vec::new();
    for (name, value) in &spec.headers {
        let value = render(value, context)?;
        ensure!(
            !value.contains(['\r', '\n']),
            trf!(
                "请求头 {name} 含换行，已拦截",
                "Header {name} contains a line break. Blocked."
            )
        );
        headers.push((name.clone(), value));
    }
    let body = match (&spec.json, &spec.body) {
        (Some(json), _) => {
            if !headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            {
                headers.push(("Content-Type".into(), "application/json".into()));
            }
            Some(render_json(json, context)?.to_string())
        }
        (None, Some(body)) => Some(render(body, context)?),
        (None, None) => None,
    };

    let method = spec.method.name();
    if crate::effects::dry_run() && !loopback {
        return Ok(Outcome::Message(format!(
            "[dry-run] http {method} {url} {}",
            body.as_deref().unwrap_or_default()
        )));
    }

    let timeout = spec.timeout_ms.map_or(HTTP_TIMEOUT, Duration::from_millis);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        // 不跟随重定向：跳转目标可能是权限之外的主机。
        .max_redirects(0)
        .build()
        .into();
    let sent = match spec.method {
        Method::Get | Method::Delete => {
            let mut request = if spec.method == Method::Get {
                agent.get(&url)
            } else {
                agent.delete(&url)
            };
            for (name, value) in &headers {
                request = request.header(name.as_str(), value.as_str());
            }
            request.call()
        }
        Method::Post | Method::Put | Method::Patch => {
            let mut request = match spec.method {
                Method::Post => agent.post(&url),
                Method::Put => agent.put(&url),
                _ => agent.patch(&url),
            };
            for (name, value) in &headers {
                request = request.header(name.as_str(), value.as_str());
            }
            request.send(body.unwrap_or_default())
        }
    };
    let mut response = sent.map_err(|err| {
        anyhow!(trf!(
            "「{title}」连不上 {host}：{err}",
            "“{title}” couldn't reach {host}: {err}"
        ))
    })?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().unwrap_or_default();
    let json: Option<Value> = serde_json::from_str(&text).ok();

    let status_ok = if spec.success.status.is_empty() {
        (200..300).contains(&status)
    } else {
        spec.success.status.contains(&status)
    };
    let check_ok = match (&spec.success.json_path, &spec.success.equals) {
        (Some(path), Some(expected)) => {
            match json.as_ref().and_then(|j| template::json_path(j, path)) {
                Some(actual) => actual == expected,
                None => spec.success.optional,
            }
        }
        _ => true,
    };
    if !status_ok || !check_ok {
        let reason = spec
            .error_path
            .as_deref()
            .and_then(|path| json.as_ref().and_then(|j| template::json_path(j, path)))
            .map(|value| match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .unwrap_or_else(|| clip(&text, 300));
        if status_ok {
            bail!(trf!(
                "「{title}」失败：服务端返回 HTTP {status}，但结果表示失败：{reason}",
                "“{title}” failed: the server answered HTTP {status} but reported an error: {reason}"
            ));
        }
        bail!(trf!(
            "「{title}」失败：HTTP {status}：{reason}",
            "“{title}” failed: HTTP {status}: {reason}"
        ));
    }

    let message = match &spec.message {
        Some(message) => {
            let with_response = Context {
                response: json.as_ref(),
                ..*context
            };
            render(message.get(), &with_response)?
        }
        None => trf!("「{title}」已完成", "“{title}” done"),
    };
    Ok(Outcome::Message(message))
}

/// 插件目录里的脚本：解析符号链接后仍必须在插件目录里。
fn script_path(plugin: &Plugin, relative: &str) -> Result<(PathBuf, PathBuf)> {
    let dir = plugin
        .dir
        .as_ref()
        .with_context(|| tr("自带插件不能运行脚本", "Bundled plugins can't run scripts"))?;
    let dir = std::fs::canonicalize(dir).with_context(|| {
        trf!(
            "插件目录 {} 不可用",
            "Plugin folder {} is unavailable",
            dir.display()
        )
    })?;
    let path = std::fs::canonicalize(dir.join(relative))
        .with_context(|| trf!("找不到脚本 {relative}", "Script {relative} not found"))?;
    ensure!(
        path.starts_with(&dir),
        trf!(
            "脚本 {relative} 指向插件目录之外，已拦截",
            "Script {relative} points outside the plugin folder. Blocked."
        )
    );
    Ok((path, dir))
}

/// 一次脚本调用：路径、参数、环境变量、stdin 都是模板原文。
struct ScriptCall<'a> {
    path: &'a str,
    args: &'a [String],
    env: &'a BTreeMap<String, String>,
    stdin: Option<&'a str>,
    timeout: Duration,
}

fn run_script(plugin: &Plugin, title: &str, call: ScriptCall, context: &Context) -> Result<String> {
    let ScriptCall {
        path,
        args,
        env,
        stdin,
        timeout,
    } = call;
    let (path, dir) = script_path(plugin, path)?;
    let args: Vec<String> = args
        .iter()
        .map(|arg| render(arg, context))
        .collect::<Result<_>>()?;
    let mut vars: Vec<(String, String)> = env
        .iter()
        .map(|(key, value)| Ok((key.clone(), render(value, context)?)))
        .collect::<Result<_>>()?;
    vars.push(("HELMP_PLUGIN_DIR".into(), dir.display().to_string()));
    vars.push(("HELMP_PLUGIN_ID".into(), plugin.id().to_string()));
    let stdin = stdin.map(|text| render(text, context)).transpose()?;
    let output = runner::execute_with(&path, &args, &dir, timeout, &vars, stdin.as_deref())?;
    if !output.status.success() {
        let detail = if output.stderr.trim().is_empty() {
            &output.stdout
        } else {
            &output.stderr
        };
        let tail: Vec<&str> = detail.trim_end().lines().rev().take(10).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        let code = output
            .status
            .code()
            .map_or_else(|| output.status.to_string(), |code| code.to_string());
        bail!(trf!(
            "「{title}」的脚本失败（退出码 {code}）：\n{}",
            "“{title}”: the script failed (exit code {code}):\n{}",
            tail.join("\n")
        ));
    }
    Ok(output.stdout)
}

fn script(input: &Input, spec: &ScriptSpec, context: &Context) -> Result<Outcome> {
    let title = input.title;
    let timeout = spec
        .timeout_ms
        .map_or(SCRIPT_TIMEOUT, Duration::from_millis);
    let call = ScriptCall {
        path: &spec.path,
        args: &spec.args,
        env: &spec.env,
        stdin: spec.stdin.as_deref(),
        timeout,
    };
    let stdout = run_script(input.plugin, title, call, context)?;
    let text = stdout.trim_end().to_string();
    match spec.output {
        ScriptOutput::Silent => Ok(Outcome::Done),
        ScriptOutput::Copy => Action::copy(text).run(),
        ScriptOutput::Paste => paste(text),
        ScriptOutput::Message if text.trim().is_empty() => Ok(Outcome::Message(trf!(
            "「{title}」已完成（没有输出）",
            "“{title}” finished with no output"
        ))),
        ScriptOutput::Message => Ok(Outcome::Show(Detail {
            icon: input.icon.clone(),
            title: title.to_string(),
            blocks: vec![Block::Code(text)],
            ..Detail::default()
        })),
    }
}

/// 列表脚本输出的一项（Alfred Script Filter JSON）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    /// title / subtitle / arg / uid / url，供 `{{item.*}}` 用。
    pub fields: BTreeMap<String, String>,
    pub icon: Option<Icon>,
    /// Alfred 的 `valid: false`：只是提示，不能执行。
    pub valid: bool,
    /// `text.copy`：⌘K「复制」的内容，缺省为 arg。
    pub copy: Option<String>,
}

/// 运行列表脚本并解析输出。查询时调用：超时短、不读剪贴板、不读密钥以外的东西。
pub fn run_list(
    plugin: &Plugin,
    title: &str,
    spec: &ListSpec,
    query: &str,
) -> Result<Vec<ListItem>> {
    let used = RefCell::new(Vec::new());
    let pref = |key: &str| -> Result<String> {
        let value = pref_value(plugin, key)?;
        if plugin
            .manifest
            .preference(key)
            .is_some_and(|p| p.kind == PrefKind::Secret)
        {
            used.borrow_mut().push(value.clone());
        }
        Ok(value)
    };
    let no_clipboard = || -> Result<String> { Ok(String::new()) };
    let form = BTreeMap::new();
    let context = Context {
        query,
        form: &form,
        pref: &pref,
        clipboard: &no_clipboard,
        item: None,
        response: None,
        now: template::now(),
    };
    let timeout = spec
        .script
        .timeout_ms
        .map_or(LIST_TIMEOUT, Duration::from_millis);
    let call = ScriptCall {
        path: &spec.script.path,
        args: &spec.script.args,
        env: &spec.script.env,
        stdin: None,
        timeout,
    };
    let stdout = run_script(plugin, title, call, &context)
        .map_err(|err| anyhow!(redact(&format!("{err:#}"), &used.borrow())))?;
    parse_alfred(&stdout, plugin.dir.as_deref())
        .map_err(|err| anyhow!(redact(&format!("{err:#}"), &used.borrow())))
}

/// 解析 `{"items":[{"title":..,"subtitle":..,"arg":..,"uid":..,"icon":{"path":..},"valid":..,"text":{"copy":..}}]}`。
pub fn parse_alfred(text: &str, dir: Option<&Path>) -> Result<Vec<ListItem>> {
    let value: Value = serde_json::from_str(text.trim()).map_err(|err| {
        anyhow!(trf!(
            "列表脚本的输出不是 JSON（要输出 {{\"items\": [...]}}）：{err}；输出开头：{}",
            "The list script didn't print JSON (expected {{\"items\": [...]}}): {err}; output starts with: {}",
            clip(text, 120)
        ))
    })?;
    let items = value["items"].as_array().with_context(|| {
        tr(
            "列表脚本的输出缺少 items 数组",
            "The list script's output has no items array",
        )
    })?;
    let mut out = Vec::new();
    for (index, item) in items.iter().take(MAX_LIST_ITEMS).enumerate() {
        let title = item["title"].as_str().with_context(|| {
            trf!(
                "列表第 {} 项没有 title",
                "List item {} has no title",
                index + 1
            )
        })?;
        let string = |key: &str| match &item[key] {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            // Alfred 允许 arg 是数组（多个参数），这里按换行拼起来。
            Value::Array(values) => Some(
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        };
        let mut fields = BTreeMap::new();
        fields.insert("title".to_string(), title.to_string());
        for key in ["subtitle", "arg", "uid"] {
            if let Some(value) = string(key) {
                fields.insert(key.to_string(), value);
            }
        }
        if let Some(url) = item["quicklookurl"].as_str() {
            fields.insert("url".into(), url.to_string());
        }
        let icon = item["icon"]["path"].as_str().and_then(|path| {
            let path = match dir {
                Some(dir) if !path.starts_with('/') => dir.join(path),
                _ => PathBuf::from(path),
            };
            if item["icon"]["type"].as_str() == Some("fileicon") {
                return Some(Icon::File(path));
            }
            let image = path.extension().is_some_and(|ext| {
                ["png", "jpg", "jpeg"].contains(&ext.to_string_lossy().to_lowercase().as_str())
            });
            (image && path.is_file()).then_some(Icon::Image(path))
        });
        out.push(ListItem {
            fields,
            icon,
            valid: item["valid"].as_bool().unwrap_or(true),
            copy: item["text"]["copy"].as_str().map(str::to_string),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_masked_in_messages() {
        // webhook 地址本身就是密钥：出错信息、演练回报里原样打出来，就等于写进了日志和截图。
        let secrets = vec!["https://flomoapp.com/iwh/abc/def/".to_string(), "ab".into()];
        assert_eq!(
            redact("POST https://flomoapp.com/iwh/abc/def/ failed ab", &secrets),
            "POST •••• failed ab"
        );
    }

    #[test]
    fn alfred_items_map_to_list_items() {
        // 兼容 Alfred Script Filter 的输出：社区里现成的列表脚本才能直接拿来用。
        let items = parse_alfred(
            r#"{"items":[
                {"uid":"1","title":"helep","subtitle":"~/code/helep","arg":"/Users/x/helep","text":{"copy":"helep"}},
                {"title":"No results","valid":false},
                {"title":"multi","arg":["a","b"]}
            ]}"#,
            None,
        )
        .unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].fields["arg"], "/Users/x/helep");
        assert_eq!(items[0].fields["subtitle"], "~/code/helep");
        assert_eq!(items[0].copy.as_deref(), Some("helep"));
        assert!(!items[1].valid);
        assert_eq!(items[2].fields["arg"], "a\nb");
    }

    #[test]
    fn bad_list_output_is_an_error_not_an_empty_list() {
        // 脚本输出坏了要显示原因；悄悄显示空列表，作者会以为是查询没命中。
        let err = crate::i18n::with_lang(crate::i18n::Lang::En, || {
            parse_alfred("hello", None).unwrap_err().to_string()
        });
        assert!(
            err.contains("didn't print JSON") && err.contains("hello"),
            "{err}"
        );
        assert!(parse_alfred(r#"{"items":[{"subtitle":"x"}]}"#, None).is_err());
        assert!(parse_alfred(r#"{"rows":[]}"#, None).is_err());
    }

    #[test]
    fn json_bodies_render_leaves_without_breaking_structure() {
        // 用户输入里的引号、换行不能把 JSON 正文弄坏，也不能注入别的字段。
        let form = BTreeMap::from([("c".to_string(), "a \"quote\"\nline2".to_string())]);
        let pref = |_: &str| Ok(String::new());
        let clipboard = || Ok(String::new());
        let context = Context {
            query: "",
            form: &form,
            pref: &pref,
            clipboard: &clipboard,
            item: None,
            response: None,
            now: 0,
        };
        let body = render_json(
            &serde_json::json!({"content": "{{form.c}}", "n": 1, "list": ["{{form.c}}"]}),
            &context,
        )
        .unwrap();
        assert_eq!(body["content"], "a \"quote\"\nline2");
        assert_eq!(body["n"], 1);
        assert_eq!(body["list"][0], "a \"quote\"\nline2");
    }
}
