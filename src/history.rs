//! 最近使用：记录执行过的结果，首页「最近使用」展示，再次回车时按 id 重新查到原结果执行。
//! 存在 `<data_dir>/history.json`，最多保留 `MAX` 条，同一 id 只留最新一次。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::provider::{Icon, Item};

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
    /// 当时的查询与范围：重放时用它把原结果查回来。
    pub query: String,
    pub scope: Option<String>,
    pub at_ms: u64,
}

impl Entry {
    pub fn icon(&self) -> Icon {
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

/// 记录一次执行。失败只打印，不影响执行本身。
pub fn record(item: &Item, query: &str, scope: Option<&str>) {
    let (icon, icon_is_file) = match &item.icon {
        Icon::File(path) => (Some(path.clone()), true),
        Icon::Image(path) => (Some(path.clone()), false),
        _ => (None, false),
    };
    let entry = Entry {
        id: item.id.clone(),
        title: item.title.clone(),
        subtitle: item.subtitle.clone(),
        badge: item.badge.clone(),
        icon,
        icon_is_file,
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
    if let Err(err) = path().and_then(|path| {
        std::fs::write(&path, serde_json::to_string(&snapshot)?)
            .with_context(|| format!("写入 {}", path.display()))
    }) {
        eprintln!("helmp: 保存最近使用失败：{err:#}");
    }
}
