//! Agent 会话端到端测试：在隔离的 CLAUDE_CONFIG_DIR 里伪造一个「正在等你」的 Claude Code 会话，
//! 确认它能被搜到、标出在等什么，回车跳回会话（演练模式只回报跳转计划）。

mod support;

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
