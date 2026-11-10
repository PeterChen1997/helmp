use gpui_kit::*;
pub struct McpPane;
pub fn new(_window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|_| McpPane).into()
}
impl Render for McpPane {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
