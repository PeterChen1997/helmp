//! 设置窗口用到的系统接口：登录项（SMAppService）、按 bundle id 找 App、目录统计。

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};

use crate::i18n::tr;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSBundle, NSString};
use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// 当前进程是否从 `.app` 包里运行。从 `target/` 直接跑的二进制没有包，注册不了登录项。
pub fn running_from_bundle() -> bool {
    NSBundle::mainBundle()
        .bundlePath()
        .to_string()
        .ends_with(".app")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginItemStatus {
    Enabled,
    NotRegistered,
    /// 已注册但被用户在「系统设置 › 登录项」里关掉，需要用户手动允许。
    RequiresApproval,
    NotFound,
}

pub fn login_item_status() -> LoginItemStatus {
    // SAFETY: 只读查询，主 App 服务对象由系统管理。
    let status = unsafe { SMAppService::mainAppService().status() };
    match status {
        SMAppServiceStatus::Enabled => LoginItemStatus::Enabled,
        SMAppServiceStatus::RequiresApproval => LoginItemStatus::RequiresApproval,
        SMAppServiceStatus::NotRegistered => LoginItemStatus::NotRegistered,
        _ => LoginItemStatus::NotFound,
    }
}

/// 注册 / 注销「开机时自动启动」。失败时返回给界面显示的原因。
pub fn set_launch_at_login(enabled: bool) -> Result<()> {
    if !running_from_bundle() {
        bail!(
            "{}",
            tr(
                "当前不是从 Helmp.app 运行（例如直接运行 target/ 里的二进制），系统不允许注册登录项",
                "Helmp isn't running from Helmp.app (for example, a binary in target/), so macOS won't add it as a login item."
            )
        );
    }
    // SAFETY: SMAppService 的标准用法，在主线程调用。
    let result = unsafe {
        let service = SMAppService::mainAppService();
        if enabled {
            service.registerAndReturnError()
        } else {
            service.unregisterAndReturnError()
        }
    };
    result.map_err(|err| anyhow!("{}", err.localizedDescription()))?;
    if enabled && login_item_status() == LoginItemStatus::RequiresApproval {
        bail!(
            "{}",
            tr(
                "已注册，但需要在「系统设置 › 通用 › 登录项」中允许 Helmp",
                "Registered, but Helmp needs to be allowed in System Settings › General › Login Items."
            )
        );
    }
    Ok(())
}

pub fn open_login_items_settings() {
    // SAFETY: 打开系统设置的登录项面板，无参数。
    unsafe { SMAppService::openSystemSettingsLoginItems() };
}

/// 按 bundle id 找到已安装的 App 路径（用于显示名字和图标）。
pub fn app_path_for_bundle_id(bundle_id: &str) -> Option<PathBuf> {
    let url = NSWorkspace::sharedWorkspace()
        .URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))?;
    url.path().map(|path| PathBuf::from(path.to_string()))
}

/// 目录里的脚本文件数（不含隐藏文件和子目录）。目录不存在时返回 None。
pub fn count_scripts(dir: &Path) -> Option<usize> {
    let entries = std::fs::read_dir(dir).ok()?;
    Some(
        entries
            .flatten()
            .filter(|entry| {
                !entry.file_name().to_string_lossy().starts_with('.')
                    && entry.file_type().is_ok_and(|kind| kind.is_file())
            })
            .count(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binary_is_not_a_bundle() {
        // 从 target/ 运行时必须识别出来，界面才能说明登录项为什么开不了，而不是静默失败。
        assert!(!running_from_bundle());
        assert!(set_launch_at_login(true).is_err());
    }

    #[test]
    fn finds_system_apps_by_bundle_id() {
        let path = app_path_for_bundle_id("com.apple.calculator").unwrap();
        assert!(path.to_string_lossy().ends_with("Calculator.app"));
        assert_eq!(app_path_for_bundle_id("dev.helmp.does-not-exist"), None);
    }

    #[test]
    fn counts_only_visible_files() {
        let dir = std::env::temp_dir().join(format!("helmp-settings-count-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.sh"), "").unwrap();
        std::fs::write(dir.join(".hidden"), "").unwrap();
        assert_eq!(count_scripts(&dir), Some(1));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(count_scripts(&dir), None);
    }
}
