//! 会话的统一模型、排序打分与副标题格式。纯函数，不做 IO（git 分支读取除外，见 `git_branch`）。

use std::path::{Path, PathBuf};

use crate::matcher;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Claude,
    Codex,
}

impl Source {
    pub fn badge(self) -> &'static str {
        match self {
            Source::Claude => "Claude",
            Source::Codex => "Codex",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Codex => "codex",
        }
    }
}

/// 统一状态机（调研 10 §5.1）。第一版没有「看过没看过」，所以 Claude 的 idle 不区分 Done。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Running,
    /// 等你：权限确认 / 提问。字符串是给人看的短标签。
    NeedsYou(String),
    Idle,
    Done,
    /// 被中断，带 Codex 的 reason（interrupted / replaced / …）。
    Interrupted(String),
    /// 尾部 64 KB 里找不到任何 turn 边界，判断不了。
    Unknown,
}

impl Status {
    pub fn needs_you(&self) -> bool {
        matches!(self, Status::NeedsYou(_))
    }

    pub fn label(&self) -> String {
        match self {
            Status::Running => "运行中".into(),
            Status::NeedsYou(label) => label.clone(),
            Status::Idle => "空闲".into(),
            Status::Done => "已完成".into(),
            Status::Interrupted(reason) if reason == "interrupted" || reason.is_empty() => {
                "已中断".into()
            }
            Status::Interrupted(reason) => format!("已中断（{reason}）"),
            Status::Unknown => "状态未知".into(),
        }
    }
}

/// 回车后怎么跳过去。终端类目标在执行时才解析 tty，因为 tty 只在那一刻有意义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Jump {
    /// 桌面 app 深链：`claude://claude.ai/epitaxy/<id>`、`codex://threads/<id>`。
    DeepLink(String),
    /// 已知进程号（Claude 注册表给的 pid）。
    Pid(i32),
    /// Codex CLI：执行时用 lsof 查谁持有这把写锁，得到 pid。
    CodexLock(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Session {
    pub source: Source,
    /// Claude 的 sessionId / Codex 的 threadId。
    pub session_id: String,
    pub title: String,
    pub status: Status,
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
    /// 毫秒时间戳。
    pub last_activity_ms: u64,
    pub jump: Jump,
}

impl Session {
    /// 不含 provider 前缀的 domain id：`claude:<sessionId>` / `codex:<threadId>`。
    pub fn domain_id(&self) -> String {
        format!("{}:{}", self.source.key(), self.session_id)
    }

    /// 「状态 · ~/项目目录 · git 分支 · 3 分钟前」，缺的段落省略。
    pub fn subtitle(&self, home: Option<&Path>, now_ms: u64) -> String {
        let mut parts = vec![self.status.label()];
        if let Some(cwd) = &self.cwd {
            parts.push(display_path(cwd, home));
        }
        if let Some(branch) = &self.branch {
            parts.push(branch.clone());
        }
        parts.push(ago(now_ms, self.last_activity_ms));
        parts.join(" · ")
    }
}

/// 列出全部会话的关键词。`claude` / `codex` 只列对应来源。
fn keyword_filter(query: &str) -> Option<Option<Source>> {
    match query.trim().to_lowercase().as_str() {
        "" | "agent" | "agents" | "会话" => Some(None),
        "claude" => Some(Some(Source::Claude)),
        "codex" => Some(Some(Source::Codex)),
        _ => None,
    }
}

/// 应用精确命中是 1000 分。会话的分数在同等匹配质量下要比应用低一档，避免抢走「回车开应用」。
const MATCH_PENALTY: u32 = 50;
/// 普通查询里「等你」的会话略靠前，但不跨档（50 > 20）。
const NEEDS_YOU_MATCH_BONUS: u32 = 20;
/// 列全部时：「等你」一档、其余一档，档内按最近活动加 0..=99。最高 999，仍低于应用精确命中。
const LIST_NEEDS_YOU_BASE: u32 = 900;
const LIST_OTHER_BASE: u32 = 800;

/// 给每个会话打分，返回 `(下标, 分数)`，不匹配的不返回。
pub fn rank(sessions: &[Session], query: &str, home: Option<&Path>) -> Vec<(usize, u32)> {
    if let Some(filter) = keyword_filter(query) {
        let mut listed: Vec<usize> = (0..sessions.len())
            .filter(|&i| filter.is_none_or(|source| sessions[i].source == source))
            .collect();
        listed.sort_by_key(|&i| std::cmp::Reverse(sessions[i].last_activity_ms));
        return listed
            .into_iter()
            .enumerate()
            .map(|(rank, i)| {
                let base = if sessions[i].status.needs_you() {
                    LIST_NEEDS_YOU_BASE
                } else {
                    LIST_OTHER_BASE
                };
                (i, base + 99 - (rank as u32).min(99))
            })
            .collect();
    }

    sessions
        .iter()
        .enumerate()
        .filter_map(|(i, session)| {
            let best = match_score(session, query, home)?;
            let bonus = if session.status.needs_you() {
                NEEDS_YOU_MATCH_BONUS
            } else {
                0
            };
            Some((i, best.saturating_sub(MATCH_PENALTY) + bonus))
        })
        .collect()
}

/// 标题任何匹配都算；目录与分支只认子串及以上（≥700），否则长路径的子序列匹配会到处误中。
fn match_score(session: &Session, query: &str, home: Option<&Path>) -> Option<u32> {
    let strict = |candidate: &str| matcher::score(query, candidate).filter(|&s| s >= 700);
    let mut scores = vec![matcher::score(query, &session.title)];
    if let Some(cwd) = &session.cwd {
        scores.push(strict(&display_path(cwd, home)));
        if let Some(name) = cwd.file_name() {
            scores.push(matcher::score(query, &name.to_string_lossy()).filter(|&s| s >= 700));
        }
    }
    if let Some(branch) = &session.branch {
        scores.push(strict(branch));
    }
    scores.into_iter().flatten().max()
}

pub fn display_path(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

pub fn ago(now_ms: u64, then_ms: u64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        0..60 => "刚刚".into(),
        60..3600 => format!("{} 分钟前", secs / 60),
        3600..86_400 => format!("{} 小时前", secs / 3600),
        _ => format!("{} 天前", secs / 86_400),
    }
}

/// 压成一行并截断到 `max` 个字符。
pub fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// 读 `<repo>/.git/HEAD` 拿当前分支（支持 worktree 的 `.git` 文件）。不 spawn git，便宜。
pub fn git_branch(cwd: &Path) -> Option<String> {
    let dir = cwd.ancestors().find(|dir| dir.join(".git").exists())?;
    let dot_git = dir.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let content = std::fs::read_to_string(&dot_git).ok()?;
        let path = PathBuf::from(content.strip_prefix("gitdir:")?.trim());
        if path.is_absolute() {
            path
        } else {
            dir.join(path)
        }
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => Some(branch.to_string()),
        None => head.get(..7).map(|sha| format!("detached {sha}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(title: &str, status: Status, last_activity_ms: u64, source: Source) -> Session {
        Session {
            source,
            session_id: title.into(),
            title: title.into(),
            status,
            cwd: Some(PathBuf::from("/Users/me/code/helep")),
            branch: Some("master".into()),
            last_activity_ms,
            jump: Jump::Pid(1),
        }
    }

    fn ordered(sessions: &[Session], query: &str) -> Vec<(String, u32)> {
        let mut ranked = rank(sessions, query, None);
        ranked.sort_by_key(|&(_, score)| std::cmp::Reverse(score));
        ranked
            .into_iter()
            .map(|(i, score)| (sessions[i].title.clone(), score))
            .collect()
    }

    #[test]
    fn empty_query_puts_waiting_sessions_first_then_recency() {
        // 唤起面板第一眼就该看到「哪个会话卡在等我」，哪怕它很久没动；其余按最近活动，最近的最可能是要切回去的。
        let sessions = vec![
            session("旧的空闲", Status::Idle, 1_000, Source::Claude),
            session("最近在跑", Status::Running, 9_000, Source::Codex),
            session(
                "很久前就在等你",
                Status::NeedsYou("等你回答".into()),
                10,
                Source::Codex,
            ),
            session("较新的完成", Status::Done, 5_000, Source::Claude),
        ];
        let titles: Vec<_> = ordered(&sessions, "").into_iter().map(|(t, _)| t).collect();
        assert_eq!(
            titles,
            ["很久前就在等你", "最近在跑", "较新的完成", "旧的空闲"]
        );
    }

    #[test]
    fn listing_never_reaches_exact_app_match() {
        // 输入「claude」时 Claude.app 精确命中 1000 分，回车必须仍然打开应用而不是某个会话。
        let sessions = vec![session(
            "x",
            Status::NeedsYou("等你".into()),
            1,
            Source::Claude,
        )];
        for query in ["", "claude", "agent", "会话"] {
            let (_, score) = ordered(&sessions, query)[0].clone();
            assert!(score < 1000, "{query}: {score}");
        }
    }

    #[test]
    fn source_keyword_filters_to_that_source() {
        // 输入「codex」只想看 Codex 会话，混进 Claude 会话是噪音。
        let sessions = vec![
            session("c", Status::Idle, 1, Source::Claude),
            session("x", Status::Idle, 2, Source::Codex),
        ];
        let titles: Vec<_> = ordered(&sessions, "codex")
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(titles, ["x"]);
        assert_eq!(ordered(&sessions, "agent").len(), 2);
    }

    #[test]
    fn normal_query_stays_below_apps_of_same_match_quality() {
        // 会话标题恰好叫「Safari」也不能压过 Safari.app 的精确命中；前缀命中同理压不过应用的前缀命中。
        let sessions = vec![session(
            "Safari",
            Status::NeedsYou("等你".into()),
            1,
            Source::Claude,
        )];
        let (_, exact) = ordered(&sessions, "safari")[0].clone();
        assert!(exact < 1000);
        let (_, prefix) = ordered(&sessions, "saf")[0].clone();
        assert!(prefix < 900);
    }

    #[test]
    fn normal_query_matches_title_dir_and_branch_but_not_path_noise() {
        // 用户记得的是项目名或分支，要能搜到；但路径很长，子序列匹配会让随便几个字母都命中。
        let mut s = session("调研 GPUI", Status::Idle, 1, Source::Claude);
        s.branch = Some("feat-agents".into());
        let sessions = vec![s];
        assert_eq!(ordered(&sessions, "helep").len(), 1);
        assert_eq!(ordered(&sessions, "feat-ag").len(), 1);
        assert_eq!(ordered(&sessions, "gpui").len(), 1);
        assert!(ordered(&sessions, "usmh").is_empty());
    }

    #[test]
    fn subtitle_has_status_dir_branch_and_age() {
        let s = session(
            "t",
            Status::NeedsYou("等你确认权限".into()),
            0,
            Source::Claude,
        );
        let home = PathBuf::from("/Users/me");
        assert_eq!(
            s.subtitle(Some(&home), 3 * 60_000 + 5),
            "等你确认权限 · ~/code/helep · master · 3 分钟前"
        );
    }

    #[test]
    fn one_line_flattens_and_truncates() {
        assert_eq!(one_line("a\n\n b  c", 10), "a b c");
        assert_eq!(one_line("一二三四五", 3), "一二三…");
    }
}
