//! L1 脚本插件：兼容 Raycast Script Commands。
//!
//! - 目录：`<data_dir>/scripts/` + 环境变量 `HUAN_SCRIPT_DIRS`（冒号分隔），递归扫描，跳过隐藏文件。
//! - 每 5 秒按 (修改时间, 大小, 可执行位) 检查变化，只重读变了的文件。
//! - 参数：查询以脚本标题或 packageName 开头时，余下文本是参数（见 `args`）。
//! - `needsConfirmation: true` 的脚本只有查询以 ` !` 结尾才执行。

mod args;
mod header;
mod runner;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result};

use crate::matcher;
use crate::provider::{Action, Item, Outcome, Provider};
use args::ArgError;
use header::{Header, Parsed};

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
const CONFIRM_HINT: &str = "该脚本需要确认，在查询末尾加 ! 后回车执行";

#[derive(Debug)]
pub struct Script {
    pub path: PathBuf,
    pub header: Header,
}

impl Script {
    fn run(&self, args: &[String], timeout: Duration) -> Result<Outcome> {
        let cwd = runner::working_dir(self.header.current_directory_path.as_deref(), &self.path)?;
        let output = runner::execute(&self.path, args, &cwd, timeout)?;
        runner::outcome(&self.header.title, self.header.mode, &output)
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
}

pub struct Scripts {
    /// `None` 表示在 `start` 时按默认规则解析（数据目录 + 环境变量）。
    dirs: Option<Vec<PathBuf>>,
    index: Arc<RwLock<Index>>,
}

impl Scripts {
    pub fn new() -> Self {
        Self {
            dirs: None,
            index: Arc::default(),
        }
    }
}

impl Provider for Scripts {
    fn id(&self) -> &'static str {
        "scripts"
    }

    fn start(&self) -> Result<()> {
        let dirs = match &self.dirs {
            Some(dirs) => dirs.clone(),
            None => script_dirs()?,
        };
        let dirs: Vec<PathBuf> = dirs.iter().map(|dir| absolute_dir(dir)).collect();
        rescan(&self.index, &dirs, true);
        let index = Arc::clone(&self.index);
        std::thread::Builder::new()
            .name("huan-scripts-watch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(POLL_INTERVAL);
                    rescan(&index, &dirs, false);
                }
            })
            .context("启动脚本目录轮询线程")?;
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
            .filter_map(|script| item_for(script, query))
            .collect()
    }
}

/// 默认脚本目录（不存在就创建，方便用户找到放哪）+ `HUAN_SCRIPT_DIRS`。
fn script_dirs() -> Result<Vec<PathBuf>> {
    let default = crate::paths::data_dir()?.join("scripts");
    fs::create_dir_all(&default).with_context(|| format!("创建 {}", default.display()))?;
    let mut dirs = vec![default];
    if let Some(extra) = std::env::var_os("HUAN_SCRIPT_DIRS") {
        dirs.extend(std::env::split_paths(&extra).filter(|dir| !dir.as_os_str().is_empty()));
    }
    Ok(dirs)
}

/// 规范成绝对路径（Item id 要用绝对路径）。目录暂不存在也保留，出现后轮询会自动加载。
fn absolute_dir(dir: &Path) -> PathBuf {
    match fs::canonicalize(dir) {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!(
                "huan: scripts 目录 {} 暂不可用：{err}（出现后会自动加载）",
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
        eprintln!("huan: scripts {problem}");
    }
    for (path, stamp) in to_parse {
        let entry = parse_file(&path, &stamp);
        if let Entry::Skipped(reason) = &entry {
            eprintln!("huan: 跳过脚本 {}：{reason}", path.display());
        }
        files.insert(path, (stamp, entry));
    }

    let mut index = index
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    index.reported.extend(new_problems);
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
        "huan: scripts 已加载 {} 个脚本，跳过 {skipped} 个",
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

fn item_for(script: &Arc<Script>, query: &str) -> Option<Item> {
    let header = &script.header;
    let (query, confirmed) = match args::strip_confirmation(query) {
        Some(stripped) if header.needs_confirmation => (stripped, true),
        _ => (query, false),
    };
    let (score, arg_text) = match_query(header, query)?;
    let resolved = args::resolve(&header.arguments, arg_text);
    let subtitle = match &resolved {
        Err(err) => err.describe(),
        Ok(_) => [
            header.package_name.as_deref(),
            header.description.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · "),
    };
    Some(Item {
        id: format!("scripts:{}", script.path.display()),
        title: header.title.clone(),
        subtitle,
        badge: "脚本".into(),
        score,
        action: action(Arc::clone(script), resolved, confirmed),
    })
}

fn action(script: Arc<Script>, resolved: Result<Vec<String>, ArgError>, confirmed: bool) -> Action {
    Action::new(move || {
        let title = &script.header.title;
        let args = match &resolved {
            Ok(args) => args,
            Err(err) => {
                return Ok(Outcome::Message(format!(
                    "「{title}」{}。在查询里输入「{title} <参数>」后回车",
                    err.describe()
                )));
            }
        };
        if script.header.needs_confirmation && !confirmed {
            return Ok(Outcome::Message(CONFIRM_HINT.into()));
        }
        script.run(args, runner::TIMEOUT)
    })
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
                "huan-scripts-{tag}-{}-{}",
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
        };
        rescan(&scripts.index, &[absolute_dir(dir.path())], true);
        scripts
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
        // 启动器没有参数表单，「标题 + 参数」是唯一的传参方式，必须把参数原样送进脚本。
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
        assert_eq!(
            item.action.run().unwrap(),
            Outcome::Message("北京->上海 浦东".into())
        );

        let by_package = only(scripts.query("maps 杭州 苏州"));
        assert_eq!(by_package.score, PACKAGE_PREFIX_SCORE);
        assert_eq!(
            by_package.action.run().unwrap(),
            Outcome::Message("杭州->苏州".into())
        );
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
        assert_eq!(item.subtitle, "需要参数：内容");
        let Outcome::Message(message) = item.action.run().unwrap() else {
            panic!("缺参数应返回说明");
        };
        assert!(message.contains("需要参数：内容"), "{message}");
        assert!(!marker.exists(), "缺参数时脚本不应运行");
    }

    #[test]
    fn needs_confirmation_requires_trailing_bang() {
        // needsConfirmation 用于清空废纸篓、关机这类不可逆操作，误触回车不能直接执行。
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
        assert_eq!(
            blocked.action.run().unwrap(),
            Outcome::Message(CONFIRM_HINT.into())
        );
        assert!(!marker.exists(), "未确认时不应运行");

        let confirmed = only(scripts.query("empty trash !"));
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
        let Outcome::Message(now) = item.action.run().unwrap() else {
            panic!("compact 应返回最后一行");
        };
        // 形如 2026-11-05 14:03:59 CST
        assert!(now.starts_with("20") && now.contains(':'), "{now}");
    }
}
