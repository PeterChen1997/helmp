//! 真实的窗口读写：辅助功能 API（AXUIElement）取前台 App 的焦点窗口、读写位置与大小；
//! `NSScreen` 取各屏的可见区域（只能在主线程读，后台线程经主队列同步取）。

use std::ffi::c_void;

use anyhow::{Result, bail};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSScreen, NSWorkspace};
use objc2_foundation::NSString;

use super::geometry::{Rect, Screen, cocoa_to_ax};
use crate::i18n::{tr, trf};

type CFTypeRef = *const c_void;

#[repr(C)]
#[derive(Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Default)]
struct CGSize {
    width: f64,
    height: f64,
}

const AX_VALUE_CG_POINT: u32 = 1;
const AX_VALUE_CG_SIZE: u32 = 2;
const AX_SUCCESS: i32 = 0;
const AX_API_DISABLED: i32 = -25211;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(element: CFTypeRef, attribute: CFTypeRef, value: *mut CFTypeRef) -> i32;
    fn AXUIElementSetAttributeValue(element: CFTypeRef, attribute: CFTypeRef, value: CFTypeRef) -> i32;
    fn AXUIElementIsAttributeSettable(element: CFTypeRef, attribute: CFTypeRef, settable: *mut u8) -> i32;
    fn AXValueCreate(kind: u32, value: *const c_void) -> CFTypeRef;
    fn AXValueGetValue(value: CFTypeRef, kind: u32, out: *mut c_void) -> u8;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
}

unsafe extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_sync_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

/// 自己持有、析构时 CFRelease 的 CF 对象。
struct Owned(CFTypeRef);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: 只包装 Create / Copy 得到的非空对象。
        unsafe { CFRelease(self.0) }
    }
}

fn attribute(name: &str) -> Retained<NSString> {
    NSString::from_str(name)
}

fn cf(name: &NSString) -> CFTypeRef {
    // NSString 与 CFString 免费桥接。
    std::ptr::from_ref(name).cast()
}

/// 前台 App 的焦点窗口。
pub struct Window {
    element: Owned,
    pub app: String,
    pub pid: i32,
}

impl Window {
    /// 唤起 Helmp 之前的前台 App 的焦点窗口。面板是非激活面板，前台 App 一直是用户在用的那个。
    pub fn frontmost() -> Result<Self> {
        let (pid, app) = objc2::rc::autoreleasepool(|_| {
            let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
            let name = app
                .localizedName()
                .map(|name| name.to_string())
                .unwrap_or_default();
            Some((app.processIdentifier(), name))
        })
        .ok_or_else(|| anyhow::anyhow!(tr("找不到前台应用", "Couldn’t find the frontmost app")))?;
        if u32::try_from(pid).ok() == Some(std::process::id()) {
            bail!(tr(
                "前台是 Helmp 自己，没有可调整的窗口",
                "Helmp itself is in front, so there’s no window to arrange"
            ));
        }
        // SAFETY: 返回新创建的对象，交给 Owned 释放。
        let application = unsafe { AXUIElementCreateApplication(pid) };
        if application.is_null() {
            bail!(trf!("无法访问「{app}」", "Couldn’t access “{app}”"));
        }
        let application = Owned(application);
        let element = ["AXFocusedWindow", "AXMainWindow"]
            .iter()
            .find_map(|name| copy(application.0, name).ok())
            .ok_or_else(|| {
                anyhow::anyhow!(trf!(
                    "「{app}」没有打开的窗口",
                    "“{app}” has no open window"
                ))
            })?;
        Ok(Self { element, app, pid })
    }

    pub fn frame(&self) -> Result<Rect> {
        let mut point = CGPoint::default();
        let mut size = CGSize::default();
        let position = copy(self.element.0, "AXPosition")?;
        let extent = copy(self.element.0, "AXSize")?;
        // SAFETY: 两个值都是对应类型的 AXValue；输出缓冲区类型匹配。
        let ok = unsafe {
            AXValueGetValue(position.0, AX_VALUE_CG_POINT, std::ptr::from_mut(&mut point).cast()) != 0
                && AXValueGetValue(extent.0, AX_VALUE_CG_SIZE, std::ptr::from_mut(&mut size).cast()) != 0
        };
        if !ok {
            bail!(trf!(
                "读不到「{}」窗口的位置和大小",
                "Couldn’t read the position and size of “{}”’s window",
                self.app
            ));
        }
        Ok(Rect::new(point.x, point.y, size.width, size.height))
    }

    pub fn settable(&self, name: &str) -> bool {
        let name = attribute(name);
        let mut settable = 0u8;
        // SAFETY: element 有效；settable 是输出参数。
        let status = unsafe { AXUIElementIsAttributeSettable(self.element.0, cf(&name), &mut settable) };
        status == AX_SUCCESS && settable != 0
    }

    /// 先改大小再移动，再改一次大小：跨屏移动时，目标屏放不下旧尺寸会被系统挡住位置。
    pub fn set_frame(&self, rect: Rect, resize: bool) -> Result<()> {
        if resize {
            self.set_size(rect)?;
        }
        let point = CGPoint { x: rect.x, y: rect.y };
        self.set("AXPosition", AX_VALUE_CG_POINT, std::ptr::from_ref(&point).cast())?;
        if resize {
            self.set_size(rect)?;
        }
        Ok(())
    }

    fn set_size(&self, rect: Rect) -> Result<()> {
        let size = CGSize {
            width: rect.w,
            height: rect.h,
        };
        self.set("AXSize", AX_VALUE_CG_SIZE, std::ptr::from_ref(&size).cast())
    }

    fn set(&self, name: &str, kind: u32, value: *const c_void) -> Result<()> {
        // SAFETY: value 指向与 kind 匹配的结构体。
        let value = unsafe { AXValueCreate(kind, value) };
        if value.is_null() {
            bail!(tr("AXValueCreate 失败", "AXValueCreate failed"));
        }
        let value = Owned(value);
        let name_string = attribute(name);
        // SAFETY: 三个参数都是有效的 CF 对象。
        let status = unsafe { AXUIElementSetAttributeValue(self.element.0, cf(&name_string), value.0) };
        check(status, &self.app, name)
    }
}

fn check(status: i32, app: &str, what: &str) -> Result<()> {
    match status {
        AX_SUCCESS => Ok(()),
        AX_API_DISABLED => bail!(tr(
            "没有辅助功能权限，无法调整其他 App 的窗口",
            "Accessibility permission is missing, so other apps’ windows can’t be arranged"
        )),
        other => bail!(trf!(
            "「{app}」拒绝修改窗口（{what}，错误 {other}）",
            "“{app}” refused to change its window ({what}, error {other})"
        )),
    }
}

fn copy(element: CFTypeRef, name: &str) -> Result<Owned> {
    let name_string = attribute(name);
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: element 有效；成功时 value 是 +1 引用，交给 Owned 释放。
    let status = unsafe { AXUIElementCopyAttributeValue(element, cf(&name_string), &mut value) };
    if status != AX_SUCCESS || value.is_null() {
        bail!("{name}: AX error {status}");
    }
    Ok(Owned(value))
}

/// 各屏的整块区域与可见区域（AX 坐标），第一个是主屏。
pub fn screens() -> Vec<Screen> {
    on_main(|mtm| {
        let screens = NSScreen::screens(mtm);
        let rect = |r: objc2_foundation::NSRect| Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height);
        let Some(primary) = screens.firstObject().map(|s| s.frame().size.height) else {
            return Vec::new();
        };
        screens
            .iter()
            .map(|screen| Screen {
                frame: cocoa_to_ax(rect(screen.frame()), primary),
                visible: cocoa_to_ax(rect(screen.visibleFrame()), primary),
            })
            .collect()
    })
}

/// 在主线程上执行：已经在主线程（命令行）就直接执行，否则经主队列同步执行（图形界面的动作在后台线程）。
fn on_main<R: Send>(f: impl FnOnce(MainThreadMarker) -> R + Send) -> R {
    if let Some(mtm) = MainThreadMarker::new() {
        return f(mtm);
    }
    let mut job = Job { f: Some(f), out: None };
    let work = job.trampoline();
    // SAFETY: 主队列是全局对象；同步执行，job 在返回前有效。
    unsafe {
        dispatch_sync_f(
            (&raw const _dispatch_main_q).cast(),
            std::ptr::from_mut(&mut job).cast(),
            work,
        );
    }
    job.out.expect("主队列任务已执行")
}

struct Job<F, R> {
    f: Option<F>,
    out: Option<R>,
}

impl<F: FnOnce(MainThreadMarker) -> R, R> Job<F, R> {
    fn trampoline(&self) -> extern "C" fn(*mut c_void) {
        Self::run
    }

    extern "C" fn run(context: *mut c_void) {
        // SAFETY: context 是 on_main 栈上的 Job，dispatch_sync_f 返回前一直有效、只在这里访问。
        let job = unsafe { &mut *context.cast::<Self>() };
        let mtm = MainThreadMarker::new().expect("主队列在主线程上执行");
        if let Some(f) = job.f.take() {
            job.out = Some(f(mtm));
        }
    }
}
