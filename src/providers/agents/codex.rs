//! Codex 会话。Codex 没有注册表，审批 / 提问事件也不落盘，所以：
//!
//! - 判活：`$CODEX_HOME/thread-writer-locks/<threadId>.lock` 用 `flock(LOCK_SH|LOCK_NB)` 试锁；
//!   拿到锁说明没有活写者（立刻释放），拿不到才算活。文件存在不代表在跑。
//! - 元数据：`state_5.sqlite` 以 `?mode=ro&immutable=1` 只读打开，取 `threads` 表的标题 / cwd / 分支 / 来源。
//! - 状态：只读 rollout 尾部 ≤ 64 KB，找最后一个 turn 边界（`task_started` / `task_complete` / `turn_aborted`）
//!   和 async 提问（`item_completed{AgentMessage, delivery:"async"}`）。

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead as _, BufReader};
use std::os::fd::AsRawFd as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use serde_json::Value;

use super::state::{self, Jump, Session, Source, Status};
use super::{TAIL_BYTES, Warnings, read_tail, tail_lines};

/// 试锁判活。只读打开，不写不删。
pub fn writer_alive(lock: &Path) -> std::io::Result<bool> {
    let file = File::open(lock)?;
    let fd = file.as_raw_fd();
    // SAFETY: fd 在 file 存活期间有效。
    if unsafe { libc::flock(fd, libc::LOCK_SH | libc::LOCK_NB) } == 0 {
        // SAFETY: 同上；释放刚拿到的共享锁，避免哪怕一瞬间挡住 Codex 拿独占锁。
        unsafe { libc::flock(fd, libc::LOCK_UN) };
        return Ok(false);
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(true)
    } else {
        Err(err)
    }
}

/// rollout 尾部解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailState {
    pub status: Status,
    /// 尾部最后一条用户消息，标题兜底用。
    pub last_user: Option<String>,
}

enum Edge {
    Started,
    Complete,
    Aborted(String),
}

/// 判 turn 状态。规则（调研 10 §5.1）：
/// - 最后一个边界是 `turn_aborted` → Interrupted（用户主动停的，挂着的提问已无意义）
/// - 最近一次 async 提问之后没有 `UserMessage` → 等你回答（async 提问不阻塞 turn，turn 可能照常 complete）
/// - `task_started` 未闭合 → Running；`task_complete` → Done；尾部一个边界都没有 → Unknown
pub fn parse_tail<'a>(lines: impl Iterator<Item = &'a [u8]>) -> TailState {
    let mut edge = None;
    let mut question = false;
    let mut last_user = None;
    for line in lines {
        // 大部分行是 response_item（工具输出、推理），先粗筛再解析。
        if !contains(line, b"\"event_msg\"") {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let payload = &value["payload"];
        match payload["type"].as_str() {
            Some("task_started") => {
                edge = Some(Edge::Started);
                question = false;
            }
            Some("task_complete") => edge = Some(Edge::Complete),
            Some("turn_aborted") => {
                edge = Some(Edge::Aborted(
                    payload["reason"].as_str().unwrap_or_default().to_string(),
                ))
            }
            Some("item_completed") => {
                let item = &payload["item"];
                match item["type"].as_str() {
                    Some("UserMessage") => {
                        question = false;
                        last_user = item_text(item).map(|t| state::one_line(&t, 60));
                    }
                    Some("AgentMessage") if item["delivery"].as_str() == Some("async") => {
                        question = true;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    let status = match edge {
        Some(Edge::Aborted(reason)) => Status::Interrupted(reason),
        _ if question => Status::NeedsYou("等你回答".into()),
        Some(Edge::Started) => Status::Running,
        Some(Edge::Complete) => Status::Done,
        None => Status::Unknown,
    };
    TailState { status, last_user }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn item_text(item: &Value) -> Option<String> {
    let text: Vec<&str> = item["content"]
        .as_array()?
        .iter()
        .filter_map(|part| part["text"].as_str())
        .collect();
    (!text.is_empty()).then(|| text.join(" "))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadMeta {
    pub rollout_path: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub title: Option<String>,
    pub branch: Option<String>,
    pub originator: Option<String>,
    /// 子 agent 线程（Wake #146 的坑：别当成会话列出来）。
    pub subagent: bool,
    pub archived: bool,
}

/// 标题：用户起的 `name` > `title`（附件前言只取 `## My request:` 之后）> 首条用户消息。
pub fn clean_title(name: &str, title: &str, first_user_message: &str) -> Option<String> {
    let pick = |text: &str| {
        let text = match text.split_once("## My request:") {
            Some((_, request)) => request,
            None => text,
        };
        Some(state::one_line(text, 80)).filter(|t| !t.is_empty())
    };
    pick(name)
        .or_else(|| pick(title))
        .or_else(|| pick(first_user_message))
}

fn sqlite_uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for ch in path.to_string_lossy().chars() {
        match ch {
            '%' => uri.push_str("%25"),
            '?' => uri.push_str("%3f"),
            '#' => uri.push_str("%23"),
            ' ' => uri.push_str("%20"),
            other => uri.push(other),
        }
    }
    uri.push_str("?mode=ro&immutable=1");
    uri
}

/// 从 `state_5.sqlite` 只读批量取元数据。查不到的 id 不在返回值里。
pub fn load_meta(db: &Path, ids: &[String]) -> Result<HashMap<String, ThreadMeta>> {
    use rusqlite::{Connection, OpenFlags};
    let conn = Connection::open_with_flags(
        sqlite_uri(db),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("只读打开 {}", db.display()))?;
    let mut stmt = conn.prepare(
        "SELECT rollout_path, cwd, coalesce(name, ''), title, first_user_message, git_branch, \
         originator, source, thread_source, archived FROM threads WHERE id = ?1",
    )?;
    let mut out = HashMap::new();
    for id in ids {
        let mut rows = stmt.query([id])?;
        let Some(row) = rows.next()? else { continue };
        let name: String = row.get(2)?;
        let title: String = row.get(3)?;
        let first: String = row.get(4)?;
        let source: String = row.get(7)?;
        let thread_source: Option<String> = row.get(8)?;
        out.insert(
            id.clone(),
            ThreadMeta {
                rollout_path: Some(PathBuf::from(row.get::<_, String>(0)?)),
                cwd: Some(PathBuf::from(row.get::<_, String>(1)?)),
                title: clean_title(&name, &title, &first),
                branch: row.get::<_, Option<String>>(5)?.filter(|b| !b.is_empty()),
                originator: row.get(6)?,
                subagent: thread_source.as_deref() == Some("subagent")
                    || source.contains("\"subagent\""),
                archived: row.get::<_, i64>(9)? != 0,
            },
        );
    }
    Ok(out)
}

/// sqlite 里还没有的线程（刚建）：在 `sessions/YYYY/MM/DD/` 里按文件名找 rollout，读首行 `session_meta`。
pub fn meta_from_rollout(sessions_dir: &Path, id: &str) -> Result<ThreadMeta> {
    let suffix = format!("-{id}.jsonl");
    let path = walk_days(sessions_dir)
        .into_iter()
        .flat_map(|day| std::fs::read_dir(day).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(&suffix))
        })
        .with_context(|| format!("sqlite 和 {} 里都找不到线程 {id}", sessions_dir.display()))?;
    let mut first = String::new();
    BufReader::new(File::open(&path)?).read_line(&mut first)?;
    let mut meta = parse_session_meta(&first)
        .with_context(|| format!("{} 首行不是 session_meta", path.display()))?;
    meta.rollout_path = Some(path);
    Ok(meta)
}

fn walk_days(sessions_dir: &Path) -> Vec<PathBuf> {
    let children = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect()
    };
    children(sessions_dir)
        .iter()
        .flat_map(|year| children(year))
        .flat_map(|month| children(&month))
        .collect()
}

pub fn parse_session_meta(line: &str) -> Option<ThreadMeta> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value["type"].as_str() != Some("session_meta") {
        return None;
    }
    let payload = &value["payload"];
    Some(ThreadMeta {
        rollout_path: None,
        cwd: payload["cwd"].as_str().map(PathBuf::from),
        title: None,
        branch: payload["git"]["branch"].as_str().map(String::from),
        originator: payload["originator"].as_str().map(String::from),
        subagent: payload["thread_source"].as_str() == Some("subagent")
            || payload["source"].is_object(),
        archived: false,
    })
}

struct Cached {
    meta: ThreadMeta,
    /// rollout 的 (长度, mtime)，变了才重读尾部。
    sig: Option<(u64, SystemTime)>,
    tail: TailState,
}

pub struct CodexScanner {
    home: PathBuf,
    cache: HashMap<String, Cached>,
}

impl CodexScanner {
    pub fn new(home: &Path) -> Self {
        let home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        Self {
            home,
            cache: HashMap::new(),
        }
    }

    fn locks_dir(&self) -> PathBuf {
        self.home.join("thread-writer-locks")
    }

    pub fn scan(&mut self, warnings: &mut Warnings) -> Vec<Session> {
        let alive = self.alive_threads(warnings);
        self.cache.retain(|id, _| alive.contains(id));

        // 新出现的线程，或 rollout 有变化的线程（标题可能刚被改），才去查 sqlite / 重读尾部。
        let mut changed = Vec::new();
        for id in &alive {
            let sig = self
                .cache
                .get(id)
                .and_then(|c| c.meta.rollout_path.as_deref())
                .and_then(signature);
            match self.cache.get(id) {
                Some(cached) if cached.sig == sig => {}
                _ => changed.push(id.clone()),
            }
        }
        if changed.is_empty() {
            return self.sessions();
        }

        let db = self.home.join("state_5.sqlite");
        let mut metas = match load_meta(&db, &changed) {
            Ok(metas) => metas,
            Err(err) => {
                warnings.push(format!("读取 Codex 线程元数据失败：{err:#}"));
                HashMap::new()
            }
        };
        for id in changed {
            let meta = match metas.remove(&id) {
                Some(meta) => meta,
                None => match self.cache.get(&id) {
                    Some(cached) => cached.meta.clone(),
                    None => match meta_from_rollout(&self.home.join("sessions"), &id) {
                        Ok(meta) => meta,
                        Err(err) => {
                            warnings.push(format!("{err:#}"));
                            continue;
                        }
                    },
                },
            };
            let Some(rollout) = meta.rollout_path.clone() else {
                continue;
            };
            let sig = signature(&rollout);
            let tail = match read_tail(&rollout, TAIL_BYTES) {
                Ok((bytes, truncated)) => parse_tail(tail_lines(&bytes, truncated)),
                Err(err) => {
                    warnings.push(format!("读取 {} 失败：{err}", rollout.display()));
                    continue;
                }
            };
            self.cache.insert(id, Cached { meta, sig, tail });
        }
        self.sessions()
    }

    fn alive_threads(&self, warnings: &mut Warnings) -> HashSet<String> {
        let dir = self.locks_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return HashSet::new(),
            Err(err) => {
                warnings.push(format!("读取 {} 失败：{err}", dir.display()));
                return HashSet::new();
            }
        };
        let mut alive = HashSet::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            // `.coordination.lock` 之类不是线程锁。
            let Some(id) = name
                .strip_suffix(".lock")
                .filter(|_| !name.starts_with('.'))
            else {
                continue;
            };
            match writer_alive(&path) {
                Ok(true) => {
                    alive.insert(id.to_string());
                }
                Ok(false) => {}
                Err(err) => warnings.push(format!("试锁 {} 失败：{err}", path.display())),
            }
        }
        alive
    }

    fn sessions(&self) -> Vec<Session> {
        self.cache
            .iter()
            .filter(|(_, c)| !c.meta.subagent && !c.meta.archived)
            .map(|(id, c)| {
                let jump = if c.meta.originator.as_deref() == Some("Codex Desktop") {
                    Jump::DeepLink(format!("codex://threads/{id}"))
                } else {
                    Jump::CodexLock(self.locks_dir().join(format!("{id}.lock")))
                };
                let cwd = c.meta.cwd.clone();
                Session {
                    source: Source::Codex,
                    session_id: id.clone(),
                    title: c
                        .meta
                        .title
                        .clone()
                        .or_else(|| c.tail.last_user.clone())
                        .unwrap_or_else(|| "（无标题会话）".into()),
                    status: c.tail.status.clone(),
                    branch: cwd
                        .as_deref()
                        .and_then(state::git_branch)
                        .or_else(|| c.meta.branch.clone()),
                    cwd,
                    last_activity_ms: c.sig.map(|(_, t)| to_ms(t)).unwrap_or(0),
                    jump,
                }
            })
            .collect()
    }
}

fn signature(path: &Path) -> Option<(u64, SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

fn to_ms(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNNING: &str = include_str!("fixtures/codex_tail_running.jsonl");
    const ASYNC_QUESTION: &str = include_str!("fixtures/codex_tail_async_question.jsonl");
    const DONE: &str = include_str!("fixtures/codex_tail_done.jsonl");
    const INTERRUPTED: &str = include_str!("fixtures/codex_tail_interrupted.jsonl");
    const ANSWERED: &str = include_str!("fixtures/codex_tail_answered.jsonl");
    const SESSION_META: &str = include_str!("fixtures/codex_session_meta.jsonl");

    /// 模拟从文件中间截取：首行是残行，必须丢掉而不是误解析。
    fn tail(text: &str) -> TailState {
        parse_tail(tail_lines(text.as_bytes(), true))
    }

    #[test]
    fn open_turn_is_running() {
        // task_started 之后还在出工具调用、没有 task_complete：Codex 正在干活，别打扰也别当成完成。
        assert_eq!(tail(RUNNING).status, Status::Running);
    }

    #[test]
    fn async_question_is_needs_you_even_after_turn_completes() {
        // request_user_input_async 不阻塞 turn，turn 会照常 task_complete；
        // 只看 turn 边界会把「等你回答」误判成「已完成」，用户就错过了提问。
        let state = tail(ASYNC_QUESTION);
        assert_eq!(state.status, Status::NeedsYou("等你回答".into()));
    }

    #[test]
    fn answered_question_no_longer_needs_you() {
        // 用户回答后（新 turn 带 UserMessage）提问就失效，不能一直挂着「等你」。
        let state = tail(ANSWERED);
        assert_eq!(state.status, Status::Done);
        assert_eq!(state.last_user.as_deref(), Some("先做电脑单机"));
    }

    #[test]
    fn completed_turn_is_done() {
        assert_eq!(tail(DONE).status, Status::Done);
    }

    #[test]
    fn aborted_turn_is_interrupted_not_done() {
        // 中断 ≠ 完成：用户按了停止，活没干完，显示成「已完成」会误导。
        assert_eq!(
            tail(INTERRUPTED).status,
            Status::Interrupted("interrupted".into())
        );
        assert_eq!(Status::Interrupted("interrupted".into()).label(), "已中断");
    }

    #[test]
    fn tail_without_turn_edges_is_unknown() {
        // 64 KB 内全是工具输出时宁可说不知道，也不瞎猜。
        let only_items = RUNNING
            .lines()
            .filter(|l| !l.contains("\"task_"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(tail(&only_items).status, Status::Unknown);
    }

    #[test]
    fn title_prefers_name_then_request_part_of_attachment_preamble() {
        // 带附件的首条消息以「# Files mentioned by the user」开头，直接截断会得到一串没用的前言。
        let title = "# Files mentioned by the user:\n\n## a.txt: /tmp/a.txt\n\n## My request:\n帮我检查下 vpc 接入问题";
        assert_eq!(
            clean_title("", title, "").as_deref(),
            Some("帮我检查下 vpc 接入问题")
        );
        assert_eq!(
            clean_title("检查 VPC", title, "").as_deref(),
            Some("检查 VPC")
        );
        assert_eq!(clean_title("", "", "  ").as_deref(), None);
    }

    #[test]
    fn session_meta_marks_subagents_and_desktop_origin() {
        // 子 agent 线程不是用户的会话；originator 决定回车走深链还是找终端。
        let mut lines = SESSION_META.lines();
        let user = parse_session_meta(lines.next().unwrap()).unwrap();
        assert!(!user.subagent);
        assert_eq!(user.originator.as_deref(), Some("Codex Desktop"));
        assert_eq!(user.cwd, Some(PathBuf::from("/Users/me/code/game")));
        let sub = parse_session_meta(lines.next().unwrap()).unwrap();
        assert!(sub.subagent);
    }

    #[test]
    fn lock_probe_detects_live_writer_only() {
        // 锁文件在进程退出后会留下来；只有真有人持锁才算活，否则列表全是死会话。
        let dir = std::env::temp_dir().join(format!("huan-lock-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lock = dir.join("thread.lock");
        std::fs::write(&lock, b"").unwrap();
        assert!(!writer_alive(&lock).unwrap());
        let holder = File::open(&lock).unwrap();
        // SAFETY: holder 存活期间 fd 有效。
        assert_eq!(
            unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        assert!(writer_alive(&lock).unwrap());
        drop(holder);
        assert!(!writer_alive(&lock).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sqlite_uri_escapes_special_chars() {
        assert_eq!(
            sqlite_uri(Path::new("/a b/c?#%.sqlite")),
            "file:/a%20b/c%3f%23%25.sqlite?mode=ro&immutable=1"
        );
    }
}
