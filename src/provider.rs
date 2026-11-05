//! 插件（Provider）接口：所有功能模块都通过它接入启动器。
//!
//! 约定：
//! - `start` 在后台线程调用一次，负责建索引、起监听线程；可以阻塞到首次索引完成。
//! - `query` 在每次输入变化时调用，必须只读内存、不做 IO，几毫秒内返回。
//! - 每个 `Item.id` 在同一 provider 内稳定唯一，格式 `<provider>:<domain id>`。

use std::fmt;
use std::sync::Arc;

use anyhow::Result;

/// 执行一个结果后的去向。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 完成，面板保持隐藏。
    Done,
    /// 完成，并重新打开面板展示一段文字（脚本输出、OCR 结果、错误说明）。
    Message(String),
}

type RunFn = dyn Fn() -> Result<Outcome> + Send + Sync;

/// 结果被选中时要做的事。启动器会先隐藏面板，再在后台线程执行它。
#[derive(Clone)]
pub struct Action(Arc<RunFn>);

impl Action {
    pub fn new(f: impl Fn() -> Result<Outcome> + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// 用 macOS `open` 打开文件、.app 或 URL（含 `claude://`、`codex://` 深链）。
    pub fn open(target: impl Into<String>) -> Self {
        let target = target.into();
        Self::new(move || {
            let status = std::process::Command::new("open").arg(&target).status()?;
            anyhow::ensure!(status.success(), "open {target} 失败：{status}");
            Ok(Outcome::Done)
        })
    }

    pub fn run(&self) -> Result<Outcome> {
        (self.0)()
    }
}

impl fmt::Debug for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Action(..)")
    }
}

#[derive(Debug, Clone)]
pub struct Item {
    /// `<provider>:<domain id>`，同一 provider 内稳定唯一。
    pub id: String,
    pub title: String,
    pub subtitle: String,
    /// 右侧小标签，如「应用」「Claude」「剪贴板」。
    pub badge: String,
    /// 越大越靠前。匹配分见 `matcher::score`，provider 可以再加权。
    pub score: u32,
    pub action: Action,
}

pub trait Provider: Send + Sync {
    /// 短标识，如 `apps`、`agents`。也是 `Item.id` 的前缀。
    fn id(&self) -> &'static str;

    /// 建索引 / 起监听。后台线程调用一次，可阻塞。
    fn start(&self) -> Result<()> {
        Ok(())
    }

    /// 返回与 `query` 匹配的结果。空查询时 provider 自行决定是否返回（如最近的 agent 会话）。
    fn query(&self, query: &str) -> Vec<Item>;
}
