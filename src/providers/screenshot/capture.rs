//! 录屏权限：截图类命令执行前检查，没有权限就给「前往授权」，不去截一片空白。
//!
//! 屏幕录制权限只用 `crate::permissions::screen_recording_granted` 实时查询，不调用 Request 弹框。
//! 真正的截屏在 `crate::capture::freeze`（`screencapture -x`）；注意子进程继承父进程的 TCC 身份：
//! 终端里跑问的是终端的权限，打包后问的是 Helmp。

use crate::i18n::tr;

pub fn no_screen_access() -> &'static str {
    tr(
        "Helmp 没有屏幕录制权限，无法截图。\n\
请到「系统设置 → 隐私与安全性 → 录屏与系统录音」里允许 Helmp（从终端运行时是允许该终端），然后重试。",
        "Helmp doesn’t have Screen Recording permission, so it can’t take screenshots.\n\
Allow Helmp in System Settings → Privacy & Security → Screen & System Audio Recording \
(when running from Terminal, allow that terminal app instead), then try again.",
    )
}

pub fn has_screen_access() -> bool {
    crate::permissions::screen_recording_granted()
}
