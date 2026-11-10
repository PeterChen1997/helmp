//! 片段的存储：`<数据目录>/snippets.json`，`{"snippets": [{id, name, keyword, content, tags}]}`。
//!
//! 启动器和设置窗口在同一个进程里各自读写这个文件：写入走临时文件再改名，读取按修改时间缓存。
//! 导入：Raycast 导出的 JSON（`[{"name","text","keyword"}]`）、Alfred 的 `.alfredsnippets`
//! （zip 包，每个片段一个 `{"alfredsnippet": {...}}` JSON）或解压后的目录 / 单个 JSON。

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use crate::i18n::{tr, trf};

pub const FILE: &str = "snippets.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    /// 输入这个词能直接定位到片段（v1 不做自动展开）。
    #[serde(default)]
    pub keyword: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Document {
    #[serde(default)]
    snippets: Vec<Snippet>,
}

pub fn path() -> Result<PathBuf> {
    Ok(crate::paths::data_dir()?.join(FILE))
}

/// 读全部片段。文件不存在时是空列表；文件坏了要报错（不能当成空列表，下次保存会把用户的片段覆盖掉）。
pub fn load_from(path: &Path) -> Result<Vec<Snippet>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| trf!("读取 {}", "Couldn’t read {}", path.display()));
        }
    };
    let document: Document = serde_json::from_str(&text).with_context(|| {
        trf!(
            "{} 不是合法的片段文件",
            "{} isn’t a valid snippets file",
            path.display()
        )
    })?;
    Ok(document.snippets)
}

pub fn save_to(path: &Path, snippets: &[Snippet]) -> Result<()> {
    let document = Document {
        snippets: snippets.to_vec(),
    };
    let text = serde_json::to_string_pretty(&document)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)
        .with_context(|| trf!("写入 {}", "Couldn’t write {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| trf!("保存 {}", "Couldn’t save {}", path.display()))?;
    Ok(())
}

pub fn load() -> Result<Vec<Snippet>> {
    load_from(&path()?)
}

/// 读出、修改、写回。
pub fn update<T>(f: impl FnOnce(&mut Vec<Snippet>) -> Result<T>) -> Result<T> {
    let path = path()?;
    let mut snippets = load_from(&path)?;
    let out = f(&mut snippets)?;
    save_to(&path, &snippets)?;
    Ok(out)
}

/// 随机 id（16 个十六进制字符）。
pub fn new_id() -> String {
    random_bytes::<8>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 随机 UUID v4（`{uuid}` 占位符）。
pub fn uuid_v4() -> String {
    let mut bytes = random_bytes::<16>();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    // SAFETY: 写满给定长度的缓冲区。
    unsafe { libc::arc4random_buf(bytes.as_mut_ptr().cast(), N) };
    bytes
}

/// 新建或保存一条的校验：名字和正文不能空，关键词不能和别的片段重复。
pub fn validate(snippets: &[Snippet], snippet: &Snippet) -> Result<()> {
    if snippet.name.trim().is_empty() {
        bail!(tr("片段要有名字", "A snippet needs a name"));
    }
    if snippet.content.is_empty() {
        bail!(tr("片段内容是空的", "The snippet is empty"));
    }
    let keyword = snippet.keyword.trim();
    if !keyword.is_empty()
        && let Some(other) = snippets
            .iter()
            .find(|other| other.id != snippet.id && other.keyword.trim().eq_ignore_ascii_case(keyword))
    {
        bail!(trf!(
            "关键词「{keyword}」已经给了片段「{}」",
            "The keyword “{keyword}” is already used by “{}”",
            other.name
        ));
    }
    Ok(())
}

/// 逗号（中英文）或空白分隔的标签。
pub fn parse_tags(text: &str) -> Vec<String> {
    text.split([',', '，', ' ', '\n'])
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportReport {
    pub added: usize,
    pub skipped: usize,
}

/// 把导入的片段合进现有列表：名字和正文都相同的算重复，跳过；关键词冲突时清掉新片段的关键词。
pub fn merge(existing: &mut Vec<Snippet>, incoming: Vec<Snippet>) -> ImportReport {
    let mut report = ImportReport {
        added: 0,
        skipped: 0,
    };
    for mut snippet in incoming {
        if existing
            .iter()
            .any(|old| old.name == snippet.name && old.content == snippet.content)
        {
            report.skipped += 1;
            continue;
        }
        if validate(existing, &snippet).is_err() {
            snippet.keyword.clear();
            if validate(existing, &snippet).is_err() {
                report.skipped += 1;
                continue;
            }
        }
        existing.push(snippet);
        report.added += 1;
    }
    report
}

/// 读一个导入文件（或目录）里的片段。
pub fn read_import(path: &Path) -> Result<Vec<Snippet>> {
    if path.is_dir() {
        let mut out = Vec::new();
        let entries = std::fs::read_dir(path)
            .with_context(|| trf!("读取 {}", "Couldn’t read {}", path.display()))?;
        for entry in entries.flatten() {
            let file = entry.path();
            if file.extension().is_some_and(|ext| ext == "json") {
                out.extend(parse_import(&std::fs::read_to_string(&file).with_context(|| {
                    trf!("读取 {}", "Couldn’t read {}", file.display())
                })?)?);
            }
        }
        return Ok(out);
    }
    let text = if path.extension().is_some_and(|ext| ext == "alfredsnippets") {
        // zip 包：unzip -p 把每个 JSON 依次输出，parse_import 按 JSON 流读。
        let output = std::process::Command::new("/usr/bin/unzip")
            .args(["-p"])
            .arg(path)
            .arg("*.json")
            .output()
            .context(tr("运行 unzip 失败", "Couldn’t run unzip"))?;
        if !output.status.success() {
            bail!(trf!(
                "解压 {} 失败（{}）",
                "Couldn’t unpack {} ({})",
                path.display(),
                output.status
            ));
        }
        String::from_utf8_lossy(&output.stdout).into_owned()
    } else {
        std::fs::read_to_string(path)
            .with_context(|| trf!("读取 {}", "Couldn’t read {}", path.display()))?
    };
    parse_import(&text)
}

/// 认三种写法：Raycast 的数组 `[{"name","text","keyword"}]`、Helmp 自己的 `{"snippets": [...]}`、
/// Alfred 的 `{"alfredsnippet": {"name","snippet","keyword"}}`（可以一个接一个）。
pub fn parse_import(text: &str) -> Result<Vec<Snippet>> {
    let mut out = Vec::new();
    for value in serde_json::Deserializer::from_str(text).into_iter::<serde_json::Value>() {
        let value = value.context(tr(
            "导入文件不是合法的 JSON",
            "The import file isn’t valid JSON",
        ))?;
        collect(&value, &mut out);
    }
    if out.is_empty() {
        bail!(tr(
            "导入文件里没有找到片段（支持 Raycast 导出的 JSON 与 Alfred 的 .alfredsnippets）",
            "No snippets found (supported: Raycast’s exported JSON and Alfred’s .alfredsnippets)"
        ));
    }
    Ok(out)
}

fn collect(value: &serde_json::Value, out: &mut Vec<Snippet>) {
    let text = |value: &serde_json::Value, key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    if let Some(items) = value.as_array() {
        for item in items {
            collect(item, out);
        }
    } else if let Some(items) = value.get("snippets").and_then(serde_json::Value::as_array) {
        for item in items {
            collect(item, out);
        }
    } else if let Some(alfred) = value.get("alfredsnippet") {
        let content = alfred_placeholders(&text(alfred, "snippet"));
        push(out, text(alfred, "name"), text(alfred, "keyword"), content);
    } else if value.is_object() {
        let mut content = text(value, "text");
        if content.is_empty() {
            content = text(value, "content");
        }
        push(out, text(value, "name"), text(value, "keyword"), content);
    }
}

fn push(out: &mut Vec<Snippet>, name: String, keyword: String, content: String) {
    if content.is_empty() {
        return;
    }
    let name = if name.trim().is_empty() {
        content.lines().next().unwrap_or_default().chars().take(30).collect()
    } else {
        name
    };
    out.push(Snippet {
        id: new_id(),
        name,
        keyword,
        content,
        tags: Vec::new(),
    });
}

/// Alfred 的占位符换成我们的写法（只换语义一致的；其他原样保留）。
fn alfred_placeholders(text: &str) -> String {
    text.replace("{random:UUID}", "{uuid}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet(name: &str, keyword: &str, content: &str) -> Snippet {
        Snippet {
            id: new_id(),
            name: name.into(),
            keyword: keyword.into(),
            content: content.into(),
            tags: Vec::new(),
        }
    }

    #[test]
    fn imports_raycast_export() {
        // 从 Raycast 迁移的用户最多：它导出的就是一个数组，name / text / keyword。
        let json = r#"[{"name":"Email","text":"me@example.com","keyword":"@@"},{"name":"Sig","text":"— Ada {cursor}"}]"#;
        let snippets = parse_import(json).unwrap();
        assert_eq!(snippets.len(), 2);
        assert_eq!(snippets[0].name, "Email");
        assert_eq!(snippets[0].keyword, "@@");
        assert_eq!(snippets[0].content, "me@example.com");
        assert_eq!(snippets[1].keyword, "");
        assert_ne!(snippets[0].id, snippets[1].id);
    }

    #[test]
    fn imports_alfred_snippet_stream() {
        // .alfredsnippets 解压出来是一个接一个的 JSON 对象，没有外层数组。
        let json = r#"{"alfredsnippet":{"snippet":"id {random:UUID}","uid":"x","name":"ID","keyword":"uid"}}
{"alfredsnippet":{"snippet":"hello","uid":"y","name":"","keyword":""}}"#;
        let snippets = parse_import(json).unwrap();
        assert_eq!(snippets.len(), 2);
        assert_eq!(snippets[0].content, "id {uuid}");
        assert_eq!(snippets[1].name, "hello", "没名字时用正文开头");
        assert!(parse_import("[]").is_err(), "空文件要说清楚，而不是悄悄导入 0 个");
        assert!(parse_import("not json").is_err());
    }

    #[test]
    fn merge_skips_duplicates_and_keyword_clashes() {
        // 同一个文件导入两次不能出现两份；关键词撞了保留片段、去掉新片段的关键词。
        let mut existing = vec![snippet("Email", "@@", "me@example.com")];
        let report = merge(
            &mut existing,
            vec![
                snippet("Email", "@@", "me@example.com"),
                snippet("Work", "@@", "ada@work.com"),
                snippet("", "", ""),
            ],
        );
        assert_eq!(report, ImportReport { added: 1, skipped: 2 });
        assert_eq!(existing[1].keyword, "");
    }

    #[test]
    fn validation_and_round_trip() {
        // 坏文件不能被当成空列表（否则下一次保存会把用户的片段全冲掉）。
        let dir = std::env::temp_dir().join(format!("helmp-snippets-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(FILE);
        assert!(load_from(&file).unwrap().is_empty());
        let list = vec![snippet("签名", "qm", "— 陈")];
        save_to(&file, &list).unwrap();
        assert_eq!(load_from(&file).unwrap(), list);
        std::fs::write(&file, "{broken").unwrap();
        assert!(load_from(&file).is_err());
        let _ = std::fs::remove_dir_all(&dir);

        assert!(validate(&list, &snippet("", "", "x")).is_err());
        assert!(validate(&list, &snippet("x", "", "")).is_err());
        assert!(validate(&list, &snippet("x", "QM", "y")).is_err());
        assert!(validate(&list, &list[0]).is_ok(), "改自己不算重复");
        assert_eq!(parse_tags("工作, 邮件，签名  en"), ["工作", "邮件", "签名", "en"]);
        let uuid = uuid_v4();
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "4");
    }
}
