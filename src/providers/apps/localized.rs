//! 读取 .app 的显示名：Info.plist 与中文本地化名。
//!
//! 中文名有两个来源：
//! - `Contents/Resources/<zh>.lproj/InfoPlist.strings`：第三方应用常见。文件可能是二进制 plist、
//!   XML plist，或 `"key" = "value";` 文本（UTF-8 或 UTF-16），三种都要认。
//! - `Contents/Resources/InfoPlist.loctable`：macOS 系统应用（计算器、日历…）的新格式，
//!   一个按语言分组的二进制 plist：`{ "zh_CN": { "CFBundleDisplayName": "计算器" } }`。

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

use anyhow::{Context as _, Result, bail};

/// 按优先级排列的简体中文语言目录名。
const ZH_LOCALES: [&str; 5] = ["zh-Hans", "zh_CN", "zh-CN", "zh_Hans", "zh"];
/// 同一份本地化里，显示名优先于 bundle 名。
const NAME_KEYS: [&str; 2] = ["CFBundleDisplayName", "CFBundleName"];

/// Info.plist 里与搜索、详情有关的字段。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BundleInfo {
    /// 英文名（显示名、bundle 名，去重）。
    pub names: Vec<String>,
    /// `CFBundleShortVersionString`，如 `4.1.2`。
    pub version: Option<String>,
}

/// 读 Info.plist。没有 Info.plist 返回空。
pub fn bundle_info(app: &Path) -> Result<BundleInfo> {
    let path = app.join("Contents/Info.plist");
    if !path.exists() {
        return Ok(BundleInfo::default());
    }
    let value =
        plist::Value::from_file(&path).with_context(|| format!("解析 {}", path.display()))?;
    let dict = value
        .as_dictionary()
        .with_context(|| format!("{} 顶层不是字典", path.display()))?;
    let mut names: Vec<String> = Vec::new();
    for key in NAME_KEYS {
        if let Some(name) = dict.get(key).and_then(plist::Value::as_string) {
            let name = name.trim();
            if !name.is_empty() && !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
        }
    }
    let version = dict
        .get("CFBundleShortVersionString")
        .and_then(plist::Value::as_string)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    Ok(BundleInfo { names, version })
}

/// 简体中文本地化名。没有中文本地化返回 `Ok(None)`；文件存在但解析失败返回 `Err`。
pub fn chinese_name(app: &Path) -> Result<Option<String>> {
    let resources = app.join("Contents/Resources");
    for locale in ZH_LOCALES {
        let path = resources.join(format!("{locale}.lproj/InfoPlist.strings"));
        if !path.exists() {
            continue;
        }
        let bytes = std::fs::read(&path).with_context(|| format!("读取 {}", path.display()))?;
        let table = parse_strings(&bytes).with_context(|| format!("解析 {}", path.display()))?;
        if let Some(name) = pick_name(&table) {
            return Ok(Some(name));
        }
    }

    let loctable = resources.join("InfoPlist.loctable");
    if loctable.exists() {
        let value = plist::Value::from_file(&loctable)
            .with_context(|| format!("解析 {}", loctable.display()))?;
        return Ok(loctable_name(&value));
    }
    Ok(None)
}

fn loctable_name(value: &plist::Value) -> Option<String> {
    let locales = value.as_dictionary()?;
    ZH_LOCALES.iter().find_map(|locale| {
        let table = string_map(locales.get(locale)?.as_dictionary()?);
        pick_name(&table)
    })
}

fn pick_name(table: &HashMap<String, String>) -> Option<String> {
    NAME_KEYS
        .iter()
        .filter_map(|key| table.get(*key))
        .map(|name| name.trim())
        .find(|name| !name.is_empty())
        .map(str::to_string)
}

fn string_map(dict: &plist::Dictionary) -> HashMap<String, String> {
    dict.iter()
        .filter_map(|(key, value)| Some((key.clone(), value.as_string()?.to_string())))
        .collect()
}

/// 解析 `.strings` 文件的三种格式：二进制 plist、XML plist、文本（UTF-8 / UTF-16）。
pub fn parse_strings(bytes: &[u8]) -> Result<HashMap<String, String>> {
    if bytes.starts_with(b"bplist") {
        let value = plist::Value::from_reader(Cursor::new(bytes))?;
        let dict = value.as_dictionary().context("二进制 plist 顶层不是字典")?;
        return Ok(string_map(dict));
    }
    let text = decode_text(bytes)?;
    if text.trim_start().starts_with('<') {
        let value = plist::Value::from_reader_xml(text.as_bytes())?;
        let dict = value.as_dictionary().context("XML plist 顶层不是字典")?;
        return Ok(string_map(dict));
    }
    parse_strings_text(&text)
}

/// 按 BOM 判断编码；没有 BOM 时用零字节位置猜 UTF-16 的字节序。
fn decode_text(bytes: &[u8]) -> Result<String> {
    let utf16 = |rest: &[u8], little: bool| -> Result<String> {
        let (pairs, odd) = rest.as_chunks::<2>();
        if !odd.is_empty() {
            bail!("UTF-16 文本字节数为奇数");
        }
        let units: Vec<u16> = pairs
            .iter()
            .map(|&pair| {
                if little {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            })
            .collect();
        Ok(String::from_utf16(&units)?)
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => Ok(std::str::from_utf8(rest)?.to_string()),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [first, 0, ..] if *first != 0 => utf16(bytes, true),
        [0, second, ..] if *second != 0 => utf16(bytes, false),
        _ => Ok(std::str::from_utf8(bytes)?.to_string()),
    }
}

/// 解析 `"key" = "value";` 形式的文本 strings，支持 `/* */` 与 `//` 注释、未加引号的键、
/// 只有键的条目（值等于键），以及 `\n \t \" \\ \UXXXX` 等转义。
pub fn parse_strings_text(text: &str) -> Result<HashMap<String, String>> {
    let mut parser = TextParser {
        chars: text.chars().collect(),
        pos: 0,
    };
    let mut table = HashMap::new();
    loop {
        parser.skip_trivia()?;
        if parser.peek().is_none() {
            return Ok(table);
        }
        let key = parser.token()?;
        parser.skip_trivia()?;
        let value = match parser.next() {
            Some('=') => {
                parser.skip_trivia()?;
                let value = parser.token()?;
                parser.skip_trivia()?;
                parser.expect(';')?;
                value
            }
            Some(';') => key.clone(),
            other => bail!("第 {} 个字符处应为 '=' 或 ';'，实际 {other:?}", parser.pos),
        };
        table.insert(key, value);
    }
}

struct TextParser {
    chars: Vec<char>,
    pos: usize,
}

impl TextParser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn next(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    fn expect(&mut self, want: char) -> Result<()> {
        match self.next() {
            Some(ch) if ch == want => Ok(()),
            other => bail!("第 {} 个字符处应为 {want:?}，实际 {other:?}", self.pos),
        }
    }

    fn skip_trivia(&mut self) -> Result<()> {
        loop {
            match (self.peek(), self.peek_at(1)) {
                (Some(ch), _) if ch.is_whitespace() || ch == '\u{FEFF}' => self.pos += 1,
                (Some('/'), Some('/')) => while self.next().is_some_and(|ch| ch != '\n') {},
                (Some('/'), Some('*')) => {
                    self.pos += 2;
                    loop {
                        match (self.next(), self.peek()) {
                            (Some('*'), Some('/')) => {
                                self.pos += 1;
                                break;
                            }
                            (None, _) => bail!("注释没有闭合"),
                            _ => {}
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn token(&mut self) -> Result<String> {
        match self.peek() {
            Some('"') => {
                self.pos += 1;
                self.quoted()
            }
            Some(ch) if is_bare(ch) => {
                let mut out = String::new();
                while let Some(ch) = self.peek().filter(|ch| is_bare(*ch)) {
                    out.push(ch);
                    self.pos += 1;
                }
                Ok(out)
            }
            other => bail!("第 {} 个字符处应为字符串，实际 {other:?}", self.pos),
        }
    }

    fn quoted(&mut self) -> Result<String> {
        let mut out = String::new();
        // UTF-16 代理对要两段 \U 拼起来，先攒成 u16 再解码。
        let mut pending: Vec<u16> = Vec::new();
        loop {
            let ch = self.next().context("字符串没有闭合")?;
            if ch == '\\' && matches!(self.peek(), Some('U' | 'u')) {
                self.pos += 1;
                let hex: String = (0..4).filter_map(|_| self.next()).collect();
                let unit = u16::from_str_radix(&hex, 16)
                    .with_context(|| format!("无效的 \\U 转义：{hex}"))?;
                pending.push(unit);
                continue;
            }
            if !pending.is_empty() {
                out.push_str(&String::from_utf16(&pending)?);
                pending.clear();
            }
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let escaped = self.next().context("转义没有结束")?;
                    match escaped {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        'a' => out.push('\u{7}'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'v' => out.push('\u{b}'),
                        '0'..='7' => {
                            let mut code = escaped.to_digit(8).unwrap_or(0);
                            for _ in 0..2 {
                                match self.peek().and_then(|c| c.to_digit(8)) {
                                    Some(digit) => {
                                        code = code * 8 + digit;
                                        self.pos += 1;
                                    }
                                    None => break,
                                }
                            }
                            out.push(char::from_u32(code).context("无效的八进制转义")?);
                        }
                        other => out.push(other),
                    }
                }
                other => out.push(other),
            }
        }
    }
}

/// 不加引号的键/值允许的字符（与 Foundation 的 OpenStep 解析一致）。
fn is_bare(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '.' | ':' | '/' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name_plist() -> plist::Value {
        let mut dict = plist::Dictionary::new();
        dict.insert("CFBundleName".into(), "飞书".into());
        dict.insert("CFBundleDisplayName".into(), "飞书".into());
        dict.insert("NSCameraUsageDescription".into(), "访问摄像头".into());
        plist::Value::Dictionary(dict)
    }

    fn utf16(text: &str, little: bool) -> Vec<u8> {
        let mut bytes = if little {
            vec![0xFF, 0xFE]
        } else {
            vec![0xFE, 0xFF]
        };
        for unit in text.encode_utf16() {
            let pair = if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            };
            bytes.extend(pair);
        }
        bytes
    }

    // 三种格式在真实机器上都存在（Safari 二进制、微信 XML、飞书 UTF-8 文本、老应用 UTF-16 文本），
    // 任何一种解析不了，那个应用就只能用英文名搜到，中文用户打「微信」就找不到。

    #[test]
    fn parses_binary_plist_strings() {
        let mut bytes = Vec::new();
        name_plist().to_writer_binary(&mut bytes).unwrap();
        let table = parse_strings(&bytes).unwrap();
        assert_eq!(pick_name(&table).as_deref(), Some("飞书"));
    }

    #[test]
    fn parses_xml_plist_strings() {
        let mut bytes = Vec::new();
        name_plist().to_writer_xml(&mut bytes).unwrap();
        let table = parse_strings(&bytes).unwrap();
        assert_eq!(
            table.get("CFBundleDisplayName").map(String::as_str),
            Some("飞书")
        );
    }

    #[test]
    fn parses_text_strings_in_utf8_and_utf16() {
        let text = "/* 本地化 */\n\"CFBundleName\" = \"微信\";\n// 行注释\nCFBundleDisplayName = \"微信\";\n";
        for bytes in [
            text.as_bytes().to_vec(),
            utf16(text, true),
            utf16(text, false),
            // 有的文件不带 BOM，只能靠零字节猜字节序。
            utf16(text, true).split_off(2),
        ] {
            let table = parse_strings(&bytes).unwrap();
            assert_eq!(pick_name(&table).as_deref(), Some("微信"));
        }
    }

    #[test]
    fn text_strings_handle_escapes() {
        // 系统应用常把中文写成 \U 转义（含代理对），不解码就会显示成乱码。
        let table = parse_strings_text(
            r#""a" = "\U8ba1\U7b97\U5668"; "b" = "say \"hi\"\n"; "c" = "\Ud83d\Ude00"; "k";"#,
        )
        .unwrap();
        assert_eq!(table["a"], "计算器");
        assert_eq!(table["b"], "say \"hi\"\n");
        assert_eq!(table["c"], "😀");
        assert_eq!(table["k"], "k");
    }

    #[test]
    fn malformed_text_is_an_error() {
        // 解析失败要报出来，而不是悄悄当成「没有中文名」。
        assert!(parse_strings_text("\"a\" = \"b\"").is_err());
        assert!(parse_strings_text("\"a\" = \"b").is_err());
    }

    #[test]
    fn display_name_wins_over_bundle_name() {
        // 显示名是 Finder/Dock 里看到的名字，bundle 名常是内部代号（飞书的 CFBundleName 是 Feishu）。
        let table =
            parse_strings_text(r#""CFBundleName" = "内部名"; "CFBundleDisplayName" = "显示名";"#)
                .unwrap();
        assert_eq!(pick_name(&table).as_deref(), Some("显示名"));
    }

    #[test]
    fn reads_system_app_loctable() {
        // 计算器等系统应用没有 zh_CN.lproj/InfoPlist.strings，中文名只在 loctable 里。
        let mut zh = plist::Dictionary::new();
        zh.insert("CFBundleDisplayName".into(), "计算器".into());
        let mut en = plist::Dictionary::new();
        en.insert("CFBundleDisplayName".into(), "Calculator".into());
        let mut root = plist::Dictionary::new();
        root.insert("en".into(), plist::Value::Dictionary(en));
        root.insert("zh_CN".into(), plist::Value::Dictionary(zh));
        let name = loctable_name(&plist::Value::Dictionary(root));
        assert_eq!(name.as_deref(), Some("计算器"));
    }

    #[test]
    fn reads_chinese_name_from_bundle_on_disk() {
        let app =
            std::env::temp_dir().join(format!("helmp-test-{}/飞书测试.app", std::process::id()));
        let lproj = app.join("Contents/Resources/zh_CN.lproj");
        std::fs::create_dir_all(&lproj).unwrap();
        std::fs::write(
            lproj.join("InfoPlist.strings"),
            "\"CFBundleDisplayName\" = \"飞书\";",
        )
        .unwrap();
        let name = chinese_name(&app);
        std::fs::remove_dir_all(app.parent().unwrap()).unwrap();
        assert_eq!(name.unwrap().as_deref(), Some("飞书"));
    }
}
