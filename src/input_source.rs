//! 唤起面板时切到英文键盘布局，隐藏时切回原输入法。
//!
//! 原因：中文输入法下打 `wx` 只会进入组字（marked text），搜索框收不到变化；Esc 还会被输入法
//! 拿去取消组字，关不掉面板。启动器的查询本来就支持拼音缩写，所以唤起时用 ABC 更顺手。
//! 设置 `HUAN_KEEP_INPUT_SOURCE=1` 可关闭此行为。

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_foundation::NSString;

type TISInputSourceRef = *const c_void;
type CFStringRef = *const c_void;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> TISInputSourceRef;
    fn TISCopyCurrentASCIICapableKeyboardLayoutInputSource() -> TISInputSourceRef;
    fn TISSelectInputSource(source: TISInputSourceRef) -> i32;
    fn TISGetInputSourceProperty(source: TISInputSourceRef, key: CFStringRef) -> *const c_void;
    static kTISPropertyInputSourceID: CFStringRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const c_void);
}

/// 持有一个 +1 引用的输入源，析构时释放。
struct Source(NonNull<c_void>);

impl Source {
    /// SAFETY: `ptr` 必须来自 TIS 的 Copy 函数（调用方拥有一个引用），或为空。
    unsafe fn from_copy(ptr: TISInputSourceRef) -> Option<Self> {
        NonNull::new(ptr.cast_mut()).map(Self)
    }

    fn id(&self) -> String {
        // SAFETY: self.0 是有效的输入源；返回的 CFString 归输入源所有，不需要释放。
        // CFString 与 NSString 免费桥接，借用期间输入源仍存活。
        unsafe {
            let id = TISGetInputSourceProperty(self.0.as_ptr(), kTISPropertyInputSourceID);
            match id.cast::<NSString>().as_ref() {
                Some(id) => id.to_string(),
                None => String::new(),
            }
        }
    }

    fn select(&self) -> i32 {
        // SAFETY: self.0 是有效的输入源。
        unsafe { TISSelectInputSource(self.0.as_ptr()) }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: 我们拥有这个引用。
        unsafe { CFRelease(self.0.as_ptr()) }
    }
}

fn enabled() -> bool {
    std::env::var_os("HUAN_KEEP_INPUT_SOURCE").is_none()
}

/// 面板显示期间切到英文布局，记住原输入法以便恢复。只能在主线程调用。
#[derive(Default)]
pub struct InputSourceSwitch {
    previous: Option<Source>,
}

impl InputSourceSwitch {
    pub fn on_show(&mut self) {
        if !enabled() || self.previous.is_some() {
            return;
        }
        // SAFETY: 两个 Copy 函数返回 +1 引用，交给 Source 管理。
        let (current, ascii) = unsafe {
            (
                Source::from_copy(TISCopyCurrentKeyboardInputSource()),
                Source::from_copy(TISCopyCurrentASCIICapableKeyboardLayoutInputSource()),
            )
        };
        let (Some(current), Some(ascii)) = (current, ascii) else {
            eprintln!("huan: 读取当前输入法失败，保持不变");
            return;
        };
        let (from, to) = (current.id(), ascii.id());
        if from == to {
            crate::trace::log(format!("input source 已是 {to}，不切换"));
            return;
        }
        let status = ascii.select();
        crate::trace::log(format!("input source {from} → {to}（status={status}）"));
        if status == 0 {
            self.previous = Some(current);
        } else {
            eprintln!("huan: 切换到 {to} 失败（OSStatus {status}）");
        }
    }

    pub fn on_hide(&mut self) {
        let Some(previous) = self.previous.take() else {
            return;
        };
        let status = previous.select();
        crate::trace::log(format!(
            "input source 恢复 {}（status={status}）",
            previous.id()
        ));
        if status != 0 {
            eprintln!(
                "huan: 恢复输入法 {} 失败（OSStatus {status}）",
                previous.id()
            );
        }
    }
}
