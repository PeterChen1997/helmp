//! ScreenCaptureKit 录屏（macOS 15+）：`SCStream` 只取一块屏上的选区，`SCRecordingOutput` 直接写 mp4，
//! 系统声音（`capturesAudio`）和麦克风（`captureMicrophone`）由同一条流一起写进去。
//!
//! Helmp 自己的窗口（选区外的红框、录制浮条）用 `SCContentFilter` 整个 App 排除，不会被录进去。
//!
//! 系统的回调都在 ScreenCaptureKit 自己的队列上：这里只把结果投进 channel，由 GPUI 前台任务处理。

use std::path::Path;

use anyhow::{Context as _, Result, anyhow, bail};
use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, Bool, NSObject, NSObjectProtocol};
use objc2::{AnyThread as _, ClassType as _, DefinedClass as _, define_class, msg_send, sel};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSError, NSString, NSURL};

use crate::capture::geometry::Rect;
use crate::i18n::{tr, trf};

#[link(name = "ScreenCaptureKit", kind = "framework")]
unsafe extern "C" {}

/// 录制中系统报来的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// mp4 写完了（停止后）。
    Finished,
    /// 写文件失败（磁盘满、目录没了……）。
    WriteFailed(String),
    /// 流被系统停掉（显示器拔掉、权限被收回……）。
    StreamStopped(String),
}

/// 要录的东西。
pub struct Request<'a> {
    /// CGDirectDisplayID。
    pub display: u32,
    /// 选区（这块屏的点坐标，原点在左上）。
    pub rect: Rect,
    /// 像素 / 点。
    pub scale: f64,
    pub system_audio: bool,
    pub microphone: bool,
    pub path: &'a Path,
}

/// 正在录的流。drop 不会停止录制，要调 [`Recorder::stop`]。
pub struct Recorder {
    stream: Retained<AnyObject>,
    _output: Retained<AnyObject>,
    _delegate: Retained<Delegate>,
}

/// 视频的像素尺寸：按 Retina 倍数换算，取偶数（H.264 要求宽高是偶数），至少 2。
pub fn video_size(rect: Rect, scale: f64) -> (usize, usize) {
    let even = |points: f64| {
        let pixels = (points * scale).round().max(2.0) as usize;
        pixels - pixels % 2
    };
    (even(rect.w), even(rect.h))
}

/// 开始录制。`events` 收录制中的事件（写完、出错）。
pub async fn start(request: Request<'_>, events: async_channel::Sender<Event>) -> Result<Recorder> {
    let content = shareable_content().await?;
    // SAFETY: 下面的消息都是 ScreenCaptureKit / Foundation 公开的接口，参数类型与头文件一致；
    // 拿到的对象都由 Retained 持有。
    unsafe {
        let display = find_display(&content, request.display)?;
        let own = own_application(&content);
        let excluded = match &own {
            Some(app) => NSArray::from_slice(&[&**app]),
            None => {
                // Helmp 此刻没有窗口在屏幕上时不在列表里：红框画在选区外、浮条放在选区外，录不进去。
                crate::trace::log("record: Helmp 不在可共享的 App 列表里，不排除");
                NSArray::new()
            }
        };
        let no_windows: Retained<NSArray<AnyObject>> = NSArray::new();
        let filter: Allocated<AnyObject> = msg_send![class(c"SCContentFilter")?, alloc];
        let filter: Option<Retained<AnyObject>> = msg_send![
            filter,
            initWithDisplay: &*display,
            excludingApplications: &*excluded,
            exceptingWindows: &*no_windows
        ];
        let filter = filter.context("SCContentFilter init")?;

        let config: Retained<AnyObject> = msg_send![class(c"SCStreamConfiguration")?, new];
        let (width, height) = video_size(request.rect, request.scale);
        let rect = request.rect;
        let source = CGRect::new(CGPoint::new(rect.x, rect.y), CGSize::new(rect.w, rect.h));
        let _: () = msg_send![&*config, setWidth: width];
        let _: () = msg_send![&*config, setHeight: height];
        let _: () = msg_send![&*config, setSourceRect: source];
        let _: () = msg_send![&*config, setShowsCursor: Bool::YES];
        let _: () = msg_send![&*config, setCapturesAudio: Bool::new(request.system_audio)];
        // 不录 Helmp 自己的声音（提示音之类）。
        let _: () = msg_send![&*config, setExcludesCurrentProcessAudio: Bool::YES];
        if request.microphone {
            let supported: Bool =
                msg_send![&*config, respondsToSelector: sel!(setCaptureMicrophone:)];
            if !supported.as_bool() {
                bail!(tr(
                    "这个 macOS 版本不支持录麦克风",
                    "This version of macOS can't record the microphone"
                ));
            }
            let _: () = msg_send![&*config, setCaptureMicrophone: Bool::YES];
        }

        let delegate = Delegate::new(events);
        let stream: Allocated<AnyObject> = msg_send![class(c"SCStream")?, alloc];
        let stream: Option<Retained<AnyObject>> = msg_send![
            stream,
            initWithFilter: &*filter,
            configuration: &*config,
            delegate: &*delegate
        ];
        let stream = stream.context("SCStream init")?;

        let output_config: Retained<AnyObject> =
            msg_send![class(c"SCRecordingOutputConfiguration")?, new];
        let url = NSURL::fileURLWithPath(&NSString::from_str(&request.path.to_string_lossy()));
        let _: () = msg_send![&*output_config, setOutputURL: &*url];
        // AVFileTypeMPEG4 / AVVideoCodecTypeH264 的值。H.264 的 mp4 哪里都能播。
        let _: () = msg_send![&*output_config, setOutputFileType: &*NSString::from_str("public.mpeg-4")];
        let _: () = msg_send![&*output_config, setVideoCodecType: &*NSString::from_str("avc1")];
        let output: Allocated<AnyObject> = msg_send![class(c"SCRecordingOutput")?, alloc];
        let output: Option<Retained<AnyObject>> =
            msg_send![output, initWithConfiguration: &*output_config, delegate: &*delegate];
        let output = output.context("SCRecordingOutput init")?;
        let added: Result<(), Retained<NSError>> =
            msg_send![&*stream, addRecordingOutput: &*output, error: _];
        added.map_err(|err| describe("addRecordingOutput", &err))?;

        let (sender, receiver) = async_channel::bounded(1);
        let done = RcBlock::new(move |error: *mut NSError| {
            let _ = sender.try_send(error.as_ref().map(|err| describe("startCapture", err)));
        });
        let _: () = msg_send![&*stream, startCaptureWithCompletionHandler: &*done];
        if let Some(err) = receiver.recv().await.context("startCapture 没有回调")? {
            return Err(err);
        }
        Ok(Recorder {
            stream,
            _output: output,
            _delegate: delegate,
        })
    }
}

impl Recorder {
    /// 停止录制。mp4 写完时 `events` 收到 [`Event::Finished`]（或 [`Event::WriteFailed`]）。
    /// 流已经被系统停掉时这里会报错，调用方忽略即可。
    pub async fn stop(&self) -> Result<()> {
        let (sender, receiver) = async_channel::bounded(1);
        let done = RcBlock::new(move |error: *mut NSError| {
            // SAFETY: 回调期间 error 有效。
            let _ = sender.try_send(unsafe { error.as_ref() }.map(|err| describe("stopCapture", err)));
        });
        // SAFETY: 公开接口，block 签名与头文件一致（可空的 NSError）。
        unsafe {
            let _: () = msg_send![&*self.stream, stopCaptureWithCompletionHandler: &*done];
        }
        match receiver.recv().await.context("stopCapture 没有回调")? {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

fn class(name: &std::ffi::CStr) -> Result<&'static AnyClass> {
    AnyClass::get(name).with_context(|| {
        trf!(
            "系统里没有 {}（区域录屏需要 macOS 15 或更新）",
            "{} is missing (screen recording needs macOS 15 or later)",
            name.to_string_lossy()
        )
    })
}

fn describe(place: &str, err: &NSError) -> anyhow::Error {
    anyhow!(
        "{place}: {} ({} {})",
        err.localizedDescription(),
        err.domain(),
        err.code()
    )
}

async fn shareable_content() -> Result<Retained<AnyObject>> {
    let (sender, receiver) = async_channel::bounded(1);
    let done = RcBlock::new(move |content: *mut AnyObject, error: *mut NSError| {
        // SAFETY: 回调期间参数有效；content 要留着用，retain 一份。
        let result = unsafe {
            match (Retained::retain(content), error.as_ref()) {
                (Some(content), _) => Ok(content),
                (None, Some(err)) => Err(describe("SCShareableContent", err)),
                (None, None) => Err(anyhow!("SCShareableContent: 没有结果")),
            }
        };
        let _ = sender.try_send(result);
    });
    // SAFETY: 公开的类方法；block 的两个参数都可空，与头文件一致。
    unsafe {
        let _: () = msg_send![
            class(c"SCShareableContent")?,
            getShareableContentExcludingDesktopWindows: Bool::NO,
            onScreenWindowsOnly: Bool::YES,
            completionHandler: &*done
        ];
    }
    receiver
        .recv()
        .await
        .context("SCShareableContent 没有回调")?
}

/// # Safety
/// `content` 是 SCShareableContent。
unsafe fn find_display(content: &AnyObject, id: u32) -> Result<Retained<AnyObject>> {
    // SAFETY: 由调用方保证类型；displays 是 NSArray<SCDisplay>。
    let displays: Retained<NSArray<AnyObject>> = unsafe { msg_send![content, displays] };
    displays
        .iter()
        .find(|display| {
            // SAFETY: SCDisplay.displayID 是 CGDirectDisplayID（uint32_t）。
            let display_id: u32 = unsafe { msg_send![&**display, displayID] };
            display_id == id
        })
        .with_context(|| {
            trf!(
                "找不到要录的显示器（{}），可能刚刚断开了",
                "Couldn't find the display to record ({}); it may have been disconnected",
                id
            )
        })
}

/// # Safety
/// `content` 是 SCShareableContent。
unsafe fn own_application(content: &AnyObject) -> Option<Retained<AnyObject>> {
    let pid = std::process::id() as i32;
    // SAFETY: applications 是 NSArray<SCRunningApplication>，processID 是 pid_t。
    let apps: Retained<NSArray<AnyObject>> = unsafe { msg_send![content, applications] };
    apps.iter().find(|app| {
        // SAFETY: 同上。
        let app_pid: i32 = unsafe { msg_send![&**app, processID] };
        app_pid == pid
    })
}

struct Ivars {
    events: async_channel::Sender<Event>,
}

define_class!(
    // SAFETY: NSObject 没有子类化要求；Delegate 没有实现 Drop。
    #[unsafe(super = NSObject)]
    #[ivars = Ivars]
    struct Delegate;

    impl Delegate {
        // SAFETY: 签名与 SCRecordingOutputDelegate 一致。
        #[unsafe(method(recordingOutputDidFinishRecording:))]
        fn did_finish(&self, _output: &AnyObject) {
            let _ = self.ivars().events.try_send(Event::Finished);
        }

        // SAFETY: 签名与 SCRecordingOutputDelegate 一致。
        #[unsafe(method(recordingOutput:didFailWithError:))]
        fn did_fail(&self, _output: &AnyObject, error: &NSError) {
            let message = describe("SCRecordingOutput", error).to_string();
            let _ = self.ivars().events.try_send(Event::WriteFailed(message));
        }

        // SAFETY: 签名与 SCStreamDelegate 一致。
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop(&self, _stream: &AnyObject, error: &NSError) {
            let message = describe("SCStream", error).to_string();
            let _ = self.ivars().events.try_send(Event::StreamStopped(message));
        }
    }

    // SAFETY: NSObjectProtocol 没有额外安全要求。
    unsafe impl NSObjectProtocol for Delegate {}
);

impl Delegate {
    fn new(events: async_channel::Sender<Event>) -> Retained<Self> {
        declare_protocols();
        let this = Self::alloc().set_ivars(Ivars { events });
        // SAFETY: NSObject 的 init。
        unsafe { msg_send![super(this), init] }
    }
}

/// 让 `conformsToProtocol:` 也认这两个协议（ScreenCaptureKit 只按方法名回调，这里是保险）。
fn declare_protocols() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let class = Delegate::class();
        for name in [c"SCStreamDelegate", c"SCRecordingOutputDelegate"] {
            if let Some(protocol) = AnyProtocol::get(name) {
                // SAFETY: 类已注册；只是追加协议声明，不改方法。
                unsafe {
                    objc2::ffi::class_addProtocol(
                        std::ptr::from_ref(class).cast_mut(),
                        protocol,
                    );
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真的录 3 秒主屏左上角 400×300 pt，看 mp4 写没写出来。要录屏权限、屏幕不能锁着，所以默认不跑：
    /// `HELMP_RECORD_TEST_OUT=/tmp/a.mp4 cargo test --release real_recording -- --ignored`，
    /// 再用 `ffprobe` / QuickTime 看尺寸和时长。
    #[test]
    #[ignore = "真的录屏：要录屏权限、屏幕解锁"]
    fn real_recording_writes_an_mp4() {
        let out = std::env::var("HELMP_RECORD_TEST_OUT")
            .unwrap_or_else(|_| std::env::temp_dir().join("helmp-record-test.mp4").display().to_string());
        let path = std::path::PathBuf::from(&out);
        let _ = std::fs::remove_file(&path);
        let display = objc2_core_graphics::CGMainDisplayID();
        let (tx, rx) = async_channel::unbounded();
        let audio = std::env::var("HELMP_RECORD_TEST_AUDIO").is_ok();
        let recorder = futures_lite::future::block_on(start(
            Request {
                display,
                rect: Rect::new(0.0, 0.0, 400.0, 300.0),
                scale: 2.0,
                system_audio: audio,
                microphone: false,
                path: &path,
            },
            tx,
        ))
        .expect("start");
        std::thread::sleep(std::time::Duration::from_secs(3));
        futures_lite::future::block_on(recorder.stop()).expect("stop");
        let event = futures_lite::future::block_on(async {
            futures_lite::future::or(async { rx.recv().await.ok() }, async {
                async_io_timeout().await;
                None
            })
            .await
        });
        assert_eq!(event, Some(Event::Finished));
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        assert!(len > 0, "{out} 是空的");
        println!("recorded {out} ({len} bytes)");
    }

    async fn async_io_timeout() {
        let (tx, rx) = async_channel::bounded::<()>(1);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(8));
            let _ = tx.try_send(());
        });
        let _ = rx.recv().await;
    }

    #[test]
    fn video_size_is_even_retina_pixels() {
        // H.264 只收偶数宽高：选区换成像素后取偶数，Retina 下是点数的 2 倍（录出来不糊）。
        assert_eq!(video_size(Rect::new(0.0, 0.0, 700.0, 450.0), 2.0), (1400, 900));
        assert_eq!(video_size(Rect::new(0.0, 0.0, 333.0, 201.0), 1.0), (332, 200));
        assert_eq!(video_size(Rect::new(0.0, 0.0, 100.25, 50.5), 2.0), (200, 100));
        // 极小的选区也不能是 0（编码器会直接失败）。
        assert_eq!(video_size(Rect::new(0.0, 0.0, 0.2, 0.2), 1.0), (2, 2));
    }
}
