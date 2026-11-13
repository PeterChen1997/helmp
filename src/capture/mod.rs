//! 截图界面（对标 Snipaste）：先冻结屏幕，再在静止的画面上框选。
//!
//! 流程：截图命令返回 `Outcome::Capture` → 面板已收起 → [`open`] 截下每块屏（[`freeze`]）→
//! 每块屏一个铺满整屏的遮罩窗口（[`overlay`]），显示冻结的画面、选区外压暗。
//! - 截图模式：松开鼠标后选区留在原地，可拖控制点调整、在里面拖动平移；选区右下角外侧是工具栏
//!   可标注和打马赛克，操作按钮依次是「保存、钉图、识别文字、关闭、复制」。回车 / 选区里双击 = 默认动作（复制；「截图并保存」是保存），
//!   ⌘C 复制，⌘S 保存，Esc / ⌘W 关闭，右键取消选区（没有选区时右键关闭）。
//! - 识别文字模式：松开鼠标立即识别，结果显示在选区旁的浮窗里（[`result`]），文字自动复制。
//! - 钉图（[`pin`]）：把选区原地变成置顶的无边框小窗，可拖动、滚轮缩放，悬停出工具栏，Esc / ⌘W / 双击关闭，可以钉多张。
//! - 截图历史（[`history`]）：复制、保存、钉图、识别文字之后的最终图记进最近 10 张的历史，
//!   启动器里的「截图历史」可以再钉到屏幕上、复制、另存为、识别文字（[`Request::History`]）。
//!
//! 所有窗口都用 orderFrontRegardless 前置：别的 App 在前台时也一定看得到反馈。遮罩和结果窗口激活 Helmp
//! 拿键盘，关闭后把焦点还给截图前的 App（`panel::app_to_return_to` / `panel::return_focus`）。
//!
//! 副作用都过演练开关：`HELMP_DRY_RUN=1` 时复制只记一行日志，不碰用户剪贴板。
//! 图形自检（`HELMP_SELF_TEST`）时窗口都在屏幕外、不激活 Helmp，另存为不弹面板、直接存进默认目录。

mod annotation;
mod composite;
mod freeze;
mod geometry;
/// 启动器的「截图历史」（`providers::screenshot`）也读它。
pub mod history;
mod native;
mod overlay;
mod pin;
mod preview;
mod result;
mod screen_capture_kit;
mod selector;
mod toast;
mod toolbar;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use composite::{MosaicSource, crop_png};
use freeze::Frozen;
pub use freeze::fixture;
use geometry::Rect;
use gpui_kit::*;
use selector::Selector;

use crate::i18n::{tr, trf};
use crate::providers::screenshot;

/// 回车（以及选区里双击）执行的默认动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primary {
    Copy,
    Save,
}

/// 截图界面的模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 选好后留在原地，用工具栏或快捷键处理。
    Capture(Primary),
    /// 松开鼠标立即识别文字。
    Ocr,
}

impl Mode {
    /// 命令行、日志里的名字。
    pub fn name(self) -> &'static str {
        match self {
            Mode::Capture(Primary::Copy) => "copy",
            Mode::Capture(Primary::Save) => "save",
            Mode::Ocr => "ocr",
        }
    }
}

/// 从截图历史里重新打开一张时做什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reopen {
    Pin,
    Recognize,
    Save,
}

/// `Outcome::Capture` 要截图界面做的事：冻结屏幕后框选，或者把截图历史里的一张（按 id）重新打开。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Select(Mode),
    History(Reopen, u64),
}

impl From<Mode> for Request {
    fn from(mode: Mode) -> Self {
        Request::Select(mode)
    }
}

impl Request {
    /// 命令行、日志里的名字（`helmp run` 打印「open capture: <名字>」）。
    pub fn name(self) -> String {
        match self {
            Request::Select(mode) => mode.name().to_string(),
            Request::History(reopen, id) => {
                let act = match reopen {
                    Reopen::Pin => "pin",
                    Reopen::Recognize => "recognize",
                    Reopen::Save => "save",
                };
                format!("history-{act} {id}")
            }
        }
    }
}

/// 截图界面、钉图、识别结果窗口共用的「关闭」键：Esc，或者和普通窗口一样的 ⌘W。
pub fn is_close_key(keystroke: &Keystroke) -> bool {
    let m = &keystroke.modifiers;
    match keystroke.key.as_str() {
        "escape" => true,
        "w" => m.platform && !m.control && !m.alt && !m.shift,
        _ => false,
    }
}

/// 工具栏 / 快捷键触发的操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Save,
    Copy,
    Pin,
    Recognize,
    Close,
}

/// 一次截图：冻结下来的各块屏和各屏的选区。各屏的遮罩视图共用它。
pub struct Session {
    pub mode: Mode,
    pub screens: Vec<Frozen>,
    pub selectors: Vec<Selector>,
    editor: annotation::Editor,
    /// 当前屏的取样图，None 表示后台正在读取。
    mosaic: Option<(usize, Option<Arc<MosaicSource>>)>,
    pub overlays: Vec<AnyWindowHandle>,
    /// 正在裁图、等另存为面板：重复的操作忽略（连按两次回车不会复制两次）。
    pub busy: bool,
}

impl Session {
    fn mosaic(&self, index: usize) -> Option<&Arc<MosaicSource>> {
        self.mosaic
            .as_ref()
            .filter(|(ix, _)| *ix == index)
            .and_then(|(_, source)| source.as_ref())
    }

    fn select_tool(
        &mut self,
        index: usize,
        tool: Option<annotation::Tool>,
        cx: &mut Context<Self>,
    ) {
        if self.editor.drawing() {
            return;
        }
        self.editor.tool = tool.filter(|tool| Some(*tool) != self.editor.tool);
        if self.editor.tool != Some(annotation::Tool::Mosaic)
            || self.mosaic.as_ref().is_some_and(|(ix, _)| *ix == index)
        {
            return;
        }
        self.mosaic = Some((index, None));
        let screen = self.screens[index].clone();
        cx.spawn(async move |session, cx: &mut AsyncApp| {
            let source = cx
                .background_spawn(async move { MosaicSource::new(&screen.image, screen.scale) })
                .await;
            let _ = session.update(cx, |session, cx| {
                if matches!(session.mosaic, Some((ix, None)) if ix == index) {
                    session.mosaic = source.ok().map(|source| (index, Some(Arc::new(source))));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 选好的选区在哪块屏、是哪一块（同一时刻只有一块屏有选区）。
    pub fn selected(&self) -> Option<(usize, Rect)> {
        self.selectors
            .iter()
            .enumerate()
            .find_map(|(ix, selector)| {
                matches!(selector.state, selector::State::Selected { .. })
                    .then(|| selector.selection())
                    .flatten()
                    .map(|rect| (ix, rect))
            })
    }
}

/// 从截图里裁出来的一块，连同它在屏幕上的位置。钉图、识别结果窗口据此摆放。
#[derive(Clone)]
pub struct Shot {
    pub png: Arc<Vec<u8>>,
    pub pixels: (u32, u32),
    /// 选区（本屏坐标）。
    pub rect: Rect,
    /// 所在屏的 frame（AppKit 全局坐标）。
    pub screen: Rect,
    /// 所在屏的可见区域（本屏坐标）。
    pub visible: Rect,
}

#[derive(Default)]
struct Captures {
    session: Option<Entity<Session>>,
    /// 正在冻结屏幕（截图进程还没返回）：这期间再按快捷键不重复开。
    opening: bool,
    result: Option<(AnyWindowHandle, Entity<result::ResultView>)>,
    pins: Vec<(AnyWindowHandle, Entity<pin::Pin>)>,
    /// 关闭截图界面后把焦点还给谁（截图前的前台 App）。
    return_to: i32,
    /// 自检用：做过的事（复制了什么、存到哪）。
    log: Vec<String>,
}

impl Global for Captures {}

fn state(cx: &mut App) -> &mut Captures {
    if !cx.has_global::<Captures>() {
        cx.set_global(Captures::default());
    }
    cx.global_mut::<Captures>()
}

/// 图形自检时窗口摆在屏幕外、不激活 Helmp、不弹另存为面板。
pub fn offscreen() -> bool {
    crate::settings::self_test_offscreen()
}

/// 记一行发生了什么（自检读它判断复制 / 保存是否发生；`HELMP_TRACE` 时也打到 stderr）。
fn log(cx: &mut App, line: String) {
    crate::trace::log(format!("capture: {line}"));
    state(cx).log.push(line);
}

/// 打开截图界面（已经在截图时不重复打开），或者重新打开截图历史里的一张。
pub fn open(request: Request, panel_hidden_at: Option<Instant>, cx: &mut App) {
    let mode = match request {
        Request::Select(mode) => mode,
        Request::History(reopen, id) => {
            reopen_history(reopen, id, cx);
            return;
        }
    };
    let captures = state(cx);
    if captures.session.is_some() || captures.opening {
        return;
    }
    captures.opening = true;
    if !offscreen() {
        captures.return_to = crate::panel::app_to_return_to();
    }
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        state(cx).opening = false;
        return;
    };
    log(cx, format!("open {}", mode.name()));
    let screens = freeze::screens(mtm);
    let dir = freeze::session_dir();
    crate::trace::log("capture: screens ready");
    cx.spawn(async move |cx: &mut AsyncApp| {
        // 用户按下快捷键正等着：高优先级，且不让系统把此刻没有可见窗口的 Helmp 当后台 App 降速（App Nap）。
        let _awake = cx.background_executor().prevent_app_nap("冻结屏幕");
        if let Some(hidden_at) = panel_hidden_at.filter(|_| !offscreen()) {
            // 给窗口服务器时间撤下面板。面板已经收起的快捷键无需等待。
            let remaining = Duration::from_millis(80).saturating_sub(hidden_at.elapsed());
            if !remaining.is_zero() {
                cx.background_executor().timer(remaining).await;
            }
        }
        let frozen = cx
            .background_executor()
            .spawn_with_priority(Priority::High, async move { freeze::grab(screens, &dir) })
            .await;
        cx.update(|cx| {
            crate::trace::log("capture: frozen");
            state(cx).opening = false;
            match frozen {
                Ok(frozen) => open_overlays(mode, frozen, cx),
                Err(err) => fail(err, cx),
            }
        });
    })
    .detach();
}

/// 截图界面出错：在面板里大声说出来（面板为展示结果重新弹出时不会被焦点抖动收起，见 launcher）。
fn fail(err: anyhow::Error, cx: &mut App) {
    eprintln!("helmp: 截图失败：{err:#}");
    log(cx, format!("error {err:#}"));
    let item = crate::provider::Item {
        id: "screenshot:clipboard".into(),
        title: tr("截图", "Screenshot").into(),
        icon: crate::provider::Icon::Symbol {
            name: "square-dashed",
            tint: crate::provider::Tint::Indigo,
        },
        action: crate::provider::Action::new({
            let message = format!("{err:#}");
            move || Err(anyhow::anyhow!(message.clone()))
        }),
        ..crate::provider::Item::blank()
    };
    crate::launcher::LauncherHandle::update(cx, |launcher, window, cx| {
        launcher.present(item, window, cx)
    });
}

fn open_overlays(mode: Mode, frozen: Vec<Frozen>, cx: &mut App) {
    let offscreen = offscreen();
    let session = cx.new(|_| Session {
        mode,
        selectors: frozen.iter().map(|f| Selector::new(f.bounds())).collect(),
        screens: frozen.clone(),
        editor: Default::default(),
        mosaic: None,
        overlays: Vec::new(),
        busy: false,
    });
    let mouse = native::mouse_location();
    let mut key_index = 0;
    let mut overlays = Vec::new();
    for (ix, screen) in frozen.iter().enumerate() {
        let frame = screen.screen.frame;
        let options = WindowOptions {
            kind: WindowKind::PopUp,
            titlebar: None,
            focus: false,
            show: false,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(px(frame.w as f32), px(frame.h as f32)),
            ))),
            display_id: (screen.screen.id != 0)
                .then(|| DisplayId::new(u64::from(screen.screen.id))),
            ..Default::default()
        };
        let session_for_view = session.clone();
        let handle = match crate::settings::open_transparent_window(options, cx, |window, cx| {
            cx.new(|cx| overlay::Overlay::new(session_for_view, ix, window, cx))
        }) {
            Ok(handle) => handle,
            Err(err) => {
                // 一块屏开不出遮罩就整个放弃：只盖住部分屏幕的截图界面会让人困惑。
                for handle in overlays {
                    remove(handle, cx);
                }
                let _ = std::fs::remove_dir_all(freeze::session_dir());
                fail(
                    err.context(tr("打开截图界面失败", "Couldn't open the capture overlay")),
                    cx,
                );
                return;
            }
        };
        let image = screen.image.clone();
        let _ = handle.update(cx, |_, window, _| {
            native::setup(window, native::Kind::Overlay, frame, offscreen);
            native::underlay_image(window, &image, (frame.w, frame.h));
        });
        let contains = mouse.0 >= frame.x
            && mouse.0 <= frame.x + frame.w
            && mouse.1 >= frame.y
            && mouse.1 <= frame.y + frame.h;
        if contains {
            key_index = ix;
        }
        overlays.push(handle);
    }
    session.update(cx, |session, _| session.overlays = overlays.clone());
    // 先不抢焦点地把各屏的遮罩都摆上，最后让鼠标所在屏的那块成为 key window（键盘落在那里）。
    for (ix, handle) in overlays.iter().enumerate() {
        let key = ix == key_index;
        let _ = handle.update(cx, |_, window, _| native::present(window, key, offscreen));
    }
    state(cx).session = Some(session);
    crate::trace::log("capture: overlays ready");
}

fn remove(handle: AnyWindowHandle, cx: &mut App) {
    let _ = handle.update(cx, |_, window, _| window.remove_window());
}

/// 关掉截图遮罩，删掉冻结图。`return_focus`：没有接着打开要键盘的窗口（识别结果）时把焦点还回去。
fn close_session(return_focus: bool, cx: &mut App) {
    let Some(session) = state(cx).session.take() else {
        return;
    };
    let overlays = session.read(cx).overlays.clone();
    for handle in overlays {
        remove(handle, cx);
    }
    let _ = std::fs::remove_dir_all(freeze::session_dir());
    log(cx, "closed".into());
    if return_focus {
        give_focus_back(cx);
    }
}

/// 截图界面全部关掉后把焦点还给截图前的 App（窗口要等这一轮更新结束才真正关掉，所以推迟一下）。
fn give_focus_back(cx: &mut App) {
    if offscreen() {
        return;
    }
    let pid = state(cx).return_to;
    cx.defer(move |cx| {
        let captures = state(cx);
        if captures.session.is_none() && captures.result.is_none() {
            crate::panel::return_focus(pid, None);
        }
    });
}

/// 遮罩上的操作（工具栏按钮、快捷键）。在遮罩视图的事件里调用时要先 `cx.defer`：
/// 这里会关掉遮罩窗口，不能在那个窗口自己的更新中途做。
pub fn act(action: Act, cx: &mut App) {
    let Some(session) = state(cx).session.clone() else {
        return;
    };
    if action == Act::Close {
        log(cx, "cancel".into());
        close_session(true, cx);
        return;
    }
    let picked = session.update(cx, |session, cx| {
        if session.busy {
            return None;
        }
        let (ix, rect) = session.selected()?;
        if session.editor.drawing() {
            return None;
        }
        session.busy = true;
        cx.notify();
        Some((
            session.screens[ix].clone(),
            rect,
            session.editor.marks(),
            session.mosaic(ix).cloned(),
        ))
    });
    let Some((frozen, rect, marks, mosaic)) = picked else {
        return;
    };
    let Some(crop) = geometry::pixel_crop(rect, frozen.scale, frozen.pixels) else {
        session.update(cx, |session, cx| {
            session.busy = false;
            cx.notify();
        });
        return;
    };
    let image = frozen.image.clone();
    let scale = frozen.scale;
    cx.spawn(async move |cx: &mut AsyncApp| {
        let _awake = cx.background_executor().prevent_app_nap("裁剪截图");
        let png = cx
            .background_executor()
            .spawn_with_priority(Priority::High, async move {
                crop_png(&image, crop, scale, &marks, mosaic.as_deref())
            })
            .await;
        cx.update(|cx| {
            if state(cx).session.as_ref() != Some(&session) {
                return;
            }
            let png = match png {
                Ok(png) => png,
                Err(err) => {
                    close_session(true, cx);
                    fail(err, cx);
                    return;
                }
            };
            let shot = Shot {
                png: Arc::new(png),
                pixels: (crop.w, crop.h),
                rect,
                screen: frozen.screen.frame,
                visible: frozen.visible(),
            };
            // 「保存」在另存为面板真的存好之后才记进历史（取消了就不算完成），见 `finish_save`。
            match action {
                Act::Copy => {
                    close_session(true, cx);
                    if copy(&shot, cx) {
                        record_history(&shot, history::Ended::Copied, cx);
                    }
                }
                Act::Save => save(shot, true, cx),
                Act::Pin => {
                    close_session(true, cx);
                    record_history(&shot, history::Ended::Pinned, cx);
                    pin::open(shot, cx);
                }
                Act::Recognize => {
                    close_session(false, cx);
                    record_history(&shot, history::Ended::Recognized, cx);
                    result::open(shot, cx);
                }
                Act::Close => {}
            }
        });
    })
    .detach();
}

/// 复制图片并提示「已复制」，返回是否复制成了。演练模式下只记日志，不碰用户剪贴板。
pub fn copy(shot: &Shot, cx: &mut App) -> bool {
    let (w, h) = shot.pixels;
    if crate::effects::dry_run() {
        log(cx, format!("[dry-run] copy image {w}x{h}"));
    } else if let Err(err) =
        screenshot::pasteboard::write_png(&screenshot::pasteboard::general(), &shot.png)
    {
        fail(err, cx);
        return false;
    } else {
        log(cx, format!("copy image {w}x{h}"));
    }
    toast::show(tr("已复制到剪贴板", "Copied to Clipboard"), shot, cx);
    true
}

/// 把截图界面里完成的一张记进截图历史（后台写文件）。设置里关掉了就不记。
/// 记不下来不影响这次截图本身，但要大声说出来（stderr 和自检日志）。
fn record_history(shot: &Shot, ended: history::Ended, cx: &mut App) {
    if !history::enabled() {
        return;
    }
    let shot = shot.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let recorded = cx
            .background_spawn(async move {
                let record = history::Record {
                    png: &shot.png,
                    pixels: shot.pixels,
                    rect: shot.rect,
                    screen: shot.screen,
                    visible: shot.visible,
                    ended,
                };
                history::record(&history::dir()?, &record, history::now_ms())
            })
            .await;
        cx.update(|cx| match recorded {
            Ok(entry) => log(
                cx,
                format!("history {ended:?} {}x{}", entry.width, entry.height),
            ),
            Err(err) => {
                eprintln!("helmp: 记录截图历史失败：{err:#}");
                log(cx, format!("history error {err:#}"));
            }
        });
    })
    .detach();
}

/// 截图历史里的一张重新打开：钉到屏幕上、识别文字或另存为。读文件放后台，读不到（已被删除）就在面板里说出来。
fn reopen_history(reopen: Reopen, id: u64, cx: &mut App) {
    log(
        cx,
        format!("reopen {}", Request::History(reopen, id).name()),
    );
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return;
    };
    let screens: Vec<(Rect, Rect)> = freeze::screens(mtm)
        .into_iter()
        .map(|screen| (screen.frame, screen.visible))
        .collect();
    let mouse = native::mouse_location();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let loaded = cx
            .background_spawn(async move {
                let dir = history::dir()?;
                let entry = history::load_entry(&dir, id)?;
                let path = entry.png(&dir);
                let png = std::fs::read(&path)
                    .with_context(|| trf!("读取 {}", "Couldn't read {}", path.display()))?;
                anyhow::Ok((entry, png))
            })
            .await;
        cx.update(|cx| {
            let (entry, png) = match loaded {
                Ok(loaded) => loaded,
                Err(err) => {
                    fail(err, cx);
                    return;
                }
            };
            let (screen, visible, rect) = history::placement(&entry, &screens, mouse);
            let shot = Shot {
                png: Arc::new(png),
                pixels: (entry.width, entry.height),
                rect,
                screen,
                visible,
            };
            match reopen {
                Reopen::Pin => pin::open(shot, cx),
                Reopen::Recognize => {
                    remember_return_app(true, cx);
                    result::open(shot, cx);
                }
                Reopen::Save => {
                    remember_return_app(true, cx);
                    save(shot, false, cx);
                }
            }
        });
    })
    .detach();
}

/// 不经截图界面、直接打开要键盘的窗口（钉图或截图历史上的识别文字、另存为）之前：记下关掉后焦点还给谁。
/// Helmp 不在前台时就是此刻前台的 App；Helmp 在前台（比如识别结果窗口还开着）时沿用之前记下的。
/// `from_launcher`：从面板里执行的，面板刚收起、前台可能还是 Helmp，按面板收起时还回去的那个 App 算。
fn remember_return_app(from_launcher: bool, cx: &mut App) {
    if offscreen() || (!from_launcher && native::app_is_active()) {
        return;
    }
    state(cx).return_to = crate::panel::app_to_return_to();
}

/// 另存为 PNG：弹出另存为面板（默认目录是设置里的截图目录、文件名带时间），存好后提示。
/// `from_overlay`：从遮罩上保存时，另存为面板出现期间先收起遮罩（屏保层级会盖住面板），取消就放回来。
pub fn save(shot: Shot, from_overlay: bool, cx: &mut App) {
    let dir = match screenshot::save_dir(crate::config::get().screenshot.save_dir.as_deref())
        .and_then(|dir| {
            std::fs::create_dir_all(&dir)
                .with_context(|| trf!("创建 {}", "Couldn't create {}", dir.display()))?;
            Ok(dir)
        }) {
        Ok(dir) => dir,
        Err(err) => {
            close_session(true, cx);
            fail(err, cx);
            return;
        }
    };
    let ts = screenshot::timestamp();
    let name = trf!("Helmp 截图 {ts}.png", "Helmp Screenshot {ts}.png");
    if offscreen() {
        // 自检：不弹面板，直接存进默认目录（e2e 的数据目录是临时的）。
        finish_save(&shot, Some(dir.join(name)), from_overlay, cx);
        return;
    }
    set_overlays_visible(false, cx);
    cx.activate(true);
    let chosen = cx.prompt_for_new_path(&dir, Some(&name));
    cx.spawn(async move |cx: &mut AsyncApp| {
        let chosen = chosen.await;
        cx.update(|cx| match chosen {
            Ok(Ok(path)) => finish_save(&shot, path, from_overlay, cx),
            Ok(Err(err)) => {
                close_session(true, cx);
                fail(err, cx);
            }
            Err(_) => finish_save(&shot, None, from_overlay, cx),
        });
    })
    .detach();
}

fn finish_save(shot: &Shot, path: Option<PathBuf>, from_overlay: bool, cx: &mut App) {
    let Some(path) = path else {
        // 取消另存为：回到选区，什么都没丢。
        log(cx, "save cancelled".into());
        if from_overlay && let Some(session) = state(cx).session.clone() {
            session.update(cx, |session, cx| {
                session.busy = false;
                cx.notify();
            });
            set_overlays_visible(true, cx);
        }
        return;
    };
    let path = with_png_extension(path);
    if let Err(err) = std::fs::write(&path, shot.png.as_slice())
        .with_context(|| trf!("写入 {}", "Couldn't write {}", path.display()))
    {
        close_session(true, cx);
        fail(err, cx);
        return;
    }
    log(cx, format!("saved {}", path.display()));
    if from_overlay {
        close_session(true, cx);
        record_history(shot, history::Ended::Saved, cx);
    } else {
        give_focus_back(cx);
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    toast::show(&trf!("已保存 {name}", "Saved {name}"), shot, cx);
}

/// 用户在另存为面板里删掉了扩展名时补回 .png（存出来的就是 PNG）。
fn with_png_extension(path: PathBuf) -> PathBuf {
    let is_png = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"));
    if is_png {
        path
    } else {
        let mut name = path.clone().into_os_string();
        name.push(".png");
        PathBuf::from(name)
    }
}

fn set_overlays_visible(visible: bool, cx: &mut App) {
    let Some(session) = state(cx).session.clone() else {
        return;
    };
    let overlays = session.read(cx).overlays.clone();
    let offscreen = offscreen();
    for handle in overlays {
        let _ = handle.update(cx, |_, window, _| {
            native::set_visible(window, visible, offscreen)
        });
    }
}

/// 图形自检：此刻该往哪个窗口送按键和鼠标（遮罩 → 识别结果 → 最新的钉图）。
pub fn self_test_target(cx: &mut App) -> Option<AnyWindowHandle> {
    let captures = state(cx);
    if let Some(session) = captures.session.clone() {
        return session.read(cx).overlays.first().copied();
    }
    let captures = state(cx);
    captures
        .result
        .as_ref()
        .map(|(handle, _)| *handle)
        .or_else(|| captures.pins.last().map(|(handle, _)| *handle))
}

/// README 配图 / 自检：截图窗口里垫在 GPUI 下面的截图（钉图的图片、遮罩的冻结画面）另存一张 PNG。
#[cfg(feature = "readme-shots")]
pub fn snapshot_underlay(window: &Window, path: &std::path::Path) -> anyhow::Result<()> {
    native::snapshot_underlay(window, path)
}

/// 图形自检：截图界面此刻的状态，一行打印。
pub fn debug_state(cx: &mut App) -> String {
    let session = state(cx).session.clone();
    let (overlays, selection, busy) = match &session {
        Some(session) => {
            let session = session.read(cx);
            (
                session.overlays.len(),
                session
                    .selected()
                    .map(|(_, rect)| (rect.x, rect.y, rect.w, rect.h)),
                session.busy,
            )
        }
        None => (0, None, false),
    };
    let result = state(cx).result.as_ref().map(|(_, view)| view.clone());
    let result = result.map(|view| view.read(cx).summary());
    // 识别结果窗口里键盘焦点是否在结果上（回车、⌘T 不用先点一下就能用）。
    let result_focused = state(cx).result.clone().and_then(|(handle, view)| {
        handle
            .update(cx, |_, window, cx| view.read(cx).focused(window, cx))
            .ok()
    });
    // 每张钉图「宽x高@缩放」，最新的在后。
    let pins: Vec<String> = state(cx)
        .pins
        .clone()
        .into_iter()
        .filter_map(|(handle, pin)| {
            handle
                .update(cx, |_, window, cx| pin.read(cx).summary(window))
                .ok()
        })
        .collect();
    let captures = state(cx);
    format!(
        "overlays={overlays} selection={selection:?} busy={busy} result={result:?} result_focused={result_focused:?} pins={} pin_sizes={pins:?} log={:?}",
        captures.pins.len(),
        captures.log
    )
}

/// 已打开的识别结果窗口（只留一个：新的替换旧的）。
fn set_result(window: Option<(AnyWindowHandle, Entity<result::ResultView>)>, cx: &mut App) {
    if let Some((old, _)) = std::mem::replace(&mut state(cx).result, window) {
        remove(old, cx);
    }
}

#[cfg(test)]
mod tests {
    // gpui_kit::* 里也有一个 `test` 属性宏，经 `super::*` 引入后会遮住标准库的。
    use core::prelude::v1::test;

    use super::geometry::PixelRect;
    use super::*;
    use std::path::Path;

    #[test]
    fn crop_keeps_full_pixel_resolution() {
        // 裁出来的必须是选区里的原始像素（Retina 下是点数的 2 倍），而不是按点缩小后的糊图。
        let fixture = crate::providers::screenshot::ocr::tests::FIXTURE;
        let image = composite::load_image(Path::new(fixture)).unwrap();
        let (w, h) = freeze::png_size(&std::fs::read(fixture).unwrap()).unwrap();
        let rect = PixelRect {
            x: 2,
            y: 4,
            w: w / 2,
            h: h / 2,
        };
        let png = crop_png(&image, rect, 2.0, &[], None).unwrap();
        assert_eq!(freeze::png_size(&png), Some((w / 2, h / 2)));
        // 裁剪框超出图片：系统会裁到图内，不会崩。
        let edge = PixelRect {
            x: w - 10,
            y: h - 10,
            w: 50,
            h: 50,
        };
        assert_eq!(
            freeze::png_size(&crop_png(&image, edge, 1.0, &[], None).unwrap()),
            Some((10, 10))
        );
        assert!(composite::load_image(Path::new("/nonexistent/helmp.png")).is_err());
    }

    #[test]
    fn saved_files_always_end_in_png() {
        // 另存为面板里用户可能删掉扩展名：存出来的是 PNG，文件名就得是 .png，否则双击打不开。
        assert_eq!(
            with_png_extension(PathBuf::from("/tmp/a.png")),
            PathBuf::from("/tmp/a.png")
        );
        assert_eq!(
            with_png_extension(PathBuf::from("/tmp/a.PNG")),
            PathBuf::from("/tmp/a.PNG")
        );
        assert_eq!(
            with_png_extension(PathBuf::from("/tmp/a")),
            PathBuf::from("/tmp/a.png")
        );
        assert_eq!(
            with_png_extension(PathBuf::from("/tmp/a.jpg")),
            PathBuf::from("/tmp/a.jpg.png")
        );
    }

    #[test]
    fn mode_names_are_stable() {
        // 命令行 `helmp run` 和 e2e 测试按这些名字认模式。
        assert_eq!(Mode::Capture(Primary::Copy).name(), "copy");
        assert_eq!(Mode::Capture(Primary::Save).name(), "save");
        assert_eq!(Mode::Ocr.name(), "ocr");
        assert_eq!(Request::from(Mode::Ocr).name(), "ocr");
        assert_eq!(
            Request::History(Reopen::Pin, 1731480000123).name(),
            "history-pin 1731480000123"
        );
    }

    #[test]
    fn escape_and_cmd_w_close_capture_windows() {
        // 截图窗口和普通窗口一样认 ⌘W：用户习惯了 ⌘W 关窗口，按了没反应就像卡住了。
        // 但只认单纯的 ⌘W：⌘⇧W、⌃W 这些不是「关闭」，光按 W 也不是（遮罩里字母键是标注工具）。
        let close = |spec: &'static str| is_close_key(&Keystroke::parse(spec).unwrap());
        assert!(close("escape"));
        assert!(close("cmd-w"));
        assert!(!close("w"));
        assert!(!close("cmd-shift-w"));
        assert!(!close("ctrl-w"));
        assert!(!close("cmd-c"));
        assert!(!close("enter"));
    }
}
