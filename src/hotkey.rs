//! 全局热键：`global-hotkey`（Carbon `RegisterEventHotKey`，免辅助功能权限）。
//! 事件在 Carbon 回调线程产生，这里用前台任务定时轮询 receiver，
//! 把「切换面板」投回 GPUI 主线程（Loungy 的做法）。

use std::time::Duration;

use anyhow::{Context as _, Result};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::*;

use crate::launcher::Launcher;

/// 默认热键 ⌥Space。⌘Space 被 Spotlight 占用时 Carbon 会直接注册失败，
/// 骨架阶段不去和它抢。
fn default_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::ALT), Code::Space)
}

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 注册切换热键，并启动轮询任务。`manager` 必须活到进程结束，
/// 否则 Carbon 侧的 handler 会被卸载，所以这里把它交给 GPUI 的 App 保管。
pub fn register_toggle(
    cx: &mut App,
    window: AnyWindowHandle,
    launcher: Entity<Launcher>,
) -> Result<()> {
    let manager = GlobalHotKeyManager::new().context("GlobalHotKeyManager::new")?;
    let hotkey = default_hotkey();
    manager
        .register(hotkey)
        .with_context(|| format!("register hotkey {hotkey:?}"))?;
    cx.set_global(HotkeyGlobal { _manager: manager });

    let hotkey_id = hotkey.id();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let receiver = GlobalHotKeyEvent::receiver();
        loop {
            while let Ok(event) = receiver.try_recv() {
                if event.id == hotkey_id && event.state == HotKeyState::Pressed {
                    crate::trace::log("hotkey pressed");
                    let launcher = launcher.clone();
                    let _ = window.update(cx, |_, window, cx| {
                        launcher.update(cx, |launcher, cx| launcher.toggle(window, cx));
                    });
                }
            }
            cx.background_executor().timer(POLL_INTERVAL).await;
        }
    })
    .detach();

    Ok(())
}

struct HotkeyGlobal {
    _manager: GlobalHotKeyManager,
}

impl Global for HotkeyGlobal {}
