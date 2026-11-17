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
