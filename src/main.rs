//! 唤（Huan）：macOS 原生启动器。
//!
//! `huan`                   启动图形界面（⌥Space 唤起）
//! `huan query <text>`      无头查询，见 `cli.rs`
//! `HUAN_SELF_TEST=1 huan`  图形界面自检：程序化 show → 查询 → hide 后退出

mod cli;
mod engine;
mod hotkey;
mod launcher;
mod matcher;
mod panel;
mod paths;
mod provider;
mod providers;

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

/// 程序化走一遍 show → 输入查询 → hide，把状态打到 stdout 后退出。
/// `HUAN_SELF_TEST` 的值作为查询词（`1` 表示只测显隐）。
fn self_test(cx: &mut App, window: AnyWindowHandle, launcher: Entity<Launcher>, query: String) {
    use std::time::Duration;

    cx.spawn(async move |cx: &mut AsyncApp| {
        let step = Duration::from_millis(800);
        cx.background_executor().timer(step).await;

        let report = |label: &'static str, cx: &mut AsyncApp| {
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
                    for item in launcher.items().iter().take(5) {
                        println!("  {}\t{}\t{}", item.badge, item.title, item.id);
                    }
                })
                .expect("launcher window gone");
        };

        report("initial", cx);
        toggle(window, launcher.clone(), cx);
        cx.background_executor().timer(step).await;
        report("after-show", cx);

        if query != "1" {
            let launcher = launcher.clone();
            window
                .update(cx, move |_, window, cx| {
                    launcher.update(cx, |launcher, cx| launcher.set_query(&query, window, cx));
                })
                .expect("launcher window gone");
            cx.background_executor().timer(step).await;
            report("after-query", cx);
        }

        toggle(window, launcher.clone(), cx);
        cx.background_executor().timer(step).await;
        report("after-hide", cx);

        cx.update(|cx| cx.quit());
    })
    .detach();
}

fn toggle(window: AnyWindowHandle, launcher: Entity<Launcher>, cx: &mut AsyncApp) {
    window
        .update(cx, |_, window, cx| {
            launcher.update(cx, |launcher, cx| launcher.toggle(window, cx));
        })
        .expect("launcher window gone");
}
