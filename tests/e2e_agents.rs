//! Agent 会话端到端测试：在隔离的 CLAUDE_CONFIG_DIR 里伪造一个「正在等你」的 Claude Code 会话，
//! 确认它能被搜到、标出在等什么，回车跳回会话（演练模式只回报跳转计划）。

mod support;

use std::fs::File;
use std::os::fd::AsRawFd as _;
use std::process::{Child, Command};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use support::Helmp;

/// 测试结束时结束充当会话进程的 sleep。
struct Session(Child);

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn now_ms() -> u64 {
    u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()).unwrap()
}

fn iso(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(i64::try_from(ms).unwrap())
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// 写一个活着的 Claude Code 会话：注册表指向一个真实存在的进程，转录里有最后一条提问。
fn claude_session(h: &Helmp, prompt: &str, waiting_for: &str) -> Session {
    let child = Command::new("sleep").arg("120").spawn().unwrap();
    let pid = child.id();
    let sid = "aaaaaaaa-0001-4000-8000-000000000001";
    let cwd = h.home.join("code/billing-api");
    std::fs::create_dir_all(&cwd).unwrap();
    let at = now_ms() - 60_000;
    let registry = json!({
        "pid": pid, "sessionId": sid, "cwd": cwd, "startedAt": at, "version": "2.1.309",
        "kind": "interactive", "entrypoint": "cli", "status": "waiting",
        "waitingFor": waiting_for, "updatedAt": at, "statusUpdatedAt": at
    });
    h.write(&format!("claude/sessions/{pid}.json"), &registry.to_string());
    let lines = [
        json!({"type": "user", "message": {"role": "user", "content": prompt}, "uuid": "u1",
               "timestamp": iso(at - 120_000), "cwd": cwd, "sessionId": sid}),
        json!({"type": "assistant", "message": {"role": "assistant", "content": [
                   {"type": "text", "text": "Ready to apply the migration."},
                   {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "pnpm prisma migrate deploy"}}]},
               "uuid": "a1", "timestamp": iso(at), "cwd": cwd, "sessionId": sid}),
        json!({"type": "last-prompt", "lastPrompt": prompt, "leafUuid": "a1", "sessionId": sid}),
    ];
    let slug = format!("-{}", cwd.to_string_lossy().trim_start_matches('/').replace('/', "-"));
    let body: Vec<String> = lines.iter().map(ToString::to_string).collect();
    h.write(&format!("claude/projects/{slug}/{sid}.jsonl"), &(body.join("\n") + "\n"));
    Session(child)
}

#[test]
fn waiting_claude_session_is_found_and_jumped_to() {
    // 这是 Helmp 的招牌功能：在等你批准的 Claude Code 会话要能搜到、标明在等什么，回车跳回去。
    let h = Helmp::new();
    let _session = claude_session(&h, "Migrate the invoices table", "permission prompt");
    let row = h.find("ag migrate", |r| r.title.contains("Migrate the invoices table"));
    assert!(row.subtitle.contains("billing-api"), "{row:?}");
    let out = h.run(&row.id, "ag migrate", &[]);
    assert!(out.contains("Would run") || out.contains("[dry-run]"), "跳转在演练模式下只能回报计划：{out}");
}

#[test]
fn ended_sessions_are_not_listed() {
    // 会话进程已经退出的注册表是残留，列出来会让用户跳到一个不存在的会话。
    let h = Helmp::new();
    let session = claude_session(&h, "Refactor the auth module", "permission prompt");
    drop(session);
    let rows = h.query("ag refactor");
    assert!(
        !rows.iter().any(|r| r.title.contains("Refactor the auth module")),
        "{rows:#?}"
    );
}

fn codex_reply(h: &Helmp, id: &str, title: &str, turn: &str) {
    let events = [
        json!({"type": "session_meta", "payload": {"originator": "Codex Desktop"}}),
        json!({"type": "event_msg", "payload": {"type": "item_completed", "item": {
            "type": "UserMessage", "content": [{"text": title}]
        }}}),
        json!({"type": "event_msg", "payload": {"type": "task_complete", "turn_id": turn,
            "last_agent_message": "**完成。**\n\n1. 查看结果"
        }}),
    ];
    let body = events
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    h.write(
        &format!("codex/sessions/2026/11/11/rollout-{id}.jsonl"),
        &body,
    );
}

fn codex_writer(h: &Helmp, id: &str) -> File {
    let lock = File::open(h.write(&format!("codex/thread-writer-locks/{id}.lock"), "")).unwrap();
    // SAFETY: 文件在测试结束前保持打开，模拟 Codex 持有写锁。
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    lock
}

#[test]
fn completed_sessions_move_back_until_the_next_turn_finishes() {
    let h = Helmp::new();
    let older = "aaaaaaaa-0001-4000-8000-000000000001";
    let newer = "aaaaaaaa-0001-4000-8000-000000000002";
    let _writers = [codex_writer(&h, older), codex_writer(&h, newer)];
    codex_reply(&h, older, "Session B (earlier)", "one");
    codex_reply(&h, newer, "Session B", "one");
    // B 的标题完全匹配查询：即便匹配分更高，看过的完成项也必须让给未读项。
    h.write(
        "data/agent-completions.json",
        &json!({(format!("agents:codex:{newer}")): "done:one"}).to_string(),
    );
    let titles = || {
        h.query_in("agents", "Session B")
            .into_iter()
            .map(|row| row.title)
            .collect::<Vec<_>>()
    };
    assert_eq!(titles(), ["Session B (earlier)", "Session B"]);

    // 同一个会话完成了新一轮，旧回执不再适用。
    codex_reply(&h, newer, "Session B", "two");
    assert_eq!(titles(), ["Session B", "Session B (earlier)"]);
}
