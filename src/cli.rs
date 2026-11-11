//! 无头命令行：不开窗口，直接走 provider，用于测试和端到端验证。
//!
//! ```text
//! helmp query [--scope <范围 id>] <text>
//!                               打印结果：badge \t title \t subtitle \t id。--scope 在某个范围里查，
//!                               如 `clipboard`，或命令范围 `plugins:dev.helmp.quicklinks/gh`（相当于选中命令按 Tab）
//! helmp run <id> [query] [--scope <范围 id>] [--form k=v]... [--action <标题>] [--then <标题>]
//!                               执行某个结果（先用同一 query 查出来）。--form 填表单字段，
//!                               --action 改为执行 ⌘K 里同名的次级操作，--then 再执行返回详情里的同名操作
//!                               （如确认页的「确定」）。全局搜索里没有这个结果、而 id 本身是一个命令范围时
//!                               （要输入内容的插件命令、带参数的 MCP 工具），在它的范围里用 query 查：
//!                               `helmp run plugins:dev.helmp.quicklinks/gh rust` 相当于 gh ⇥ rust ↵。
//!                               配合 HELMP_DRY_RUN=1 做端到端测试。
//! helmp providers               列出已注册的 provider
//! helmp plugins                 列出清单插件：状态 \t id \t 版本 \t 来源 \t 名称（出错的附原因）
//! helmp plugins validate <目录> [--json]
//!                               校验一个插件目录（开发者用）：通过打印 OK 与推导出的权限，否则列出问题并失败退出；
//!                               --json 输出 {ok, id, version, commands, permissions, permissions_text, errors}
//! helmp plugins available       列出插件中心的插件：id \t 版本 \t 状态 \t 名称
//! helmp plugins install <id>    从插件中心安装或更新（校验每个文件的 sha256，装好后默认停用，要在设置里确认权限）
//! helmp plugins submit <目录> [--yes]
//!                               以 PR 形式提交到插件中心；不带 --yes 只说明将要做什么并失败退出
//! helmp mcp [install <claude|codex|cursor> | status]
//!                               给 coding agent 用的 MCP 服务（stdio），见 src/mcp 与 docs/mcp.md
//! helmp mcp-client …            管理 MCP 服务器（list / add / remove / tools / import，见 providers/mcp/cli.rs）
//! helmp update check            检查更新：打印 current / latest / status（update-available | up-to-date）/ feed
//! helmp update install [--yes]  下载、校验并安装更新；不带 --yes 只打印计划并失败退出（见 src/updates）
//! ```

use anyhow::{Context as _, Result, bail};

use crate::engine::Engine;
use crate::i18n::{tr, trf};
use crate::provider::{Block, Outcome};

/// 返回 `None` 表示不是 CLI 子命令，应启动图形界面。
pub fn run(engine: &Engine, args: &[String]) -> Option<Result<()>> {
    let command = args.first()?;
    let result = match command.as_str() {
        "query" => query(engine, &args[1..]),
        "run" => run_item(engine, args.get(1).map(String::as_str), &args[2.min(args.len())..]),
        "plugins" => plugins(&args[1..]),
        "mcp" => crate::mcp::cli(engine, &args[1..]),
        "mcp-client" => crate::providers::mcp::cli::run(&args[1..]),
        "update" => crate::updates::cli(&args[1..]),
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

/// `helmp plugins [validate <目录>]`。
fn plugins(args: &[String]) -> Result<()> {
    use crate::providers::plugins::registry::{self, Source, State};
    match args.first().map(String::as_str) {
        None => {
            let config = crate::config::get();
            for entry in registry::load_all() {
                let state = match entry.state(&config) {
                    State::Enabled => "enabled",
                    State::Disabled => "disabled",
                    State::NeedsConsent => "needs-consent",
                    State::Broken => "error",
                };
                let source = match entry.source {
                    Source::Bundled => "bundled",
                    Source::User => "user",
                };
                match &entry.loaded {
                    Ok(plugin) => println!(
                        "{state}\t{}\t{}\t{source}\t{}",
                        entry.id,
                        plugin.manifest.version,
                        plugin.manifest.name.get()
                    ),
                    Err(errors) => {
                        println!("{state}\t{}\t-\t{source}\t-", entry.id);
                        for error in errors {
                            println!("  - {error}");
                        }
                    }
                }
            }
            Ok(())
        }
        Some("validate") => {
            let json = args.iter().any(|arg| arg == "--json");
            let dir = args[1..].iter().find(|arg| *arg != "--json").context(tr(
                "用法：helmp plugins validate <插件目录> [--json]",
                "Usage: helmp plugins validate <plugin folder> [--json]",
            ))?;
            let entry = registry::load_dir(std::path::Path::new(dir), Source::User);
            if json {
                return validate_json(&entry);
            }
            match &entry.loaded {
                Ok(plugin) => {
                    println!(
                        "OK {} {} ({})",
                        plugin.id(),
                        plugin.manifest.version,
                        trf!("{} 个命令", "{} commands", plugin.manifest.commands.len())
                    );
                    let permissions = plugin.permissions.describe();
                    if permissions.is_empty() {
                        println!("{}", tr("权限：无", "Permissions: none"));
                    } else {
                        println!("{}", tr("权限：", "Permissions:"));
                        for line in permissions {
                            println!("  - {line}");
                        }
                    }
                    Ok(())
                }
                Err(errors) => {
                    for error in errors {
                        eprintln!("  - {error}");
                    }
                    bail!(trf!(
                        "插件校验失败：{} 个问题",
                        "Plugin validation failed: {} problem(s)",
                        errors.len()
                    ))
                }
            }
        }
        Some("available") => plugins_available(),
        Some("install") => {
            let id = args.get(1).context(tr(
                "用法：helmp plugins install <插件 id>",
                "Usage: helmp plugins install <plugin id>",
            ))?;
            plugins_install(id)
        }
        Some("submit") => {
            let yes = args.iter().any(|arg| arg == "--yes");
            let dir = args[1..].iter().find(|arg| *arg != "--yes").context(tr(
                "用法：helmp plugins submit <插件目录> [--yes]",
                "Usage: helmp plugins submit <plugin folder> [--yes]",
            ))?;
            plugins_submit(std::path::Path::new(dir), yes)
        }
        Some(other) => bail!(trf!(
            "不认识的子命令 {other:?}（可用：helmp plugins、validate <目录>、available、install <id>、submit <目录>）",
            "Unknown subcommand {other:?} (use: helmp plugins, validate <folder>, available, install <id>, submit <folder>)"
        )),
    }
}

/// `helmp plugins validate <目录> --json`：给注册表脚本和 agent 读的结果。失败时也打印 JSON，再以失败退出。
fn validate_json(entry: &crate::providers::plugins::registry::Entry) -> Result<()> {
    let value = match &entry.loaded {
        Ok(plugin) => serde_json::json!({
            "ok": true,
            "id": plugin.id(),
            "version": plugin.manifest.version,
            "commands": plugin.manifest.commands.len(),
            "permissions": plugin.permissions.keys(),
            "permissions_text": plugin.permissions.describe(),
            "errors": [],
        }),
        Err(errors) => serde_json::json!({
            "ok": false,
            "id": entry.id,
            "version": null,
            "commands": 0,
            "permissions": [],
            "permissions_text": [],
            "errors": errors,
        }),
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    if let Err(errors) = &entry.loaded {
        bail!(trf!(
            "插件校验失败：{} 个问题",
            "Plugin validation failed: {} problem(s)",
            errors.len()
        ));
    }
    Ok(())
}

/// `helmp plugins available`：插件中心里的插件及其在本机的状态。
fn plugins_available() -> Result<()> {
    use crate::providers::plugins::{center, registry};
    let config = crate::config::get();
    let index = center::fetch_index(&center::registry_base(&config))?;
    let entries = registry::load_all();
    for listing in &index.plugins {
        let state = match center::availability(listing, &config, &entries) {
            center::Availability::Bundled => "bundled".to_string(),
            center::Availability::NotInstalled => "not-installed".to_string(),
            center::Availability::Installed => "installed".to_string(),
            center::Availability::UpdateAvailable { installed } => format!("update-available({installed})"),
            center::Availability::LocalConflict => "local-conflict".to_string(),
        };
        println!("{}\t{}\t{state}\t{}", listing.id, listing.version, listing.name.get());
    }
    Ok(())
}

fn plugins_install(id: &str) -> Result<()> {
    use crate::providers::plugins::{center, registry};
    let done = center::install(id)?;
    if done.up_to_date {
        println!(
            "{}",
            trf!("{id} {} 已是最新", "{id} {} is already up to date", done.version)
        );
        return Ok(());
    }
    match &done.previous {
        Some(previous) => println!(
            "{}",
            trf!("已更新 {id}：{previous} → {}", "Updated {id}: {previous} → {}", done.version)
        ),
        None => println!("{}", trf!("已安装 {id} {}", "Installed {id} {}", done.version)),
    }
    println!("{}", done.dir.display());
    let config = crate::config::get();
    let state = registry::load_all()
        .into_iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.state(&config));
    match state {
        Some(registry::State::Enabled) => println!(
            "{}",
            tr("权限没有变化，保持启用。", "Permissions are unchanged; it stays turned on.")
        ),
        Some(registry::State::NeedsConsent) => println!(
            "{}",
            tr(
                "权限有变化：在「设置 › 插件」里重新确认后才会生效。",
                "Its permissions changed: review them in Settings › Plugins to turn it back on."
            )
        ),
        _ => println!(
            "{}",
            tr(
                "插件默认停用：在「设置 › 插件」里查看权限并启用。",
                "It starts turned off: review its permissions and turn it on in Settings › Plugins."
            )
        ),
    }
    Ok(())
}

fn plugins_submit(dir: &std::path::Path, yes: bool) -> Result<()> {
    use crate::providers::plugins::submit;
    let plan = submit::plan(dir)?;
    print!("{}", plan.describe());
    if !yes {
        bail!(tr(
            "还没有提交：确认无误后加 --yes 再运行（替用户操作的 agent 必须先征得用户同意）",
            "Nothing submitted yet: run again with --yes once you've confirmed (agents must ask the user first)"
        ));
    }
    let url = submit::submit(&plan)?;
    println!("{}", trf!("已提交，等待审核：{url}", "Submitted for review: {url}"));
    Ok(())
}

pub(crate) fn start(engine: &Engine) {
    crate::providers::clipboard::disable_capture();
    for (id, err) in engine.start_blocking() {
        eprintln!(
            "helmp: {}",
            trf!("provider {id} 启动失败：{err:#}", "provider {id} failed to start: {err:#}")
        );
    }
}

/// 查询时识别范围前缀（如 `cb `），与图形界面一致。
pub(crate) fn lookup(engine: &Engine, text: &str) -> Vec<crate::provider::Item> {
    match engine.resolve_prefix(text) {
        Some((scope, rest)) => {
            let filter = scope.filters.first().cloned().unwrap_or_default();
            engine.query_scoped(&scope.id, &rest, &filter)
        }
        None => engine.query(text),
    }
}

/// 在某个范围里查（插件范围或命令范围，用默认筛选）；没给范围时同 [`lookup`]。
pub(crate) fn lookup_in(
    engine: &Engine,
    scope: Option<&str>,
    text: &str,
) -> Vec<crate::provider::Item> {
    let Some(scope) = scope else {
        return lookup(engine, text);
    };
    let filter = engine
        .scope(scope)
        .and_then(|scope| scope.filters.first().cloned())
        .unwrap_or_default();
    engine.query_scoped(scope, text, &filter)
}

/// 找出要执行的结果（CLI `run`、MCP 服务的 `helmp_run`）：给了范围就在范围里找；否则在全局搜索里找，
/// 找不到而 `id` 本身是一个命令范围（要输入内容的命令）时，在它的范围里用 `text` 找
/// ——`helmp run plugins:…/gh rust` 相当于在面板里选中 gh、按 Tab、输入 rust、回车。
pub(crate) fn find(
    engine: &Engine,
    id: &str,
    text: &str,
    scope: Option<&str>,
) -> Option<crate::provider::Item> {
    let matches = |item: &crate::provider::Item| item.id == id;
    if scope.is_some() {
        return lookup_in(engine, scope, text).into_iter().find(matches);
    }
    lookup(engine, text).into_iter().find(matches).or_else(|| {
        engine.scope(id)?;
        engine.query_scoped(id, text, "").into_iter().find(matches)
    })
}

/// `query` 的参数：`--scope <id>` 加查询词。
fn split_scope(args: &[String]) -> Result<(Option<String>, String)> {
    let mut scope = None;
    let mut words = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--scope" {
            scope = Some(
                rest.next()
                    .cloned()
                    .context(tr("--scope 后面缺少范围 id", "--scope needs a scope id"))?,
            );
        } else {
            words.push(arg.as_str());
        }
    }
    Ok((scope, words.join(" ")))
}

fn query(engine: &Engine, args: &[String]) -> Result<()> {
    let (scope, text) = split_scope(args)?;
    start(engine);
    if let Some(scope) = &scope
        && engine.scope(scope).is_none()
    {
        bail!(trf!("没有范围 {scope}", "No scope {scope}"));
    }
    for item in lookup_in(engine, scope.as_deref(), &text) {
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
    scope: Option<String>,
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
            "--scope" => parsed.scope = Some(value("--scope")?),
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
    let Some(item) = find(engine, id, &text, args.scope.as_deref()) else {
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
    println!("{}", outcome_text(outcome));
}

/// 执行结果的纯文本形式（CLI 输出、MCP 服务返回给 agent 的文字共用）。
pub(crate) fn outcome_text(outcome: Outcome) -> String {
    let mut lines = Vec::new();
    match outcome {
        Outcome::Done => lines.push("done".to_string()),
        Outcome::Message(message) => lines.push(message),
        Outcome::EnterScope(scope) => lines.push(format!("enter scope {scope}")),
        Outcome::EnterScopeWith(scope, text) => lines.push(format!("enter scope {scope}: {text}")),
        Outcome::OpenSettingsPage(page) => lines.push(format!("open settings: {page:?}")),
        Outcome::Dismiss => lines.push("dismiss".to_string()),
        Outcome::Capture(mode) => lines.push(format!("open capture: {}", mode.name())),
        Outcome::Show(detail) => {
            lines.push(detail.title);
            for block in detail.blocks {
                match block {
                    Block::Text(text)
                    | Block::Code(text)
                    | Block::Caption(text)
                    | Block::Result { text, .. } => lines.push(text),
                    Block::Quote { label, text } | Block::Markdown { label, text } => {
                        lines.push(format!("{label}{}{text}", tr("：", ": ")))
                    }
                    Block::Image(path) => {
                        lines.push(trf!("[图片 {}]", "[Image {}]", path.display()))
                    }
                }
            }
            for (key, value) in detail.metadata {
                lines.push(format!("{key}{}{value}", tr("：", ": ")));
            }
            // 确认页这类带操作的详情：列出可选操作，第一个是回车。
            if !detail.actions.is_empty() {
                let titles: Vec<_> = detail.actions.iter().map(|a| a.title.as_str()).collect();
                lines.push(format!("{}{}", tr("操作：", "Actions: "), titles.join(" / ")));
            }
        }
    }
    lines.join("\n")
}
