//! 便签端到端回归测试：经由真实 `helmp` 进程与 `main.rs` 退出钩子验证防抖期退出不丢正文。

mod support;

use serde_json::json;
use support::Helmp;

#[test]
fn quitting_within_500ms_debounce_flushes_editor_text_and_reloads_latest() {
    // 为什么测这条链路：用户刚在便签编辑器里输入内容、尚未触发 500ms 防抖保存就立即退出应用时，
    // main.rs 的 `on_app_quit` 必须同步调用 `flush_on_quit` → `leave_edit_mode_for_quit` → `store::save`
    // 将编辑器中的最新正文冲刷到磁盘；否则最后输入的正文会在进程终止瞬间丢失。
    let h = Helmp::new();
    let notes_path = h.data_file("notes.json");
    let seed = json!({
        "notes": [{
            "id": "note-quit-e2e",
            "text": "草稿 v1",
            "created_at": 100,
            "updated_at": 100,
            "placement": {
                "kind": "floating",
                "frame": { "x": 120.0, "y": 120.0, "w": 280.0, "h": 200.0 }
            }
        }]
    });
    std::fs::write(&notes_path, serde_json::to_string_pretty(&seed).unwrap()).unwrap();

    let h = h
        .env("HELMP_NO_WELCOME", "1")
        .env("HELMP_SELF_TEST", "1")
        .env("HELMP_SELF_TEST_NOTES_QUIT", "-urgent");
    let out = h.cli(&[]);

    let state = out
        .lines()
        .find(|line| line.starts_with("self-test notes-quit-state:"))
        .unwrap_or_else(|| panic!("缺少编辑态日志：\n{out}"));
    assert!(
        state.contains("草稿 v1-urgent"),
        "编辑器按键未生效：{state}"
    );

    let before_quit = out
        .lines()
        .find(|line| line.starts_with("self-test notes-before-quit:"))
        .unwrap_or_else(|| panic!("缺少退出前磁盘日志：\n{out}"));
    assert!(
        before_quit.contains("草稿 v1") && !before_quit.contains("草稿 v1-urgent"),
        "预期退出前仍处于 500ms debounce 期内、尚未写盘：{before_quit}"
    );

    // 模拟重启应用：重新拉起新进程从磁盘加载便签列表，必须读到退出冲刷后的最新正文。
    let reloaded = h.query_in("notes", "");
    assert!(
        reloaded.iter().any(|row| row.title == "草稿 v1-urgent"),
        "重启重新加载后未读到退出前输入的新正文：{reloaded:#?}"
    );
    let saved = std::fs::read_to_string(&notes_path).unwrap();
    assert!(saved.contains("草稿 v1-urgent"), "{saved}");
}

#[test]
fn note_created_while_the_file_is_unreadable_is_saved_once_it_recovers() {
    // 为什么测这条链路：启动时 notes.json 读不出（权限、磁盘错误）会进入拒写保护，便签照常能新建和编辑；
    // 这期间的改动必须标成未保存、对用户可见，并在文件恢复后重读、合并、写盘，
    // 否则用户看着正常的便签会在重启后全部消失。这里用「notes.json 是个目录」稳定模拟读取 IO 错误。
    let h = Helmp::new();
    let notes_path = h.data_file("notes.json");
    std::fs::create_dir_all(&notes_path).unwrap();

    let h = h
        .env("HELMP_NO_WELCOME", "1")
        .env("HELMP_SELF_TEST", "1")
        .env("HELMP_SELF_TEST_NOTES_RECOVER", "1");
    let out = h.cli(&[]);

    assert!(
        out.contains("self-test notes-recover save-failed: true"),
        "拒写期间新建便签后应处于「未保存」状态：\n{out}"
    );
    let saved = std::fs::read_to_string(&notes_path)
        .unwrap_or_else(|err| panic!("退出时没有把便签写回磁盘：{err}\n{out}"));
    assert!(saved.contains("读不出期间新建"), "{saved}");
}
