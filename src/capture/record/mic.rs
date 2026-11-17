//! 麦克风权限（AVFoundation）。区域录屏打开了「录制麦克风」时先查：没授权就只录画面，并提示去哪里打开。
//!
//! 第一次用（未决定）时请求权限，系统弹框问用户；打包后的 Info.plist 带 `NSMicrophoneUsageDescription`
//! （见 script/bundle.sh），没有它进程会被系统直接杀掉。

use block2::RcBlock;
use objc2::msg_send;
use objc2::runtime::{AnyClass, Bool};
use objc2_foundation::NSString;

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

/// 系统设置里「麦克风」那一页。
pub const SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Granted,
    /// 还没问过用户。
    NotDetermined,
    /// 用户拒绝了，或被管理策略禁止。
    Denied,
}

/// AVAuthorizationStatus → Access（0 未决定、1 受限制、2 拒绝、3 已授权）。
fn from_status(status: isize) -> Access {
    match status {
        3 => Access::Granted,
        0 => Access::NotDetermined,
        _ => Access::Denied,
    }
}

fn device_class() -> Option<&'static AnyClass> {
    AnyClass::get(c"AVCaptureDevice")
}

/// AVMediaTypeAudio 的值。
fn audio() -> objc2::rc::Retained<NSString> {
    NSString::from_str("soun")
}

pub fn access() -> Access {
    let Some(class) = device_class() else {
        return Access::Denied;
    };
    // SAFETY: 公开的类方法，参数是 AVMediaType（NSString），返回 NSInteger。
    let status: isize = unsafe { msg_send![class, authorizationStatusForMediaType: &*audio()] };
    from_status(status)
}

/// 请求权限（系统弹框），用户回答后返回是否允许。已经决定过的直接返回结果、不弹框。
pub async fn request() -> bool {
    let Some(class) = device_class() else {
        return false;
    };
    let (sender, receiver) = async_channel::bounded(1);
    let done = RcBlock::new(move |granted: Bool| {
        let _ = sender.try_send(granted.as_bool());
    });
    // SAFETY: 公开的类方法；block 签名 `void (^)(BOOL granted)` 与头文件一致。
    unsafe {
        let _: () =
            msg_send![class, requestAccessForMediaType: &*audio(), completionHandler: &*done];
    }
    receiver.recv().await.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_counts_as_denied() {
        // 「受限制」（家长控制 / MDM）和「拒绝」一样：只录画面，不能当成可以录。
        assert_eq!(from_status(3), Access::Granted);
        assert_eq!(from_status(0), Access::NotDetermined);
        assert_eq!(from_status(1), Access::Denied);
        assert_eq!(from_status(2), Access::Denied);
    }
}
