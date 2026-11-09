//! 无头命令行：不开窗口，直接走 provider，用于测试和端到端验证。
//!
//! ```text
//! helmp query <text>            打印结果：badge \t title \t subtitle \t id
//! helmp run <id> [query]        执行某个结果（先用同一 query 查出来）
//! helmp providers               列出已注册的 provider
//! ```

use anyhow::{Context as _, Result, bail};

use crate::engine::Engine;
use crate::i18n::{tr, trf};
use crate::provider::{Block, Outcome};

/// 返回 `None` 表示不是 CLI 子命令，应启动图形界面。
pub fn run(engine: &Engine, args: &[String]) -> Option<Result<()>> {
    let command = args.first()?;
    let result = match command.as_str() {
        "query" => query(engine, &args[1..].join(" ")),
        "run" => run_item(
            engine,
            args.get(1).map(String::as_str),
            &args[2.min(args.len())..].join(" "),
        ),
        "providers" => {
            for id in engine.provider_ids() {
                println!("{id}");
            }
            Ok(())
        }
        _ => return None,
    };
    Some(result)
}

fn start(engine: &Engine) {
    for (id, err) in engine.start_blocking() {
        eprintln!(
            "helmp: {}",
            trf!("provider {id} 启动失败：{err:#}", "provider {id} failed to start: {err:#}")
        );
    }
}

/// 查询时识别范围前缀（如 `cb `），与图形界面一致。
fn lookup(engine: &Engine, text: &str) -> Vec<crate::provider::Item> {
    match engine.resolve_prefix(text) {
        Some((scope, rest)) => {
            let filter = scope.filters.first().cloned().unwrap_or_default();
            engine.query_scoped(scope.id, &rest, &filter)
        }
        None => engine.query(text),
    }
}

fn query(engine: &Engine, text: &str) -> Result<()> {
    start(engine);
    for item in lookup(engine, text) {
        println!(
            "{}\t{}\t{}\t{}",
            item.badge, item.title, item.subtitle, item.id
        );
    }
    Ok(())
}

fn run_item(engine: &Engine, id: Option<&str>, text: &str) -> Result<()> {
    let id = id.context(tr("用法：helmp run <id> [query]", "Usage: helmp run <id> [query]"))?;
    start(engine);
    let Some(item) = lookup(engine, text).into_iter().find(|item| item.id == id) else {
        bail!(trf!("查询 {text:?} 的结果里没有 {id}", "No result {id} for query {text:?}"));
    };
    match item.action.run()? {
        Outcome::Done => println!("done"),
        Outcome::Message(message) => println!("{message}"),
        Outcome::EnterScope(scope) => println!("enter scope {scope}"),
        Outcome::EnterScopeWith(scope, text) => println!("enter scope {scope}: {text}"),
        Outcome::OpenSettings => println!("open settings"),
        Outcome::Show(detail) => {
            println!("{}", detail.title);
            for block in detail.blocks {
                match block {
                    Block::Text(text) | Block::Code(text) => println!("{text}"),
                    Block::Quote { label, text } => println!("{label}{}{text}", tr("：", ": ")),
                    Block::Image(path) => {
                        println!("{}", trf!("[图片 {}]", "[Image {}]", path.display()))
                    }
                }
            }
            for (key, value) in detail.metadata {
                println!("{key}{}{value}", tr("：", ": "));
            }
        }
    }
    Ok(())
}
