//! 区域录屏：对截图界面里框出的一块区域录 mp4（ScreenCaptureKit，见 [`sck`]）。
//!
//! 两个入口：截图工具栏切到「录屏」点「开始录制」（[`Entry::Toolbar`]），或「区域录屏」快捷键框选后松手即录
//! （[`Entry::Hotkey`]）。录制中：
//! - 选区外侧一圈红框（[`native::Kind::Frame`] 窗口，不接收鼠标），选区下方居中一条深色浮条：计时、声音状态、停止；
//! - 菜单栏上一个计时按钮，点一下停止；
//! - 再按一次区域录屏快捷键、按 Esc、点浮条上的停止都能停。
//!
//! 红框和浮条都是 Helmp 的窗口，录制时整个 Helmp 从画面里排除，录不进去。
//! 停止后 mp4 存到截图目录（`config.screenshot.save_dir`），弹出可点的提示：复制文件、在访达中显示。
//!
//! 声音：系统声音、麦克风两个开关记在配置里（截图工具栏、框选提示条、设置都改它）。
//! 麦克风没授权时这一次只录画面（加系统声音），并提示去系统设置打开。
//!
//! 测试接缝：截图夹具（`HELMP_CAPTURE_FIXTURE`）下不真的录，开始 / 停止都立即成功，
//! 停止时写一个占位文件，好让自检走完「存好 → 提示」的整条路。

mod mic;
mod native;
mod sck;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::geometry::{self, Rect};
use super::toast::{self, Placement, ToastAction};
use super::{log, offscreen};
use crate::i18n::{tr, trf};
use crate::providers::screenshot;

/// 红框画在选区外多远（pt）：框本身 2 pt，和选区之间留 1 pt，录出来的画面里一点红都没有。
const FRAME_OUTSET: f64 = 3.0;
const FRAME_WIDTH: f32 = 2.;
/// 浮条大小与离选区的距离（pt）。
const PILL_SIZE: (f64, f64) = (236.0, 36.0);
const PILL_GAP: f64 = 10.0;
/// 停止后等 mp4 写完最多多久；超时就看文件在不在。
const FINISH_TIMEOUT: Duration = Duration::from_secs(8);
const RED: u32 = 0xFF3B30;

/// 从哪开始录的（埋点用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Toolbar,
    Hotkey,
}

impl Entry {
    fn name(self) -> &'static str {
        match self {
            Entry::Toolbar => "toolbar",
            Entry::Hotkey => "hotkey",
        }
    }
}

/// 怎么停的（埋点、日志用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopBy {
    Button,
    Hotkey,
    Esc,
    MenuBar,
    Quit,
    /// 系统把流停了（显示器断开、权限被收回……）。
    Error,
}

impl StopBy {
    fn name(self) -> &'static str {
        match self {
            StopBy::Button => "button",
            StopBy::Hotkey => "hotkey",
            StopBy::Esc => "esc",
            StopBy::MenuBar => "menubar",
            StopBy::Quit => "quit",
            StopBy::Error => "error",
        }
    }
}

/// 录不录声音。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Audio {
    pub system: bool,
    pub microphone: bool,
}

/// 配置里记住的声音开关。
pub fn audio() -> Audio {
    let config = crate::config::get();
    Audio {
        system: config.screenshot.record_system_audio,
        microphone: config.screenshot.record_microphone,
    }
}

/// 截图工具栏 / 框选提示条上点了声音开关：翻转并记进配置（下次录屏沿用）。
pub fn toggle_audio(microphone: bool) {
    let result = crate::config::update(|config| {
        let shot = &mut config.screenshot;
        if microphone {
            shot.record_microphone = !shot.record_microphone;
        } else {
            shot.record_system_audio = !shot.record_system_audio;
        }
    });
    if let Err(err) = result {
        eprintln!("helmp: 保存录屏声音开关失败：{err:#}");
        crate::telemetry::report("recording.settings", &err);
    }
}

/// 要录的区域。
#[derive(Debug, Clone, Copy)]
pub struct Target {
    /// CGDirectDisplayID。
    pub display: u32,
    /// 所在屏的 frame（AppKit 全局坐标）。
    pub screen: Rect,
    /// 所在屏的可见区域（本屏坐标）。
    pub visible: Rect,
    /// 选区（本屏坐标）。
    pub rect: Rect,
    /// 像素 / 点。
    pub scale: f64,
}

impl Target {
    fn placement(&self) -> Placement {
        Placement {
            rect: self.rect,
            screen: self.screen,
            visible: self.visible,
        }
    }
}

enum Backend {
    Real(Rc<sck::Recorder>),
    /// 测试夹具：不真的录。
    Fake,
}

struct Active {
    id: u64,
    target: Target,
    entry: Entry,
    path: PathBuf,
    started: Instant,
    backend: Backend,
    frame: AnyWindowHandle,
    pill: (AnyWindowHandle, Entity<Pill>),
    status: Option<native::StatusItem>,
    _esc: Option<native::EscMonitor>,
    /// 已经在停（等 mp4 写完）：重复的停止忽略。
    stopping: Option<StopBy>,
    /// 退出时同步等写完用。
    events: async_channel::Receiver<sck::Event>,
}

#[derive(Default)]
struct Recording {
    active: Option<Active>,
    /// 正在开始（等权限、等系统开流）：这期间的再次开始忽略。
    starting: bool,
    next_id: u64,
}

impl Global for Recording {}

fn state(cx: &mut App) -> &mut Recording {
    if !cx.has_global::<Recording>() {
        cx.set_global(Recording::default());
    }
    cx.global_mut::<Recording>()
}

/// 正在录（或正在开始 / 停止）。
pub fn busy(cx: &mut App) -> bool {
    let recording = state(cx);
    recording.starting || recording.active.is_some()
}

/// 「12:05」「1:02:03」。
pub fn clock(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// 红框窗口的位置（本屏坐标）：选区往外扩 [`FRAME_OUTSET`]。
fn frame_rect(rect: Rect) -> Rect {
    Rect::new(
        rect.x - FRAME_OUTSET,
        rect.y - FRAME_OUTSET,
        rect.w + 2.0 * FRAME_OUTSET,
        rect.h + 2.0 * FRAME_OUTSET,
    )
}

/// 浮条的位置（本屏坐标）：选区下方居中；下面放不下放上面；上下都放不下（选区几乎整屏）就放在选区里靠下
/// （Helmp 的窗口不会被录进去）。横向始终留在可见区域里。
fn pill_rect(rect: Rect, visible: Rect) -> Rect {
    let (w, h) = PILL_SIZE;
    let x = (rect.x + rect.w / 2.0 - w / 2.0).clamp(visible.x, (visible.right() - w).max(visible.x));
    let below = rect.bottom() + FRAME_OUTSET + PILL_GAP;
    let above = rect.y - FRAME_OUTSET - PILL_GAP - h;
    let y = if below + h <= visible.bottom() {
        below
    } else if above >= visible.y {
        above
    } else {
        (rect.bottom() - PILL_GAP - h).clamp(visible.y, (visible.bottom() - h).max(visible.y))
    };
    Rect::new(x, y, w, h)
}

/// 录好的文件名：「Helmp 录屏 2026-11-17 15.04.05.mp4」。
fn file_name() -> String {
    let ts = screenshot::timestamp();
    trf!("Helmp 录屏 {ts}.mp4", "Helmp Recording {ts}.mp4")
}

fn output_path() -> Result<PathBuf> {
    let dir = screenshot::save_dir(crate::config::get().screenshot.save_dir.as_deref())?;
    std::fs::create_dir_all(&dir)
        .with_context(|| trf!("创建 {}", "Couldn't create {}", dir.display()))?;
    Ok(dir.join(file_name()))
}

/// 开始录 `target`。已经在录时忽略。
pub fn start(target: Target, entry: Entry, cx: &mut App) {
    let recording = state(cx);
    if recording.starting || recording.active.is_some() {
        crate::trace::log("record: start ignored, already recording");
        return;
    }
    recording.starting = true;
    recording.next_id += 1;
    let id = recording.next_id;
    let path = match output_path() {
        Ok(path) => path,
        Err(err) => {
            state(cx).starting = false;
            start_failed(err, None, cx);
            return;
        }
    };
    let fake = super::fixture().is_some();
    let wanted = audio();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let _awake = cx.background_executor().prevent_app_nap("开始录屏");
        // 麦克风：没问过就现在问；拒绝了就这一次不录麦克风，开录后提示去哪里打开。
        let mut audio = wanted;
        let mut mic_denied = false;
        if audio.microphone && !fake {
            let granted = match mic::access() {
                mic::Access::Granted => true,
                mic::Access::NotDetermined => mic::request().await,
                mic::Access::Denied => false,
            };
            if !granted {
                audio.microphone = false;
                mic_denied = true;
            }
        }
        // 先摆好红框和浮条：开流时 Helmp 有窗口在屏幕上，才排除得掉。
        let (stop_tx, stop_rx) = async_channel::unbounded::<StopBy>();
        let windows = cx.update(|cx| open_windows(&target, audio, stop_tx.clone(), cx));
        let (frame, pill) = match windows {
            Ok(windows) => windows,
            Err(err) => {
                cx.update(|cx| {
                    state(cx).starting = false;
                    start_failed(err, None, cx);
                });
                return;
            }
        };
        let (events_tx, events_rx) = async_channel::unbounded::<sck::Event>();
        let backend = if fake {
            Ok(Backend::Fake)
        } else {
            let request = sck::Request {
                display: target.display,
                rect: target.rect,
                scale: target.scale,
                system_audio: audio.system,
                microphone: audio.microphone,
                path: &path,
            };
            sck::start(request, events_tx.clone())
                .await
                .map(|recorder| Backend::Real(Rc::new(recorder)))
        };
        cx.update(|cx| {
            state(cx).starting = false;
            let backend = match backend {
                Ok(backend) => backend,
                Err(err) => {
                    remove(frame, cx);
                    remove(pill.0, cx);
                    start_failed(err, Some(&path), cx);
                    return;
                }
            };
            let offscreen = offscreen();
            let active = Active {
                id,
                target,
                entry,
                path,
                started: Instant::now(),
                backend,
                frame,
                pill,
                // 自检在屏幕外跑：不往真实菜单栏上加东西，也不监听真实键盘。
                status: (!offscreen).then(|| native::StatusItem::new(stop_tx.clone())).flatten(),
                _esc: (!offscreen).then(|| native::EscMonitor::new(stop_tx.clone())),
                stopping: None,
                events: events_rx.clone(),
            };
            log(
                cx,
                format!(
                    "record start {} system={} mic={}",
                    entry.name(),
                    audio.system,
                    audio.microphone
                ),
            );
            crate::telemetry::recording_start(entry.name(), audio.system, audio.microphone);
            state(cx).active = Some(active);
            if mic_denied {
                mic_denied_toast(&target, cx);
            }
            watch(id, stop_rx, events_rx, cx);
        });
        drop(events_tx);
    })
    .detach();
}

/// 开录前出错：说出来（面板里）并上报；已经建出的空文件删掉。
fn start_failed(err: anyhow::Error, path: Option<&Path>, cx: &mut App) {
    if let Some(path) = path {
        let _ = std::fs::remove_file(path);
    }
    super::fail_at(
        "recording.start",
        err.context(tr("开始录屏失败", "Couldn't start recording")),
        cx,
    );
}

/// 录制中的三件事：每秒刷新计时、收「停止」、收系统事件（写完 / 出错）。
fn watch(
    id: u64,
    stop_rx: async_channel::Receiver<StopBy>,
    events: async_channel::Receiver<sck::Event>,
    cx: &mut App,
) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(by) = stop_rx.recv().await {
            if !cx.update(|cx| is_current(id, cx)) {
                break;
            }
            cx.update(|cx| stop(by, cx));
        }
    })
    .detach();
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(event) = events.recv().await {
            if !cx.update(|cx| is_current(id, cx)) {
                break;
            }
            cx.update(|cx| on_event(event, cx));
        }
    })
    .detach();
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            let alive = cx.update(|cx| {
                let Some(active) = state(cx).active.as_ref().filter(|a| a.id == id) else {
                    return false;
                };
                let seconds = active.started.elapsed().as_secs();
                if let Some(status) = &active.status {
                    status.set_elapsed(seconds);
                }
                let pill = active.pill.1.clone();
                pill.update(cx, |pill, cx| {
                    if pill.seconds != seconds {
                        pill.seconds = seconds;
                        cx.notify();
                    }
                });
                true
            });
            if !alive {
                break;
            }
        }
    })
    .detach();
}

fn is_current(id: u64, cx: &mut App) -> bool {
    state(cx).active.as_ref().is_some_and(|active| active.id == id)
}

/// 停止录制（停止按钮、快捷键、Esc、菜单栏）。mp4 写完后由 [`on_event`] 收尾。
pub fn stop(by: StopBy, cx: &mut App) {
    let Some(active) = state(cx).active.as_mut() else {
        return;
    };
    if active.stopping.is_some() {
        return;
    }
    active.stopping = Some(by);
    let id = active.id;
    log(cx, format!("record stop {}", by.name()));
    let Some(active) = state(cx).active.as_ref() else {
        return;
    };
    let pill = active.pill.1.clone();
    let path = active.path.clone();
    let recorder = match &active.backend {
        Backend::Real(recorder) => Some(recorder.clone()),
        Backend::Fake => None,
    };
    pill.update(cx, |pill, cx| {
        pill.stopping = true;
        cx.notify();
    });
    match recorder {
        None => {
            // 夹具：写一个占位文件，当作系统写完了。
            let written = std::fs::write(&path, b"helmp fake recording")
                .with_context(|| trf!("写入 {}", "Couldn't write {}", path.display()));
            cx.defer(move |cx| {
                if is_current(id, cx) {
                    finish(written, cx);
                }
            });
        }
        Some(recorder) => {
            cx.spawn(async move |cx: &mut AsyncApp| {
                if let Err(err) = recorder.stop().await {
                    // 流已经被系统停掉（随后会收到写完 / 出错）时也会到这里：只记下来。
                    crate::trace::log(format!("record: stopCapture: {err:#}"));
                }
                // 兜底：系统一直没报写完，就按文件在不在收尾。
                cx.background_executor().timer(FINISH_TIMEOUT).await;
                cx.update(|cx| {
                    if is_current(id, cx) {
                        let path = state(cx).active.as_ref().map(|a| a.path.clone());
                        let err = anyhow::anyhow!(
                            "停止录屏后 {} 秒仍没有写完",
                            FINISH_TIMEOUT.as_secs()
                        );
                        let written = match path {
                            Some(path) if non_empty(&path) => {
                                // 文件有内容，照常存下；但系统没报写完，记下来查。
                                crate::telemetry::report("recording.write", &err);
                                Ok(())
                            }
                            _ => Err(err),
                        };
                        finish(written, cx);
                    }
                });
            })
            .detach();
        }
    }
}

fn non_empty(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.len() > 0)
}

fn on_event(event: sck::Event, cx: &mut App) {
    match event {
        sck::Event::Finished => finish(Ok(()), cx),
        // 由 finish 上报（recording.write）。
        sck::Event::WriteFailed(message) => finish(Err(anyhow::anyhow!(message)), cx),
        sck::Event::StreamStopped(message) => {
            let err = anyhow::anyhow!(message);
            eprintln!("helmp: 录屏的流被系统停止：{err:#}");
            crate::telemetry::report("recording.stream", &err);
            // 当成一次停止：已经录下的部分照常写完、存下来。
            stop(StopBy::Error, cx);
        }
    }
}

/// 收尾：撤下红框、浮条、菜单栏计时，记埋点；存好了弹提示，没存好大声说出来。
fn finish(written: Result<()>, cx: &mut App) {
    let Some(active) = state(cx).active.take() else {
        return;
    };
    remove(active.frame, cx);
    remove(active.pill.0, cx);
    let seconds = active.started.elapsed().as_secs();
    let by = active.stopping.unwrap_or(StopBy::Error);
    crate::telemetry::recording_stop(by.name(), seconds);
    let path = active.path.clone();
    let placement = active.target.placement();
    drop(active);
    let written = written.and_then(|()| {
        anyhow::ensure!(
            non_empty(&path),
            trf!(
                "录屏文件没有写出来：{}",
                "The recording wasn't written: {}",
                path.display()
            )
        );
        Ok(())
    });
    if let Err(err) = written {
        let _ = std::fs::remove_file(&path);
        log(cx, format!("record error {err:#}"));
        super::fail_at(
            "recording.write",
            err.context(tr("录屏没有存下来", "The recording wasn't saved")),
            cx,
        );
        return;
    }
    log(cx, format!("record saved {}", path.display()));
    crate::telemetry::recording_save(seconds);
    saved_toast(&path, placement, cx);
}

fn saved_toast(path: &Path, at: Placement, cx: &mut App) {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let copy_path = path.to_path_buf();
    let reveal_path = path.to_path_buf();
    toast::show_actions(
        &trf!("已保存 {name}", "Saved {name}"),
        "check",
        at,
        vec![
            ToastAction {
                id: "recording-copy",
                label: tr("复制文件", "Copy File"),
                run: Rc::new(move |cx| copy_file(&copy_path, cx)),
            },
            ToastAction {
                id: "recording-reveal",
                label: tr("在访达中显示", "Show in Finder"),
                run: Rc::new(move |cx| reveal(&reveal_path, cx)),
            },
        ],
        cx,
    );
}

/// 把 mp4 作为文件放进剪贴板（在访达、聊天软件里能直接粘贴）。
fn copy_file(path: &Path, cx: &mut App) {
    if crate::effects::dry_run() {
        log(cx, format!("[dry-run] copy file {}", path.display()));
        return;
    }
    match screenshot::pasteboard::write_file_url(&screenshot::pasteboard::general(), path) {
        Ok(()) => log(cx, format!("copy file {}", path.display())),
        Err(err) => super::fail_at("recording.copy", err, cx),
    }
}

fn reveal(path: &Path, cx: &mut App) {
    if crate::effects::dry_run() {
        log(cx, format!("[dry-run] reveal {}", path.display()));
        return;
    }
    let status = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .status();
    match status {
        Ok(status) if status.success() => log(cx, format!("reveal {}", path.display())),
        Ok(status) => super::fail_at(
            "recording.reveal",
            anyhow::anyhow!(trf!(
                "在访达中显示失败：{status}",
                "Couldn't show it in Finder: {status}"
            )),
            cx,
        ),
        Err(err) => super::fail_at("recording.reveal", anyhow::Error::new(err), cx),
    }
}

fn mic_denied_toast(target: &Target, cx: &mut App) {
    toast::show_actions(
        tr(
            "没有麦克风权限，这次只录画面和系统声音",
            "No microphone access. Recording without the microphone",
        ),
        "mic-off",
        target.placement(),
        vec![ToastAction {
            id: "recording-mic-settings",
            label: tr("前往授权", "Open Settings"),
            run: Rc::new(|_| {
                if let Err(err) = crate::provider::Action::open(mic::SETTINGS_URL).run() {
                    eprintln!("helmp: 打开麦克风设置失败：{err:#}");
                }
            }),
        }],
        cx,
    );
}

/// 退出 Helmp 时还在录：停下并等 mp4 写完（最多几秒），不留一个打不开的半截文件。
pub fn stop_for_quit(cx: &mut App) {
    let Some(active) = state(cx).active.as_mut() else {
        return;
    };
    active.stopping.get_or_insert(StopBy::Quit);
    let Backend::Real(recorder) = &active.backend else {
        return;
    };
    let recorder = recorder.clone();
    let events = active.events.clone();
    crate::telemetry::recording_stop(
        StopBy::Quit.name(),
        active.started.elapsed().as_secs(),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    // 系统的回调都在它自己的队列上，主线程这里同步等不会死锁。
    let _ = futures_lite::future::block_on(recorder.stop());
    while Instant::now() < deadline {
        match events.try_recv() {
            Ok(sck::Event::Finished | sck::Event::WriteFailed(_)) => break,
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn remove(handle: AnyWindowHandle, cx: &mut App) {
    let _ = handle.update(cx, |_, window, _| window.remove_window());
}

// ======================= 窗口 =======================

/// 选区外的红框。
struct Frame;

impl Render for Frame {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .border(px(FRAME_WIDTH))
            .border_color(rgb(RED))
    }
}

/// 选区下方的浮条：● 计时 | 声音状态 | 停止。
pub(super) struct Pill {
    seconds: u64,
    audio: Audio,
    stopping: bool,
    stop: async_channel::Sender<StopBy>,
    focus: FocusHandle,
}

impl Pill {
    pub(super) fn summary(&self) -> String {
        format!(
            "{} system={} mic={} stopping={}",
            clock(self.seconds),
            self.audio.system,
            self.audio.microphone,
            self.stopping
        )
    }
}

impl Render for Pill {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = |name: &str, on: bool, tip: &'static str| {
            div()
                .id(SharedString::from(format!("recording-{name}")))
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(12.))
                .when(on, |d| d.bg(white().opacity(0.14)))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                })
                .child(
                    gpui_kit::component::Icon::empty()
                        .path(SharedString::from(format!("icons/{name}.svg")))
                        .text_color(if on { white() } else { white().opacity(0.45) })
                        .size(px(14.)),
                )
        };
        let (system, system_tip) = if self.audio.system {
            ("volume-2", tr("正在录系统声音", "Recording system audio"))
        } else {
            ("volume-x", tr("不录系统声音", "Not recording system audio"))
        };
        let (mic, mic_tip) = if self.audio.microphone {
            ("mic", tr("正在录麦克风", "Recording the microphone"))
        } else {
            ("mic-off", tr("不录麦克风", "Not recording the microphone"))
        };
        let stop = self.stop.clone();
        div()
            .id("recording-pill")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, _| {
                if event.keystroke.key == "escape" {
                    let _ = this.stop.try_send(StopBy::Esc);
                }
            }))
            .size_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(12.))
            .pr(px(5.))
            .rounded(px(18.))
            .bg(rgba(0x1c1c1eeb))
            .border_1()
            .border_color(white().opacity(0.12))
            .text_color(white())
            .text_size(px(13.))
            .child(div().size(px(8.)).rounded_full().bg(rgb(RED)))
            .child(
                div()
                    .font_family("Menlo")
                    .child(clock(self.seconds)),
            )
            .child(div().w(px(1.)).h(px(16.)).bg(white().opacity(0.2)))
            .child(status(system, self.audio.system, system_tip))
            .child(status(mic, self.audio.microphone, mic_tip))
            .child(div().flex_1())
            .child(
                div()
                    .id("recording-stop")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(26.))
                    .px(px(10.))
                    .rounded(px(13.))
                    .bg(rgb(RED))
                    .when(self.stopping, |d| d.opacity(0.6))
                    .when(!self.stopping, |d| {
                        d.cursor(CursorStyle::PointingHand)
                            .hover(|style| style.opacity(0.88))
                    })
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation();
                        let _ = stop.try_send(StopBy::Button);
                    })
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tr(
                            "停止录制（Esc）",
                            "Stop recording (Esc)",
                        ))
                        .build(window, cx)
                    })
                    .child(div().size(px(10.)).rounded(px(2.)).bg(white()))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if self.stopping {
                                tr("正在保存", "Saving")
                            } else {
                                tr("停止", "Stop")
                            }),
                    ),
            )
    }
}

fn window_options(frame: Rect) -> WindowOptions {
    WindowOptions {
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
        ..Default::default()
    }
}

fn open_windows(
    target: &Target,
    audio: Audio,
    stop: async_channel::Sender<StopBy>,
    cx: &mut App,
) -> Result<(AnyWindowHandle, (AnyWindowHandle, Entity<Pill>))> {
    let offscreen = offscreen();
    let frame_at = geometry::to_appkit(target.screen, frame_rect(target.rect));
    let frame = crate::settings::open_transparent_window(window_options(frame_at), cx, |_, cx| {
        cx.new(|_| Frame)
    })?;
    let _ = frame.update(cx, |_, window, _| {
        super::native::setup(window, super::native::Kind::Frame, frame_at, offscreen);
        super::native::present(window, false, offscreen);
    });
    let pill_at = geometry::to_appkit(target.screen, pill_rect(target.rect, target.visible));
    let mut view = None;
    let pill = crate::settings::open_transparent_window(window_options(pill_at), cx, |_, cx| {
        let entity = cx.new(|cx| Pill {
            seconds: 0,
            audio,
            stopping: false,
            stop,
            focus: cx.focus_handle(),
        });
        view = Some(entity.clone());
        entity
    });
    let pill = match pill {
        Ok(pill) => pill,
        Err(err) => {
            remove(frame, cx);
            return Err(err);
        }
    };
    let _ = pill.update(cx, |_, window, _| {
        super::native::setup(window, super::native::Kind::Pin, pill_at, offscreen);
        super::native::present(window, false, offscreen);
    });
    let view = view.context("录制浮条没有建出来")?;
    Ok((frame, (pill, view)))
}

/// 图形自检：录制中按键和鼠标送给浮条。
pub fn self_test_target(cx: &mut App) -> Option<AnyWindowHandle> {
    let active = state(cx).active.as_ref()?;
    let (handle, view) = active.pill.clone();
    // 自检送的按键要落在浮条上（真实使用时浮条不抢焦点，Esc 靠全局监听）。
    let focus = view.read(cx).focus.clone();
    let _ = handle.update(cx, |_, window, cx| focus.focus(window, cx));
    Some(handle)
}

/// 图形自检：录制状态，一行。
pub fn debug_state(cx: &mut App) -> String {
    let recording = state(cx);
    if recording.starting {
        return "starting".into();
    }
    let Some(active) = recording.active.as_ref() else {
        return "none".into();
    };
    let pill = active.pill.1.clone();
    let rect = active.target.rect;
    let head = format!(
        "{}({}) rect=({},{},{},{})",
        active.entry.name(),
        active.id,
        rect.x,
        rect.y,
        rect.w,
        rect.h
    );
    format!("{head} {}", pill.read(cx).summary())
}

#[cfg(test)]
mod tests {
    // gpui_kit::* 里也有一个 `test` 属性宏，经 `super::*` 引入后会遮住标准库的。
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn clock_reads_like_a_stopwatch() {
        assert_eq!(clock(0), "00:00");
        assert_eq!(clock(65), "01:05");
        assert_eq!(clock(3599), "59:59");
        assert_eq!(clock(3723), "1:02:03");
    }

    #[test]
    fn frame_sits_outside_the_recorded_area() {
        // 红框不能压进选区：录出来的画面里不该有一丝红边。
        let sel = Rect::new(100.0, 80.0, 400.0, 300.0);
        let frame = frame_rect(sel);
        let inner = Rect::new(
            frame.x + f64::from(FRAME_WIDTH),
            frame.y + f64::from(FRAME_WIDTH),
            frame.w - 2.0 * f64::from(FRAME_WIDTH),
            frame.h - 2.0 * f64::from(FRAME_WIDTH),
        );
        assert!(inner.x < sel.x && inner.y < sel.y);
        assert!(inner.right() > sel.right() && inner.bottom() > sel.bottom());
    }

    #[test]
    fn pill_goes_below_then_above_then_inside() {
        let visible = Rect::new(0.0, 25.0, 1512.0, 900.0);
        // 平常：选区下方居中，不压选区。
        let sel = Rect::new(300.0, 150.0, 700.0, 450.0);
        let pill = pill_rect(sel, visible);
        assert_eq!(pill.x + pill.w / 2.0, sel.x + sel.w / 2.0);
        assert!(pill.y > sel.bottom());
        // 选区贴着屏幕底部：放到上面。
        let low = Rect::new(300.0, 600.0, 700.0, 320.0);
        let pill = pill_rect(low, visible);
        assert!(pill.bottom() < low.y);
        // 选区几乎整屏：放在选区里靠下（Helmp 的窗口不会被录进去），且留在可见区域。
        let full = Rect::new(0.0, 25.0, 1512.0, 900.0);
        let pill = pill_rect(full, visible);
        assert!(pill.y >= visible.y && pill.bottom() <= visible.bottom());
        assert!(pill.bottom() <= full.bottom());
        // 选区在屏幕最左边：浮条不出屏。
        let left = Rect::new(0.0, 100.0, 60.0, 60.0);
        assert_eq!(pill_rect(left, visible).x, 0.0);
    }

    #[test]
    fn recordings_are_mp4_named_like_screenshots() {
        // 存进截图目录、和截图一样带时间，扩展名必须是 .mp4（访达、播放器按它认）。
        let name = file_name();
        assert!(name.ends_with(".mp4"), "{name}");
        assert!(name.starts_with("Helmp "), "{name}");
    }
}
