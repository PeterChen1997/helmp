//! Claude Code 会话：以 `~/.claude/sessions/<pid>.json` 注册表为主（每个活进程一份，状态变化即重写）。
//!
//! - 判活：pid 存活且启动时间与 `startedAt` 吻合；文件在、进程没了的是 kill -9 留下的孤儿，忽略。
//! - 桌面 app 会话：`hostSessionId` 指向 `~/Library/Application Support/Claude/claude-code-sessions/*/*/local_<id>.json`，
//!   从那里取标题和最近活动时间，跳转用 `claude://claude.ai/epitaxy/<id>`。
//! - 没有标题时才读 `~/.claude/projects/*/<sessionId>.jsonl` 尾部 64 KB 找 `custom-title` / `last-prompt`，绝不全量读 jsonl。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use serde::Deserialize;

use super::state::{self, Jump, Session, Source, Status};
use super::{TAIL_BYTES, Warnings, mtime_ms, process, read_tail};
use crate::i18n::{tr, trf};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryEntry {
    pub pid: i32,
    pub session_id: String,
    pub cwd: Option<String>,
    pub status: Option<String>,
    pub waiting_for: Option<String>,
    pub name: Option<String>,
    pub entrypoint: Option<String>,
    pub kind: Option<String>,
    pub host_session_id: Option<String>,
    pub started_at: Option<u64>,
    pub updated_at: Option<u64>,
    pub status_updated_at: Option<u64>,
}

pub fn parse_registry(text: &str) -> Result<RegistryEntry> {
    Ok(serde_json::from_str(text)?)
}

/// 注册表 status → 统一状态。`waiting` 一律算「等你」，`waitingFor` 决定标签。
/// 标签存中文原文（缓存会跨语言切换），显示时由 `Status::label` / `tag` 翻译。
pub fn status_of(entry: &RegistryEntry) -> Status {
    match entry.status.as_deref() {
        Some("busy") => Status::Running,
        Some("waiting") => Status::NeedsYou(match entry.waiting_for.as_deref() {
            Some("permission prompt") => "等你确认权限".into(),
            Some(other) if !other.is_empty() => format!("等你：{other}"),
            _ => "等你".into(),
        }),
        Some("idle") => Status::Idle,
        _ => Status::Unknown,
    }
}

/// 桌面 app 的 `local_*.json`（单个 200–300 KB，只在注册表变化时读）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSession {
    pub session_id: String,
    pub title: Option<String>,
    pub last_activity_at: Option<u64>,
}

pub fn parse_desktop(text: &str) -> Result<DesktopSession> {
    Ok(serde_json::from_str(text)?)
}

/// 会话 jsonl 尾部里能拿到的信息。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tail {
    /// 优先最后一条 `custom-title`（/rename 或桌面改名），其次最后一条 `last-prompt`。
    pub title: Option<String>,
    /// 最后一段 assistant 文字。
    pub last_message: Option<String>,
    /// 最后一个还没有 `tool_result` 的工具调用，等权限确认时就是它在等。
    pub pending_tool: Option<PendingTool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingTool {
    pub name: String,
    /// 给人看的参数摘要：命令、文件路径、URL 或提问。
    pub summary: Option<String>,
}

impl PendingTool {
    /// 「需要你确认：是否允许运行 npm test？」这类一句话（设计稿 Agents 的「最后一条消息」）。
    /// 每轮扫描都从缓存的 `PendingTool` 重新生成，所以切换语言后下一轮就是新语言。
    pub fn describe(&self) -> String {
        let name = match self.name.as_str() {
            "" => tr("工具", "this tool"),
            name => name,
        };
        match (name, self.summary.as_deref()) {
            ("AskUserQuestion", Some(question)) => question.to_string(),
            ("Bash", Some(command)) => trf!(
                "需要你确认：是否允许运行 {command}？",
                "Approval needed: run {command}?"
            ),
            (name, Some(arg)) => trf!(
                "需要你确认：是否允许 {name} {arg}？",
                "Approval needed: allow {name} {arg}?"
            ),
            (name, None) => trf!(
                "需要你确认：是否允许 {name}？",
                "Approval needed: allow {name}?"
            ),
        }
    }
}

pub fn parse_tail<'a>(lines: impl Iterator<Item = &'a [u8]>) -> Tail {
    let mut custom = None;
    let mut prompt = None;
    let mut last_message = None;
    let mut pending: Option<(String, PendingTool)> = None;
    for line in lines {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        match value.get("type").and_then(|t| t.as_str()) {
            Some("custom-title") => {
                custom = value
                    .get("customTitle")
                    .and_then(|t| t.as_str())
                    .map(String::from)
            }
            Some("last-prompt") => {
                prompt = value
                    .get("lastPrompt")
                    .and_then(|t| t.as_str())
                    .map(String::from)
            }
            Some("assistant" | "user") => {
                let blocks = value["message"]["content"].as_array();
                let mut reply = Vec::new();
                for block in blocks.into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") if value["type"] == "assistant" => {
                            if let Some(text) =
                                block["text"].as_str().filter(|t| !t.trim().is_empty())
                            {
                                reply.push(text);
                            }
                        }
                        Some("tool_use") => {
                            let id = block["id"].as_str().unwrap_or_default().to_string();
                            // 缺名字时留空，`describe` 显示时再填「工具」，避免把某一种语言存进缓存。
                            let name = block["name"].as_str().unwrap_or_default().to_string();
                            let summary = tool_summary(&name, &block["input"]);
                            pending = Some((id, PendingTool { name, summary }));
                        }
                        Some("tool_result")
                            if pending.as_ref().is_some_and(|(id, _)| {
                                block["tool_use_id"].as_str() == Some(id)
                            }) =>
                        {
                            pending = None;
                        }
                        _ => {}
                    }
                }
                if !reply.is_empty() {
                    last_message = Some(reply.join("\n\n"));
                }
            }
            _ => {}
        }
    }
    Tail {
        title: custom
            .or(prompt)
            .filter(|t| !t.trim().is_empty())
            .map(|t| state::one_line(&t, 60)),
        last_message,
        pending_tool: pending.map(|(_, tool)| tool),
    }
}

fn tool_summary(name: &str, input: &serde_json::Value) -> Option<String> {
    let field = |key: &str| input[key].as_str().filter(|s| !s.trim().is_empty());
    let text = match name {
        "AskUserQuestion" => input["questions"][0]["question"].as_str(),
        _ => field("command")
            .or_else(|| field("file_path"))
            .or_else(|| field("notebook_path"))
            .or_else(|| field("url"))
            .or_else(|| field("pattern")),
    }?;
    Some(state::one_line(text, 120))
}

struct Cached {
    mtime: SystemTime,
    entry: RegistryEntry,
    desktop: Option<DesktopSession>,
    jsonl: Option<PathBuf>,
    tail: Tail,
}

pub struct ClaudeScanner {
    sessions_dir: PathBuf,
    projects_dir: PathBuf,
    desktop_dir: PathBuf,
    cache: HashMap<PathBuf, Cached>,
}

impl ClaudeScanner {
    pub fn new(home: &Path) -> Self {
        let config = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude"));
        Self {
            sessions_dir: config.join("sessions"),
            projects_dir: config.join("projects"),
            desktop_dir: home.join("Library/Application Support/Claude/claude-code-sessions"),
            cache: HashMap::new(),
        }
    }

    pub fn scan(&mut self, warnings: &mut Warnings) -> Vec<Session> {
        let entries = match std::fs::read_dir(&self.sessions_dir) {
            Ok(entries) => entries,
            // 没装 / 没用过 Claude Code：不是错误。
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                self.cache.clear();
                return Vec::new();
            }
            Err(err) => {
                warnings.push(format!("读取 {} 失败：{err}", self.sessions_dir.display()));
                return Vec::new();
            }
        };

        let mut seen = Vec::new();
        for dirent in entries.flatten() {
            let path = dirent.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let Ok(mtime) = dirent.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            seen.push(path.clone());
            if self.cache.get(&path).is_some_and(|c| c.mtime == mtime) {
                continue;
            }
            match self.load(&path, mtime, warnings) {
                Ok(cached) => {
                    self.cache.insert(path, cached);
                }
                Err(err) => {
                    // 可能正被重写到一半；不缓存，下一轮重试。
                    self.cache.remove(&path);
                    warnings.push(format!("解析 {} 失败：{err:#}", path.display()));
                }
            }
        }
        self.cache.retain(|path, _| seen.contains(path));

        self.cache
            .values()
            .filter(|c| c.entry.kind.as_deref() != Some("bg"))
            .filter(|c| process::alive(c.entry.pid, c.entry.started_at))
            .map(to_session)
            .collect()
    }

    fn load(&self, path: &Path, mtime: SystemTime, warnings: &mut Warnings) -> Result<Cached> {
        let text = std::fs::read_to_string(path)?;
        let entry = parse_registry(&text)?;

        let desktop = match entry.host_session_id.as_deref() {
            Some(id) => match self.find_desktop(id) {
                Some(local) => match std::fs::read_to_string(&local)
                    .map_err(anyhow::Error::from)
                    .and_then(|text| parse_desktop(&text))
                {
                    Ok(desktop) => Some(desktop),
                    Err(err) => {
                        warnings.push(format!("解析 {} 失败：{err:#}", local.display()));
                        None
                    }
                },
                None => None,
            },
            None => None,
        };

        // 尾部给出标题兜底、最后一条消息和「在等哪个工具」。注册表在状态变化时重写，
        // 所以每次状态变化读一次尾部 64 KB；读失败只告警，会话照样列出。
        let jsonl = self.find_jsonl(&entry.session_id);
        let tail = match &jsonl {
            Some(jsonl) => match read_tail(jsonl, TAIL_BYTES) {
                Ok((bytes, truncated)) => parse_tail(super::tail_lines(&bytes, truncated)),
                Err(err) => {
                    warnings.push(format!("读取 {} 失败：{err}", jsonl.display()));
                    Tail::default()
                }
            },
            None => Tail::default(),
        };

        Ok(Cached {
            mtime,
            entry,
            desktop,
            jsonl,
            tail,
        })
    }

    /// `claude-code-sessions/<uuidA>/<uuidB>/<hostSessionId>.json`，两层目录，只看文件名不读内容。
    fn find_desktop(&self, host_session_id: &str) -> Option<PathBuf> {
        let file = format!("{host_session_id}.json");
        std::fs::read_dir(&self.desktop_dir)
            .ok()?
            .flatten()
            .filter_map(|a| std::fs::read_dir(a.path()).ok())
            .flat_map(|b| b.flatten())
            .map(|b| b.path().join(&file))
            .find(|candidate| candidate.is_file())
    }

    /// `projects/<cwd 转 slug>/<sessionId>.jsonl`。slug 规则不公开，逐个项目目录试，只 stat 不读。
    fn find_jsonl(&self, session_id: &str) -> Option<PathBuf> {
        let file = format!("{session_id}.jsonl");
        std::fs::read_dir(&self.projects_dir)
            .ok()?
            .flatten()
            .map(|dir| dir.path().join(&file))
            .find(|candidate| candidate.is_file())
    }
}

fn nonempty(text: Option<&str>) -> Option<&str> {
    text.filter(|t| !t.trim().is_empty())
}

fn to_session(cached: &Cached) -> Session {
    let entry = &cached.entry;
    let desktop = cached.desktop.as_ref();
    let title = nonempty(desktop.and_then(|d| d.title.as_deref()))
        .or(nonempty(entry.name.as_deref()))
        .map(|t| state::one_line(t, 80))
        .or_else(|| cached.tail.title.clone())
        .unwrap_or_else(|| tr("（无标题会话）", "(Untitled session)").into());

    let status = status_of(entry);
    // 等你的会话优先说清楚在等什么（哪条命令要确认、问了什么）；拿不到就退回最后一段回复。
    let waiting_for = cached
        .tail
        .pending_tool
        .as_ref()
        .filter(|_| status.needs_you())
        .map(PendingTool::describe);
    let last_message = waiting_for.or_else(|| cached.tail.last_message.clone());

    let last_activity_ms = [
        entry.updated_at,
        entry.status_updated_at,
        desktop.and_then(|d| d.last_activity_at),
        cached.jsonl.as_deref().and_then(mtime_ms),
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or(0);

    let cwd = entry.cwd.as_deref().map(PathBuf::from);
    let branch = cwd.as_deref().and_then(state::git_branch);

    let desktop_id = desktop
        .map(|d| d.session_id.as_str())
        .or(entry.host_session_id.as_deref());
    let jump = match (entry.entrypoint.as_deref(), desktop_id) {
        (Some("claude-desktop"), Some(id)) => {
            Jump::DeepLink(format!("claude://claude.ai/epitaxy/{id}"))
        }
        _ => Jump::Pid(entry.pid),
    };

    Session {
        source: Source::Claude,
        session_id: entry.session_id.clone(),
        title,
        status,
        cwd,
        branch,
        last_activity_ms,
        status_revision: entry
            .status_updated_at
            .or(entry.updated_at)
            .unwrap_or(last_activity_ms)
            .to_string(),
        last_message,
        jump,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESKTOP_REGISTRY: &str = include_str!("fixtures/claude_registry_desktop.json");
    const WAITING_REGISTRY: &str = include_str!("fixtures/claude_registry_waiting.json");
    const CLI_REGISTRY: &str = include_str!("fixtures/claude_registry_cli.json");
    const DESKTOP_LOCAL: &str = include_str!("fixtures/claude_desktop_local.json");
    const SESSION_TAIL: &str = include_str!("fixtures/claude_session_tail.jsonl");

    #[test]
    fn registry_states_map_to_unified_status() {
        // 「等你」是切换器存在的理由：waiting 必须识别出来，并把权限确认说成人话。
        let idle = parse_registry(DESKTOP_REGISTRY).unwrap();
        assert_eq!(status_of(&idle), Status::Idle);
        let waiting = parse_registry(WAITING_REGISTRY).unwrap();
        assert_eq!(status_of(&waiting), Status::NeedsYou("等你确认权限".into()));
        let busy = parse_registry(CLI_REGISTRY).unwrap();
        assert_eq!(status_of(&busy), Status::Running);
    }

    #[test]
    fn desktop_session_jumps_by_host_session_id_deep_link() {
        // 桌面会话必须用 epitaxy 深链打开原会话；`claude://resume` 会把它导入成一个新会话。
        let entry = parse_registry(DESKTOP_REGISTRY).unwrap();
        let desktop = parse_desktop(DESKTOP_LOCAL).unwrap();
        assert_eq!(
            Some(desktop.session_id.as_str()),
            entry.host_session_id.as_deref()
        );
        let session = to_session(&Cached {
            mtime: SystemTime::UNIX_EPOCH,
            entry,
            desktop: Some(desktop),
            jsonl: None,
            tail: Tail::default(),
        });
        assert_eq!(
            session.jump,
            Jump::DeepLink(
                "claude://claude.ai/epitaxy/local_11111111-2222-4333-8444-555555555555".into()
            )
        );
        assert_eq!(session.title, "作品页更新：三张封面");
        // 桌面 lastActivityAt 比注册表 updatedAt 新，取较新的那个作为「最近活动」。
        assert_eq!(session.last_activity_ms, 1_793_877_533_280);
    }

    #[test]
    fn cli_session_jumps_by_pid_and_falls_back_to_last_prompt_title() {
        // 终端会话没有桌面 id，只能按进程找终端窗口；没起名时用最后一句用户提问，好过一串 uuid。
        let entry = parse_registry(CLI_REGISTRY).unwrap();
        let tail = parse_tail(super::super::tail_lines(SESSION_TAIL.as_bytes(), true));
        assert_eq!(
            tail.title.as_deref(),
            Some("把首页截图换成视频第一帧，然后部署")
        );
        let session = to_session(&Cached {
            mtime: SystemTime::UNIX_EPOCH,
            entry,
            desktop: None,
            jsonl: None,
            tail,
        });
        assert_eq!(session.jump, Jump::Pid(424_242));
        assert_eq!(session.title, "把首页截图换成视频第一帧，然后部署");
        // 详情的「最后一条消息」：已完成的会话给最后一段回复。
        assert_eq!(session.last_message.as_deref(), Some("已完成。"));
    }

    #[test]
    fn custom_title_beats_last_prompt() {
        // 用户 /rename 过的名字是他自己认的，优先于自动取的最后一句话。
        let tail = format!(
            "{SESSION_TAIL}\n{{\"type\":\"custom-title\",\"customTitle\":\"我起的名字\",\"sessionId\":\"x\"}}\n"
        );
        let tail = parse_tail(super::super::tail_lines(tail.as_bytes(), true));
        assert_eq!(tail.title.as_deref(), Some("我起的名字"));
    }

    const PENDING_BASH: &str = concat!(
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"先跑一遍测试。"}]}}"#,
        "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"npm test","description":"run"}}]}}"#,
        "\n"
    );

    #[test]
    fn waiting_session_says_what_it_waits_for() {
        // 设计稿 Agents：等确认的会话要直接说「是否允许运行 npm test」，用户不切过去就能判断要不要马上处理。
        let tail = parse_tail(super::super::tail_lines(PENDING_BASH.as_bytes(), false));
        assert_eq!(
            tail.pending_tool,
            Some(PendingTool {
                name: "Bash".into(),
                summary: Some("npm test".into())
            })
        );
        let session = to_session(&Cached {
            mtime: SystemTime::UNIX_EPOCH,
            entry: parse_registry(WAITING_REGISTRY).unwrap(),
            desktop: None,
            jsonl: None,
            tail: tail.clone(),
        });
        assert_eq!(
            session.last_message.as_deref(),
            Some("需要你确认：是否允许运行 npm test？")
        );
        // 不在等你时，挂着的工具调用只是正在执行，不能说成「需要你确认」。
        let busy = to_session(&Cached {
            mtime: SystemTime::UNIX_EPOCH,
            entry: parse_registry(CLI_REGISTRY).unwrap(),
            desktop: None,
            jsonl: None,
            tail,
        });
        assert_eq!(busy.last_message.as_deref(), Some("先跑一遍测试。"));
    }

    #[test]
    fn answered_tool_call_is_not_pending() {
        // 工具已经有结果说明权限早就批了；还拿它当「在等什么」会误导用户。
        let done = format!(
            "{PENDING_BASH}{}\n",
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#
        );
        let tail = parse_tail(super::super::tail_lines(done.as_bytes(), false));
        assert_eq!(tail.pending_tool, None);
    }

    #[test]
    fn ask_user_question_shows_the_question() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"q","name":"AskUserQuestion","input":{"questions":[{"question":"先做哪个平台？","options":[]}]}}]}}"#;
        let tail = parse_tail(super::super::tail_lines(line.as_bytes(), false));
        assert_eq!(
            tail.pending_tool.map(|t| t.describe()).as_deref(),
            Some("先做哪个平台？")
        );
    }
}
