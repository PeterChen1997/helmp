//! 区域截图：包系统 `screencapture -i`（十字准星选区，空格切窗口，Esc 取消）。
//!
//! 屏幕录制权限只用 `CGPreflightScreenCaptureAccess` 检查，不调用 Request 弹框。
//! 注意 screencapture 子进程继承父进程的 TCC 身份：终端里跑问的是终端的权限，打包后问的是 Helmp。

use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context as _, Result};

use super::pasteboard;
use crate::i18n::{tr, trf};

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

/// 交互区域截图写入 `path`。返回 `Ok(false)` 表示用户按 Esc 取消。
pub fn to_file(path: &Path) -> Result<bool> {
    let output = Command::new("screencapture")
        .arg("-i")
        .arg(path)
        .output()
        .with_context(|| tr("启动 screencapture 失败", "Couldn't start screencapture"))?;
    judge(path.exists(), &output)
}

/// 交互区域截图写入剪贴板。返回 `Ok(false)` 表示用户按 Esc 取消。
pub fn to_clipboard() -> Result<bool> {
    let pb = pasteboard::general();
    let before = pb.changeCount();
    let output = Command::new("screencapture")
        .args(["-i", "-c"])
        .output()
        .with_context(|| tr("启动 screencapture 失败", "Couldn't start screencapture"))?;
    judge(pb.changeCount() != before, &output)
}

/// 根据「有没有产出」和 screencapture 的 stderr 判断结果。
///
/// Esc 取消时 screencapture 什么也不产出、也不往 stderr 写东西；
/// 截不到图（如无权限：`could not create image from display`）时会写 stderr，这必须报错。
fn judge(produced: bool, output: &Output) -> Result<bool> {
    if produced {
        return Ok(true);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    anyhow::ensure!(
        stderr.is_empty(),
        trf!(
            "screencapture 失败（{}）：{stderr}",
            "screencapture failed ({}): {stderr}",
            output.status
        )
    );
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt as _;
    use std::process::ExitStatus;

    fn output(code: i32, stderr: &str) -> Output {
        Output {
            status: ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn esc_cancel_is_not_an_error() {
        // 用户按 Esc 是正常操作，弹一条「执行失败」会很烦；无论退出码如何，没产出且没报错就当取消。
        assert!(!judge(false, &output(0, "")).unwrap());
        assert!(!judge(false, &output(1, "")).unwrap());
    }

    #[test]
    fn capture_failure_is_loud() {
        // 无权限 / 显示器异常时 screencapture 只在 stderr 说一句，必须冒泡给用户而不是当成取消。
        let err = judge(false, &output(1, "could not create image from display\n")).unwrap_err();
        assert!(format!("{err:#}").contains("could not create image from display"));
    }

    #[test]
    fn produced_output_wins() {
        assert!(judge(true, &output(0, "")).unwrap());
    }
}
