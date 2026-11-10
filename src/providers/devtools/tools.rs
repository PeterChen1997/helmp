//! 开发者小工具的纯函数：生成（UUID、密码、假文）与转换（哈希、Base64、URL、JSON、命名、进制、JWT）。
//! 都不碰界面和系统，单测直接覆盖。

use std::fmt::Write as _;

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;

use crate::i18n::{tr, trf};

// ---------------------------------------------------------------- 随机

/// 系统 CSPRNG（`SecRandomCopyBytes`）。
fn random_bytes(buf: &mut [u8]) {
    if let Err(err) = security_framework::random::SecRandom::default().copy_bytes(buf) {
        // 系统随机数源失败几乎不可能；真发生时宁可崩溃也不能给出可预测的密码。
        panic!("SecRandomCopyBytes failed: {err}");
    }
}

/// `[0, bound)` 里的均匀随机数（拒绝采样，没有取模偏差）。
fn random_below(bound: u32) -> u32 {
    let limit = u32::MAX - u32::MAX % bound;
    loop {
        let mut buf = [0u8; 4];
        random_bytes(&mut buf);
        let value = u32::from_le_bytes(buf);
        if value < limit {
            return value % bound;
        }
    }
}

pub fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn uuid_v7() -> String {
    uuid::Uuid::now_v7().to_string()
}

const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &str = "0123456789";
const SYMBOLS: &str = "!@#$%^&*-_=+?";

pub const DEFAULT_PASSWORD_LENGTH: usize = 20;

/// 随机密码：每类字符至少一个（长度 ≥ 类数时），整体均匀抽取。返回密码和熵（比特）。
pub fn password(length: usize, symbols: bool) -> (String, f64) {
    let mut classes = vec![LOWER, UPPER, DIGITS];
    if symbols {
        classes.push(SYMBOLS);
    }
    let alphabet: Vec<char> = classes.iter().flat_map(|c| c.chars()).collect();
    let entropy = length as f64 * (alphabet.len() as f64).log2();
    loop {
        let text: String = (0..length)
            .filter_map(|_| alphabet.get(random_below(alphabet.len() as u32) as usize))
            .collect();
        let complete = length < classes.len()
            || classes
                .iter()
                .all(|class| text.chars().any(|c| class.contains(c)));
        if complete {
            return (text, entropy);
        }
    }
}

// ---------------------------------------------------------------- 哈希与编码

pub fn md5(text: &str) -> String {
    use md5::Digest as _;
    hex(&md5::Md5::digest(text.as_bytes()))
}

pub fn sha1(text: &str) -> String {
    use sha1::Digest as _;
    hex(&sha1::Sha1::digest(text.as_bytes()))
}

pub fn sha256(text: &str) -> String {
    use sha2::Digest as _;
    hex(&sha2::Sha256::digest(text.as_bytes()))
}

pub fn sha512(text: &str) -> String {
    use sha2::Digest as _;
    hex(&sha2::Sha512::digest(text.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

pub fn base64_encode(text: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
}

/// 标准与 URL 安全两种字母表、有无补位都接受；解出来必须是 UTF-8 文本。
pub fn base64_decode(text: &str) -> Option<String> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let bytes = [STANDARD, URL_SAFE, STANDARD_NO_PAD, URL_SAFE_NO_PAD]
        .iter()
        .find_map(|engine| engine.decode(text).ok())?;
    String::from_utf8(bytes).ok()
}

/// 百分号编码（URL 组件）：只保留 RFC 3986 的非保留字符。
pub fn url_encode(text: &str) -> String {
    text.bytes().fold(String::new(), |mut out, byte| {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
        out
    })
}

/// 解百分号编码；`%` 后不是两位十六进制或解出来不是 UTF-8 时返回 `None`。
pub fn url_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        if byte == b'%' {
            let pair = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(pair, 16).ok()?);
            i += 3;
        } else {
            out.push(byte);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

// ---------------------------------------------------------------- JSON

/// 校验 JSON；出错时说清楚第几行第几列。
pub fn json_check(text: &str) -> Result<()> {
    serde_json::from_str::<serde::de::IgnoredAny>(text)
        .map(|_| ())
        .map_err(|err| {
            anyhow::anyhow!(trf!(
                "JSON 无效：第 {} 行第 {} 列，{err}",
                "Invalid JSON: line {}, column {}: {err}",
                err.line(),
                err.column()
            ))
        })
}

/// 格式化（两空格缩进）或压缩。按字符重排空白，不经过 Map：键的顺序和数字的写法保持原样。
pub fn json_format(text: &str, pretty: bool) -> Result<String> {
    json_check(text)?;
    let mut out = String::with_capacity(text.len() * 2);
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut chars = text.chars().peekable();
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    };
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            c if c.is_whitespace() => {}
            '{' | '[' => {
                out.push(c);
                // 空对象、空数组保持 `{}`、`[]`。
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                if matches!(chars.peek(), Some('}' | ']')) {
                    if let Some(close) = chars.next() {
                        out.push(close);
                    }
                } else {
                    depth += 1;
                    if pretty {
                        newline(&mut out, depth);
                    }
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                if pretty {
                    newline(&mut out, depth);
                }
                out.push(c);
            }
            ',' => {
                out.push(c);
                if pretty {
                    newline(&mut out, depth);
                }
            }
            ':' => {
                out.push(':');
                if pretty {
                    out.push(' ');
                }
            }
            c => out.push(c),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- 命名风格

/// 把 `helloWorld`、`HTTPServer`、`snake_case`、`kebab-case`、`Some words` 拆成单词。
pub fn words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (index, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        let prev = index.checked_sub(1).and_then(|i| chars.get(i)).copied();
        let next = chars.get(index + 1).copied();
        let boundary = c.is_uppercase()
            && prev.is_some_and(|p| {
                p.is_lowercase()
                    || p.is_ascii_digit()
                    || (p.is_uppercase() && next.is_some_and(char::is_lowercase))
            });
        if boundary && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        current.push(c);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

/// 各种命名风格：(键, 显示名, 结果)。
pub fn cases(text: &str) -> Vec<(&'static str, &'static str, String)> {
    let words = words(text);
    if words.is_empty() {
        return Vec::new();
    }
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let camel = lower
        .iter()
        .enumerate()
        .map(|(i, w)| if i == 0 { w.clone() } else { capitalize(w) })
        .collect::<String>();
    vec![
        ("camel", "camelCase", camel),
        (
            "pascal",
            "PascalCase",
            lower.iter().map(|w| capitalize(w)).collect(),
        ),
        ("snake", "snake_case", lower.join("_")),
        ("kebab", "kebab-case", lower.join("-")),
        ("constant", "CONSTANT_CASE", lower.join("_").to_uppercase()),
        (
            "title",
            "Title Case",
            lower
                .iter()
                .map(|w| capitalize(w))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        ("upper", "UPPER CASE", text.to_uppercase()),
        ("lower", "lower case", text.to_lowercase()),
    ]
}

// ---------------------------------------------------------------- 进制

/// 读一个整数：`0x` / `0b` / `0o` 前缀按对应进制，`hex_hint` 为真时无前缀且含 a–f 的按十六进制，
/// 其余按十进制。允许 `_` 分隔。
pub fn parse_integer(text: &str, hex_hint: bool) -> Option<u128> {
    let text = text.trim().replace('_', "").to_lowercase();
    let (digits, radix) = if let Some(rest) = text.strip_prefix("0x") {
        (rest.to_string(), 16)
    } else if let Some(rest) = text.strip_prefix("0b") {
        (rest.to_string(), 2)
    } else if let Some(rest) = text.strip_prefix("0o") {
        (rest.to_string(), 8)
    } else if hex_hint && text.chars().any(|c| ('a'..='f').contains(&c)) {
        (text.clone(), 16)
    } else {
        (text.clone(), 10)
    };
    if digits.is_empty() {
        return None;
    }
    u128::from_str_radix(&digits, radix).ok()
}

/// (键, 显示名, 写法)：十进制、十六进制、二进制、八进制。
pub fn bases(value: u128) -> Vec<(&'static str, &'static str, String)> {
    vec![
        ("dec", tr("十进制", "Decimal"), value.to_string()),
        ("hex", tr("十六进制", "Hexadecimal"), format!("0x{value:x}")),
        ("bin", tr("二进制", "Binary"), format!("0b{value:b}")),
        ("oct", tr("八进制", "Octal"), format!("0o{value:o}")),
    ]
}

// ---------------------------------------------------------------- JWT

/// 解出的 JWT（不校验签名）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jwt {
    pub header: String,
    pub payload: String,
    /// `exp` / `iat` / `nbf` 这类时间声明：(名字, Unix 秒)。
    pub times: Vec<(String, i64)>,
}

pub fn looks_like_jwt(text: &str) -> bool {
    let text = text.trim();
    text.starts_with("eyJ")
        && text.split('.').count() == 3
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.=".contains(c))
}

pub fn jwt_decode(token: &str) -> Result<Jwt> {
    let mut parts = token.trim().split('.');
    let (Some(header), Some(payload)) = (parts.next(), parts.next()) else {
        bail!(tr(
            "JWT 应是用点分开的三段",
            "A JWT has three dot-separated parts"
        ));
    };
    let decode = |part: &str, name: &str| -> Result<String> {
        let text = base64_decode(part).with_context(|| {
            trf!(
                "{name} 不是 Base64URL 编码的文本",
                "{name} isn’t Base64URL text"
            )
        })?;
        json_format(&text, true)
    };
    let header = decode(header, "header")?;
    let payload = decode(payload, "payload")?;
    let claims: serde_json::Value = serde_json::from_str(&payload)?;
    let times = ["exp", "iat", "nbf", "auth_time"]
        .iter()
        .filter_map(|name| Some((name.to_string(), claims.get(*name)?.as_i64()?)))
        .collect();
    Ok(Jwt {
        header,
        payload,
        times,
    })
}

// ---------------------------------------------------------------- 假文

const LOREM: &str = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua ut enim ad minim veniam quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur excepteur sint occaecat cupidatat non proident sunt in culpa qui officia deserunt mollit anim id est laborum";

/// 中文占位文字：自写的无意义句子，循环取用（不是任何作品的摘录）。
const CHINESE: &[&str] = &[
    "清晨的街道还很安静，路灯一盏接一盏熄灭。",
    "这个功能的目标是让常用操作少点几下。",
    "窗外的树叶被风吹得沙沙作响。",
    "我们先把需求写清楚，再决定怎么实现。",
    "咖啡馆里坐满了低头看屏幕的人。",
    "一份好的文档能省下很多来回沟通的时间。",
    "傍晚的天空从橙色慢慢变成深蓝。",
    "测试数据只用于演示，不代表真实情况。",
    "小河绕过村口，流向远处的田野。",
    "按下回车之后，结果会复制到剪贴板。",
    "周末的市集上摆满了新鲜的蔬菜和水果。",
    "界面上的每一行文字都应该有它存在的理由。",
];

/// `words` 个单词的 Lorem ipsum，首字母大写、句号结尾。
pub fn lorem(words: usize) -> String {
    let source: Vec<&str> = LOREM.split(' ').collect();
    let mut text = source
        .iter()
        .cycle()
        .take(words.max(1))
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(first) = text.get(..1) {
        text = first.to_uppercase() + text.get(1..).unwrap_or_default();
    }
    text + "."
}

/// 至少 `chars` 个字的中文假文（整句截断）。
pub fn chinese_filler(chars: usize) -> String {
    let mut text = String::new();
    for sentence in CHINESE.iter().cycle() {
        if text.chars().count() >= chars.max(1) {
            break;
        }
        text.push_str(sentence);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_match_known_vectors() {
        // 哈希用来核对文件、签名：错一位就毫无用处，用公开的测试向量校验。
        assert_eq!(md5("hello"), "5d41402abc4b2a76b9719d911017c592");
        assert_eq!(sha1("hello"), "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d");
        assert_eq!(
            sha256("hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(sha512("").len(), 128);
    }

    #[test]
    fn base64_and_url_round_trip_unicode() {
        // 中文、emoji 编码后要能原样解回来；解不出 UTF-8 的不能冒充结果。
        assert_eq!(base64_encode("你好, world"), "5L2g5aW9LCB3b3JsZA==");
        assert_eq!(
            base64_decode("5L2g5aW9LCB3b3JsZA==").as_deref(),
            Some("你好, world")
        );
        assert_eq!(
            base64_decode("5L2g5aW9LCB3b3JsZA").as_deref(),
            Some("你好, world")
        );
        assert_eq!(base64_decode("not base64!"), None);
        assert_eq!(url_encode("a b&c=中"), "a%20b%26c%3D%E4%B8%AD");
        assert_eq!(
            url_decode("a%20b%26c%3D%E4%B8%AD").as_deref(),
            Some("a b&c=中")
        );
        assert_eq!(url_decode("100%"), None);
    }

    #[test]
    fn json_keeps_key_order_and_reports_position() {
        // 格式化不能把键排序或改写数字，用户要的是同一份数据换个排版。
        let input = r#"{"b":1,"a":[1.50,{"x":"y, z"}],"e":{},"s":"q\"}"}"#;
        assert_eq!(
            json_format(input, true).unwrap(),
            "{\n  \"b\": 1,\n  \"a\": [\n    1.50,\n    {\n      \"x\": \"y, z\"\n    }\n  ],\n  \"e\": {},\n  \"s\": \"q\\\"}\"\n}"
        );
        assert_eq!(
            json_format(&json_format(input, true).unwrap(), false).unwrap(),
            input
        );
        let err = json_check("{\"a\": }").unwrap_err().to_string();
        assert!(err.contains("第 1 行第 7 列"), "{err}");
    }

    #[test]
    fn case_conversion_splits_words_sensibly() {
        // 驼峰里的缩写（HTTPServer）和数字不能被拆碎。
        assert_eq!(
            words("parseHTTPServer2Response"),
            ["parse", "HTTP", "Server2", "Response"]
        );
        assert_eq!(words("user_id-and spaces"), ["user", "id", "and", "spaces"]);
        let cases = cases("hello world_api");
        let get = |key| cases.iter().find(|(k, ..)| *k == key).unwrap().2.clone();
        assert_eq!(get("camel"), "helloWorldApi");
        assert_eq!(get("pascal"), "HelloWorldApi");
        assert_eq!(get("snake"), "hello_world_api");
        assert_eq!(get("kebab"), "hello-world-api");
        assert_eq!(get("constant"), "HELLO_WORLD_API");
        assert_eq!(get("title"), "Hello World Api");
    }

    #[test]
    fn integers_parse_by_prefix() {
        assert_eq!(parse_integer("0xff", false), Some(255));
        assert_eq!(parse_integer("0b1010", false), Some(10));
        assert_eq!(parse_integer("0o17", false), Some(15));
        assert_eq!(parse_integer("1_000", false), Some(1000));
        assert_eq!(parse_integer("ff", true), Some(255));
        assert_eq!(parse_integer("ff", false), None);
        let bases = bases(255);
        assert_eq!(
            bases.iter().map(|b| b.2.as_str()).collect::<Vec<_>>(),
            ["255", "0xff", "0b11111111", "0o377"]
        );
    }

    #[test]
    fn jwt_payload_is_decoded_without_verifying() {
        // 只看内容不验签：头和载荷要格式化成可读 JSON，exp 等时间单独拿出来。
        let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        assert!(looks_like_jwt(token));
        let jwt = jwt_decode(token).unwrap();
        assert!(jwt.header.contains("\"alg\": \"HS256\""), "{}", jwt.header);
        assert!(
            jwt.payload.contains("\"name\": \"John Doe\""),
            "{}",
            jwt.payload
        );
        assert_eq!(jwt.times, [("iat".to_string(), 1516239022)]);
        assert!(jwt_decode("eyJ.bad").is_err());
    }

    #[test]
    fn passwords_are_random_and_cover_every_class() {
        // 密码必须每次不同、长度准确、各类字符都有；不含符号的选项真的不含符号。
        let (a, entropy) = password(20, true);
        let (b, _) = password(20, true);
        assert_ne!(a, b);
        assert_eq!(a.chars().count(), 20);
        assert!(entropy > 120.0, "{entropy}");
        assert!(a.chars().any(|c| SYMBOLS.contains(c)));
        let (plain, _) = password(32, false);
        assert!(plain.chars().all(|c| c.is_ascii_alphanumeric()), "{plain}");
        let v4 = uuid_v4();
        assert_eq!(v4.len(), 36);
        assert_eq!(v4.chars().nth(14), Some('4'));
        assert_eq!(uuid_v7().chars().nth(14), Some('7'));
    }

    #[test]
    fn filler_text_has_the_requested_size() {
        assert_eq!(lorem(5), "Lorem ipsum dolor sit amet.");
        assert!(chinese_filler(50).chars().count() >= 50);
    }
}
