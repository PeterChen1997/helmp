//! 设计稿配色（macOS Tahoe 原生，见 design/canvas 的视觉规范便签）。浅色、深色两套，
//! 按窗口外观和「设置 › 外观」选择。启动器面板、设置窗口、引导窗口共用。

use gpui_kit::*;

use crate::config::Appearance;
use crate::provider::{Tint, Tone};

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    /// 面板毛玻璃底色（半透明，窗口本身是 Blurred）。
    pub panel: Hsla,
    /// 设置窗口内容区底色。
    pub window: Hsla,
    /// 设置侧栏。
    pub sidebar: Hsla,
    /// 分组表单底色。
    pub group: Hsla,
    pub fg: Hsla,
    pub muted: Hsla,
    pub separator: Hsla,
    pub selection: Hsla,
    pub selection_fg: Hsla,
    pub selection_muted: Hsla,
    pub key_bg: Hsla,
    pub key_fg: Hsla,
    pub card: Hsla,
    pub control: Hsla,
    pub accent: Hsla,
    pub danger: Hsla,
}

fn c(hex: u32) -> Hsla {
    rgba(hex).into()
}

pub fn light() -> Palette {
    Palette {
        dark: false,
        panel: c(0xf6f6f8bd),
        window: c(0xfcfcfdf0),
        sidebar: c(0xececf0b3),
        group: c(0x00000009),
        fg: c(0x1d1d1fff),
        muted: c(0x6e6e73ff),
        separator: c(0x00000014),
        selection: c(0x0b63e5ff),
        selection_fg: c(0xffffffff),
        selection_muted: c(0xffffffe0),
        key_bg: c(0x00000012),
        key_fg: c(0x3a3a3cff),
        card: c(0xffffff99),
        control: c(0xffffffff),
        accent: c(0x0b63e5ff),
        danger: c(0xb3261eff),
    }
}

pub fn dark() -> Palette {
    Palette {
        dark: true,
        panel: c(0x262629c7),
        window: c(0x1e1e20f2),
        sidebar: c(0x2c2c30b3),
        group: c(0xffffff0d),
        fg: c(0xf5f5f7ff),
        muted: c(0xa1a1a6ff),
        separator: c(0xffffff1a),
        selection: c(0x0a5fd6ff),
        selection_fg: c(0xffffffff),
        selection_muted: c(0xffffffd9),
        key_bg: c(0xffffff1f),
        key_fg: c(0xe5e5eaff),
        card: c(0xffffff12),
        control: c(0x3a3a3eff),
        accent: c(0x3b8bffff),
        danger: c(0xff6b5eff),
    }
}

/// 当前应使用的配色：设置里强制浅 / 深色优先，否则跟随窗口外观。
pub fn palette(window: &Window) -> Palette {
    match crate::config::get().appearance {
        Appearance::Light => light(),
        Appearance::Dark => dark(),
        Appearance::System => match window.appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => dark(),
            _ => light(),
        },
    }
}

/// 图标底块颜色。
pub fn tint(tint: Tint) -> Hsla {
    c(match tint {
        Tint::Blue => 0x0b63e5ff,
        Tint::Indigo => 0x5e5ce6ff,
        Tint::Purple => 0x8944abff,
        Tint::Orange => 0xe8790aff,
        Tint::Teal => 0x0a7f8aff,
        Tint::Green => 0x1f8a3bff,
        Tint::Red => 0xd93a2fff,
        Tint::Gray => 0x8e8e93ff,
        Tint::Graphite => 0x3a3a3cff,
    })
}

/// 范围标签（搜索栏左侧）的底色与文字色。
pub fn scope_colors(tint: Tint, dark: bool) -> (Hsla, Hsla) {
    let base = self::tint(tint);
    let bg = base.opacity(if dark { 0.3 } else { 0.14 });
    let fg = if dark {
        base.blend(c(0xffffffb3))
    } else {
        base.blend(c(0x0000004d))
    };
    (bg, fg)
}

/// 状态标签的底色与文字色。
pub fn tone_colors(tone: Tone, dark: bool) -> (Hsla, Hsla) {
    let (bg, fg) = match (tone, dark) {
        (Tone::Waiting, false) => (0xffe7c7ff, 0x8a3b00ff),
        (Tone::Running, false) => (0xdceaffff, 0x0a4fb8ff),
        (Tone::Done, false) => (0xdcf2e1ff, 0x1c6b30ff),
        (Tone::Accent, false) => (0x0b63e51f, 0x0a4fb8ff),
        (Tone::Neutral, false) => (0x0000000f, 0x48484aff),
        (Tone::Waiting, true) => (0x8a3b0059, 0xffc58aff),
        (Tone::Running, true) => (0x0a4fb859, 0x9cc4ffff),
        (Tone::Done, true) => (0x1c6b3059, 0x9be0abff),
        (Tone::Accent, true) => (0x3b8bff33, 0x9cc4ffff),
        (Tone::Neutral, true) => (0xffffff1a, 0xd1d1d6ff),
    };
    (c(bg), c(fg))
}
