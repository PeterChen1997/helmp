//! `helmp mcp`：给 coding agent（Claude Code、Codex、Cursor）用的 MCP 服务（stdio）。
//!
//! - 开发 Helmp：agent 按用户的需求写插件——指南、脚手架、校验、安装（保持停用，等用户启用）、
//!   演练试运行、发布到插件中心（必须先问用户）。见 `plugin_dev`。
//! - 操作 Helmp：搜索与执行；有副作用的操作经本地套接字交给图形界面，由用户在面板里确认。
//!   见 `operate`、`ipc`。
//! - 连接 agent：`helmp mcp install <claude|codex|cursor>` 与设置里的按钮，见 `connect`。
//!
//! 文档：docs/mcp.md。

pub mod connect;
mod guide;
pub mod ipc;
mod operate;
mod plugin_dev;
mod server;

use anyhow::{Result, bail};

use crate::config::Config;
use crate::engine::Engine;
use crate::i18n::trf;

/// `helmp mcp …`。
pub fn cli(engine: &Engine, args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        None | Some("serve") => server::serve(engine.clone()),
        Some("install") => {
            let Some(agent) = args.get(1).and_then(|name| connect::Agent::parse(name)) else {
                bail!(trf!(
                    "用法：helmp mcp install <claude|codex|cursor>",
                    "Usage: helmp mcp install <claude|codex|cursor>"
                ));
            };
            let installed = connect::install(agent)?;
            println!("{}", connect::describe(agent, &installed));
            Ok(())
        }
        Some("status") => {
            for agent in connect::Agent::ALL {
                let status = connect::status(agent)?;
                println!(
                    "{}\t{}\t{}",
                    agent.slug(),
                    status.label(),
                    agent.config_path()?.display()
                );
            }
            Ok(())
        }
        Some(other) => bail!(trf!(
            "不认识的子命令 {other:?}（可用：helmp mcp、helmp mcp install <claude|codex|cursor>、helmp mcp status）",
            "Unknown subcommand {other:?} (use: helmp mcp, helmp mcp install <claude|codex|cursor>, helmp mcp status)"
        )),
    }
}

/// 从磁盘重新读一次配置：MCP 服务是长时间运行的进程，用户期间可能在图形界面里改了设置（比如启用了插件）。
pub fn fresh_config() -> Config {
    let path = crate::paths::data_dir().map(|dir| dir.join("config.json"));
    path.ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| Config::clone(&crate::config::get()))
}
