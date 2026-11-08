//! 无头命令行：不开窗口，直接走 provider，用于测试和端到端验证。
//!
//! ```text
//! helmp query <text>            打印结果：badge \t title \t subtitle \t id
//! helmp run <id> [query]        执行某个结果（先用同一 query 查出来）
//! helmp providers               列出已注册的 provider
//! ```

use anyhow::{Context as _, Result, bail};

use crate::engine::Engine;
use crate::provider::Outcome;

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
        eprintln!("helmp: provider {id} 启动失败：{err:#}");
    }
}

fn query(engine: &Engine, text: &str) -> Result<()> {
    start(engine);
    for item in engine.query(text) {
        println!(
            "{}\t{}\t{}\t{}",
            item.badge, item.title, item.subtitle, item.id
        );
    }
    Ok(())
}

fn run_item(engine: &Engine, id: Option<&str>, text: &str) -> Result<()> {
    let id = id.context("用法：helmp run <id> [query]")?;
    start(engine);
    let Some(item) = engine.query(text).into_iter().find(|item| item.id == id) else {
        bail!("查询 {text:?} 的结果里没有 {id}");
    };
    match item.action.run()? {
        Outcome::Done => println!("done"),
        Outcome::Message(message) => println!("{message}"),
    }
    Ok(())
}
