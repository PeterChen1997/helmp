//! 插件主题：插件在清单里声明浅 / 深两套配色（`themes`），用户在「设置 › 外观」里选；
//! 浅 / 深仍按原有的外观设置，主题只换配色。
//!
//! - 配色只开放四个令牌（底色、文字、强调色、选中色），其余颜色由它们推导（见 `Colors`）。
//!   校验时检查对比度：文字、次要文字、强调色都要够清楚，背景图压在搜索栏下面时也一样；
//!   选中行、主按钮上的文字自动取白或黑，不用校验。
//! - 可选背景图：只铺在面板顶部、往下渐隐（见 `launcher`），列表文字始终落在纯色底上。
//! - 日期段：`dates` 里的某一段包含今天（本机时区）且用户没关「按日期自动切换」时自动换上，
//!   过后恢复用户自己选的主题。写法沿用寄语（`MM-DD` 每年重复、`YYYY-MM-DD` 只那一段，
//!   平年的 02-29 落到 2 月 28 日）。几个主题同时在日期内时取插件 id 排序最前的那个。
//! - 只看已启用的插件：提供主题的插件被停用或卸载，面板马上回到默认配色（选择仍记在配置里，
//!   重新启用后恢复）。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{Datelike as _, NaiveDate};

use super::greeting::{DateRule, local_date};
use super::manifest::{ThemeColors, ThemeDates};
use super::registry::{self, Entry, State};
use crate::config::Config;
use crate::i18n::{tr, trf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const WHITE: Rgb = Rgb {
        r: 255,
        g: 255,
        b: 255,
    };
    pub const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };

    /// 只认 `#rrggbb`：不带透明度（半透明由 Helmp 按窗口决定）。
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#')?;
        if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let value = u32::from_str_radix(hex, 16).ok()?;
        let [_, r, g, b] = value.to_be_bytes();
        Some(Rgb { r, g, b })
    }

    /// 往 `other` 那边混 `share`（0 = 自己，1 = other）。
    pub fn mix(self, other: Rgb, share: f64) -> Rgb {
        let channel = |a: u8, b: u8| {
            let value = f64::from(a) + (f64::from(b) - f64::from(a)) * share;
            // 0..=255 之内，round 后的转换不会截断。
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let value = value.round().clamp(0.0, 255.0) as u8;
            value
        };
        Rgb {
            r: channel(self.r, other.r),
            g: channel(self.g, other.g),
            b: channel(self.b, other.b),
        }
    }

    /// WCAG 相对亮度。
    pub fn luminance(self) -> f64 {
        let linear = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(self.r) + 0.7152 * linear(self.g) + 0.0722 * linear(self.b)
    }

    /// `0xRRGGBB`。
    pub fn hex(self) -> u32 {
        u32::from_be_bytes([0, self.r, self.g, self.b])
    }
}

/// WCAG 对比度（1–21）。
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (a.luminance(), b.luminance());
    let (light, dark) = if la > lb { (la, lb) } else { (lb, la) };
    (light + 0.05) / (dark + 0.05)
}

/// 次要文字 = 文字往底色混这么多。
const MUTED_SHARE: f64 = 0.4;
/// 背景图在搜索栏一带至少压着这么多底色（渐隐从这里开始，见 `launcher`）。
pub const IMAGE_SCRIM: f64 = 0.5;
/// 正文、选中行文字的最低对比度（WCAG AA）。
const MIN_TEXT: f64 = 4.5;
/// 次要文字、强调色的最低对比度（WCAG AA 大字 / 界面元素）。
const MIN_SECONDARY: f64 = 3.0;

/// 一套配色：清单里的四个令牌加上推导出来的颜色。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    pub background: Rgb,
    pub foreground: Rgb,
    pub accent: Rgb,
    pub selection: Rgb,
    /// 次要文字（副标题、占位文字）。
    pub muted: Rgb,
    /// 选中行上的文字：白或黑，取对比度高的那个。
    pub selection_fg: Rgb,
    /// 强调色底上的文字（主按钮）：同上。
    pub accent_fg: Rgb,
}

impl Colors {
    fn from_parts(background: Rgb, foreground: Rgb, accent: Rgb, selection: Rgb) -> Self {
        let on = |fill: Rgb| {
            if contrast(Rgb::WHITE, fill) >= contrast(Rgb::BLACK, fill) {
                Rgb::WHITE
            } else {
                Rgb::BLACK
            }
        };
        Colors {
            background,
            foreground,
            accent,
            selection,
            muted: foreground.mix(background, MUTED_SHARE),
            selection_fg: on(selection),
            accent_fg: on(accent),
        }
    }
}

/// 背景图的平均颜色（带平均透明度），用来估算压在图上的文字够不够清楚。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tone {
    pub rgb: Rgb,
    pub alpha: f64,
}

/// 读出图片的平均颜色（缩到 64×64 再平均，4 MiB 的图也只要几十毫秒）。
pub fn average_color(path: &Path) -> anyhow::Result<Tone> {
    let image = image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()?
        .thumbnail(64, 64)
        .to_rgba8();
    let (mut sum, mut weight, mut count) = ([0.0_f64; 3], 0.0_f64, 0.0_f64);
    for pixel in image.pixels() {
        let [r, g, b, a] = pixel.0;
        let a = f64::from(a) / 255.0;
        sum[0] += f64::from(r) * a;
        sum[1] += f64::from(g) * a;
        sum[2] += f64::from(b) * a;
        weight += a;
        count += 1.0;
    }
    anyhow::ensure!(count > 0.0, "empty image");
    let channel = |total: f64| {
        let value = if weight > 0.0 { total / weight } else { 0.0 };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let value = value.round().clamp(0.0, 255.0) as u8;
        value
    };
    Ok(Tone {
        rgb: Rgb {
            r: channel(sum[0]),
            g: channel(sum[1]),
            b: channel(sum[2]),
        },
        alpha: weight / count,
    })
}

/// 校验一套配色。出错时返回 (字段, 原因)：字段是 `background` 这类令牌名，背景图的问题是 `image`。
pub fn check_colors(
    spec: &ThemeColors,
    dark: bool,
    image: Option<&Tone>,
) -> Result<Colors, Vec<(&'static str, String)>> {
    let mut problems = Vec::new();
    let mut parse = |field: &'static str, text: &str| {
        let color = Rgb::parse(text);
        if color.is_none() {
            problems.push((
                field,
                trf!(
                    "{text:?} 不是 #rrggbb 形式的颜色",
                    "{text:?} isn't a #rrggbb color"
                ),
            ));
        }
        color
    };
    let parsed = (
        parse("background", &spec.background),
        parse("foreground", &spec.foreground),
        parse("accent", &spec.accent),
        parse("selection", &spec.selection),
    );
    let (Some(background), Some(foreground), Some(accent), Some(selection)) = parsed else {
        return Err(problems);
    };
    let colors = Colors::from_parts(background, foreground, accent, selection);
    let ratio = |a, b| contrast(a, b);
    fn low(
        problems: &mut Vec<(&'static str, String)>,
        field: &'static str,
        (what_zh, what_en): (&str, &str),
        value: f64,
        min: f64,
    ) {
        if value < min {
            problems.push((
                field,
                trf!(
                    "{what_zh}与底色的对比度只有 {value:.1}:1，至少要 {min}:1",
                    "{what_en} has a contrast of only {value:.1}:1 against the background (needs {min}:1)"
                ),
            ));
        }
    }
    // 浅色套的底色要比文字亮，深色套反过来：输入框等系统组件按明暗配色，反了会看不清。
    if (background.luminance() > foreground.luminance()) == dark {
        problems.push((
            "background",
            if dark {
                tr(
                    "深色套的底色要比文字暗",
                    "the dark set needs a background darker than its foreground",
                )
            } else {
                tr(
                    "浅色套的底色要比文字亮",
                    "the light set needs a background lighter than its foreground",
                )
            }
            .into(),
        ));
    }
    low(
        &mut problems,
        "foreground",
        ("文字", "Text"),
        ratio(foreground, background),
        MIN_TEXT,
    );
    low(
        &mut problems,
        "foreground",
        (
            "次要文字（由文字和底色混出来）",
            "Secondary text (mixed from foreground and background)",
        ),
        ratio(colors.muted, background),
        MIN_SECONDARY,
    );
    low(
        &mut problems,
        "accent",
        ("强调色", "The accent"),
        ratio(accent, background),
        MIN_SECONDARY,
    );
    if let Some(tone) = image {
        // 图在搜索栏一带至少压着 IMAGE_SCRIM 的底色：用平均颜色估算那里的底色。
        let under = background
            .mix(tone.rgb, tone.alpha)
            .mix(background, IMAGE_SCRIM);
        let (text, secondary) = (ratio(foreground, under), ratio(colors.muted, under));
        if text < MIN_TEXT || secondary < MIN_SECONDARY {
            problems.push((
                "image",
                trf!(
                    "图片太{}：压在图上的文字对比度只有 {text:.1}:1（次要文字 {secondary:.1}:1），至少要 {MIN_TEXT}:1 和 {MIN_SECONDARY}:1",
                    "the image is too {}: text over it has a contrast of only {text:.1}:1 (secondary {secondary:.1}:1; needs {MIN_TEXT}:1 and {MIN_SECONDARY}:1)",
                    if dark { tr("亮", "bright") } else { tr("暗", "dark") }
                ),
            ));
        }
    }
    if problems.is_empty() {
        Ok(colors)
    } else {
        Err(problems)
    }
}

// ---------------------------------------------------------------- 日期段

/// 每年 `month-day` 在 `year` 落在哪天：平年的 02-29 落到 2 月 28 日（与寄语、定时任务相同）。
fn yearly_in(year: i32, (month, day): (u32, u32)) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day)
        .or_else(|| NaiveDate::from_ymd_opt(year, 2, 28))
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateRange {
    /// 每年 `from` 到 `to`（月, 日）；`from` 比 `to` 晚表示跨年。
    Yearly { from: (u32, u32), to: (u32, u32) },
    /// 只这一段。
    Once { from: NaiveDate, to: NaiveDate },
}

impl DateRange {
    pub fn parse(spec: &ThemeDates) -> Result<Self, String> {
        let rule = |field: &str, text: &str| {
            DateRule::parse(text).ok_or_else(|| {
                trf!(
                    "{field}：{text:?} 不是合法日期（MM-DD 每年重复，或 YYYY-MM-DD）",
                    "{field}: {text:?} isn't a valid date (MM-DD every year, or YYYY-MM-DD)"
                )
            })
        };
        let from = rule("from", &spec.from)?;
        let to = match &spec.to {
            Some(to) => rule("to", to)?,
            None => from,
        };
        match (from, to) {
            (DateRule::Yearly { month, day }, DateRule::Yearly { month: m, day: d }) => {
                Ok(DateRange::Yearly {
                    from: (month, day),
                    to: (m, d),
                })
            }
            (DateRule::Once(from), DateRule::Once(to)) if from <= to => {
                Ok(DateRange::Once { from, to })
            }
            (DateRule::Once(_), DateRule::Once(_)) => {
                Err(tr("to 早于 from", "to is earlier than from").into())
            }
            _ => Err(tr(
                "from 和 to 要用同一种写法（都是 MM-DD 或都是 YYYY-MM-DD）",
                "from and to must use the same form (both MM-DD or both YYYY-MM-DD)",
            )
            .into()),
        }
    }

    /// 今天在不在这一段里，在的话返回这一段的最后一天。
    pub fn current(&self, today: NaiveDate) -> Option<NaiveDate> {
        match *self {
            DateRange::Once { from, to } => (from <= today && today <= to).then_some(to),
            DateRange::Yearly { from, to } => {
                let year = today.year();
                if from <= to {
                    let (start, end) = (yearly_in(year, from), yearly_in(year, to));
                    (start <= today && today <= end).then_some(end)
                } else if today >= yearly_in(year, from) {
                    // 跨年：今年的开头那半段，明年结束。
                    Some(yearly_in(year + 1, to))
                } else {
                    let end = yearly_in(year, to);
                    (today <= end).then_some(end)
                }
            }
        }
    }

    /// 设置里显示的写法：`每年 12-20 – 12-26`。
    pub fn describe(&self) -> String {
        let md = |(month, day): (u32, u32)| format!("{month:02}-{day:02}");
        match *self {
            DateRange::Yearly { from, to } if from == to => {
                trf!("每年 {}", "Every year {}", md(from))
            }
            DateRange::Yearly { from, to } => {
                trf!("每年 {} – {}", "Every year {} – {}", md(from), md(to))
            }
            DateRange::Once { from, to } if from == to => from.to_string(),
            DateRange::Once { from, to } => format!("{from} – {to}"),
        }
    }
}

// ---------------------------------------------------------------- 可选主题与当前主题

/// 一个可选的主题（来自已启用的插件）。
#[derive(Debug, Clone, PartialEq)]
pub struct Available {
    /// 配置里记的键：`插件 id/主题 id`。
    pub key: String,
    pub name: String,
    pub plugin_name: String,
    pub light: Colors,
    pub dark: Colors,
    pub image_light: Option<PathBuf>,
    pub image_dark: Option<PathBuf>,
    pub dates: Vec<DateRange>,
}

impl Available {
    pub fn colors(&self, dark: bool) -> &Colors {
        if dark { &self.dark } else { &self.light }
    }

    pub fn image(&self, dark: bool) -> Option<&PathBuf> {
        if dark {
            self.image_dark.as_ref()
        } else {
            self.image_light.as_ref()
        }
    }
}

pub fn key(plugin: &str, theme: &str) -> String {
    format!("{plugin}/{theme}")
}

/// 已启用插件提供的全部主题：按插件 id 排序，同一插件内按清单顺序。
pub fn available(config: &Config, entries: &[Entry]) -> Vec<Available> {
    let mut plugins: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.state(config) == State::Enabled)
        .collect();
    plugins.sort_by(|a, b| a.id.cmp(&b.id));
    let mut out = Vec::new();
    for entry in plugins {
        let Some(plugin) = entry.plugin() else {
            continue;
        };
        for spec in &plugin.manifest.themes {
            // 加载时已经校验过；这里再解析一次只是为了拿到推导色，失败（不会发生）就跳过。
            let (Ok(light), Ok(dark)) = (
                check_colors(&spec.light, false, None),
                check_colors(&spec.dark, true, None),
            ) else {
                continue;
            };
            let image = |path: Option<&String>| {
                path.and_then(|path| plugin.dir.as_ref().map(|dir| dir.join(path)))
            };
            let images = spec.image.as_ref();
            out.push(Available {
                key: key(plugin.id(), &spec.id),
                name: spec.name.get().to_string(),
                plugin_name: plugin.manifest.name.get().to_string(),
                light,
                dark,
                image_light: image(images.and_then(|image| image.light.as_ref())),
                image_dark: image(images.and_then(|image| image.dark.as_ref())),
                dates: spec
                    .dates
                    .iter()
                    .filter_map(|dates| DateRange::parse(dates).ok())
                    .collect(),
            });
        }
    }
    out
}

/// 今天在日期内的主题（不管用户有没有关掉按日期切换）：返回它和这一段的最后一天。
pub fn dated_today(available: &[Available], today: NaiveDate) -> Option<(&Available, NaiveDate)> {
    available.iter().find_map(|theme| {
        theme
            .dates
            .iter()
            .find_map(|range| range.current(today))
            .map(|end| (theme, end))
    })
}

/// 用户点了「今天不用」：今天不按日期换主题。
pub fn skipped_today(config: &Config, today: NaiveDate) -> bool {
    config.theme.skip_date.as_deref() == Some(today.to_string().as_str())
}

/// 现在生效的主题。
#[derive(Debug, Clone, PartialEq)]
pub struct Active {
    pub theme: Available,
    /// 是按日期自动换上的。
    pub dated: bool,
    /// 按日期换上时，这一段的最后一天（之后恢复用户自己选的）。
    pub until: Option<NaiveDate>,
}

/// 按日期的优先（用户开着「按日期自动切换」且今天没点「今天不用」），否则用户选的；都没有用默认配色。
pub fn resolve(config: &Config, available: &[Available], today: NaiveDate) -> Option<Active> {
    if config.theme.by_date
        && !skipped_today(config, today)
        && let Some((theme, until)) = dated_today(available, today)
    {
        return Some(Active {
            theme: theme.clone(),
            dated: true,
            until: Some(until),
        });
    }
    let selected = config.theme.selected.as_deref()?;
    available
        .iter()
        .find(|theme| theme.key == selected)
        .map(|theme| Active {
            theme: theme.clone(),
            dated: false,
            until: None,
        })
}

/// 用户选的主题还在不在（插件停用 / 卸载后为 false，设置里按默认显示）。
pub fn selected_available(config: &Config, available: &[Available]) -> Option<String> {
    let selected = config.theme.selected.as_deref()?;
    available
        .iter()
        .any(|theme| theme.key == selected)
        .then(|| selected.to_string())
}

type Cached = (Arc<Config>, Arc<Vec<Entry>>, NaiveDate, Option<Arc<Active>>);

/// 上一次的结果：配置、插件列表、日期都没变就直接用（每次重绘都会问）。
static CACHE: Mutex<Option<Cached>> = Mutex::new(None);
/// 上一次上报「主题生效」的日期：一天最多报一次。
static REPORTED: Mutex<Option<NaiveDate>> = Mutex::new(None);

/// 现在生效的主题（没有时用默认配色）。启动器面板、设置窗口、引导窗口每次重绘都调用：
/// 配置、插件列表和日期都没变时直接返回上一次的结果。
pub fn current() -> Option<Arc<Active>> {
    let config = crate::config::get();
    let entries = registry::current();
    let today = local_date(chrono::Utc::now(), &chrono::Local);
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((c, e, day, active)) = cache.as_ref()
        && Arc::ptr_eq(c, &config)
        && Arc::ptr_eq(e, &entries)
        && *day == today
    {
        return active.clone();
    }
    let active = resolve(&config, &available(&config, &entries), today).map(Arc::new);
    if let Some(active) = &active {
        report_applied(active, today);
    }
    *cache = Some((config, entries, today, active.clone()));
    active
}

fn report_applied(active: &Active, today: NaiveDate) {
    let mut reported = REPORTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *reported == Some(today) {
        return;
    }
    *reported = Some(today);
    let images = [&active.theme.image_light, &active.theme.image_dark];
    let has_image = images.iter().any(|image| image.is_some());
    crate::telemetry::plugin_theme_applied(active.dated, has_image);
    // 加载时检查过图片；之后被删掉的话面板顶部会是空的：报一次（不带路径）。
    if images
        .iter()
        .any(|image| image.as_ref().is_some_and(|path| !path.is_file()))
    {
        crate::telemetry::report(
            "plugins.theme_image",
            &anyhow::anyhow!("theme background image disappeared after loading"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::plugins::manifest;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn range(from: &str, to: Option<&str>) -> Result<DateRange, String> {
        DateRange::parse(&ThemeDates {
            from: from.into(),
            to: to.map(Into::into),
        })
    }

    fn colors(background: &str, foreground: &str, accent: &str, selection: &str) -> ThemeColors {
        ThemeColors {
            background: background.into(),
            foreground: foreground.into(),
            accent: accent.into(),
            selection: selection.into(),
        }
    }

    /// 清单里的 `themes` 校验错误（英文）。
    fn errors(themes: &str) -> String {
        let text = format!(
            r#"{{"schemaVersion": 1, "id": "com.example.themes", "name": "Themes", "version": "1.0.0",
                "author": "Example", "themes": {themes}}}"#
        );
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            manifest::parse(&text, None)
                .err()
                .unwrap_or_default()
                .join("\n")
        })
    }

    const LIGHT: &str = r##"{"background": "#fdf8f2", "foreground": "#2b1d17", "accent": "#b3261e", "selection": "#b3261e"}"##;
    const DARK: &str = r##"{"background": "#182030", "foreground": "#f3efe6", "accent": "#e0b64a", "selection": "#9a2a22"}"##;

    #[test]
    fn colors_parse_only_as_six_digit_hex() {
        assert_eq!(
            Rgb::parse("#0b63e5"),
            Some(Rgb {
                r: 0x0b,
                g: 0x63,
                b: 0xe5
            })
        );
        assert_eq!(Rgb::parse("#FFFFFF"), Some(Rgb::WHITE));
        // 透明度由 Helmp 按窗口决定；简写、颜色名猜错了会是另一种颜色：都拒绝。
        for bad in ["#fff", "#0b63e5ff", "0b63e5", "red", "#gggggg", ""] {
            assert_eq!(Rgb::parse(bad), None, "{bad}");
        }
        assert_eq!(Rgb::parse("#0b63e5").unwrap().hex(), 0x0b63e5);
    }

    #[test]
    fn contrast_matches_wcag() {
        assert!((contrast(Rgb::WHITE, Rgb::BLACK) - 21.0).abs() < 1e-9);
        assert!((contrast(Rgb::WHITE, Rgb::WHITE) - 1.0).abs() < 1e-9);
        // #767676 是白底上刚好过 4.5:1 的灰。
        let gray = Rgb::parse("#767676").unwrap();
        assert!(contrast(gray, Rgb::WHITE) >= 4.5);
        assert!(contrast(Rgb::parse("#777777").unwrap(), Rgb::WHITE) < 4.5);
    }

    #[test]
    fn helmp_default_palettes_would_pass_the_same_checks() {
        // 校验不能比 Helmp 自己的配色还严：默认的浅 / 深色按同样的规则也得通过。
        let light = colors("#f6f6f8", "#1d1d1f", "#0b63e5", "#0b63e5");
        let dark = colors("#262629", "#f5f5f7", "#3b8bff", "#0a5fd6");
        assert!(check_colors(&light, false, None).is_ok());
        assert!(check_colors(&dark, true, None).is_ok());
    }

    #[test]
    fn unreadable_colors_are_rejected_with_the_field_and_ratio() {
        // 对比度不够的配色会做出读不出的界面：校验时就拦下，告诉作者是哪个令牌、差多少。
        let messages = |spec: ThemeColors, dark: bool| {
            crate::i18n::with_lang(crate::i18n::Lang::En, || {
                check_colors(&spec, dark, None).err().unwrap_or_default()
            })
        };
        let low_text = messages(colors("#ffffff", "#aaaaaa", "#0b63e5", "#0b63e5"), false);
        assert!(
            low_text
                .iter()
                .any(|(field, m)| *field == "foreground" && m.contains(":1")),
            "{low_text:?}"
        );
        let pale_accent = messages(colors("#ffffff", "#111111", "#ffe680", "#0b63e5"), false);
        assert_eq!(
            pale_accent.first().map(|(f, _)| *f),
            Some("accent"),
            "{pale_accent:?}"
        );
        // 选中色不用校验：上面的文字在白和黑里取对比度高的，任何颜色都至少有 4.5:1（最差的中间调约 4.6:1）。
        for grey in ["#808080", "#757575", "#8a8a8a", "#ff0000", "#00a000"] {
            let picked =
                check_colors(&colors("#ffffff", "#111111", "#0b63e5", grey), false, None).unwrap();
            assert!(
                contrast(picked.selection_fg, picked.selection) >= MIN_TEXT,
                "{grey}"
            );
        }
        // 浅色套写成深底：输入框等系统组件按浅色画，会看不清。
        let inverted = messages(colors("#111111", "#ffffff", "#3b8bff", "#0a5fd6"), false);
        assert!(
            inverted.iter().any(|(f, _)| *f == "background"),
            "{inverted:?}"
        );
        let bad_hex = messages(colors("white", "#111111", "#0b63e5", "#0b63e5"), false);
        assert!(
            bad_hex
                .iter()
                .any(|(f, m)| *f == "background" && m.contains("#rrggbb")),
            "{bad_hex:?}"
        );
    }

    #[test]
    fn selection_text_picks_white_or_black() {
        let dark_selection = check_colors(
            &colors("#ffffff", "#111111", "#0b63e5", "#0b63e5"),
            false,
            None,
        )
        .unwrap();
        assert_eq!(dark_selection.selection_fg, Rgb::WHITE);
        let light_selection = check_colors(
            &colors("#ffffff", "#111111", "#0b63e5", "#ffd60a"),
            false,
            None,
        )
        .unwrap();
        assert_eq!(light_selection.selection_fg, Rgb::BLACK);
        // 主按钮的文字跟着强调色走，不跟选中色：金色强调色上是黑字（白字只有 2:1 左右）。
        let gold = check_colors(
            &colors("#182030", "#f3efe6", "#e0b64a", "#9a2a22"),
            true,
            None,
        )
        .unwrap();
        assert_eq!(
            (gold.accent_fg, gold.selection_fg),
            (Rgb::BLACK, Rgb::WHITE)
        );
    }

    #[test]
    fn a_dark_image_under_a_light_set_is_rejected() {
        // 浅色套配一张深色图：压在图上的占位文字看不清。图的平均色按 IMAGE_SCRIM 混上底色估算。
        let light = colors("#fdf8f2", "#2b1d17", "#b3261e", "#b3261e");
        let dark_red = Tone {
            rgb: Rgb::parse("#7e1d1a").unwrap(),
            alpha: 1.0,
        };
        let cream = Tone {
            rgb: Rgb::parse("#f4d6cb").unwrap(),
            alpha: 1.0,
        };
        let problems = crate::i18n::with_lang(crate::i18n::Lang::En, || {
            check_colors(&light, false, Some(&dark_red))
                .err()
                .unwrap_or_default()
        });
        assert!(
            problems
                .iter()
                .any(|(f, m)| *f == "image" && m.contains("too dark")),
            "{problems:?}"
        );
        assert!(check_colors(&light, false, Some(&cream)).is_ok());
        // 几乎全透明的图不影响可读性。
        let faint = Tone {
            rgb: Rgb::BLACK,
            alpha: 0.05,
        };
        assert!(check_colors(&light, false, Some(&faint)).is_ok());
    }

    #[test]
    fn manifest_themes_are_validated() {
        let ok =
            format!(r#"[{{"id": "winter", "name": "Winter", "light": {LIGHT}, "dark": {DARK}}}]"#);
        assert_eq!(errors(&ok), "");
        let bad = format!(
            r#"[{{"id": "winter", "name": "Winter", "light": {LIGHT}, "dark": {LIGHT},
                 "dates": [{{"from": "12-20", "to": "2026-12-26"}}, {{"from": "13-01"}}]}}]"#
        );
        let errors = errors(&bad);
        assert!(
            errors.contains("themes[0].dark.background: the dark set needs"),
            "{errors}"
        );
        assert!(
            errors.contains("themes[0].dates[0]: from and to must use the same form"),
            "{errors}"
        );
        assert!(
            errors.contains("themes[0].dates[1]: from: \"13-01\" isn't a valid date"),
            "{errors}"
        );
        // 只有主题的插件是合法的；字段拼错、多出来的令牌都报错。
        let typo = format!(
            r#"[{{"id": "w", "name": "W", "light": {LIGHT}, "dark": {DARK}, "colour": 1}}]"#
        );
        assert!(super::super::manifest::parse(
            &format!(r#"{{"schemaVersion": 1, "id": "com.example.t", "name": "T", "version": "1.0.0", "author": "E", "themes": {typo}}}"#),
            None
        )
        .is_err());
    }

    #[test]
    fn manifest_theme_images_must_be_safe_supported_and_present() {
        let with_image = |image: &str| {
            format!(
                r#"[{{"id": "w", "name": "W", "light": {LIGHT}, "dark": {DARK}, "image": {image}}}]"#
            )
        };
        assert!(
            errors(&with_image(r#"{"light": "../x.png"}"#))
                .contains("relative to the plugin folder")
        );
        assert!(errors(&with_image(r#"{"light": "x.svg"}"#)).contains("supported image"));
        assert!(
            errors(&with_image(r#"{}"#)).contains("themes[0].image: needs light, dark or both")
        );
        // 有插件目录时检查文件在不在、多大、对比度。
        let dir = std::env::temp_dir().join(format!("helmp-theme-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = |image: &str| {
            format!(
                r#"{{"schemaVersion": 1, "id": "com.example.t", "name": "T", "version": "1.0.0", "author": "E", "themes": {}}}"#,
                with_image(image)
            )
        };
        let parse = |image: &str| {
            crate::i18n::with_lang(crate::i18n::Lang::En, || {
                manifest::parse(&manifest(image), Some(&dir))
                    .err()
                    .unwrap_or_default()
                    .join("\n")
            })
        };
        assert!(parse(r#"{"light": "missing.png"}"#).contains("image missing.png not found"));
        let png = |name: &str, rgb: [u8; 3]| {
            let mut bytes = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut bytes, 4, 4);
                encoder.set_color(png::ColorType::Rgb);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().unwrap();
                writer.write_image_data(&rgb.repeat(16)).unwrap();
            }
            std::fs::write(dir.join(name), bytes).unwrap();
        };
        png("light.png", [0xf4, 0xd6, 0xcb]);
        png("black.png", [0, 0, 0]);
        assert_eq!(parse(r#"{"light": "light.png"}"#), "");
        assert!(
            parse(r#"{"light": "black.png"}"#)
                .contains("themes[0].image.light: the image is too dark")
        );
        let huge = vec![0_u8; usize::try_from(manifest::MAX_THEME_IMAGE_BYTES).unwrap() + 1];
        std::fs::write(dir.join("huge.png"), huge).unwrap();
        assert!(parse(r#"{"light": "huge.png"}"#).contains("larger than 4 MiB"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn date_ranges_cover_both_ends_and_wrap_the_year() {
        let holidays = range("12-20", Some("12-26")).unwrap();
        assert_eq!(holidays.current(date(2026, 12, 19)), None);
        assert_eq!(
            holidays.current(date(2026, 12, 20)),
            Some(date(2026, 12, 26))
        );
        assert_eq!(
            holidays.current(date(2026, 12, 26)),
            Some(date(2026, 12, 26))
        );
        assert_eq!(holidays.current(date(2026, 12, 27)), None);
        // 每年重复。
        assert_eq!(
            holidays.current(date(2031, 12, 22)),
            Some(date(2031, 12, 26))
        );
        // 跨年：12-28 到 01-03，两边都算，结束日落在明年。
        let new_year = range("12-28", Some("01-03")).unwrap();
        assert_eq!(new_year.current(date(2026, 12, 28)), Some(date(2027, 1, 3)));
        assert_eq!(new_year.current(date(2026, 12, 31)), Some(date(2027, 1, 3)));
        assert_eq!(new_year.current(date(2027, 1, 1)), Some(date(2027, 1, 3)));
        assert_eq!(new_year.current(date(2027, 1, 3)), Some(date(2027, 1, 3)));
        assert_eq!(new_year.current(date(2027, 1, 4)), None);
        assert_eq!(new_year.current(date(2026, 12, 27)), None);
        // 缺省 to：只那一天。
        assert_eq!(
            range("05-20", None).unwrap().current(date(2026, 5, 20)),
            Some(date(2026, 5, 20))
        );
        assert_eq!(
            range("05-20", None).unwrap().current(date(2026, 5, 21)),
            None
        );
    }

    #[test]
    fn one_off_ranges_end_for_good() {
        let once = range("2026-12-20", Some("2027-01-02")).unwrap();
        assert_eq!(once.current(date(2026, 12, 31)), Some(date(2027, 1, 2)));
        assert_eq!(once.current(date(2027, 1, 2)), Some(date(2027, 1, 2)));
        // 下一年同一时间不再生效。
        assert_eq!(once.current(date(2027, 12, 25)), None);
        assert!(range("2027-01-02", Some("2026-12-20")).is_err());
        assert!(range("2027-02-29", None).is_err());
    }

    #[test]
    fn leap_day_ends_fall_on_feb_28_in_common_years() {
        // 只有 02-29 一天的主题：平年落到 2 月 28 日，而不是整整三年都不出现。
        let leap = range("02-29", None).unwrap();
        assert_eq!(leap.current(date(2028, 2, 29)), Some(date(2028, 2, 29)));
        assert_eq!(leap.current(date(2028, 2, 28)), None);
        assert_eq!(leap.current(date(2027, 2, 28)), Some(date(2027, 2, 28)));
        assert_eq!(leap.current(date(2027, 3, 1)), None);
        // 以 02-29 结尾的一段：平年在 2 月 28 日结束。
        let february = range("02-20", Some("02-29")).unwrap();
        assert_eq!(february.current(date(2027, 2, 28)), Some(date(2027, 2, 28)));
        assert_eq!(february.current(date(2027, 3, 1)), None);
        assert_eq!(february.current(date(2028, 2, 29)), Some(date(2028, 2, 29)));
    }

    #[test]
    fn the_day_follows_the_local_time_zone() {
        // 2026-12-19 20:00 UTC：东八区已经是 12 月 20 日（主题开始），洛杉矶还是 19 日。
        use chrono::TimeZone as _;
        let instant = chrono::Utc
            .with_ymd_and_hms(2026, 12, 19, 20, 0, 0)
            .unwrap();
        let holidays = range("12-20", Some("12-26")).unwrap();
        assert!(
            holidays
                .current(local_date(instant, &chrono_tz::Asia::Shanghai))
                .is_some()
        );
        assert!(
            holidays
                .current(local_date(instant, &chrono_tz::America::Los_Angeles))
                .is_none()
        );
    }

    #[test]
    fn describes_ranges_for_settings() {
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            assert_eq!(
                range("12-20", Some("12-26")).unwrap().describe(),
                "Every year 12-20 – 12-26"
            );
            assert_eq!(range("05-20", None).unwrap().describe(), "Every year 05-20");
            assert_eq!(
                range("2026-12-20", Some("2026-12-26")).unwrap().describe(),
                "2026-12-20 – 2026-12-26"
            );
        });
    }

    // ------------------------------------------------ 选哪个主题

    fn theme(key: &str, dates: &[(&str, &str)]) -> Available {
        let light = check_colors(
            &colors("#fdf8f2", "#2b1d17", "#b3261e", "#b3261e"),
            false,
            None,
        )
        .unwrap();
        let dark = check_colors(
            &colors("#182030", "#f3efe6", "#e0b64a", "#9a2a22"),
            true,
            None,
        )
        .unwrap();
        Available {
            key: key.into(),
            name: key.into(),
            plugin_name: "Example".into(),
            light,
            dark,
            image_light: None,
            image_dark: None,
            dates: dates
                .iter()
                .map(|(from, to)| range(from, Some(to)).unwrap())
                .collect(),
        }
    }

    fn config(selected: Option<&str>, by_date: bool, skip: Option<&str>) -> Config {
        let mut config = Config::default();
        config.theme.selected = selected.map(Into::into);
        config.theme.by_date = by_date;
        config.theme.skip_date = skip.map(Into::into);
        config
    }

    fn active_key(active: Option<Active>) -> Option<String> {
        active.map(|active| active.theme.key)
    }

    #[test]
    fn a_dated_theme_takes_over_on_its_days_and_hands_back_after() {
        let available = [
            theme("com.example.a/plain", &[]),
            theme("com.example.b/winter", &[("12-20", "12-26")]),
        ];
        let config = config(Some("com.example.a/plain"), true, None);
        // 平常：用户自己选的。
        assert_eq!(
            active_key(resolve(&config, &available, date(2026, 12, 19))).as_deref(),
            Some("com.example.a/plain")
        );
        // 日期内：自动换上，并告诉设置页哪天结束。
        let during = resolve(&config, &available, date(2026, 12, 24)).unwrap();
        assert_eq!(during.theme.key, "com.example.b/winter");
        assert!(during.dated);
        assert_eq!(during.until, Some(date(2026, 12, 26)));
        // 过后：恢复用户选的，选择本身从来没被改写。
        let after = resolve(&config, &available, date(2026, 12, 27)).unwrap();
        assert_eq!(after.theme.key, "com.example.a/plain");
        assert!(!after.dated);
        assert_eq!(
            config.theme.selected.as_deref(),
            Some("com.example.a/plain")
        );
    }

    #[test]
    fn users_can_turn_dates_off_or_skip_just_today() {
        let available = [theme("com.example.b/winter", &[("12-20", "12-26")])];
        let day = date(2026, 12, 24);
        // 关掉按日期切换：回到默认（用户没选别的主题）。
        assert_eq!(resolve(&config(None, false, None), &available, day), None);
        // 「今天不用」只管今天，明天照常。
        let skip = config(None, true, Some("2026-12-24"));
        assert_eq!(resolve(&skip, &available, day), None);
        assert!(resolve(&skip, &available, date(2026, 12, 25)).is_some());
        // 用户手动选了这个日期主题：不管日期都用它。
        assert!(
            resolve(
                &config(Some("com.example.b/winter"), false, None),
                &available,
                date(2026, 6, 1)
            )
            .is_some()
        );
    }

    #[test]
    fn a_theme_from_a_disabled_or_removed_plugin_falls_back_to_the_default() {
        // 选中的主题不在可选列表里（插件停用 / 卸载 / 新版去掉了这个主题）：用默认配色，不留读不出的界面。
        let config = config(Some("com.example.gone/winter"), true, None);
        assert_eq!(resolve(&config, &[], date(2026, 12, 24)), None);
        assert_eq!(selected_available(&config, &[]), None);
        let available = [theme("com.example.gone/winter", &[])];
        assert_eq!(
            selected_available(&config, &available).as_deref(),
            Some("com.example.gone/winter")
        );
    }

    #[test]
    fn several_dated_themes_pick_the_first_plugin_id() {
        let available = [
            theme("com.example.a/spring", &[("03-20", "04-10")]),
            theme("com.example.b/april", &[("04-01", "04-30")]),
        ];
        let config = config(None, true, None);
        assert_eq!(
            active_key(resolve(&config, &available, date(2026, 4, 5))).as_deref(),
            Some("com.example.a/spring")
        );
        assert_eq!(
            active_key(resolve(&config, &available, date(2026, 4, 20))).as_deref(),
            Some("com.example.b/april")
        );
    }

    #[test]
    fn only_enabled_plugins_provide_themes() {
        // 走真正的插件列表：插件停用后它的主题不再可选，当前主题回到默认。
        let text = format!(
            r#"{{"schemaVersion": 1, "id": "com.example.themes", "name": "Themes", "version": "1.0.0",
                "author": "Example", "themes": [{{"id": "winter", "name": "Winter", "light": {LIGHT}, "dark": {DARK}}}]}}"#
        );
        let dir = std::env::temp_dir().join(format!("helmp-theme-entries-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.json"), text).unwrap();
        let entry = registry::load_dir(&dir, registry::Source::User);
        let mut config = config(Some("com.example.themes/winter"), true, None);
        assert!(
            available(&config, std::slice::from_ref(&entry)).is_empty(),
            "未启用（未确认权限）时不提供主题"
        );
        registry::set_enabled(&mut config, &entry, true);
        let list = available(&config, std::slice::from_ref(&entry));
        assert_eq!(list.len(), 1);
        assert_eq!(
            active_key(resolve(&config, &list, date(2026, 1, 1))).as_deref(),
            Some("com.example.themes/winter")
        );
        registry::set_enabled(&mut config, &entry, false);
        let list = available(&config, std::slice::from_ref(&entry));
        assert_eq!(resolve(&config, &list, date(2026, 1, 1)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_example_plugin_validates() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/plugins/com.example.seasonal-themes");
        let text = std::fs::read_to_string(dir.join("plugin.json")).unwrap();
        let manifest =
            manifest::parse(&text, Some(&dir)).unwrap_or_else(|errors| panic!("{errors:?}"));
        assert_eq!(manifest.themes.len(), 2);
    }
}
