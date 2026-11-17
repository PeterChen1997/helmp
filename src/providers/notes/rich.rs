//! 便签正文的所见即所得编辑器：嵌在便签窗口里的原生 `NSTextView`。
//!
//! - 正文按 Markdown 原文存；样式由 [`markdown::parse`] 切片后直接上到 `NSTextStorage`。
//! - 光标所在行露出 `#`、`**`、`- [ ]` 等标记（淡色），其余行把标记藏起来：标题变大、粗体加粗、
//!   复选框画成勾选框、列表符画成圆点，可以直接删改格式。
//! - 中文输入法由 `NSTextView` 原生处理；组字期间（有 marked text）不重新上样式，免得打断组字。
//! - 用 TextKit 1（`initUsingTextLayoutManager: NO`），勾选框要按字形位置画。

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSTextInputClient as _;
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezierPath, NSColor,
    NSEvent, NSFont, NSFontWeightBold, NSFontWeightRegular, NSFontWeightSemibold,
    NSMutableParagraphStyle, NSScrollView, NSText, NSTextDelegate, NSTextView, NSTextViewDelegate,
    NSView,
};
use objc2_foundation::{
    NSDictionary, NSMutableAttributedString, NSNotification, NSNumber, NSObjectProtocol, NSPoint,
    NSRange, NSRect, NSSize, NSString,
};

use super::markdown::{self, Kind, Span};

/// 原生编辑器发回给便签窗口的事件（经 channel 回到 GPUI 主循环处理，不在 AppKit 回调里碰 `App`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorEvent {
    /// 正文变了（打字、勾选复选框、粘贴、撤销）。
    Changed(String),
    /// 没在编辑时点了正文：进入编辑（光标已落在点的位置）。
    BeginEdit,
    /// 编辑中按了 Esc（组字时的 Esc 由输入法自己吃掉，不会到这里）。
    Escape,
    /// 点了链接（只会是 http / https）。
    OpenUrl(String),
}

const BODY_SIZE: f64 = 13.0;
/// 藏起来的标记用的字号：仍在原文里、光标能走过去，只是看不见也几乎不占宽度。
const HIDDEN_SIZE: f64 = 0.1;

/// 编辑器用到的颜色（跟随 Helmp 的亮 / 暗色板，sRGB 0..1）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    pub fg: [f64; 4],
    pub muted: [f64; 4],
    pub accent: [f64; 4],
    pub code_bg: [f64; 4],
}

fn ns_color(c: [f64; 4]) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(c[0], c[1], c[2], c[3])
}

pub struct Ivars {
    tx: async_channel::Sender<EditorEvent>,
    editing: Cell<bool>,
    restyling: Cell<bool>,
    /// 当前光标所在的行（选区跨行时是多行）；不在编辑时为空 = 全部渲染。
    caret_lines: RefCell<Vec<usize>>,
    spans: RefCell<Vec<Span>>,
    colors: Cell<Colors>,
}

define_class!(
    // SAFETY: NSTextView 子类化没有额外要求；本类不实现 Drop。
    #[unsafe(super(NSTextView, NSText, NSView, objc2_app_kit::NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "HelmpNoteTextView"]
    #[ivars = Ivars]
    pub struct NoteTextView;

    unsafe impl NSObjectProtocol for NoteTextView {}

    unsafe impl NSTextDelegate for NoteTextView {
        #[unsafe(method(textDidChange:))]
        fn text_did_change(&self, _notification: &NSNotification) {
            if self.ivars().restyling.get() {
                return;
            }
            self.update_caret_lines();
            self.restyle();
            let _ = self.ivars().tx.try_send(EditorEvent::Changed(self.text()));
        }
    }

    unsafe impl NSTextViewDelegate for NoteTextView {
        #[unsafe(method(textViewDidChangeSelection:))]
        fn selection_did_change(&self, _notification: &NSNotification) {
            if self.ivars().restyling.get() {
                return;
            }
            if self.update_caret_lines() {
                self.restyle();
            }
        }

        #[unsafe(method(textView:doCommandBySelector:))]
        fn do_command(&self, _view: &NSTextView, selector: Sel) -> Bool {
            if selector == sel!(cancelOperation:) {
                let _ = self.ivars().tx.try_send(EditorEvent::Escape);
                return Bool::YES;
            }
            Bool::NO
        }
    }

    impl NoteTextView {
        // 便签平时是不激活的浮窗：第一下点击就要落光标 / 勾选，不能被吞掉用来激活窗口。
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            if let Some(span) = self.rendered_span_at(point) {
                match span.kind {
                    Kind::TaskBox { .. } => {
                        self.toggle_task(&span);
                        return;
                    }
                    Kind::Link(url) if url.starts_with("https://") || url.starts_with("http://") => {
                        let _ = self.ivars().tx.try_send(EditorEvent::OpenUrl(url));
                        return;
                    }
                    _ => {}
                }
            }
            if !self.ivars().editing.get() {
                let _ = self.ivars().tx.try_send(EditorEvent::BeginEdit);
            }
            // SAFETY: 交给 NSTextView 落光标 / 选字。
            unsafe {
                let _: () = msg_send![super(self), mouseDown: event];
            }
        }

        #[unsafe(method(drawViewBackgroundInRect:))]
        fn draw_view_background(&self, rect: NSRect) {
            // SAFETY: 先让 NSTextView 画自己的背景（本编辑器背景透明）。
            unsafe {
                let _: () = msg_send![super(self), drawViewBackgroundInRect: rect];
            }
            self.draw_decorations();
        }
    }
);

impl NoteTextView {
    fn new(mtm: MainThreadMarker, tx: async_channel::Sender<EditorEvent>, colors: Colors) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            tx,
            editing: Cell::new(false),
            restyling: Cell::new(false),
            caret_lines: RefCell::new(Vec::new()),
            spans: RefCell::new(Vec::new()),
            colors: Cell::new(colors),
        });
        // SAFETY: NSTextView 的指定初始化方法；NO = 用 TextKit 1（勾选框要按字形位置画）。
        unsafe { msg_send![super(this), initUsingTextLayoutManager: false] }
    }

    pub fn text(&self) -> String {
        self.string().to_string()
    }

    /// 选区所在行变了就返回 true。
    fn update_caret_lines(&self) -> bool {
        let lines = if self.ivars().editing.get() {
            let sel = self.selectedRange();
            let ranges = markdown::line_ranges(&self.text());
            let (from, to) = (sel.location, sel.location + sel.length);
            ranges
                .iter()
                .enumerate()
                .filter(|(_, r)| r.start <= to && from <= r.end)
                .map(|(i, _)| i)
                .collect()
        } else {
            Vec::new()
        };
        let mut cur = self.ivars().caret_lines.borrow_mut();
        if *cur == lines {
            return false;
        }
        *cur = lines;
        true
    }

    fn rendered(&self, span: &Span) -> bool {
        !self.ivars().caret_lines.borrow().contains(&span.line)
    }

    /// 点击位置上「已渲染」（不在光标行）的复选框或链接。
    fn rendered_span_at(&self, point: NSPoint) -> Option<Span> {
        // SAFETY: 本视图用 TextKit 1 初始化，layoutManager / textContainer 一定在。
        let (layout, container) = unsafe { (self.layoutManager()?, self.textContainer()?) };
        let origin = self.textContainerOrigin();
        let p = NSPoint::new(point.x - origin.x, point.y - origin.y);
        let spans = self.ivars().spans.borrow();
        spans
            .iter()
            .filter(|s| matches!(s.kind, Kind::TaskBox { .. } | Kind::Link(_)) && self.rendered(s))
            .find(|s| {
                let rect = self.span_rect(&layout, &container, s);
                let pad = if matches!(s.kind, Kind::TaskBox { .. }) { 3.0 } else { 0.0 };
                p.x >= rect.origin.x - pad
                    && p.x <= rect.origin.x + rect.size.width + pad
                    && p.y >= rect.origin.y - pad
                    && p.y <= rect.origin.y + rect.size.height + pad
            })
            .cloned()
    }

    fn span_rect(
        &self,
        layout: &objc2_app_kit::NSLayoutManager,
        container: &objc2_app_kit::NSTextContainer,
        span: &Span,
    ) -> NSRect {
        let chars = NSRange::new(span.range.start, span.range.end - span.range.start);
        // SAFETY: actualCharacterRange 传空指针是允许的。
        let glyphs =
            unsafe { layout.glyphRangeForCharacterRange_actualCharacterRange(chars, std::ptr::null_mut()) };
        layout.boundingRectForGlyphRange_inTextContainer(glyphs, container)
    }

    /// 勾选 / 取消：只改 `[ ]` 中间那个字符，走 shouldChangeText → didChangeText，能撤销、会存盘。
    fn toggle_task(&self, span: &Span) {
        let Some((at, with)) = markdown::toggle_task(span) else {
            return;
        };
        let range = NSRange::new(at, 1);
        let replacement = NSString::from_str(with);
        if !self.shouldChangeTextInRange_replacementString(range, Some(&replacement)) {
            return;
        }
        // SAFETY: 本视图自带 textStorage。
        if let Some(storage) = unsafe { self.textStorage() } {
            storage.replaceCharactersInRange_withString(range, &replacement);
        }
        self.didChangeText();
    }

    /// 按当前正文和光标行重新上样式；组字中跳过（改属性会打断输入法）。
    pub fn restyle(&self) {
        if self.hasMarkedText() {
            return;
        }
        // SAFETY: 本视图自带 textStorage。
        let Some(storage) = (unsafe { self.textStorage() }) else {
            return;
        };
        let text = self.text();
        let spans = markdown::parse(&text);
        let colors = self.ivars().colors.get();
        let len = text.encode_utf16().count();
        let full = NSRange::new(0, len);

        self.ivars().restyling.set(true);
        storage.beginEditing();
        let base = base_attributes(colors);
        // SAFETY: base 是合法的属性字典。
        unsafe { storage.setAttributes_range(Some(&base), full) };
        for span in &spans {
            let range = NSRange::new(span.range.start, span.range.end - span.range.start);
            if range.location + range.length > len {
                continue;
            }
            apply_span(&storage, span, range, self.rendered(span), colors);
        }
        storage.endEditing();
        // SAFETY: 打字 / 组字一律用正文样式，不沿用前一个字（可能是藏起来的标记）的属性。
        unsafe { self.setTypingAttributes(&base) };
        self.ivars().restyling.set(false);
        *self.ivars().spans.borrow_mut() = spans;
        self.setNeedsDisplay(true);
    }

    /// 已渲染行上的复选框和列表圆点（字符本身是透明的，这里按字形位置画出来）。
    fn draw_decorations(&self) {
        // SAFETY: 本视图用 TextKit 1 初始化。
        let Some((layout, container)) = (unsafe { self.layoutManager().zip(self.textContainer()) }) else {
            return;
        };
        let origin = self.textContainerOrigin();
        let colors = self.ivars().colors.get();
        for span in self.ivars().spans.borrow().iter().filter(|s| self.rendered(s)) {
            let rect = self.span_rect(&layout, &container, span);
            let cy = origin.y + rect.origin.y + rect.size.height / 2.0;
            let x = origin.x + rect.origin.x;
            match span.kind {
                Kind::TaskBox { checked } => {
                    let size = 12.0;
                    let box_rect = NSRect::new(NSPoint::new(x + 1.0, cy - size / 2.0), NSSize::new(size, size));
                    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(box_rect, 3.0, 3.0);
                    if checked {
                        ns_color(colors.accent).setFill();
                        path.fill();
                        let check = NSBezierPath::bezierPath();
                        check.moveToPoint(NSPoint::new(x + 4.0, cy));
                        check.lineToPoint(NSPoint::new(x + 6.3, cy + 2.6));
                        check.lineToPoint(NSPoint::new(x + 10.2, cy - 2.8));
                        check.setLineWidth(1.6);
                        NSColor::whiteColor().setStroke();
                        check.stroke();
                    } else {
                        path.setLineWidth(1.2);
                        ns_color(colors.muted).setStroke();
                        path.stroke();
                    }
                }
                Kind::Bullet => {
                    let d = 4.5;
                    let dot = NSRect::new(NSPoint::new(x + 1.0, cy - d / 2.0), NSSize::new(d, d));
                    ns_color(colors.fg).setFill();
                    NSBezierPath::bezierPathWithOvalInRect(dot).fill();
                }
                _ => {}
            }
        }
    }
}

fn system_font(size: f64, weight: f64) -> Retained<NSFont> {
    // SAFETY: 权重常量来自 AppKit。
    NSFont::systemFontOfSize_weight(size, weight)
}

fn base_attributes(colors: Colors) -> Retained<NSDictionary<objc2_foundation::NSAttributedStringKey, AnyObject>> {
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setLineSpacing(3.0);
    // SAFETY: AppKit 导出的属性键常量。
    let keys = unsafe {
        [
            objc2_app_kit::NSFontAttributeName,
            objc2_app_kit::NSForegroundColorAttributeName,
            objc2_app_kit::NSParagraphStyleAttributeName,
        ]
    };
    let font = system_font(BODY_SIZE, unsafe { NSFontWeightRegular });
    let color = ns_color(colors.fg);
    let objects: [&AnyObject; 3] = [&font, &color, &paragraph];
    NSDictionary::from_slices(&keys, &objects)
}

fn apply_span(
    storage: &NSMutableAttributedString,
    span: &Span,
    range: NSRange,
    rendered: bool,
    colors: Colors,
) {
    // SAFETY: 下面用到的都是 AppKit 导出的属性键常量，值的类型与键匹配。
    unsafe {
        use objc2_app_kit::{
            NSBackgroundColorAttributeName as BG, NSFontAttributeName as FONT,
            NSForegroundColorAttributeName as FG, NSStrikethroughStyleAttributeName as STRIKE,
            NSUnderlineStyleAttributeName as UNDERLINE,
        };
        let hide = |storage: &NSMutableAttributedString| {
            storage.addAttribute_value_range(FONT, &system_font(HIDDEN_SIZE, NSFontWeightRegular), range);
            storage.addAttribute_value_range(FG, &NSColor::clearColor(), range);
        };
        let muted = ns_color(colors.muted);
        match &span.kind {
            Kind::Marker => {
                if rendered {
                    hide(storage);
                } else {
                    storage.addAttribute_value_range(FG, &muted, range);
                }
            }
            Kind::TaskBox { .. } | Kind::Bullet => {
                // 渲染时字符透明、保留宽度，由 draw_decorations 在原位画勾选框 / 圆点。
                let color = if rendered { NSColor::clearColor() } else { muted };
                storage.addAttribute_value_range(FG, &color, range);
            }
            Kind::Heading(level) => {
                let size = match level {
                    1 => 19.0,
                    2 => 16.5,
                    3 => 14.5,
                    _ => BODY_SIZE + 0.5,
                };
                storage.addAttribute_value_range(FONT, &system_font(size, NSFontWeightBold), range);
            }
            Kind::Bold => {
                storage.addAttribute_value_range(FONT, &system_font(BODY_SIZE, NSFontWeightSemibold), range);
            }
            Kind::Italic => {
                // 中文字体没有斜体字形，用倾斜度模拟（TextKit 1 支持）。
                #[allow(deprecated)]
                storage.addAttribute_value_range(
                    objc2_app_kit::NSObliquenessAttributeName,
                    &NSNumber::new_f64(0.18),
                    range,
                );
            }
            Kind::Code => {
                let mono = NSFont::monospacedSystemFontOfSize_weight(BODY_SIZE - 1.0, NSFontWeightRegular);
                storage.addAttribute_value_range(FONT, &mono, range);
                storage.addAttribute_value_range(BG, &ns_color(colors.code_bg), range);
            }
            Kind::Link(_) => {
                storage.addAttribute_value_range(FG, &ns_color(colors.accent), range);
                storage.addAttribute_value_range(UNDERLINE, &NSNumber::new_isize(1), range);
            }
            Kind::Done => {
                storage.addAttribute_value_range(FG, &muted, range);
                storage.addAttribute_value_range(STRIKE, &NSNumber::new_isize(1), range);
            }
        }
    }
}

/// 一张便签的原生编辑器：滚动容器 + 文本视图，挂在便签窗口内容视图的最上层。
pub struct RichEditor {
    scroll: Retained<NSScrollView>,
    text: Retained<NoteTextView>,
    frame: Cell<NSRect>,
}

impl RichEditor {
    /// 挂到 `ns_window` 的内容视图上（初始隐藏，由 [`RichEditor::layout`] 摆位置并显示）。
    pub fn attach(
        ns_window: &objc2_app_kit::NSWindow,
        text: &str,
        colors: Colors,
        tx: async_channel::Sender<EditorEvent>,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let content = ns_window.contentView()?;
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(200.0, 100.0));

        let view = NoteTextView::new(mtm, tx, colors);
        view.setFrame(frame);
        view.setRichText(false);
        view.setImportsGraphics(false);
        view.setAllowsUndo(true);
        view.setDrawsBackground(false);
        view.setTextContainerInset(NSSize::new(0.0, 2.0));
        view.setAutomaticQuoteSubstitutionEnabled(false);
        view.setAutomaticDashSubstitutionEnabled(false);
        view.setAutomaticTextReplacementEnabled(false);
        view.setAutomaticSpellingCorrectionEnabled(false);
        view.setInsertionPointColor(Some(&ns_color(colors.accent)));
        view.setVerticallyResizable(true);
        view.setHorizontallyResizable(false);
        view.setMinSize(NSSize::new(0.0, 0.0));
        view.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
        view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        // SAFETY: TextKit 1 初始化后 textContainer 一定在。
        if let Some(container) = unsafe { view.textContainer() } {
            container.setWidthTracksTextView(true);
            container.setLineFragmentPadding(0.0);
        }
        view.setString(&NSString::from_str(text));
        view.setDelegate(Some(ProtocolObject::from_ref(&*view)));

        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setBorderType(objc2_app_kit::NSBorderType::NoBorder);
        scroll.setDocumentView(Some(&view));
        scroll.setHidden(true);
        content.addSubview(&scroll);

        view.restyle();
        Some(Self {
            scroll,
            text: view,
            frame: Cell::new(NSRect::ZERO),
        })
    }

    /// 摆到卡片正文区（窗口内容视图坐标，左上原点的 GPUI 像素）；`None` = 隐藏（耳标态）。
    pub fn layout(&self, body: Option<(f64, f64, f64, f64)>, window_h: f64) {
        let Some((x, y, w, h)) = body else {
            if !self.scroll.isHidden() {
                self.scroll.setHidden(true);
            }
            return;
        };
        // 内容视图不翻转：y 轴向上。
        let rect = NSRect::new(NSPoint::new(x, window_h - y - h), NSSize::new(w.max(1.0), h.max(1.0)));
        let cur = self.frame.get();
        if cur != rect {
            self.frame.set(rect);
            self.scroll.setFrame(rect);
        }
        if self.scroll.isHidden() {
            self.scroll.setHidden(false);
        }
    }

    pub fn text(&self) -> String {
        self.text.text()
    }

    /// 外部改了正文（例如别处同步过来）时覆盖；正在编辑时不覆盖，免得冲掉光标和组字。
    pub fn set_text(&self, text: &str) {
        if self.text.ivars().editing.get() || self.text.text() == text {
            return;
        }
        self.text.setString(&NSString::from_str(text));
        self.text.restyle();
    }

    pub fn set_colors(&self, colors: Colors) {
        if self.text.ivars().colors.get() == colors {
            return;
        }
        self.text.ivars().colors.set(colors);
        self.text.setInsertionPointColor(Some(&ns_color(colors.accent)));
        self.text.restyle();
    }

    /// 进入 / 退出编辑：进入时把文本视图设成第一响应者（`caret_to_end` 时光标放到末尾），
    /// 退出时交还第一响应者，并把所有行都渲染出来。
    pub fn set_editing(&self, editing: bool, caret_to_end: bool) {
        let ivars = self.text.ivars();
        ivars.editing.set(editing);
        if let Some(window) = self.text.window() {
            if editing {
                window.makeFirstResponder(Some(&self.text));
                if caret_to_end {
                    let len = self.text.string().length();
                    self.text.setSelectedRange(NSRange::new(len, 0));
                }
            } else if window
                .firstResponder()
                .is_some_and(|r| std::ptr::eq(Retained::as_ptr(&r).cast::<()>(), Retained::as_ptr(&self.text).cast::<()>()))
            {
                // 收尾组字：没上屏的拼音不能丢在 marked text 里。
                self.text.unmarkText();
                window.makeFirstResponder(None);
            }
        }
        self.text.update_caret_lines();
        self.text.restyle();
    }

    /// 文本视图此刻是不是第一响应者（点正文时 AppKit 已经设好，不必再把光标挪到末尾）。
    pub fn is_first_responder(&self) -> bool {
        self.text.window().is_some_and(|w| {
            w.firstResponder()
                .is_some_and(|r| std::ptr::eq(Retained::as_ptr(&r).cast::<()>(), Retained::as_ptr(&self.text).cast::<()>()))
        })
    }

    pub fn has_marked_text(&self) -> bool {
        self.text.hasMarkedText()
    }

    /// 自检用：模拟输入法上屏一段文字（与真实按键 / 组字结束走同一条 insertText 路径）。
    pub fn insert_text_for_test(&self, text: &str) {
        let s = NSString::from_str(text);
        let len = self.text.string().length();
        self.text.setSelectedRange(NSRange::new(len, 0));
        // SAFETY: insertText:replacementRange: 接受 NSString；NSNotFound 表示替换当前选区。
        unsafe {
            let _: () = msg_send![&*self.text, insertText: &*s, replacementRange: NSRange::new(usize::MAX >> 1, 0)];
        }
    }
}

impl Drop for RichEditor {
    fn drop(&mut self) {
        self.text.setDelegate(None);
        self.scroll.removeFromSuperview();
    }
}
