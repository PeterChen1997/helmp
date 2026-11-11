//! macOS 26 起可按桌面坐标直接抓一块屏的 SDR 图像。每块屏独立取图，以保留各自的缩放和色彩空间。

use std::ptr::NonNull;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use block2::RcBlock;
use objc2::msg_send;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGDisplayBounds, CGImage};
use objc2_foundation::NSError;

use super::freeze::{Frozen, Screen, fill_windows};
use crate::i18n::{tr, trf};

#[link(name = "ScreenCaptureKit", kind = "framework")]
unsafe extern "C" {
    // The manager class is available on macOS 14 and later.
    #[link_name = "OBJC_CLASS_$_SCScreenshotManager"]
    static SCREENSHOT_MANAGER: AnyClass;
}

type CaptureResult = Result<CFRetained<CGImage>>;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(1);

/// `None` 表示本机的 ScreenCaptureKit 没有 macOS 26 的截图接口。
pub(super) fn grab(screens: &[Screen]) -> Option<Result<Vec<Frozen>>> {
    let config_class = AnyClass::get(c"SCScreenshotConfiguration")?;
    // SAFETY: The linked Objective-C class exists on every supported macOS version.
    let manager = unsafe { &SCREENSHOT_MANAGER };
    // SAFETY: Objective-C class objects respond to this standard NSObject selector.
    let available: Bool = unsafe {
        msg_send![manager, respondsToSelector: objc2::sel!(captureScreenshotWithRect:configuration:completionHandler:)]
    };
    if !available.as_bool() || screens.is_empty() {
        return None;
    }

    Some(autoreleasepool(|_| {
        let mut screens = screens.to_vec();
        let pending: Vec<_> = screens
            .iter()
            .map(|screen| request(manager, config_class, screen))
            .collect();
        fill_windows(&mut screens);

        screens
            .into_iter()
            .zip(pending)
            .map(|(screen, receiver)| {
                let image = receiver.recv_timeout(CAPTURE_TIMEOUT).context(tr(
                    "等待 ScreenCaptureKit 截图超时",
                    "Timed out waiting for ScreenCaptureKit",
                ))??;
                let width = CGImage::width(Some(&image));
                let height = CGImage::height(Some(&image));
                let expected = resolution(&screen);
                ensure!(
                    (width, height) == expected,
                    trf!(
                        "显示器 {} 的截图尺寸不符合预期：{}×{}",
                        "Unexpected capture size for display {}: {}×{}",
                        screen.id,
                        width,
                        height
                    )
                );
                let scale = screen.scale;
                Ok(Frozen {
                    screen,
                    image,
                    pixels: (width as u32, height as u32),
                    scale,
                })
            })
            .collect()
    }))
}

fn resolution(screen: &Screen) -> (usize, usize) {
    (
        (screen.frame.w * screen.scale).round() as usize,
        (screen.frame.h * screen.scale).round() as usize,
    )
}

fn request(
    manager: &AnyClass,
    config_class: &AnyClass,
    screen: &Screen,
) -> Receiver<CaptureResult> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let completion = RcBlock::new(move |output: *mut AnyObject, error: *mut NSError| {
        // SAFETY: ScreenCaptureKit keeps the callback arguments alive for the call.
        let image = unsafe {
            if let Some(output) = output.as_ref() {
                let image: *mut CGImage = msg_send![output, sdrImage];
                NonNull::new(image).map(|image| CFRetained::retain(image))
            } else {
                None
            }
        };
        let result = image.with_context(|| {
            // SAFETY: A non-null error belongs to this callback and is still alive.
            unsafe { error.as_ref() }
                .map(|error| error.localizedDescription().to_string())
                .unwrap_or_else(|| tr("系统未返回截图", "No screenshot returned").into())
        });
        let _ = sender.try_send(result);
    });
    let (width, height) = resolution(screen);
    let rect = CGDisplayBounds(screen.id);
    // SAFETY: The classes and selector were checked above. The block has the two
    // nullable object parameters from SCScreenshotManager.h and is copied for async use.
    unsafe {
        let config: Retained<AnyObject> = msg_send![config_class, new];
        let _: () = msg_send![&*config, setWidth: width as isize];
        let _: () = msg_send![&*config, setHeight: height as isize];
        let _: () = msg_send![&*config, setShowsCursor: Bool::NO];
        let _: () = msg_send![&*config, setDisplayIntent: 1isize]; // SCScreenshotDisplayIntentLocal
        let _: () = msg_send![manager, captureScreenshotWithRect: rect, configuration: &*config, completionHandler: &*completion];
    }
    receiver
}
