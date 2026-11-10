//! 取色器与颜色转换。
//!
//! - 命令「取色 / Pick Color」：调起系统取色放大镜（`NSColorSampler`），选中后在面板里显示色块和
//!   HEX、RGB、HSL、OKLCH、SwiftUI、NSColor 各种写法，回车复制 HEX，⌘K 复制其他写法。
//!   取色器必须在主线程打开：动作在后台线程，通过 GCD 主队列打开，再等用户选完（取消时什么也不做）。
//! - 直接输入 `#ff8800`、`rgb(255, 136, 0)`、`hsl(32, 100%, 50%)`：每种写法一行，列表图标和详情里有色块。
//! - 色块是写到 `<data_dir>/colors/` 的小 PNG（详情区没有原生色块，用图片显示）。
//! - 测试接缝：`HELMP_COLOR_FIXTURE=#3366ff` 时「取色」不打开取色器，直接当作取到了这个颜色；
//!   否则演练模式下只回报 `[dry-run] pick-color`。

mod convert;

use std::path::PathBuf;

use anyhow::{Context as _, Result};

use crate::i18n::{tr, trf};
use crate::matcher;
use crate::provider::{Action, Block, Detail, Icon, Item, NamedAction, Outcome, Provider, Tint};
use convert::Rgba;

const PROVIDER_ID: &str = "color";
/// 输入就是一个颜色：转换结果排第一。
const SCORE: u32 = 1050;
const ICON: Icon = Icon::Symbol {
    name: "pipette",
    tint: Tint::Purple,
};

fn badge() -> &'static str {
    tr("颜色", "Color")
}

pub struct Color;

impl Provider for Color {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let query = query.trim();
        if let Some(color) = convert::parse(query) {
            return conversion_items(color);
        }
        pick_command(query).into_iter().collect()
    }
}

/// 某个颜色的各种写法：(键, 显示名, 写法)。第一个（HEX）是默认复制的。
fn formats(color: Rgba) -> Vec<(&'static str, &'static str, String)> {
    vec![
        ("hex", "HEX", color.hex()),
        ("rgb", "RGB", color.css_rgb()),
        ("hsl", "HSL", color.css_hsl()),
        ("oklch", "OKLCH", color.css_oklch()),
        ("swiftui", "SwiftUI", color.swiftui()),
        ("nscolor", "NSColor", color.nscolor()),
    ]
}

fn conversion_items(color: Rgba) -> Vec<Item> {
    let swatch = swatch(color).ok();
    let detail = detail(color, swatch.clone(), Vec::new());
    formats(color)
        .into_iter()
        .enumerate()
        .map(|(rank, (key, label, value))| Item {
            id: format!("{PROVIDER_ID}:{key}"),
            title: value.clone(),
            subtitle: label.into(),
            badge: badge().into(),
            score: SCORE.saturating_sub(rank as u32),
            icon: swatch.clone().map(Icon::Image).unwrap_or(ICON),
            action: Action::copy(value),
            action_title: trf!("复制 {label}", "Copy {label}"),
            detail: Some(detail.clone()),
            ..Item::blank()
        })
        .collect()
}

/// 详情：色块 + 全部写法（键值表）。`actions` 非空时（取色结果）第一个是回车。
fn detail(color: Rgba, swatch: Option<PathBuf>, actions: Vec<NamedAction>) -> Detail {
    Detail {
        icon: swatch.clone().map(Icon::Image).unwrap_or(ICON),
        title: color.hex(),
        tag: None,
        blocks: swatch_strip(color).ok().into_iter().map(Block::Image).collect(),
        metadata: formats(color)
            .into_iter()
            .map(|(_, label, value)| (label.to_string(), value))
            .collect(),
        actions,
    }
}

/// 「取色」命令：中文、拼音、英文都能找到。
fn pick_command(query: &str) -> Option<Item> {
    if query.chars().count() < 2 {
        return None;
    }
    let score = [
        "取色",
        "取色器",
        "拾色器",
        "吸管",
        "颜色",
        "pick color",
        "color picker",
        "eyedropper",
    ]
    .iter()
    .filter_map(|keyword| matcher::score(query, keyword))
    .max()
    .filter(|score| *score >= 700)?;
    Some(Item {
        id: format!("{PROVIDER_ID}:pick"),
        title: tr("取色", "Pick Color").into(),
        subtitle: tr(
            "从屏幕上取一个颜色，显示 HEX、RGB、HSL、SwiftUI 等写法",
            "Pick a color on screen and get HEX, RGB, HSL, SwiftUI and more",
        )
        .into(),
        badge: badge().into(),
        score,
        icon: ICON,
        action: Action::new(pick),
        action_title: tr("取色", "Pick").into(),
        ..Item::blank()
    })
}

/// 取色：打开系统取色器，等用户选完，返回结果详情。
fn pick() -> Result<Outcome> {
    let color = match std::env::var("HELMP_COLOR_FIXTURE") {
        Ok(fixture) => Some(convert::parse(&fixture).with_context(|| {
            trf!(
                "HELMP_COLOR_FIXTURE 不是颜色：{fixture}",
                "HELMP_COLOR_FIXTURE isn’t a color: {fixture}"
            )
        })?),
        Err(_) => {
            if let Some(outcome) = crate::effects::dry("pick-color", "") {
                return Ok(outcome);
            }
            sample()?
        }
    };
    let Some(color) = color else {
        // 用户按 Esc 取消了取色。
        return Ok(Outcome::Done);
    };
    let swatch = swatch(color).ok();
    let actions = formats(color)
        .into_iter()
        .map(|(_, label, value)| {
            NamedAction::new(
                trf!("复制 {label}", "Copy {label}"),
                None,
                Action::copy(value),
            )
        })
        .collect();
    Ok(Outcome::Show(detail(color, swatch, actions)))
}

/// 在主线程打开 `NSColorSampler`，阻塞等待结果（本函数在动作的后台线程里调用）。取消返回 `None`。
fn sample() -> Result<Option<Rgba>> {
    use objc2_app_kit::{NSColor, NSColorSampler, NSColorSpace};

    let (tx, rx) = std::sync::mpsc::channel::<Option<Rgba>>();
    dispatch2::DispatchQueue::main().exec_async(move || {
        let sampler = NSColorSampler::new();
        let handler = block2::RcBlock::new(move |picked: *mut NSColor| {
            // SAFETY: 取色器在主线程回调，传入的颜色在回调期间有效；取消时是空指针。
            let color = unsafe { picked.as_ref() }
                .and_then(|color| color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace()))
                .map(|srgb| Rgba {
                    r: srgb.redComponent(),
                    g: srgb.greenComponent(),
                    b: srgb.blueComponent(),
                    a: srgb.alphaComponent(),
                });
            let _ = tx.send(color);
        });
        // SAFETY: 在主线程调用；取色器在会话结束前自己持有自己和回调。
        unsafe { sampler.showSamplerWithSelectionHandler(&handler) };
    });
    rx.recv()
        .context(tr("取色器没有返回结果", "The color sampler didn’t return"))
}

/// 列表图标用的 48×48 色块 PNG（按颜色缓存，已存在就不重写）。
fn swatch(color: Rgba) -> Result<PathBuf> {
    swatch_sized(color, 48, 48)
}

/// 详情区的横条色块：图片按原始尺寸（pt）居中画在卡片里，方块图只占中间一小块、两边露白底。
/// 宽度取详情区卡片的宽（面板 880 pt 时约 448 pt），高度留给下面的各种写法。
fn swatch_strip(color: Rgba) -> Result<PathBuf> {
    swatch_sized(color, 448, 64)
}

fn swatch_sized(color: Rgba, width: u32, height: u32) -> Result<PathBuf> {
    let dir = crate::paths::data_dir()?.join("colors");
    std::fs::create_dir_all(&dir)?;
    let [r, g, b, a] = color.bytes();
    // 方块沿用旧文件名（已有缓存照用），其他尺寸带上尺寸。
    let name = if (width, height) == (48, 48) {
        format!("{r:02x}{g:02x}{b:02x}{a:02x}.png")
    } else {
        format!("{r:02x}{g:02x}{b:02x}{a:02x}-{width}x{height}.png")
    };
    let path = dir.join(name);
    if path.exists() {
        return Ok(path);
    }
    let pixels: Vec<u8> = std::iter::repeat_n([r, g, b, a], (width * height) as usize)
        .flatten()
        .collect();
    let tmp = path.with_extension("png.tmp");
    {
        let file = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&pixels)?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_colors_list_every_format_with_hex_first() {
        // 输入一个颜色就出各种写法，第一条是 HEX，回车复制它。
        let items = Color.query("rgb(255, 136, 0)");
        let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "#ff8800",
                "rgb(255, 136, 0)",
                "hsl(32, 100%, 50%)",
                "oklch(0.7442 0.1812 56.46)",
                "Color(red: 1.000, green: 0.533, blue: 0.000)",
                "NSColor(srgbRed: 1.000, green: 0.533, blue: 0.000, alpha: 1.000)",
            ]
        );
        assert!(items.first().unwrap().score > 1000);
    }

    #[test]
    fn plain_searches_produce_nothing() {
        for query in ["wx", "微信", "safari", "ff8800", "#", "rgb", "c"] {
            assert!(Color.query(query).is_empty(), "{query:?}");
        }
    }

    #[test]
    fn pick_command_is_found_by_chinese_pinyin_and_english() {
        for query in ["取色", "qs", "pick color", "color picker"] {
            let items = Color.query(query);
            assert!(items.iter().any(|i| i.id == "color:pick"), "{query:?}");
        }
    }

    #[test]
    fn icon_exists() {
        use gpui_kit::AssetSource as _;
        assert!(
            gpui_kit::assets::AllAssets
                .load("icons/pipette.svg")
                .unwrap()
                .is_some()
        );
    }
}
