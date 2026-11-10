//! 文件搜索、系统设置面板、系统开关、浏览器书签、词典的端到端测试：真实的 helmp 可执行文件 + 隔离环境
//! （见 tests/support）。所有会改动环境的动作都在 HELMP_DRY_RUN=1 下只回报命令，不碰用户的系统设置、
//! 音量、访达、磁盘和浏览器。
//!
//! 文件搜索用的测试开关（见 src/providers/files.rs）：
//! - `HELMP_FILES_ROOTS`：只搜夹具目录；
//! - `HELMP_FILES_BACKEND=walk`：不依赖 Spotlight 给临时目录建索引（/var/folders 往往不建）；
//! - `HELMP_FILES_WAIT_MS`：一次性的命令行查询要等到搜索结束。

mod support;

use serde_json::json;
use support::{Helmp, repo_file};

/// 文件搜索的隔离环境：只搜 `<root>/files`，用目录遍历代替 Spotlight。
fn files_env() -> (Helmp, std::path::PathBuf) {
    let h = Helmp::new();
    let dir = h.root.join("files");
    let h = h
        .env("HELMP_FILES_BACKEND", "walk")
        .env("HELMP_FILES_ROOTS", dir.to_string_lossy())
        .env("HELMP_FILES_WAIT_MS", "10000");
    (h, dir)
}

#[test]
fn files_scope_finds_by_pinyin_and_opens_reveals_copies() {
    // 中文文件名要能用拼音找到（`f cesbg` → 测试报告.pdf），回车打开、⌘↵ 在访达中显示、⌘⇧C 复制路径，
    // 三个动作的目标都必须是那个文件本身。
    let (h, dir) = files_env();
    let report = h.write("files/docs/测试报告.pdf", "%PDF-1.4\n");
    h.write("files/docs/notes.md", "hello\n");
    let id = format!("files:{}", report.display());
    let row = h.find("f cesbg", |r| r.id == id);
    assert_eq!(row.badge, "Files");
    assert_eq!(row.title, "测试报告.pdf");
    assert!(row.subtitle.ends_with("files/docs"), "{row:?}");
    assert!(dir.join("docs/notes.md").exists());

    let path = report.display().to_string();
    assert_eq!(
        h.run(&id, "f cesbg", &[]).trim(),
        format!("[dry-run] open {path}")
    );
    assert_eq!(
        h.run(&id, "f cesbg", &["--action", "Show in Finder"])
            .trim(),
        format!("[dry-run] reveal {path}")
    );
    assert_eq!(
        h.run(&id, "f cesbg", &["--action", "Copy Path"]).trim(),
        format!("[dry-run] copy {path}")
    );
}

#[test]
fn files_respect_excluded_folders_and_list_recent_when_empty() {
    // node_modules 里成千上万个同名文件会把真正要找的挤出去：排除目录里的结果一条都不能出现。
    // 空查询列出最近用过的文件，「文件」范围不能是一片空白。
    let (h, dir) = files_env();
    let h = h.config(|c| c["files"] = json!({"excluded": ["node_modules"]}));
    h.write("files/notes.md", "a");
    h.write("files/node_modules/pkg/notes.md", "b");
    let rows = h.query("f notes");
    let ids: Vec<_> = rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        [format!("files:{}", dir.join("notes.md").display())],
        "{rows:#?}"
    );

    let recent = h.query("f ");
    assert!(recent.iter().any(|r| r.title == "notes.md"), "{recent:#?}");
    assert!(
        recent.iter().all(|r| !r.id.contains("node_modules")),
        "{recent:#?}"
    );
}

#[test]
fn files_mix_into_main_search_unless_turned_off() {
    // 根搜索里能直接搜到文件（排在应用之后）；设置里关掉后只在「文件」范围里出现。
    let (h, _) = files_env();
    h.write("files/quarterly-plan.key", "x");
    let row = h.find("quarterly", |r| r.badge == "Files");
    assert_eq!(row.title, "quarterly-plan.key");

    let (h, _) = files_env();
    let h = h.config(|c| c["files"] = json!({"in_root_search": false}));
    h.write("files/quarterly-plan.key", "x");
    assert!(h.query("quarterly").iter().all(|r| r.badge != "Files"));
    h.find("f quarterly", |r| r.title == "quarterly-plan.key");
}

#[test]
#[ignore = "依赖 Spotlight 已经给仓库目录建好索引：仓库在隐藏目录（如 .claude/worktrees）、CI 机器或刚克隆时都可能没有索引，不可靠。手动运行：cargo test --release --test e2e_files_system -- --ignored"]
fn files_real_spotlight_finds_repo_fixture() {
    // 真实的 mdfind 路径：只搜仓库里的夹具目录，验证 Spotlight 查询语句、-onlyin 与结果解析。
    let dir = repo_file("tests/fixtures/files");
    let h = Helmp::new()
        .env("HELMP_FILES_ROOTS", dir.to_string_lossy())
        .env("HELMP_FILES_WAIT_MS", "5000");
    let mut found = Vec::new();
    for _ in 0..10 {
        found = h.query("f notes");
        if !found.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let id = format!("files:{}", dir.join("notes.md").display());
    assert!(found.iter().any(|r| r.id == id), "{found:#?}");
    // Spotlight 自带中文分词：整词全拼（ceshi → 测试）能命中没打开过的中文文件名。
    let id = format!("files:{}", dir.join("测试报告.pdf").display());
    h.find("f ceshi", |r| r.id == id);
}

#[test]
fn settings_pane_opens_the_exact_url() {
    // 打开错的 URL 时系统设置只会停在首页，不报错；必须精确到那一页（隐私子页带锚点）。
    let h = Helmp::new();
    let row = h.find("full disk access", |r| {
        r.id == "settings-panes:privacy-full-disk"
    });
    assert_eq!(row.subtitle, "System Settings › Privacy & Security");
    assert_eq!(
        h.run(&row.id, "full disk access", &[]).trim(),
        "[dry-run] open x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles"
    );
    // 拼音首字母找面板，⌘⇧C 复制同一个链接（写脚本用）。
    h.find("ly", |r| r.id == "settings-panes:bluetooth");
    assert_eq!(
        h.run("settings-panes:bluetooth", "ly", &["--action", "Copy Link"])
            .trim(),
        "[dry-run] copy x-apple.systempreferences:com.apple.BluetoothSettings"
    );
}

#[test]
fn keep_awake_runs_caffeinate_tied_to_helmp() {
    // 防休眠必须带 -w <Helmp pid>：Helmp 退出后 caffeinate 跟着退出，电脑不会一直不睡。
    let h = Helmp::new();
    h.find("caffeinate 2h", |r| r.id == "toggles:caffeinate");
    let out = h.run("toggles:caffeinate", "caffeinate 2h", &[]);
    let rest = out
        .trim()
        .strip_prefix("[dry-run] run /usr/bin/caffeinate -d -i -t 7200 -w ")
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()),
        "{out}"
    );
    // 不带时长：一直开着直到再执行一次。
    let out = h.run("toggles:caffeinate", "防休眠", &[]);
    assert!(out.contains("/usr/bin/caffeinate -d -i -w "), "{out}");
    assert!(!out.contains("-t "), "{out}");
}

#[test]
fn volume_and_mute_run_exact_osascript() {
    // 音量调错（或静音状态下调了没反应）用户马上就会发现：命令必须是设音量并取消静音。
    let h = Helmp::new();
    h.find("vol 30", |r| r.id == "toggles:volume");
    assert_eq!(
        h.run("toggles:volume", "vol 30", &[]).trim(),
        "[dry-run] run /usr/bin/osascript -e 'set volume output volume 30 without output muted'"
    );
    assert_eq!(
        h.run("toggles:mute", "mute", &[]).trim(),
        "[dry-run] run /usr/bin/osascript -e 'set volume with output muted'"
    );
    assert_eq!(
        h.run("toggles:unmute", "unmute", &[]).trim(),
        "[dry-run] run /usr/bin/osascript -e 'set volume without output muted'"
    );
}

#[test]
fn hidden_files_and_eject_confirm_before_running() {
    // 显示隐藏文件会重启访达、弹出磁盘会打断读写：第一次回车只出确认页，确认后才执行确切的命令。
    let h = Helmp::new();
    let first = h.run("toggles:hidden-files", "hidden files", &[]);
    assert!(
        first.contains("Show hidden files and relaunch Finder?"),
        "{first}"
    );
    assert!(!first.contains("[dry-run]"), "确认前不能执行：{first}");
    let confirmed = h.run(
        "toggles:hidden-files",
        "hidden files",
        &["--then", "Show Hidden Files"],
    );
    assert!(
        confirmed.contains(
            "[dry-run] run /usr/bin/defaults write com.apple.finder AppleShowAllFiles -bool true && /usr/bin/killall Finder"
        ),
        "{confirmed}"
    );

    let first = h.run("toggles:eject-all", "eject all", &[]);
    assert!(first.contains("Eject all disks?"), "{first}");
    let confirmed = h.run(
        "toggles:eject-all",
        "eject all",
        &["--then", "Eject All Disks"],
    );
    assert!(
        confirmed.contains(
            r#"[dry-run] run /usr/bin/osascript -e 'tell application "Finder" to eject (every disk whose ejectable is true)'"#
        ),
        "{confirmed}"
    );
}

#[test]
fn hidden_files_toggle_follows_finder_setting() {
    // 已经显示隐藏文件时，同一条结果要变成「隐藏」，执行的是 false（读的是隔离 HOME 里的访达偏好）。
    let h = Helmp::new();
    h.write(
        "home/Library/Preferences/com.apple.finder.plist",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>AppleShowAllFiles</key><true/></dict></plist>"#,
    );
    let row = h.find("hidden files !", |r| r.id == "toggles:hidden-files");
    assert_eq!(row.title, "Hide Hidden Files");
    let out = h.run("toggles:hidden-files", "hidden files !", &[]);
    assert!(out.contains("AppleShowAllFiles -bool false"), "{out}");
}

const CHROME_BOOKMARKS: &str = r#"{
  "roots": {
    "bookmark_bar": {"name": "Bookmarks Bar", "type": "folder", "children": [
      {"name": "GitHub 首页", "type": "url", "url": "https://github.com/"},
      {"name": "工作", "type": "folder", "children": [
        {"name": "飞书文档", "type": "url", "url": "https://www.feishu.cn/docs"}
      ]}
    ]},
    "other": {"name": "Other Bookmarks", "type": "folder", "children": []},
    "synced": {"name": "Mobile Bookmarks", "type": "folder", "children": []}
  }
}"#;

const SAFARI_BOOKMARKS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>WebBookmarkType</key><string>WebBookmarkTypeList</string>
  <key>Children</key><array>
    <dict>
      <key>WebBookmarkType</key><string>WebBookmarkTypeList</string>
      <key>Title</key><string>BookmarksBar</string>
      <key>Children</key><array>
        <dict>
          <key>WebBookmarkType</key><string>WebBookmarkTypeLeaf</string>
          <key>URLString</key><string>https://developer.apple.com/</string>
          <key>URIDictionary</key><dict><key>title</key><string>苹果开发者</string></dict>
        </dict>
      </array>
    </dict>
  </array>
</dict></plist>"#;

#[test]
fn bookmarks_from_chromium_profiles_open_in_their_browser() {
    // 书签按拼音能搜到，回车要在书签所在的浏览器里打开（不是默认浏览器），多个 profile 都要读到。
    let h = Helmp::new();
    h.write(
        "home/Library/Application Support/Google/Chrome/Default/Bookmarks",
        CHROME_BOOKMARKS,
    );
    h.write(
        "home/Library/Application Support/Microsoft Edge/Profile 1/Bookmarks",
        CHROME_BOOKMARKS,
    );
    let row = h.find("bm gthb", |r| {
        r.id == "bookmarks:chrome/Default:https://github.com/"
    });
    assert_eq!(row.title, "GitHub 首页");
    assert_eq!(row.subtitle, "github.com · Bookmarks Bar");
    assert_eq!(
        h.run(&row.id, "bm gthb", &[]).trim(),
        "[dry-run] open -b com.google.Chrome https://github.com/"
    );
    assert_eq!(
        h.run(
            "bookmarks:edge/Profile 1:https://github.com/",
            "bm gthb",
            &[]
        )
        .trim(),
        "[dry-run] open -b com.microsoft.edgemac https://github.com/"
    );
    assert_eq!(
        h.run(&row.id, "bm gthb", &["--action", "Copy Link"]).trim(),
        "[dry-run] copy https://github.com/"
    );
    // 根搜索里也能直接搜到（≥ 2 个字符），文件夹路径写在副标题里。
    let row = h.find("fswd", |r| {
        r.id == "bookmarks:chrome/Default:https://www.feishu.cn/docs"
    });
    assert_eq!(row.subtitle, "www.feishu.cn · Bookmarks Bar / 工作");
}

#[test]
fn safari_bookmarks_and_missing_permission() {
    // Safari 书签能读就和 Chromium 一样搜；读不到（没有完全磁盘访问）时给一条说明并能直达设置，不能崩。
    let h = Helmp::new();
    let plist = h.write("home/Library/Safari/Bookmarks.plist", SAFARI_BOOKMARKS);
    let row = h.find("bm pgkfz", |r| {
        r.id == "bookmarks:safari/Default:https://developer.apple.com/"
    });
    assert_eq!(row.subtitle, "developer.apple.com · Favorites");
    assert_eq!(
        h.run(&row.id, "bm pgkfz", &[]).trim(),
        "[dry-run] open -b com.apple.Safari https://developer.apple.com/"
    );

    std::fs::set_permissions(&plist, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
    let row = h.find("bm ", |r| r.id == "bookmarks:safari-permission");
    assert_eq!(row.title, "Can’t read Safari bookmarks");
    assert_eq!(
        h.run(&row.id, "bm ", &[]).trim(),
        "[dry-run] open x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles"
    );
    std::fs::set_permissions(&plist, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
}

#[test]
fn dictionary_defines_a_common_word_or_says_it_cannot() {
    // 查到时回车复制的是释义；系统词典没下载（从没打开过「词典」App）时要明确说查不到，并能去词典 App 查。
    let h = Helmp::new();
    let row = h.find("define apple", |r| r.id.starts_with("dictionary:"));
    if row.id == "dictionary:apple" {
        let out = h.run(&row.id, "define apple", &[]);
        assert!(out.starts_with("[dry-run] copy apple"), "{out}");
        assert!(out.to_lowercase().contains("fruit"), "{out}");
        assert_eq!(
            h.run(&row.id, "define apple", &["--action", "Open in Dictionary"])
                .trim(),
            "[dry-run] open dict://apple"
        );
    } else {
        eprintln!(
            "跳过释义断言：本机系统词典没有返回 apple 的释义（词典资源未下载，打开一次「词典」App 即可）。只验证查不到时的提示。"
        );
        assert_eq!(row.id, "dictionary:none:apple");
        assert_eq!(row.title, "No definition for “apple”");
        assert_eq!(
            h.run(&row.id, "define apple", &[]).trim(),
            "[dry-run] open dict://apple"
        );
    }
    // 送去翻译：带着单词进入翻译范围。
    let out = h.run(&row.id, "define apple", &["--action", "Translate"]);
    assert_eq!(out.trim(), "enter scope translate: apple");
}
