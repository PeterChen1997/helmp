//! 启动器面板：统一的 Spotlight 式结构（设计稿「统一的面板结构」）。
//!
//! 720 × 520：顶部 60 搜索栏，底部 40 状态栏，中间三种视图之一——
//! 列表 / 列表 + 详情 / 表单 + 详情。插件只给数据（`provider.rs`），这里负责全部渲染与按键。
//!
//! 按键：↑↓ 选择，↵ 主操作，⌘K 操作菜单，次级操作自带快捷键，⇥ 进入选中项的扩展 / 切换范围内的筛选，
//! 范围内 ↑ 调出历史输入（如翻译过的原文），esc 依次关闭操作菜单 → 清掉结果 → 退出范围 → 隐藏面板。
//!
//! 表单项标了 `auto_run` 时（翻译），输入停顿 `AUTO_RUN_DELAY` 后自动执行，等待和执行期间详情区显示加载占位。

use std::time::{Duration, Instant};

use gpui_kit::component::input::{
    Input, InputEvent, InputState, Position, Textarea, TextareaState,
};
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::engine::{Engine, RECENT_ID_PREFIX};
use crate::i18n::{tr, trf};
use crate::provider::{
    Accessory, ActionInput, Block, Detail, FieldKind, Form, Icon as ItemIcon, Item, NamedAction,
    Outcome, Scope,
};
use crate::style::{self, Palette};

pub const PANEL_SIZE: Size<Pixels> = size(px(720.), px(520.));
const SIDE_WIDTH: Pixels = px(310.);

/// 面板可见时定期重查，让 agent 状态、剪贴板等后台变化与新生成的图标反映到界面。
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

/// 输入即译：输入停顿这么久才自动执行，避免每敲一个字就调一次模型。
const AUTO_RUN_DELAY: Duration = Duration::from_millis(400);

/// 隐藏超过这么久再唤起，就清空上次输入回到空白搜索（参考 Raycast「Pop to Root Search」默认 90 秒）。
/// `HELMP_RESET_AFTER_SECS` 优先于设置里的值，0 表示每次唤起都清空。
fn reset_after() -> Duration {
    std::env::var("HELMP_RESET_AFTER_SECS")
        .ok()
        .and_then(|secs| secs.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(crate::config::get().general.reset_after_secs))
}

/// 唤起时是否清空上次输入。
/// - 刚执行过一个结果（`executed`）：任务已完成，下次唤起是新任务，清空。
/// - 隐藏时长达到阈值：大概率是新的意图，清空。
/// - 否则保留上次输入（调用方会全选它，直接打字即替换），方便改一下再搜。
fn should_reset(hidden_for: Option<Duration>, reset_after: Duration, executed: bool) -> bool {
    executed || hidden_for.is_some_and(|hidden_for| hidden_for >= reset_after)
}

struct ActiveScope {
    scope: Scope,
    filter: usize,
}

impl ActiveScope {
    fn filter_name(&self) -> &str {
        self.scope
            .filters
            .get(self.filter)
            .map(String::as_str)
            .unwrap_or_default()
    }
}

/// 执行后要在详情区展示的结果（`Outcome::Show` / `Outcome::Message` / 出错）。
struct ResultView {
    item_id: String,
    detail: Detail,
    /// 结果来自列表之外（全局快捷键、菜单栏直接执行）：列表只显示这一项，后台刷新不替换它，
    /// 否则结果会被当前查询的列表顶掉，回车也会落到别的项上。
    pinned: bool,
}

enum FieldState {
    Line(Entity<InputState>),
    Area(Entity<TextareaState>),
    /// 选中的 option 下标。
    Select(usize),
}

struct FormState {
    item_id: String,
    form: Form,
    states: Vec<FieldState>,
    _subscriptions: Vec<Subscription>,
}

impl FormState {
    fn values(&self, cx: &App) -> ActionInput {
        let mut input = ActionInput::default();
        for (field, state) in self.form.fields.iter().zip(&self.states) {
            let value = match (state, &field.kind) {
                (FieldState::Line(input), _) => input.read(cx).value().to_string(),
                (FieldState::Area(area), _) => area.read(cx).value().to_string(),
                (FieldState::Select(ix), FieldKind::Select { options }) => options
                    .get(*ix)
                    .map(|(_, value)| value.clone())
                    .unwrap_or_default(),
                (FieldState::Select(_), _) => String::new(),
            };
            input.form.insert(field.key.clone(), value);
        }
        input
    }

    /// 文本字段里有没有字（输入即译：全空就不必执行）。
    fn has_text(&self, cx: &App) -> bool {
        self.states.iter().any(|state| match state {
            FieldState::Line(input) => !input.read(cx).value().trim().is_empty(),
            FieldState::Area(area) => !area.read(cx).value().trim().is_empty(),
            FieldState::Select(_) => false,
        })
    }

    /// 所有文本字段都是空的（连空格、换行也没有）：命令范围里再按 ⌫ 就退出范围。
    fn is_blank(&self, cx: &App) -> bool {
        self.states.iter().all(|state| match state {
            FieldState::Line(input) => input.read(cx).value().is_empty(),
            FieldState::Area(area) => area.read(cx).value().is_empty(),
            FieldState::Select(_) => true,
        })
    }

    fn focused(&self, window: &Window, cx: &App) -> bool {
        self.states.iter().any(|state| match state {
            FieldState::Line(input) => input.read(cx).focus_handle(cx).is_focused(window),
            FieldState::Area(area) => area.read(cx).focus_handle(cx).is_focused(window),
            FieldState::Select(_) => false,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    List,
    Split,
    Form,
}

pub struct Launcher {
    engine: Engine,
    input: Entity<InputState>,
    query: String,
    scope: Option<ActiveScope>,
    items: Vec<Item>,
    selected: usize,
    /// 每次发起查询加一，后台结果回来时版本不对就丢弃。
    revision: u64,
    result: Option<ResultView>,
    form: Option<FormState>,
    /// 刚进入范围：下一次建出输入即译的表单（翻译）时，把焦点放进表单的文本框。
    focus_form: bool,
    actions_open: bool,
    action_selected: usize,
    running: bool,
    visible: bool,
    hidden_at: Option<Instant>,
    /// 上次唤起后执行过结果，下次唤起清空。
    executed: bool,
    scroll: ScrollHandle,
    /// 渲染时记录：第 i 个结果在滚动容器里是第几个子元素（小节标题也占位）。
    row_children: Vec<usize>,
    /// 上次取文案时的语言代数（见 `sync_language`）。
    lang_generation: u64,
    window_handle: AnyWindowHandle,
    /// 搜索框当前的占位文字（随范围、索引状态变化，见 `placeholder_for`）。
    placeholder: String,
    /// 等待防抖结束的自动执行（输入即译）。被替换或丢弃即取消。
    auto_task: Option<Task<()>>,
    /// 自动执行已排队、还没开始：详情区显示加载占位。
    auto_pending: bool,
    /// 每次在表单里原地执行加一；结果回来时不是最新一次就丢弃（输入即译时旧请求可能晚到）。
    form_run: u64,
    /// 范围内 ↑ 调出的历史：(历史输入，最近的在前；当前下标)。
    recall: Option<(Vec<String>, usize)>,
    /// 主结果文字的只读文本框（可选中复制），按文字缓存。
    result_text: Option<(String, Entity<TextareaState>)>,
    _refresh: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Launcher {
    pub fn new(engine: Engine, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let placeholder = placeholder_for(None, engine.is_started(APPS_PROVIDER));
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.clone()));

        let subscriptions = vec![
            cx.subscribe_in(&input, window, {
                let input = input.clone();
                move |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::Change = event {
                        let value = input.read(cx).value().to_string();
                        // 诊断：输入法组字中的未确认文字（marked text）范围。
                        let marked = input.update(cx, |input, cx| {
                            gpui_kit::EntityInputHandler::marked_text_range(input, window, cx)
                        });
                        crate::trace::log(format!("input change {value:?} marked={marked:?}"));
                        this.on_query_changed(value, window, cx);
                    }
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                crate::trace::log(format!(
                    "window activation：active={}",
                    window.is_window_active()
                ));
                if this.visible && !window.is_window_active() {
                    // 不立刻收起：从「已在运行」卡片、设置等别的窗口唤起面板时，焦点会在窗口之间
                    // 短暂跳一下（失去又马上回来），立刻收起会让面板闪一下就没了。150 ms 后仍不在前台才收。
                    cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(150))
                            .await;
                        let _ = this.update_in(cx, |this, window, cx| {
                            if this.visible && !window.is_window_active() {
                                crate::trace::log("面板失去焦点，收起");
                                this.hide(window, cx);
                            }
                        });
                    })
                    .detach();
                }
            }),
        ];

        let window_handle = window.window_handle();
        let refresh = cx.spawn(async move |this, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(REFRESH_INTERVAL).await;
                let alive = window_handle
                    .update(cx, |_, window, cx| {
                        this.update(cx, |this, cx| {
                            if this.visible {
                                // 应用索引建完后占位文字从「正在建立应用索引…」换回常规提示。
                                this.sync_placeholder(window, cx);
                                if !this.running {
                                    this.refresh(cx);
                                }
                            }
                        })
                        .is_ok()
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        });

        Self {
            engine,
            input,
            query: String::new(),
            scope: None,
            items: Vec::new(),
            selected: 0,
            revision: 0,
            result: None,
            form: None,
            focus_form: false,
            actions_open: false,
            action_selected: 0,
            running: false,
            visible: false,
            hidden_at: None,
            executed: false,
            scroll: ScrollHandle::new(),
            row_children: Vec::new(),
            lang_generation: crate::i18n::generation(),
            window_handle,
            placeholder,
            auto_task: None,
            auto_pending: false,
            form_run: 0,
            recall: None,
            result_text: None,
            _refresh: refresh,
            _subscriptions: subscriptions,
        }
    }

    // ---------- 供外部（菜单栏、全局快捷键、自检）调用 ----------

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn input_is_focused(&self, window: &Window, cx: &App) -> bool {
        self.input.read(cx).focus_handle(cx).is_focused(window)
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn scope_id(&self) -> Option<&str> {
        self.scope.as_ref().map(|active| active.scope.id.as_str())
    }

    pub fn message(&self) -> Option<String> {
        self.result.as_ref().map(|result| {
            result
                .detail
                .blocks
                .iter()
                .map(|block| match block {
                    Block::Text(text)
                    | Block::Code(text)
                    | Block::Caption(text)
                    | Block::Result { text, .. } => text.clone(),
                    Block::Quote { text, .. } => text.clone(),
                    Block::Image(path) => path.display().to_string(),
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    /// 菜单栏等需要直接查询时用（同一个引擎，结果与面板一致）。
    pub fn engine(&self) -> Engine {
        self.engine.clone()
    }

    /// 图形自检用：范围、焦点位置、表单内容和结果摘要，一行打印。
    pub fn debug_state(&self, window: &Window, cx: &App) -> String {
        let focus = if self.input_is_focused(window, cx) {
            "search"
        } else if self.form.as_ref().is_some_and(|form| form.focused(window, cx)) {
            "form"
        } else {
            "none"
        };
        let form = self
            .form
            .as_ref()
            .map(|form| {
                let mut values: Vec<_> = form.values(cx).form.into_iter().collect();
                values.sort();
                format!("{values:?}")
            })
            .unwrap_or_default();
        let result = self
            .result
            .as_ref()
            .map(|result| {
                let blocks: Vec<String> = result
                    .detail
                    .blocks
                    .iter()
                    .filter_map(|block| match block {
                        Block::Text(text) | Block::Caption(text) | Block::Code(text) => {
                            Some(text.clone())
                        }
                        Block::Result { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect();
                format!("{} | {}", result.detail.title, blocks.join(" | "))
            })
            .unwrap_or_default();
        format!(
            "scope={:?} query={:?} focus={focus} form={form} result={result:?}",
            self.scope_id(),
            self.query
        )
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// 回到根视图（退出范围、清空输入与结果）。自检与带查询的 `helmp://show` 用。
    pub fn reset_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reset(window, cx);
        self.refresh(cx);
    }

    /// 自检用：等价于用户在输入框里敲入 `query`。
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_value(query.to_string(), window, cx)
        });
        self.on_query_changed(query.to_string(), window, cx);
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible {
            self.hide(window, cx);
        } else {
            self.show(window, cx);
        }
    }

    /// 唤起面板并直接进入某个范围（菜单栏「剪贴板历史」、⌘⇧V 等）。
    pub fn open_scope(&mut self, scope_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_scope_with(scope_id, String::new(), window, cx);
    }

    /// 唤起面板、进入范围，并把 `text` 作为范围里的初始输入（如把识别出的文字带进翻译）。
    pub fn open_scope_with(
        &mut self,
        scope_id: &str,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 面板显示之前、用户的 App 还在前台时通知插件（翻译在这里取词）。带着文字进入的不需要取词。
        if text.is_empty() {
            self.engine.will_enter(scope_id, !self.visible);
        }
        if !self.visible {
            self.show(window, cx);
        }
        match self.engine.scope(scope_id) {
            Some(scope) => self.enter_scope(scope, text, window, cx),
            None => eprintln!("helmp: 没有名为 {scope_id} 的范围（扩展可能被停用）"),
        }
    }

    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_inner(true, window, cx);
    }

    /// `allow_reset = false`：为了展示刚执行完的结果而重新打开面板，不能按「执行过就清空」把结果清掉。
    fn show_inner(&mut self, allow_reset: bool, window: &mut Window, cx: &mut Context<Self>) {
        crate::trace::log("launcher.show");
        self.visible = true;
        let hidden_for = self.hidden_at.take().map(|at| at.elapsed());
        let executed = std::mem::take(&mut self.executed);
        if allow_reset && should_reset(hidden_for, reset_after(), executed) {
            crate::trace::log("reset query");
            self.reset(window, cx);
        } else if !self.query.is_empty() {
            self.input
                .update(cx, |input, cx| input.select_all(window, cx));
        }
        self.sync_language(window, cx);
        crate::panel::show(window, cx);
        self.input.read(cx).focus_handle(cx).focus(window, cx);
        self.refresh(cx);
        cx.notify();
    }

    pub fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::trace::log("launcher.hide");
        self.visible = false;
        self.actions_open = false;
        self.hidden_at = Some(Instant::now());
        crate::panel::hide(window);
        cx.notify();
    }

    // ---------- 状态变化 ----------

    /// 界面语言切换后（在设置窗口里切的，那时面板已隐藏），唤起时重取构造时固定下来的文案：
    /// 搜索框占位，以及当前范围的标签、占位与筛选名（筛选按下标保持不变）。结果随后由 `refresh` 重查。
    fn sync_language(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let generation = crate::i18n::generation();
        if generation == self.lang_generation {
            return;
        }
        self.lang_generation = generation;
        if let Some(active) = self.scope.as_mut()
            && let Some(scope) = self.engine.scope(&active.scope.id)
        {
            active.scope = scope;
        }
        self.sync_placeholder(window, cx);
    }

    /// 按当前范围和索引状态更新搜索框占位文字；没变就不动输入框。
    fn sync_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let want = placeholder_for(
            self.scope
                .as_ref()
                .map(|active| active.scope.placeholder.as_str()),
            self.engine.is_started(APPS_PROVIDER),
        );
        if want != self.placeholder {
            self.placeholder = want.clone();
            self.input
                .update(cx, |input, cx| input.set_placeholder(want, window, cx));
        }
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.query.clear();
        self.scope = None;
        self.selected = 0;
        self.result = None;
        self.form = None;
        self.actions_open = false;
        self.recall = None;
        self.cancel_auto_run();
        self.sync_placeholder(window, cx);
    }

    fn on_query_changed(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        if value == self.query {
            return;
        }
        if self.scope.is_none()
            && let Some((scope, rest)) = self.engine.resolve_prefix(&value)
        {
            // 输入了范围前缀（如「cb 」）：进入范围，把前缀从输入框里拿掉。
            self.engine.will_enter(&scope.id, false);
            self.enter_scope(scope, rest, window, cx);
            return;
        }
        // 用户自己改了输入，就不再处于「↑ 翻历史」中。
        self.recall = None;
        self.query = value;
        self.result = None;
        self.actions_open = false;
        self.selected = 0;
        self.refresh(cx);
    }

    fn enter_scope(
        &mut self,
        scope: Scope,
        rest: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::trace::log(format!("enter scope {}", scope.id));
        self.scope = Some(ActiveScope { scope, filter: 0 });
        self.result = None;
        self.form = None;
        self.actions_open = false;
        self.selected = 0;
        self.recall = None;
        self.query = rest.clone();
        self.sync_placeholder(window, cx);
        self.input
            .update(cx, |input, cx| input.set_value(rest, window, cx));
        self.input.read(cx).focus_handle(cx).focus(window, cx);
        self.focus_form = true;
        self.refresh(cx);
    }

    fn exit_scope(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::trace::log("exit scope");
        self.scope = None;
        self.result = None;
        self.form = None;
        self.selected = 0;
        self.recall = None;
        self.cancel_auto_run();
        self.query.clear();
        self.focus_form = false;
        self.sync_placeholder(window, cx);
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        // 焦点可能在刚丢掉的表单里，回到搜索框。
        self.input.read(cx).focus_handle(cx).focus(window, cx);
        self.refresh(cx);
    }

    /// Tab：选中的是要输入内容的命令（`opens_scope`，如 flomo）就进入它自己的范围；否则进入选中项
    /// 所属插件的范围（如在首页选中「翻译」「剪贴板历史」，或某条剪贴板记录）。
    fn tab_scope(&self) -> Option<Scope> {
        let item = self.selected_item()?;
        match &item.opens_scope {
            Some(id) => self.engine.scope(id),
            None => self.engine.scope(provider_of(&item.id)?),
        }
    }

    /// 当前是命令范围、选中的就是这个命令自己的表单（范围 id 就是结果 id），且表单全空。
    fn own_form_is_blank(&self, cx: &App) -> bool {
        self.form.as_ref().is_some_and(|form| {
            self.scope_id() == Some(form.item_id.as_str()) && form.is_blank(cx)
        })
    }

    /// 范围内 ↑（`older = true`）/ ↓ 翻历史输入。返回是否处理了这次按键。
    fn step_recall(&mut self, older: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(scope_id) = self.scope_id() else {
            return false;
        };
        let next = match (&self.recall, older) {
            // 只在输入框为空时开始翻：已经打了字，↑ 还是普通的上移。
            (None, true) if self.query.is_empty() => {
                let list = self.engine.recall(scope_id);
                if list.is_empty() {
                    return false;
                }
                self.recall = Some((list, 0));
                Some(0)
            }
            (None, _) => return false,
            (Some((list, at)), true) => Some((at + 1).min(list.len() - 1)),
            (Some((_, 0)), false) => None,
            (Some((_, at)), false) => Some(at - 1),
        };
        let text = match (next, &mut self.recall) {
            (Some(at), Some((list, current))) => {
                *current = at;
                list[at].clone()
            }
            _ => {
                // 翻回最新之后再 ↓：回到空输入。
                self.recall = None;
                String::new()
            }
        };
        // 先改 query 再改输入框：输入框的 Change 事件看到值没变，不会把翻历史的状态清掉。
        self.query = text.clone();
        self.result = None;
        self.selected = 0;
        self.input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.refresh(cx);
        true
    }

    /// 后台查询；结果回来时若 revision 已过期则丢弃。
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        let revision = self.revision;
        let engine = self.engine.clone();
        let query = self.query.clone();
        let scope = self
            .scope
            .as_ref()
            .map(|active| (active.scope.id.clone(), active.filter_name().to_string()));
        let window_handle = self.window_handle;
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let items = cx
                .background_spawn(async move {
                    match scope {
                        Some((scope, filter)) => engine.query_scoped(&scope, &query, &filter),
                        None => engine.query(&query),
                    }
                })
                .await;
            let _ = window_handle.update(cx, |_, window, cx| this.update(cx, |this, cx| {
                if this.revision != revision {
                    return;
                }
                if this.result.as_ref().is_some_and(|result| result.pinned) {
                    // 正在展示列表外的结果（见 `ResultView::pinned`），不替换列表。
                    return;
                }
                crate::trace::log(format!(
                    "results query={:?} count={} top={:?}",
                    this.query,
                    items.len(),
                    items.first().map(|item| item.title.as_str())
                ));
                // 后台刷新时尽量保持选中同一个结果。
                let selected_id = this.items.get(this.selected).map(|item| item.id.clone());
                this.items = items;
                this.selected = selected_id
                    .and_then(|id| this.items.iter().position(|item| item.id == id))
                    .unwrap_or(0)
                    .min(this.items.len().saturating_sub(1));
                // 结果一到就建表单状态（不等下一帧渲染）：输入即译的防抖从这里开始计时。
                this.sync_form(window, cx);
                cx.notify();
            }));
        })
        .detach();
    }

    fn select(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        let last = self.items.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        self.actions_open = false;
        if let Some(&child) = self.row_children.get(self.selected) {
            self.scroll.scroll_to_item(child);
        }
        cx.notify();
    }

    fn selected_item(&self) -> Option<&Item> {
        self.items.get(self.selected)
    }

    fn layout(&self) -> Layout {
        match self.selected_item() {
            Some(item) if item.form.is_some() => Layout::Form,
            _ if self.result.is_some() || self.items.iter().any(|item| item.detail.is_some()) => {
                Layout::Split
            }
            _ => Layout::List,
        }
    }

    /// 选中项带表单时，为它建立输入框状态；换了选中项就重建。
    fn sync_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 表单重建（如 ↑ 调出历史、换了结果）前焦点在表单里：新表单接着拿焦点，否则焦点会落空。
        let had_focus = self
            .form
            .as_ref()
            .is_some_and(|form| form.focused(window, cx));
        let Some(item) = self.items.get(self.selected) else {
            self.drop_form(had_focus, window, cx);
            return;
        };
        let Some(form) = item.form.clone() else {
            self.drop_form(had_focus, window, cx);
            return;
        };
        if self
            .form
            .as_ref()
            .is_some_and(|state| state.item_id == item.id)
        {
            return;
        }
        let states: Vec<FieldState> = form
            .fields
            .iter()
            .map(|field| match &field.kind {
                FieldKind::Text {
                    placeholder,
                    multiline: true,
                } => {
                    let placeholder = placeholder.clone();
                    let value = field.value.clone();
                    FieldState::Area(cx.new(|cx| {
                        let mut area = TextareaState::new(window, cx).placeholder(placeholder);
                        area.set_value(value, window, cx);
                        area
                    }))
                }
                FieldKind::Text {
                    placeholder,
                    multiline: false,
                } => {
                    let placeholder = placeholder.clone();
                    let value = field.value.clone();
                    FieldState::Line(cx.new(|cx| {
                        let mut input = InputState::new(window, cx).placeholder(placeholder);
                        input.set_value(value, window, cx);
                        input
                    }))
                }
                FieldKind::Secret { placeholder } => {
                    let placeholder = placeholder.clone();
                    let value = field.value.clone();
                    FieldState::Line(cx.new(|cx| {
                        let mut input = InputState::new(window, cx)
                            .placeholder(placeholder)
                            .masked(true);
                        input.set_value(value, window, cx);
                        input
                    }))
                }
                FieldKind::Select { options } => FieldState::Select(
                    options
                        .iter()
                        .position(|(_, value)| *value == field.value)
                        .unwrap_or(0),
                ),
            })
            .collect();
        // 输入即译：多行文本框里改字也要防抖重跑（搜索框的变化走 on_query_changed → 表单重建）。
        let auto_run = item.auto_run;
        let item_id = item.id.clone();
        let subscriptions = if auto_run {
            states
                .iter()
                .filter_map(|state| match state {
                    FieldState::Area(area) => Some(cx.subscribe_in(
                        area,
                        window,
                        |this, _, event: &InputEvent, window, cx| {
                            if let InputEvent::Change = event {
                                // 在表单里改了字就不再处于「翻历史」状态，↑↓ 回到移动光标。
                                this.recall = None;
                                this.schedule_auto_run(window, cx);
                                cx.notify();
                            }
                        },
                    )),
                    FieldState::Line(_) | FieldState::Select(_) => None,
                })
                .collect()
        } else {
            Vec::new()
        };
        let own_form = self.scope_id() == Some(item_id.as_str());
        self.form = Some(FormState {
            item_id,
            form,
            states,
            _subscriptions: subscriptions,
        });
        // 输入即译的表单（翻译）、命令范围里这个命令自己的表单（flomo，范围 id 就是结果 id）：
        // 进入范围后文字直接打在表单的文本框里，不用再点一下。
        if (std::mem::take(&mut self.focus_form) && (auto_run || own_form)) || had_focus {
            self.focus_form_field(window, cx);
        }
        if auto_run {
            self.schedule_auto_run(window, cx);
        }
    }

    fn drop_form(&mut self, had_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.form = None;
        if had_focus {
            self.input.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    /// 把焦点放进表单的第一个文本框；没有文本框时留在搜索框。
    fn focus_form_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.form else {
            return;
        };
        for state in &form.states {
            match state {
                // 光标放到文末（顺带聚焦）：↑ 调出的历史接着往后打，而不是插到开头。
                FieldState::Area(area) => {
                    area.update(cx, |area, cx| {
                        let end = end_position(&area.value());
                        area.set_cursor_position(end, window, cx);
                    });
                    return;
                }
                FieldState::Line(input) => {
                    input.update(cx, |input, cx| {
                        let end = end_position(&input.value());
                        input.set_cursor_position(end, window, cx);
                    });
                    return;
                }
                FieldState::Select(_) => {}
            }
        }
    }

    /// 输入即译：（重新）开始防抖计时，到点后在表单里原地执行主操作。表单里没有字就只清掉旧结果。
    fn schedule_auto_run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_auto_run();
        let Some(item) = self.selected_item().filter(|item| item.auto_run) else {
            return;
        };
        let Some(form) = self.form.as_ref().filter(|form| form.item_id == item.id) else {
            return;
        };
        let item_id = item.id.clone();
        if self
            .result
            .as_ref()
            .is_some_and(|result| result.item_id == item_id)
        {
            self.result = None;
        }
        if !form.has_text(cx) {
            return;
        }
        crate::trace::log(format!("auto-run scheduled {item_id}"));
        self.auto_pending = true;
        let window_handle = window.window_handle();
        self.auto_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            cx.background_executor().timer(AUTO_RUN_DELAY).await;
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    if !this.auto_pending {
                        return;
                    }
                    this.auto_pending = false;
                    let Some(item) = this
                        .selected_item()
                        .filter(|item| item.id == item_id)
                        .cloned()
                    else {
                        return;
                    };
                    crate::trace::log(format!("auto-run {}", item.id));
                    let action = item.action.clone();
                    this.run(item, action, false, window, cx);
                });
            });
        }));
    }

    fn cancel_auto_run(&mut self) {
        self.auto_task = None;
        self.auto_pending = false;
    }

    /// 详情区该不该显示加载占位：选中的表单项正在（或即将）原地执行，还没有结果。
    fn loading(&self) -> bool {
        let Some(item) = self.selected_item() else {
            return false;
        };
        item.form.is_some()
            && (self.auto_pending || self.running)
            && !self
                .result
                .as_ref()
                .is_some_and(|result| result.item_id == item.id)
    }

    // ---------- 执行 ----------

    /// 直接执行一个项（菜单栏、全局快捷键）：与面板里回车同一条路径，
    /// 需要展示的结果（Message / Show / 出错）会打开面板显示。
    pub fn run_item(&mut self, item: Item, window: &mut Window, cx: &mut Context<Self>) {
        let action = item.action.clone();
        self.run(item, action, true, window, cx);
    }

    /// 展示一个外来的项并执行它（agent 的确认请求，见 src/mcp/ipc.rs）：与 `run_item` 相同，
    /// 只是不记进「最近使用」。
    pub fn present(&mut self, item: Item, window: &mut Window, cx: &mut Context<Self>) {
        let action = item.action.clone();
        self.run(item, action, false, window, cx);
    }

    pub fn execute_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.selected_item().cloned() else {
            return;
        };
        // 已经有结果（如译文、识别结果）时，回车执行结果的主操作（复制），而不是重新运行。
        let action = match self.result_actions().and_then(|actions| actions.first()) {
            Some(primary) => primary.action.clone(),
            None => item.action.clone(),
        };
        // 还在等防抖就按了回车：不等了，马上执行。
        self.cancel_auto_run();
        self.run(item, action, true, window, cx);
    }

    /// 选中项当前展示的结果自带的操作（第一个是回车的主操作）。
    fn result_actions(&self) -> Option<&[NamedAction]> {
        let result = self.result.as_ref()?;
        let item = self.selected_item()?;
        (result.item_id == item.id && !result.detail.actions.is_empty())
            .then_some(result.detail.actions.as_slice())
    }

    /// 执行一个动作。`record`：记进「最近使用」（输入即译的自动执行不记，否则每敲一句都是一条）。
    fn run(
        &mut self,
        item: Item,
        action: crate::provider::Action,
        record: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::trace::log(format!("execute {} ({})", item.title, item.id));
        self.actions_open = false;
        // 「重试」「再试一次」：重新执行这个结果的主操作（表单里带着当前表单值原地重跑）。
        let action = if action.is_rerun() {
            item.action.clone()
        } else {
            action
        };
        if record && !item.id.starts_with("recent:") {
            crate::history::record(&item, &self.query, self.scope_id());
        }

        // 只有提交表单本身（回车运行）才在原地显示结果；结果上的操作（复制、替换）要先隐藏面板，
        // 让后续的 ⌘V 等落到用户原来的 App。
        let in_form = action == item.action
            && self
                .form
                .as_ref()
                .is_some_and(|form| form.item_id == item.id);
        let input = match &self.form {
            Some(form) if in_form => form.values(cx),
            _ => ActionInput::default(),
        };
        // 表单视图在原地显示结果、标记了 keep_open 的动作原地刷新，都不隐藏面板；其他情况先隐藏再在后台执行。
        let keep_open = action.keeps_open();
        if in_form || keep_open {
            self.running = true;
            cx.notify();
        } else {
            self.hide(window, cx);
        }
        if in_form {
            self.form_run += 1;
        }
        let form_run = self.form_run;
        // 结果要展示在列表之外的项上（全局快捷键、菜单栏直接执行）。
        let foreign = !self.items.iter().any(|listed| listed.id == item.id);

        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let outcome = cx
                .background_spawn(async move { action.run_with(&input) })
                .await;
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    if in_form && form_run != this.form_run {
                        // 输入即译：这是被后来的输入取代的旧请求，结果作废。
                        return;
                    }
                    this.running = false;
                    let detail = match outcome {
                        Ok(Outcome::Done) if keep_open => {
                            this.refresh(cx);
                            return;
                        }
                        Ok(Outcome::Done) => {
                            this.executed = true;
                            if this.visible {
                                this.hide(window, cx);
                            }
                            return;
                        }
                        Ok(Outcome::EnterScope(scope)) => {
                            this.open_scope(&scope, window, cx);
                            return;
                        }
                        Ok(Outcome::EnterScopeWith(scope, text)) => {
                            if this.scope_id() == Some(scope.as_str()) {
                                this.exit_scope(window, cx);
                            }
                            this.open_scope_with(&scope, text, window, cx);
                            return;
                        }
                        Ok(Outcome::Dismiss) => {
                            this.result = None;
                            cx.notify();
                            return;
                        }
                        Ok(Outcome::OpenSettings) => {
                            this.executed = true;
                            // 推迟到本次更新结束：设置窗口会读引擎等全局状态，不能在启动器更新中途打开。
                            cx.defer(crate::settings::open);
                            return;
                        }
                        Ok(Outcome::OpenSettingsPage(page)) => {
                            this.executed = true;
                            cx.defer(move |cx| crate::settings::open_page(cx, page));
                            return;
                        }
                        Ok(Outcome::Show(detail)) => detail,
                        Ok(Outcome::Message(message)) => Detail {
                            title: item.title.clone(),
                            icon: item.icon.clone(),
                            blocks: vec![Block::Text(message)],
                            ..Detail::default()
                        },
                        Err(err) => Detail {
                            title: trf!("「{}」执行失败", "“{}” failed", item.title),
                            icon: item.icon.clone(),
                            blocks: vec![Block::Code(format!("{err:#}"))],
                            ..Detail::default()
                        },
                    };
                    if !this.visible {
                        this.show_inner(false, window, cx);
                    }
                    let pinned = foreign && !this.items.iter().any(|listed| listed.id == item.id);
                    if pinned {
                        this.items = vec![item.clone()];
                        this.selected = 0;
                    }
                    this.result = Some(ResultView {
                        item_id: item.id.clone(),
                        detail,
                        pinned,
                    });
                    // 看完结果再关掉面板，下次唤起就是新任务。
                    this.executed = true;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn menu_actions(&self) -> Vec<NamedAction> {
        let Some(item) = self.selected_item() else {
            return Vec::new();
        };
        let mut actions = match self.result_actions() {
            Some(result_actions) => {
                let mut actions = result_actions.to_vec();
                if let Some(primary) = actions.first_mut() {
                    primary.shortcut = Some("↵".into());
                }
                // 原命令仍可再跑一次。
                actions.push(NamedAction::new(
                    trf!(
                        "再次{}",
                        "{} Again",
                        if item.form.is_some() {
                            tr("运行", "Run")
                        } else {
                            item.action_title.as_str()
                        }
                    ),
                    None,
                    item.action.clone(),
                ));
                actions
            }
            None => vec![NamedAction::new(
                item.action_title.clone(),
                Some("↵"),
                item.action.clone(),
            )],
        };
        actions.extend(item.actions.iter().cloned());
        actions
    }

    fn run_named(&mut self, action: NamedAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.selected_item().cloned() else {
            return;
        };
        self.cancel_auto_run();
        self.run(item, action.action, true, window, cx);
    }

    // ---------- 按键 ----------

    /// 图形自检用：把一个功能键（Tab、Esc、方向键、⌘K 等）直接交给面板的按键处理。
    pub fn simulate_key(&mut self, keystroke: Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let event = KeyDownEvent {
            keystroke,
            is_held: false,
            prefer_character_input: false,
        };
        self.on_key_down(&event, window, cx);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // 在捕获阶段拦截：单行 Input 会吞掉 ↑↓，这里先于它处理。
        let keystroke = &event.keystroke;
        let mods = &keystroke.modifiers;
        let key = keystroke.key.as_str();
        crate::trace::log(format!(
            "key down {key:?} char={:?} modified={}",
            keystroke.key_char,
            mods.modified()
        ));

        if self.actions_open {
            let actions = self.menu_actions();
            match key {
                "up" => self.action_selected = self.action_selected.saturating_sub(1),
                "down" => {
                    self.action_selected =
                        (self.action_selected + 1).min(actions.len().saturating_sub(1))
                }
                "enter" => {
                    if let Some(action) = actions.get(self.action_selected).cloned() {
                        self.run_named(action, window, cx);
                    }
                }
                "escape" => self.actions_open = false,
                "k" if mods.platform => self.actions_open = false,
                _ => return,
            }
            cx.notify();
            cx.stop_propagation();
            return;
        }

        if key == "," && mods.platform && !mods.shift && !mods.alt && !mods.control {
            self.hide(window, cx);
            // 推迟到本次更新结束：设置窗口会读引擎等全局状态，不能在启动器更新中途打开。
            cx.defer(crate::settings::open);
            cx.stop_propagation();
            return;
        }

        if key == "k" && mods.platform && !mods.shift && !mods.alt && !mods.control {
            if self.selected_item().is_some() {
                self.actions_open = true;
                self.action_selected = 0;
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }

        let form_focused = self
            .form
            .as_ref()
            .is_some_and(|form| form.focused(window, cx));
        // ⌘C / ⌘X / ⌘V / ⌘A / ⌘Z 在有选中文字或焦点在表单里时属于文字编辑，不当作结果的快捷键。
        let editing = mods.platform
            && !mods.shift
            && !mods.alt
            && !mods.control
            && matches!(key, "c" | "x" | "v" | "a" | "z")
            && (form_focused || self.input.read(cx).is_copyable());

        // 次级操作自带的快捷键（如 ⌘↵、⌘⇧C），结果的操作优先。
        if mods.modified()
            && !editing
            && let Some(action) = self
                .result_actions()
                .unwrap_or_default()
                .iter()
                .chain(
                    self.selected_item()
                        .into_iter()
                        .flat_map(|item| item.actions.iter()),
                )
                .find(|action| {
                    action
                        .shortcut
                        .as_deref()
                        .is_some_and(|s| shortcut_matches(s, keystroke))
                })
                .cloned()
        {
            self.run_named(action, window, cx);
            cx.stop_propagation();
            return;
        }

        // 范围内输入框为空时 ↑ 调出历史输入（翻译过的原文），↓ 往回翻。
        // 焦点在空的翻译文本框里时也能翻（进入翻译后焦点默认就在那里）。
        let form_empty = !self.form.as_ref().is_some_and(|form| form.has_text(cx));
        if !mods.modified()
            && (!form_focused || form_empty || self.recall.is_some())
            && self.selected == 0
            && matches!(key, "up" | "down")
            && self.step_recall(key == "up", window, cx)
        {
            cx.stop_propagation();
            return;
        }

        match key {
            "up" if !mods.modified() && !form_focused => self.select(-1, cx),
            "down" if !mods.modified() && !form_focused => self.select(1, cx),
            // 多行文本框里 ⇧↵ 换行，↵ 提交。
            "enter" if mods.shift && form_focused => return,
            "enter" if !mods.platform && !mods.alt && !mods.control => {
                self.execute_selected(window, cx)
            }
            // 首页 / 全局搜索：Tab 进入选中项所属的扩展（如「翻译」「剪贴板历史」）或选中命令自己的范围
            // （flomo、gh 这类要输入内容的插件命令）。范围里选中的也是命令入口（如 MCP 范围里的工具）时同样进入。
            "tab"
                if !mods.modified()
                    && !form_focused
                    && (self.scope.is_none()
                        || self
                            .selected_item()
                            .is_some_and(|item| item.opens_scope.is_some())) =>
            {
                let Some(scope) = self.tab_scope() else {
                    return;
                };
                self.engine.will_enter(&scope.id, false);
                self.enter_scope(scope, String::new(), window, cx);
            }
            "tab" if !mods.modified() && !form_focused => {
                let Some(active) = self
                    .scope
                    .as_mut()
                    .filter(|active| active.scope.filters.len() > 1)
                else {
                    return;
                };
                active.filter = (active.filter + 1) % active.scope.filters.len();
                self.selected = 0;
                self.refresh(cx);
            }
            "escape" if !mods.modified() => {
                if let Some(result) = self.result.take() {
                    if result.pinned {
                        // 列表里只剩那一项，回到当前查询的正常结果。
                        self.refresh(cx);
                    }
                    cx.notify();
                } else if self.scope.is_some() {
                    self.exit_scope(window, cx);
                } else {
                    self.hide(window, cx);
                }
            }
            // 范围里删空了再按 ⌫ 退出范围。命令范围的焦点在命令自己的表单里（flomo）：表单也全空时同样退出
            // （翻译的文本框不算：那里 ⌫ 只是删字）。
            "backspace"
                if self.scope.is_some()
                    && self.query.is_empty()
                    && (!form_focused || self.own_form_is_blank(cx)) =>
            {
                self.exit_scope(window, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }
}

/// 文本末尾的（行, 列）位置，列按 UTF-16 计（与输入框的 Position 一致）。
fn end_position(text: &str) -> Position {
    let line = text.matches('\n').count();
    let last = text.rsplit('\n').next().unwrap_or_default();
    Position::new(
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(last.encode_utf16().count()).unwrap_or(u32::MAX),
    )
}

/// `⌘⇧C`、`⌘↵`、`⌥↵` 这类显示用快捷键是否与按下的键一致。
fn shortcut_matches(shortcut: &str, keystroke: &Keystroke) -> bool {
    let mut want = Modifiers::default();
    let mut key = String::new();
    for ch in shortcut.chars() {
        match ch {
            '⌘' => want.platform = true,
            '⇧' => want.shift = true,
            '⌥' => want.alt = true,
            '⌃' => want.control = true,
            '↵' => key = "enter".into(),
            '⌫' => key = "backspace".into(),
            '⇥' => key = "tab".into(),
            other => key.push(other.to_ascii_lowercase()),
        }
    }
    keystroke.modifiers == want && keystroke.key == key
}

/// 全局可达的启动器：菜单栏、全局快捷键、设置窗口通过它驱动面板。
pub struct LauncherHandle {
    pub window: AnyWindowHandle,
    pub launcher: Entity<Launcher>,
}

impl Global for LauncherHandle {}

impl LauncherHandle {
    /// 在面板窗口上下文里操作启动器；面板窗口不存在时打印原因。
    pub fn update(
        cx: &mut App,
        f: impl FnOnce(&mut Launcher, &mut Window, &mut Context<Launcher>),
    ) {
        let Some(handle) = cx.try_global::<LauncherHandle>() else {
            eprintln!("helmp: 启动器还没创建");
            return;
        };
        let (window, launcher) = (handle.window, handle.launcher.clone());
        if let Err(err) = window.update(cx, |_, window, cx| {
            launcher.update(cx, |launcher, cx| f(launcher, window, cx))
        }) {
            eprintln!("helmp: 启动器窗口不可用：{err:#}");
        }
    }
}

// ======================= 渲染 =======================

/// 结果 id（`<provider>:<…>`，「最近使用」再带 `recent:` 前缀）所属的插件。
fn provider_of(id: &str) -> Option<&str> {
    let id = id.strip_prefix(RECENT_ID_PREFIX).unwrap_or(id);
    id.split_once(':').map(|(provider, _)| provider)
}

/// 应用搜索插件的 id：它的索引没建完时，搜索框提示「正在建立应用索引…」。
const APPS_PROVIDER: &str = "apps";

/// 根视图（不在任何范围里）搜索框的占位文字。
fn root_placeholder() -> &'static str {
    tr(
        "搜索应用、Agent 会话、剪贴板、命令… · Tab 进入扩展",
        "Search apps, agent sessions, clipboard, and commands… · Tab to open an extension",
    )
}

/// 搜索框占位文字：范围里用范围自己的提示；首页在应用索引建好之前说明正在建索引
/// （此时搜不到应用是正常的，不说明的话用户会以为坏了）。
fn placeholder_for(scope_placeholder: Option<&str>, apps_indexed: bool) -> String {
    match scope_placeholder {
        Some(placeholder) => placeholder.to_string(),
        None if !apps_indexed => tr("正在建立应用索引…", "Indexing apps…").to_string(),
        None => root_placeholder().to_string(),
    }
}

/// 首页「最近使用」的结果（平铺成图标）。
fn is_recent(item: &Item) -> bool {
    item.id.starts_with(RECENT_ID_PREFIX)
}

fn kbd(text: impl Into<SharedString>, pal: &Palette, on_selection: bool) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(20.))
        .h(px(20.))
        .px(px(5.))
        .rounded(px(5.))
        .bg(if on_selection {
            pal.selection_fg.opacity(0.22)
        } else {
            pal.key_bg
        })
        .text_color(if on_selection {
            pal.selection_fg
        } else {
            pal.key_fg
        })
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

fn chip(
    text: impl Into<SharedString>,
    tone: crate::provider::Tone,
    pal: &Palette,
    on_selection: bool,
) -> Div {
    let (bg, fg) = if on_selection {
        (pal.selection_fg.opacity(0.24), pal.selection_fg)
    } else {
        style::tone_colors(tone, pal.dark)
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(20.))
        .px(px(8.))
        .rounded(px(10.))
        .bg(bg)
        .text_color(fg)
        .text_size(px(11.5))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .child(text.into())
}

pub fn icon_element(icon: &ItemIcon, size: Pixels, pal: &Palette) -> AnyElement {
    match icon {
        ItemIcon::None => div().flex_none().size(size).into_any_element(),
        ItemIcon::Image(path) => img(path.clone())
            .flex_none()
            .size(size)
            .rounded(size * 0.2)
            .into_any_element(),
        ItemIcon::File(path) => match crate::icons::png_for(path) {
            Some(png) => img(png).flex_none().size(size).into_any_element(),
            None => div()
                .flex_none()
                .size(size * 0.86)
                .m(size * 0.07)
                .rounded(size * 0.22)
                .bg(pal.key_bg)
                .into_any_element(),
        },
        ItemIcon::Symbol { name, tint } => div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(size)
            .rounded(size * 0.25)
            .bg(style::tint(*tint))
            .child(
                gpui_kit::component::Icon::empty()
                    .path(SharedString::from(format!("icons/{name}.svg")))
                    .text_color(gpui_kit::white())
                    .size(size * 0.58),
            )
            .into_any_element(),
        ItemIcon::Glyph(text) => div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(size)
            .text_size(size * 0.82)
            .line_height(size)
            .child(text.clone())
            .into_any_element(),
    }
}

fn brand_icon(size: Pixels) -> Img {
    static BYTES: &[u8] = include_bytes!("../resources/AppIcon-64.png");
    img(std::sync::Arc::new(Image::from_bytes(
        ImageFormat::Png,
        BYTES.to_vec(),
    )))
    .flex_none()
    .size(size)
}

/// 正在执行（输入即译的防抖 / 等模型）时详情区的加载占位：标题 + 几条骨架，而不是一片空白。
fn render_loading(item: &Item, pal: &Palette) -> Div {
    let bar = |width: f32| {
        Skeleton::new()
            .w(relative(width))
            .h(px(18.))
            .rounded(px(6.))
    };
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .flex_1()
        .min_w_0()
        .px(px(26.))
        .py(px(22.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(icon_element(&item.icon, px(40.), pal))
                .child(
                    div()
                        .text_size(px(15.))
                        .text_color(pal.muted)
                        .child(trf!("正在{}…", "{}…", item.action_title)),
                ),
        )
        .child(bar(0.92))
        .child(bar(0.78))
        .child(bar(0.55))
}

fn render_detail(
    detail: &Detail,
    pal: &Palette,
    result_text: Option<&Entity<TextareaState>>,
) -> Div {
    let mut pane = div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .flex_1()
        .min_w_0()
        .px(px(26.))
        .py(px(22.))
        .overflow_hidden();
    if !detail.title.is_empty() || detail.icon != ItemIcon::None {
        let mut heading = div().flex().flex_col().gap(px(4.)).min_w_0().child(
            div()
                .text_size(px(17.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(pal.fg)
                .child(detail.title.clone()),
        );
        if let Some((tag, tone)) = &detail.tag {
            heading = heading.child(div().flex().child(chip(tag.clone(), *tone, pal, false)));
        }
        pane = pane.child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(icon_element(&detail.icon, px(40.), pal))
                .child(heading),
        );
    }
    let has_result = detail
        .blocks
        .iter()
        .any(|block| matches!(block, Block::Result { .. }));
    for block in &detail.blocks {
        pane = pane.child(match block {
            Block::Text(text) => div()
                .text_size(px(15.))
                .line_height(px(24.))
                .text_color(pal.fg)
                .child(text.clone()),
            // 主结果用只读文本框：能用鼠标选中一部分复制，长文可滚动，换行原样保留。
            Block::Result { text, large } => {
                let (size, line) = if *large { (20., 30.) } else { (15., 23.) };
                let body = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(line * 3.))
                    .text_size(px(size))
                    .line_height(px(line))
                    .text_color(pal.fg);
                match result_text {
                    // 只读文本框自带 10 pt 的左右内边距：往外让出这么多，文字才和标题、说明左对齐。
                    Some(state) => body.mx(px(-10.)).child(
                        Textarea::new(state)
                            .appearance(false)
                            .readonly(true)
                            .h_full(),
                    ),
                    None => body.child(text.clone()),
                }
            }
            Block::Caption(text) => div()
                .text_size(px(12.))
                .text_color(pal.muted)
                .child(text.clone()),
            Block::Code(text) => div()
                .px(px(16.))
                .py(px(12.))
                .rounded(px(10.))
                .bg(pal.card)
                .font_family("Menlo")
                .text_size(px(13.))
                .line_height(px(20.))
                .text_color(pal.fg)
                .child(text.clone()),
            Block::Quote { label, text } => div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .px(px(14.))
                .py(px(12.))
                .rounded(px(10.))
                .bg(pal.card)
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(pal.muted)
                        .child(label.clone()),
                )
                .child(
                    div()
                        .text_color(pal.fg)
                        .line_height(px(21.))
                        .child(text.clone()),
                ),
            // 有可选中的结果文字时图只当参照，缩成矮条，把高度让给文字。
            Block::Image(path) => div()
                .flex()
                .flex_none()
                .justify_center()
                .rounded(px(12.))
                .bg(pal.control)
                .overflow_hidden()
                .child(
                    img(path.clone())
                        .max_h(px(if has_result { 96. } else { 220. }))
                        .max_w_full()
                        // 卡片的 overflow_hidden 只按矩形裁：铺满卡片的图（如色块）要自己圆角。
                        .rounded(px(12.))
                        .object_fit(ObjectFit::Contain),
                ),
        });
    }
    if !detail.metadata.is_empty() {
        // 标签列固定宽、值列占满：两列平分时标签列太宽，「Last Activity」反被挤成两行。
        let mut grid = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .pt(px(14.))
            .border_t_1()
            .border_color(pal.separator)
            .text_size(px(12.5));
        for (key, value) in &detail.metadata {
            grid = grid.child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(
                        div()
                            .w(px(96.))
                            .flex_none()
                            .text_color(pal.muted)
                            .child(key.clone()),
                    )
                    .child(div().flex_1().min_w_0().text_color(pal.fg).child(value.clone())),
            );
        }
        pane = pane.child(div().mt_auto().child(grid));
    }
    pane
}

impl Launcher {
    fn render_row(
        &self,
        ix: usize,
        item: &Item,
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = ix == self.selected;
        let mut accessories = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .ml_auto()
            .pl(px(8.));
        for accessory in &item.accessories {
            accessories = accessories.child(match accessory {
                Accessory::Text(text) => div()
                    .text_size(px(12.))
                    .whitespace_nowrap()
                    .text_color(if on { pal.selection_muted } else { pal.muted })
                    .child(text.clone())
                    .into_any_element(),
                Accessory::Tag { text, tone } => {
                    chip(text.clone(), *tone, pal, on).into_any_element()
                }
                Accessory::Keys(keys) => div()
                    .flex()
                    .gap(px(3.))
                    .children(keys.iter().map(|k| kbd(k.clone(), pal, on)))
                    .into_any_element(),
            });
        }
        let two_line = !item.subtitle.is_empty();
        div()
            .id(ElementId::Name(item.id.clone().into()))
            .flex()
            .items_center()
            .gap(px(10.))
            .min_h(px(if two_line { 48. } else { 44. }))
            // 结果列表是可滚动的纵向 flex：不禁止收缩的话，结果一多每行就被压扁到 min_h，
            // 行高和结果少时不一样，副标题也贴着下沿。
            .flex_none()
            .px(px(10.))
            .py(px(4.))
            .rounded(px(10.))
            .when(on, |row| row.bg(pal.selection).text_color(pal.selection_fg))
            .when(!on, |row| {
                row.text_color(pal.fg).hover(|row| row.bg(pal.key_bg))
            })
            .child(match &item.icon {
                // 蓝色底块和选中行的底色几乎同色，选中时图标只剩一个白色符号、看不出底块：描一圈白边。
                ItemIcon::Symbol {
                    tint: crate::provider::Tint::Blue,
                    ..
                } if on => div()
                    .flex_none()
                    .rounded(px(7.))
                    .shadow(vec![BoxShadow {
                        color: gpui_kit::white().opacity(0.6),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(1.),
                        inset: false,
                    }])
                    .child(icon_element(&item.icon, px(28.), pal))
                    .into_any_element(),
                _ => icon_element(&item.icon, px(28.), pal),
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(item.title.clone()),
                    )
                    .when(two_line, |col| {
                        col.child(
                            div()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(if on { pal.selection_muted } else { pal.muted })
                                .child(item.subtitle.clone()),
                        )
                    }),
            )
            .child(accessories)
            // 单击只选中（右侧看详情），双击才执行；剪贴板条目双击是「复制」而不是粘贴到前台 App。
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.selected = ix;
                this.result = None;
                cx.notify();
                if event.click_count() >= 2 {
                    this.double_click_selected(window, cx);
                }
            }))
            .into_any_element()
    }

    /// 双击：剪贴板条目执行「复制」（⌘C 那个次级操作），其他结果执行主操作。
    fn double_click_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let copy = self
            .selected_item()
            .filter(|item| provider_of(&item.id) == Some("clipboard"))
            .and_then(|item| {
                item.actions
                    .iter()
                    .find(|named| named.shortcut.as_deref() == Some("⌘C"))
                    .cloned()
            });
        match copy {
            Some(action) => self.run_named(action, window, cx),
            None => self.execute_selected(window, cx),
        }
    }

    fn render_tiles(
        &self,
        items: &[(usize, &Item)],
        pal: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_none()
            .gap(px(6.))
            .px(px(4.))
            .children(items.iter().map(|(ix, item)| {
                let ix = *ix;
                let on = ix == self.selected;
                div()
                    .id(ElementId::Name(item.id.clone().into()))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .w(px(104.))
                    .pt(px(8.))
                    .pb(px(6.))
                    .rounded(px(12.))
                    .text_size(px(12.))
                    .when(on, |tile| {
                        tile.bg(pal.selection).text_color(pal.selection_fg)
                    })
                    .when(!on, |tile| {
                        tile.text_color(pal.fg).hover(|tile| tile.bg(pal.key_bg))
                    })
                    .child(icon_element(&item.icon, px(44.), pal))
                    .child(div().max_w(px(96.)).truncate().child(item.title.clone()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.selected = ix;
                        this.execute_selected(window, cx);
                    }))
            }))
            .into_any_element()
    }

    fn render_list(&mut self, pal: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.items.is_empty() {
            let text = if self.query.trim().is_empty() && self.scope.is_none() {
                tr("输入以搜索", "Type to search")
            } else {
                tr("没有结果", "No results")
            };
            return div()
                .flex_1()
                .p(px(20.))
                .text_color(pal.muted)
                .child(text)
                .into_any_element();
        }
        let mut children: Vec<AnyElement> = Vec::new();
        let mut row_children = vec![0; self.items.len()];
        let mut last_badge: Option<&str> = None;
        let items: Vec<(usize, Item)> = self.items.iter().cloned().enumerate().collect();
        let mut i = 0;
        while i < items.len() {
            let (ix, item) = &items[i];
            if last_badge != Some(item.badge.as_str()) && !item.badge.is_empty() {
                children.push(
                    div()
                        .flex_none()
                        .px(px(12.))
                        .pt(px(10.))
                        .pb(px(4.))
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(pal.muted)
                        .child(if self.scope.is_some() {
                            // 范围内（如 Agent 会话）的小节标题带数量：「等你处理 · 2」。
                            let count = self.items.iter().filter(|i| i.badge == item.badge).count();
                            format!("{} · {count}", item.badge)
                        } else {
                            item.badge.clone()
                        })
                        .into_any_element(),
                );
            }
            last_badge = Some(item.badge.as_str());
            if is_recent(item) {
                // 「最近使用」平铺成图标。按 id 认而不按分组名认：分组名随语言变。
                let start = i;
                while i < items.len() && is_recent(&items[i].1) {
                    row_children[items[i].0] = children.len();
                    i += 1;
                }
                let group: Vec<(usize, &Item)> = items[start..i]
                    .iter()
                    .map(|(ix, item)| (*ix, item))
                    .collect();
                children.push(self.render_tiles(&group, pal, cx));
                continue;
            }
            row_children[*ix] = children.len();
            children.push(self.render_row(*ix, item, pal, cx));
            i += 1;
        }
        self.row_children = row_children;
        div()
            .id("results")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px(px(8.))
            .py(px(4.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(children)
            .into_any_element()
    }

    fn current_detail(&self) -> Option<Detail> {
        let item = self.selected_item();
        if let Some(result) = &self.result
            && item.is_none_or(|item| item.id == result.item_id)
        {
            return Some(result.detail.clone());
        }
        let item = item?;
        Some(item.detail.clone().unwrap_or_else(|| Detail {
            icon: item.icon.clone(),
            title: item.title.clone(),
            blocks: if item.subtitle.is_empty() {
                Vec::new()
            } else {
                vec![Block::Text(item.subtitle.clone())]
            },
            ..Detail::default()
        }))
    }

    /// 详情区：加载中显示占位，否则显示当前详情（主结果文字放进可选中的只读文本框）。
    fn render_current_detail(
        &mut self,
        pal: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if self.loading() {
            return self.selected_item().map(|item| render_loading(item, pal));
        }
        let detail = self.current_detail()?;
        let text = detail.blocks.iter().find_map(|block| match block {
            Block::Result { text, .. } => Some(text.clone()),
            _ => None,
        });
        let state = text.map(|text| self.result_text_state(text, window, cx));
        Some(render_detail(&detail, pal, state.as_ref()))
    }

    /// 主结果文字的只读文本框；文字变了才重建（保留用户的选区）。
    fn result_text_state(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextareaState> {
        if let Some((cached, state)) = &self.result_text
            && *cached == text
        {
            return state.clone();
        }
        let value = text.clone();
        let state = cx.new(|cx| {
            let mut area = TextareaState::new(window, cx);
            area.set_value(value, window, cx);
            area
        });
        self.result_text = Some((text, state.clone()));
        state
    }

    fn render_form(&mut self, pal: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.form else {
            return div().into_any_element();
        };
        let mut pane = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .w(SIDE_WIDTH)
            .flex_none()
            .px(px(20.))
            .py(px(18.))
            .border_r_1()
            .border_color(pal.separator);
        for (fx, (field, state)) in form.form.fields.iter().zip(&form.states).enumerate() {
            let label = div()
                .text_size(px(12.))
                .text_color(pal.muted)
                .child(field.label.clone());
            let control = match state {
                FieldState::Line(input) => div()
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(9.))
                    .bg(pal.card)
                    .child(Input::new(input).appearance(false))
                    .into_any_element(),
                FieldState::Area(area) => div()
                    .flex_1()
                    .min_h(px(120.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(12.))
                    .bg(pal.card)
                    .child(Textarea::new(area).appearance(false).h_full())
                    .into_any_element(),
                FieldState::Select(selected) => {
                    let options = match &field.kind {
                        FieldKind::Select { options } => options.clone(),
                        FieldKind::Text { .. } | FieldKind::Secret { .. } => Vec::new(),
                    };
                    let label = options
                        .get(*selected)
                        .map(|(label, _)| label.clone())
                        .unwrap_or_default();
                    div()
                        .id(SharedString::from(format!("field-{fx}")))
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(28.))
                        .px(px(10.))
                        .rounded(px(7.))
                        .bg(pal.control)
                        .border_1()
                        .border_color(pal.separator)
                        .text_color(pal.fg)
                        .child(label)
                        .child(div().text_color(pal.muted).child("⌄"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            // 点一下切到下一个选项（菜单式下拉留待后续）。
                            if let Some(form) = &mut this.form
                                && let Some(FieldState::Select(ix)) = form.states.get_mut(fx)
                            {
                                *ix = (*ix + 1) % options.len().max(1);
                                // 输入即译：换了目标语言 / 模型就按新选项重新翻译。
                                this.schedule_auto_run(window, cx);
                                cx.notify();
                            }
                        }))
                        .into_any_element()
                }
            };
            let grow = matches!(state, FieldState::Area(_));
            pane = pane.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .when(grow, |field| field.flex_1())
                    .child(label)
                    .child(control),
            );
        }
        pane.into_any_element()
    }

    fn render_actions_menu(&self, pal: &Palette) -> AnyElement {
        let Some(item) = self.selected_item() else {
            return div().into_any_element();
        };
        let mut menu = div()
            .absolute()
            .right(px(10.))
            .bottom(px(46.))
            .w(px(260.))
            .p(px(6.))
            .rounded(px(14.))
            .bg(pal.window)
            .border_1()
            .border_color(pal.separator)
            .shadow_lg()
            .child(
                div()
                    .px(px(10.))
                    .py(px(6.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(pal.muted)
                    .truncate()
                    .child(item.title.clone()),
            );
        for (ix, action) in self.menu_actions().into_iter().enumerate() {
            let on = ix == self.action_selected;
            menu = menu.child(
                div()
                    .flex()
                    .items_center()
                    .h(px(28.))
                    .px(px(10.))
                    .rounded(px(7.))
                    .when(on, |row| row.bg(pal.selection).text_color(pal.selection_fg))
                    .when(!on, |row| row.text_color(pal.fg))
                    .child(action.title.clone())
                    .when_some(action.shortcut.clone(), |row, shortcut| {
                        row.child(
                            div()
                                .ml_auto()
                                .text_size(px(12.))
                                .text_color(if on { pal.selection_muted } else { pal.muted })
                                .child(shortcut),
                        )
                    }),
            );
        }
        menu.into_any_element()
    }

    fn render_search_bar(&self, pal: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let mut bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.))
            .h(px(60.))
            .px(px(20.))
            .border_b_1()
            .border_color(pal.separator)
            .child(
                gpui_kit::component::Icon::empty()
                    .path("icons/search.svg")
                    .text_color(pal.muted)
                    .size(px(20.)),
            );
        if let Some(active) = &self.scope {
            let (bg, fg) = style::scope_colors(active.scope.tint, pal.dark);
            bar = bar.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(26.))
                    .px(px(10.))
                    .rounded(px(7.))
                    .bg(bg)
                    .text_color(fg)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(active.scope.label.clone()),
            );
        }
        bar = bar.child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(20.))
                .child(Input::new(&self.input).appearance(false)),
        );
        if let Some(active) = &self.scope
            && active.scope.filters.len() > 1
        {
            let mut seg = div()
                .flex()
                .p(px(2.))
                .rounded(px(8.))
                .bg(pal.key_bg)
                .text_size(px(12.));
            for (fx, name) in active.scope.filters.iter().enumerate() {
                let on = fx == active.filter;
                seg = seg.child(
                    div()
                        .id(SharedString::from(format!("filter-{fx}")))
                        .h(px(24.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .when(on, |b| {
                            b.bg(pal.control)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(pal.fg)
                                .shadow_sm()
                        })
                        .when(!on, |b| b.text_color(pal.key_fg))
                        .child(name.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(active) = &mut this.scope {
                                active.filter = fx;
                                this.selected = 0;
                                this.refresh(cx);
                            }
                        })),
                );
            }
            bar = bar.child(seg);
        }
        bar.into_any_element()
    }

    fn render_footer(&self, pal: &Palette) -> AnyElement {
        let mut context = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(brand_icon(px(16.)).rounded(px(4.)))
            .child("Helmp");
        if let Some(active) = &self.scope {
            context = context.child(
                div()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(pal.muted)
                    .child(format!("› {}", active.scope.label)),
            );
        }
        let hint = |label: SharedString, key: SharedString| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(label)
                .child(kbd(key, pal, false))
        };
        let mut footer = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(16.))
            .h(px(40.))
            .pl(px(16.))
            .pr(px(14.))
            .border_t_1()
            .border_color(pal.separator)
            .text_size(px(12.))
            .text_color(pal.key_fg)
            .child(context);
        if self.running {
            footer = footer.child(div().ml_auto().text_color(pal.muted).child(tr("正在运行…", "Running…")));
        } else if let Some(item) = self.selected_item() {
            let primary = match self.result_actions().and_then(|actions| actions.first()) {
                Some(action) => action.title.clone(),
                // 表单也用结果自己的动作名（保存、提交、翻译），比笼统的「运行」说得清回车会做什么。
                None if item.action_title.is_empty() => tr("运行", "Run").to_string(),
                None => item.action_title.clone(),
            };
            footer = footer.child(div().ml_auto().child(hint(primary.into(), "↵".into())));
            if !item.actions.is_empty() || self.result_actions().is_some() {
                footer = footer.child(hint(tr("操作", "Actions").into(), "⌘K".into()));
            }
        }
        footer.into_any_element()
    }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 输入框等 GPUI Kit 组件跟随明暗（只在模式真的变化时切换全局主题）。
        crate::settings::sync_component_theme(window, cx);
        self.sync_form(window, cx);
        let pal = style::palette(window);
        let layout = self.layout();

        let body = match layout {
            Layout::List => self.render_list(&pal, cx),
            Layout::Split => {
                let list = self.render_list(&pal, cx);
                let detail = self.render_current_detail(&pal, window, cx);
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w(SIDE_WIDTH)
                            .flex_none()
                            .border_r_1()
                            .border_color(pal.separator)
                            .child(list),
                    )
                    .children(detail)
                    .into_any_element()
            }
            Layout::Form => {
                let form = self.render_form(&pal, cx);
                let detail = self.render_current_detail(&pal, window, cx);
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(form)
                    .children(detail)
                    .into_any_element()
            }
        };

        div()
            .id("launcher")
            .relative()
            .capture_key_down(cx.listener(Self::on_key_down))
            .flex()
            .flex_col()
            .size_full()
            .rounded(px(crate::panel::CORNER_RADIUS as f32))
            .bg(pal.panel)
            .text_color(pal.fg)
            .text_size(px(13.))
            .overflow_hidden()
            .child(self.render_search_bar(&pal, cx))
            .child(body)
            .child(self.render_footer(&pal))
            .when(self.actions_open, |root| {
                root.child(self.render_actions_menu(&pal))
            })
    }
}

#[cfg(test)]
mod tests {
    // 不用 `super::*`：那会把 gpui 的 `test` 宏一起导进来，盖掉标准 `#[test]`。
    use std::time::Duration;

    use super::{end_position, placeholder_for, provider_of, should_reset};

    const LIMIT: Duration = Duration::from_secs(90);

    #[test]
    fn cursor_end_counts_lines_and_utf16_columns() {
        // ↑ 调出历史后光标要落在文末：列按 UTF-16 算错一位，中文或 emoji 后接着打的字就会插进中间。
        let end = end_position("第一行\nhi 👋");
        assert_eq!((end.line, end.character), (1, 5));
        let end = end_position("");
        assert_eq!((end.line, end.character), (0, 0));
        let end = end_position("末尾换行\n");
        assert_eq!((end.line, end.character), (1, 0));
    }

    #[test]
    fn keeps_query_when_reopened_quickly() {
        // 刚关掉又唤起，多半是想接着改上次的搜索，清掉会逼用户重打。
        assert!(!should_reset(Some(Duration::from_secs(5)), LIMIT, false));
    }

    #[test]
    fn clears_query_after_reset_window() {
        // 隔了一段时间再唤起是新意图，残留旧词会让第一屏结果全是无关内容。
        assert!(should_reset(Some(Duration::from_secs(90)), LIMIT, false));
        assert!(should_reset(Some(Duration::from_secs(600)), LIMIT, false));
    }

    #[test]
    fn clears_query_after_executing_a_result() {
        // 打开了微信就算任务完成，下次唤起不应还停在「wx」上。
        assert!(should_reset(Some(Duration::from_secs(1)), LIMIT, true));
    }

    #[test]
    fn zero_limit_always_clears() {
        // HELMP_RESET_AFTER_SECS=0 的语义是「每次都是新的」。
        assert!(should_reset(Some(Duration::ZERO), Duration::ZERO, false));
    }

    #[test]
    fn first_show_has_nothing_to_clear() {
        assert!(!should_reset(None, LIMIT, false));
    }

    #[test]
    fn placeholder_explains_the_current_state() {
        // 刚启动时搜不到应用是因为索引还没建完：不说明，用户会以为坏了。
        assert_eq!(placeholder_for(None, false), "正在建立应用索引…");
        // 首页提示能搜什么，以及 Tab 能进入扩展（否则这个功能没人知道）。
        let home = placeholder_for(None, true);
        assert!(home.starts_with("搜索应用、Agent 会话、剪贴板、命令…"), "{home}");
        assert!(home.contains("Tab 进入扩展"), "{home}");
        // 进入范围后提示这个范围能做什么，与索引状态无关。
        assert_eq!(
            placeholder_for(Some("输入要翻译的文字"), false),
            "输入要翻译的文字"
        );
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            assert_eq!(placeholder_for(None, false), "Indexing apps…");
            assert!(placeholder_for(None, true).contains("Tab to open an extension"));
        });
    }

    #[test]
    fn tab_finds_the_extension_of_the_selected_item() {
        // Tab 进入选中项所属的扩展：首页「最近使用」里的项也要认得出来源。
        assert_eq!(provider_of("translate:open"), Some("translate"));
        assert_eq!(provider_of("recent:clipboard:open"), Some("clipboard"));
        assert_eq!(provider_of("screenshot:ocr-file:/tmp/a:b.png"), Some("screenshot"));
        assert_eq!(provider_of("no-provider"), None);
    }
}
