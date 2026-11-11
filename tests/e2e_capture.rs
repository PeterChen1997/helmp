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

#[test]
#[ignore = "开真实的图形界面（屏幕外、不抢焦点，但菜单栏会短暂出现图标）。手动运行：cargo test --release --test e2e_capture -- --ignored"]
fn gui_screenshot_and_ocr_are_two_separate_registered_shortcuts() {
    // 「截图」⇧⌘1 和「截图识别文字」⇧⌘2 是两条独立的全局快捷键：默认两条都真的注册上。
    let h = Helmp::new()
        .env("HELMP_NO_WELCOME", "1")
        .env("HELMP_SELF_TEST", "1");
    let out = gui(&h);
    let hotkeys = line(&out, "self-test hotkeys:");
    for expected in [" screenshot-capture=⇧⌘1 ", " screenshot=⇧⌘2 "] {
        assert!(hotkeys.contains(expected), "缺少 {expected:?}：{hotkeys}");
    }

    // 手改配置让「截图识别文字」也用 ⇧⌘1：不能悄悄覆盖掉「截图」，要把冲突记下来（设置里显示「未生效」）。
    let h = Helmp::new()
        .config(|c| c["extensions"] = serde_json::json!({"screenshot": {"hotkey": "cmd-shift-1"}}))
        .env("HELMP_NO_WELCOME", "1")
        .env("HELMP_SELF_TEST", "1");
    let out = gui(&h);
    let hotkeys = line(&out, "self-test hotkeys:");
    assert!(hotkeys.contains(" screenshot-capture=⇧⌘1 "), "{hotkeys}");
    assert!(hotkeys.contains(" screenshot=-(Duplicate"), "{hotkeys}");
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
    // 选区 (100,10)–(400,100)，工具栏从屏幕左边开始，「钉图」在 (355,125)。
    // 钉住后 ⌘C 复制钉图（600×180 像素），Esc 关掉钉图；Esc 也能直接关掉截图界面。
    let keys = "drag:100,10,400,100 click:355,125 cmd-c escape";
    let h = capture_env(Helmp::new(), "screenshot-capture", keys);
    let out = gui(&h);
    let pinned = line(&out, "self-test key click:355,125:");
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

#[test]
#[ignore = "开屏幕外的真实截图界面。手动运行：cargo test --release --test e2e_capture -- --ignored"]
fn gui_annotations_are_baked_into_the_saved_retina_png() {
    let h = Helmp::new();
    let fixture = h.root.join("annotation-fixture.png");
    let source_value = |x, y| ((x + 2 * y) % 127) as u8;
    let mut encoder = png::Encoder::new(std::fs::File::create(&fixture).unwrap(), 1200, 320);
    encoder.set_color(png::ColorType::Rgb);
    let pixels: Vec<_> = (0..320)
        .flat_map(|y| (0..1200).flat_map(move |x| [source_value(x, y); 3]))
        .collect();
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&pixels).unwrap();
    writer.finish().unwrap();

    // 选区 (12,6)–(550,110)，主工具栏下方只有一排的空间，选项排到主工具栏上面。
    let keys = concat!(
        "drag:12,6,550,110 click:135,135 click:250,92 drag:32,24,80,48 ",
        "click:389,92 cmd-a ~B2285C enter drag:90,24,130,48 ",
        "click:315,135 drag:150,24,198,54 cmd-z cmd-shift-z cmd-s"
    );
    let shots = h.root.join("shots");
    let shots_for_config = shots.display().to_string();
    let h = capture_env(
        h.config(move |c| c["screenshot"] = serde_json::json!({"save_dir": shots_for_config})),
        "screenshot-capture",
        keys,
    )
    .env("HELMP_CAPTURE_FIXTURE", fixture.display().to_string());
    let out = gui(&h);
    let saved = line(&out, "self-test key cmd-s:");
    assert!(saved.contains("toast Saved"), "{out}");
    let path = std::fs::read_dir(shots)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let bytes = std::fs::read(path).unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buffer).unwrap();
    assert_eq!((info.width, info.height), (1076, 208));
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    let channels = info.color_type.samples();
    assert!(matches!(
        info.color_type,
        png::ColorType::Rgb | png::ColorType::Rgba
    ));
    let rgb = |x, y| {
        let offset = y * info.line_size + x * channels;
        <[u8; 3]>::try_from(&buffer[offset..offset + 3]).unwrap()
    };
    let near = |actual: [u8; 3], expected: [u8; 3]| {
        assert!(
            actual
                .into_iter()
                .zip(expected)
                .all(|(a, b)| a.abs_diff(b) <= 10),
            "{actual:?} differs from {expected:?}\n{out}"
        );
    };
    near(rgb(70, 36), [0x0a, 0x84, 0xff]); // 预设蓝色矩形
    near(rgb(185, 36), [0xb2, 0x28, 0x5c]); // HEX 自定义色矩形
    let tile = rgb(278, 38);
    assert_eq!(tile, rgb(287, 43), "同一块马赛克里的像素应相同");
    assert_ne!(tile, rgb(298, 38), "相邻色块应取到各自的颜色");
    near(rgb(274, 38), [source_value(274 + 24, 38 + 12); 3]); // 色块边外不受影响
}

#[test]
#[ignore = "开屏幕外的真实截图界面。手动运行：cargo test --release --test e2e_capture -- --ignored"]
fn gui_rightmost_capture_button_copies() {
    let keys = "drag:12,6,550,110 click:531,135";
    let out = gui(&capture_env(Helmp::new(), "screenshot-capture", keys));
    let copied = line(&out, "self-test key click:531,135:");
    assert!(copied.contains("overlays=0"), "{out}");
    assert!(copied.contains("[dry-run] copy image 1076x208"), "{out}");
}
