//! Helmp：macOS 原生启动器（读作 help me）。
//!
//! `huan`                   启动图形界面（⌥Space 唤起）
//! `helmp query <text>`      无头查询，见 `cli.rs`
//! `HELMP_SELF_TEST=1 helmp`  图形界面自检：程序化 show → 查询 → hide 后退出

mod cli;
mod config;
mod deeplink;
mod dock;
mod effects;
mod engine;
mod history;
mod hotkey;
mod i18n;
mod icons;
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
/// - `HELMP_SELF_TEST_HOTKEY`：可选，执行某个扩展的全局快捷键命令（如 `translate`）。
/// - `HELMP_SELF_TEST_KEYS`：可选，模拟按键序列，每步后报告焦点、表单与结果（见下）。
/// - `HELMP_SELF_TEST_SETTINGS`：可选，在屏幕外打开设置窗口的某一页（页面名中英文都行），
///   之后的按键、`click:` / `scroll:` 和 `snap:` 都作用于设置窗口。
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

        // HELMP_SELF_TEST_HOTKEY=<扩展 id>：像按下那个扩展的全局快捷键一样执行它的命令。
        if let Ok(provider) = std::env::var("HELMP_SELF_TEST_HOTKEY") {
            match hotkey::extension_command(&provider) {
                Some(command) => {
                    cx.update(|cx| hotkey::execute(command, cx));
                    cx.background_executor().timer(settle).await;
                    let launcher = launcher.clone();
                    window
                        .update(cx, move |_, window, cx| {
                            println!(
                                "self-test hotkey {provider}: {}",
                                launcher.read(cx).debug_state(window, cx)
                            );
                        })
                        .expect("launcher window gone");
                }
                None => println!("self-test hotkey {provider}: 没有这个扩展的快捷键"),
            }
        }

        // HELMP_SELF_TEST_SETTINGS=<页面名>：在屏幕外打开设置窗口的这一页（不激活、不抢焦点），
        // 之后的按键、点击、滚动和截图都作用于设置窗口。
        let mut target = window;
        if let Ok(name) = std::env::var("HELMP_SELF_TEST_SETTINGS") {
            match settings::Page::from_name(&name) {
                Some(page) => {
                    cx.update(|cx| settings::open_page(cx, page));
                    cx.background_executor().timer(settle).await;
                    match cx.update(|cx| settings::window_handle(cx)) {
                        Some(handle) => target = handle,
                        None => println!("self-test 设置窗口没有打开"),
                    }
                }
                None => println!("self-test 没有这个设置页：{name}"),
            }
        }
        let on_settings = target != window;

        // HELMP_SELF_TEST_KEYS：空格分隔的按键（GPUI 写法，如 `tab`、`up`、`cmd-k`），`~文字` 逐字输入，
        // `wait:<秒>` 干等，`snap:<名字>` 截图（需 `--features readme-shots`，见 docs/images/README.md）。
        // 每步之后打印范围、焦点、表单和结果；最后一步多等一会儿，留给输入即译和模型调用。
        if let Ok(keys) = std::env::var("HELMP_SELF_TEST_KEYS") {
            let tokens: Vec<String> = keys.split_whitespace().map(str::to_string).collect();
            for (ix, token) in tokens.iter().enumerate() {
                // `wait:<秒>`：干等，留给慢操作（如图片 OCR）跑完。
                if let Some(secs) = token.strip_prefix("wait:").and_then(|s| s.parse().ok()) {
                    cx.background_executor().timer(Duration::from_secs(secs)).await;
                    continue;
                }
                // `click:<x>,<y>` 在窗口坐标处单击；`scroll:<x>,<y>,<dy>` 在那里滚动（dy 为负 = 往下翻）。
                let numbers = |rest: &str| -> Vec<f32> {
                    rest.split(',').filter_map(|n| n.trim().parse().ok()).collect()
                };
                let pointer = if let Some(rest) = token.strip_prefix("click:") {
                    match numbers(rest)[..] {
                        [x, y] => {
                            let position = point(px(x), px(y));
                            let down = MouseDownEvent {
                                button: MouseButton::Left,
                                position,
                                click_count: 1,
                                ..Default::default()
                            };
                            let up = MouseUpEvent {
                                button: MouseButton::Left,
                                position,
                                click_count: 1,
                                ..Default::default()
                            };
                            // 先把鼠标挪过去：悬停状态按上一帧的鼠标位置算，有的控件只认悬停中的点击。
                            let hover = MouseMoveEvent {
                                position,
                                pressed_button: None,
                                modifiers: Modifiers::default(),
                            };
                            Some(vec![
                                PlatformInput::MouseMove(hover),
                                PlatformInput::MouseDown(down),
                                PlatformInput::MouseUp(up),
                            ])
                        }
                        _ => None,
                    }
                } else if let Some(rest) = token.strip_prefix("scroll:") {
                    match numbers(rest)[..] {
                        [x, y, dy] => Some(vec![PlatformInput::Scroll(ScrollEvent {
                            position: point(px(x), px(y)),
                            delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                            ..Default::default()
                        })]),
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some(events) = pointer {
                    for event in events {
                        // 屏幕外的窗口系统不驱动重绘：每个事件后手动画一帧，按下 / 抬起之间的
                        // 状态（开关等控件靠它判断一次完整的点击）才会更新。
                        target
                            .update(cx, |_, window, cx| {
                                window.dispatch_event(event, cx);
                                window.draw(cx).clear(cx);
                            })
                            .expect("self-test window gone");
                        cx.background_executor().timer(Duration::from_millis(80)).await;
                    }
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    continue;
                }
                // `snap:<名字>`：把面板此刻的画面存成 `$HELMP_SNAPSHOT_DIR/<名字>.png`（README 配图）。
                if let Some(name) = token.strip_prefix("snap:") {
                    let dir = std::env::var_os("HELMP_SNAPSHOT_DIR").unwrap_or_else(|| ".".into());
                    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
                    // 面板在屏幕外时系统不驱动重绘：手动画几帧，让图标（先生成 PNG、再异步解码）加载出来再截。
                    for _ in 0..2 {
                        target
                            .update(cx, |_, window, cx| {
                                // 把鼠标「挪」出窗口，免得启动时光标所在的那一行带着悬停高亮
                                // （没有详情栏的列表铺满整个面板，挪到面板里哪儿都可能压着一行）。
                                let rest = MouseMoveEvent {
                                    position: point(px(-10.), px(-10.)),
                                    pressed_button: None,
                                    modifiers: Modifiers::default(),
                                };
                                window.dispatch_event(PlatformInput::MouseMove(rest), cx);
                                window.draw(cx).clear(cx)
                            })
                            .expect("launcher window gone");
                        cx.background_executor().timer(settle).await;
                    }
                    target
                        .update(cx, move |_, window, cx| match panel::snapshot(window, cx, &path) {
                            Ok(()) => println!("self-test snap {}", path.display()),
                            Err(err) => println!("self-test snap {} 失败：{err:#}", path.display()),
                        })
                        .expect("launcher window gone");
                    continue;
                }
                if let Some(text) = token.strip_prefix('~') {
                    // 文字走窗口的正常输入路径，落到当前有焦点的输入框。
                    for ch in text.chars() {
                        let stroke = gpui_kit::Keystroke {
                            key: ch.to_string(),
                            key_char: Some(ch.to_string()),
                            ..Default::default()
                        };
                        target
                            .update(cx, move |_, window, cx| {
                                window.dispatch_keystroke(stroke, cx);
                            })
                            .expect("launcher window gone");
                        cx.background_executor().timer(Duration::from_millis(30)).await;
                    }
                } else if on_settings {
                    match gpui_kit::Keystroke::parse(Box::leak(token.clone().into_boxed_str())) {
                        Ok(stroke) => target
                            .update(cx, move |_, window, cx| {
                                window.dispatch_keystroke(stroke, cx);
                            })
                            .expect("settings window gone"),
                        Err(err) => println!("self-test 不认识的按键 {token:?}：{err}"),
                    }
                } else {
                    // 功能键直接交给面板的按键处理（模拟分发不经过面板的捕获监听）。
                    let stroke = match gpui_kit::Keystroke::parse(Box::leak(token.clone().into_boxed_str())) {
                        Ok(stroke) => stroke,
                        Err(err) => {
                            println!("self-test 不认识的按键 {token:?}：{err}");
                            continue;
                        }
                    };
                    let launcher = launcher.clone();
                    window
                        .update(cx, move |_, window, cx| {
                            launcher.update(cx, |launcher, cx| launcher.simulate_key(stroke, window, cx));
                        })
                        .expect("launcher window gone");
                }
                let wait = if ix + 1 == tokens.len() { settle * 2 } else { Duration::from_millis(600) };
                cx.background_executor().timer(wait).await;
                let launcher = launcher.clone();
                let token = token.clone();
                window
                    .update(cx, move |_, window, cx| {
                        println!("self-test key {token}: {}", launcher.read(cx).debug_state(window, cx));
                    })
                    .expect("launcher window gone");
            }
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
