//! 会话的统一模型、排序打分与副标题格式。纯函数，不做 IO（git 分支读取除外，见 `git_branch`）。

use std::path::{Path, PathBuf};

use crate::engine::waiting_section;
use crate::i18n::{is_en, tr, trf};
use crate::matcher;
use crate::provider::Tone;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Claude,
    Codex,
}

impl Source {
    /// 范围筛选的分段名（「全部」不对应任何来源）。
    pub fn from_filter(filter: &str) -> Option<Source> {
        match filter {
            "Claude" => Some(Source::Claude),
            "Codex" => Some(Source::Codex),
            _ => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Codex => "codex",
        }
    }
}

/// 统一状态机（调研 10 §5.1）。Claude 报告 idle，Codex 能明确报告 Done。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Running,
    /// 等你：权限确认 / 提问。字符串是中文短标签（「等你确认权限」「等你回答」「等你：…」），
    /// 扫描时生成、会跨语言切换保存，所以存中文原文当键，显示时再按语言翻译（见 `label` / `tag`）。
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
            Status::Running => tr("运行中", "Running").into(),
            Status::NeedsYou(label) => needs_you_label(label),
            Status::Idle => tr("空闲", "Idle").into(),
            Status::Done => tr("已完成", "Done").into(),
            Status::Interrupted(reason) if reason == "interrupted" || reason.is_empty() => {
                tr("已中断", "Interrupted").into()
            }
            Status::Interrupted(reason) => trf!("已中断（{reason}）", "Interrupted ({reason})"),
            Status::Unknown => tr("状态未知", "Unknown").into(),
        }
    }

    /// 列表行右侧的状态标签：比 `label` 短（设计稿列表写「等你确认」，详情写「等你确认权限」）。
    pub fn tag(&self) -> (String, Tone) {
        match self {
            Status::NeedsYou(label) if label.starts_with("等你确认") => {
                (tr("等你确认", "Needs permission").into(), Tone::Waiting)
            }
            Status::NeedsYou(label) if label.starts_with("等你：") => {
                (tr("等你处理", "Needs you").into(), Tone::Waiting)
            }
            Status::NeedsYou(label) if is_en() && label == "等你回答" => {
                ("Needs answer".into(), Tone::Waiting)
            }
            Status::NeedsYou(label) if is_en() && label == "等你" => {
                ("Needs you".into(), Tone::Waiting)
            }
            Status::NeedsYou(label) => (needs_you_label(label), Tone::Waiting),
            Status::Running => (self.label(), Tone::Running),
            Status::Done => (self.label(), Tone::Done),
            Status::Idle | Status::Interrupted(_) | Status::Unknown => {
                (self.label(), Tone::Neutral)
            }
        }
    }

    pub fn tone(&self) -> Tone {
        self.tag().1
    }

    /// 范围视图里的分组和组的基础分。
    fn section(&self) -> (&'static str, u32) {
        let index = match self {
            Status::NeedsYou(_) => 0,
            Status::Done | Status::Idle => 1,
            Status::Running => 2,
            Status::Interrupted(_) | Status::Unknown => 3,
        };
        (scoped_sections()[index], 900 - index as u32 * 100)
    }
}

pub(crate) fn scoped_sections() -> [&'static str; 4] {
    [
        waiting_section(),
        tr("最近完成", "Recently Completed"),
        tr("进行中", "In Progress"),
        tr("其他", "Other"),
    ]
}

/// `NeedsYou` 的中文短标签 → 当前语言的说明（详情标签用）。认不出的原样返回。
fn needs_you_label(label: &str) -> String {
    if !is_en() {
        return label.into();
    }
    match label {
        "等你确认权限" => "Waiting for permission".into(),
        "等你回答" => "Waiting for your answer".into(),
        "等你" => "Waiting for you".into(),
        _ => match label.strip_prefix("等你：") {
            Some(what) => format!("Waiting for {what}"),
            None => label.into(),
        },
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
    /// 状态所属的轮次或更新时间。与列表的最近活动时间分开，避免改标题等操作重置已读。
    pub status_revision: String,
    /// 来源确认已读的当前轮次，例如 Codex 桌面端写入的回执。
    pub read_in_source: bool,
    /// 详情里「最后一条消息」：等你的会话是在等什么（要确认的命令、提的问题），否则是最后一段回复。
    pub last_message: Option<String>,
    pub jump: Jump,
}

impl Session {
    pub fn completion_revision(&self) -> Option<String> {
        let kind = match self.status {
            Status::Done => "done",
            Status::Idle => "idle",
            _ => return None,
        };
        Some(format!("{kind}:{}", self.status_revision))
    }

    /// 不含 provider 前缀的 domain id：`claude:<sessionId>` / `codex:<threadId>`。
    pub fn domain_id(&self) -> String {
        format!("{}:{}", self.source.key(), self.session_id)
    }

    /// 详情「来源」：深链跳转的是桌面 app，其余是终端里的 CLI。
    pub fn origin(&self) -> &'static str {
        let desktop = matches!(self.jump, Jump::DeepLink(_));
        match (self.source, desktop) {
            (Source::Claude, true) => tr("Claude 桌面版", "Claude Desktop"),
            (Source::Claude, false) => tr("Claude Code 终端", "Claude Code (Terminal)"),
            (Source::Codex, true) => tr("Codex 桌面版", "Codex Desktop"),
            (Source::Codex, false) => "Codex CLI",
        }
    }

    /// 「~/项目目录 · git 分支 · 3 分钟前」，缺的段落省略。状态在右侧标签里，不重复。
    pub fn subtitle(&self, home: Option<&Path>, now_ms: u64) -> String {
        let mut parts = Vec::new();
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

/// 列出全部会话的关键词（与界面语言无关，中英都认）。`claude` / `codex` 只列对应来源。
fn keyword_filter(query: &str) -> Option<Option<Source>> {
    match query.trim().to_lowercase().as_str() {
        "" | "agent" | "agents" | "会话" | "session" | "sessions" => Some(None),
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

/// 范围视图（`ag `）：只看 `only` 来源的会话，返回 `(下标, 分数, 分组名)`。
/// 空查询列出全部，按 等你处理 / 最近完成 / 进行中 / 其他 分组，组内最近活动在前；
/// 有查询时只留命中的，分数是匹配分，分组仍按状态。
pub fn rank_scoped(
    sessions: &[Session],
    query: &str,
    only: Option<Source>,
    home: Option<&Path>,
) -> Vec<(usize, u32, &'static str)> {
    let of_source = |i: &usize| only.is_none_or(|source| sessions[*i].source == source);
    if query.trim().is_empty() {
        let mut listed: Vec<usize> = (0..sessions.len()).filter(of_source).collect();
        listed.sort_by_key(|&i| std::cmp::Reverse(sessions[i].last_activity_ms));
        return listed
            .into_iter()
            .enumerate()
            .map(|(rank, i)| {
                let (section, base) = sessions[i].status.section();
                (i, base + 99 - (rank as u32).min(99), section)
            })
            .collect();
    }
    sessions
        .iter()
        .enumerate()
        .filter(|(i, _)| of_source(i))
        .filter_map(|(i, session)| {
            let score = match_score(session, query, home)?;
            Some((i, score, session.status.section().0))
        })
        .collect()
}

/// 首页「等你处理」：只放等你的会话，最近活动在前，分数 900..=999（engine 约定 ≥ 900）。
pub fn waiting(sessions: &[Session]) -> Vec<(usize, u32)> {
    let mut listed: Vec<usize> = (0..sessions.len())
        .filter(|&i| sessions[i].status.needs_you())
        .collect();
    listed.sort_by_key(|&i| std::cmp::Reverse(sessions[i].last_activity_ms));
    listed
        .into_iter()
        .enumerate()
        .map(|(rank, i)| (i, LIST_NEEDS_YOU_BASE + 99 - (rank as u32).min(99)))
        .collect()
}

/// 标题、目录与分支都只认子串及以上（≥700，含拼音首字母从头命中）。
/// 会话标题往往是一整句话，子序列和拼音中段匹配会让「wx」之类的短查询到处误中，挤掉真正想找的应用。
fn match_score(session: &Session, query: &str, home: Option<&Path>) -> Option<u32> {
    let strict = |candidate: &str| matcher::score(query, candidate).filter(|&s| s >= 700);
    let mut scores = vec![strict(&session.title)];
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
        0..60 => tr("刚刚", "Just now").into(),
        60..3600 => trf!("{} 分钟前", "{} min ago", secs / 60),
        3600..86_400 => trf!("{} 小时前", "{} hr ago", secs / 3600),
        86_400..172_800 => trf!("{} 天前", "{} day ago", 1),
        _ => trf!("{} 天前", "{} days ago", secs / 86_400),
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
            status_revision: last_activity_ms.to_string(),
            read_in_source: false,
            last_message: None,
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
    fn short_query_does_not_fuzzy_match_long_titles() {
        // 「wx」是在找微信：会话标题是一整句话，拼音中段（位…巡）或子序列命中只会把无关会话塞到应用前面。
        let sessions = vec![
            session(
                "投资人物仓位巡检",
                Status::NeedsYou("等你确认".into()),
                10,
                Source::Claude,
            ),
            session("修复 wx 登录", Status::Idle, 10, Source::Claude),
        ];
        let titles: Vec<_> = ordered(&sessions, "wx")
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(titles, ["修复 wx 登录"]);
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
    fn subtitle_has_dir_branch_and_age_without_status() {
        // 设计稿：状态在右侧彩色标签里，副标题再写一遍会挤掉项目和分支。
        let s = session(
            "t",
            Status::NeedsYou("等你确认权限".into()),
            0,
            Source::Claude,
        );
        let home = PathBuf::from("/Users/me");
        assert_eq!(
            s.subtitle(Some(&home), 3 * 60_000 + 5),
            "~/code/helep · master · 3 分钟前"
        );
    }

    #[test]
    fn status_tags_use_design_tones() {
        // 颜色是一眼区分「要不要我管」的唯一线索：等你橙、运行中蓝、已完成绿、空闲灰。
        assert_eq!(
            Status::NeedsYou("等你确认权限".into()).tag(),
            ("等你确认".into(), Tone::Waiting)
        );
        assert_eq!(
            Status::NeedsYou("等你回答".into()).tag(),
            ("等你回答".into(), Tone::Waiting)
        );
        assert_eq!(Status::Running.tag(), ("运行中".into(), Tone::Running));
        assert_eq!(Status::Done.tag(), ("已完成".into(), Tone::Done));
        assert_eq!(Status::Idle.tag(), ("空闲".into(), Tone::Neutral));
    }

    fn scoped(
        sessions: &[Session],
        query: &str,
        only: Option<Source>,
    ) -> Vec<(String, &'static str)> {
        let mut ranked = rank_scoped(sessions, query, only, None);
        ranked.sort_by_key(|&(_, score, _)| std::cmp::Reverse(score));
        ranked
            .into_iter()
            .map(|(i, _, section)| (sessions[i].title.clone(), section))
            .collect()
    }

    #[test]
    fn scope_lists_by_status_section_then_recency() {
        // 要确认的排在前面，其次是完成的，运行中的排在两者后面。
        let sessions = vec![
            session("完成", Status::Done, 9_000, Source::Codex),
            session("空闲", Status::Idle, 8_000, Source::Claude),
            session("在跑", Status::Running, 1_000, Source::Claude),
            session(
                "等你旧",
                Status::NeedsYou("等你回答".into()),
                10,
                Source::Codex,
            ),
            session(
                "等你新",
                Status::NeedsYou("等你".into()),
                20,
                Source::Claude,
            ),
            session(
                "中断",
                Status::Interrupted("interrupted".into()),
                9_500,
                Source::Codex,
            ),
        ];
        assert_eq!(
            scoped(&sessions, "", None),
            [
                ("等你新".to_string(), waiting_section()),
                ("等你旧".to_string(), waiting_section()),
                ("完成".to_string(), "最近完成"),
                ("空闲".to_string(), "最近完成"),
                ("在跑".to_string(), "进行中"),
                ("中断".to_string(), "其他"),
            ]
        );
    }

    #[test]
    fn scope_filter_keeps_only_that_source() {
        // 分段筛选选了 Codex，就不能混进 Claude 会话；「全部」和未知筛选值都不过滤。
        let sessions = vec![
            session("c 修复", Status::Idle, 1, Source::Claude),
            session("x 修复", Status::Running, 2, Source::Codex),
        ];
        let titles = |query: &str, filter: &str| -> Vec<String> {
            scoped(&sessions, query, Source::from_filter(filter))
                .into_iter()
                .map(|(t, _)| t)
                .collect()
        };
        assert_eq!(titles("", "Codex"), ["x 修复"]);
        assert_eq!(titles("", "Claude"), ["c 修复"]);
        assert_eq!(titles("", "全部").len(), 2);
        assert_eq!(titles("修复", "Codex"), ["x 修复"]);
        // 范围里打的是筛选词，不是全局关键词：「codex」不该把所有 Codex 会话列出来。
        assert!(titles("codex", "全部").is_empty());
    }

    #[test]
    fn home_suggestions_only_waiting_sessions() {
        // 首页「等你处理」和菜单栏都读这一组：混进运行中 / 已完成的会话，「等你」的提醒就失去意义。
        let sessions = vec![
            session("在跑", Status::Running, 9_000, Source::Claude),
            session(
                "等你",
                Status::NeedsYou("等你回答".into()),
                10,
                Source::Codex,
            ),
            session("完成", Status::Done, 5_000, Source::Codex),
        ];
        let waiting = waiting(&sessions);
        assert_eq!(waiting.len(), 1);
        let (i, score) = waiting[0];
        assert_eq!(sessions[i].title, "等你");
        // engine 约定「等你处理」≥ 900，排在最近使用（850）之前；但不能到 1000 压过应用精确命中。
        assert!((900..1000).contains(&score), "{score}");
    }

    #[test]
    fn origin_follows_jump_kind() {
        // 「来源」告诉用户回车会切到哪个 App：桌面版走深链，终端会话找终端窗口。
        let mut s = session("t", Status::Idle, 0, Source::Claude);
        assert_eq!(s.origin(), "Claude Code 终端");
        s.jump = Jump::DeepLink("claude://x".into());
        assert_eq!(s.origin(), "Claude 桌面版");
        s.source = Source::Codex;
        assert_eq!(s.origin(), "Codex 桌面版");
    }

    #[test]
    fn english_ui_translates_status_and_age_at_display_time() {
        // 「等你」标签在扫描时以中文存进会话（缓存跨语言切换），所以必须在显示时翻译；
        // 否则切到英文后，列表里最显眼的状态标签还是中文。
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let permission = Status::NeedsYou("等你确认权限".into());
            assert_eq!(permission.label(), "Waiting for permission");
            assert_eq!(permission.tag().0, "Needs permission");
            let answer = Status::NeedsYou("等你回答".into());
            assert_eq!(answer.label(), "Waiting for your answer");
            assert_eq!(answer.tag().0, "Needs answer");
            let other = Status::NeedsYou("等你：input".into());
            assert_eq!(other.label(), "Waiting for input");
            assert_eq!(other.tag(), ("Needs you".into(), Tone::Waiting));
            assert_eq!(Status::Running.tag(), ("Running".into(), Tone::Running));
            assert_eq!(
                Status::Interrupted("replaced".into()).label(),
                "Interrupted (replaced)"
            );
            assert_eq!(ago(60_000, 0), "1 min ago");
            assert_eq!(ago(30, 0), "Just now");
            assert_eq!(ago(86_400_000, 0), "1 day ago");
            assert_eq!(ago(3 * 86_400_000, 0), "3 days ago");
            // 分组名随语言变，但仍按状态分组。
            assert_eq!(Status::Done.section().0, "Recently Completed");
            assert_eq!(
                Status::NeedsYou("等你".into()).section().0,
                waiting_section()
            );
        });
    }

    #[test]
    fn one_line_flattens_and_truncates() {
        assert_eq!(one_line("a\n\n b  c", 10), "a b c");
        assert_eq!(one_line("一二三四五", 3), "一二三…");
    }
}
