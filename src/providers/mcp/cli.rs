//! `helmp mcp-client …`：脚本与测试用的 MCP 服务器管理（`helmp mcp` 留给「Helmp 作为 MCP 服务器」）。
//!
//! ```text
//! helmp mcp-client list                      状态 \t 名字 \t 来源 \t 工具数 \t 命令或地址
//! helmp mcp-client add <名字> [--env K=V]... [--resident] -- <命令> [参数...]
//! helmp mcp-client add <名字> --url <地址> [--header "K: V"]...
//! helmp mcp-client add --json '<{"mcpServers": {...}}>'
//! helmp mcp-client remove <名字>
//! helmp mcp-client tools <名字>              启动服务器并列出工具：名字 \t 标题 \t 需确认 \t 描述
//! helmp mcp-client import [--from claude|codex|cursor] [--project <目录>] [--list] [名字...]
//!                                            从别的客户端导入（只读那些文件）；--list 只列出，不给名字导入全部
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};

use super::import::{self, Found, Source};
use super::manager;
use super::servers::{self, ServerConfig, Spec, Transport};
use crate::i18n::{tr, trf};

const USAGE: &str = "helmp mcp-client list | add | remove <name> | tools <name> | import";

pub fn run(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        None | Some("list") => list(),
        Some("add") => add(&args[1..]),
        Some("remove") => remove(args.get(1).context(USAGE)?),
        Some("tools") => tools(args.get(1).context(USAGE)?),
        Some("import") => import_cmd(&args[1..]),
        Some(other) => bail!(trf!(
            "不认识的子命令 {other:?}（可用：{USAGE}）",
            "Unknown subcommand {other:?} (use: {USAGE})"
        )),
    }
}

fn list() -> Result<()> {
    let config = crate::config::get();
    let manager = manager::global();
    for spec in servers::all(&config) {
        let status = if spec.enabled {
            manager.info(&spec.key).status.word()
        } else {
            "disabled"
        };
        let count = manager
            .cached_tools(&spec)
            .map_or_else(|| "-".to_string(), |tools| tools.len().to_string());
        let origin = match &spec.origin {
            servers::Origin::User => "user".to_string(),
            servers::Origin::Plugin { id, .. } => format!("plugin:{id}"),
        };
        println!(
            "{status}\t{}\t{origin}\t{count}\t{}",
            spec.key,
            spec.transport.summary()
        );
    }
    Ok(())
}

/// 加进配置：密钥先挪进钥匙串，再写 config.json。
pub fn save_new(server: ServerConfig) -> Result<String> {
    servers::upsert(None, server, &[])
}

fn add(args: &[String]) -> Result<()> {
    let mut name: Option<String> = None;
    let mut url: Option<String> = None;
    let mut env = BTreeMap::new();
    let mut headers = BTreeMap::new();
    let mut resident = false;
    let mut command: Vec<String> = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = |flag: &str| {
            rest.next()
                .cloned()
                .with_context(|| trf!("{flag} 后面缺少参数", "{flag} needs a value"))
        };
        match arg.as_str() {
            "--json" => {
                let found = import::parse_pasted(&value("--json")?).map_err(anyhow::Error::msg)?;
                return save_found(found);
            }
            "--url" => url = Some(value("--url")?),
            "--env" => {
                let pair = value("--env")?;
                let (k, v) = pair
                    .split_once('=')
                    .with_context(|| trf!("--env 要写成 K=V：{pair}", "--env expects K=V: {pair}"))?;
                env.insert(k.to_string(), v.to_string());
            }
            "--header" => {
                let pair = value("--header")?;
                let (k, v) = pair.split_once(':').with_context(|| {
                    trf!("--header 要写成 \"名字: 值\"：{pair}", "--header expects \"Name: value\": {pair}")
                })?;
                headers.insert(k.trim().to_string(), v.trim().to_string());
            }
            "--resident" => resident = true,
            "--" => {
                command = rest.by_ref().cloned().collect();
            }
            _ if name.is_none() => name = Some(arg.clone()),
            _ => command.push(arg.clone()),
        }
    }
    let name = name.context(tr(
        "用法：helmp mcp-client add <名字> -- <命令> [参数…] 或 --url <地址>",
        "Usage: helmp mcp-client add <name> -- <command> [args…] or --url <url>",
    ))?;
    let transport = match (url, command.split_first()) {
        (Some(url), None) => Transport::Http { url, headers },
        (None, Some((command, args))) => Transport::Stdio {
            command: command.clone(),
            args: args.to_vec(),
            env,
            cwd: None,
        },
        _ => bail!(tr(
            "要么给命令（-- 之后），要么给 --url，不能都给或都不给",
            "Give either a command (after --) or --url"
        )),
    };
    let saved = save_new(ServerConfig {
        name,
        enabled: true,
        resident,
        transport,
        secrets: Vec::new(),
        timeout_secs: None,
    })?;
    println!("{}", trf!("已添加 {saved}", "Added {saved}"));
    Ok(())
}

fn save_found(found: Vec<Found>) -> Result<()> {
    let mut failed = 0;
    for found in found {
        let result = found
            .server
            .clone()
            .map_err(anyhow::Error::msg)
            .and_then(save_new);
        match result {
            Ok(name) => {
                println!("{}", trf!("已添加 {name}", "Added {name}"));
                if let Some(note) = &found.note {
                    println!("  {note}");
                }
            }
            Err(err) => {
                failed += 1;
                eprintln!("{}: {err:#}", found.name);
            }
        }
    }
    if failed > 0 {
        bail!(trf!("{failed} 个没有添加", "{failed} not added"));
    }
    Ok(())
}

fn remove(name: &str) -> Result<()> {
    servers::remove(name)?;
    println!("{}", trf!("已删除 {name}", "Removed {name}"));
    Ok(())
}

fn find_spec(key: &str) -> Result<Spec> {
    servers::find(&crate::config::get(), key)
        .with_context(|| trf!("没有服务器 {key}", "No server {key}"))
}

fn tools(key: &str) -> Result<()> {
    let spec = find_spec(key)?;
    let manager = manager::global();
    let result = manager.reload_tools(&spec);
    let tools = match result {
        Ok(tools) => tools,
        Err(err) if manager::is_dry_run_blocked(&err) => {
            println!("[dry-run] mcp-start {} {}", spec.key, spec.transport.summary());
            return Ok(());
        }
        Err(err) => return Err(err),
    };
    manager.stop(&spec.key);
    for tool in tools {
        let confirm = if super::schema::needs_confirmation(&tool.annotations) {
            "confirm"
        } else {
            "-"
        };
        println!(
            "{}\t{}\t{confirm}\t{}",
            tool.name,
            tool.display_title(),
            tool.description.lines().next().unwrap_or_default()
        );
    }
    Ok(())
}

fn import_cmd(args: &[String]) -> Result<()> {
    let mut sources: Vec<Source> = Vec::new();
    let mut project: Option<PathBuf> = None;
    let mut only_list = false;
    let mut names: Vec<String> = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--from" => {
                let word = rest.next().context("--from")?;
                sources.push(Source::from_word(word).with_context(|| {
                    trf!(
                        "不认识的来源 {word:?}（可用：claude、codex、cursor）",
                        "Unknown source {word:?} (use claude, codex or cursor)"
                    )
                })?);
            }
            "--project" => project = Some(PathBuf::from(rest.next().context("--project")?)),
            "--list" => only_list = true,
            _ => names.push(arg.clone()),
        }
    }
    if sources.is_empty() {
        sources = vec![Source::Claude, Source::Codex, Source::Cursor];
    }
    let found: Vec<Found> = sources
        .into_iter()
        .flat_map(|source| import::scan(source, project.as_deref()))
        .collect();
    let existing: Vec<String> = crate::config::get()
        .mcp
        .servers
        .iter()
        .map(|s| s.name.clone())
        .collect();
    if only_list {
        for found in &found {
            let state = match &found.server {
                Err(reason) => format!("unsupported: {reason}"),
                Ok(_) if existing.contains(&found.name) => "exists".into(),
                Ok(_) => "new".into(),
            };
            let summary = found
                .server
                .as_ref()
                .map(|s| s.transport.summary())
                .unwrap_or_default();
            println!(
                "{}\t{}\t{state}\t{summary}",
                found.source.label(),
                found.name
            );
        }
        return Ok(());
    }
    let mut seen = std::collections::BTreeSet::new();
    let picked: Vec<Found> = found
        .into_iter()
        .filter(|found| names.is_empty() || names.contains(&found.name))
        .filter(|found| found.server.is_ok() && !existing.contains(&found.name))
        // 几个客户端里同名的服务器只导入第一个。
        .filter(|found| seen.insert(found.name.clone()))
        .collect();
    if picked.is_empty() {
        println!("{}", tr("没有可以导入的新服务器", "No new servers to import"));
        return Ok(());
    }
    save_found(picked)
}
