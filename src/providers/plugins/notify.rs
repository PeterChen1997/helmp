//! 系统通知（UserNotifications）：定时任务的脚本请求发的通知从这里发出（见 `schedule`）。
//!
//! - 授权：第一次要发通知时才向 macOS 请求（系统弹一次授权框）；本次运行里只请求一次。
//!   被拒绝后不再请求、不再弹框，任务结果里写明原因，「设置 › 插件」里说明去系统设置打开。
//! - 点击通知：带了链接的打开链接（只允许 http / https，校验在 `schedule::parse_output`）。
//!   链接放在通知的 userInfo 里，Helmp 重启后点通知中心里的旧通知也能打开。
//! - 只有从 `.app` 包运行时才有通知中心（`target/` 下直接跑的二进制没有 bundle，调用会抛异常），
//!   这时如实报错，不崩溃。演练模式（端到端测试）只在 stdout 回报，不碰通知中心。

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread as _, define_class, msg_send};
use objc2_foundation::{NSDictionary, NSError, NSString, NSURL};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};

use crate::i18n::tr;

/// 一条要发的通知（已校验过长度和链接）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    /// 点击后打开的 http(s) 链接。
    pub url: Option<String>,
}

/// macOS 给 Helmp 的通知授权。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// 还没问过用户。
    NotDetermined,
    Denied,
    Authorized,
}

/// 这一次该怎么做（纯函数，便于测试「被拒绝时不重复请求」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// 发出去。
    Send,
    /// 先向系统请求授权（弹一次框），同意了再发。
    Request,
    /// 用户拒绝过：不发，也不再请求。
    Denied,
    /// 本次运行已经请求过、用户还没在弹框里选：不再请求，这条先不发。
    Pending,
}

pub fn decide(status: Status, already_requested: bool) -> Decision {
    match (status, already_requested) {
        (Status::Authorized, _) => Decision::Send,
        (Status::Denied, _) => Decision::Denied,
        (Status::NotDetermined, false) => Decision::Request,
        (Status::NotDetermined, true) => Decision::Pending,
    }
}

/// 本次运行里已经向系统请求过授权。
static REQUESTED: AtomicBool = AtomicBool::new(false);
/// 最近一次知道的授权状态（设置页据此说明「被拒绝了」）。None = 还没查过。
static LAST_STATUS: Mutex<Option<Status>> = Mutex::new(None);

/// 最近一次知道的授权状态。
pub fn last_status() -> Option<Status> {
    *LAST_STATUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn remember(status: Status) {
    *LAST_STATUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(status);
}

/// 有没有通知中心可用（从 `.app` 包运行）。
fn available() -> bool {
    crate::settings::system::running_from_bundle()
}

pub fn denied_message() -> &'static str {
    tr(
        "通知被系统关掉了：到「系统设置 › 通知 › Helmp」里允许通知",
        "Notifications are turned off for Helmp: allow them in System Settings › Notifications › Helmp",
    )
}

/// 系统回调最多等多久（查询状态、发通知都很快；授权弹框要等用户点，给长一些）。
const CALLBACK_WAIT: Duration = Duration::from_secs(10);
const PROMPT_WAIT: Duration = Duration::from_secs(120);

fn center() -> Retained<UNUserNotificationCenter> {
    UNUserNotificationCenter::currentNotificationCenter()
}

fn query_status() -> Result<Status> {
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |settings: std::ptr::NonNull<UNNotificationSettings>| {
        // SAFETY: 系统传入的设置对象在回调期间有效。
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        let _ = tx.send(status);
    });
    center().getNotificationSettingsWithCompletionHandler(&block);
    let status = rx
        .recv_timeout(CALLBACK_WAIT)
        .map_err(|_| anyhow!("notification settings didn't answer"))?;
    let status = match status {
        UNAuthorizationStatus::NotDetermined => Status::NotDetermined,
        UNAuthorizationStatus::Denied => Status::Denied,
        // Provisional / Ephemeral 也能发（静默送达通知中心）。
        _ => Status::Authorized,
    };
    remember(status);
    Ok(status)
}

/// 弹一次系统授权框，等用户选（最多 `PROMPT_WAIT`）。
fn request_authorization() -> Result<Status> {
    REQUESTED.store(true, Ordering::SeqCst);
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
        let _ = tx.send(granted.as_bool());
    });
    center().requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &block,
    );
    let granted = rx.recv_timeout(PROMPT_WAIT).ok();
    let status = match granted {
        Some(true) => Status::Authorized,
        Some(false) => Status::Denied,
        None => Status::NotDetermined,
    };
    if let Some(granted) = granted {
        crate::telemetry::plugin_notification_permission(granted);
    }
    remember(status);
    Ok(status)
}

/// 发一条通知（阻塞，在定时任务的线程上调用）。返回 Err 时是给用户看的原因。
pub fn send(notice: &Notice) -> Result<()> {
    if crate::effects::dry_run() {
        // 演练模式：回报到 stdout（端到端测试据此断言），不碰通知中心。
        println!(
            "[dry-run] notify {} | {} | {}",
            notice.title,
            notice.body,
            notice.url.as_deref().unwrap_or_default()
        );
        return Ok(());
    }
    if !available() {
        bail!(tr(
            "只有从 Helmp.app 运行时才能发通知",
            "Notifications are only available when Helmp runs as Helmp.app",
        ));
    }
    let status = query_status()?;
    let status = match decide(status, REQUESTED.load(Ordering::SeqCst)) {
        Decision::Send => status,
        Decision::Request => request_authorization()?,
        Decision::Denied => bail!(denied_message()),
        Decision::Pending => bail!(tr(
            "还在等你在系统弹框里允许通知",
            "Waiting for you to allow notifications in the system prompt",
        )),
    };
    match status {
        Status::Authorized => deliver(notice),
        Status::Denied => bail!(denied_message()),
        Status::NotDetermined => bail!(tr(
            "系统弹框里还没选是否允许通知",
            "The notification prompt hasn't been answered yet",
        )),
    }
}

const URL_KEY: &str = "url";

fn deliver(notice: &Notice) -> Result<()> {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&notice.title));
    content.setBody(&NSString::from_str(&notice.body));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    if let Some(url) = &notice.url {
        let key = NSString::from_str(URL_KEY);
        let value = NSString::from_str(url);
        let info: Retained<NSDictionary<NSString, NSString>> =
            NSDictionary::from_slices(&[&*key], &[&*value]);
        // SAFETY: 泛型参数只是编译期标注，NSDictionary<NSString, NSString> 就是 NSDictionary；
        // userInfo 只放 plist 类型（NSString → NSString）。
        let info: Retained<NSDictionary> = unsafe { Retained::cast_unchecked(info) };
        unsafe { content.setUserInfo(&info) };
    }
    let identifier = NSString::from_str(&format!(
        "helmp-plugin-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let request =
        UNNotificationRequest::requestWithIdentifier_content_trigger(&identifier, &content, None);
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |error: *mut NSError| {
        // SAFETY: 非空时是系统传入、回调期间有效的 NSError。
        let message = unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string());
        let _ = tx.send(message);
    });
    center().addNotificationRequest_withCompletionHandler(&request, Some(&block));
    match rx.recv_timeout(CALLBACK_WAIT) {
        Ok(None) => Ok(()),
        Ok(Some(message)) => {
            let err = anyhow!("couldn't add the notification request");
            crate::telemetry::report("plugins.notify", &err);
            bail!(crate::i18n::trf!("发通知失败：{message}", "Couldn't send the notification: {message}"))
        }
        Err(_) => bail!(tr(
            "通知中心没有响应",
            "The notification center didn't respond",
        )),
    }
}

// ---------------------------------------------------------------- 点击通知

define_class!(
    // SAFETY: NSObject 没有子类化要求；本类没有实例变量，不实现 Drop。
    #[unsafe(super(NSObject))]
    #[name = "HelmpNotificationDelegate"]
    struct NotificationDelegate;

    // SAFETY: NSObjectProtocol 没有额外安全要求。
    unsafe impl NSObjectProtocol for NotificationDelegate {}

    // SAFETY: 两个方法的签名与 UNUserNotificationCenterDelegate 一致。
    unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
        /// Helmp 在前台（面板开着）时也照常弹横幅。
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        /// 点了通知：带链接的打开链接。
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &block2::DynBlock<dyn Fn()>,
        ) {
            let info = response.notification().request().content().userInfo();
            let key = NSString::from_str(URL_KEY);
            let url = info
                .objectForKey(&key)
                .and_then(|value| value.downcast::<NSString>().ok())
                .map(|value| value.to_string());
            if let Some(url) = url.filter(|url| clickable(url))
                && let Some(url) = NSURL::URLWithString(&NSString::from_str(&url))
            {
                objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(&url);
            }
            completion.call(());
        }
    }
);

impl NotificationDelegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        // SAFETY: NSObject 的 init。
        unsafe { msg_send![super(this), init] }
    }
}

/// 点击通知能打开的链接：只允许 http / https。
pub fn clickable(url: &str) -> bool {
    url.split_once("://").is_some_and(|(scheme, rest)| {
        (scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http"))
            && !rest.is_empty()
    })
}

/// 图形界面启动时调用：设好点击通知的处理，并在后台查一次授权状态（不弹框）。
pub fn install() {
    if !available() || crate::effects::dry_run() {
        return;
    }
    let delegate = NotificationDelegate::new();
    center().setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // 通知中心只弱引用 delegate：整个进程期间留着。
    std::mem::forget(delegate);
    std::thread::spawn(|| {
        if let Err(err) = query_status() {
            eprintln!("helmp: 查询通知授权失败：{err:#}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_denied_permission_is_never_requested_again() {
        // 用户拒绝过就不能每次定时任务都再弹框（macOS 也不会再弹，但我们连请求都不发，结果里写清原因）。
        assert_eq!(decide(Status::Denied, false), Decision::Denied);
        assert_eq!(decide(Status::Denied, true), Decision::Denied);
        // 第一次：请求一次。
        assert_eq!(decide(Status::NotDetermined, false), Decision::Request);
        // 本次运行已请求过、弹框还没选：不叠第二个请求。
        assert_eq!(decide(Status::NotDetermined, true), Decision::Pending);
        assert_eq!(decide(Status::Authorized, true), Decision::Send);
    }

    #[test]
    fn only_web_links_open_from_a_notification() {
        // 点通知打开的是插件脚本给的地址：只放 http(s)，不让它借通知去开 file:// 或别的 App 的 scheme。
        assert!(clickable("https://example.com/today"));
        assert!(clickable("HTTP://example.com"));
        for bad in ["file:///etc/hosts", "javascript:alert(1)", "obsidian://open", "https://", "example.com"] {
            assert!(!clickable(bad), "{bad}");
        }
    }
}
