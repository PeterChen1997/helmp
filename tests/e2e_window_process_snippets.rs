//! 窗口管理、进程与端口、文本片段、剪贴板增强的端到端测试：真实的 helmp 可执行文件 + 隔离环境
//! （见 tests/support）。全部在演练模式下跑：不移动用户的窗口、不碰真实剪贴板、不结束别人的进程。
//! 唯一真正发信号的用例只结束测试自己起的 `sleep`。

mod support;

use std::net::TcpListener;
use std::process::{Child, Command};

use serde_json::json;
use support::Helmp;

/// 两块屏：13 寸主屏（菜单栏 25、Dock 75）+ 右侧外接屏（比主屏高出 300）。窗口在主屏上。
const SCREENS: &str = r#"{"screens":[
  {"frame":[0,0,1440,900],"visible":[0,25,1440,800]},
  {"frame":[1440,-300,2560,1440],"visible":[1440,-275,2560,1415]}
], "window":[100,100,800,600]}"#;

fn with_window(fixture: &str) -> Helmp {
    Helmp::new().env("HELMP_WINDOW_FIXTURE", fixture)
}

#[test]
fn window_commands_compute_the_target_rect_on_the_right_screen() {
    // 窗口管理的核心是算对位置：要躲开菜单栏和 Dock，跨屏时按目标屏的可见区域摆。
    // 用夹具屏幕代替真实窗口，断言演练输出的目标矩形，绝不移动用户的窗口。
    let h = with_window(SCREENS);
    let row = h.find("zbp", |r| r.id == "window:left-half");
    assert_eq!(row.badge, "Window");
    assert_eq!(
        h.run("window:left-half", "zbp", &[]).trim(),
        "[dry-run] window left-half 0,25 720x800"
    );
    assert_eq!(
        h.run("window:maximize", "maximize", &[]).trim(),
        "[dry-run] window maximize 0,25 1440x800"
    );
    assert_eq!(
        h.run("window:center", "jz", &[]).trim(),
        "[dry-run] window center 320,125 800x600"
    );
    assert_eq!(
        h.run("window:last-third", "right third", &[]).trim(),
        "[dry-run] window last-third 960,25 480x800"
    );
    // 移到下一个显示器：按比例搬到外接屏的可见区域。
    assert_eq!(
        h.run("window:next-display", "next display", &[]).trim(),
        "[dry-run] window next-display 1618,-142 1422x1061"
    );
}

#[test]
fn window_on_external_display_lays_out_within_that_display() {
    // 窗口在外接屏上时，「右半屏」是外接屏的右半边，不是主屏的。
    let h = with_window(
        r#"{"screens":[{"frame":[0,0,1440,900],"visible":[0,25,1440,800]},
            {"frame":[1440,-300,2560,1440],"visible":[1440,-275,2560,1415]}],
            "window":[2000,0,600,400]}"#,
    );
    assert_eq!(
        h.run("window:right-half", "right half", &[]).trim(),
        "[dry-run] window right-half 2720,-275 1280x1415"
    );
}

#[test]
fn window_restore_and_single_display_fail_loudly() {
    // 做不了的事要说原因（没调整过就没得恢复、只有一块屏），不能按了回车什么都不发生。
    let h = with_window(
        r#"{"screens":[{"frame":[0,0,1440,900],"visible":[0,25,1440,800]}],"window":[10,40,500,400]}"#,
    );
    let err = h.cli_err(&["run", "window:restore", "restore"]);
    assert!(err.contains("nothing to restore"), "{err}");
    let err = h.cli_err(&["run", "window:next-display", "next display"]);
    assert!(err.contains("Only one display"), "{err}");
    let h = with_window(
        r#"{"screens":[{"frame":[0,0,1440,900],"visible":[0,25,1440,800]}],"window":[10,40,500,400],"restore":[1,2,3,4]}"#,
    );
    assert_eq!(
        h.run("window:restore", "restore", &[]).trim(),
        "[dry-run] window restore 1,2 3x4"
    );
}

#[test]
fn window_without_accessibility_explains_and_opens_settings() {
    // 没有辅助功能权限时静默失败最糟：列表里要标出来，回车给说明，确认后直达那一页系统设置。
    let h = with_window(
        r#"{"screens":[{"frame":[0,0,1440,900],"visible":[0,25,1440,800]}],"window":[0,0,10,10],"trusted":false}"#,
    );
    let row = h.find("zdh", |r| r.id == "window:maximize");
    assert!(row.subtitle.contains("Accessibility"), "{row:?}");
    let out = h.run("window:maximize", "zdh", &["--then", "Open Accessibility Settings"]);
    assert!(out.contains("Accessibility Permission Needed"), "{out}");
    assert!(!out.contains("window maximize"), "没授权不能去算 / 动窗口：{out}");
    assert!(
        out.contains(
            "[dry-run] open x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        ),
        "{out}"
    );
}

/// 测试自己起的 `sleep`，结束时一定收掉（不管断言是否通过）。
struct Sleeper(Child);

impl Sleeper {
    fn spawn() -> Self {
        Self(Command::new("/bin/sleep").arg("600").spawn().expect("spawn sleep"))
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn process_is_found_by_name_and_quit_sends_sigterm() {
    // 「kill sleep」要找到这个进程；回车是温和的 SIGTERM，给进程收尾的机会。
    let h = Helmp::new();
    let child = Sleeper::spawn();
    let id = format!("processes:{}", child.pid());
    let row = h.find("kill sleep", |r| r.id == id);
    assert_eq!(row.title, "sleep");
    assert!(row.subtitle.starts_with(&format!("pid {} · ", child.pid())), "{row:?}");
    assert_eq!(
        h.run(&id, "kill sleep", &[]).trim(),
        format!("[dry-run] kill SIGTERM {} sleep", child.pid())
    );
    assert_eq!(
        h.run(&id, "kill sleep", &["--action", "Copy PID"]).trim(),
        format!("[dry-run] copy {}", child.pid())
    );
}

#[test]
fn force_quit_asks_first_then_sends_sigkill() {
    // SIGKILL 不给进程保存的机会：第一次只出确认页，确认后才发，并且发给的是这个 pid。
    let h = Helmp::new();
    let child = Sleeper::spawn();
    let id = format!("processes:{}", child.pid());
    let first = h.run(&id, "进程 sleep", &["--action", "Force Quit"]);
    assert!(first.contains("Force quit “sleep”?"), "{first}");
    assert!(!first.contains("[dry-run]"), "确认前不能发信号：{first}");
    let confirmed = h.run(&id, "进程 sleep", &["--action", "Force Quit", "--then", "Force Quit"]);
    assert!(
        confirmed.contains(&format!("[dry-run] kill SIGKILL {} sleep", child.pid())),
        "{confirmed}"
    );
}

#[test]
fn quitting_for_real_stops_the_test_owned_child() {
    // 演练之外也要真的生效：只对测试自己起的 sleep 关掉演练，断言它被 SIGTERM 结束。
    use std::os::unix::process::ExitStatusExt as _;
    let h = Helmp::new().env("HELMP_DRY_RUN", "0");
    let mut child = Sleeper::spawn();
    let id = format!("processes:{}", child.pid());
    let out = h.run(&id, "kill sleep", &[]);
    assert_eq!(out.trim(), "done");
    let status = child.0.wait().expect("wait sleep");
    assert_eq!(status.signal(), Some(15), "{status:?}");
}

#[test]
fn port_lookup_puts_the_listener_first() {
    // 「port 3000」是在问谁占着端口：占用它的进程要排第一，结束它发给的是那个 pid。
    // 监听者是测试进程自己，只在演练模式下「结束」。
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let me = std::process::id();
    let h = Helmp::new();
    let rows = h.query(&format!("port {port}"));
    let first = rows.first().unwrap_or_else(|| panic!("没有结果：{rows:#?}"));
    assert_eq!(first.id, format!("processes:port:{port}:{me}"), "{rows:#?}");
    assert!(first.title.starts_with(&format!(":{port} · ")), "{first:?}");
    assert_eq!(first.badge, "Ports");
    let out = h.run(&first.id, &format!("port {port}"), &[]);
    assert!(out.starts_with(&format!("[dry-run] kill SIGTERM {me} ")), "{out}");
    // 中文写法和范围里的 :端口 也一样。
    h.find(&format!("端口 {port}"), |r| r.id == first.id);
    h.find(&format!("kill :{port}"), |r| r.id == first.id);
    drop(listener);
}

#[test]
fn free_port_says_nothing_is_listening() {
    // 没人占用时要明说，否则用户分不清是没占用还是功能坏了。
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let h = Helmp::new();
    let row = h.find(&format!("port {port}"), |r| r.id == "processes:port-none");
    assert_eq!(row.title, format!("Nothing is listening on port {port}"));
}

fn snippets_fixture(h: &Helmp) {
    h.write(
        "data/snippets.json",
        &json!({"snippets": [
            {"id": "sig", "name": "签名", "keyword": "", "content": "— 陈 {date}", "tags": []},
            {"id": "reply", "name": "回信", "keyword": "hx", "tags": ["邮件"],
             "content": "Dear {argument name=\"name\"},\n{cursor}\n{snippet name=\"签名\"}"},
            {"id": "quote", "name": "引用剪贴板", "keyword": "", "tags": [],
             "content": "> {clipboard | trim}"}
        ]})
        .to_string(),
    );
}

fn snippet_env() -> Helmp {
    Helmp::new()
        .env("HELMP_NOW", "2026-11-10T09:30:00")
        .env("TZ", "UTC")
}

#[test]
fn snippet_found_by_pinyin_initials_pastes_expanded_text() {
    // 片段的主路径：拼音首字母找到「签名」，回车粘贴展开后的文字（日期按固定的「现在」）。
    let h = snippet_env();
    snippets_fixture(&h);
    let row = h.find("sn qm", |r| r.title == "签名");
    assert_eq!(row.id, "snippets:sig");
    assert_eq!(row.badge, "Snippets");
    assert_eq!(
        h.run("snippets:sig", "sn qm", &[]).trim(),
        "[dry-run] paste — 陈 2026-11-10"
    );
    assert_eq!(
        h.run("snippets:sig", "sn qm", &["--action", "Copy"]).trim(),
        "[dry-run] copy — 陈 2026-11-10"
    );
}

#[test]
fn snippet_arguments_cursor_and_nested_snippets() {
    // 表单参数要填进正文，{cursor} 要算出粘贴后退回几格，引用的片段要展开。
    let h = snippet_env();
    snippets_fixture(&h);
    let row = h.find("片段 hx", |r| r.id == "snippets:reply");
    assert_eq!(row.title, "回信");
    let out = h.run("snippets:reply", "片段 hx", &["--form", "name=Ada"]);
    assert_eq!(
        out.trim_end(),
        "[dry-run] paste Dear Ada,\n\n— 陈 2026-11-10 [cursor ←15]"
    );
}

#[test]
fn snippet_clipboard_comes_from_fixture_never_the_real_clipboard() {
    // 测试绝不能读用户的剪贴板：没给夹具时 {clipboard} 原样留着；给了夹具就用夹具。
    let h = snippet_env();
    snippets_fixture(&h);
    let out = h.run("snippets:quote", "sn yy", &[]);
    assert_eq!(out.trim(), "[dry-run] paste > {clipboard | trim}");
    let fixture = h.write("clipboard.txt", "  copied text \n");
    let h = h.env("HELMP_CLIPBOARD_FIXTURE", fixture.to_string_lossy());
    let out = h.run("snippets:quote", "sn yy", &[]);
    assert_eq!(out.trim(), "[dry-run] paste > copied text");
}

#[test]
fn create_snippet_from_the_launcher_then_find_it() {
    // 在启动器里就能新建：保存到数据目录，马上能按关键词搜到；关键词重复要拒绝并说明。
    let h = snippet_env();
    let out = h.run(
        "snippets:new",
        "新建片段",
        &["--form", "name=周报", "--form", "keyword=zb", "--form", "content=本周 {date}", "--form", "tags=工作"],
    );
    assert!(out.contains("Saved the snippet “周报”"), "{out}");
    let saved = std::fs::read_to_string(h.data_file("snippets.json")).unwrap();
    assert!(saved.contains("本周 {date}"), "{saved}");
    let first = h.query("sn zb").into_iter().next().expect("有结果");
    assert_eq!(first.title, "周报");
    let err = h.cli_err(&[
        "run", "snippets:new", "新建片段", "--form", "name=别的", "--form", "keyword=ZB", "--form", "content=x",
    ]);
    assert!(err.contains("already used by “周报”"), "{err}");
}

#[test]
fn delete_snippet_needs_confirmation() {
    // 删除不可恢复：第一次只出确认页，确认后才从文件里删掉。
    let h = snippet_env();
    snippets_fixture(&h);
    let first = h.run("snippets:sig", "sn qm", &["--action", "Delete"]);
    assert!(first.contains("Delete the snippet “签名”?"), "{first}");
    assert!(std::fs::read_to_string(h.data_file("snippets.json")).unwrap().contains("签名"));
    h.run("snippets:sig", "sn qm", &["--action", "Delete", "--then", "Delete"]);
    let left = std::fs::read_to_string(h.data_file("snippets.json")).unwrap();
    assert!(!left.contains("\"签名\""), "{left}");
}

#[test]
fn import_raycast_export_once() {
    // 从 Raycast 迁移：导入它导出的 JSON；同一个文件再导一次不能出现两份。
    let h = snippet_env();
    let file = h.write(
        "raycast.json",
        r#"[{"name":"Email","text":"me@example.com","keyword":"@@"},{"name":"Thanks","text":"Thanks, {cursor}!"}]"#,
    );
    let path = file.to_string_lossy().into_owned();
    let out = h.run("snippets:import", "导入片段", &["--form", &format!("path={path}")]);
    assert!(out.contains("Imported 2 snippets, skipped 0"), "{out}");
    h.find("sn @@", |r| r.title == "Email");
    let again = h.run("snippets:import", "import snippets", &["--form", &format!("path={path}")]);
    assert!(again.contains("Imported 0 snippets, skipped 2"), "{again}");
}

/// 按老版本的表结构（没有 pinned 列）直接写剪贴板库，模拟升级前的历史。
fn seed_clipboard(h: &Helmp, rows: &[(&str, i64)]) {
    let conn = rusqlite::Connection::open(h.data_file("clipboard.sqlite")).unwrap();
    conn.execute_batch(
        "CREATE TABLE clips (id INTEGER PRIMARY KEY, kind TEXT NOT NULL, content TEXT NOT NULL,
         hash TEXT NOT NULL UNIQUE, width INTEGER NOT NULL DEFAULT 0, height INTEGER NOT NULL DEFAULT 0,
         source_name TEXT, source_bundle TEXT, copied_at INTEGER NOT NULL, seq INTEGER NOT NULL);",
    )
    .unwrap();
    for (seq, (text, copied_at)) in rows.iter().enumerate() {
        conn.execute(
            "INSERT INTO clips (id, kind, content, hash, copied_at, seq) VALUES (?1, 'text', ?2, ?3, ?4, ?1)",
            rusqlite::params![seq as i64 + 1, text, format!("hash-{seq}"), copied_at],
        )
        .unwrap();
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[test]
fn pinned_clip_lists_first_and_survives_retention() {
    // 置顶的意思是「一直留着、一眼看到」：排在最新记录前面，保留天数到期也不删；没置顶的照常清理。
    let h = Helmp::new();
    let old = now_ms() - 10 * 86_400_000;
    seed_clipboard(&h, &[("old pinned", old), ("old plain", old), ("fresh", now_ms())]);
    let rows = h.query("cb ");
    let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(titles, ["fresh", "old plain", "old pinned"]);
    assert_eq!(h.run("clipboard:1", "cb ", &["--action", "Pin"]).trim(), "done");
    let rows = h.query("cb ");
    assert_eq!(rows[0].title, "old pinned");
    assert_eq!(rows[0].badge, "Pinned");

    let h = h.config(|c| c["clipboard"]["retention_days"] = json!(7));
    let titles: Vec<String> = h.query("cb ").into_iter().map(|r| r.title).collect();
    assert_eq!(titles, ["old pinned", "fresh"], "过期的未置顶记录要清掉，置顶的留下");
    // 取消置顶后回到按时间排（下一次清理就会删它）。
    h.run("clipboard:1", "cb ", &["--action", "Unpin"]);
    let titles: Vec<String> = h.query("cb ").into_iter().map(|r| r.title).collect();
    assert_eq!(titles, ["fresh"]);
}

#[test]
fn clipboard_plain_text_and_sequence_paste() {
    // 「粘贴为纯文本」是已有功能，回归一下；合并粘贴要按复制先后（旧的在前）把这一段都粘出来。
    let h = Helmp::new();
    let t = now_ms();
    seed_clipboard(&h, &[("first", t - 3000), ("second", t - 2000), ("third", t - 1000)]);
    assert_eq!(
        h.run("clipboard:2", "cb ", &["--action", "Paste as Plain Text"]).trim(),
        "[dry-run] paste 2 PlainText"
    );
    assert_eq!(
        h.run("clipboard:1", "cb ", &["--action", "Paste Through Latest in Copy Order"]).trim(),
        "[dry-run] paste-sequence 1 2 3"
    );
}
