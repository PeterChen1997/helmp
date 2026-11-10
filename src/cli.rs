//! 无头命令行：不开窗口，直接走 provider，用于测试和端到端验证。
//!
//! ```text
//! helmp query <text>            打印结果：badge \t title \t subtitle \t id
//! helmp run <id> [query] [--form k=v]... [--action <标题>] [--then <标题>]
//!                               执行某个结果（先用同一 query 查出来）。--form 填表单字段，
//!                               --action 改为执行 ⌘K 里同名的次级操作，--then 再执行返回详情里的同名操作
//!                               （如确认页的「确定」）。配合 HELMP_DRY_RUN=1 做端到端测试。
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
        "run" => run_item(engine, args.get(1).map(String::as_str), &args[2.min(args.len())..]),
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
    crate::providers::clipboard::disable_capture();
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

/// `run` 的参数：查询词和各个开关。
#[derive(Default)]
struct RunArgs {
    query: Vec<String>,
    form: crate::provider::ActionInput,
    action: Option<String>,
    then: Option<String>,
}

fn parse_run_args(args: &[String]) -> Result<RunArgs> {
    let mut parsed = RunArgs::default();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = |flag: &str| {
            rest.next()
                .cloned()
                .with_context(|| trf!("{flag} 后面缺少参数", "{flag} needs a value"))
        };
        match arg.as_str() {
            "--form" => {
                let pair = value("--form")?;
                let (key, val) = pair
                    .split_once('=')
                    .with_context(|| trf!("--form 要写成 键=值：{pair}", "--form expects key=value: {pair}"))?;
                parsed.form.form.insert(key.to_string(), val.to_string());
            }
            "--action" => parsed.action = Some(value("--action")?),
            "--then" => parsed.then = Some(value("--then")?),
            _ => parsed.query.push(arg.clone()),
        }
    }
    Ok(parsed)
}

fn run_item(engine: &Engine, id: Option<&str>, args: &[String]) -> Result<()> {
    let id = id.context(tr("用法：helmp run <id> [query]", "Usage: helmp run <id> [query]"))?;
    let args = parse_run_args(args)?;
    let text = args.query.join(" ");
    start(engine);
    let Some(item) = lookup(engine, &text).into_iter().find(|item| item.id == id) else {
        bail!(trf!("查询 {text:?} 的结果里没有 {id}", "No result {id} for query {text:?}"));
    };
    let action = match &args.action {
        None => item.action.clone(),
        Some(title) => find_action(&item.actions, title)?,
    };
    let outcome = action.run_with(&args.form)?;
    let detail_actions = match &outcome {
        Outcome::Show(detail) => detail.actions.clone(),
        _ => Vec::new(),
    };
    print_outcome(outcome);
    if let Some(title) = &args.then {
        let next = find_action(&detail_actions, title)?;
        print_outcome(next.run_with(&args.form)?);
    }
    Ok(())
}

fn find_action(actions: &[crate::provider::NamedAction], title: &str) -> Result<crate::provider::Action> {
    actions
        .iter()
        .find(|named| named.title.eq_ignore_ascii_case(title))
        .map(|named| named.action.clone())
        .with_context(|| {
            let titles: Vec<_> = actions.iter().map(|a| a.title.as_str()).collect();
            trf!(
                "没有名为「{title}」的操作（可选：{}）",
                "No action named {title:?} (available: {})",
                titles.join(" / ")
            )
        })
}

fn print_outcome(outcome: Outcome) {
    match outcome {
        Outcome::Done => println!("done"),
        Outcome::Message(message) => println!("{message}"),
        Outcome::EnterScope(scope) => println!("enter scope {scope}"),
        Outcome::EnterScopeWith(scope, text) => println!("enter scope {scope}: {text}"),
        Outcome::OpenSettings => println!("open settings"),
        Outcome::OpenSettingsPage(page) => println!("open settings: {page:?}"),
        Outcome::Dismiss => println!("dismiss"),
        Outcome::Show(detail) => {
            println!("{}", detail.title);
            for block in detail.blocks {
                match block {
                    Block::Text(text)
                    | Block::Code(text)
                    | Block::Caption(text)
                    | Block::Result { text, .. } => println!("{text}"),
                    Block::Quote { label, text } => println!("{label}{}{text}", tr("：", ": ")),
                    Block::Image(path) => {
                        println!("{}", trf!("[图片 {}]", "[Image {}]", path.display()))
                    }
                }
            }
            for (key, value) in detail.metadata {
                println!("{key}{}{value}", tr("：", ": "));
            }
            // 确认页这类带操作的详情：列出可选操作，第一个是回车。
            if !detail.actions.is_empty() {
                let titles: Vec<_> = detail.actions.iter().map(|a| a.title.as_str()).collect();
                println!("{}{}", tr("操作：", "Actions: "), titles.join(" / "));
            }
        }
    }
}
