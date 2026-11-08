//! 唤（Huan）：macOS 原生启动器。
//!
//! `huan`                   启动图形界面（⌥Space 唤起）
//! `huan query <text>`      无头查询，见 `cli.rs`
//! `HUAN_SELF_TEST=1 huan`  图形界面自检：程序化 show → 查询 → hide 后退出

mod cli;
mod engine;
mod hotkey;
mod input_source;
mod launcher;
mod matcher;
mod panel;
mod paths;
mod provider;
mod providers;
mod trace;

use gpui_kit::assets::Assets;
use gpui_kit::*;

use engine::Engine;
use launcher::Launcher;

/// 面板尺寸。后续按内容自适应，骨架先固定。
const PANEL_SIZE: Size<Pixels> = size(px(720.), px(480.));

fn main() {
    let engine = Engine::new(providers::all());

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = cli::run(&engine, &args) {
        if let Err(err) = result {
            eprintln!("huan: {err:#}");
            std::process::exit(1);
        }
        return;
    }

    engine.start_in_background();
    run_gui(engine);
}

fn run_gui(engine: Engine) {
    gpui_kit::application().with_assets(Assets).run(move |cx| {
        // 菜单栏工具型 App：不进 Dock、不参与 ⌘Tab。
        cx.set_activation_policy(ActivationPolicy::Accessory);
        gpui_kit::init(cx);

        let bounds = Bounds::centered(None, PANEL_SIZE, cx);
        let options = WindowOptions {
            kind: WindowKind::PopUp,
            titlebar: None,
            focus: true,
            show: false,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Blurred,
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };

        let (window, launcher) = gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Launcher::new(engine, window, cx))
        })
        .expect("failed to open launcher window");

        if let Err(err) = hotkey::register_toggle(cx, window, launcher.clone()) {
            // 大声失败：热键注册不上整个 App 就没有意义。
            eprintln!("huan: failed to register global hotkey: {err:#}");
            cx.quit();
            return;
        }

        if let Some(query) = std::env::var_os("HUAN_SELF_TEST") {
            self_test(cx, window, launcher, query.to_string_lossy().into_owned());
        }
    });
}

/// 图形界面自检：在真实窗口里走一遍 show → 逐个查询 → 执行 → hide，把状态打到 stdout 后退出。
///
/// - `HUAN_SELF_TEST`：用 `|` 分隔的查询词，`1` 表示只测显隐。
/// - `HUAN_SELF_TEST_EXEC`：可选，输入这个查询后回车执行第一条结果，并报告面板上的消息。
fn self_test(cx: &mut App, window: AnyWindowHandle, launcher: Entity<Launcher>, queries: String) {
    use std::time::{Duration, Instant};

    let exec = std::env::var("HUAN_SELF_TEST_EXEC").ok();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let settle = Duration::from_millis(1500);
        // 等 provider 首次索引完成。
        cx.background_executor().timer(Duration::from_secs(3)).await;

        let report = |label: String, cx: &mut AsyncApp| {
            let launcher = launcher.clone();
            window
                .update(cx, move |_, window, cx| {
                    let launcher = launcher.read(cx);
                    println!(
                        "self-test {label}: visible={} window_active={} input_focused={} results={}",
                        launcher.is_visible(),
                        window.is_window_active(),
                        launcher.input_is_focused(window, cx),
                        launcher.items().len(),
                    );
                    for item in launcher.items().iter().take(3) {
                        println!("  {}\t{}\t{}", item.badge, item.title, item.subtitle);
                    }
                    if let Some(message) = launcher.message() {
                        println!("  message: {message}");
                    }
                })
                .expect("launcher window gone");
        };
        let is_active = |cx: &mut AsyncApp| {
            window
                .update(cx, |_, window, _| window.is_window_active())
                .unwrap_or(false)
        };

        report("initial".into(), cx);

        // 唤起延迟：从 toggle 到窗口成为 key window。
        let started = Instant::now();
        toggle(window, launcher.clone(), cx);
        while !is_active(cx) && started.elapsed() < Duration::from_secs(2) {
            cx.background_executor().timer(Duration::from_millis(2)).await;
        }
        println!("self-test show-latency: {} ms", started.elapsed().as_millis());
        report("after-show".into(), cx);

        if queries != "1" {
            for query in queries.split('|') {
                set_query(window, launcher.clone(), query, cx);
                cx.background_executor().timer(settle).await;
                report(format!("query {query:?}"), cx);
            }
        }

        if let Some(query) = exec {
            set_query(window, launcher.clone(), &query, cx);
            cx.background_executor().timer(settle).await;
            let launcher_for_exec = launcher.clone();
            window
                .update(cx, move |_, window, cx| {
                    launcher_for_exec.update(cx, |launcher, cx| launcher.execute_selected(window, cx));
                })
                .expect("launcher window gone");
            cx.background_executor().timer(settle).await;
            report(format!("exec {query:?}"), cx);
        }

        let started = Instant::now();
        toggle(window, launcher.clone(), cx);
        while is_active(cx) && started.elapsed() < Duration::from_secs(2) {
            cx.background_executor().timer(Duration::from_millis(2)).await;
        }
        println!("self-test hide-latency: {} ms", started.elapsed().as_millis());
        report("after-hide".into(), cx);

        // HUAN_SELF_TEST_RESHOW=<秒>：隐藏后等这么久再唤起，检查上次输入是被保留还是清空。
        if let Some(secs) = std::env::var("HUAN_SELF_TEST_RESHOW").ok().and_then(|s| s.parse().ok()) {
            cx.background_executor().timer(Duration::from_secs(secs)).await;
            toggle(window, launcher.clone(), cx);
            cx.background_executor().timer(settle).await;
            let launcher = launcher.clone();
            window
                .update(cx, move |_, _, cx| {
                    println!("self-test reshow after {secs}s: query={:?}", launcher.read(cx).query());
                })
                .expect("launcher window gone");
        }

        cx.update(|cx| cx.quit());
    })
    .detach();
}

fn set_query(window: AnyWindowHandle, launcher: Entity<Launcher>, query: &str, cx: &mut AsyncApp) {
    let query = query.to_string();
    window
        .update(cx, move |_, window, cx| {
            launcher.update(cx, |launcher, cx| launcher.set_query(&query, window, cx));
        })
        .expect("launcher window gone");
}

fn toggle(window: AnyWindowHandle, launcher: Entity<Launcher>, cx: &mut AsyncApp) {
    window
        .update(cx, |_, window, cx| {
            launcher.update(cx, |launcher, cx| launcher.toggle(window, cx));
        })
        .expect("launcher window gone");
}
