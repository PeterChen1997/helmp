//! 已读回执存在 Helmp 的数据目录，只记录会话 ID 和看过的完成轮次。

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use anyhow::Result;

type Receipts = BTreeMap<String, String>;
static SEEN: OnceLock<Mutex<Receipts>> = OnceLock::new();

fn path() -> Result<PathBuf> {
    Ok(crate::paths::data_dir()?.join("agent-completions.json"))
}

fn load() -> Result<Receipts> {
    match std::fs::read(path()?) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(Receipts::new()),
        Err(err) => Err(err.into()),
    }
}

pub fn init() {
    SEEN.get_or_init(|| {
        Mutex::new(load().unwrap_or_else(|err| {
            eprintln!("helmp: 读取 Agent 已读记录失败：{err:#}");
            Receipts::new()
        }))
    });
}

pub fn is_read(id: &str, revision: &str) -> bool {
    SEEN.get().is_some_and(|seen| {
        seen.lock()
            .expect("Agent 已读记录锁中毒")
            .get(id)
            .is_some_and(|saved| saved == revision)
    })
}

pub fn mark_read(id: String, revision: String) -> bool {
    mark_read_batch([(id, revision)])
}

pub fn mark_read_batch(completions: impl IntoIterator<Item = (String, String)>) -> bool {
    let Some(seen) = SEEN.get() else { return false };
    let mut seen = seen.lock().expect("Agent 已读记录锁中毒");
    let mut changed = false;
    for (id, revision) in completions {
        if seen.get(&id) != Some(&revision) {
            seen.insert(id, revision);
            changed = true;
        }
    }
    if !changed {
        return false;
    }
    if let Err(err) = save(&seen) {
        eprintln!("helmp: 保存 Agent 已读记录失败：{err:#}");
    }
    true
}

fn save(seen: &Receipts) -> Result<()> {
    let path = path()?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(seen)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}
