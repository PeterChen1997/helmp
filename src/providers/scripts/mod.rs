//! L1 脚本插件：兼容 Raycast Script Commands。
//!
//! - 目录：`<data_dir>/scripts/` + `config.script_dirs` + 环境变量 `HELMP_SCRIPT_DIRS`（冒号分隔），
//!   递归扫描，跳过隐藏文件。目录列表每个轮询周期重读，设置里改了不用重启。
//! - 每 5 秒按 (修改时间, 大小, 可执行位) 检查变化，只重读变了的文件。
//! - 参数：有参数的脚本返回表单（每个参数一个字段）；查询以脚本标题或 packageName 开头时，
//!   余下文本作为表单初值（见 `args`）。
//! - 输出：compact / fullOutput / inline 在详情里展示（等宽），silent 跑完不打扰。
//! - `needsConfirmation: true` 的脚本回车后先显示确认页（`Detail::confirm`），再回车才运行；
//!   查询以 ` !` 结尾时视为已确认，直接运行（兼容老习惯）。

pub(super) mod args;
mod header;
pub(super) mod runner;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result};

use crate::i18n::{tr, trf};
use crate::matcher;
use crate::provider::{
    Action, ActionInput, Block, Detail, Field, FieldKind, Form, Icon, Item, NamedAction, Outcome,
    Provider, Tint,
};
use header::{ArgKind, Argument, Header, Parsed};

const ICON: Icon = Icon::Symbol {
    name: "square-terminal",
    tint: Tint::Graphite,
};
fn badge() -> &'static str {
    tr("脚本", "Scripts")
}

const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// 只在文件开头这么多字节里找头注释，避免把大文件整个读进来。
const HEADER_BYTES: u64 = 64 * 1024;
/// 递归深度上限，同时防住符号链接成环。
const MAX_DEPTH: usize = 16;
/// 查询以完整标题开头（后面是参数）时的分数，等同完全匹配。
const TITLE_PREFIX_SCORE: u32 = 1000;
/// 查询以 packageName 开头（后面是参数）时的分数，低于标题。
const PACKAGE_PREFIX_SCORE: u32 = 850;
/// packageName 模糊匹配相对标题匹配的降权。
const PACKAGE_PENALTY: u32 = 100;

#[derive(Debug)]
pub struct Script {
    pub path: PathBuf,
    pub header: Header,
}

impl Script {
    /// 运行并返回要展示的输出（silent 为 `None`）与退出码。非零退出已在 runner 里变成错误。
    fn run(&self, args: &[String], timeout: Duration) -> Result<(Option<String>, i32)> {
        let cwd = runner::working_dir(self.header.current_directory_path.as_deref(), &self.path)?;
        let output = runner::execute(&self.path, args, &cwd, timeout)?;
        let shown = runner::shown_output(&self.header.title, self.header.mode, &output)?;
        Ok((shown, output.status.code().unwrap_or_default()))
    }

    /// 详情：输出（等宽）在上，说明 / 脚本 / 来源 / 退出码在下。未运行时只有元数据。
    fn detail(&self, display_path: &str, ran: Option<(&str, i32)>) -> Detail {
        let header = &self.header;
        let mut metadata = Vec::new();
        if let Some(description) = &header.description {
            metadata.push((tr("说明", "Description").to_string(), description.clone()));
        }
        metadata.push((tr("脚本", "Script").to_string(), display_path.to_string()));
        let dialect = if header.helmp_only {
            tr("Helmp 脚本", "Helmp Script")
        } else {
            "Raycast Script Commands"
        };
        let mode = header.mode.name();
        metadata.push((
            tr("来源", "Source").to_string(),
            trf!("{dialect} · 输出模式 {mode}", "{dialect} · {mode} mode"),
        ));
        let mut blocks = Vec::new();
        if let Some((output, code)) = ran {
            blocks.push(Block::Code(output.to_string()));
            metadata.push((tr("退出码", "Exit Code").to_string(), code.to_string()));
        }
        Detail {
            actions: Vec::new(),
            icon: ICON,
            title: header.title.clone(),
            tag: None,
            blocks,
            metadata,
        }
    }
}

/// 文件指纹：任一变化就重新解析。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
    executable: bool,
}

#[derive(Debug, Clone)]
enum Entry {
    /// 没有元数据，不是脚本。
    Ignored,
    Skipped(String),
    Script(Arc<Script>),
}

#[derive(Debug, Default)]
struct Index {
    files: HashMap<PathBuf, (Stamp, Entry)>,
    /// 按路径排序的可用脚本，`query` 只读它。
    scripts: Vec<Arc<Script>>,
    /// 已经打印过的目录读取问题，避免每 5 秒刷屏。
    reported: HashSet<String>,
    /// 当前扫描的根目录（绝对路径），详情里的「脚本」显示相对它们的路径。
    roots: Vec<PathBuf>,
}

/// 每个脚本最近一次的输出，⌘K「复制输出」用。
type Outputs = Arc<Mutex<HashMap<PathBuf, String>>>;

pub struct Scripts {
    /// `None` 表示按默认规则解析（数据目录 + 配置 + 环境变量），且每个轮询周期重读。
    dirs: Option<Vec<PathBuf>>,
    index: Arc<RwLock<Index>>,
    outputs: Outputs,
}

impl Scripts {
    pub fn new() -> Self {
        Self {
            dirs: None,
            index: Arc::default(),
            outputs: Outputs::default(),
        }
    }
}

impl Provider for Scripts {
    fn id(&self) -> &'static str {
        "scripts"
    }

    fn start(&self) -> Result<()> {
        let fixed = self.dirs.clone();
        let mut configured = match &fixed {
            Some(dirs) => dirs.clone(),
            None => script_dirs()?,
        };
        let mut dirs: Vec<PathBuf> = configured.iter().map(|dir| absolute_dir(dir)).collect();
        rescan(&self.index, &dirs, true);
        let index = Arc::clone(&self.index);
        std::thread::Builder::new()
            .name("helmp-scripts-watch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(POLL_INTERVAL);
                    if fixed.is_none() {
                        match script_dirs() {
                            Ok(next) if next != configured => {
                                dirs = next.iter().map(|dir| absolute_dir(dir)).collect();
                                configured = next;
                            }
                            Ok(_) => {}
                            Err(err) => eprintln!("helmp: scripts 读取脚本目录配置失败：{err:#}"),
                        }
                    }
                    rescan(&index, &dirs, false);
                }
            })
            .with_context(|| {
                tr(
                    "启动脚本目录轮询线程",
                    "Couldn't start the script folder watcher",
                )
            })?;
        Ok(())
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let index = self
            .index
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        index
            .scripts
            .iter()
            .filter_map(|script| item_for(script, query, &index.roots, &self.outputs))
            .collect()
    }
}

/// 默认脚本目录（不存在就创建，方便用户找到放哪）+ `config.script_dirs` + `HELMP_SCRIPT_DIRS`，去重。
fn script_dirs() -> Result<Vec<PathBuf>> {
    let default = crate::paths::data_dir()?.join("scripts");
    fs::create_dir_all(&default)
        .with_context(|| trf!("创建 {}", "Couldn't create {}", default.display()))?;
    let home = crate::paths::home_dir()?;
    let configured = crate::config::get()
        .script_dirs
        .iter()
        .filter(|dir| !dir.trim().is_empty())
        .map(|dir| expand_tilde(dir.trim(), &home))
        .collect();
    let env: Vec<PathBuf> = std::env::var_os("HELMP_SCRIPT_DIRS")
        .map(|extra| {
            std::env::split_paths(&extra)
                .filter(|dir| !dir.as_os_str().is_empty())
                .collect()
        })
        .unwrap_or_default();
    Ok(merge_dirs(default, configured, env))
}

fn merge_dirs(default: PathBuf, configured: Vec<PathBuf>, env: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut dirs = vec![default];
    for dir in configured.into_iter().chain(env) {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

fn expand_tilde(dir: &str, home: &Path) -> PathBuf {
    match dir {
        "~" => home.to_path_buf(),
        _ => match dir.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(dir),
        },
    }
}

/// 详情里显示的脚本路径：相对所在的脚本目录（如 `developer-utils/count-characters.sh`）。
fn display_path(path: &Path, roots: &[PathBuf]) -> String {
    roots
        .iter()
        .filter_map(|root| path.strip_prefix(root).ok())
        .min_by_key(|relative| relative.components().count())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// 规范成绝对路径（Item id 要用绝对路径）。目录暂不存在也保留，出现后轮询会自动加载。
fn absolute_dir(dir: &Path) -> PathBuf {
    match fs::canonicalize(dir) {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!(
                "helmp: scripts 目录 {} 暂不可用：{err}（出现后会自动加载）",
                dir.display()
            );
            std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf())
        }
    }
}

fn walk(dir: &Path, depth: usize, found: &mut HashMap<PathBuf, Stamp>, problems: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        // 根目录不存在已在启动时说过；子目录在两次轮询之间被删属于正常变化。
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => {
            problems.push(format!("读取目录 {} 失败：{err}", dir.display()));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                problems.push(format!("读取目录 {} 失败：{err}", dir.display()));
                continue;
            }
        };
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        // 跟随符号链接：用户常把脚本仓库链进来。
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(err) => {
                problems.push(format!("读取 {} 失败：{err}", path.display()));
                continue;
            }
        };
        if metadata.is_dir() {
            if depth + 1 >= MAX_DEPTH {
                problems.push(format!(
                    "{} 超过最大递归深度 {MAX_DEPTH}，不再深入",
                    path.display()
                ));
            } else {
                walk(&path, depth + 1, found, problems);
            }
        } else if metadata.is_file() {
            let stamp = Stamp {
                modified: metadata.modified().ok(),
                len: metadata.len(),
                executable: metadata.permissions().mode() & 0o111 != 0,
            };
            found.insert(path, stamp);
        }
    }
}

fn parse_file(path: &Path, stamp: &Stamp) -> Entry {
    let mut bytes = Vec::new();
    let read =
        fs::File::open(path).and_then(|file| file.take(HEADER_BYTES).read_to_end(&mut bytes));
    if let Err(err) = read {
        return Entry::Skipped(format!("读取失败：{err}"));
    }
    let text = String::from_utf8_lossy(&bytes);
    match header::parse(&text) {
        Parsed::NotAScript => Entry::Ignored,
        Parsed::Skipped(reason) => Entry::Skipped(reason),
        // 没有 shebang 又不可执行的（如 README 里的示例代码块）无法运行。
        Parsed::Script(_) if !stamp.executable && !text.starts_with("#!") => {
            Entry::Skipped("既没有 shebang 也不可执行".into())
        }
        Parsed::Script(header) => Entry::Script(Arc::new(Script {
            path: path.to_path_buf(),
            header,
        })),
    }
}

/// 扫描全部目录，只重新解析指纹变了的文件；有变化才替换索引并打印汇总。
fn rescan(index: &RwLock<Index>, dirs: &[PathBuf], initial: bool) {
    let mut found = HashMap::new();
    let mut problems = Vec::new();
    for dir in dirs {
        walk(dir, 0, &mut found, &mut problems);
    }

    let (mut files, to_parse, changed, new_problems) = {
        let index = index
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut files = HashMap::with_capacity(found.len());
        let mut to_parse = Vec::new();
        for (path, stamp) in found {
            match index.files.get(&path) {
                Some((old, entry)) if *old == stamp => {
                    files.insert(path, (stamp, entry.clone()));
                }
                _ => to_parse.push((path, stamp)),
            }
        }
        // 没有要重新解析的文件时，复用的条目都来自旧索引；数量变少就说明有文件被删。
        let changed = !to_parse.is_empty() || files.len() != index.files.len();
        let new_problems: Vec<String> = problems
            .into_iter()
            .filter(|problem| !index.reported.contains(problem))
            .collect();
        (files, to_parse, changed, new_problems)
    };

    for problem in &new_problems {
        eprintln!("helmp: scripts {problem}");
    }
    for (path, stamp) in to_parse {
        let entry = parse_file(&path, &stamp);
        if let Entry::Skipped(reason) = &entry {
            eprintln!("helmp: 跳过脚本 {}：{reason}", path.display());
        }
        files.insert(path, (stamp, entry));
    }

    let mut index = index
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    index.reported.extend(new_problems);
    index.roots = dirs.to_vec();
    if !changed && !initial {
        return;
    }
    let mut scripts: Vec<Arc<Script>> = files
        .values()
        .filter_map(|(_, entry)| match entry {
            Entry::Script(script) => Some(Arc::clone(script)),
            Entry::Ignored | Entry::Skipped(_) => None,
        })
        .collect();
    scripts.sort_by(|a, b| a.path.cmp(&b.path));
    let skipped = files
        .values()
        .filter(|(_, entry)| matches!(entry, Entry::Skipped(_)))
        .count();
    eprintln!(
        "helmp: scripts 已加载 {} 个脚本，跳过 {skipped} 个",
        scripts.len()
    );
    index.files = files;
    index.scripts = scripts;
}

/// 查询与脚本的匹配：返回 (分数, 参数文本)。
fn match_query<'a>(header: &Header, query: &'a str) -> Option<(u32, &'a str)> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    // 只有声明了参数的脚本才把「名字 + 余下文本」当参数，否则 `ip foo` 也会命中 `ip`。
    if !header.arguments.is_empty() {
        if let Some(rest) = args::strip_name(query, &header.title) {
            return Some((TITLE_PREFIX_SCORE, rest));
        }
        if let Some(rest) = header
            .package_name
            .as_deref()
            .and_then(|package| args::strip_name(query, package))
        {
            return Some((PACKAGE_PREFIX_SCORE, rest));
        }
    }
    let title = matcher::score(query, &header.title);
    let package = header
        .package_name
        .as_deref()
        .and_then(|package| matcher::score(query, package))
        .map(|score| score.saturating_sub(PACKAGE_PENALTY));
    title.max(package).map(|score| (score, ""))
}

fn item_for(
    script: &Arc<Script>,
    query: &str,
    roots: &[PathBuf],
    outputs: &Outputs,
) -> Option<Item> {
    let header = &script.header;
    let (query, confirmed) = match args::strip_confirmation(query) {
        Some(stripped) if header.needs_confirmation => (stripped, true),
        _ => (query, false),
    };
    let (score, arg_text) = match_query(header, query)?;
    let initial = args::initial_values(&header.arguments, arg_text);
    let subtitle = [
        header.package_name.as_deref(),
        header.description.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let display = display_path(&script.path, roots);
    let copy_output = {
        let (path, title) = (script.path.clone(), header.title.clone());
        let outputs = Arc::clone(outputs);
        Action::new(move || {
            let output = outputs
                .lock()
                .expect("script outputs poisoned")
                .get(&path)
                .cloned()
                .with_context(|| {
                    trf!(
                        "「{title}」还没有输出：先运行一次",
                        "“{title}” has no output yet. Run it first."
                    )
                })?;
            let pasteboard = super::screenshot::pasteboard::general();
            super::screenshot::pasteboard::write_text(&pasteboard, &output)?;
            Ok(Outcome::Done)
        })
    };
    Some(Item {
        id: format!("scripts:{}", script.path.display()),
        title: header.title.clone(),
        subtitle,
        badge: badge().into(),
        score,
        icon: ICON,
        form: (!header.arguments.is_empty()).then(|| form_for(&header.arguments, &initial)),
        detail: Some(script.detail(&display, None)),
        action: {
            let action = action(
                Arc::clone(script),
                initial,
                confirmed,
                display,
                Arc::clone(outputs),
            );
            // 要确认时回车只出确认页：不收起面板，直接在原地显示。
            if header.needs_confirmation && !confirmed {
                action.keep_open()
            } else {
                action
            }
        },
        action_title: tr("运行", "Run").into(),
        actions: vec![NamedAction::new(
            tr("复制输出", "Copy Output"),
            Some("⌘C"),
            copy_output,
        )],
        ..Item::blank()
    })
}

/// 表单字段的 key：`argument1`…`argument3`，与头注释里的字段名一致。
fn field_key(index: usize) -> String {
    format!("argument{}", index + 1)
}

/// 每个参数一个字段：text / password 用单行文本，dropdown 用下拉。
fn form_for(arguments: &[Argument], initial: &[String]) -> Form {
    let fields = arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| {
            let label = argument.label(index + 1);
            let placeholder = if argument.optional {
                trf!("{label}（可选）", "{label} (optional)")
            } else {
                label.clone()
            };
            Field {
                key: field_key(index),
                label,
                kind: match &argument.kind {
                    ArgKind::Dropdown(options) => FieldKind::Select {
                        options: options
                            .iter()
                            .map(|option| (option.title.clone(), option.value.clone()))
                            .collect(),
                    },
                    ArgKind::Password => FieldKind::Secret { placeholder },
                    ArgKind::Text => FieldKind::Text {
                        placeholder,
                        multiline: false,
                    },
                },
                value: initial.get(index).cloned().unwrap_or_default(),
                required: !argument.optional,
            }
        })
        .collect();
    Form { fields }
}

fn action(
    script: Arc<Script>,
    initial: Vec<String>,
    confirmed: bool,
    display: String,
    outputs: Outputs,
) -> Action {
    Action::with_input(move |input: &ActionInput| {
        let header = &script.header;
        let title = &header.title;
        // 表单值优先；没有表单（命令行 `helmp run`）时用查询里解析出的初值。
        let given: Vec<String> = initial
            .iter()
            .enumerate()
            .map(|(index, fallback)| {
                input
                    .form
                    .get(&field_key(index))
                    .unwrap_or(fallback)
                    .clone()
            })
            .collect();
        let args = match args::from_values(&header.arguments, &given) {
            Ok(args) => args,
            Err(err) => {
                return Ok(Outcome::Message(trf!(
                    "「{title}」{}，填好后回车",
                    "“{title}”: {}. Complete the form, then press Return.",
                    err.describe()
                )));
            }
        };
        if header.needs_confirmation && !confirmed {
            let (script, display, outputs) =
                (Arc::clone(&script), display.clone(), Arc::clone(&outputs));
            return Ok(Outcome::Show(Detail::confirm(
                ICON,
                trf!("确定要运行「{title}」吗？", "Run “{title}”?"),
                tr(
                    "这个脚本标记为需要确认（needsConfirmation），通常意味着操作不可撤销。",
                    "This script is marked as needing confirmation (needsConfirmation), which usually means it can’t be undone.",
                )
                .into(),
                tr("运行", "Run").into(),
                Action::new(move || run_now(&script, &args, &display, &outputs)),
            )));
        }
        run_now(&script, &args, &display, &outputs)
    })
}

/// 运行脚本并展示输出（silent 跑完不打扰）。
fn run_now(script: &Script, args: &[String], display: &str, outputs: &Outputs) -> Result<Outcome> {
    let (shown, code) = script.run(args, runner::TIMEOUT)?;
    let Some(output) = shown else {
        return Ok(Outcome::Done);
    };
    outputs
        .lock()
        .expect("script outputs poisoned")
        .insert(script.path.clone(), output.clone());
    Ok(Outcome::Show(script.detail(display, Some((&output, code)))))
}

#[cfg(test)]
pub mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// 测试用临时目录，析构时删除。
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "helmp-scripts-{tag}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    pub fn write_script(dir: &TempDir, name: &str, body: &str, executable: bool) -> PathBuf {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, body).unwrap();
        let mode = if executable { 0o755 } else { 0o644 };
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    fn provider(dir: &TempDir) -> Scripts {
        let scripts = Scripts {
            dirs: Some(vec![dir.path().to_path_buf()]),
            index: Arc::default(),
            outputs: Outputs::default(),
        };
        rescan(&scripts.index, &[absolute_dir(dir.path())], true);
        scripts
    }

    /// 输出详情里的等宽输出文本。
    fn shown_code(outcome: Outcome) -> String {
        let Outcome::Show(detail) = outcome else {
            panic!("应返回输出详情：{outcome:?}");
        };
        match detail.blocks.as_slice() {
            [Block::Code(text)] => text.clone(),
            other => panic!("输出应是一个等宽块：{other:?}"),
        }
    }

    fn only(items: Vec<Item>) -> Item {
        assert_eq!(items.len(), 1, "{items:?}");
        items.into_iter().next().unwrap()
    }

    #[test]
    fn scans_recursively_and_skips_unusable_files() {
        // 用户会把整个脚本仓库丢进来：子目录要扫到，隐藏目录 / 图片要忽略，坏脚本要跳过而不是拖垮全部。
        let dir = TempDir::new("scan");
        write_script(
            &dir,
            "a/b/ok.sh",
            "#!/bin/sh\n# @raycast.title Deep\n# @raycast.mode silent\n",
            true,
        );
        write_script(
            &dir,
            "broken.sh",
            "#!/bin/sh\n# @raycast.title Broken\n",
            true,
        );
        write_script(
            &dir,
            "README.md",
            "```\n# @raycast.title Doc\n# @raycast.mode silent\n```\n",
            false,
        );
        write_script(
            &dir,
            ".git/hook.sh",
            "#!/bin/sh\n# @raycast.title Hidden\n# @raycast.mode silent\n",
            true,
        );
        write_script(&dir, "icon.png", "\u{89}PNG binary", false);
        let scripts = provider(&dir);

        let index = scripts.index.read().unwrap();
        let titles: Vec<&str> = index
            .scripts
            .iter()
            .map(|s| s.header.title.as_str())
            .collect();
        assert_eq!(titles, vec!["Deep"]);
        let mut skipped: Vec<String> = index
            .files
            .iter()
            .filter_map(|(path, (_, entry))| match entry {
                Entry::Skipped(reason) => Some(format!(
                    "{}: {reason}",
                    path.file_name().unwrap().to_string_lossy()
                )),
                _ => None,
            })
            .collect();
        skipped.sort();
        assert_eq!(
            skipped,
            vec![
                "README.md: 既没有 shebang 也不可执行".to_string(),
                "broken.sh: 缺少 @raycast.mode".to_string(),
            ]
        );
    }

    #[test]
    fn rescan_picks_up_added_changed_and_removed_scripts() {
        // 用户在启动器运行时编辑脚本目录，不重启就要看到变化。
        let dir = TempDir::new("rescan");
        let dirs = [absolute_dir(dir.path())];
        let scripts = provider(&dir);
        assert!(scripts.query("new").is_empty());

        let path = write_script(
            &dir,
            "new.sh",
            "#!/bin/sh\n# @raycast.title New One\n# @raycast.mode silent\n",
            true,
        );
        rescan(&scripts.index, &dirs, false);
        assert_eq!(only(scripts.query("new")).title, "New One");

        // 改内容同时改了长度，指纹必然变化。
        fs::write(
            &path,
            "#!/bin/sh\n# @raycast.title Renamed Script\n# @raycast.mode silent\n",
        )
        .unwrap();
        rescan(&scripts.index, &dirs, false);
        assert_eq!(only(scripts.query("renamed")).title, "Renamed Script");

        fs::remove_file(&path).unwrap();
        rescan(&scripts.index, &dirs, false);
        assert!(scripts.query("renamed").is_empty());
    }

    #[test]
    fn item_shape_follows_launcher_conventions() {
        let dir = TempDir::new("item");
        let path = write_script(
            &dir,
            "t.sh",
            "#!/bin/sh\n# @raycast.title Show Time\n# @raycast.mode compact\n# @raycast.packageName Clock\n# @raycast.description 显示时间\n",
            true,
        );
        let item = only(provider(&dir).query("show"));
        let canonical = fs::canonicalize(&path).unwrap();
        assert_eq!(item.id, format!("scripts:{}", canonical.display()));
        assert_eq!(item.subtitle, "Clock · 显示时间");
        assert_eq!(item.badge, "脚本");
        // 空查询不展示脚本，避免面板一打开就被几百个脚本淹没。
        assert!(provider(&dir).query("  ").is_empty());
    }

    #[test]
    fn text_after_title_is_passed_as_arguments() {
        // 「标题 + 参数」一行输完回车（以及命令行 `helmp run`）不经过表单编辑，参数必须原样送进脚本。
        let dir = TempDir::new("args");
        write_script(
            &dir,
            "route.sh",
            "#!/bin/sh\n# @raycast.title Route\n# @raycast.mode compact\n# @raycast.packageName Maps\n\
             # @raycast.argument1 {\"type\":\"text\",\"placeholder\":\"from\"}\n\
             # @raycast.argument2 {\"type\":\"text\",\"placeholder\":\"to\"}\n\
             echo \"$1->$2\"\n",
            true,
        );
        let scripts = provider(&dir);
        let item = only(scripts.query("route 北京 上海 浦东"));
        assert_eq!(item.score, TITLE_PREFIX_SCORE);
        assert_eq!(shown_code(item.action.run().unwrap()), "北京->上海 浦东");

        let by_package = only(scripts.query("maps 杭州 苏州"));
        assert_eq!(by_package.score, PACKAGE_PREFIX_SCORE);
        assert_eq!(shown_code(by_package.action.run().unwrap()), "杭州->苏州");
    }

    #[test]
    fn missing_required_argument_blocks_execution() {
        // 缺参数还执行，脚本会拿空串去做事（比如删除空路径对应的东西）。必须提示且不运行。
        let dir = TempDir::new("missing");
        let marker = dir.path().join("ran");
        write_script(
            &dir,
            "touch.sh",
            &format!(
                "#!/bin/sh\n# @raycast.title Touch It\n# @raycast.mode silent\n\
                 # @raycast.argument1 {{\"type\":\"text\",\"placeholder\":\"文件名\"}}\n\
                 # @raycast.argument2 {{\"type\":\"text\",\"placeholder\":\"内容\"}}\n\
                 touch '{}'\n",
                marker.display()
            ),
            true,
        );
        let item = only(provider(&dir).query("touch it a"));
        let Outcome::Message(message) = item.action.run().unwrap() else {
            panic!("缺参数应返回说明");
        };
        assert!(message.contains("需要参数：内容"), "{message}");
        assert!(!marker.exists(), "缺参数时脚本不应运行");

        // 表单里把必填项清空再回车，同样要拦下。
        let mut input = ActionInput::default();
        input.form.insert("argument1".into(), String::new());
        input.form.insert("argument2".into(), "x".into());
        let Outcome::Message(message) = item.action.run_with(&input).unwrap() else {
            panic!("缺参数应返回说明");
        };
        assert!(message.contains("需要参数：文件名"), "{message}");
        assert!(!marker.exists(), "缺参数时脚本不应运行");
    }

    #[test]
    fn needs_confirmation_shows_a_confirmation_page_first() {
        // needsConfirmation 用于清空废纸篓、关机这类不可逆操作：第一次回车只出确认页，脚本不能运行；
        // 确认页上回车才运行，「取消」只回到列表。
        let dir = TempDir::new("confirm");
        let marker = dir.path().join("ran");
        write_script(
            &dir,
            "danger.sh",
            &format!(
                "#!/bin/sh\n# @raycast.title Empty Trash\n# @raycast.mode silent\n# @raycast.needsConfirmation true\ntouch '{}'\n",
                marker.display()
            ),
            true,
        );
        let scripts = provider(&dir);
        let blocked = only(scripts.query("empty trash"));
        assert!(blocked.action.keeps_open(), "确认页应直接出现在面板里");
        let Outcome::Show(page) = blocked.action.run().unwrap() else {
            panic!("未确认时应显示确认页");
        };
        assert_eq!(page.title, "确定要运行「Empty Trash」吗？");
        assert!(!marker.exists(), "未确认时不应运行");
        assert_eq!(page.actions[1].action.run().unwrap(), Outcome::Dismiss);
        assert!(!marker.exists(), "取消后不应运行");
        assert_eq!(page.actions[0].action.run().unwrap(), Outcome::Done);
        assert!(marker.exists(), "确认后应运行");
    }

    #[test]
    fn trailing_bang_still_confirms_directly() {
        // 老用户习惯在末尾加 ` !` 直接运行，加了确认页后不能失效。
        let dir = TempDir::new("confirm-bang");
        let marker = dir.path().join("ran");
        write_script(
            &dir,
            "danger.sh",
            &format!(
                "#!/bin/sh\n# @raycast.title Empty Trash\n# @raycast.mode silent\n# @raycast.needsConfirmation true\ntouch '{}'\n",
                marker.display()
            ),
            true,
        );
        let confirmed = only(provider(&dir).query("empty trash !"));
        assert!(!confirmed.action.keeps_open());
        assert_eq!(confirmed.action.run().unwrap(), Outcome::Done);
        assert!(marker.exists(), "确认后应运行");
    }

    #[test]
    fn bundled_examples_parse_and_current_time_runs() {
        // examples/scripts 既是用户示例也是 fixture：头注释写错会让示例在用户机器上静默消失。
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/scripts");
        let scripts = Scripts {
            dirs: None,
            index: Arc::default(),
            outputs: Outputs::default(),
        };
        rescan(&scripts.index, &[absolute_dir(&examples)], true);
        let index = scripts.index.read().unwrap();
        let mut found: Vec<(&str, header::Mode)> = index
            .scripts
            .iter()
            .map(|s| (s.header.title.as_str(), s.header.mode))
            .collect();
        found.sort_by_key(|(title, _)| *title);
        assert_eq!(
            found,
            vec![
                ("当前时间", header::Mode::Compact),
                ("本机 IP", header::Mode::FullOutput),
                ("生成 UUID", header::Mode::Silent),
            ]
        );
        drop(index);

        let item = only(scripts.query("时间"));
        let now = shown_code(item.action.run().unwrap());
        // 形如 2026-11-05 14:03:59 CST
        assert!(now.starts_with("20") && now.contains(':'), "{now}");
    }

    #[test]
    fn arguments_become_form_fields_with_initial_values() {
        // 有参数的脚本在面板里是「表单 + 详情」：每个参数一个字段，dropdown 是下拉，
        // 查询里「标题 + 文本」作为初值，这样老习惯（一行输完回车）依然成立。
        let dir = TempDir::new("form");
        write_script(
            &dir,
            "flight.sh",
            "#!/bin/sh\n# @raycast.title Flight\n# @raycast.mode compact\n\
             # @raycast.argument1 {\"type\":\"text\",\"placeholder\":\"出发地\"}\n\
             # @raycast.argument2 {\"type\":\"password\",\"placeholder\":\"口令\",\"optional\":true}\n\
             # @raycast.argument3 {\"type\":\"dropdown\",\"placeholder\":\"舱位\",\"data\":[{\"title\":\"经济舱\",\"value\":\"y\"},{\"title\":\"商务舱\",\"value\":\"c\"}]}\n\
             echo \"$1|$2|$3\"\n",
            true,
        );
        let item = only(provider(&dir).query("flight 北京 pw 商务舱"));
        let form = item.form.as_ref().expect("有参数应返回表单");
        let shape: Vec<(&str, &str, bool, &str)> = form
            .fields
            .iter()
            .map(|f| {
                let kind = match &f.kind {
                    FieldKind::Text { .. } => "text",
                    // password 参数用掩码输入：口令不该明文显示在面板上被旁人看到。
                    FieldKind::Secret { .. } => "secret",
                    FieldKind::Select { .. } => "select",
                };
                (f.key.as_str(), f.label.as_str(), f.required, kind)
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("argument1", "出发地", true, "text"),
                ("argument2", "口令", false, "secret"),
                ("argument3", "舱位", true, "select"),
            ]
        );
        let values: Vec<&str> = form.fields.iter().map(|f| f.value.as_str()).collect();
        assert_eq!(values, ["北京", "pw", "c"]);
        assert_eq!(
            form.fields[2].kind,
            FieldKind::Select {
                options: vec![("经济舱".into(), "y".into()), ("商务舱".into(), "c".into())]
            }
        );

        // 回车时以表单值为准（用户可能改过），不是查询里的初值。
        let mut input = ActionInput::default();
        input.form.insert("argument1".into(), "上海".into());
        input.form.insert("argument2".into(), String::new());
        input.form.insert("argument3".into(), "y".into());
        assert_eq!(shown_code(item.action.run_with(&input).unwrap()), "上海||y");
    }

    #[test]
    fn output_detail_carries_code_block_and_metadata() {
        // 设计稿「脚本」：输出用等宽块，底部 说明 / 脚本（相对路径）/ 来源 · 输出模式 / 退出码；
        // ⌘K「复制输出」拿的是这次的输出。
        let dir = TempDir::new("detail");
        write_script(
            &dir,
            "developer-utils/count.sh",
            "#!/bin/sh\n# @raycast.title Count Characters\n# @raycast.mode fullOutput\n\
             # @raycast.description 统计字符数\n\
             # @raycast.argument1 {\"type\":\"text\",\"placeholder\":\"Text\",\"optional\":true}\n\
             printf '%s chars\\n' \"${#1}\"\n",
            true,
        );
        let item = only(provider(&dir).query("count characters hello huan"));
        assert_eq!(
            item.icon,
            Icon::Symbol {
                name: "square-terminal",
                tint: Tint::Graphite
            }
        );
        // 未运行时详情只有元数据。
        let before = item.detail.as_ref().unwrap();
        assert!(before.blocks.is_empty());
        assert_eq!(
            before.metadata[1],
            ("脚本".into(), "developer-utils/count.sh".into())
        );

        let Outcome::Show(detail) = item.action.run().unwrap() else {
            panic!("fullOutput 应返回详情");
        };
        assert_eq!(detail.title, "Count Characters");
        assert_eq!(detail.blocks, [Block::Code("10 chars".into())]);
        assert_eq!(
            detail.metadata,
            [
                ("说明".to_string(), "统计字符数".to_string()),
                ("脚本".to_string(), "developer-utils/count.sh".to_string()),
                (
                    "来源".to_string(),
                    "Raycast Script Commands · 输出模式 fullOutput".to_string()
                ),
                ("退出码".to_string(), "0".to_string()),
            ]
        );
        let copy = item.actions.iter().find(|a| a.title == "复制输出").unwrap();
        assert_eq!(copy.shortcut.as_deref(), Some("⌘C"));
    }

    #[test]
    fn copy_output_before_running_fails_loudly() {
        // 没运行过就「复制输出」必须报错，不能静默往剪贴板写空串。
        let dir = TempDir::new("nocopy");
        write_script(
            &dir,
            "a.sh",
            "#!/bin/sh\n# @raycast.title Alpha\n# @raycast.mode compact\necho hi\n",
            true,
        );
        let item = only(provider(&dir).query("alpha"));
        // 没有参数的脚本不出表单，回车直接运行。
        assert!(item.form.is_none());
        let copy = item.actions.iter().find(|a| a.title == "复制输出").unwrap();
        assert!(copy.action.run().is_err());
    }

    #[test]
    fn script_dirs_merge_config_and_env_without_duplicates() {
        // 设置里加的目录和环境变量里的目录都要扫；重复的目录扫两遍会让同一脚本出现两次。
        let home = Path::new("/Users/me");
        assert_eq!(expand_tilde("~/scripts", home), home.join("scripts"));
        assert_eq!(expand_tilde("~", home), home.to_path_buf());
        assert_eq!(expand_tilde("/opt/s", home), PathBuf::from("/opt/s"));
        let merged = merge_dirs(
            PathBuf::from("/d"),
            vec![PathBuf::from("/a"), PathBuf::from("/d")],
            vec![PathBuf::from("/a"), PathBuf::from("/b")],
        );
        assert_eq!(merged, ["/d", "/a", "/b"].map(PathBuf::from).to_vec());
        assert_eq!(
            display_path(
                Path::new("/a/x/y.sh"),
                &[PathBuf::from("/a"), PathBuf::from("/a/x")]
            ),
            "y.sh"
        );
        assert_eq!(display_path(Path::new("/z/y.sh"), &[]), "/z/y.sh");
    }

    #[test]
    fn symbol_icon_exists_in_bundled_assets() {
        // 图标名拼错时界面只会画一个空白方块，不会报错。
        use gpui_kit::AssetSource as _;
        let Icon::Symbol { name, .. } = ICON else {
            panic!("脚本图标应是内置符号");
        };
        let path = format!("icons/{name}.svg");
        assert!(gpui_kit::assets::AllAssets.load(&path).unwrap().is_some());
    }

    #[test]
    fn english_ui_translates_helmp_text_but_not_script_header() {
        // 英文界面下 Helmp 自己的文案（分组、动作、可选标记、缺参提示）要是英文，
        // 但标题、placeholder、下拉选项是用户脚本自带的内容，原样显示；
        // 搜索仍按脚本标题匹配，与界面语言无关。
        use crate::i18n::{Lang, with_lang};
        let dir = TempDir::new("english");
        write_script(
            &dir,
            "note.sh",
            "#!/bin/sh\n# @raycast.title 记一笔\n# @raycast.mode compact\n\
             # @raycast.argument1 {\"type\":\"text\",\"placeholder\":\"内容\"}\n\
             # @raycast.argument2 {\"type\":\"text\",\"optional\":true}\n\
             echo \"$1\"\n",
            true,
        );
        let scripts = provider(&dir);
        with_lang(Lang::En, || {
            let item = only(scripts.query("jyb"));
            assert_eq!(item.title, "记一笔");
            assert_eq!(item.badge, "Scripts");
            assert_eq!(item.action_title, "Run");
            assert_eq!(item.actions[0].title, "Copy Output");
            let form = item.form.as_ref().expect("有参数应返回表单");
            assert_eq!(form.fields[0].label, "内容");
            // 没写 placeholder 的参数用 Helmp 生成的默认名，要跟着语言走。
            assert_eq!(form.fields[1].label, "Argument 2");
            assert_eq!(
                form.fields[1].kind,
                FieldKind::Text {
                    placeholder: "Argument 2 (optional)".into(),
                    multiline: false,
                }
            );
            let Outcome::Message(message) = item.action.run().unwrap() else {
                panic!("缺参数应返回说明");
            };
            assert_eq!(
                message,
                "“记一笔”: Missing arguments: 内容. Complete the form, then press Return."
            );
            let Some(detail) = &item.detail else {
                panic!("应有详情");
            };
            let keys: Vec<&str> = detail.metadata.iter().map(|(k, _)| k.as_str()).collect();
            assert_eq!(keys, ["Script", "Source"]);
            assert_eq!(
                detail.metadata[1].1,
                "Raycast Script Commands · compact mode"
            );
        });
        // 解析结果常驻索引：同一份索引切回中文时默认参数名也要切回来，不能停在解析时的语言。
        let item = only(scripts.query("记一笔"));
        assert_eq!(item.form.as_ref().unwrap().fields[1].label, "参数2");
        assert_eq!(item.badge, "脚本");
    }
}
