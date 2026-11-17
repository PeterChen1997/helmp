//! 便签窗口、边缘书签队列调度、自定义指针拖拽吸边与原地 IME 编辑。

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui_kit::component::input::{InputEvent, Position, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::native::{
    self, CARD_CORNER_RADIUS, NoteFrameHandle, TAB_CORNER_RADIUS, left_mouse_pressed,
    mouse_location_tl, offscreen,
};
use super::screen::{
    DisplayInfo, all_screens, card_size, clamp_floating, default_floating_frame,
    expanded_frame_for_docked, resolve_display, screen_for_frame,
};
use super::store::{self, Edge, Frame, Note, Placement, TAB_W};
use crate::i18n::tr;
use crate::style::{self, Palette};
use crate::telemetry::{self, NoteEvent};

/// 拖拽靠近屏幕边缘多少 pt 时吸附。
pub const SNAP_THRESHOLD: f64 = 16.0;
/// 鼠标移到耳标上多久后展开。
const HOVER_OPEN_DELAY: Duration = Duration::from_millis(150);
/// 移出展开卡片多久后校验鼠标位置并收起。
const HOVER_CLOSE_DELAY: Duration = Duration::from_millis(90);
/// 编辑停顿多久自动落盘。
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
/// 超过多少 pt 才算拖拽，避免轻点误拖。
const DRAG_THRESHOLD: f64 = 4.0;
const DRAG_TICK: Duration = Duration::from_millis(8);

/// 供 `Provider` 线程只读查询的内存快照（主线程更新完便签后同步刷新）。
static SNAPSHOT: OnceLock<RwLock<Vec<Note>>> = OnceLock::new();
/// 菜单栏轮询用：全部便签是否处于隐藏态。
static ALL_HIDDEN: AtomicBool = AtomicBool::new(false);
/// 菜单栏轮询用：便签隐藏状态或桌面数量变化时加一。
static MENU_GENERATION: AtomicU64 = AtomicU64::new(0);
/// 快照是否已初始化（用于无头 CLI 懒加载）。
static INITIALIZED: AtomicBool = AtomicBool::new(false);
/// 防止异步布局应用任务乱序覆盖。
static LAYOUT_SEQ: AtomicU64 = AtomicU64::new(0);
/// `HELMP_SELF_TEST` 选中的目标便签 id。
static SELF_TEST_NOTE_ID: OnceLock<RwLock<Option<String>>> = OnceLock::new();

fn self_test_note_lock() -> &'static RwLock<Option<String>> {
    SELF_TEST_NOTE_ID.get_or_init(|| RwLock::new(None))
}

fn ordered_visible_notes(mgr: &NotesManager) -> Vec<&Note> {
    let mut visible: Vec<&Note> = mgr
        .notes
        .iter()
        .filter(|n| !matches!(n.placement, Placement::Archived))
        .collect();
    visible.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
    visible
}

pub fn self_test_select(index: usize, cx: &mut App) -> bool {
    ensure_manager(cx);
    let mgr = cx.global::<NotesManager>();
    let selected = ordered_visible_notes(mgr).get(index).map(|n| n.id.clone());
    *self_test_note_lock().write().expect("self test note lock") = selected.clone();
    selected.is_some()
}

pub fn self_test_clear() {
    *self_test_note_lock().write().expect("self test note lock") = None;
}

pub fn self_test_target(cx: &mut App) -> Option<AnyWindowHandle> {
    if !cx.has_global::<NotesManager>() {
        return None;
    }
    let mgr = cx.global::<NotesManager>();
    if let Some(id) = self_test_note_lock()
        .read()
        .expect("self test note lock")
        .as_deref()
        && let Some(entry) = mgr.windows.get(id)
    {
        return Some(entry.handle);
    }
    mgr.editing_id
        .as_deref()
        .and_then(|id| mgr.windows.get(id))
        .map(|e| e.handle)
}

pub fn self_test_debug_state(cx: &mut App) -> String {
    if !cx.has_global::<NotesManager>() {
        return "uninitialized".into();
    }
    let (total, windows, hidden_all, editing_id, hovered_dock_id, rows) = {
        let mgr = cx.global::<NotesManager>();
        let rows: Vec<_> = ordered_visible_notes(mgr)
            .into_iter()
            .enumerate()
            .map(|(idx, note)| {
                let placement = match &note.placement {
                    Placement::Docked { edge, order, .. } => format!("docked({edge:?}#{order})"),
                    Placement::Floating { frame, .. } => {
                        format!("float({:.0},{:.0},{:.0}x{:.0})", frame.x, frame.y, frame.w, frame.h)
                    }
                    Placement::Archived => "archived".into(),
                };
                let win_info = mgr.windows.get(&note.id).map(|entry| {
                    let frame = entry
                        .native
                        .as_ref()
                        .and_then(|h| h.frame_tl())
                        .map(|f| format!("{:.0},{:.0},{:.0}x{:.0}", f.x, f.y, f.w, f.h))
                        .unwrap_or_else(|| "none".into());
                    (entry.handle, entry.view.clone(), frame)
                });
                (idx, note.id.clone(), note.text.clone(), placement, win_info)
            })
            .collect();
        (
            mgr.notes.len(),
            mgr.windows.len(),
            mgr.hidden_all,
            mgr.editing_id.clone(),
            mgr.hovered_dock_id.clone(),
            rows,
        )
    };
    let mut parts = Vec::new();
    for (idx, id, text, placement, win_info) in rows {
        let (frame_desc, state_desc) = if let Some((handle, view, frame)) = win_info {
            let (exp, ed, hov, key) = handle
                .update(cx, |_, window, cx| {
                    let w = view.read(cx);
                    let is_key = crate::panel::ns_window(window).is_some_and(|n| n.isKeyWindow());
                    (w.expanded, w.editing, w.hovered, is_key)
                })
                .unwrap_or((false, false, false, false));
            (frame, format!("exp={exp} ed={ed} hov={hov} key={key}"))
        } else {
            ("nowin".into(), "-".into())
        };
        parts.push(format!(
            "[{idx} id={} {placement} frame={frame_desc} {state_desc} text={:?}]",
            &id[..8.min(id.len())],
            text
        ));
    }
    format!(
        "total={} windows={} hidden={} editing={:?} hovered={:?} items=[{}]",
        total,
        windows,
        hidden_all,
        editing_id.as_deref().map(|s| &s[..8.min(s.len())]),
        hovered_dock_id.as_deref().map(|s| &s[..8.min(s.len())]),
        parts.join(", ")
    )
}

fn snapshot_lock() -> &'static RwLock<Vec<Note>> {
    SNAPSHOT.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn snapshot() -> Vec<Note> {
    let lock = snapshot_lock();
    let guard = lock.read().expect("notes snapshot poisoned");
    if guard.is_empty() && !INITIALIZED.load(Ordering::Relaxed) {
        drop(guard);
        let loaded = store::load();
        let mut w = lock.write().expect("notes snapshot poisoned");
        if !INITIALIZED.swap(true, Ordering::Relaxed) {
            *w = loaded;
        }
        return w.clone();
    }
    guard.clone()
}

fn set_snapshot(notes: &[Note]) {
    INITIALIZED.store(true, Ordering::Relaxed);
    *snapshot_lock().write().expect("notes snapshot poisoned") = notes.to_vec();
    MENU_GENERATION.fetch_add(1, Ordering::Relaxed);
}

pub fn all_hidden() -> bool {
    ALL_HIDDEN.load(Ordering::Relaxed)
}

pub fn menu_generation() -> u64 {
    MENU_GENERATION.load(Ordering::Relaxed)
}

/// 由 `Outcome::Notes` 分发到界面主线程执行的请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotesRequest {
    /// 新建便签：`docked = true` 时直接吸到主显示器右侧队尾；`enter_edit = true` 时展开并进入编辑。
    Create {
        text: String,
        docked: bool,
        enter_edit: bool,
    },
    /// 从 Scope 把归档或桌面上的便签钉回桌面（归档的吸到主屏右侧队尾；已在桌面则取消隐藏并展开）。
    PinToDesktop(String),
    /// 永久删除一条便签。
    Delete(String),
}

impl NotesRequest {
    pub fn describe(&self) -> String {
        match self {
            NotesRequest::Create { docked, .. } => format!("create(docked={docked})"),
            NotesRequest::PinToDesktop(id) => format!("pin({id})"),
            NotesRequest::Delete(id) => format!("delete({id})"),
        }
    }
}

struct WindowEntry {
    handle: AnyWindowHandle,
    view: Entity<NoteWindow>,
    native: Option<NoteFrameHandle>,
    visible: bool,
}

struct ScreenObserver {
    _gpui: Subscription,
    _native: native::NotificationObserver,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditEndMode {
    /// 便签之间转移编辑：退出旧便签编辑态，保留外层会话捕获的归还 PID 与 hand-over。
    TransferToAnotherNote,
    /// 编辑会话结束但不主动归还焦点（失焦到其他 App / 窗口、拖拽、菜单栏收边/隐藏、退出应用）。
    EndSessionNoReturn,
    /// 编辑会话结束并把焦点归还给编辑前记录的 App（Esc、归档或删除正在编辑的便签）。
    EndSessionReturnFocus,
}

#[derive(Debug, Clone)]
struct HoverCandidate {
    id: String,
    frame: Frame,
    visible: bool,
}

/// 全局便签管理器（主线程 `Global`）：持有真源 `notes`、各窗口句柄、悬停与编辑协调态。
pub struct NotesManager {
    notes: Vec<Note>,
    windows: HashMap<String, WindowEntry>,
    hidden_all: bool,
    hovered_dock_id: Option<String>,
    editing_id: Option<String>,
    editing_return_pid: i32,
    dirty_save: bool,
    hover_poll_running: bool,
    hover_candidate: Option<(String, Instant)>,
    unhover_started: Option<(String, Instant)>,
    screen_observer: Option<ScreenObserver>,
}

impl Global for NotesManager {}

impl NotesManager {
    fn new(notes: Vec<Note>) -> Self {
        set_snapshot(&notes);
        Self::from_notes(notes)
    }

    fn from_notes(notes: Vec<Note>) -> Self {
        Self {
            notes,
            windows: HashMap::new(),
            hidden_all: false,
            hovered_dock_id: None,
            editing_id: None,
            editing_return_pid: 0,
            dirty_save: false,
            hover_poll_running: false,
            hover_candidate: None,
            unhover_started: None,
            screen_observer: None,
        }
    }

    fn start_edit_session(&mut self, id: &str, capture_pid: impl FnOnce() -> i32) -> Option<i32> {
        self.editing_id = Some(id.to_string());
        if self.editing_return_pid == 0 {
            let pid = capture_pid();
            self.editing_return_pid = pid;
            if pid != 0 {
                return Some(pid);
            }
        }
        None
    }

    fn end_edit_session(&mut self, id: &str, mode: EditEndMode) -> (i32, bool) {
        if self.editing_id.as_deref() == Some(id) {
            self.editing_id = None;
        }
        match mode {
            EditEndMode::TransferToAnotherNote => (0, false),
            EditEndMode::EndSessionNoReturn => {
                self.editing_return_pid = 0;
                (0, true)
            }
            EditEndMode::EndSessionReturnFocus => {
                let pid = std::mem::take(&mut self.editing_return_pid);
                (pid, true)
            }
        }
    }

    fn apply_text_edit(&mut self, id: &str, text: String, update_global_snapshot: bool) -> bool {
        let Some(note) = self.notes.iter_mut().find(|n| n.id == id) else {
            return false;
        };
        if note.text == text {
            return false;
        }
        note.text = text;
        note.updated_at = now_unix_secs();
        self.dirty_save = true;
        if update_global_snapshot {
            set_snapshot(&self.notes);
        }
        true
    }

    fn flush_dirty_with(
        &mut self,
        pending_editor: Option<(&str, String)>,
        update_global_snapshot: bool,
        save_fn: impl FnOnce(&[Note]),
    ) {
        if let Some((id, text)) = pending_editor {
            self.apply_text_edit(id, text, update_global_snapshot);
        }
        if self.dirty_save {
            self.dirty_save = false;
            save_fn(&self.notes);
        }
    }

    fn step_dock_hover(
        &mut self,
        mx: f64,
        my: f64,
        now: Instant,
        candidates: &[HoverCandidate],
    ) -> (Option<String>, Option<String>) {
        let mut clear_open = None;
        if let Some(open_id) = self.hovered_dock_id.clone() {
            let inside_open = candidates
                .iter()
                .find(|c| c.visible && c.id == open_id)
                .is_some_and(|c| point_in_frame(mx, my, c.frame, 4.0));
            if inside_open {
                self.unhover_started = None;
                self.hover_candidate = None;
                return (None, None);
            }
            let close_now = match &self.unhover_started {
                Some((id, since)) if id == &open_id => {
                    now.saturating_duration_since(*since) >= HOVER_CLOSE_DELAY
                }
                _ => {
                    self.unhover_started = Some((open_id.clone(), now));
                    false
                }
            };
            if close_now {
                self.unhover_started = None;
                clear_open = Some(open_id);
            }
        } else {
            self.unhover_started = None;
        }

        let hit_id = candidates
            .iter()
            .find(|c| {
                c.visible
                    && self.hovered_dock_id.as_deref() != Some(c.id.as_str())
                    && point_in_frame(mx, my, c.frame, 2.0)
            })
            .map(|c| c.id.clone());

        let Some(hit_id) = hit_id else {
            self.hover_candidate = None;
            return (clear_open, None);
        };

        let open_now = match &self.hover_candidate {
            Some((id, since)) if id == &hit_id => {
                now.saturating_duration_since(*since) >= HOVER_OPEN_DELAY
            }
            _ => {
                self.hover_candidate = Some((hit_id.clone(), now));
                false
            }
        };
        if open_now {
            self.hover_candidate = None;
            self.unhover_started = None;
            return (clear_open, Some(hit_id));
        }
        (clear_open, None)
    }
}

pub fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// 启动时调用一次：载入 `notes.json`，在各自原显示器（缺屏回退主屏同侧）恢复所有非归档便签。
pub fn restore_on_launch(cx: &mut App) {
    let notes = store::load();
    cx.set_global(NotesManager::new(notes));
    ensure_screen_observer(cx);
    sync_all_windows(cx);
}

fn ensure_manager(cx: &mut App) {
    if !cx.has_global::<NotesManager>() {
        let notes = store::load();
        cx.set_global(NotesManager::new(notes));
    }
    ensure_screen_observer(cx);
}

fn ensure_screen_observer(cx: &mut App) {
    if !cx.has_global::<NotesManager>() || cx.global::<NotesManager>().screen_observer.is_some() {
        return;
    }
    let (tx, rx) = async_channel::bounded::<()>(1);
    let tx_gpui = tx.clone();
    let gpui_sub = cx.observe_displays(move |_, _| {
        let _ = tx_gpui.try_send(());
    });
    let native_sub = native::observe_screen_parameters(move || {
        let _ = tx.try_send(());
    });
    cx.global_mut::<NotesManager>().screen_observer = Some(ScreenObserver {
        _gpui: gpui_sub,
        _native: native_sub,
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        while rx.recv().await.is_ok() {
            while rx.try_recv().is_ok() {}
            cx.update(handle_screens_changed);
        }
    })
    .detach();
}

fn handle_screens_changed(cx: &mut App) {
    if !cx.has_global::<NotesManager>() {
        return;
    }
    let screens = current_screens();
    if screens.is_empty() {
        return;
    }
    sync_all_windows_for_screens(&screens, cx);
}

/// 处理来自启动器 / 全局快捷键的便签请求（主线程）。
pub fn handle(request: NotesRequest, cx: &mut App) {
    ensure_manager(cx);
    match request {
        NotesRequest::Create {
            text,
            docked,
            enter_edit,
        } => create_note(text, docked, enter_edit, cx),
        NotesRequest::PinToDesktop(id) => pin_to_desktop(&id, cx),
        NotesRequest::Delete(id) => delete_note(&id, cx),
    }
}

/// 菜单栏「全部收边」：退出编辑、取消隐藏，将全部桌面便签收到各自所在屏幕（优先右侧）队列。
pub fn dock_all(cx: &mut App) {
    ensure_manager(cx);
    exit_any_edit(EditEndMode::EndSessionNoReturn, cx);
    let screens = current_screens();
    let Some(primary) = screens.first().cloned() else {
        return;
    };
    let docked_count = {
        let mgr = cx.global_mut::<NotesManager>();
        mgr.hovered_dock_id = None;
        if mgr.hidden_all {
            mgr.hidden_all = false;
            ALL_HIDDEN.store(false, Ordering::Relaxed);
        }
        // 先记下自由浮窗要吸往的屏幕与目标，避免在迭代同时 mutable 借用两次。
        let updates: Vec<(usize, Edge, String)> = mgr
            .notes
            .iter()
            .enumerate()
            .filter_map(|(idx, note)| match note.placement {
                Placement::Floating { frame } => {
                    let screen = screen_for_frame(&screens, frame).unwrap_or(&primary);
                    let edge = store::snap_edge(frame, screen.visible, f64::INFINITY)
                        .unwrap_or(Edge::Right);
                    Some((idx, edge, screen.uuid.clone()))
                }
                _ => None,
            })
            .collect();
        let count = updates.len();
        for (idx, edge, display) in updates {
            let order = store::next_order(&mgr.notes, edge, &display);
            mgr.notes[idx].placement = Placement::Docked {
                edge,
                display,
                order,
            };
            mgr.notes[idx].updated_at = now_unix_secs();
        }
        if count > 0 {
            store::save(&mgr.notes);
            set_snapshot(&mgr.notes);
        } else {
            MENU_GENERATION.fetch_add(1, Ordering::Relaxed);
        }
        count
    };
    // 口径与逐张拖动吸边一致：每张由 Floating 变为 Docked 的便签记一次 `NoteEvent::Docked`（仅传枚举）。
    for _ in 0..docked_count {
        telemetry::note(NoteEvent::Docked);
    }
    sync_all_windows(cx);
}

/// 菜单栏「隐藏 / 显示全部便签」。
pub fn toggle_visibility(cx: &mut App) {
    ensure_manager(cx);
    let hide_now = !cx.global::<NotesManager>().hidden_all;
    if hide_now {
        exit_any_edit(EditEndMode::EndSessionNoReturn, cx);
    }
    {
        let mgr = cx.global_mut::<NotesManager>();
        mgr.hidden_all = hide_now;
        mgr.hovered_dock_id = None;
        ALL_HIDDEN.store(hide_now, Ordering::Relaxed);
        MENU_GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    sync_all_windows(cx);
}

fn current_screens() -> Vec<DisplayInfo> {
    objc2::MainThreadMarker::new()
        .map(all_screens)
        .unwrap_or_default()
}

fn create_note(text: String, docked: bool, enter_edit: bool, cx: &mut App) {
    let screens = current_screens();
    let Some(primary) = screens.first() else {
        eprintln!("helmp: 没有可用显示器，无法创建便签");
        return;
    };
    let now = now_unix_secs();
    let note = {
        let mgr = cx.global_mut::<NotesManager>();
        if mgr.hidden_all {
            mgr.hidden_all = false;
            ALL_HIDDEN.store(false, Ordering::Relaxed);
        }
        let placement = if docked {
            let order = store::next_order(&mgr.notes, Edge::Right, &primary.uuid);
            Placement::Docked {
                edge: Edge::Right,
                display: primary.uuid.clone(),
                order,
            }
        } else {
            Placement::Floating {
                frame: default_floating_frame(primary.visible),
            }
        };
        let note = store::new_note(&text, now, placement);
        let cloned = note.clone();
        mgr.notes.push(note);
        store::save(&mgr.notes);
        set_snapshot(&mgr.notes);
        telemetry::note(NoteEvent::Created);
        cloned
    };
    let id = note.id.clone();
    if enter_edit {
        cx.global_mut::<NotesManager>().hovered_dock_id =
            matches!(note.placement, Placement::Docked { .. }).then(|| id.clone());
    }
    sync_all_windows(cx);
    if enter_edit {
        begin_edit(&id, cx);
    }
}

fn pin_to_desktop(id: &str, cx: &mut App) {
    let screens = current_screens();
    let Some(primary) = screens.first() else {
        return;
    };
    let mut found = false;
    let mut was_archived = false;
    {
        let mgr = cx.global_mut::<NotesManager>();
        if mgr.hidden_all {
            mgr.hidden_all = false;
            ALL_HIDDEN.store(false, Ordering::Relaxed);
            MENU_GENERATION.fetch_add(1, Ordering::Relaxed);
        }
        let next = store::next_order(&mgr.notes, Edge::Right, &primary.uuid);
        if let Some(note) = mgr.notes.iter_mut().find(|n| n.id == id) {
            found = true;
            if matches!(note.placement, Placement::Archived) {
                was_archived = true;
                note.placement = Placement::Docked {
                    edge: Edge::Right,
                    display: primary.uuid.clone(),
                    order: next,
                };
                note.updated_at = now_unix_secs();
            }
        }
        if was_archived {
            store::save(&mgr.notes);
            set_snapshot(&mgr.notes);
        }
    }
    if !found {
        return;
    }
    cx.global_mut::<NotesManager>().hovered_dock_id = Some(id.to_string());
    sync_all_windows(cx);
    begin_edit(id, cx);
}

fn delete_note(id: &str, cx: &mut App) {
    let was_editing = cx
        .global::<NotesManager>()
        .editing_id
        .as_deref()
        == Some(id);
    if was_editing {
        exit_any_edit(EditEndMode::EndSessionReturnFocus, cx);
    }
    let removed_entry = {
        let mgr = cx.global_mut::<NotesManager>();
        let before = mgr.notes.len();
        mgr.notes.retain(|n| n.id != id);
        if mgr.hovered_dock_id.as_deref() == Some(id) {
            mgr.hovered_dock_id = None;
        }
        if mgr.notes.len() < before {
            store::save(&mgr.notes);
            set_snapshot(&mgr.notes);
            telemetry::note(NoteEvent::Deleted);
        }
        mgr.windows.remove(id)
    };
    if let Some(entry) = removed_entry {
        let _ = entry.handle.update(cx, |_, window, _| window.remove_window());
    }
    sync_all_windows(cx);
}

fn archive_note(id: &str, cx: &mut App) {
    let was_editing = cx
        .global::<NotesManager>()
        .editing_id
        .as_deref()
        == Some(id);
    if was_editing {
        exit_any_edit(EditEndMode::EndSessionReturnFocus, cx);
    }
    let removed_entry = {
        let mgr = cx.global_mut::<NotesManager>();
        if let Some(note) = mgr.notes.iter_mut().find(|n| n.id == id) {
            note.placement = Placement::Archived;
            note.updated_at = now_unix_secs();
            store::save(&mgr.notes);
            set_snapshot(&mgr.notes);
        }
        if mgr.hovered_dock_id.as_deref() == Some(id) {
            mgr.hovered_dock_id = None;
        }
        mgr.windows.remove(id)
    };
    if let Some(entry) = removed_entry {
        let _ = entry.handle.update(cx, |_, window, _| window.remove_window());
    }
    sync_all_windows(cx);
}

fn begin_edit(id: &str, cx: &mut App) {
    let prev_id = cx.global::<NotesManager>().editing_id.clone();
    if prev_id.as_deref() == Some(id) {
        if let Some(entry) = cx.global::<NotesManager>().windows.get(id) {
            let view = entry.view.clone();
            let handle = entry.handle;
            let _ = handle.update(cx, |_, window, cx| {
                view.update(cx, |w, cx| w.focus_editor(window, cx));
            });
        }
        return;
    }
    if let Some(prev) = prev_id {
        finish_edit_window(&prev, EditEndMode::TransferToAnotherNote, cx);
    }
    let hand_over_pid = {
        let mgr = cx.global_mut::<NotesManager>();
        mgr.start_edit_session(id, || {
            if offscreen() {
                0
            } else {
                crate::panel::app_to_return_to()
            }
        })
    };
    if let Some(pid) = hand_over_pid {
        crate::panel::hand_over_previous_app(pid);
    }
    sync_all_windows(cx);
    if let Some(entry) = cx.global::<NotesManager>().windows.get(id) {
        let view = entry.view.clone();
        let handle = entry.handle;
        let _ = handle.update(cx, |_, window, cx| {
            view.update(cx, |w, cx| w.enter_edit_mode(window, cx));
        });
    }
}

fn finish_edit_window(id: &str, mode: EditEndMode, cx: &mut App) {
    let entry = cx
        .global::<NotesManager>()
        .windows
        .get(id)
        .map(|e| (e.handle, e.view.clone()));
    if let Some((handle, view)) = entry {
        let _ = handle.update(cx, |_, window, cx| {
            view.update(cx, |w, cx| w.leave_edit_mode(window, cx));
        });
    }
    let (return_pid, clear_hand_over, ns_win) = {
        let mgr = cx.global_mut::<NotesManager>();
        let (return_pid, clear_hand_over) = mgr.end_edit_session(id, mode);
        let ns_win = mgr
            .windows
            .get(id)
            .and_then(|e| e.native.clone());
        (return_pid, clear_hand_over, ns_win)
    };
    if clear_hand_over {
        crate::panel::hand_over_previous_app(0);
    }
    if return_pid != 0 && !offscreen() {
        if let Some(handle) = ns_win {
            crate::panel::return_focus(return_pid, Some(handle.ns_window()));
        } else {
            crate::panel::return_focus(return_pid, None);
        }
    }
    flush_dirty_save(cx);
    sync_all_windows(cx);
}

fn exit_any_edit(mode: EditEndMode, cx: &mut App) {
    let current = cx.global::<NotesManager>().editing_id.clone();
    if let Some(id) = current {
        finish_edit_window(&id, mode, cx);
    }
}

/// 应用退出前同步结束正在编辑的便签并把 500ms debounce 内尚未写盘的修改落盘。
pub fn flush_on_quit(cx: &mut App) {
    if !cx.has_global::<NotesManager>() {
        return;
    }
    let current = cx.global::<NotesManager>().editing_id.clone();
    if let Some(id) = current {
        let entry = cx
            .global::<NotesManager>()
            .windows
            .get(&id)
            .map(|e| (e.handle, e.view.clone()));
        if let Some((handle, view)) = entry {
            let _ = handle.update(cx, |_, window, cx| {
                view.update(cx, |w, cx| w.leave_edit_mode_for_quit(window, cx));
            });
        }
        let (_, clear_hand_over) = cx
            .global_mut::<NotesManager>()
            .end_edit_session(&id, EditEndMode::EndSessionNoReturn);
        if clear_hand_over {
            crate::panel::hand_over_previous_app(0);
        }
    }
    flush_dirty_save(cx);
}

fn set_hovered_dock(id: Option<String>, cx: &mut App) {
    let current = cx.global::<NotesManager>().hovered_dock_id.clone();
    if current == id {
        return;
    }
    cx.global_mut::<NotesManager>().hovered_dock_id = id;
    sync_all_windows(cx);
}

fn clear_hovered_dock(id: &str, cx: &mut App) {
    if cx.global::<NotesManager>().hovered_dock_id.as_deref() == Some(id) {
        cx.global_mut::<NotesManager>().hovered_dock_id = None;
        sync_all_windows(cx);
    }
}

fn update_note_text(id: &str, text: String, cx: &mut App) {
    cx.global_mut::<NotesManager>()
        .apply_text_edit(id, text, true);
}

fn flush_dirty_save(cx: &mut App) {
    cx.global_mut::<NotesManager>()
        .flush_dirty_with(None, true, store::save);
}

/// 纯函数：算出所有非归档便签的目标展示形态。
#[derive(Debug, Clone, PartialEq)]
struct PlannedWindow {
    id: String,
    frame: Frame,
    radius: f64,
    visible: bool,
    order_front: bool,
    docked: bool,
    dock_edge: Option<Edge>,
    expanded: bool,
}

fn plan_windows(
    notes: &[Note],
    screens: &[DisplayInfo],
    hidden_all: bool,
    hovered_dock_id: Option<&str>,
    editing_id: Option<&str>,
) -> Vec<PlannedWindow> {
    if screens.is_empty() {
        return Vec::new();
    }
    let primary = &screens[0];
    // 先把已停靠便签按 (实际可用显示器 UUID, Edge) 分组，并按 order 升序排序，
    // 确保缺失显示器上的便签会被合并到主屏同侧一起参与 `dock_layout` 错峰排队。
    let mut groups: BTreeMap<(String, u8), Vec<&Note>> = BTreeMap::new();
    for note in notes {
        if let Placement::Docked { edge, display, .. } = &note.placement {
            let target = resolve_display(screens, display).unwrap_or(primary);
            let side = match edge {
                Edge::Left => 0,
                Edge::Right => 1,
            };
            groups.entry((target.uuid.clone(), side)).or_default().push(note);
        }
    }

    let mut docked_frames: HashMap<String, (Frame, Edge, Frame)> = HashMap::new();
    for ((display_uuid, side), mut list) in groups {
        list.sort_by_key(|note| match note.placement {
            Placement::Docked { order, .. } => order,
            _ => 0,
        });
        let edge = if side == 0 { Edge::Left } else { Edge::Right };
        let screen = resolve_display(screens, &display_uuid).unwrap_or(primary);
        let ids: Vec<&str> = list.iter().map(|n| n.id.as_str()).collect();
        for (id, tab_frame) in store::dock_layout(&ids, edge, screen.visible) {
            docked_frames.insert(id, (tab_frame, edge, screen.visible));
        }
    }

    let mut out = Vec::new();
    for note in notes {
        match &note.placement {
            Placement::Archived => continue,
            Placement::Floating { frame } => {
                let screen = screen_for_frame(screens, *frame).unwrap_or(primary);
                let clamped = clamp_floating(*frame, screen.visible);
                let editing = editing_id == Some(note.id.as_str());
                out.push(PlannedWindow {
                    id: note.id.clone(),
                    frame: clamped,
                    radius: CARD_CORNER_RADIUS,
                    visible: !hidden_all,
                    order_front: editing,
                    docked: false,
                    dock_edge: None,
                    expanded: true,
                });
            }
            Placement::Docked { edge, display, .. } => {
                let editing = editing_id == Some(note.id.as_str());
                let hovered = hovered_dock_id == Some(note.id.as_str());
                let expanded = editing || hovered;
                let screen = resolve_display(screens, display).unwrap_or(primary);
                if let Some((tab_frame, resolved_edge, visible_area)) = docked_frames.get(&note.id)
                {
                    let frame = if expanded {
                        expanded_frame_for_docked(*tab_frame, note.size, *resolved_edge, *visible_area)
                    } else {
                        *tab_frame
                    };
                    out.push(PlannedWindow {
                        id: note.id.clone(),
                        frame,
                        radius: if expanded {
                            CARD_CORNER_RADIUS
                        } else {
                            TAB_CORNER_RADIUS
                        },
                        visible: !hidden_all,
                        order_front: expanded,
                        docked: true,
                        dock_edge: Some(*resolved_edge),
                        expanded,
                    });
                } else if expanded {
                    // 即使超出队列上限被截断，从 Scope 钉回/编辑时仍要在屏幕边缘展开。
                    let fallback_tab = Frame {
                        x: match edge {
                            Edge::Left => screen.visible.x,
                            Edge::Right => screen.visible.x + screen.visible.w - TAB_W,
                        },
                        y: screen.visible.y + screen.visible.h / 2.0,
                        w: TAB_W,
                        h: store::TAB_H,
                    };
                    let frame =
                        expanded_frame_for_docked(fallback_tab, note.size, *edge, screen.visible);
                    out.push(PlannedWindow {
                        id: note.id.clone(),
                        frame,
                        radius: CARD_CORNER_RADIUS,
                        visible: !hidden_all,
                        order_front: true,
                        docked: true,
                        dock_edge: Some(*edge),
                        expanded: true,
                    });
                } else {
                    // 队列过长放不下时隐藏溢出耳标，避免越界或重叠。
                    out.push(PlannedWindow {
                        id: note.id.clone(),
                        frame: Frame {
                            x: screen.visible.x,
                            y: screen.visible.y,
                            w: TAB_W,
                            h: store::TAB_MIN_H,
                        },
                        radius: TAB_CORNER_RADIUS,
                        visible: false,
                        order_front: false,
                        docked: true,
                        dock_edge: Some(*edge),
                        expanded: false,
                    });
                }
            }
        }
    }
    out
}

/// 同步所有非归档便签窗口：打开缺失窗口、关掉多余窗口、更新视图展示态并异步套用几何位置。
fn sync_all_windows(cx: &mut App) {
    ensure_manager(cx);
    let screens = current_screens();
    if screens.is_empty() {
        return;
    }
    sync_all_windows_for_screens(&screens, cx);
}

fn sync_all_windows_for_screens(screens: &[DisplayInfo], cx: &mut App) {
    let (plans, snapshots_by_id, hidden_all) = {
        let mgr = cx.global::<NotesManager>();
        let plans = plan_windows(
            &mgr.notes,
            screens,
            mgr.hidden_all,
            mgr.hovered_dock_id.as_deref(),
            mgr.editing_id.as_deref(),
        );
        let map: HashMap<String, Note> = mgr
            .notes
            .iter()
            .map(|n| (n.id.clone(), n.clone()))
            .collect();
        (plans, map, mgr.hidden_all)
    };

    let wanted_ids: std::collections::HashSet<String> =
        plans.iter().map(|p| p.id.clone()).collect();
    let to_remove: Vec<WindowEntry> = {
        let mgr = cx.global_mut::<NotesManager>();
        let stale: Vec<String> = mgr
            .windows
            .keys()
            .filter(|id| !wanted_ids.contains(*id))
            .cloned()
            .collect();
        stale
            .into_iter()
            .filter_map(|id| mgr.windows.remove(&id))
            .collect()
    };
    for entry in to_remove {
        let _ = entry.handle.update(cx, |_, window, _| window.remove_window());
    }

    let mut native_ops: Vec<(NoteFrameHandle, Frame, f64, bool, bool)> = Vec::new();
    for plan in plans {
        let Some(note) = snapshots_by_id.get(&plan.id).cloned() else {
            continue;
        };
        let existing = {
            let mgr = cx.global_mut::<NotesManager>();
            mgr.windows.get_mut(&plan.id).map(|e| {
                e.visible = plan.visible;
                (e.handle, e.view.clone(), e.native.clone())
            })
        };
        if let Some((handle, view, native_opt)) = existing {
            let dragging = view.read(cx).drag.is_some();
            let _ = handle.update(cx, |_, _, cx| {
                view.update(cx, |w, cx| {
                    w.sync_state(
                        &note,
                        plan.docked,
                        plan.dock_edge,
                        plan.expanded,
                        plan.visible,
                        cx,
                    );
                });
            });
            if !dragging
                && let Some(native_handle) = native_opt
            {
                native_ops.push((
                    native_handle,
                    plan.frame,
                    plan.radius,
                    plan.visible,
                    plan.order_front,
                ));
            }
        } else if let Some(entry) = open_note_window(&note, &plan, hidden_all, cx) {
            cx.global_mut::<NotesManager>()
                .windows
                .insert(plan.id.clone(), entry);
        }
    }

    if !native_ops.is_empty() {
        let seq = LAYOUT_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        cx.spawn(async move |cx: &mut AsyncApp| {
            // 推迟 1ms 等外层 `cx` 借用彻底退出后再调用 `setFrame`，避免 GPUI 尺寸变更通知重入报错。
            cx.background_executor()
                .timer(Duration::from_millis(1))
                .await;
            if LAYOUT_SEQ.load(Ordering::Relaxed) != seq {
                return;
            }
            for (handle, frame, radius, visible, _) in &native_ops {
                if *visible {
                    handle.set_frame_tl(*frame, *radius);
                    handle.set_visible(true);
                } else {
                    handle.set_visible(false);
                }
            }
            // 先完成全部窗口布局后再置顶展开/编辑卡片，确保相邻耳标不会遮挡展开后的卡片一角。
            for (handle, _, _, visible, order_front) in native_ops {
                if visible && order_front {
                    handle.order_front();
                }
            }
        })
        .detach();
    }

    ensure_hover_poll(cx);
}

fn point_in_frame(mx: f64, my: f64, f: Frame, pad: f64) -> bool {
    mx >= f.x - pad && mx <= f.x + f.w + pad && my >= f.y - pad && my <= f.y + f.h + pad
}

/// 用系统全局指针坐标轮询停靠耳标的悬停与离开：后台 `NonactivatingPanel` 窗口
/// 不依赖是否激活或 `mouseEntered:` 事件即可稳定触发 150ms 悬停展开与移出收起。
fn ensure_hover_poll(cx: &mut App) {
    if offscreen() || !cx.has_global::<NotesManager>() {
        return;
    }
    let should_run = {
        let mgr = cx.global::<NotesManager>();
        !mgr.hidden_all
            && mgr.editing_id.is_none()
            && mgr
                .notes
                .iter()
                .any(|n| matches!(n.placement, Placement::Docked { .. }))
    };
    if !should_run {
        return;
    }
    {
        let mgr = cx.global_mut::<NotesManager>();
        if mgr.hover_poll_running {
            return;
        }
        mgr.hover_poll_running = true;
    }
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            let keep_going = cx.update(poll_hover_step);
            if !keep_going {
                break;
            }
        }
    })
    .detach();
}

fn poll_hover_step(cx: &mut App) -> bool {
    if offscreen() || !cx.has_global::<NotesManager>() {
        return false;
    }
    let (should_stop, any_dragging, candidates) = {
        let mgr = cx.global::<NotesManager>();
        let has_docked = mgr
            .notes
            .iter()
            .any(|n| matches!(n.placement, Placement::Docked { .. }));
        let stop = mgr.hidden_all || mgr.editing_id.is_some() || !has_docked;
        let dragging = mgr.windows.values().any(|e| e.view.read(cx).drag.is_some());
        let candidates: Vec<HoverCandidate> = mgr
            .notes
            .iter()
            .filter(|n| matches!(n.placement, Placement::Docked { .. }))
            .filter_map(|n| {
                let entry = mgr.windows.get(&n.id)?;
                let frame = entry.native.as_ref()?.frame_tl()?;
                Some(HoverCandidate {
                    id: n.id.clone(),
                    frame,
                    visible: entry.visible,
                })
            })
            .collect();
        (stop, dragging, candidates)
    };
    if should_stop {
        let mgr = cx.global_mut::<NotesManager>();
        mgr.hover_poll_running = false;
        mgr.hover_candidate = None;
        mgr.unhover_started = None;
        return false;
    }
    if any_dragging {
        return true;
    }
    let Some((mx, my)) = mouse_location_tl() else {
        return true;
    };

    let (clear_open, set_open) = cx.global_mut::<NotesManager>().step_dock_hover(
        mx,
        my,
        Instant::now(),
        &candidates,
    );
    if let Some(id) = clear_open {
        clear_hovered_dock(&id, cx);
    }
    if let Some(id) = set_open {
        set_hovered_dock(Some(id), cx);
    }
    true
}

fn open_note_window(
    note: &Note,
    plan: &PlannedWindow,
    hidden_all: bool,
    cx: &mut App,
) -> Option<WindowEntry> {
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
            size(px(plan.frame.w as f32), px(plan.frame.h as f32)),
        ))),
        ..Default::default()
    };
    let mut view_out = None;
    let note_clone = note.clone();
    let docked = plan.docked;
    let dock_edge = plan.dock_edge;
    let expanded = plan.expanded;
    let visible = !hidden_all && plan.visible;
    let opened = crate::settings::open_transparent_window(options, cx, |window, cx| {
        let entity = cx.new(|cx| {
            NoteWindow::new(note_clone, docked, dock_edge, expanded, visible, window, cx)
        });
        view_out = Some(entity.clone());
        entity
    });
    let (handle, view) = match (opened, view_out) {
        (Ok(handle), Some(view)) => (handle, view),
        (Err(err), _) => {
            eprintln!("helmp: 打开便签窗口失败：{err:#}");
            telemetry::report("notes.open_window", &err);
            return None;
        }
        (Ok(_), None) => return None,
    };
    let mut native_handle = None;
    let _ = handle.update(cx, |_, window, _| {
        native::setup(window, plan.frame, plan.radius, !visible);
        native_handle = NoteFrameHandle::of(window);
    });
    Some(WindowEntry {
        handle,
        view,
        native: native_handle,
        visible,
    })
}

/// 单个便签窗口内的拖拽起点状态。
#[derive(Clone, Copy)]
struct DragSession {
    start_mouse_tl: (f64, f64),
    start_frame_tl: Frame,
    last_frame_tl: Frame,
    started_docked: bool,
    active: bool,
    polling: bool,
}

/// 单个便签窗口视图。
pub struct NoteWindow {
    id: String,
    text: String,
    docked: bool,
    dock_edge: Option<Edge>,
    expanded: bool,
    visible: bool,
    editing: bool,
    activated_for_edit: bool,
    hovered: bool,
    drag: Option<DragSession>,
    snap_preview: Option<Edge>,
    textarea: Entity<TextareaState>,
    focus: FocusHandle,
    hover_task: Option<Task<()>>,
    unhover_task: Option<Task<()>>,
    save_task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl NoteWindow {
    fn new(
        note: Note,
        docked: bool,
        dock_edge: Option<Edge>,
        expanded: bool,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let initial_text = note.text.clone();
        let textarea = cx.new(|cx| {
            let mut ta = TextareaState::new(window, cx)
                .placeholder(tr("写点什么…（Esc 完成）", "Write something… (Esc to finish)"));
            ta.set_value(initial_text.clone(), window, cx);
            ta
        });

        let id_for_change = note.id.clone();
        let id_for_blur = note.id.clone();
        let subs = vec![
            cx.subscribe_in(&textarea, window, move |this, ta, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    let value = ta.read(cx).value().to_string();
                    if value != this.text {
                        this.text = value.clone();
                        update_note_text(&id_for_change, value, cx);
                        this.schedule_debounced_save(cx);
                        cx.notify();
                    }
                }
            }),
            cx.observe_window_activation(window, move |this, window, cx| {
                if window.is_window_active() {
                    if this.editing {
                        this.activated_for_edit = true;
                    }
                    return;
                }
                if this.editing && this.activated_for_edit {
                    let id = id_for_blur.clone();
                    // 给窗口切换留 80ms 缓冲，避免激活瞬间偶发抖动导致立即退出编辑。
                    cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(80))
                            .await;
                        let _ = this.update_in(cx, |this, window, cx| {
                            if this.editing && this.activated_for_edit && !window.is_window_active()
                            {
                                cx.defer(move |cx| {
                                    finish_edit_window(&id, EditEndMode::EndSessionNoReturn, cx);
                                });
                            }
                        });
                    })
                    .detach();
                }
            }),
        ];

        Self {
            id: note.id,
            text: note.text,
            docked,
            dock_edge,
            expanded,
            visible,
            editing: false,
            activated_for_edit: false,
            hovered: false,
            drag: None,
            snap_preview: None,
            textarea,
            focus,
            hover_task: None,
            unhover_task: None,
            save_task: None,
            _subs: subs,
        }
    }

    fn sync_state(
        &mut self,
        note: &Note,
        docked: bool,
        dock_edge: Option<Edge>,
        expanded: bool,
        visible: bool,
        cx: &mut Context<Self>,
    ) {
        let changed = self.docked != docked
            || self.dock_edge != dock_edge
            || self.expanded != expanded
            || self.visible != visible
            || self.text != note.text;
        self.docked = docked;
        self.dock_edge = dock_edge;
        self.expanded = expanded;
        self.visible = visible;
        if !visible {
            self.hovered = false;
            self.hover_task = None;
            self.unhover_task = None;
        }
        if self.text != note.text {
            self.text = note.text.clone();
        }
        if changed {
            cx.notify();
        }
    }

    fn schedule_debounced_save(&mut self, cx: &mut Context<Self>) {
        self.save_task = Some(cx.spawn(async move |_, cx: &mut AsyncApp| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            cx.update(flush_dirty_save);
        }));
    }

    fn enter_edit_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing {
            self.focus_editor(window, cx);
            return;
        }
        self.hover_task = None;
        self.unhover_task = None;
        self.editing = true;
        self.activated_for_edit = false;
        let current_text = self.text.clone();
        self.textarea.update(cx, |ta, cx| {
            if ta.value().as_ref() != current_text {
                ta.set_value(current_text, window, cx);
            }
        });
        if let Some(handle) = NoteFrameHandle::of(window) {
            handle.set_editing(true);
        }
        self.focus_editor(window, cx);
        cx.notify();
    }

    fn focus_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.textarea.update(cx, |ta, cx| {
            let end = end_position(&ta.value());
            ta.set_cursor_position(end, window, cx);
        });
    }

    fn leave_edit_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_edit_mode_inner(window, cx, true);
    }

    fn leave_edit_mode_for_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_edit_mode_inner(window, cx, false);
    }

    fn leave_edit_mode_inner(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        interactive: bool,
    ) {
        if !self.editing {
            return;
        }
        self.editing = false;
        self.activated_for_edit = false;
        self.save_task = None;
        let value = self.textarea.read(cx).value().to_string();
        if value != self.text {
            self.text = value.clone();
            update_note_text(&self.id, value, cx);
        }
        if let Some(handle) = NoteFrameHandle::of(window) {
            handle.set_editing(false);
        }
        if !interactive {
            return;
        }
        self.focus.focus(window, cx);
        // 停靠便签退出编辑后，若鼠标不在其上，直接把悬停标记也收掉。
        if self.docked && !self.mouse_inside_window(window) {
            self.hovered = false;
            let id = self.id.clone();
            cx.defer(move |cx| clear_hovered_dock(&id, cx));
        }
        cx.notify();
    }

    fn mouse_inside_window(&self, window: &Window) -> bool {
        let Some(frame) = NoteFrameHandle::of(window).and_then(|h| h.frame_tl()) else {
            return false;
        };
        let Some((mx, my)) = mouse_location_tl() else {
            return false;
        };
        mx >= frame.x - 2.0
            && mx <= frame.x + frame.w + 2.0
            && my >= frame.y - 2.0
            && my <= frame.y + frame.h + 2.0
    }

    fn on_hover(&mut self, hovered: &bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible {
            self.hovered = false;
            self.hover_task = None;
            self.unhover_task = None;
            return;
        }
        self.hovered = *hovered;
        if *hovered {
            self.unhover_task = None;
            if self.docked && !self.expanded && !self.editing && self.drag.is_none() {
                let id = self.id.clone();
                let window_handle = window.window_handle();
                self.hover_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
                    cx.background_executor().timer(HOVER_OPEN_DELAY).await;
                    let still_hovered = window_handle
                        .update(cx, |_, window, cx| {
                            this.read_with(cx, |w, _| {
                                w.visible && w.hovered && w.mouse_inside_window(window)
                            })
                            .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if still_hovered {
                        cx.update(|cx| set_hovered_dock(Some(id), cx));
                    }
                }));
            }
        } else {
            self.hover_task = None;
            if self.docked && self.expanded && !self.editing && self.drag.is_none() {
                let id = self.id.clone();
                let window_handle = window.window_handle();
                self.unhover_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
                    cx.background_executor().timer(HOVER_CLOSE_DELAY).await;
                    let should_close = window_handle
                        .update(cx, |_, window, cx| {
                            this.read_with(cx, |w, _| {
                                w.docked
                                    && w.expanded
                                    && !w.editing
                                    && w.drag.is_none()
                                    && !w.mouse_inside_window(window)
                            })
                            .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if should_close {
                        cx.update(|cx| clear_hovered_dock(&id, cx));
                    }
                }));
            }
        }
        cx.notify();
    }

    fn start_drag(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if offscreen() {
            return;
        }
        if self.editing {
            let id = self.id.clone();
            cx.defer(move |cx| finish_edit_window(&id, EditEndMode::EndSessionNoReturn, cx));
        }
        let Some(handle) = NoteFrameHandle::of(window) else {
            return;
        };
        let Some(frame_tl) = handle.frame_tl() else {
            return;
        };
        let polling = left_mouse_pressed();
        let event_tl = (
            frame_tl.x + f64::from(event.position.x),
            frame_tl.y + f64::from(event.position.y),
        );
        let mouse_tl = if polling {
            mouse_location_tl().unwrap_or(event_tl)
        } else {
            event_tl
        };
        self.drag = Some(DragSession {
            start_mouse_tl: mouse_tl,
            start_frame_tl: frame_tl,
            last_frame_tl: frame_tl,
            started_docked: self.docked,
            active: false,
            polling,
        });
        self.hover_task = None;
        self.unhover_task = None;

        if polling {
            let view = cx.entity().downgrade();
            let window_handle = window.window_handle();
            cx.spawn(async move |_, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor().timer(DRAG_TICK).await;
                    let still_pressed = left_mouse_pressed();
                    let next = window_handle
                        .update(cx, |_, window, cx| {
                            view.update(cx, |w, cx| w.step_drag_poll(still_pressed, window, cx))
                                .ok()
                        })
                        .ok()
                        .flatten();
                    match next {
                        Some(Some((target, resize, radius))) => {
                            if resize {
                                handle.set_frame_tl(target, radius);
                            } else {
                                handle.set_origin_tl(target.x, target.y, target.h);
                            }
                        }
                        Some(None) => {}
                        None => break,
                    }
                    if !still_pressed {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    /// 从初始或当前指针坐标推进一步拖拽：耳标拖离时切回展开卡片尺寸，并刷新吸边预览。
    fn step_drag_to(
        &mut self,
        cur_mouse: (f64, f64),
        cx: &mut Context<Self>,
    ) -> Option<(Frame, bool)> {
        let drag = self.drag.as_mut()?;
        let dx = cur_mouse.0 - drag.start_mouse_tl.0;
        let dy = cur_mouse.1 - drag.start_mouse_tl.1;
        if !drag.active && (dx * dx + dy * dy).sqrt() < DRAG_THRESHOLD {
            return None;
        }
        let mut resize = false;
        if !drag.active {
            drag.active = true;
            if drag.started_docked && !self.expanded {
                let screens = current_screens();
                let visible = screen_for_frame(&screens, drag.start_frame_tl)
                    .map(|s| s.visible)
                    .unwrap_or(Frame {
                        x: 0.0,
                        y: 37.0,
                        w: 1440.0,
                        h: 863.0,
                    });
                let (cw, ch) = card_size(None, visible);
                drag.start_frame_tl = Frame {
                    x: (cur_mouse.0 - cw / 2.0).round(),
                    y: (cur_mouse.1 - 14.0).round(),
                    w: cw,
                    h: ch,
                };
                drag.start_mouse_tl = cur_mouse;
                self.expanded = true;
                resize = true;
            }
        }
        let cur_frame = Frame {
            x: (drag.start_frame_tl.x + (cur_mouse.0 - drag.start_mouse_tl.0)).round(),
            y: (drag.start_frame_tl.y + (cur_mouse.1 - drag.start_mouse_tl.1)).round(),
            w: drag.start_frame_tl.w,
            h: drag.start_frame_tl.h,
        };
        drag.last_frame_tl = cur_frame;
        let screens = current_screens();
        let preview = screen_for_frame(&screens, cur_frame)
            .and_then(|s| store::snap_edge(cur_frame, s.visible, SNAP_THRESHOLD));
        if self.snap_preview != preview {
            self.snap_preview = preview;
            cx.notify();
        }
        Some((cur_frame, resize))
    }

    fn step_drag_poll(
        &mut self,
        still_pressed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(Frame, bool, f64)> {
        self.drag.as_ref()?;
        if !still_pressed {
            self.finish_drag(window, cx);
            return None;
        }
        let cur_mouse = mouse_location_tl()?;
        let (cur_frame, resize) = self.step_drag_to(cur_mouse, cx)?;
        Some((cur_frame, resize, CARD_CORNER_RADIUS))
    }

    fn advance_pointer_drag(
        &mut self,
        pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.drag.as_ref().is_none_or(|d| d.polling) {
            return;
        }
        let Some(cur_frame) = NoteFrameHandle::of(window).and_then(|h| h.frame_tl()) else {
            return;
        };
        let cur_mouse = (
            cur_frame.x + f64::from(pos.x),
            cur_frame.y + f64::from(pos.y),
        );
        let Some((target, resize)) = self.step_drag_to(cur_mouse, cx) else {
            return;
        };
        if let Some(handle) = NoteFrameHandle::of(window) {
            cx.spawn(async move |_, _| {
                if resize {
                    handle.set_frame_tl(target, CARD_CORNER_RADIUS);
                } else {
                    handle.set_origin_tl(target.x, target.y, target.h);
                }
            })
            .detach();
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.advance_pointer_drag(event.position, window, cx);
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.as_ref().is_some_and(|d| !d.polling) {
            self.advance_pointer_drag(event.position, window, cx);
            self.finish_drag(window, cx);
        }
    }

    fn on_mouse_up_out(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.as_ref().is_some_and(|d| !d.polling) {
            self.advance_pointer_drag(event.position, window, cx);
            self.finish_drag(window, cx);
        }
    }

    fn finish_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        self.snap_preview = None;
        if !drag.active {
            // 轻点未拖动：如果是折起耳标，立即展开并进入编辑。
            if drag.started_docked && !self.expanded {
                let id = self.id.clone();
                cx.defer(move |cx| {
                    set_hovered_dock(Some(id.clone()), cx);
                    begin_edit(&id, cx);
                });
            }
            cx.notify();
            return;
        }
        let frame_tl = if offscreen() || !drag.polling {
            drag.last_frame_tl
        } else {
            NoteFrameHandle::of(window)
                .and_then(|h| h.frame_tl())
                .unwrap_or(drag.last_frame_tl)
        };
        let id = self.id.clone();
        cx.defer(move |cx| commit_drop(&id, frame_tl, cx));
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editing {
            return;
        }
        let key = event.keystroke.key.as_str();
        let mods = &event.keystroke.modifiers;
        if key == "escape" && !mods.modified() {
            // 如果中文输入法仍在组字状态（有 marked text），把 Esc 留给输入法消候选词。
            let composing = self.textarea.update(cx, |ta, cx| {
                gpui_kit::EntityInputHandler::marked_text_range(ta, window, cx).is_some()
            });
            if composing {
                return;
            }
            cx.stop_propagation();
            let id = self.id.clone();
            cx.defer(move |cx| finish_edit_window(&id, EditEndMode::EndSessionReturnFocus, cx));
        }
    }
}

fn commit_drop(id: &str, dropped_frame: Frame, cx: &mut App) {
    let screens = current_screens();
    let Some(screen) = screen_for_frame(&screens, dropped_frame) else {
        return;
    };
    let snapped = store::snap_edge(dropped_frame, screen.visible, SNAP_THRESHOLD);
    let mut docked_now = false;
    {
        let mgr = cx.global_mut::<NotesManager>();
        let order = snapped
            .map(|edge| store::next_order(&mgr.notes, edge, &screen.uuid))
            .unwrap_or(0);
        if let Some(note) = mgr.notes.iter_mut().find(|n| n.id == id) {
            note.size = Some((
                dropped_frame.w.max(super::screen::MIN_CARD_W),
                dropped_frame.h.max(super::screen::MIN_CARD_H),
            ));
            note.updated_at = now_unix_secs();
            note.placement = match snapped {
                Some(edge) => {
                    docked_now = true;
                    Placement::Docked {
                        edge,
                        display: screen.uuid.clone(),
                        order,
                    }
                }
                None => Placement::Floating {
                    frame: clamp_floating(dropped_frame, screen.visible),
                },
            };
        }
        if mgr.hovered_dock_id.as_deref() == Some(id) {
            mgr.hovered_dock_id = None;
        }
        store::save(&mgr.notes);
        set_snapshot(&mgr.notes);
    }
    if docked_now {
        telemetry::note(NoteEvent::Docked);
    }
    sync_all_windows(cx);
}

fn end_position(text: &str) -> Position {
    let line = text.matches('\n').count();
    let last = text.rsplit('\n').next().unwrap_or_default();
    Position::new(
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(last.encode_utf16().count()).unwrap_or(u32::MAX),
    )
}

/// 卡片微缩标题栏摘要（取首行非空文字，截断至 24 个字符）。
pub fn summary_title(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if first.is_empty() {
        return tr("便签", "Note").to_string();
    }
    match first.char_indices().nth(24) {
        Some((cut, _)) => format!("{}…", &first[..cut]),
        None => first.to_string(),
    }
}

/// 书签耳标上的竖排短字（最多 2~3 字，避免在小高度压缩下溢出）。
pub fn tab_chars(text: &str) -> Vec<String> {
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_else(|| tr("便签", "Note"));
    first
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .take(3)
        .map(|ch| ch.to_string())
        .collect()
}

impl Render for NoteWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::settings::sync_component_theme(window, cx);
        let pal = style::palette(window);
        let id = self.id.clone();

        if self.docked && !self.expanded {
            return self.render_tab(&pal, cx);
        }
        self.render_card(id, &pal, window, cx)
    }
}

impl NoteWindow {
    fn render_tab(&mut self, pal: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let accent = style::tint(crate::provider::Tint::Orange);
        let strip_color = if self.hovered { pal.accent } else { accent };
        let chars = tab_chars(&self.text);
        let right_edge = self.dock_edge != Some(Edge::Left);

        let strip = div()
            .flex_none()
            .w(px(3.5))
            .h_full()
            .bg(strip_color);
        let label = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(1.))
            .children(chars.into_iter().map(|ch| {
                div()
                    .text_size(px(10.5))
                    .line_height(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if self.hovered { pal.fg } else { pal.muted })
                    .child(ch)
            }));

        let tab = div()
            .id(SharedString::from(format!("note-tab-{}", self.id)))
            .size_full()
            .flex()
            .overflow_hidden()
            .rounded(px(TAB_CORNER_RADIUS as f32))
            .bg(pal.panel)
            .border_1()
            .border_color(if self.hovered {
                pal.accent.opacity(0.65)
            } else {
                pal.separator
            })
            .cursor(CursorStyle::PointingHand)
            .on_hover(cx.listener(Self::on_hover))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::start_drag))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up_out));
        if right_edge {
            tab.child(label).child(strip).into_any_element()
        } else {
            tab.child(strip).child(label).into_any_element()
        }
    }

    fn render_card(
        &mut self,
        id: String,
        pal: &Palette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dot_color = if self.docked {
            pal.accent
        } else {
            style::tint(crate::provider::Tint::Orange)
        };
        let title = summary_title(&self.text);

        let header = div()
            .id(SharedString::from(format!("note-header-{id}")))
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(10.))
            .border_b_1()
            .border_color(pal.separator.opacity(0.75))
            .cursor(CursorStyle::OpenHand)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::start_drag))
            .child(div().flex_none().size(px(7.)).rounded_full().bg(dot_color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(pal.fg.opacity(0.88))
                    .truncate()
                    .child(title),
            )
            .when(self.editing, |el| {
                el.child(
                    div()
                        .flex_none()
                        .px(px(5.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(pal.accent.opacity(0.15))
                        .text_size(px(10.))
                        .text_color(pal.accent)
                        .child(tr("编辑中", "Editing")),
                )
            })
            .child(
                div()
                    .id(SharedString::from(format!("note-close-{id}")))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(18.))
                    .rounded(px(5.))
                    .cursor(CursorStyle::PointingHand)
                    .hover(|el| el.bg(pal.key_bg))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        // 防止关闭按钮按下触发标题栏拖拽。
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        let id = id.clone();
                        cx.defer(move |cx| archive_note(&id, cx));
                    }))
                    .child(
                        gpui_kit::component::Icon::empty()
                            .path("icons/x.svg")
                            .text_color(pal.muted)
                            .size(px(11.)),
                    ),
            );

        let body = if self.editing {
            div()
                .flex_1()
                .min_h_0()
                .px(px(10.))
                .py(px(8.))
                .text_size(px(13.))
                .text_color(pal.fg)
                .child(
                    Textarea::new(&self.textarea)
                        .appearance(false)
                        .bordered(false)
                        .h_full(),
                )
                .into_any_element()
        } else {
            let empty = self.text.trim().is_empty();
            let preview_text = if empty {
                tr("点击输入便签内容…", "Click to write a note…").to_string()
            } else {
                self.text.clone()
            };
            div()
                .id(SharedString::from(format!("note-body-{}", self.id)))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(11.))
                .py(px(9.))
                .cursor(CursorStyle::IBeam)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    let id = this.id.clone();
                    cx.defer(move |cx| begin_edit(&id, cx));
                }))
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(19.))
                        .text_color(if empty { pal.muted } else { pal.fg })
                        .whitespace_normal()
                        .child(preview_text),
                )
                .into_any_element()
        };

        let mut card = div()
            .id(SharedString::from(format!("note-card-{}", self.id)))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(CARD_CORNER_RADIUS as f32))
            .bg(pal.panel)
            .border_1()
            .border_color(if self.editing {
                pal.accent
            } else {
                pal.separator
            })
            .track_focus(&self.focus)
            .on_hover(cx.listener(Self::on_hover))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up_out))
            .child(header)
            .child(body);

        if let Some(edge) = self.snap_preview {
            let indicator = div()
                .absolute()
                .top_0()
                .bottom_0()
                .w(px(3.))
                .bg(pal.accent)
                .when(edge == Edge::Left, |el| el.left_0())
                .when(edge == Edge::Right, |el| el.right_0());
            card = card.child(indicator);
        }

        card.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // gpui_kit::* 里也有 `#[test]` 宏，显式引标准库 test。
    use core::prelude::v1::test;

    use super::*;

    fn sample_screens() -> Vec<DisplayInfo> {
        vec![
            DisplayInfo {
                id: 1,
                uuid: "main".into(),
                frame: Frame {
                    x: 0.0,
                    y: 0.0,
                    w: 1512.0,
                    h: 982.0,
                },
                visible: Frame {
                    x: 0.0,
                    y: 37.0,
                    w: 1512.0,
                    h: 880.0,
                },
            },
            DisplayInfo {
                id: 2,
                uuid: "side".into(),
                frame: Frame {
                    x: 1512.0,
                    y: 0.0,
                    w: 1920.0,
                    h: 1080.0,
                },
                visible: Frame {
                    x: 1512.0,
                    y: 25.0,
                    w: 1920.0,
                    h: 1055.0,
                },
            },
        ]
    }

    #[test]
    fn summary_and_tab_chars_extract_first_non_empty_line() {
        // 首行空行或缩进不能让耳标和标题变成空白。
        assert_eq!(summary_title("\n  发版检查\n第二行"), "发版检查");
        assert_eq!(tab_chars("\n  发版检查\n第二行"), ["发", "版", "检"]);
        assert_eq!(summary_title("   \n"), "便签");
    }

    #[test]
    fn plan_windows_hides_archived_and_merges_missing_display_to_primary() {
        // 已归档便签不出桌面窗；拔掉外接屏时原外接屏同侧便签回主屏同侧队列且不重叠。
        let screens = vec![sample_screens().remove(0)];
        let notes = vec![
            Note {
                id: "n1".into(),
                text: "主屏".into(),
                created_at: 1,
                updated_at: 1,
                placement: Placement::Docked {
                    edge: Edge::Right,
                    display: "main".into(),
                    order: 0,
                },
                size: None,
            },
            Note {
                id: "n2".into(),
                text: "原副屏".into(),
                created_at: 2,
                updated_at: 2,
                placement: Placement::Docked {
                    edge: Edge::Right,
                    display: "missing-uuid".into(),
                    order: 1,
                },
                size: None,
            },
            Note {
                id: "n3".into(),
                text: "已归档".into(),
                created_at: 3,
                updated_at: 3,
                placement: Placement::Archived,
                size: None,
            },
        ];
        let planned = plan_windows(&notes, &screens, false, None, None);
        assert_eq!(planned.len(), 2);
        assert!(planned.iter().all(|p| p.visible));
        assert!(planned[1].frame.y >= planned[0].frame.y + planned[0].frame.h);
        assert_eq!(planned[0].frame.x, 1512.0 - TAB_W);
        assert_eq!(planned[1].frame.x, 1512.0 - TAB_W);
    }

    #[test]
    fn hovering_one_docked_note_expands_only_that_note() {
        // 悬停某张耳标只展开这一张，同列其他便签必须保持 24pt 窄耳标不展开。
        let screens = sample_screens();
        let notes = vec![
            Note {
                id: "a".into(),
                text: "A".into(),
                created_at: 1,
                updated_at: 1,
                placement: Placement::Docked {
                    edge: Edge::Right,
                    display: "main".into(),
                    order: 0,
                },
                size: None,
            },
            Note {
                id: "b".into(),
                text: "B".into(),
                created_at: 2,
                updated_at: 2,
                placement: Placement::Docked {
                    edge: Edge::Right,
                    display: "main".into(),
                    order: 1,
                },
                size: None,
            },
        ];
        let planned = plan_windows(&notes, &screens, false, Some("b"), None);
        assert!(!planned[0].expanded);
        assert_eq!(planned[0].frame.w, TAB_W);
        assert!(planned[1].expanded);
        assert!(planned[1].frame.w > 200.0);
        assert!(planned[1].order_front);
    }

    #[test]
    fn hidden_all_hides_windows_without_losing_positions() {
        let screens = sample_screens();
        let notes = vec![Note {
            id: "a".into(),
            text: "A".into(),
            created_at: 1,
            updated_at: 1,
            placement: Placement::Docked {
                edge: Edge::Left,
                display: "main".into(),
                order: 0,
            },
            size: None,
        }];
        let planned = plan_windows(&notes, &screens, true, None, None);
        assert_eq!(planned.len(), 1);
        assert!(!planned[0].visible);
    }

    #[test]
    fn blur_clears_editing_return_pid_while_note_to_note_transfer_preserves_it() {
        // 失焦结束编辑会话必须清掉归还 PID（避免下次从其他 App 编辑后 Esc 误跳回旧 App）；
        // 只有从一张便签直接切到另一张便签编辑时，才保留首次捕获的外部 App PID。
        let mut mgr = NotesManager::from_notes(Vec::new());

        assert_eq!(mgr.start_edit_session("n1", || 101), Some(101));
        assert_eq!(mgr.editing_return_pid, 101);

        // 便签间转移：退出 n1 但保留归还 PID 101，也不应清理 hand-over。
        assert_eq!(
            mgr.end_edit_session("n1", EditEndMode::TransferToAnotherNote),
            (0, false)
        );
        assert_eq!(mgr.editing_return_pid, 101);
        assert_eq!(mgr.start_edit_session("n2", || 999), None);
        assert_eq!(mgr.editing_return_pid, 101);

        // 失焦结束编辑（切到 App B）：不主动 return_focus，但必须清零 PID 并要求清理 hand-over。
        assert_eq!(
            mgr.end_edit_session("n2", EditEndMode::EndSessionNoReturn),
            (0, true)
        );
        assert_eq!(mgr.editing_id, None);
        assert_eq!(mgr.editing_return_pid, 0);

        // 从 App B 再次 begin_edit：PID 已清零，必须重新捕获 B(202)。
        assert_eq!(mgr.start_edit_session("n1", || 202), Some(202));
        assert_eq!(mgr.editing_return_pid, 202);

        // 按 Esc 结束会话：返回新捕获的 202 并清空 PID 与 hand-over。
        assert_eq!(
            mgr.end_edit_session("n1", EditEndMode::EndSessionReturnFocus),
            (202, true)
        );
        assert_eq!(mgr.editing_id, None);
        assert_eq!(mgr.editing_return_pid, 0);
    }

    #[test]
    fn hidden_overflow_tabs_never_enter_hovered_on_pointer_hit() {
        // 880pt 可见区同侧 25 张停靠便签时第 25 张溢出并隐藏在左上角；
        // 即使鼠标停在那块不可见坐标超过 150ms，也不得触发悬停展开。
        let screens = vec![sample_screens().remove(0)]; // visible.h = 880
        let notes: Vec<Note> = (0..25)
            .map(|i| Note {
                id: format!("n{i}"),
                text: format!("便签 {i}"),
                created_at: i as i64,
                updated_at: i as i64,
                placement: Placement::Docked {
                    edge: Edge::Right,
                    display: "main".into(),
                    order: i as u32,
                },
                size: None,
            })
            .collect();

        let planned = plan_windows(&notes, &screens, false, None, None);
        assert_eq!(planned.len(), 25);
        assert!(planned[..24].iter().all(|p| p.visible));
        assert!(!planned[24].visible);

        let candidates: Vec<HoverCandidate> = planned
            .iter()
            .map(|p| HoverCandidate {
                id: p.id.clone(),
                frame: p.frame,
                visible: p.visible,
            })
            .collect();
        let hidden_frame = planned[24].frame;
        let mx = hidden_frame.x + hidden_frame.w / 2.0;
        let my = hidden_frame.y + hidden_frame.h / 2.0;

        let mut mgr = NotesManager::from_notes(notes.clone());
        let t0 = Instant::now();
        assert_eq!(
            mgr.step_dock_hover(mx, my, t0, &candidates),
            (None, None)
        );
        assert_eq!(
            mgr.step_dock_hover(mx, my, t0 + Duration::from_millis(200), &candidates),
            (None, None)
        );
        assert_eq!(mgr.hover_candidate, None);
        assert_eq!(mgr.hovered_dock_id, None);

        // 停在第一张真正可见的耳标上超过 150ms，应该正常进入 hovered。
        let visible_frame = planned[0].frame;
        let vx = visible_frame.x + visible_frame.w / 2.0;
        let vy = visible_frame.y + visible_frame.h / 2.0;
        assert_eq!(
            mgr.step_dock_hover(vx, vy, t0, &candidates),
            (None, None)
        );
        assert_eq!(
            mgr.step_dock_hover(vx, vy, t0 + Duration::from_millis(200), &candidates),
            (None, Some("n0".into()))
        );
    }

    #[test]
    fn runtime_display_changes_replan_missing_screen_and_shrunk_visible_area() {
        // 运行中拔掉外屏或分辨率/可见区缩小时，必须将原外屏停靠便签并入主屏重排、
        // 把超出容量的耳标收起隐藏，并将浮窗约束回新的可见区域。
        let initial_screens = sample_screens();
        let mut notes: Vec<Note> = (0..10)
            .map(|i| Note {
                id: format!("main-{i}"),
                text: format!("M{i}"),
                created_at: i,
                updated_at: i,
                placement: Placement::Docked {
                    edge: Edge::Right,
                    display: "main".into(),
                    order: i as u32,
                },
                size: None,
            })
            .collect();
        notes.push(Note {
            id: "ext-dock".into(),
            text: "外屏停靠".into(),
            created_at: 20,
            updated_at: 20,
            placement: Placement::Docked {
                edge: Edge::Right,
                display: "side".into(),
                order: 100,
            },
            size: None,
        });
        notes.push(Note {
            id: "ext-float".into(),
            text: "外屏浮窗".into(),
            created_at: 21,
            updated_at: 21,
            placement: Placement::Floating {
                frame: Frame {
                    x: 2000.0,
                    y: 400.0,
                    w: 280.0,
                    h: 200.0,
                },
            },
            size: None,
        });

        let initial_plan = plan_windows(&notes, &initial_screens, false, None, None);
        assert!(initial_plan.iter().all(|p| p.visible));
        assert_eq!(
            initial_plan.iter().find(|p| p.id == "ext-dock").unwrap().frame.x,
            1512.0 + 1920.0 - TAB_W
        );

        // 模拟外屏断开且主屏可见区高度缩小为 300pt（仅能容纳 8 张，第 9 张起隐藏）。
        let shrunk_main = vec![DisplayInfo {
            id: 1,
            uuid: "main".into(),
            frame: Frame {
                x: 0.0,
                y: 0.0,
                w: 1280.0,
                h: 400.0,
            },
            visible: Frame {
                x: 0.0,
                y: 30.0,
                w: 1280.0,
                h: 300.0,
            },
        }];
        let replanned = plan_windows(&notes, &shrunk_main, false, None, None);
        let visible_docked: Vec<&PlannedWindow> = replanned
            .iter()
            .filter(|p| p.docked && p.visible)
            .collect();
        assert_eq!(visible_docked.len(), 8);
        for pair in visible_docked.windows(2) {
            assert!(pair[1].frame.y >= pair[0].frame.y + pair[0].frame.h);
        }
        for p in &visible_docked {
            assert_eq!(p.frame.x, 1280.0 - TAB_W);
            assert!(p.frame.y >= 30.0 && p.frame.y + p.frame.h <= 330.0);
        }
        assert!(!replanned.iter().find(|p| p.id == "ext-dock").unwrap().visible);

        let floated = replanned.iter().find(|p| p.id == "ext-float").unwrap();
        assert!(floated.frame.x >= 0.0 && floated.frame.x + floated.frame.w <= 1280.0);
        assert!(floated.frame.y >= 30.0 && floated.frame.y + floated.frame.h <= 330.0);
    }

    #[test]
    fn quit_flushes_debounced_and_in_editor_text_so_reload_reads_latest() {
        // 500ms debounce 尚未触发时立即退出，必须同步结束编辑并写盘；重启重新加载能读到最新内容。
        struct TempNotesFile(std::path::PathBuf);
        impl TempNotesFile {
            fn new() -> Self {
                static SEQ: AtomicU64 = AtomicU64::new(0);
                let dir = std::env::temp_dir().join(format!(
                    "helmp-notes-quit-{}-{}",
                    std::process::id(),
                    SEQ.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&dir).unwrap();
                Self(dir)
            }
            fn file(&self) -> std::path::PathBuf {
                self.0.join(store::FILE)
            }
        }
        impl Drop for TempNotesFile {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        let dir = TempNotesFile::new();
        let write_blocked = AtomicBool::new(true);
        let initial = store::new_note("草稿 v1", 100, Placement::Archived);
        let note_id = initial.id.clone();
        assert!(
            store::load_recovering_from(&dir.file(), &write_blocked)
                .unwrap()
                .is_empty()
        );
        store::save_checked_to(&dir.file(), &[initial], &write_blocked).unwrap();

        let loaded = store::load_recovering_from(&dir.file(), &write_blocked).unwrap();
        let mut mgr = NotesManager::from_notes(loaded);

        // 模拟输入还在 500ms debounce 期内（dirty_save = true，尚未写盘）。
        assert!(mgr.apply_text_edit(&note_id, "草稿 v2（debounce 中）".into(), false));
        assert!(mgr.dirty_save);
        assert_eq!(store::load_from(&dir.file()).unwrap()[0].text, "草稿 v1");

        // 模拟临退出瞬间编辑器内又有最新正文，同步冲刷写盘（不等待 500ms）。
        mgr.flush_dirty_with(
            Some((&note_id, "草稿 v3（立即退出前最后输入）".into())),
            false,
            |notes| {
                store::save_checked_to(&dir.file(), notes, &write_blocked).unwrap();
            },
        );
        assert!(!mgr.dirty_save);

        // 模拟重启应用后从磁盘重新加载，必须读到最后输入的内容。
        let reloaded = store::load_from(&dir.file()).unwrap();
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded[0].text, "草稿 v3（立即退出前最后输入）");
    }
}
