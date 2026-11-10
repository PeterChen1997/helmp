//! 副作用开关：`HELMP_DRY_RUN=1` 时，复制、粘贴、打开、截图、结束进程这类会改动用户环境的动作
//! 只返回「将要做什么」，不真的执行。端到端测试（tests/e2e*.rs）靠它在不碰用户剪贴板、
//! 浏览器和文件的前提下跑完整条「查询 → 执行」链路。
//!
//! 新写的动作凡是会改动用户环境，都要先过 [`dry`]：
//!
//! ```ignore
//! if let Some(outcome) = crate::effects::dry("copy", &text) { return Ok(outcome); }
//! ```

use crate::provider::Outcome;

pub fn dry_run() -> bool {
    std::env::var("HELMP_DRY_RUN").as_deref() == Ok("1")
}

/// 演练模式下返回 `[dry-run] <动作> <内容>` 消息（调用方直接返回它），否则 `None`、照常执行。
pub fn dry(kind: &str, detail: &str) -> Option<Outcome> {
    dry_run().then(|| Outcome::Message(format!("[dry-run] {kind} {detail}")))
}
