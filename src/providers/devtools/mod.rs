//! 开发者小工具：UUID、密码、哈希、Base64、URL 编解码、JSON 格式化、命名转换、进制转换、JWT 解码、
//! 假文、本机 / 公网 IP。
//!
//! - 根搜索：第一个词是工具名时直接出结果（`uuid`、`md5 hello`、`json {...}`、`pwd 24`），
//!   或者输入本身一看就是（`0xff`、`eyJ…` 的 JWT、`{…}` 的 JSON）。工具名也能用中文、拼音找到（「密码」`mm`），
//!   回车进入范围并带上命令。
//! - 范围「开发工具」（前缀 `dev ` / `开发 `）：空查询列出全部工具；输入任意文字列出它的哈希、编码和各种命名。
//! - 回车复制这一行的结果；长结果（JSON、JWT）在详情里用等宽字体完整显示。
//! - 公网 IP 只在回车时请求（地址见设置 `devtools.public_ip_url`，测试用 `HELMP_PUBLIC_IP_URL`）。

mod net;
mod tools;

use crate::i18n::{tr, trf};
use crate::matcher;
use crate::provider::{
    Action, Block, Detail, Icon, Item, NamedAction, Outcome, Provider, Scope, Tint,
};

const PROVIDER_ID: &str = "devtools";
/// 明确调用了工具（`md5 hello`、`0xff`）：结果排第一。
const SCORE: u32 = 1050;
/// 范围内对任意文字给出的转换：排在按名字命中的工具后面。
const TRANSFORM_SCORE: u32 = 600;
/// 列表标题最多显示这么多字，完整结果在详情里。
const TITLE_CHARS: usize = 120;

struct Tool {
    key: &'static str,
    /// 进入范围时带上的命令词。
    command: &'static str,
    title: (&'static str, &'static str),
    usage: (&'static str, &'static str),
    keywords: &'static [&'static str],
    icon: &'static str,
}

const TOOLS: &[Tool] = &[
    Tool {
        key: "uuid",
        command: "uuid",
        title: ("生成 UUID", "Generate UUID"),
        usage: (
            "uuid · 加 upper 大写、nodash 去横线",
            "uuid · add upper or nodash",
        ),
        keywords: &["uuid", "guid", "生成 uuid", "唯一标识", "generate uuid"],
        icon: "fingerprint-pattern",
    },
    Tool {
        key: "password",
        command: "pwd",
        title: ("生成密码", "Generate Password"),
        usage: (
            "pwd 24 · 加 nosym 不含符号",
            "pwd 24 · add nosym for no symbols",
        ),
        keywords: &[
            "密码",
            "生成密码",
            "随机密码",
            "password",
            "generate password",
            "pwd",
        ],
        icon: "key-round",
    },
    Tool {
        key: "hash",
        command: "hash",
        title: ("哈希", "Hash"),
        usage: ("md5 / sha1 / sha256 <文字>", "md5 / sha1 / sha256 <text>"),
        keywords: &[
            "哈希",
            "摘要",
            "校验和",
            "hash",
            "md5",
            "sha1",
            "sha256",
            "checksum",
        ],
        icon: "hash",
    },
    Tool {
        key: "base64",
        command: "base64",
        title: ("Base64 编解码", "Base64 Encode / Decode"),
        usage: (
            "base64 <文字>，自动判断编码或解码",
            "base64 <text>, encodes or decodes",
        ),
        keywords: &["base64", "b64", "编码", "解码", "base64 编码"],
        icon: "binary",
    },
    Tool {
        key: "url",
        command: "url",
        title: ("URL 编解码", "URL Encode / Decode"),
        usage: ("url <文字>", "url <text>"),
        keywords: &[
            "url",
            "urlencode",
            "urldecode",
            "url 编码",
            "百分号编码",
            "url encode",
        ],
        icon: "link",
    },
    Tool {
        key: "json",
        command: "json",
        title: ("JSON 格式化", "Format JSON"),
        usage: (
            "json <JSON>：格式化、压缩、校验",
            "json <JSON>: pretty, minify, validate",
        ),
        keywords: &["json", "json 格式化", "格式化", "format json"],
        icon: "braces",
    },
    Tool {
        key: "case",
        command: "case",
        title: ("命名转换", "Change Case"),
        usage: (
            "case <文字>：驼峰、下划线、短横线…",
            "case <text>: camel, snake, kebab…",
        ),
        keywords: &[
            "命名",
            "命名转换",
            "驼峰",
            "下划线",
            "大小写",
            "case",
            "camel",
            "snake",
            "kebab",
            "change case",
        ],
        icon: "case-sensitive",
    },
    Tool {
        key: "base",
        command: "hex",
        title: ("进制转换", "Number Base"),
        usage: ("0xff · 0b1010 · hex 255", "0xff · 0b1010 · hex 255"),
        keywords: &[
            "进制",
            "进制转换",
            "十六进制",
            "二进制",
            "hex",
            "binary",
            "octal",
            "number base",
        ],
        icon: "binary",
    },
    Tool {
        key: "jwt",
        command: "jwt",
        title: ("JWT 解码", "Decode JWT"),
        usage: (
            "jwt <token>（不校验签名）",
            "jwt <token> (signature not verified)",
        ),
        keywords: &["jwt", "jwt 解码", "令牌", "decode jwt"],
        icon: "shield-check",
    },
    Tool {
        key: "lorem",
        command: "lorem",
        title: ("假文", "Placeholder Text"),
        usage: ("lorem 50 · 假文 100", "lorem 50 · 假文 100"),
        keywords: &[
            "假文",
            "中文假文",
            "占位文字",
            "lorem",
            "lorem ipsum",
            "placeholder text",
        ],
        icon: "pilcrow",
    },
    Tool {
        key: "ip",
        command: "ip",
        title: ("IP 地址", "IP Address"),
        usage: (
            "本机 IP；公网 IP 回车查询",
            "Local IPs; press Return for the public IP",
        ),
        keywords: &[
            "ip",
            "ip 地址",
            "本机 ip",
            "公网 ip",
            "ip address",
            "my ip",
            "public ip",
        ],
        icon: "network",
    },
];

/// 命令词 → 工具。
fn tool_for(word: &str) -> Option<&'static str> {
    Some(match word {
        "uuid" | "guid" => "uuid",
        "pwd" | "password" | "passwd" | "密码" => "password",
        "hash" | "md5" | "sha1" | "sha256" | "sha512" | "哈希" => "hash",
        "base64" | "b64" => "base64",
        "url" | "urlencode" | "urldecode" => "url",
        "json" => "json",
        "case" | "camel" | "snake" | "kebab" | "pascal" | "命名" => "case",
        "hex" | "dec" | "bin" | "oct" | "进制" => "base",
        "jwt" => "jwt",
        "lorem" | "lipsum" | "假文" | "中文假文" => "lorem",
        "ip" | "myip" | "本机ip" | "公网ip" => "ip",
        _ => return None,
    })
}

fn tool(key: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|tool| tool.key == key)
}

fn badge() -> &'static str {
    tr("开发工具", "Dev Tools")
}

fn icon(name: &'static str) -> Icon {
    Icon::Symbol {
        name,
        tint: Tint::Graphite,
    }
}

pub struct DevTools;

impl Provider for DevTools {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn scope(&self) -> Option<Scope> {
        Some(Scope {
            id: PROVIDER_ID,
            label: badge().into(),
            tint: Tint::Graphite,
            prefixes: vec!["dev ".into(), "开发 ".into(), "开发工具 ".into()],
            placeholder: tr(
                "输入命令（uuid、md5 文字、json…）或任意文字",
                "Type a command (uuid, md5 text, json…) or any text",
            )
            .into(),
            filters: Vec::new(),
        })
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let mut items = command_items(query).unwrap_or_default();
        items.extend(entries(query, &items));
        items.extend(scope_entry(query));
        items
    }

    fn query_scoped(&self, query: &str, _filter: &str) -> Vec<Item> {
        let query = query.trim();
        if query.is_empty() {
            return TOOLS
                .iter()
                .enumerate()
                .map(|(rank, tool)| entry_item(tool, 1000 - rank as u32))
                .collect();
        }
        if let Some(items) = command_items(query) {
            return items;
        }
        let mut items = entries(query, &[]);
        items.extend(transforms(query));
        items
    }
}

/// 一行结果。
struct Out {
    tool: &'static str,
    key: String,
    label: String,
    value: String,
    /// 是错误说明，不是结果：回车只把说明再显示一次。
    error: bool,
    metadata: Vec<(String, String)>,
}

impl Out {
    fn new(
        tool: &'static str,
        key: impl Into<String>,
        label: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self {
            tool,
            key: key.into(),
            label: label.into(),
            value: value.into(),
            error: false,
            metadata: Vec::new(),
        }
    }

    fn error(tool: &'static str, message: String) -> Self {
        Self {
            error: true,
            ..Self::new(tool, "error", tr("无法处理", "Can’t process"), message)
        }
    }

    fn into_item(self, score: u32) -> Item {
        let tool = tool(self.tool);
        let icon = icon(tool.map_or("wrench", |t| t.icon));
        let single_line: String = self
            .value
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(TITLE_CHARS)
            .collect();
        let truncated = single_line.chars().count() < self.value.chars().count();
        let title = if truncated {
            format!("{single_line}…")
        } else {
            single_line
        };
        if self.error {
            let shown = self.value.clone();
            return Item {
                id: format!("{PROVIDER_ID}:{}:error", self.tool),
                title: self.value,
                subtitle: self.label,
                badge: badge().into(),
                score,
                icon,
                action: Action::new(move || Ok(Outcome::Message(shown.clone()))).keep_open(),
                action_title: tr("无法处理", "Can’t Process").into(),
                ..Item::blank()
            };
        }
        let detail = (truncated || !self.metadata.is_empty()).then(|| Detail {
            icon: icon.clone(),
            title: self.label.clone(),
            blocks: vec![Block::Code(self.value.clone())],
            metadata: self.metadata.clone(),
            ..Detail::default()
        });
        Item {
            id: format!("{PROVIDER_ID}:{}:{}", self.tool, self.key),
            title,
            subtitle: self.label,
            badge: badge().into(),
            score,
            icon,
            action: Action::copy(self.value),
            action_title: tr("复制", "Copy").into(),
            detail,
            ..Item::blank()
        }
    }
}

fn into_items(outs: Vec<Out>, top: u32) -> Vec<Item> {
    outs.into_iter()
        .enumerate()
        .map(|(rank, out)| out.into_item(top.saturating_sub(rank as u32)))
        .collect()
}

/// 命令（`md5 hello`）或一眼能认出的输入（`0xff`、JWT、JSON）。认不出返回 `None`。
fn command_items(query: &str) -> Option<Vec<Item>> {
    let (word, arg) = match query.split_once(char::is_whitespace) {
        Some((word, arg)) => (word.to_lowercase(), arg.trim()),
        None => (query.to_lowercase(), ""),
    };
    if let Some(tool) = tool_for(&word) {
        // 需要输入的工具只打了命令词：交给工具入口（回车进范围），不出空结果。
        if arg.is_empty()
            && matches!(
                tool,
                "hash" | "base64" | "url" | "json" | "case" | "jwt" | "base"
            )
        {
            return None;
        }
        let mut items = into_items(run(tool, &word, arg), SCORE);
        if tool == "ip" {
            // 本机地址之后给出公网 IP 查询（回车才联网）。
            items.push(public_ip_item(SCORE.saturating_sub(items.len() as u32)));
        }
        return Some(items);
    }
    let lower = query.to_lowercase();
    if ["0x", "0b", "0o"].iter().any(|p| lower.starts_with(p))
        && let Some(value) = tools::parse_integer(query, false)
    {
        return Some(into_items(base_rows(value), SCORE));
    }
    if tools::looks_like_jwt(query) {
        return Some(into_items(jwt_rows(query), SCORE));
    }
    if (query.starts_with('{') || query.starts_with('[')) && tools::json_check(query).is_ok() {
        return Some(into_items(json_rows(query), SCORE));
    }
    None
}

fn run(tool: &'static str, word: &str, arg: &str) -> Vec<Out> {
    match tool {
        "uuid" => uuid_rows(arg),
        "password" => password_rows(arg),
        "hash" => hash_rows(word, arg),
        "base64" => base64_rows(arg),
        "url" => url_rows(arg),
        "json" => json_rows(arg),
        "case" => case_rows(word, arg),
        "base" => match tools::parse_integer(arg, word == "hex") {
            Some(value) => base_rows(value),
            None => vec![Out::error(
                "base",
                trf!("读不出整数：{arg}", "Not an integer: {arg}"),
            )],
        },
        "jwt" => jwt_rows(arg),
        "lorem" => lorem_rows(word, arg),
        "ip" => ip_rows(),
        _ => Vec::new(),
    }
}

fn has_flag(arg: &str, flags: &[&str]) -> bool {
    arg.split_whitespace()
        .any(|word| flags.contains(&word.to_lowercase().as_str()))
}

fn uuid_rows(arg: &str) -> Vec<Out> {
    let upper = has_flag(arg, &["upper", "大写"]);
    let nodash = has_flag(arg, &["nodash", "no-dash", "去横线"]);
    let shape = |id: String| {
        let id = if nodash { id.replace('-', "") } else { id };
        if upper { id.to_uppercase() } else { id }
    };
    vec![
        Out::new(
            "uuid",
            "v4",
            tr("UUID v4（随机）", "UUID v4 (random)"),
            shape(tools::uuid_v4()),
        ),
        Out::new(
            "uuid",
            "v7",
            tr("UUID v7（按时间排序）", "UUID v7 (time-ordered)"),
            shape(tools::uuid_v7()),
        ),
    ]
}

fn password_rows(arg: &str) -> Vec<Out> {
    let length = arg
        .split_whitespace()
        .find_map(|word| word.parse::<usize>().ok())
        .unwrap_or(tools::DEFAULT_PASSWORD_LENGTH);
    if !(4..=256).contains(&length) {
        return vec![Out::error(
            "password",
            tr(
                "密码长度要在 4 到 256 之间",
                "Password length must be 4–256",
            )
            .into(),
        )];
    }
    let no_symbols = has_flag(arg, &["nosym", "nosymbols", "无符号"]);
    let mut rows = Vec::new();
    let variants: &[(bool, &str)] = if no_symbols {
        &[(false, "alnum")]
    } else {
        &[(true, "symbols"), (false, "alnum")]
    };
    for (symbols, key) in variants {
        let (text, entropy) = tools::password(length, *symbols);
        let kind = if *symbols {
            tr("含符号", "with symbols")
        } else {
            tr("字母和数字", "letters and digits")
        };
        let mut out = Out::new(
            "password",
            *key,
            trf!(
                "{length} 位 · {kind} · 约 {entropy:.0} 比特",
                "{length} chars · {kind} · ~{entropy:.0} bits"
            ),
            text,
        );
        out.metadata = vec![
            (tr("长度", "Length").into(), length.to_string()),
            (
                tr("熵", "Entropy").into(),
                trf!("约 {entropy:.0} 比特", "~{entropy:.0} bits"),
            ),
        ];
        rows.push(out);
    }
    rows
}

/// (算法名, 计算函数)
type Hasher = (&'static str, fn(&str) -> String);

fn hash_rows(word: &str, text: &str) -> Vec<Out> {
    let all: [Hasher; 4] = [
        ("md5", tools::md5),
        ("sha1", tools::sha1),
        ("sha256", tools::sha256),
        ("sha512", tools::sha512),
    ];
    // 用户点名的算法排第一。
    let mut ordered: Vec<_> = all.iter().filter(|(name, _)| *name == word).collect();
    ordered.extend(all.iter().filter(|(name, _)| *name != word));
    ordered
        .into_iter()
        .map(|(name, f)| Out::new("hash", *name, name.to_uppercase(), f(text)))
        .collect()
}

fn base64_rows(text: &str) -> Vec<Out> {
    let encode = Out::new(
        "base64",
        "encode",
        tr("Base64 编码", "Base64 encoded"),
        tools::base64_encode(text),
    );
    let decoded = (!text.contains(char::is_whitespace))
        .then(|| tools::base64_decode(text))
        .flatten()
        .filter(|decoded| {
            !decoded
                .chars()
                .any(|c| c.is_control() && !c.is_whitespace())
        });
    match decoded {
        // 看起来就是 Base64 且解得出文字：解码排第一。
        Some(decoded) => vec![
            Out::new(
                "base64",
                "decode",
                tr("Base64 解码", "Base64 decoded"),
                decoded,
            ),
            encode,
        ],
        None => vec![encode],
    }
}

fn url_rows(text: &str) -> Vec<Out> {
    let encode = Out::new(
        "url",
        "encode",
        tr("URL 编码", "URL encoded"),
        tools::url_encode(text),
    );
    match text
        .contains('%')
        .then(|| tools::url_decode(text))
        .flatten()
    {
        Some(decoded) => vec![
            Out::new("url", "decode", tr("URL 解码", "URL decoded"), decoded),
            encode,
        ],
        None => vec![encode],
    }
}

fn json_rows(text: &str) -> Vec<Out> {
    let pretty = match tools::json_format(text, true) {
        Ok(pretty) => pretty,
        Err(err) => return vec![Out::error("json", format!("{err:#}"))],
    };
    let minified = tools::json_format(text, false).unwrap_or_default();
    let mut pretty = Out::new(
        "json",
        "pretty",
        tr("JSON 格式化 · 有效", "Pretty JSON · valid"),
        pretty,
    );
    pretty.metadata = vec![(
        tr("字符数", "Characters").into(),
        minified.chars().count().to_string(),
    )];
    vec![
        pretty,
        Out::new("json", "minify", tr("JSON 压缩", "Minified JSON"), minified),
    ]
}

fn case_rows(word: &str, text: &str) -> Vec<Out> {
    let mut cases = tools::cases(text);
    // `snake fooBar` 把 snake_case 排第一。
    if let Some(index) = cases.iter().position(|(key, ..)| *key == word) {
        let picked = cases.remove(index);
        cases.insert(0, picked);
    }
    cases
        .into_iter()
        .map(|(key, label, value)| Out::new("case", key, label, value))
        .collect()
}

fn base_rows(value: u128) -> Vec<Out> {
    tools::bases(value)
        .into_iter()
        .map(|(key, label, text)| Out::new("base", key, label, text))
        .collect()
}

fn jwt_rows(token: &str) -> Vec<Out> {
    let jwt = match tools::jwt_decode(token) {
        Ok(jwt) => jwt,
        Err(err) => return vec![Out::error("jwt", format!("{err:#}"))],
    };
    let mut rows = vec![
        Out::new(
            "jwt",
            "payload",
            tr(
                "JWT 载荷（未校验签名）",
                "JWT payload (signature not verified)",
            ),
            jwt.payload,
        ),
        Out::new("jwt", "header", tr("JWT 头", "JWT header"), jwt.header),
    ];
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    for (name, seconds) in jwt.times {
        let Some(at) = chrono::DateTime::from_timestamp(seconds, 0) else {
            continue;
        };
        let mut label = name.clone();
        if name == "exp" {
            label = if seconds < now {
                tr("exp · 已过期", "exp · expired").into()
            } else {
                tr("exp · 未过期", "exp · not expired").into()
            };
        }
        rows.push(Out::new(
            "jwt",
            name,
            label,
            at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        ));
    }
    rows
}

fn lorem_rows(word: &str, arg: &str) -> Vec<Out> {
    let count = arg.split_whitespace().find_map(|w| w.parse::<usize>().ok());
    let lorem = Out::new(
        "lorem",
        "latin",
        tr("Lorem ipsum（按单词数）", "Lorem ipsum (words)"),
        tools::lorem(count.unwrap_or(50).min(2000)),
    );
    let chinese = Out::new(
        "lorem",
        "chinese",
        tr("中文假文（按字数）", "Chinese filler (characters)"),
        tools::chinese_filler(count.unwrap_or(100).min(5000)),
    );
    if matches!(word, "假文" | "中文假文") {
        vec![chinese, lorem]
    } else {
        vec![lorem, chinese]
    }
}

fn ip_rows() -> Vec<Out> {
    let mut rows: Vec<Out> = net::local_addresses()
        .into_iter()
        .map(|(name, ip)| {
            Out::new(
                "ip",
                format!("{name}-{}", net::family(&ip).to_lowercase()),
                trf!("本机 {} · {name}", "Local {} · {name}", net::family(&ip)),
                ip.to_string(),
            )
        })
        .collect();
    if rows.is_empty() {
        rows.push(Out::error("ip", net::no_addresses().into()));
    }
    rows
}

/// 「公网 IP」：只在回车时请求。
fn public_ip_item(score: u32) -> Item {
    let url = net::public_ip_url();
    let subtitle = trf!(
        "回车查询（访问 {url}）",
        "Press Return to look it up ({url})"
    );
    Item {
        id: format!("{PROVIDER_ID}:ip:public"),
        title: tr("公网 IP", "Public IP").into(),
        subtitle,
        badge: badge().into(),
        score,
        icon: icon("globe"),
        action: Action::new(move || {
            if !net::may_fetch()
                && let Some(outcome) = crate::effects::dry("fetch", &url)
            {
                return Ok(outcome);
            }
            let ip = net::public_ip(&url)?.to_string();
            Ok(Outcome::Show(Detail {
                icon: icon("globe"),
                title: tr("公网 IP", "Public IP").into(),
                blocks: vec![
                    Block::Result {
                        text: ip.clone(),
                        large: true,
                    },
                    Block::Caption(url.clone()),
                ],
                actions: vec![NamedAction::new(tr("复制", "Copy"), None, Action::copy(ip))],
                ..Detail::default()
            }))
        }),
        action_title: tr("查询", "Look Up").into(),
        ..Item::blank()
    }
}

/// 范围里对任意文字给出的转换：哈希、编码、命名。
fn transforms(text: &str) -> Vec<Item> {
    let mut outs = Vec::new();
    outs.extend(base64_rows(text));
    outs.extend(url_rows(text));
    outs.extend(hash_rows("md5", text).into_iter().take(3));
    outs.extend(case_rows("", text));
    into_items(outs, TRANSFORM_SCORE)
}

/// 按名字命中的工具入口（「密码」`mm`、「进制」），跳过已经出了结果的工具。
fn entries(query: &str, shown: &[Item]) -> Vec<Item> {
    if query.chars().count() < 2 {
        return Vec::new();
    }
    let mut items: Vec<Item> = TOOLS
        .iter()
        .filter(|tool| {
            !shown
                .iter()
                .any(|item| item.id.starts_with(&format!("{PROVIDER_ID}:{}:", tool.key)))
        })
        .filter_map(|tool| {
            let score = tool
                .keywords
                .iter()
                .chain([tool.title.0, tool.title.1].iter())
                .filter_map(|keyword| matcher::score(query, keyword))
                .max()
                .filter(|score| *score >= 700)?;
            Some(entry_item(tool, score))
        })
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.score));
    items
}

fn entry_item(tool: &'static Tool, score: u32) -> Item {
    let command = format!("{} ", tool.command);
    Item {
        id: format!("{PROVIDER_ID}:tool:{}", tool.key),
        title: tr(tool.title.0, tool.title.1).into(),
        subtitle: tr(tool.usage.0, tool.usage.1).into(),
        badge: badge().into(),
        score,
        icon: icon(tool.icon),
        action: Action::new(move || Ok(Outcome::EnterScopeWith(PROVIDER_ID, command.clone())))
            .keep_open(),
        action_title: tr("使用", "Use").into(),
        ..Item::blank()
    }
}

/// 「开发工具」本身：进入范围。
fn scope_entry(query: &str) -> Option<Item> {
    let score = [
        "开发工具",
        "开发者工具",
        "dev tools",
        "developer tools",
        "devtools",
    ]
    .iter()
    .filter_map(|keyword| matcher::score(query, keyword))
    .max()
    .filter(|score| *score >= 700 && query.chars().count() >= 2)?;
    Some(Item {
        id: format!("{PROVIDER_ID}:scope"),
        title: tr("开发工具", "Dev Tools").into(),
        subtitle: tr(
            "UUID、密码、哈希、编码、JSON、命名、进制、JWT、IP",
            "UUID, passwords, hashes, encoding, JSON, case, bases, JWT, IP",
        )
        .into(),
        badge: badge().into(),
        score,
        icon: icon("wrench"),
        action: Action::enter_scope(PROVIDER_ID),
        action_title: tr("进入", "Open").into(),
        ..Item::blank()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(items: &[Item]) -> Vec<&str> {
        items.iter().map(|item| item.id.as_str()).collect()
    }

    #[test]
    fn commands_answer_inline_and_enter_copies() {
        // 根搜索里打「md5 hello」就该直接看到结果，回车复制的是结果本身。
        let items = DevTools.query("md5 hello");
        let first = items.first().unwrap();
        assert_eq!(first.id, "devtools:hash:md5");
        assert_eq!(first.title, "5d41402abc4b2a76b9719d911017c592");
        assert!(first.score > 1000);
        let items = DevTools.query("0xff");
        assert_eq!(items.first().unwrap().title, "255");
    }

    #[test]
    fn tools_are_found_by_chinese_name_and_pinyin() {
        // 中文用户记得「密码」「进制」，或者只打拼音首字母。
        for (query, id) in [
            ("密码", "devtools:password:symbols"),
            ("进制", "devtools:tool:base"),
            ("jzzh", "devtools:tool:base"),
            ("kfgj", "devtools:scope"),
        ] {
            let items = DevTools.query(query);
            assert!(ids(&items).contains(&id), "{query:?}: {:?}", ids(&items));
        }
    }

    #[test]
    fn plain_searches_produce_nothing() {
        // 普通搜索词（应用名、中文词）不能冒出工具结果。
        for query in ["wx", "微信", "safari", "c", "2026"] {
            assert!(
                DevTools.query(query).is_empty(),
                "{query:?}: {:?}",
                ids(&DevTools.query(query))
            );
        }
    }

    #[test]
    fn invalid_json_explains_where() {
        let items = DevTools.query("json {\"a\":}");
        let first = items.first().unwrap();
        assert_eq!(first.id, "devtools:json:error");
        assert!(first.title.contains("第 1 行"), "{}", first.title);
    }

    #[test]
    fn scope_lists_every_tool_and_transforms_free_text() {
        assert_eq!(DevTools.query_scoped("", "").len(), TOOLS.len());
        let items = DevTools.query_scoped("hello world", "");
        let ids = ids(&items);
        for id in [
            "devtools:base64:encode",
            "devtools:url:encode",
            "devtools:hash:md5",
            "devtools:case:camel",
        ] {
            assert!(ids.contains(&id), "{ids:?}");
        }
    }

    #[test]
    fn symbol_icons_exist_in_bundled_assets() {
        use gpui_kit::AssetSource as _;
        for name in TOOLS.iter().map(|t| t.icon).chain(["wrench", "globe"]) {
            let path = format!("icons/{name}.svg");
            assert!(
                gpui_kit::assets::AllAssets.load(&path).unwrap().is_some(),
                "{name}"
            );
        }
    }
}
