//! 最近使用：记录执行过的结果，首页「最近使用」展示，再次回车时按 id 重新查到原结果执行。
//! 存在 `<data_dir>/history.json`，最多保留 `MAX` 条，同一 id 只留最新一次。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::provider::{Icon, Item, Tint};

const MAX: usize = 30;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub badge: String,
    /// 只保存能落盘的图标（文件 / 图片）。
    pub icon: Option<PathBuf>,
    pub icon_is_file: bool,
    /// 内置符号图标（Lucide 名 + 底色）。旧记录没有这个字段。
    #[serde(default)]
    pub symbol: Option<(String, Tint)>,
    /// 当时的查询与范围：重放时用它把原结果查回来。
    pub query: String,
    pub scope: Option<String>,
    pub at_ms: u64,
}

impl Entry {
    pub fn icon(&self) -> Icon {
        if let Some((name, tint)) = &self.symbol {
            return Icon::Symbol {
                name: intern(name),
                tint: *tint,
            };
        }
        match (&self.icon, self.icon_is_file) {
            (Some(path), true) => Icon::File(path.clone()),
            (Some(path), false) => Icon::Image(path.clone()),
            (None, _) => Icon::None,
        }
    }
}

fn path() -> Result<PathBuf> {
    Ok(crate::paths::data_dir()?.join("history.json"))
}

fn entries() -> &'static Mutex<Vec<Entry>> {
    static ENTRIES: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let loaded = path()
            .and_then(|path| match std::fs::read_to_string(&path) {
                Ok(text) => {
                    serde_json::from_str(&text).with_context(|| format!("解析 {}", path.display()))
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
                Err(err) => Err(err).with_context(|| format!("读取 {}", path.display())),
            })
            .unwrap_or_else(|err| {
                eprintln!("helmp: 读取最近使用失败，从空开始：{err:#}");
                Vec::new()
            });
        Mutex::new(loaded)
    })
}

/// 最近使用，新的在前。
pub fn recent() -> Vec<Entry> {
    entries().lock().expect("history poisoned").clone()
}

/// 图标拆成能落盘的三个字段：（图片路径，是否文件图标，符号图标）。
fn stored(icon: &Icon) -> (Option<PathBuf>, bool, Option<(String, Tint)>) {
    match icon {
        Icon::File(path) => (Some(path.clone()), true, None),
        Icon::Image(path) => (Some(path.clone()), false, None),
        Icon::Symbol { name, tint } => (None, false, Some((name.to_string(), *tint))),
        Icon::None => (None, false, None),
    }
}

/// 记录一次执行。失败只打印，不影响执行本身。
pub fn record(item: &Item, query: &str, scope: Option<&str>) {
    let (icon, icon_is_file, symbol) = stored(&item.icon);
    let entry = Entry {
        id: item.id.clone(),
        title: item.title.clone(),
        subtitle: item.subtitle.clone(),
        badge: item.badge.clone(),
        icon,
        icon_is_file,
        symbol,
        query: query.to_string(),
        scope: scope.map(str::to_string),
        at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default(),
    };
    let snapshot = {
        let mut entries = entries().lock().expect("history poisoned");
        entries.retain(|e| e.id != entry.id);
        entries.insert(0, entry);
        entries.truncate(MAX);
        entries.clone()
    };
    save(&snapshot);
}

/// 给没存图标的旧记录补上图标（升级前写下的记录只存了文件 / 图片图标）。
pub fn backfill_icon(id: &str, icon: &Icon) {
    let snapshot = {
        let mut entries = entries().lock().expect("history poisoned");
        let Some(entry) = entries.iter_mut().find(|e| e.id == id) else {
            return;
        };
        (entry.icon, entry.icon_is_file, entry.symbol) = stored(icon);
        entries.clone()
    };
    save(&snapshot);
}

fn save(snapshot: &[Entry]) {
    if let Err(err) = path().and_then(|path| {
        std::fs::write(&path, serde_json::to_string(snapshot)?)
            .with_context(|| format!("写入 {}", path.display()))
    }) {
        eprintln!("helmp: 保存最近使用失败：{err:#}");
    }
}

/// 符号图标名是 `&'static str`：从历史里读回来的名字在这里驻留一次（最多几十个，不会无限增长）。
fn intern(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<std::collections::HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .expect("icon names poisoned");
    if let Some(existing) = names.get(name) {
        return existing;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.insert(leaked);
    leaked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_icons_survive_a_round_trip() {
        // 首页「最近使用」靠这里还原图标：脚本、命令这类符号图标丢了，格子里就是一块空白。
        let entry = Entry {
            id: "scripts:/x/current-time.sh".into(),
            title: "当前时间".into(),
            subtitle: String::new(),
            badge: "脚本".into(),
            icon: None,
            icon_is_file: false,
            symbol: Some(("square-terminal".into(), Tint::Graphite)),
            query: "时间".into(),
            scope: None,
            at_ms: 0,
        };
        let back: Entry = serde_json::from_str(&serde_json::to_string(&entry).unwrap()).unwrap();
        assert_eq!(
            back.icon(),
            Icon::Symbol {
                name: "square-terminal",
                tint: Tint::Graphite
            }
        );
    }

    #[test]
    fn old_entries_without_symbol_still_load() {
        // 升级前写下的 history.json 没有 symbol 字段，读不出来会让最近使用整个消失。
        let old = r#"[{"id":"apps:/Applications/WeChat.app","title":"微信","subtitle":"","badge":"应用","icon":"/Applications/WeChat.app","icon_is_file":true,"query":"wx","scope":null,"at_ms":1}]"#;
        let entries: Vec<Entry> = serde_json::from_str(old).unwrap();
        assert_eq!(
            entries[0].icon(),
            Icon::File("/Applications/WeChat.app".into())
        );
    }
}
