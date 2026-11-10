//! 颜色的解析与各种写法。分量都是 sRGB 0–1。

/// sRGB 颜色，分量 0–1。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

fn byte(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// 三位小数，SwiftUI / NSColor 写法用。
fn unit(value: f64) -> String {
    format!("{:.3}", value.clamp(0.0, 1.0))
}

/// 去掉多余的 0：`0.50` → `0.5`，`1.0` → `1`。
fn trim(text: String) -> String {
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

impl Rgba {
    pub fn bytes(&self) -> [u8; 4] {
        [byte(self.r), byte(self.g), byte(self.b), byte(self.a)]
    }

    fn opaque(&self) -> bool {
        byte(self.a) == 255
    }

    pub fn hex(&self) -> String {
        let [r, g, b, a] = self.bytes();
        if self.opaque() {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }

    pub fn css_rgb(&self) -> String {
        let [r, g, b, _] = self.bytes();
        if self.opaque() {
            format!("rgb({r}, {g}, {b})")
        } else {
            format!("rgba({r}, {g}, {b}, {})", trim(format!("{:.2}", self.a)))
        }
    }

    /// (色相 0–360, 饱和度 0–100, 亮度 0–100)
    pub fn hsl(&self) -> (f64, f64, f64) {
        let (r, g, b) = (self.r, self.g, self.b);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let light = (max + min) / 2.0;
        let delta = max - min;
        if delta == 0.0 {
            return (0.0, 0.0, light * 100.0);
        }
        let saturation = delta / (1.0 - (2.0 * light - 1.0).abs());
        let hue = if max == r {
            60.0 * ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        (hue, saturation * 100.0, light * 100.0)
    }

    pub fn css_hsl(&self) -> String {
        let (h, s, l) = self.hsl();
        let (h, s, l) = (h.round(), s.round(), l.round());
        if self.opaque() {
            format!("hsl({h}, {s}%, {l}%)")
        } else {
            format!("hsla({h}, {s}%, {l}%, {})", trim(format!("{:.2}", self.a)))
        }
    }

    /// OKLCH（Björn Ottosson 的 OKLab 变换）。
    pub fn oklch(&self) -> (f64, f64, f64) {
        let linear = |c: f64| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (linear(self.r), linear(self.g), linear(self.b));
        let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
        let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
        let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
        let lightness = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s;
        let a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
        let bb = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
        let chroma = (a * a + bb * bb).sqrt();
        let hue = if chroma < 1e-4 {
            0.0
        } else {
            bb.atan2(a).to_degrees().rem_euclid(360.0)
        };
        (lightness, chroma, hue)
    }

    pub fn css_oklch(&self) -> String {
        let (l, c, h) = self.oklch();
        let body = format!(
            "{} {} {}",
            trim(format!("{l:.4}")),
            trim(format!("{c:.4}")),
            trim(format!("{h:.2}"))
        );
        if self.opaque() {
            format!("oklch({body})")
        } else {
            format!("oklch({body} / {})", trim(format!("{:.2}", self.a)))
        }
    }

    pub fn swiftui(&self) -> String {
        let base = format!(
            "Color(red: {}, green: {}, blue: {}",
            unit(self.r),
            unit(self.g),
            unit(self.b)
        );
        if self.opaque() {
            format!("{base})")
        } else {
            format!("{base}, opacity: {})", unit(self.a))
        }
    }

    pub fn nscolor(&self) -> String {
        format!(
            "NSColor(srgbRed: {}, green: {}, blue: {}, alpha: {})",
            unit(self.r),
            unit(self.g),
            unit(self.b),
            unit(self.a)
        )
    }
}

/// `#rgb`、`#rgba`、`#rrggbb`、`#rrggbbaa`、`rgb()/rgba()`（逗号或空格分隔、可带 `/ alpha`、百分比）、
/// `hsl()/hsla()`。别的都不算颜色（`ff8800` 不带 `#` 不算，免得和搜索词混淆）。
pub fn parse(text: &str) -> Option<Rgba> {
    let text = text.trim().to_lowercase();
    if let Some(hex) = text.strip_prefix('#') {
        return parse_hex(hex);
    }
    let (name, args) = text.strip_suffix(')')?.split_once('(')?;
    let parts: Vec<&str> = args
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect();
    if !(3..=4).contains(&parts.len()) {
        return None;
    }
    let alpha = match parts.get(3) {
        Some(part) => fraction(part, 1.0)?,
        None => 1.0,
    };
    match name.trim() {
        "rgb" | "rgba" => Some(Rgba {
            r: fraction(parts.first()?, 255.0)?,
            g: fraction(parts.get(1)?, 255.0)?,
            b: fraction(parts.get(2)?, 255.0)?,
            a: alpha,
        }),
        "hsl" | "hsla" => {
            let hue: f64 = parts.first()?.trim_end_matches("deg").parse().ok()?;
            let saturation = fraction(parts.get(1)?, 100.0)?;
            let lightness = fraction(parts.get(2)?, 100.0)?;
            let (r, g, b) = hsl_to_rgb(hue, saturation, lightness);
            Some(Rgba { r, g, b, a: alpha })
        }
        _ => None,
    }
}

/// 分量：`50%` 按百分比，否则除以 `scale`（rgb 是 255，hsl 的饱和度亮度不带 % 时按 0–100）。超出范围不算。
fn fraction(part: &str, scale: f64) -> Option<f64> {
    let value = match part.strip_suffix('%') {
        Some(percent) => percent.parse::<f64>().ok()? / 100.0,
        None => part.parse::<f64>().ok()? / scale,
    };
    (0.0..=1.0).contains(&value).then_some(value)
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(hex.get(i..=i)?, 16).ok();
    let pair = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    let channels: Vec<u8> = match hex.len() {
        3 | 4 => (0..hex.len())
            .map(|i| digit(i).map(|d| d * 17))
            .collect::<Option<_>>()?,
        6 | 8 => (0..hex.len() / 2)
            .map(|i| pair(i * 2))
            .collect::<Option<_>>()?,
        _ => return None,
    };
    let channel = |i: usize| channels.get(i).map(|v| f64::from(*v) / 255.0);
    Some(Rgba {
        r: channel(0)?,
        g: channel(1)?,
        b: channel(2)?,
        a: channel(3).unwrap_or(1.0),
    })
}

fn hsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> (f64, f64, f64) {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = chroma * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    (r + m, g + m, b + m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_rgb_and_hsl_inputs_agree() {
        // 同一个颜色用三种写法输入，转出来的 HEX 必须一样，否则设计稿和代码对不上。
        for text in [
            "#ff8800",
            "#F80",
            "rgb(255, 136, 0)",
            "rgb(255 136 0)",
            "rgb(100%, 53.33%, 0%)",
            "hsl(32, 100%, 50%)",
        ] {
            assert_eq!(
                parse(text).map(|c| c.hex()).as_deref(),
                Some("#ff8800"),
                "{text}"
            );
        }
        let translucent = parse("rgba(51, 102, 255, 0.5)").unwrap();
        assert_eq!(translucent.hex(), "#3366ff80");
        assert_eq!(translucent.css_rgb(), "rgba(51, 102, 255, 0.5)");
        assert_eq!(
            parse("#3366ff80").unwrap().css_hsl(),
            "hsla(225, 100%, 60%, 0.5)"
        );
    }

    #[test]
    fn rejects_things_that_are_not_colors() {
        for text in [
            "ff8800",
            "#ggg",
            "#12345",
            "rgb(300, 0, 0)",
            "rgb(1, 2)",
            "hello(1, 2, 3)",
            "rgb",
        ] {
            assert!(parse(text).is_none(), "{text}");
        }
    }

    #[test]
    fn code_formats_use_three_decimals() {
        let color = parse("#3366ff").unwrap();
        assert_eq!(
            color.swiftui(),
            "Color(red: 0.200, green: 0.400, blue: 1.000)"
        );
        assert_eq!(
            color.nscolor(),
            "NSColor(srgbRed: 0.200, green: 0.400, blue: 1.000, alpha: 1.000)"
        );
        assert_eq!(parse("#ffffff").unwrap().css_oklch(), "oklch(1 0 0)");
        // CSS Color 4 规范给出的纯红参考值。
        assert_eq!(
            parse("#ff0000").unwrap().css_oklch(),
            "oklch(0.628 0.2577 29.23)"
        );
    }
}
