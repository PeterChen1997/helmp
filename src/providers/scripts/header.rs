//! 脚本头注释解析：兼容 Raycast Script Commands 协议（`@raycast.*`），
//! 同时接受 `@huan.*` 同名字段；两者都写时 `@huan.` 优先。
//!
//! 注释前缀支持 `#`（sh/py/rb/AppleScript）、`//`（swift/js）、`--`（AppleScript/Lua）。
//! 字段名大小写不敏感（社区仓库里有人写 `@raycast.packagename`）。

use std::collections::HashMap;

use serde_json::Value;

/// Raycast 最多支持 3 个参数。
pub const MAX_ARGUMENTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 跑完不展示（Raycast 会在 HUD 显示最后一行，启动器没有 HUD）。
    Silent,
    /// 展示 stdout 最后一行。
    Compact,
    /// 展示完整 stdout + stderr。
    FullOutput,
    /// Raycast 里是「把首行常驻在条目上并定时刷新」，这里先按 compact 处理。
    Inline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropdownOption {
    pub title: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgKind {
    Text,
    /// 启动器没有掩码输入框，按普通文本传入。
    Password,
    Dropdown(Vec<DropdownOption>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argument {
    pub kind: ArgKind,
    pub placeholder: String,
    pub optional: bool,
    pub percent_encoded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub title: String,
    pub mode: Mode,
    pub package_name: Option<String>,
    pub icon: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub arguments: Vec<Argument>,
    pub needs_confirmation: bool,
    pub current_directory_path: Option<String>,
    pub refresh_time: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// 没有任何 `@raycast.` / `@huan.` 字段：不是脚本命令（图片、README 等），静默忽略。
    NotAScript,
    Script(Header),
    /// 有元数据但不能用，附原因，由调用方打印到 stderr。
    Skipped(String),
}

/// 一行元数据：`<注释前缀> @raycast.<key> <value>`。返回 (是否 @huan., 小写 key, value)。
fn metadata_line(line: &str) -> Option<(bool, String, &str)> {
    let line = line.trim_start();
    let rest = ["//", "--", "#"]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))?
        .trim_start();
    let (is_huan, rest) = if let Some(rest) = rest.strip_prefix("@huan.") {
        (true, rest)
    } else {
        (false, rest.strip_prefix("@raycast.")?)
    };
    let key_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let (key, value) = rest.split_at(key_end);
    if key.is_empty() {
        return None;
    }
    Some((is_huan, key.to_lowercase(), value.trim()))
}

pub fn parse(source: &str) -> Parsed {
    // key -> (value, 是否来自 @huan.)
    let mut fields: HashMap<String, (String, bool)> = HashMap::new();
    for (is_huan, key, value) in source.lines().filter_map(metadata_line) {
        let keep_existing = matches!(fields.get(&key), Some((_, true))) && !is_huan;
        if !keep_existing {
            fields.insert(key, (value.to_string(), is_huan));
        }
    }
    if fields.is_empty() {
        return Parsed::NotAScript;
    }
    match build(&fields) {
        Ok(header) => Parsed::Script(header),
        Err(reason) => Parsed::Skipped(reason),
    }
}

fn build(fields: &HashMap<String, (String, bool)>) -> Result<Header, String> {
    let get = |key: &str| {
        fields
            .get(key)
            .map(|(value, _)| value.as_str())
            .filter(|value| !value.is_empty())
    };

    if let Some(version) = get("schemaversion")
        && version != "1"
    {
        return Err(format!("不支持的 schemaVersion {version}（只支持 1）"));
    }
    if let Some(platform) = get("platform")
        && !platform.to_lowercase().contains("macos")
    {
        return Err(format!("platform 为 {platform}，不含 macOS"));
    }
    let title = get("title").ok_or("缺少 @raycast.title")?.to_string();
    let mode = match get("mode").ok_or("缺少 @raycast.mode")? {
        m if m.eq_ignore_ascii_case("silent") => Mode::Silent,
        m if m.eq_ignore_ascii_case("compact") => Mode::Compact,
        m if m.eq_ignore_ascii_case("fullOutput") => Mode::FullOutput,
        m if m.eq_ignore_ascii_case("inline") => Mode::Inline,
        other => return Err(format!("未知 mode {other}")),
    };
    let needs_confirmation = match get("needsconfirmation") {
        None => false,
        Some(v) if v.eq_ignore_ascii_case("true") => true,
        Some(v) if v.eq_ignore_ascii_case("false") => false,
        Some(other) => return Err(format!("needsConfirmation 只能是 true/false，实际 {other}")),
    };

    let mut arguments = Vec::new();
    for index in 1..=MAX_ARGUMENTS {
        if let Some(raw) = get(&format!("argument{index}")) {
            arguments.push(parse_argument(index, raw)?);
        }
    }

    let owned = |key: &str| get(key).map(str::to_string);
    Ok(Header {
        title,
        mode,
        package_name: owned("packagename"),
        icon: owned("icon"),
        description: owned("description"),
        author: owned("author"),
        arguments,
        needs_confirmation,
        current_directory_path: owned("currentdirectorypath"),
        refresh_time: owned("refreshtime"),
    })
}

fn parse_argument(index: usize, raw: &str) -> Result<Argument, String> {
    let json: Value =
        serde_json::from_str(raw).map_err(|err| format!("argument{index} 不是合法 JSON：{err}"))?;
    let Value::Object(map) = json else {
        return Err(format!("argument{index} 应是 JSON 对象"));
    };
    let flag = |key: &str| map.get(key).and_then(Value::as_bool).unwrap_or(false);
    let placeholder = map
        .get("placeholder")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .map_or_else(|| format!("参数{index}"), str::to_string);
    // 旧字段 `secure: true` 等价于 `type: password`。
    let kind = match map.get("type").and_then(Value::as_str).unwrap_or("text") {
        "text" if flag("secure") => ArgKind::Password,
        "text" => ArgKind::Text,
        "password" => ArgKind::Password,
        "dropdown" => ArgKind::Dropdown(parse_dropdown(index, map.get("data"))?),
        other => return Err(format!("argument{index} 的 type {other} 不支持")),
    };
    Ok(Argument {
        kind,
        placeholder,
        optional: flag("optional"),
        percent_encoded: flag("percentEncoded"),
    })
}

fn parse_dropdown(index: usize, data: Option<&Value>) -> Result<Vec<DropdownOption>, String> {
    let missing = || format!("argument{index} 是 dropdown，但 data 不是 [{{title, value}}] 数组");
    let items = data.and_then(Value::as_array).ok_or_else(missing)?;
    items
        .iter()
        .map(|item| {
            let title = item.get("title").and_then(Value::as_str)?;
            let value = item.get("value").and_then(Value::as_str)?;
            Some(DropdownOption {
                title: title.to_string(),
                value: value.to_string(),
            })
        })
        .collect::<Option<Vec<_>>>()
        .filter(|options| !options.is_empty())
        .ok_or_else(missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(source: &str) -> Header {
        match parse(source) {
            Parsed::Script(header) => header,
            other => panic!("应解析成脚本，实际 {other:?}"),
        }
    }

    fn skipped(source: &str) -> String {
        match parse(source) {
            Parsed::Skipped(reason) => reason,
            other => panic!("应被跳过，实际 {other:?}"),
        }
    }

    #[test]
    fn parses_hash_comment_header_with_all_fields() {
        // Raycast 仓库里绝大多数脚本是 `#` 注释的 sh/py/AppleScript；字段解析错一个，脚本就跑不对。
        let header = script(
            r#"#!/bin/bash
# @raycast.schemaVersion 1
# @raycast.title Search Flights
# @raycast.mode silent
# @raycast.packageName Web Searches
# @raycast.icon 🛩
# @raycast.description Find flights
# @raycast.author Alice
# @raycast.needsConfirmation true
# @raycast.currentDirectoryPath ~/Desktop
# @raycast.refreshTime 1h
# @raycast.argument1 { "type": "text", "placeholder": "from city", "percentEncoded": true }
# @raycast.argument2 { "type": "password", "placeholder": "token", "optional": true }
# @raycast.argument3 { "type": "dropdown", "placeholder": "class", "data": [{"title": "Economy", "value": "e"}, {"title": "Business", "value": "b"}] }
open "https://example.com"
"#,
        );
        assert_eq!(header.title, "Search Flights");
        assert_eq!(header.mode, Mode::Silent);
        assert_eq!(header.package_name.as_deref(), Some("Web Searches"));
        assert_eq!(header.icon.as_deref(), Some("🛩"));
        assert_eq!(header.description.as_deref(), Some("Find flights"));
        assert_eq!(header.author.as_deref(), Some("Alice"));
        assert!(header.needs_confirmation);
        assert_eq!(header.current_directory_path.as_deref(), Some("~/Desktop"));
        assert_eq!(header.refresh_time.as_deref(), Some("1h"));
        assert_eq!(
            header.arguments,
            vec![
                Argument {
                    kind: ArgKind::Text,
                    placeholder: "from city".into(),
                    optional: false,
                    percent_encoded: true,
                },
                Argument {
                    kind: ArgKind::Password,
                    placeholder: "token".into(),
                    optional: true,
                    percent_encoded: false,
                },
                Argument {
                    kind: ArgKind::Dropdown(vec![
                        DropdownOption {
                            title: "Economy".into(),
                            value: "e".into(),
                        },
                        DropdownOption {
                            title: "Business".into(),
                            value: "b".into(),
                        },
                    ]),
                    placeholder: "class".into(),
                    optional: false,
                    percent_encoded: false,
                },
            ]
        );
    }

    #[test]
    fn accepts_slash_and_dash_comment_prefixes() {
        // Swift/JS 脚本用 `//`，AppleScript 常用 `--`；不认这两种前缀就会漏掉整类脚本。
        let swift =
            script("#!/usr/bin/swift\n// @raycast.title Swift One\n// @raycast.mode compact\n");
        assert_eq!(
            (swift.title.as_str(), swift.mode),
            ("Swift One", Mode::Compact)
        );
        let applescript = script(
            "#!/usr/bin/osascript\n-- @raycast.title Apple One\n-- @raycast.mode fullOutput\n",
        );
        assert_eq!(
            (applescript.title.as_str(), applescript.mode),
            ("Apple One", Mode::FullOutput)
        );
    }

    #[test]
    fn huan_prefix_works_alone_and_overrides_raycast() {
        // `@huan.` 让用户在不改 Raycast 字段的前提下为唤单独覆盖（比如中文标题），无论写在前还是后。
        let only_huan = script("# @huan.title 时间\n# @huan.mode inline\n");
        assert_eq!(
            (only_huan.title.as_str(), only_huan.mode),
            ("时间", Mode::Inline)
        );
        let huan_first =
            script("# @huan.title 中文标题\n# @raycast.title English\n# @raycast.mode silent\n");
        assert_eq!(huan_first.title, "中文标题");
        let huan_last =
            script("# @raycast.title English\n# @raycast.mode silent\n# @huan.title 中文标题\n");
        assert_eq!(huan_last.title, "中文标题");
    }

    #[test]
    fn keys_are_case_insensitive() {
        // 社区仓库有 5 个脚本写成 `@raycast.packagename`，Raycast 本身照样能用。
        let header =
            script("# @raycast.title T\n# @raycast.mode silent\n# @raycast.packagename Pkg\n");
        assert_eq!(header.package_name.as_deref(), Some("Pkg"));
    }

    #[test]
    fn deprecated_secure_flag_means_password() {
        let header = script(
            "# @raycast.title T\n# @raycast.mode silent\n# @raycast.argument1 { \"type\": \"text\", \"placeholder\": \"p\", \"secure\": true }\n",
        );
        assert_eq!(
            header.arguments.first().map(|a| &a.kind),
            Some(&ArgKind::Password)
        );
    }

    #[test]
    fn missing_title_or_mode_is_skipped_with_reason() {
        // 缺必填字段的文件不能悄悄消失，得告诉用户为什么没出现在结果里。
        assert!(skipped("# @raycast.mode silent\n").contains("title"));
        assert!(skipped("# @raycast.title T\n").contains("mode"));
        assert!(skipped("# @raycast.title T\n# @raycast.mode popup\n").contains("popup"));
    }

    #[test]
    fn invalid_argument_json_is_skipped_with_reason() {
        let reason = skipped(
            "# @raycast.title T\n# @raycast.mode silent\n# @raycast.argument1 {type: text}\n",
        );
        assert!(reason.contains("argument1"), "{reason}");
        let reason = skipped(
            "# @raycast.title T\n# @raycast.mode silent\n# @raycast.argument1 {\"type\":\"dropdown\",\"placeholder\":\"x\"}\n",
        );
        assert!(reason.contains("dropdown"), "{reason}");
    }

    #[test]
    fn non_macos_platform_and_unknown_schema_are_skipped() {
        // Windows 专用脚本在 macOS 上跑必然失败，提前跳过比执行时报错更清楚。
        assert!(
            skipped("# @raycast.title T\n# @raycast.mode silent\n# @raycast.platform windows\n")
                .contains("platform")
        );
        assert!(
            skipped("# @raycast.schemaVersion 2\n# @raycast.title T\n# @raycast.mode silent\n")
                .contains("schemaVersion")
        );
        assert!(matches!(
            parse(
                "# @raycast.title T\n# @raycast.mode silent\n# @raycast.platform macOS, windows\n"
            ),
            Parsed::Script(_)
        ));
    }

    #[test]
    fn files_without_metadata_are_not_scripts() {
        // 脚本目录里的 README、图片不应该刷屏报「跳过」。
        assert_eq!(
            parse("#!/bin/bash\necho hi\n# @ raycast\n"),
            Parsed::NotAScript
        );
        assert_eq!(parse(""), Parsed::NotAScript);
    }
}
