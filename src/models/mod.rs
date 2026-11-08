//! 用户自带的模型（设计稿 11「设置 · 模型」）：Anthropic、OpenAI 兼容接口、Ollama。
//! 负责：调用模型、API Key 存取（钥匙串）、设置窗口里的「模型」页。
//! 尚未实现：由「翻译与模型」子任务填充。

use gpui_kit::*;

/// 设置窗口「模型」页的视图。设置窗口创建一次后复用。
pub fn settings_pane(_window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|_| Placeholder).into()
}

struct Placeholder;

impl Render for Placeholder {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().p_4().child("模型设置尚未实现")
    }
}
