//! 识别结果浮窗：摆在选区旁边（留在屏幕可见区域内），先显示「识别中…」，识别完换成结果。
//!
//! 标题「识别到 N 行 · 语言」、二维码链接、语言名都复用面板详情的同一套函数（`providers::screenshot`）。
//! 非空的结果自动复制到剪贴板（和以前一样），窗口里写明「已自动复制」；什么都没识别到时明确说出来。
//! 回车复制并关闭，⌘T 带着文字进翻译，⌘O 打开二维码链接，Esc 关闭。文字可以选中一部分 ⌘C 复制。

use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::geometry::{self, Rect};
use super::{Shot, give_focus_back, log, native, offscreen, set_result, state};
use crate::i18n::{tr, trf};
use crate::provider::{Tint, Tone};
use crate::providers::screenshot::{self, ocr, pasteboard};
use crate::style::{self, Palette};

const SIZE: (f64, f64) = (420.0, 320.0);
/// 与选区的间距（pt）。
const GAP: f64 = 12.0;

enum Phase {
    Recognizing,
    Done {
        found: ocr::Recognition,
        copied: bool,
    },
    Failed(String),
}

pub struct ResultView {
    phase: Phase,
    text: Option<Entity<TextareaState>>,
    focus: FocusHandle,
    shot: Shot,
    languages: Vec<String>,
}

/// 结果窗口在屏幕上的位置（AppKit 全局坐标）。
fn frame(shot: &Shot) -> Rect {
    let origin = geometry::beside(shot.rect, SIZE, shot.visible, GAP);
    geometry::to_appkit(shot.screen, Rect::new(origin.x, origin.y, SIZE.0, SIZE.1))
}

/// 打开结果窗口并在后台识别 `shot`。已有的结果窗口被替换。
pub fn open(shot: Shot, cx: &mut App) {
    let frame = frame(&shot);
    let offscreen = offscreen();
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
        ..Default::default()
    };
    let mut view = None;
    let png = shot.png.clone();
    let opened = crate::settings::open_transparent_window(options, cx, |window, cx| {
        let entity = cx.new(|cx| ResultView::new(shot, window, cx));
        view = Some(entity.clone());
        entity
    });
    let (handle, view) = match (opened, view) {
        (Ok(handle), Some(view)) => (handle, view),
        (Err(err), _) => {
            super::fail(
                err.context(tr(
                    "打开识别结果窗口失败",
                    "Couldn't open the text recognition window",
                )),
                cx,
            );
            return;
        }
        (Ok(_), None) => return,
    };
    let _ = handle.update(cx, |_, window, _| {
        native::setup(window, native::Kind::Result, frame, offscreen);
        native::present(window, true, offscreen);
    });
    set_result(Some((handle, view.clone())), cx);
    log(cx, "recognizing".into());
    let languages = screenshot::languages();
    cx.spawn(async move |cx: &mut AsyncApp| {
        // 用户正等着看结果：高优先级执行，并且不让系统把 Helmp 当成后台 App 降速（App Nap）——
        // 截图界面刚关掉、识别结果窗口还没画出来的这一刻，Helmp 没有可见窗口，实测会被拖慢十几秒。
        let _awake = cx.background_executor().prevent_app_nap("识别截图中的文字");
        let found = cx
            .background_executor()
            .spawn_with_priority(
                Priority::High,
                async move { ocr::recognize(&png, &languages) },
            )
            .await;
        let _ = handle.update(cx, |_, window, cx| {
            view.update(cx, |view, cx| view.finish(found, window, cx));
        });
    })
    .detach();
}

impl ResultView {
    fn new(shot: Shot, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            phase: Phase::Recognizing,
            text: None,
            focus,
            shot,
            languages: screenshot::languages(),
        }
    }

    /// 识别完成：记下结果（面板 ⌘K 里的「复制识别结果」也能用）、非空就自动复制。
    fn finish(
        &mut self,
        found: anyhow::Result<ocr::Recognition>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.phase = match found {
            Ok(found) => {
                screenshot::remember(&found.text);
                let copied = !found.text.is_empty() && copy_text(&found.text, cx);
                let value = found.text.clone();
                self.text = Some(cx.new(|cx| {
                    let mut area = TextareaState::new(window, cx);
                    area.set_value(value, window, cx);
                    area
                }));
                cx.defer(move |cx| log(cx, format!("recognized copied={copied}")));
                Phase::Done { found, copied }
            }
            Err(err) => {
                eprintln!("helmp: 识别文字失败：{err:#}");
                Phase::Failed(format!("{err:#}"))
            }
        };
        cx.notify();
    }

    fn found(&self) -> Option<&ocr::Recognition> {
        match &self.phase {
            Phase::Done { found, .. } => Some(found),
            _ => None,
        }
    }

    fn title(&self) -> String {
        match &self.phase {
            Phase::Recognizing => tr("识别中…", "Recognizing…").to_string(),
            Phase::Done { found, .. } => screenshot::result_title(found),
            Phase::Failed(_) => tr("识别失败", "Recognition failed").to_string(),
        }
    }

    /// 自检用：标题和文字。
    pub fn summary(&self) -> String {
        let body = match &self.phase {
            Phase::Recognizing => String::new(),
            Phase::Done { found, copied } => {
                format!("{} | copied={copied}", found.text.replace('\n', "⏎"))
            }
            Phase::Failed(err) => err.clone(),
        };
        format!("{} | {body}", self.title())
    }

    fn copy_and_close(&mut self, cx: &mut Context<Self>) {
        if let Some(found) = self.found()
            && !found.text.is_empty()
        {
            let text = found.text.clone();
            copy_text(&text, cx);
        }
        cx.defer(close);
    }

    fn translate(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.found().map(|found| found.text.clone()) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        let pid = state(cx).return_to;
        cx.defer(move |cx| {
            set_result(None, cx);
            // 翻译面板收起时要把焦点还给截图前的 App，而不是此刻在前台的 Helmp 自己。
            crate::panel::hand_over_previous_app(pid);
            crate::launcher::LauncherHandle::update(cx, move |launcher, window, cx| {
                launcher.open_scope_with(screenshot::TRANSLATE_SCOPE, text, window, cx)
            });
        });
    }

    fn open_link(&mut self, cx: &mut Context<Self>) {
        let Some(url) = self.found().and_then(|found| {
            screenshot::links(&found.codes)
                .first()
                .map(|url| url.to_string())
        }) else {
            return;
        };
        if crate::effects::dry_run() {
            cx.defer(move |cx| log(cx, format!("[dry-run] open {url}")));
        } else {
            cx.open_url(&url);
        }
        cx.defer(close);
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let m = &keystroke.modifiers;
        let plain = !m.platform && !m.control && !m.alt && !m.shift;
        match keystroke.key.as_str() {
            "escape" => cx.defer(close),
            "enter" if plain => self.copy_and_close(cx),
            "t" if m.platform => self.translate(cx),
            "o" if m.platform => self.open_link(cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

/// 写剪贴板（演练模式只记日志）。返回是否写成了。
fn copy_text(text: &str, cx: &mut App) -> bool {
    if crate::effects::dry_run() {
        let line = format!("[dry-run] copy text {}", text.replace('\n', "⏎"));
        cx.defer(move |cx| log(cx, line));
        return true;
    }
    match pasteboard::write_text(&pasteboard::general(), text) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("helmp: 复制识别结果失败：{err:#}");
            false
        }
    }
}

/// 关掉结果窗口，把焦点还给截图前的 App。
fn close(cx: &mut App) {
    set_result(None, cx);
    give_focus_back(cx);
}

fn button(
    id: &'static str,
    label: String,
    keys: &'static str,
    primary: bool,
    pal: &Palette,
    on_press: impl Fn(&mut App) + 'static,
) -> Stateful<Div> {
    let (bg, fg) = if primary {
        (pal.accent, white())
    } else {
        (pal.key_bg, pal.fg)
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(28.))
        .px(px(12.))
        .rounded(px(7.))
        .bg(bg)
        .text_color(fg)
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .cursor(CursorStyle::PointingHand)
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            on_press(cx);
        })
        .child(label)
        .child(div().text_size(px(11.)).opacity(0.7).child(keys))
}

impl Render for ResultView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = style::palette(window);
        let entity = cx.entity();
        let title = self.title();
        let copied = matches!(self.phase, Phase::Done { copied: true, .. });
        let has_text = self.found().is_some_and(|found| !found.text.is_empty());
        let link = self.found().and_then(|found| {
            screenshot::links(&found.codes)
                .first()
                .map(|url| url.to_string())
        });

        let header = div()
            .id("capture-result-header")
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .pt(px(14.))
            .pb(px(8.))
            // 无边框窗口：按住标题栏拖动窗口。
            .on_mouse_down(MouseButton::Left, |_, window, _| {
                if !offscreen() {
                    window.start_window_move();
                }
            })
            .child(crate::launcher::icon_element(
                &crate::provider::Icon::Symbol {
                    name: "scan-text",
                    tint: Tint::Indigo,
                },
                px(26.),
                &pal,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(pal.fg)
                    .child(title),
            )
            .when(copied, |header| {
                let (bg, fg) = style::tone_colors(Tone::Done, pal.dark);
                header.child(
                    div()
                        .flex_none()
                        .px(px(8.))
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .rounded(px(10.))
                        .bg(bg)
                        .text_color(fg)
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(tr("已复制", "Copied")),
                )
            });

        let body = match (&self.phase, &self.text) {
            (Phase::Done { found, .. }, Some(area)) if !found.text.is_empty() => div()
                .flex_1()
                .min_h_0()
                .px(px(6.))
                .text_size(px(14.))
                .line_height(px(21.))
                .text_color(pal.fg)
                .child(
                    Textarea::new(area)
                        .appearance(false)
                        .readonly(true)
                        .h_full(),
                ),
            (Phase::Done { found, .. }, _) => {
                let text = if found.codes.is_empty() {
                    tr(
                        "这块区域里没有可识别的文字。可以把选区框得更紧一些再试；日文、韩文等要到设置 › 截图里打开对应的识别语言。",
                        "There’s no readable text in this area. Try a tighter selection; for Japanese, Korean and other languages, turn them on in Settings › Screenshot.",
                    )
                    .to_string()
                } else {
                    found.codes.join("\n")
                };
                div()
                    .flex_1()
                    .px(px(16.))
                    .text_size(px(14.))
                    .line_height(px(21.))
                    .text_color(pal.fg)
                    .child(text)
            }
            (Phase::Recognizing, _) => div()
                .flex_1()
                .px(px(16.))
                .text_size(px(14.))
                .text_color(pal.muted)
                .child(tr(
                    "正在识别选区里的文字…",
                    "Recognizing the text in your selection…",
                )),
            (Phase::Failed(err), _) => div()
                .flex_1()
                .px(px(16.))
                .text_size(px(13.))
                .text_color(pal.danger)
                .child(err.clone()),
        };

        let mut caption = screenshot::language_names(&self.languages);
        caption.push_str(&format!(
            " · {} × {} px",
            self.shot.pixels.0, self.shot.pixels.1
        ));
        if copied {
            caption = trf!(
                "已自动复制到剪贴板 · {caption}",
                "Copied to the clipboard automatically · {caption}"
            );
        }

        let mut footer = div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(8.))
            .px(px(14.))
            .pb(px(14.))
            .pt(px(8.));
        if has_text {
            let copy = entity.clone();
            footer = footer.child(button(
                "capture-result-copy",
                tr("复制", "Copy").to_string(),
                "↵",
                true,
                &pal,
                move |cx| copy.update(cx, |view, cx| view.copy_and_close(cx)),
            ));
            let translate = entity.clone();
            footer = footer.child(button(
                "capture-result-translate",
                tr("翻译", "Translate").to_string(),
                "⌘T",
                false,
                &pal,
                move |cx| translate.update(cx, |view, cx| view.translate(cx)),
            ));
        }
        if link.is_some() {
            let open = entity.clone();
            footer = footer.child(button(
                "capture-result-link",
                tr("打开链接", "Open Link").to_string(),
                "⌘O",
                !has_text,
                &pal,
                move |cx| open.update(cx, |view, cx| view.open_link(cx)),
            ));
        }
        footer = footer.child(button(
            "capture-result-close",
            tr("关闭", "Close").to_string(),
            "esc",
            false,
            &pal,
            |cx| cx.defer(close),
        ));

        div()
            .id("capture-result")
            .size_full()
            .flex()
            .flex_col()
            .rounded(px(12.))
            .overflow_hidden()
            .bg(pal.window)
            .border_1()
            .border_color(pal.separator)
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::on_key))
            .child(header)
            .child(body)
            .child(
                div()
                    .px(px(16.))
                    .pt(px(6.))
                    .text_size(px(11.5))
                    .text_color(pal.muted)
                    .truncate()
                    .child(caption),
            )
            .child(footer)
    }
}
