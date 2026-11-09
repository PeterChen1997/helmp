//! Helmp：macOS 原生启动器（读作 help me）。
//!
//! `huan`                   启动图形界面（⌥Space 唤起）
//! `helmp query <text>`      无头查询，见 `cli.rs`
//! `HELMP_SELF_TEST=1 helmp`  图形界面自检：程序化 show → 查询 → hide 后退出

mod cli;
mod config;
mod deeplink;
mod dock;
mod engine;
mod history;
mod hotkey;
mod i18n;
mod icons;
mod input_source;
mod launcher;
mod matcher;
mod menubar;
mod models;
mod onboarding;
mod panel;
mod paths;
mod permissions;
mod provider;
mod providers;
mod settings;
mod style;
mod trace;

use gpui_kit::assets::AllAssets;
use gpui_kit::*;

use engine::Engine;
use launcher::{Launcher, LauncherHandle, PANEL_SIZE};

fn main() {
    // 先定语言：CLI 输出与图形界面都按它取文案。
    i18n::init();
    let engine = Engine::new(providers::all());

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = cli::run(&engine, &args) {
        if let Err(err) = result {
            eprintln!("helmp: {err:#}");
            std::process::exit(1);
        }
        return;
    }

    engine.start_in_background();
    run_gui(engine);
}

fn run_gui(engine: Engine) {
    // 完整 Lucide 图标集：插件按名字引用符号图标（`Icon::Symbol { name: "scan-text", .. }`）。
    let app = gpui_kit::application().with_assets(AllAssets);
    // 系统在 App 启动前就可能送来 URL（用 helmp:// 链接冷启动），必须在 run 之前注册。
    let links = deeplink::listen(&app);
    app.run(move |cx| {
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
                // 毛玻璃由 panel::install_glass 垫的 NSVisualEffectView 提供，GPUI 只画半透明内容。
                window_background: WindowBackgroundAppearance::Transparent,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            };

            // 不用 gpui_kit::open_window：它的 Root 会铺一层不透明的主题底色，盖住毛玻璃。
            let mut launcher = None;
            let window: AnyWindowHandle = cx
                .open_window(options, |window, cx| {
                    let view = cx.new(|cx| Launcher::new(engine, window, cx));
                    launcher = Some(view.clone());
                    cx.new(|cx| gpui_kit::base::Root::new(view, window, cx).bg(transparent_black()))
                })
                .expect("failed to open launcher window")
                .into();
            let launcher = launcher.expect("launcher view not built");
            window
                .update(cx, |_, window, _| panel::install_glass(window))
                .expect("launcher window gone");
            cx.set_global(LauncherHandle {
                window,
                launcher: launcher.clone(),
            });
            cx.set_global(engine::EngineGlobal(launcher.read(cx).engine()));

            if let Err(err) = hotkey::register_toggle(cx, window, launcher.clone()) {
                // 大声失败：热键注册不上整个 App 就没有意义。
                eprintln!("helmp: failed to register global hotkey: {err:#}");
                cx.quit();
                return;
            }
            menubar::install(cx);
            onboarding::maybe_show(cx);
            deeplink::install(links, cx);

            if let Some(query) = std::env::var_os("HELMP_SELF_TEST") {
                self_test(cx, window, launcher, query.to_string_lossy().into_owned());
            }
        });
}

/// 图形界面自检：在真实窗口里走一遍 show → 逐个查询 → 执行 → hide，把状态打到 stdout 后退出。
///
/// - `HELMP_SELF_TEST`：用 `|` 分隔的查询词，`1` 表示只测显隐。
/// - `HELMP_SELF_TEST_EXEC`：可选，输入这个查询后回车执行第一条结果，并报告面板上的消息。
fn self_test(cx: &mut App, window: AnyWindowHandle, launcher: Entity<Launcher>, queries: String) {
    use std::time::{Duration, Instant};

    let exec = std::env::var("HELMP_SELF_TEST_EXEC").ok();
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

        // HELMP_SELF_TEST_RESHOW=<秒>：隐藏后等这么久再唤起，检查上次输入是被保留还是清空。
        if let Some(secs) = std::env::var("HELMP_SELF_TEST_RESHOW").ok().and_then(|s| s.parse().ok()) {
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
            launcher.update(cx, |launcher, cx| {
                // 每个查询都从根视图开始，免得上一个查询进入的范围（如「cb 」）吞掉后面的查询。
                launcher.reset_view(window, cx);
                launcher.set_query(&query, window, cx)
            });
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
