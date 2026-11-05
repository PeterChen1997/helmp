//! 唤（Huan）：macOS 原生启动器。骨架阶段只做三件事：
//! 全局热键唤起、非激活 PopUp 窗口、Esc / 失焦隐藏。

mod hotkey;
mod launcher;

use gpui_kit::assets::Assets;
use gpui_kit::*;

use launcher::Launcher;

/// 面板尺寸。后续按内容自适应，骨架先固定。
const PANEL_SIZE: Size<Pixels> = size(px(680.), px(420.));

fn main() {
    gpui_kit::application().with_assets(Assets).run(|cx| {
        // 菜单栏工具型 App：不进 Dock、不参与 ⌘Tab。
        cx.set_activation_policy(ActivationPolicy::Accessory);
        gpui_kit::init(cx);
        launcher::init(cx);

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
            cx.new(|cx| Launcher::new(window, cx))
        })
        .expect("failed to open launcher window");

        match hotkey::register_toggle(cx, window, launcher.clone()) {
            Ok(()) => {}
            Err(err) => {
                // 骨架阶段大声失败：热键注册不上整个 App 就没有意义。
                eprintln!("huan: failed to register global hotkey: {err:#}");
                cx.quit();
            }
        }

        if std::env::var_os("HUAN_SELF_TEST").is_some() {
            self_test(cx, window, launcher);
        }
    });
}

/// `HUAN_SELF_TEST=1`：无需人手按热键，程序化走一遍 show → hide，
/// 把窗口激活、输入框焦点、可见状态打到 stdout 后退出。给 CI 和没有屏幕权限的环境用。
fn self_test(cx: &mut App, window: AnyWindowHandle, launcher: Entity<Launcher>) {
    use std::time::Duration;

    cx.spawn(async move |cx: &mut AsyncApp| {
        let step = Duration::from_millis(300);
        cx.background_executor().timer(step).await;

        let report = |label: &'static str, window: AnyWindowHandle, cx: &mut AsyncApp| {
            let launcher = launcher.clone();
            window
                .update(cx, move |_, window, cx| {
                    let launcher = launcher.read(cx);
                    println!(
                        "self-test {label}: visible={} window_active={} input_focused={}",
                        launcher.is_visible(),
                        window.is_window_active(),
                        launcher.input_is_focused(window, cx),
                    );
                })
                .expect("launcher window gone");
        };

        report("initial", window, cx);
        toggle(window, launcher.clone(), cx);
        cx.background_executor().timer(step).await;
        report("after-show", window, cx);
        toggle(window, launcher.clone(), cx);
        cx.background_executor().timer(step).await;
        report("after-hide", window, cx);

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
