//! `helmp mcp`（agent 那边的进程）与 Helmp 图形界面之间的本地通道：数据目录里的 Unix 套接字
//! `agent.sock`，权限 0600，只接受同一用户的连接。
//!
//! 一次连接一问一答，各一行 JSON：
//!
//! ```text
//! → {"v":1,"type":"run","agent":"Claude Code","query":"…","scope":null,"id":"…","action":null,"form":{}}
//! ← {"ok":true,"decision":"allowed","output":"…","failed":false} | {"ok":true,"decision":"denied"}
//!   | {"ok":true,"decision":"timeout"} | {"ok":false,"error":"…"}
//! → {"v":1,"type":"open_plugin","plugin":"com.example.x"}
//! ← {"ok":true}
//! ```
//!
//! `run`：界面在面板里显示「Claude Code 想执行：…」（回车允许、Esc 拒绝），用户允许后由界面执行并把结果文字
//! 传回。[`CONFIRM_TIMEOUT`] 内没有回答算拒绝，之后再按回车也不会执行。

use std::io::{BufRead as _, BufReader};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::i18n::{tr, trf};
use crate::provider::{Action, Block, Detail, Icon, Item, NamedAction, Outcome, Tint, Tone};

/// 用户确认的时限。
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(60);
/// 用户允许后，等动作执行完的时限（http 最长 30 秒、脚本最长 120 秒）。
const RUN_TIMEOUT: Duration = Duration::from_secs(150);
/// 一条消息的上限，防止有人往套接字里灌数据。
const MAX_MESSAGE: u64 = 1 << 20;

pub fn socket_path() -> Result<PathBuf> {
    Ok(crate::paths::data_dir()?.join("agent.sock"))
}

// ---------------------------------------------------------------- 报文

/// 写一行 JSON。
pub fn write_message(stream: &mut impl std::io::Write, value: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    stream.flush()
}

/// 读一行 JSON（最多 [`MAX_MESSAGE`] 字节）。
pub fn read_message(stream: impl std::io::Read) -> Result<Value> {
    let mut reader = BufReader::new(stream.take(MAX_MESSAGE));
    let mut line = String::new();
    let n = reader.read_line(&mut line).context("read")?;
    if n == 0 {
        bail!("connection closed without a message");
    }
    if !line.ends_with('\n') {
        bail!("message too long or truncated");
    }
    serde_json::from_str(&line).context("message isn't JSON")
}

// ---------------------------------------------------------------- 客户端（helmp mcp）

#[derive(Debug)]
pub enum ClientError {
    /// 图形界面没在运行（套接字不存在或没人监听）。
    NotRunning,
    Other(anyhow::Error),
}

/// 发一个请求并等回答。
pub fn request(message: &Value, timeout: Duration) -> Result<Value, ClientError> {
    let path = socket_path().map_err(ClientError::Other)?;
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(ClientError::NotRunning);
        }
        Err(err) => {
            return Err(ClientError::Other(
                anyhow::Error::new(err).context("connect to Helmp"),
            ));
        }
    };
    let result = (|| {
        stream.set_read_timeout(Some(timeout))?;
        write_message(&mut stream, message)?;
        read_message(&stream)
    })();
    result.map_err(|err| ClientError::Other(err.context("talk to Helmp")))
}

// ---------------------------------------------------------------- 服务端（图形界面）

enum UiRequest {
    Confirm(Box<Item>),
    OpenPlugin(String),
}

/// 在图形界面里监听套接字。失败只打日志（agent 拿到「Helmp 没在运行」），不影响启动器本身。
pub fn serve(cx: &mut gpui_kit::App) {
    let engine = cx.global::<crate::engine::EngineGlobal>().0.clone();
    let (tx, rx) = mpsc::channel::<UiRequest>();
    if let Err(err) = listen(engine, tx) {
        eprintln!("helmp: agent 确认通道启动失败：{err:#}");
        return;
    }
    cx.spawn(async move |cx: &mut gpui_kit::AsyncApp| {
        loop {
            while let Ok(request) = rx.try_recv() {
                cx.update(|cx| handle_ui(request, cx));
            }
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
        }
    })
    .detach();
}

fn handle_ui(request: UiRequest, cx: &mut gpui_kit::App) {
    match request {
        UiRequest::Confirm(item) => {
            crate::launcher::LauncherHandle::update(cx, |launcher, window, cx| {
                launcher.present(*item, window, cx)
            })
        }
        UiRequest::OpenPlugin(id) => {
            crate::providers::plugins::registry::reload();
            crate::settings::reveal_plugin(&id);
            cx.defer(|cx| crate::settings::open_page(cx, crate::settings::Page::Plugins));
        }
    }
}

fn listen(engine: crate::engine::Engine, tx: mpsc::Sender<UiRequest>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let path = socket_path()?;
    // 上次没退干净留下的套接字文件。
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).with_context(|| format!("bind {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod {}", path.display()))?;
    std::thread::Builder::new()
        .name("helmp-agent-ipc".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let (engine, tx) = (engine.clone(), tx.clone());
                std::thread::spawn(move || {
                    if let Err(err) = serve_connection(stream, &engine, &tx) {
                        eprintln!("helmp: agent 请求处理失败：{err:#}");
                    }
                });
            }
        })
        .context("spawn agent ipc thread")?;
    Ok(())
}

/// 对端是不是当前用户（套接字已是 0600，这里再核对一次）。
fn same_user(stream: &UnixStream) -> bool {
    use std::os::fd::AsRawFd as _;
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: fd 在 stream 存活期间有效，uid / gid 是有效的可写指针。
    let ok = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } == 0;
    // SAFETY: getuid 没有前置条件。
    ok && uid == unsafe { libc::getuid() }
}

fn serve_connection(
    mut stream: UnixStream,
    engine: &crate::engine::Engine,
    tx: &mpsc::Sender<UiRequest>,
) -> Result<()> {
    if !same_user(&stream) {
        bail!("rejected a connection from another user");
    }
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let message = read_message(&stream)?;
    let reply = match message.get("type").and_then(Value::as_str) {
        Some("run") => handle_run(&message, engine, tx),
        Some("open_plugin") => match message.get("plugin").and_then(Value::as_str) {
            Some(id) => {
                let _ = tx.send(UiRequest::OpenPlugin(id.to_string()));
                json!({"ok": true})
            }
            None => json!({"ok": false, "error": "missing plugin"}),
        },
        other => json!({"ok": false, "error": format!("unknown request type {other:?}")}),
    };
    write_message(&mut stream, &reply)?;
    Ok(())
}

/// 等待中的确认：回复通道只能被拿走一次（允许、拒绝、超时三者之一）。
struct Pending {
    reply: Mutex<Option<mpsc::Sender<Value>>>,
    deadline: Instant,
}

impl Pending {
    fn take(&self) -> Option<mpsc::Sender<Value>> {
        self.reply
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

/// 确认页被关掉（Esc、被别的结果顶掉）时自动回「拒绝」：确认页的两个动作各持有一份，全部释放时触发。
struct DenyOnDrop(Arc<Pending>);

impl Drop for DenyOnDrop {
    fn drop(&mut self) {
        if let Some(reply) = self.0.take() {
            let _ = reply.send(json!({"ok": true, "decision": "denied"}));
        }
    }
}

fn handle_run(
    message: &Value,
    engine: &crate::engine::Engine,
    tx: &mpsc::Sender<UiRequest>,
) -> Value {
    let text = |key: &str| {
        message
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let (agent, query, id) = (text("agent"), text("query"), text("id"));
    let agent = super::server::agent_display_name(&agent);
    let form = super::operate::string_map(message.get("form"));
    let action_title = message
        .get("action")
        .and_then(Value::as_str)
        .map(str::to_string);
    if super::operate::is_private(&id) && !crate::config::get().mcp_server.expose_private {
        return json!({"ok": false, "error": "this result is private (clipboard history, agent sessions or recently used items)"});
    }
    let scope = message
        .get("scope")
        .and_then(Value::as_str)
        .filter(|scope| !scope.is_empty());
    let Some(item) = crate::cli::find(engine, &id, &query, scope) else {
        return json!({"ok": false, "error": format!("no result {id} for query {query:?} in Helmp")});
    };
    let (title, action) = match super::operate::pick_action(&item, action_title.as_deref()) {
        Ok(found) => found,
        Err(err) => return json!({"ok": false, "error": format!("{err:#}")}),
    };
    let input = super::operate::fill_form(&item, &form);

    let (reply_tx, reply_rx) = mpsc::channel();
    let pending = Arc::new(Pending {
        reply: Mutex::new(Some(reply_tx)),
        deadline: Instant::now() + CONFIRM_TIMEOUT,
    });
    let confirm = confirm_item(
        &agent,
        &query,
        &item,
        &title,
        &input,
        action,
        Arc::clone(&pending),
    );
    if tx.send(UiRequest::Confirm(Box::new(confirm))).is_err() {
        return json!({"ok": false, "error": "Helmp's window isn't available"});
    }
    match reply_rx.recv_timeout(CONFIRM_TIMEOUT) {
        Ok(reply) => reply,
        Err(_) => match pending.take() {
            // 没人回答：作废，之后再按回车也不执行。
            Some(_) => json!({"ok": true, "decision": "timeout"}),
            // 用户在最后一刻允许了：等动作跑完。
            None => reply_rx.recv_timeout(RUN_TIMEOUT).unwrap_or_else(
                |_| json!({"ok": false, "error": "the action didn't finish in time"}),
            ),
        },
    }
}

/// 面板里的确认页：一个不进「最近使用」的项，执行时展示确认详情。
fn confirm_item(
    agent: &str,
    query: &str,
    item: &Item,
    action_title: &str,
    input: &crate::provider::ActionInput,
    action: Action,
    pending: Arc<Pending>,
) -> Item {
    let guard = Arc::new(DenyOnDrop(pending));
    let allow = {
        let (guard, input) = (Arc::clone(&guard), input.clone());
        Action::new(move || {
            let pending = &guard.0;
            let reply = match pending.take() {
                Some(reply) if Instant::now() <= pending.deadline => reply,
                _ => {
                    return Ok(Outcome::Message(
                        tr(
                            "这个请求已过期，没有执行。",
                            "This request has expired; nothing was run.",
                        )
                        .into(),
                    ));
                }
            };
            let result = action.run_with(&input);
            let message = match &result {
                Ok(outcome) => json!({"ok": true, "decision": "allowed", "failed": false,
                    "output": crate::cli::outcome_text(outcome.clone())}),
                Err(err) => {
                    json!({"ok": true, "decision": "allowed", "failed": true, "output": format!("{err:#}")})
                }
            };
            let _ = reply.send(message);
            result
        })
    };
    let deny = {
        let guard = Arc::clone(&guard);
        Action::new(move || {
            if let Some(reply) = guard.0.take() {
                let _ = reply.send(json!({"ok": true, "decision": "denied"}));
            }
            Ok(Outcome::Dismiss)
        })
        .keep_open()
    };
    drop(guard);

    let mut blocks = Vec::new();
    if !item.subtitle.is_empty() {
        blocks.push(Block::Text(item.subtitle.clone()));
    }
    if !query.is_empty() {
        blocks.push(Block::Quote {
            label: tr("搜索", "Query").into(),
            text: query.to_string(),
        });
    }
    if !input.form.is_empty() {
        let fields: Vec<String> = input
            .form
            .iter()
            .map(|(k, v)| format!("{k} = {v}"))
            .collect();
        blocks.push(Block::Code(fields.join("\n")));
    }
    blocks.push(Block::Text(
        tr(
            "回车允许执行，Esc 拒绝。60 秒内不处理视为拒绝。",
            "Press Return to allow, Esc to deny. No answer within 60 seconds counts as denied.",
        )
        .into(),
    ));
    let detail = Detail {
        icon: item.icon.clone(),
        title: trf!(
            "{agent} 想执行：{action_title}「{}」",
            "{agent} wants to run: {action_title} “{}”",
            item.title
        ),
        tag: Some((
            tr("需要你确认", "Needs your approval").into(),
            Tone::Waiting,
        )),
        blocks,
        metadata: vec![(tr("来自", "From").into(), agent.to_string())],
        actions: vec![
            NamedAction::new(tr("允许", "Allow"), None, allow),
            NamedAction::new(tr("拒绝", "Deny"), Some("Esc"), deny),
        ],
    };
    // 确认页只展示一次：重新执行这一项不会再弹出同一个请求。
    let slot = Mutex::new(Some(detail));
    Item {
        id: "mcp:confirm".into(),
        title: trf!("{agent} 的请求", "Request from {agent}"),
        subtitle: format!("{action_title} · {}", item.title),
        badge: "Agent".into(),
        icon: Icon::Symbol {
            name: "sparkles",
            tint: Tint::Purple,
        },
        action: Action::new(move || {
            Ok(
                match slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                {
                    Some(detail) => Outcome::Show(detail),
                    None => Outcome::Message(
                        tr(
                            "这个请求已经处理过了。",
                            "This request was already handled.",
                        )
                        .into(),
                    ),
                },
            )
        })
        .keep_open(),
        action_title: tr("查看", "Review").into(),
        ..Item::blank()
    }
}

/// 图形自检（`HELMP_SELF_TEST_AGENT_CONFIRM=<查询>|<结果 id>`）：不经套接字，直接弹出确认页，供离屏截图检查。
pub fn present_for_self_test(spec: &str, cx: &mut gpui_kit::App) {
    let (query, id) = spec.split_once('|').unwrap_or((spec, ""));
    let engine = cx.global::<crate::engine::EngineGlobal>().0.clone();
    let Some(item) = crate::cli::lookup(&engine, query)
        .into_iter()
        .find(|item| id.is_empty() || item.id == id)
    else {
        println!("self-test agent-confirm: 查询 {query:?} 没有结果 {id}");
        return;
    };
    let (reply, _rx) = mpsc::channel();
    let pending = Arc::new(Pending {
        reply: Mutex::new(Some(reply)),
        deadline: Instant::now() + CONFIRM_TIMEOUT,
    });
    let title = item.action_title.clone();
    let action = item.action.clone();
    let confirm = confirm_item(
        "Claude Code",
        query,
        &item,
        &title,
        &Default::default(),
        action,
        pending,
    );
    handle_ui(UiRequest::Confirm(Box::new(confirm)), cx);
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn messages_are_one_json_line_each_way() {
        // 一问一答各一行：多出来的数据（或没有换行的半截消息）不能被当成请求执行。
        let (mut a, b) = UnixStream::pair().unwrap();
        write_message(&mut a, &json!({"type": "run", "query": "1+1\nnext"})).unwrap();
        let got = read_message(&b).unwrap();
        assert_eq!(
            got["query"], "1+1\nnext",
            "换行要在 JSON 字符串里转义，不能拆成两条消息"
        );

        let (mut a, b) = UnixStream::pair().unwrap();
        a.write_all(br#"{"type":"run""#).unwrap();
        drop(a);
        assert!(
            read_message(&b).is_err(),
            "没有换行的半截消息不能当成完整请求"
        );

        let (a, b) = UnixStream::pair().unwrap();
        drop(a);
        assert!(read_message(&b).is_err());
    }

    #[test]
    fn oversized_messages_are_rejected() {
        let (mut a, b) = UnixStream::pair().unwrap();
        let writer = std::thread::spawn(move || {
            let chunk = vec![b'x'; 64 * 1024];
            for _ in 0..20 {
                if a.write_all(&chunk).is_err() {
                    break;
                }
            }
        });
        assert!(read_message(&b).is_err());
        drop(b);
        writer.join().unwrap();
    }

    #[test]
    fn closing_the_confirmation_denies_and_late_approval_does_nothing() {
        // Esc（确认页被丢弃）要立刻回「拒绝」；超时后被拿走回复通道的请求，再按「允许」也不能执行。
        let (tx, rx) = mpsc::channel();
        let pending = Arc::new(Pending {
            reply: Mutex::new(Some(tx)),
            deadline: Instant::now() + CONFIRM_TIMEOUT,
        });
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        let action = Action::new(move || {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(Outcome::Message("did it".into()))
        });
        let target = Item {
            id: "x:y".into(),
            title: "Thing".into(),
            ..Item::blank()
        };
        let item = confirm_item(
            "Codex",
            "q",
            &target,
            "Open",
            &Default::default(),
            action,
            Arc::clone(&pending),
        );
        let Outcome::Show(detail) = item.action.run().unwrap() else {
            panic!("confirm page expected")
        };
        assert!(detail.title.contains("Codex"), "{}", detail.title);
        drop(detail);
        drop(item);
        assert_eq!(rx.try_recv().unwrap()["decision"], "denied");
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));

        // 允许：执行并把结果文字传回。
        let (tx, rx) = mpsc::channel();
        let pending = Arc::new(Pending {
            reply: Mutex::new(Some(tx)),
            deadline: Instant::now() + CONFIRM_TIMEOUT,
        });
        let flag = Arc::clone(&ran);
        let action = Action::new(move || {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(Outcome::Message("did it".into()))
        });
        let item = confirm_item(
            "Codex",
            "q",
            &target,
            "Open",
            &Default::default(),
            action,
            Arc::clone(&pending),
        );
        let Outcome::Show(detail) = item.action.run().unwrap() else {
            panic!()
        };
        // 超时：回复通道已被监听线程拿走。
        let late = Arc::clone(&pending);
        assert!(late.take().is_some());
        let out = detail.actions[0].action.run().unwrap();
        assert!(
            matches!(out, Outcome::Message(m) if m != "did it"),
            "超时后只能提示过期"
        );
        assert!(
            !ran.load(std::sync::atomic::Ordering::SeqCst),
            "超时后的允许不能执行"
        );
        drop(rx);

        let (tx, rx) = mpsc::channel();
        let pending = Arc::new(Pending {
            reply: Mutex::new(Some(tx)),
            deadline: Instant::now() + CONFIRM_TIMEOUT,
        });
        let flag = Arc::clone(&ran);
        let action = Action::new(move || {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(Outcome::Message("did it".into()))
        });
        let item = confirm_item(
            "Codex",
            "q",
            &target,
            "Open",
            &Default::default(),
            action,
            pending,
        );
        let Outcome::Show(detail) = item.action.run().unwrap() else {
            panic!()
        };
        detail.actions[0].action.run().unwrap();
        let reply = rx.try_recv().unwrap();
        assert_eq!(reply["decision"], "allowed");
        assert_eq!(reply["output"], "did it");
        assert!(ran.load(std::sync::atomic::Ordering::SeqCst));
    }
}
