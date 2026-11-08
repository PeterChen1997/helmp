//! 启动器面板：搜索框 + 结果列表。
//! 键盘：↑↓ 选择，⏎ 执行，Esc 隐藏；窗口失去 key 时隐藏。
//! 执行结果：先隐藏面板，后台跑 Action；返回 `Outcome::Message` 时重新打开面板展示文字。

use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::engine::Engine;
use crate::provider::{Item, Outcome};

const ROW_HEIGHT: Pixels = px(44.);
/// 隐藏超过这么久再唤起，就清空上次输入回到空白搜索（参考 Raycast「Pop to Root Search」默认 90 秒）。
/// `HELMP_RESET_AFTER_SECS` 可覆盖，0 表示每次唤起都清空。
const DEFAULT_RESET_AFTER: Duration = Duration::from_secs(90);

fn reset_after() -> Duration {
    std::env::var("HELMP_RESET_AFTER_SECS")
        .ok()
        .and_then(|secs| secs.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_RESET_AFTER)
}

/// 唤起时是否清空上次输入。
/// - 刚执行过一个结果（`executed`）：任务已完成，下次唤起是新任务，清空。
/// - 隐藏时长达到阈值：大概率是新的意图，清空。
/// - 否则保留上次输入（调用方会全选它，直接打字即替换），方便改一下再搜。
fn should_reset(hidden_for: Option<Duration>, reset_after: Duration, executed: bool) -> bool {
    executed || hidden_for.is_some_and(|hidden_for| hidden_for >= reset_after)
}

/// 面板可见时定期重查，让 agent 状态、剪贴板等后台变化反映到列表。
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

pub struct Launcher {
    engine: Engine,
    input: Entity<InputState>,
    query: String,
    items: Vec<Item>,
    selected: usize,
    /// 每次发起查询加一，后台结果回来时版本不对就丢弃。
    revision: u64,
    message: Option<String>,
    visible: bool,
    hidden_at: Option<Instant>,
    /// 上次唤起后执行过结果，下次唤起清空。
    executed: bool,
    input_source: crate::input_source::InputSourceSwitch,
    scroll: UniformListScrollHandle,
    _refresh: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Launcher {
    pub fn new(engine: Engine, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("搜索应用、agent 会话、剪贴板、脚本…")
        });

        let subscriptions = vec![
            cx.subscribe_in(&input, window, {
                let input = input.clone();
                move |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.query = input.read(cx).value().to_string();
                        crate::trace::log(format!("input change {:?}", this.query));
                        this.message = None;
                        this.selected = 0;
                        this.refresh(cx);
                    }
                    InputEvent::PressEnter { .. } => this.execute_selected(window, cx),
                    _ => {}
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                crate::trace::log(format!(
                    "window activation：active={}",
                    window.is_window_active()
                ));
                if this.visible && !window.is_window_active() {
                    this.hide(window, cx);
                }
            }),
        ];

        let refresh = cx.spawn(async move |this, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(REFRESH_INTERVAL).await;
                let Ok(()) = this.update(cx, |this, cx| {
                    if this.visible {
                        this.refresh(cx);
                    }
                }) else {
                    break;
                };
            }
        });

        Self {
            engine,
            input,
            query: String::new(),
            items: Vec::new(),
            selected: 0,
            revision: 0,
            message: None,
            visible: false,
            hidden_at: None,
            executed: false,
            input_source: Default::default(),
            scroll: UniformListScrollHandle::new(),
            _refresh: refresh,
            _subscriptions: subscriptions,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn input_is_focused(&self, window: &Window, cx: &App) -> bool {
        self.input.read(cx).focus_handle(cx).is_focused(window)
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// 自检用：等价于用户在输入框里敲入 `query`。
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_value(query.to_string(), window, cx)
        });
        self.query = query.to_string();
        self.selected = 0;
        self.refresh(cx);
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible {
            self.hide(window, cx);
        } else {
            self.show(window, cx);
        }
    }

    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::trace::log("launcher.show");
        self.visible = true;
        let hidden_for = self.hidden_at.take().map(|at| at.elapsed());
        if should_reset(
            hidden_for,
            reset_after(),
            std::mem::take(&mut self.executed),
        ) {
            crate::trace::log("reset query");
            self.input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.query.clear();
            self.selected = 0;
            self.message = None;
        } else if !self.query.is_empty() {
            self.input
                .update(cx, |input, cx| input.select_all(window, cx));
        }
        crate::panel::show(window, cx);
        self.input_source.on_show();
        self.input.read(cx).focus_handle(cx).focus(window, cx);
        self.refresh(cx);
        cx.notify();
    }

    pub fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::trace::log("launcher.hide");
        self.visible = false;
        self.hidden_at = Some(Instant::now());
        crate::panel::hide(window);
        self.input_source.on_hide();
        cx.notify();
    }

    /// 后台查询；结果回来时若 revision 已过期则丢弃。
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        let revision = self.revision;
        let engine = self.engine.clone();
        let query = self.query.clone();
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let items = cx
                .background_spawn(async move { engine.query(&query) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.revision != revision {
                    return;
                }
                // 后台刷新时尽量保持选中同一个结果。
                let selected_id = this.items.get(this.selected).map(|item| item.id.clone());
                crate::trace::log(format!(
                    "results query={:?} count={} top={:?}",
                    this.query,
                    items.len(),
                    items.first().map(|item| item.title.as_str())
                ));
                this.items = items;
                this.selected = selected_id
                    .and_then(|id| this.items.iter().position(|item| item.id == id))
                    .unwrap_or(0);
                cx.notify();
            });
        })
        .detach();
    }

    fn select(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        let last = self.items.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Center);
        cx.notify();
    }

    pub fn execute_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(self.selected).cloned() else {
            return;
        };
        crate::trace::log(format!("execute {} ({})", item.title, item.id));
        self.execute(item, window, cx);
    }

    fn execute(&mut self, item: Item, window: &mut Window, cx: &mut Context<Self>) {
        self.hide(window, cx);
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let action = item.action.clone();
            let outcome = cx.background_spawn(async move { action.run() }).await;
            let _ = this.update(cx, |this, _| this.executed = true);
            let message = match outcome {
                Ok(Outcome::Done) => None,
                Ok(Outcome::Message(message)) => Some(message),
                Err(err) => Some(format!("「{}」执行失败：{err:#}", item.title)),
            };
            crate::trace::log(format!("outcome message={message:?}"));
            let Some(message) = message else {
                return;
            };
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.message = Some(message);
                    this.show(window, cx);
                });
            });
        })
        .detach();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // 在捕获阶段拦截：单行 Input 会吞掉 ↑↓，这里先于它处理。
        let keystroke = &event.keystroke;
        crate::trace::log(format!(
            "key down {:?} char={:?} modified={}",
            keystroke.key,
            keystroke.key_char,
            keystroke.modifiers.modified()
        ));
        if keystroke.modifiers.modified() {
            return;
        }
        match keystroke.key.as_str() {
            "up" => self.select(-1, cx),
            "down" => self.select(1, cx),
            "escape" => self.hide(window, cx),
            "enter" => self.execute_selected(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_row(
        &self,
        ix: usize,
        item: &Item,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let selected = ix == self.selected;
        h_flex()
            .id(ElementId::Name(item.id.clone().into()))
            .h(ROW_HEIGHT)
            .px_3()
            .gap_3()
            .rounded_md()
            .when(selected, |row| row.bg(theme.list_active))
            .hover(|row| row.bg(theme.list_hover))
            .child(
                v_flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        div()
                            .truncate()
                            .text_color(theme.foreground)
                            .child(item.title.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(item.subtitle.clone()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(item.badge.clone()),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.selected = ix;
                this.execute_selected(window, cx);
            }))
    }
}

impl Render for Launcher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let count = self.items.len();

        let body = if let Some(message) = self.message.clone() {
            div()
                .id("message")
                .flex_1()
                .p_3()
                .overflow_y_scroll()
                .text_sm()
                .text_color(theme.foreground)
                .child(message)
                .into_any_element()
        } else if count == 0 {
            div()
                .flex_1()
                .p_3()
                .text_color(theme.muted_foreground)
                .child(if self.query.trim().is_empty() {
                    "输入以搜索"
                } else {
                    "没有结果"
                })
                .into_any_element()
        } else {
            uniform_list(
                "results",
                count,
                cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                    range
                        .map(|ix| {
                            let item = this.items[ix].clone();
                            this.render_row(ix, &item, cx)
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .flex_1()
            .p_2()
            .into_any_element()
        };

        v_flex()
            .id("launcher")
            .capture_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .rounded_xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .overflow_hidden()
            .child(
                div()
                    .p_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(Input::new(&self.input).appearance(false).large()),
            )
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    // 不用 `super::*`：那会把 gpui 的 `test` 宏一起导进来，盖掉标准 `#[test]`。
    use std::time::Duration;

    use super::should_reset;

    const LIMIT: Duration = Duration::from_secs(90);

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
}
