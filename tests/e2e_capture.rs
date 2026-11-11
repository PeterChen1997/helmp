//! 截图界面（冻结屏幕 → 框选 → 保存 / 复制 / 钉图 / 识别文字）的端到端测试。
//!
//! 命令行部分不开窗口：检查三个截图命令都打开截图界面、模式对。
//! 图形部分用图形自检（屏幕外、不抢焦点）真的打开遮罩：`HELMP_CAPTURE_FIXTURE` 用一张夹具图代替真实截屏
//! （不需要录屏权限，也不会截到用户的屏幕），`drag:` / `click:` / 按键模拟用户操作，
//! 再看存出来的文件和识别结果。和其他图形测试一样标了 `#[ignore]`（菜单栏会短暂出现图标），
//! 手动运行：`cargo test --release --test e2e_capture -- --ignored`。

mod support;

use support::{Helmp, repo_file};

/// 夹具：1200×320 像素，「你好 Huan 2026」+ 一行英文。按 2 倍屏用 = 600×160 点。
fn fixture() -> String {
    repo_file("tests/fixtures/ocr-zh-en.png")
        .display()
        .to_string()
}

fn png_size(path: &std::path::Path) -> (u32, u32) {
    let bytes = std::fs::read(path).expect("read png");
    assert!(
        bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "不是 PNG：{}",
        path.display()
    );
    let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    (word(16), word(20))
}

#[test]
fn capture_commands_open_the_overlay_in_the_right_mode() {
    // 三个命令都改成冻结屏幕后框选；回车的默认动作各自保留旧命令的含义。
    // 旧 id（screenshot:clipboard / screenshot:save）不能变：最近使用和快捷键都按 id 找它们。
    let h = Helmp::new().env("HELMP_CAPTURE_FIXTURE", fixture());
    assert_eq!(
        h.run("screenshot:clipboard", "截图", &[]).trim(),
        "open capture: copy"
    );
    assert_eq!(
        h.run("screenshot:save", "截图", &[]).trim(),
        "open capture: save"
    );
    assert_eq!(
        h.run("screenshot:ocr-screen", "截图", &[]).trim(),
        "open capture: ocr"
    );
    let row = h.find("screenshot", |r| r.id == "screenshot:clipboard");
    assert_eq!(row.title, "Screenshot");
    // 旧名字也还能搜到。
    let zh = Helmp::new().lang("zh_hans");
    assert_eq!(
        zh.find("截图到剪贴板", |r| r.id == "screenshot:clipboard")
            .title,
        "截图"
    );
}

/// 图形自检的输出里以 `prefix` 开头的那一行。
fn line<'a>(out: &'a str, prefix: &str) -> &'a str {
    out.lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("没有 {prefix:?}：\n{out}"))
}

/// 跑一次图形自检，要求正常退出，返回 stdout。
fn gui(h: &Helmp) -> String {
    let out = h.output(&[]);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "图形自检失败（{}）\nstdout:\n{stdout}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

/// 截图界面的图形自检环境：按 `hotkey`（`screenshot-capture` 截图、`screenshot` 截图识别文字）打开，
/// 再依次执行 `keys`。
fn capture_env(h: Helmp, hotkey: &str, keys: &str) -> Helmp {
    h.env("HELMP_NO_WELCOME", "1")
        .env("HELMP_SELF_TEST", "1")
        .env("HELMP_CAPTURE_FIXTURE", fixture())
        .env("HELMP_CAPTURE_FIXTURE_SCALE", "2")
        .env("HELMP_SELF_TEST_HOTKEY", hotkey)
        .env("HELMP_SELF_TEST_KEYS", keys)
}

#[test]
#[ignore = "开真实的图形界面（屏幕外、不抢焦点，但菜单栏会短暂出现图标）。手动运行：cargo test --release --test e2e_capture -- --ignored"]
fn gui_select_then_save_writes_the_full_resolution_crop() {
    // 截图快捷键 → 冻结的画面（夹具，2 倍屏）→ 拖出 100×50 点的选区 → ⌘S：
    // 存出来的 PNG 必须是 200×100 像素（Retina 下按像素裁，不能按点缩小变糊），放在设置的目录里。
    let keys = "drag:10,10,110,60 cmd-s";
    let h = Helmp::new();
    let shots = h.root.join("shots");
    let shots_for_config = shots.display().to_string();
    let h = capture_env(
        h.config(move |c| c["screenshot"] = serde_json::json!({ "save_dir": shots_for_config })),
        "screenshot-capture",
        keys,
    );
    let out = gui(&h);
    let opened = line(&out, "self-test hotkey screenshot-capture:");
    assert!(opened.contains("overlays=1"), "快捷键应打开遮罩：{opened}");
    let dragged = line(&out, "self-test key drag:");
    assert!(
        dragged.contains("selection=Some((10.0, 10.0, 100.0, 50.0))"),
        "{dragged}"
    );
    let saved = line(&out, "self-test key cmd-s:");
    assert!(saved.contains("overlays=0"), "保存后遮罩应关闭：{saved}");
    assert!(saved.contains("toast Saved"), "保存后要有提示：{saved}");
    let files: Vec<_> = std::fs::read_dir(&shots)
        .unwrap_or_else(|err| panic!("{} 没有创建：{err}\n{out}", shots.display()))
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("Helmp Screenshot ") && name.ends_with(".png"),
        "{name}"
    );
    assert_eq!(png_size(&files[0]), (200, 100));
}

#[test]
#[ignore = "开真实的图形界面（屏幕外、不抢焦点，但菜单栏会短暂出现图标）。手动运行：cargo test --release --test e2e_capture -- --ignored"]
fn gui_ocr_mode_recognizes_on_release_and_copies() {
    // 截图识别文字：松开鼠标立即识别（没有工具栏这一步），结果窗口里是夹具上的中英文，
    // 并且自动复制（演练模式下只记日志，不碰用户剪贴板）。
    let keys = "drag:0,0,600,160 wait:6";
    let h = capture_env(Helmp::new(), "screenshot", keys);
    let out = gui(&h);
    let opened = line(&out, "self-test hotkey screenshot:");
    assert!(opened.contains("overlays=1"), "{opened}");
    let done = line(&out, "self-test key wait:6");
    for expected in ["你好", "Huan", "2026", "quick brown fox", "copied=true"] {
        assert!(done.contains(expected), "识别结果缺少 {expected:?}：{done}");
    }
    assert!(done.contains("overlays=0"), "识别时遮罩应关闭：{done}");
    assert!(
        done.contains("[dry-run] copy text"),
        "演练模式不能写真实剪贴板：{done}"
    );
    assert!(done.contains("2 lines"), "标题要说出行数：{done}");
}

#[test]
#[ignore = "开真实的图形界面（屏幕外、不抢焦点，但菜单栏会短暂出现图标）。手动运行：cargo test --release --test e2e_capture -- --ignored"]
fn gui_pin_then_copy_and_close() {
    // 选区 (100,10)–(400,100)，工具栏在选区右下方外侧：左上角 (224,106)，第三个按钮是「钉图」。
    // 钉住后 ⌘C 复制钉图（600×180 像素），Esc 关掉钉图；Esc 也能直接关掉截图界面。
    let keys = "drag:100,10,400,100 click:312,126 cmd-c escape";
    let h = capture_env(Helmp::new(), "screenshot-capture", keys);
    let out = gui(&h);
    let pinned = line(&out, "self-test key click:312,126:");
    assert!(
        pinned.contains("overlays=0") && pinned.contains("pins=1"),
        "{pinned}"
    );
    let copied = line(&out, "self-test key cmd-c:");
    assert!(copied.contains("[dry-run] copy image 600x180"), "{copied}");
    assert!(copied.contains("toast Copied to Clipboard"), "{copied}");
    let closed = line(&out, "self-test key escape:");
    assert!(closed.contains("pins=0"), "{closed}");

    let keys = "escape";
    let h = capture_env(Helmp::new(), "screenshot-capture", keys);
    let out = gui(&h);
    let cancelled = line(&out, "self-test key escape:");
    assert!(
        cancelled.contains("overlays=0") && cancelled.contains("\"cancel\""),
        "{cancelled}"
    );
}
