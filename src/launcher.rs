//! 启动器面板视图：一个搜索框 + 结果区占位。
//! 行为：热键 toggle、Esc 隐藏、窗口失去 key 时隐藏。

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::*;

actions!(huan, [Dismiss]);

const CONTEXT: &str = "Launcher";

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", Dismiss, Some(CONTEXT))]);
}

pub struct Launcher {
    input: Entity<InputState>,
    query: SharedString,
    visible: bool,
    _subscriptions: Vec<Subscription>,
}

impl Launcher {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索应用、agent 会话…"));

        let subscriptions = vec![
            cx.subscribe_in(&input, window, {
                let input = input.clone();
                move |this, _, event: &InputEvent, _window, cx| {
                    if let InputEvent::Change = event {
                        this.query = input.read(cx).value();
                        cx.notify();
                    }
                }
            }),
            // PopUp 面板失去 key 即隐藏（Loungy 的做法）。
            cx.observe_window_activation(window, |this, window, cx| {
                if this.visible && !window.is_window_active() {
                    this.hide(cx);
                }
            }),
        ];

        Self {
            input,
            query: SharedString::default(),
            visible: false,
            _subscriptions: subscriptions,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn input_is_focused(&self, window: &Window, cx: &App) -> bool {
        self.input.read(cx).focus_handle(cx).is_focused(window)
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible {
            self.hide(cx);
        } else {
            self.show(window, cx);
        }
    }

    fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = true;
        // 骨架阶段用 NSApp 级 activate / hide；后续换成 objc2 对 NSPanel 直接 orderOut，
        // 保住 NonactivatingPanel 不抢前台 App 的特性（见 _research/launcher/04 §1）。
        cx.activate(true);
        window.activate_window();
        self.input.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn hide(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        cx.hide();
        cx.notify();
    }

    fn dismiss(&mut self, _: &Dismiss, _window: &mut Window, cx: &mut Context<Self>) {
        self.hide(cx);
    }
}

impl Render for Launcher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::dismiss))
            .size_full()
            .rounded_xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                div()
                    .p_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(Input::new(&self.input).appearance(false).large()),
            )
            .child(
                v_flex()
                    .flex_1()
                    .p_3()
                    .text_color(theme.muted_foreground)
                    .child(if self.query.is_empty() {
                        SharedString::from("输入以搜索")
                    } else {
                        format!("查询：{}（结果区待实现）", self.query).into()
                    }),
            )
    }
}
