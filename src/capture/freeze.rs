//! 冻结屏幕：先把每块屏此刻的画面截成图，遮罩窗口铺满屏幕显示这张静止的图，用户在上面框选。
//!
//! macOS 26+ 通过 ScreenCaptureKit 直接取内存里的 CGImage，保留显示器的色彩空间和像素分辨率。
//! 旧系统或框架取屏失败时，回退到继承 Helmp 录屏授权的 `screencapture -x`。
//!
//! 文件顺序与 `CGGetActiveDisplayList` 一致；换算时用图片宽高比与屏幕宽高比互相校验，对不上就报错，
//! 绝不把 A 屏的图铺到 B 屏上。冻结图放在系统临时目录（`$TMPDIR/helmp-capture-<pid>/`），截图结束即删除。
//!
//! 测试接缝：`HELMP_CAPTURE_FIXTURE=<png>` 时不截屏，把这张图当成唯一一块屏的画面
//! （`HELMP_CAPTURE_FIXTURE_SCALE` 指定像素 / 点的倍数，缺省 1），不需要录屏权限。

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};
use objc2::MainThreadMarker;
use objc2::rc::autoreleasepool;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSScreen;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{
    CGDisplayBounds, CGGetActiveDisplayList, CGImage, CGWindowListCopyWindowInfo, CGWindowListOption,
    kCGNullWindowID,
};
use objc2_foundation::{NSArray, NSRect};

use super::geometry::{self, Rect};
use crate::i18n::{tr, trf};
use crate::permissions::{double, object_for};

/// 小于这个尺寸（pt）的窗口不参与悬停高亮（状态栏小图标、隐形的辅助窗口）。
const MIN_WINDOW: f64 = 24.0;

/// 一块屏。
#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    /// CGDirectDisplayID。
    pub id: u32,
    /// NSScreen.frame（AppKit 全局坐标）。
    pub frame: Rect,
    /// NSScreen.visibleFrame（AppKit 全局坐标，去掉菜单栏和程序坞）。
    pub visible: Rect,
    pub scale: f64,
    /// 这块屏上的窗口（本屏坐标，从前到后），悬停高亮用。
    pub windows: Vec<Rect>,
}

/// 冻结下来的一块屏。
#[derive(Clone)]
pub struct Frozen {
    pub screen: Screen,
    /// 整屏图像，遮罩、裁剪和马赛克共用。
    pub image: CFRetained<CGImage>,
    /// 截图的像素尺寸。
    pub pixels: (u32, u32),
    /// 像素 / 点（Retina 屏为 2）。
    pub scale: f64,
}

impl Frozen {
    /// 本屏范围（本屏坐标）。
    pub fn bounds(&self) -> Rect {
        Rect::new(0.0, 0.0, self.screen.frame.w, self.screen.frame.h)
    }

    /// 可见区域（本屏坐标）：识别结果窗口要完整落在这里面，不压菜单栏和程序坞。
    pub fn visible(&self) -> Rect {
        geometry::from_appkit(self.screen.frame, self.screen.visible)
    }
}

/// 测试接缝：用一张图片代替真实截屏。
pub fn fixture() -> Option<PathBuf> {
    std::env::var_os("HELMP_CAPTURE_FIXTURE").map(PathBuf::from)
}

/// 本次截图的临时目录（每次重建，结束时整个删掉）。
pub fn session_dir() -> PathBuf {
    std::env::temp_dir().join(format!("helmp-capture-{}", std::process::id()))
}

/// 主线程：各块屏的位置和编号。
pub fn screens(mtm: MainThreadMarker) -> Vec<Screen> {
    NSScreen::screens(mtm)
        .iter()
        .filter_map(|screen| {
            let description = screen.deviceDescription();
            let number = object_for(&description, "NSScreenNumber")?;
            let id = double(&number)? as u32;
            Some(Screen {
                id,
                frame: rect(screen.frame()),
                visible: rect(screen.visibleFrame()),
                scale: screen.backingScaleFactor(),
                windows: Vec::new(),
            })
        })
        .collect()
}

fn rect(r: NSRect) -> Rect {
    Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

/// 屏幕上的普通窗口（CoreGraphics 全局坐标，从前到后）。不含 Helmp 自己的窗口、程序坞和菜单栏这类非普通层。
fn window_list() -> Vec<Rect> {
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCopyWindowInfo(options, kCGNullWindowID) else {
        return Vec::new();
    };
    // SAFETY: CFArray 与 NSArray 免费桥接；`list` 在本函数内一直持有。
    let array: &NSArray<AnyObject> = unsafe { &*(std::ptr::from_ref(&*list).cast()) };
    let own_pid = f64::from(std::process::id());
    let number = |entry: &AnyObject, key| object_for(entry, key).and_then(|value| double(&value));
    array
        .iter()
        .filter(|entry| {
            number(entry, "kCGWindowOwnerPID") != Some(own_pid)
                && number(entry, "kCGWindowLayer") == Some(0.0)
                && number(entry, "kCGWindowAlpha").is_none_or(|alpha| alpha > 0.0)
        })
        .filter_map(|entry| {
            let bounds = object_for(&entry, "kCGWindowBounds")?;
            let get = |key| number(&bounds, key);
            let rect = Rect::new(get("X")?, get("Y")?, get("Width")?, get("Height")?);
            (rect.w >= MIN_WINDOW && rect.h >= MIN_WINDOW).then_some(rect)
        })
        .collect()
}

/// 后台线程：冻结所有显示器。
pub fn grab(screens: Vec<Screen>, dir: &Path) -> Result<Vec<Frozen>> {
    if fixture().is_none()
        && let Some(result) = super::screen_capture_kit::grab(&screens)
    {
        match result {
            Ok(frozen) => return Ok(frozen),
            Err(err) => crate::trace::log(format!("capture: ScreenCaptureKit fallback: {err:#}")),
        }
    }
    grab_from_disk(screens, dir)
}

fn grab_from_disk(mut screens: Vec<Screen>, dir: &Path) -> Result<Vec<Frozen>> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)
        .with_context(|| trf!("创建 {}", "Couldn't create {}", dir.display()))?;
    if let Some(fixture) = fixture() {
        return from_fixture(&fixture, screens.first(), dir);
    }
    let active = active_displays();
    anyhow::ensure!(
        !active.is_empty(),
        tr("没有找到可用的显示器", "No active display found")
    );
    let files: Vec<PathBuf> = (0..active.len())
        .map(|ix| dir.join(format!("screen-{ix}.png")))
        .collect();
    let capture = Command::new("screencapture")
        .arg("-x")
        .args(["-t", "png"])
        .args(&files)
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| tr("启动 screencapture 失败", "Couldn't start screencapture"))?;
    fill_windows(&mut screens);
    let output = capture
        .wait_with_output()
        .context(tr("等待系统截图", "Couldn't wait for the screenshot"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    anyhow::ensure!(
        output.status.success() && files.iter().all(|file| file.exists()),
        trf!(
            "截取屏幕失败（{}）：{}",
            "Couldn't capture the screen ({}): {}",
            output.status,
            stderr.trim()
        )
    );
    screens
        .into_iter()
        .map(|screen| {
            let ix = active
                .iter()
                .position(|id| *id == screen.id)
                .with_context(|| {
                    trf!(
                        "显示器 {} 不在系统的活动显示器列表里",
                        "Display {} isn't in the active display list",
                        screen.id
                    )
                })?;
            frozen(screen, files[ix].clone())
        })
        .collect()
}

/// 读出截图的像素尺寸并核对宽高比：对不上说明文件和屏幕的对应关系错了，必须报错而不是显示错位的画面。
fn frozen(screen: Screen, image: PathBuf) -> Result<Frozen> {
    let pixels = read_png_size(&image)?;
    let scale = backing_scale(pixels, (screen.frame.w, screen.frame.h)).with_context(|| {
        trf!(
            "截图（{}×{} 像素）和显示器（{}×{} 点）对不上",
            "The capture ({}×{} px) doesn't match the display ({}×{} pt)",
            pixels.0,
            pixels.1,
            screen.frame.w,
            screen.frame.h
        )
    })?;
    Ok(Frozen {
        screen,
        image: super::composite::load_image(&image)?,
        pixels,
        scale,
    })
}

/// 测试接缝：把夹具图复制进会话目录（会话结束要删目录，不能删到仓库里的夹具），当成一块屏。
fn from_fixture(fixture: &Path, main: Option<&Screen>, dir: &Path) -> Result<Vec<Frozen>> {
    let image = dir.join("screen-0.png");
    std::fs::copy(fixture, &image)
        .with_context(|| trf!("读取 {}", "Couldn't read {}", fixture.display()))?;
    let pixels = read_png_size(&image)?;
    let scale: f64 = std::env::var("HELMP_CAPTURE_FIXTURE_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|s: &f64| *s > 0.0)
        .unwrap_or(1.0);
    let (w, h) = (f64::from(pixels.0) / scale, f64::from(pixels.1) / scale);
    // 放在主屏左上角（自检时窗口会挪到屏幕外，这里的位置不重要）。
    let origin = main.map_or((0.0, 0.0), |main| {
        (main.frame.x, main.frame.y + main.frame.h - h)
    });
    let frame = Rect::new(origin.0, origin.1, w, h);
    Ok(vec![Frozen {
        screen: Screen {
            id: main.map_or(0, |main| main.id),
            frame,
            visible: frame,
            scale,
            windows: Vec::new(),
        },
        image: super::composite::load_image(&image)?,
        pixels,
        scale,
    }])
}

/// 抓图时枚举普通窗口，供鼠标悬停选区使用。
pub(super) fn fill_windows(screens: &mut [Screen]) {
    let windows = autoreleasepool(|_| window_list());
    for screen in screens {
        let cg = CGDisplayBounds(screen.id);
        let cg = Rect::new(cg.origin.x, cg.origin.y, cg.size.width, cg.size.height);
        let local = Rect::new(0.0, 0.0, cg.w, cg.h);
        screen.windows = windows
            .iter()
            .map(|window| geometry::cg_to_local(cg, *window))
            .filter(|window| window.intersect(&local).is_some())
            .collect();
    }
}

fn active_displays() -> Vec<u32> {
    let mut ids = [0u32; 32];
    let mut count = 0u32;
    // SAFETY: 缓冲区长度与 max_displays 一致，count 是有效指针。
    let err = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    if err.0 != 0 {
        eprintln!("helmp: CGGetActiveDisplayList 失败：{}", err.0);
        return Vec::new();
    }
    ids[..count as usize].to_vec()
}

fn read_png_size(path: &Path) -> Result<(u32, u32)> {
    let mut header = [0; 24];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .with_context(|| trf!("读取 {}", "Couldn't read {}", path.display()))?;
    png_size(&header)
        .with_context(|| trf!("{} 不是 PNG 图片", "{} isn't a PNG image", path.display()))
}

/// PNG 文件头里的宽高（IHDR 紧跟在 8 字节签名之后）。
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let word = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    Some((word(16)?, word(20)?))
}

/// 像素 / 点。宽、高两个方向算出的倍数必须一致（容差 1%），否则说明图和屏对不上。
pub fn backing_scale(pixels: (u32, u32), points: (f64, f64)) -> Option<f64> {
    if points.0 <= 0.0 || points.1 <= 0.0 || pixels.0 == 0 || pixels.1 == 0 {
        return None;
    }
    let sx = f64::from(pixels.0) / points.0;
    let sy = f64::from(pixels.1) / points.1;
    ((sx - sy).abs() <= sx * 0.01).then_some(sx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_png_size_from_header() {
        // 冻结图很大（5K 屏 5120×2880），只为拿尺寸不该整张解码。
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/ocr-zh-en.png"
        ))
        .unwrap();
        let (w, h) = png_size(&fixture).unwrap();
        assert!(w > 0 && h > 0);
        assert_eq!(png_size(b"GIF89a not a png"), None);
        assert_eq!(png_size(b"\x89PNG\r\n\x1a\n"), None);
    }

    #[test]
    fn backing_scale_must_agree_in_both_directions() {
        // Retina 主屏 1512×982 pt → 3024×1964 px：倍数 2。
        assert_eq!(backing_scale((3024, 1964), (1512.0, 982.0)), Some(2.0));
        assert_eq!(backing_scale((1920, 1080), (1920.0, 1080.0)), Some(1.0));
        // 文件对应错了屏（宽高比不同）：必须拒绝，不能把别的屏的画面铺上来。
        assert_eq!(backing_scale((2560, 1440), (1512.0, 982.0)), None);
        assert_eq!(backing_scale((0, 0), (1512.0, 982.0)), None);
    }
}
