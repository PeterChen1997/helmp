//! 让 agent 操作 Helmp：`helmp_search` 与 `helmp_run`。
//!
//! - 隐私：剪贴板历史、Agent 会话记录与「最近使用」默认不给 agent 看（`mcp_server.expose_private` 打开才给）。
//! - 副作用：只有白名单里的纯计算结果（计算器、换算、时间、开发者工具、词典、翻译、对给定图片路径 OCR）
//!   在本进程里直接算出文字；其余一律要用户在 Helmp 面板里确认，由图形界面执行（见 `ipc`）。
//!   本进程本身是强制演练模式（`effects::force_dry_run`），即使白名单判断有误，复制、打开之类也只回报不执行。
//! - `HELMP_DRY_RUN=1`（端到端测试）时不走确认通道，直接返回演练结果。

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::engine::RECENT_ID_PREFIX;
use crate::provider::{Action, ActionInput, FieldKind, Item};

use super::ipc;
use super::server::{Server, ToolOutput};

/// 内容涉及隐私的 provider：默认不出现在 agent 的搜索结果里，也不能被执行。
const PRIVATE_PROVIDERS: &[&str] = &["clipboard", "agents"];

/// 纯计算：主操作只把结果算出来（顶多复制到剪贴板）。agent 拿到文字，什么也不复制。
const PURE_PROVIDERS: &[&str] = &["calculator", "time", "devtools", "dictionary", "translate"];
/// 纯计算的特定结果（provider 本身不在白名单里）。
const PURE_PREFIXES: &[&str] = &["screenshot:ocr-file:"];
/// 白名单 provider 里仍有副作用的结果。
const IMPURE_IDS: &[&str] = &["dictionary:open"];

/// 结果 id 所属的 provider（「最近使用」的 `recent:` 前缀会先去掉）。
fn provider_of(id: &str) -> &str {
    let id = id.strip_prefix(RECENT_ID_PREFIX).unwrap_or(id);
    id.split(':').next().unwrap_or_default()
}

/// 隐私结果：剪贴板历史、Agent 会话，以及首页的「最近使用」（用户用过什么本身就是隐私）。
pub fn is_private(id: &str) -> bool {
    id.starts_with(RECENT_ID_PREFIX) || PRIVATE_PROVIDERS.contains(&provider_of(id))
}

/// 执行这个结果的主操作（`named_action` 为 None）是否可以不经用户确认。保守：不认识的都要确认，
/// 点名要执行次级操作（如「替换选中文字」「朗读」）也要确认。
pub fn is_pure(id: &str, named_action: Option<&str>) -> bool {
    if named_action.is_some() {
        return false;
    }
    let id = id.strip_prefix(RECENT_ID_PREFIX).unwrap_or(id);
    if IMPURE_IDS.contains(&id) {
        return false;
    }
    PURE_PROVIDERS.contains(&provider_of(id))
        || PURE_PREFIXES.iter().any(|prefix| id.starts_with(prefix))
}

fn expose_private() -> bool {
    super::fresh_config().mcp_server.expose_private
}

/// JSON 对象 → 字符串表（数字、布尔转成文字）。
pub fn string_map(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| {
                    let text = match value {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    };
                    (key.clone(), text)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 表单值：agent 没给的字段用表单的初始值（与面板里直接回车一致）。
pub fn fill_form(item: &Item, given: &BTreeMap<String, String>) -> ActionInput {
    let mut input = ActionInput {
        form: given.clone(),
    };
    if let Some(form) = &item.form {
        for field in &form.fields {
            input
                .form
                .entry(field.key.clone())
                .or_insert_with(|| field.value.clone());
        }
    }
    input
}

/// 按标题找动作；None 为主操作。返回（显示用的标题, 动作）。
pub fn pick_action(item: &Item, title: Option<&str>) -> Result<(String, Action)> {
    match title {
        None => Ok((item.action_title.clone(), item.action.clone())),
        Some(title) if title.eq_ignore_ascii_case(&item.action_title) => {
            Ok((item.action_title.clone(), item.action.clone()))
        }
        Some(title) => item
            .actions
            .iter()
            .find(|named| named.title.eq_ignore_ascii_case(title))
            .map(|named| (named.title.clone(), named.action.clone()))
            .with_context(|| {
                let titles: Vec<_> = item.actions.iter().map(|a| a.title.as_str()).collect();
                format!(
                    "No action named {title:?} (available: {} / {})",
                    item.action_title,
                    titles.join(" / ")
                )
            }),
    }
}

fn describe(item: &Item) -> Value {
    let form: Vec<Value> = item
        .form
        .iter()
        .flat_map(|form| &form.fields)
        .map(|field| {
            let kind = match &field.kind {
                FieldKind::Text { multiline: true, .. } => json!("textarea"),
                FieldKind::Text { .. } => json!("text"),
                FieldKind::Secret { .. } => json!("secret"),
                FieldKind::Select { options } => {
                    json!({"select": options.iter().map(|(_, value)| value.clone()).collect::<Vec<_>>()})
                }
            };
            json!({"key": field.key, "label": field.label, "kind": kind, "required": field.required, "default": field.value})
        })
        .collect();
    json!({
        "id": item.id,
        "title": item.title,
        "subtitle": item.subtitle,
        "group": item.badge,
        "provider": provider_of(&item.id),
        "action": item.action_title,
        "actions": item.actions.iter().map(|a| a.title.clone()).collect::<Vec<_>>(),
        "form": form,
        "needs_approval": !is_pure(&item.id, None),
    })
}

pub fn search(server: &Server, args: &Value) -> Result<ToolOutput> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .context("query is required")?;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(20)
        .clamp(1, 50) as usize;
    server.ensure_started();
    let show_private = expose_private();
    let mut hidden = 0;
    let results: Vec<Value> = crate::cli::lookup(&server.engine, query)
        .iter()
        .filter(|item| {
            let keep = show_private || !is_private(&item.id);
            hidden += usize::from(!keep);
            keep
        })
        .take(limit)
        .map(describe)
        .collect();
    let mut text = String::new();
    for result in &results {
        text.push_str(&format!(
            "{}\t{}\t{}\t[{}{}]\n",
            result["id"].as_str().unwrap_or_default(),
            result["title"].as_str().unwrap_or_default(),
            result["subtitle"].as_str().unwrap_or_default(),
            result["action"].as_str().unwrap_or_default(),
            if result["needs_approval"] == true {
                ", needs approval"
            } else {
                ""
            },
        ));
    }
    if results.is_empty() {
        text.push_str("No results.\n");
    }
    if hidden > 0 {
        text.push_str(&format!(
            "({hidden} private result(s) (clipboard history, agent sessions, recently used) hidden; the user can allow them with mcp_server.expose_private)\n"
        ));
    }
    Ok(ToolOutput::ok(
        text,
        json!({"query": query, "results": results, "hidden_private": hidden}),
    ))
}

pub fn run(server: &Server, args: &Value) -> Result<ToolOutput> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .context("query is required")?;
    let id = args
        .get("id")
        .and_then(Value::as_str)
        .context("id is required")?;
    let named = args
        .get("action")
        .and_then(Value::as_str)
        .filter(|a| !a.is_empty());
    let form = string_map(args.get("form"));
    if is_private(id) && !expose_private() {
        bail!(
            "{id} is private (clipboard history, agent sessions or recently used items) and isn't available to agents"
        );
    }
    server.ensure_started();
    let found = crate::cli::lookup(&server.engine, query)
        .into_iter()
        .find(|item| item.id == id);
    let pure = is_pure(id, named);
    let local = pure || std::env::var("HELMP_DRY_RUN").as_deref() == Ok("1");
    let not_found = || {
        anyhow::anyhow!(
            "No result {id} for query {query:?}. Call helmp_search with the same query first."
        )
    };

    // 纯计算，或端到端测试的演练模式：本进程里执行（强制演练，副作用只回报）。
    if local {
        let item = found.ok_or_else(not_found)?;
        let (title, action) = pick_action(&item, named)?;
        let input = fill_form(&item, &form);
        let outcome = action.run_with(&input)?;
        let output = crate::cli::outcome_text(outcome);
        let mut text = String::new();
        if pure {
            text.push_str(&format!("{}\n{}\n", item.title, item.subtitle));
            if let Some(value) = output.strip_prefix("[dry-run] copy ") {
                text.push_str(&format!(
                    "value: {value}\n(not copied: results are returned to you instead)\n"
                ));
            } else {
                text.push_str(&output);
                text.push('\n');
            }
        } else {
            text.push_str(&output);
            text.push('\n');
        }
        return Ok(ToolOutput::ok(
            text.trim_end().to_string(),
            json!({"id": id, "title": item.title, "subtitle": item.subtitle, "action": title,
                   "pure": pure, "decision": if pure { "not_needed" } else { "dry_run" }, "output": output}),
        ));
    }

    // 有副作用：请用户在 Helmp 面板里确认，由图形界面执行。本进程的配置快照可能比界面旧
    // （比如 agent 启动后用户才启用了插件），这里查不到也照样交给界面，由界面按当前状态再查一次。
    let title = match &found {
        Some(item) => pick_action(item, named)?.0,
        None => named.unwrap_or("Run").to_string(),
    };
    let request = json!({
        "v": 1, "type": "run", "agent": server.agent_name(),
        "query": query, "id": id, "action": named, "form": form,
    });
    let reply = match ipc::request(&request, ipc::CONFIRM_TIMEOUT + Duration::from_secs(160)) {
        Ok(reply) => reply,
        Err(ipc::ClientError::NotRunning) => bail!(
            "Helmp isn't running, so the user can't approve this. Ask the user to start Helmp, then try again."
        ),
        Err(ipc::ClientError::Other(err)) => return Err(err),
    };
    confirmation_output(id, &title, &reply)
}

/// 把确认通道的回答变成工具结果。
pub fn confirmation_output(id: &str, title: &str, reply: &Value) -> Result<ToolOutput> {
    if reply.get("ok") != Some(&Value::Bool(true)) {
        bail!(
            "Helmp refused the request: {}",
            reply
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        );
    }
    let decision = reply
        .get("decision")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let output = reply
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let data = json!({"id": id, "action": title, "decision": decision, "output": output});
    Ok(match decision {
        "allowed" if reply.get("failed") == Some(&Value::Bool(true)) => ToolOutput::error(
            format!("The user allowed it, but it failed: {output}"),
            data,
        ),
        "allowed" => ToolOutput::ok(format!("The user allowed it. Result:\n{output}"), data),
        "timeout" => ToolOutput::error(
            "The user didn't answer within 60 seconds, so it was not run (counts as denied).",
            data,
        ),
        _ => ToolOutput::error(
            "The user denied it in Helmp; nothing was run. Don't retry unless they ask.",
            data,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_whitelisted_computations_skip_approval() {
        // 默认要确认：不认识的 provider、白名单里的「打开词典」、点名的次级操作都不能不经用户就执行。
        assert!(is_pure("calculator:result", None));
        assert!(is_pure("time:zone:tokyo", None));
        assert!(is_pure("devtools:base64-encode", None));
        assert!(is_pure("screenshot:ocr-file:/tmp/a.png", None));
        assert!(is_pure("recent:calculator:result", None));
        assert!(!is_pure("calculator:result", Some("Copy Expression")));
        assert!(!is_pure("dictionary:open", None));
        assert!(!is_pure("screenshot:ocr-screen", None));
        for id in [
            "apps:/Applications/Safari.app",
            "plugins:dev.helmp.flomo/memo",
            "processes:123",
            "system:sleep",
            "window:left-half",
            "toggles:dark-mode",
            "scripts:foo",
            "emoji:smile",
            "snippets:x",
            "websearch:google",
            "unknown:thing",
            "",
        ] {
            assert!(!is_pure(id, None), "{id} must need approval");
        }
    }

    #[test]
    fn clipboard_and_agent_sessions_are_private() {
        assert!(is_private("clipboard:42"));
        assert!(is_private("recent:clipboard:42"));
        assert!(is_private("agents:claude:abc"));
        assert!(
            is_private("recent:apps:/Applications/Safari.app"),
            "最近使用过什么也是隐私"
        );
        assert!(!is_private("calculator:result"));
    }

    #[test]
    fn denial_and_timeout_are_errors_the_agent_can_read() {
        let denied =
            confirmation_output("a:b", "Open", &json!({"ok": true, "decision": "denied"})).unwrap();
        assert!(denied.is_error && denied.text.contains("denied"));
        let timeout =
            confirmation_output("a:b", "Open", &json!({"ok": true, "decision": "timeout"}))
                .unwrap();
        assert!(timeout.is_error && timeout.text.contains("60 seconds"));
        let ok = confirmation_output(
            "a:b",
            "Open",
            &json!({"ok": true, "decision": "allowed", "output": "done"}),
        )
        .unwrap();
        assert!(!ok.is_error && ok.text.contains("done"));
        assert!(
            confirmation_output("a:b", "Open", &json!({"ok": false, "error": "nope"})).is_err()
        );
    }
}
