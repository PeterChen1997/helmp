//! agents：Claude Code / Codex 会话切换器。
//!
//! 后台线程每秒轮询一次（Claude 注册表目录 + Codex 写锁目录，都很小），结果写进
//! `Arc<RwLock<Vec<Session>>>`；`query` 只读这份缓存。只读用户的 `~/.claude`、`~/.codex`，不写不删、不装 hooks。

mod claude;
mod codex;
mod jump;
mod process;
mod state;

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use anyhow::Result;

use crate::engine::waiting_section;
use crate::i18n::tr;
use crate::paths;
use crate::provider::{
    Accessory, Action, Block, Detail, Icon, Item, NamedAction, Provider, Scope, Tint,
};

use self::state::{Session, Source};

/// 任何 jsonl 只读尾部这么多字节（本机 Claude 1,514 个 jsonl / 7.2 GB，绝不能全量读）。
const TAIL_BYTES: u64 = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// 全局搜索里会话的分组名（设计稿 Results）。范围视图里按状态分组，首页用 `waiting_section()`。
fn global_section() -> &'static str {
    tr("Agent 会话", "Agent Sessions")
}
const SCOPE_ID: &str = "agents";

/// 一轮扫描里遇到的问题。每条只在首次出现时打印到 stderr，避免每秒刷屏。
type Warnings = Vec<String>;

pub struct Agents {
    sessions: Arc<RwLock<Vec<Session>>>,
    home: Option<PathBuf>,
    claude_icon: Icon,
    codex_icon: Icon,
}

impl Default for Agents {
    fn default() -> Self {
        let home = paths::home_dir().ok();
        Self {
            claude_icon: app_icon("Claude.app", home.as_deref()),
            // Codex 桌面端集成在 ChatGPT.app 里，`codex://` 深链也由它处理。
            codex_icon: app_icon("ChatGPT.app", home.as_deref()),
            sessions: Arc::default(),
            home,
        }
    }
}

/// 来源 App 的图标；没装这个 App 时用紫色机器人符号，避免列表里出现空白图标。
fn app_icon(bundle: &str, home: Option<&Path>) -> Icon {
    let candidates = std::iter::once(PathBuf::from("/Applications"))
        .chain(home.map(|home| home.join("Applications")))
        .map(|dir| dir.join(bundle));
    candidates
        .into_iter()
        .find(|path| path.is_dir())
        .map(Icon::File)
        .unwrap_or(Icon::Symbol {
            name: "bot",
            tint: Tint::Purple,
        })
}

struct Poller {
    claude: claude::ClaudeScanner,
    codex: codex::CodexScanner,
    reported: HashSet<String>,
}

impl Poller {
    fn refresh(&mut self, sessions: &RwLock<Vec<Session>>) {
        let mut warnings = Warnings::new();
        // 设置 › Agent 里的来源开关（未配置视为开启）。
        let mut found = Vec::new();
        if crate::config::extension_enabled("agents.claude") {
            found.extend(self.claude.scan(&mut warnings));
        }
        if crate::config::extension_enabled("agents.codex") {
            found.extend(self.codex.scan(&mut warnings));
        }
        *sessions.write().expect("agents 缓存锁中毒") = found;

        let current: HashSet<String> = warnings.into_iter().collect();
        for warning in current.difference(&self.reported) {
            eprintln!("helmp: agents: {warning}");
        }
        self.reported = current;
    }
}

impl Provider for Agents {
    fn id(&self) -> &'static str {
        "agents"
    }

    fn start(&self) -> Result<()> {
        let home = paths::home_dir()?;
        let mut poller = Poller {
            claude: claude::ClaudeScanner::new(&home),
            codex: codex::CodexScanner::new(&home),
            reported: HashSet::new(),
        };
        // 首轮同步完成，CLI 的 `query` 紧接着就能拿到结果。
        poller.refresh(&self.sessions);
        let sessions = Arc::clone(&self.sessions);
        std::thread::Builder::new()
            .name("helmp-agents-poll".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(POLL_INTERVAL);
                    poller.refresh(&sessions);
                }
            })?;
        Ok(())
    }

    /// 全局搜索：标题 / 项目 / 分支匹配，或关键词（agent / 会话 / claude / codex）列出全部。
    fn query(&self, query: &str) -> Vec<Item> {
        let sessions = self.sessions.read().expect("agents 缓存锁中毒");
        let now = now_ms();
        state::rank(&sessions, query, self.home.as_deref())
            .into_iter()
            .map(|(i, score)| self.item(&sessions[i], score, global_section(), now))
            .collect()
    }

    fn scope(&self) -> Option<Scope> {
        Some(Scope {
            id: SCOPE_ID.into(),
            label: tr("Agent 会话", "Agent Sessions").into(),
            tint: Tint::Purple,
            prefixes: vec!["ag ".into(), "agent ".into()],
            placeholder: tr(
                "按标题、项目或分支筛选",
                "Filter by title, project, or branch",
            )
            .into(),
            // 筛选名会原样传回 `query_scoped`：只认 Claude / Codex（两种语言同名），其余（全部 / All）都不过滤。
            filters: vec![tr("全部", "All").into(), "Claude".into(), "Codex".into()],
        })
    }

    fn query_scoped(&self, query: &str, filter: &str) -> Vec<Item> {
        let sessions = self.sessions.read().expect("agents 缓存锁中毒");
        let now = now_ms();
        let only = Source::from_filter(filter);
        state::rank_scoped(&sessions, query, only, self.home.as_deref())
            .into_iter()
            .map(|(i, score, section)| self.item(&sessions[i], score, section, now))
            .collect()
    }

    /// 首页「等你处理」：只放等你的会话。菜单栏也读这一组。
    fn suggestions(&self) -> Vec<Item> {
        let sessions = self.sessions.read().expect("agents 缓存锁中毒");
        let now = now_ms();
        state::waiting(&sessions)
            .into_iter()
            .map(|(i, score)| self.item(&sessions[i], score, waiting_section(), now))
            .collect()
    }
}

impl Agents {
    fn item(&self, session: &Session, score: u32, badge: &str, now: u64) -> Item {
        let home = self.home.as_deref();
        let icon = match session.source {
            Source::Claude => self.claude_icon.clone(),
            Source::Codex => self.codex_icon.clone(),
        };
        let (tag, tone) = session.status.tag();
        Item {
            id: format!("agents:{}", session.domain_id()),
            title: session.title.clone(),
            subtitle: session.subtitle(home, now),
            badge: badge.into(),
            score,
            action: jump::action(session.jump.clone()),
            action_title: tr("跳转到会话", "Switch to Session").into(),
            accessories: vec![Accessory::Tag { text: tag, tone }],
            actions: actions(session),
            detail: Some(detail(session, icon.clone(), home, now)),
            icon,
            ..Item::blank()
        }
    }
}

/// ⌘K 次级操作。活着的会话绝不提供「恢复会话」：`--resume` 会开出第二个进程（见 jump.rs）。
fn actions(session: &Session) -> Vec<NamedAction> {
    let mut actions = Vec::new();
    if let Some(cwd) = session.cwd.as_deref().and_then(Path::to_str) {
        actions.push(NamedAction::new(
            tr("在访达中显示项目", "Show Project in Finder"),
            Some("⌘↵"),
            Action::reveal(cwd.to_string()),
        ));
        actions.push(NamedAction::new(
            tr("复制项目路径", "Copy Project Path"),
            Some("⌘⇧C"),
            Action::copy(cwd.to_string()),
        ));
    }
    // 会话 ID 用于 `claude --resume <id>` / `codex resume <id>`，或者在日志里查这个会话。
    actions.push(NamedAction::new(
        tr("复制会话 ID", "Copy Session ID"),
        None,
        Action::copy(session.session_id.clone()),
    ));
    actions
}

/// 详情：标题、状态、最后一条消息（等你时是在等什么）、项目 / 分支 / 来源 / 最后活动。
fn detail(session: &Session, icon: Icon, home: Option<&Path>, now: u64) -> Detail {
    let mut metadata = Vec::new();
    if let Some(cwd) = &session.cwd {
        metadata.push((tr("项目", "Project").into(), state::display_path(cwd, home)));
    }
    if let Some(branch) = &session.branch {
        metadata.push((tr("分支", "Branch").into(), branch.clone()));
    }
    metadata.push((tr("来源", "Source").into(), session.origin().into()));
    metadata.push((
        tr("最后活动", "Last Activity").into(),
        state::ago(now, session.last_activity_ms),
    ));
    Detail {
        actions: Vec::new(),
        icon,
        title: session.title.clone(),
        tag: Some((session.status.label(), session.status.tone())),
        blocks: session
            .last_message
            .iter()
            .map(|text| Block::Quote {
                label: tr("最后一条消息", "Last Message").into(),
                text: text.clone(),
            })
            .collect(),
        metadata,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn mtime_ms(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(
        modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_millis() as u64,
    )
}

/// 读文件最后 `max` 字节。返回 `(字节, 是否被截断)`。
fn read_tail(path: &Path, max: u64) -> std::io::Result<(Vec<u8>, bool)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut bytes)?;
    Ok((bytes, start > 0))
}

/// 按行切分；被截断时第一段是半行，丢掉。
fn tail_lines(bytes: &[u8], truncated: bool) -> impl Iterator<Item = &[u8]> {
    bytes
        .split(|&b| b == b'\n')
        .skip(usize::from(truncated))
        .filter(|line| !line.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Block, Tone};
    use state::{Jump, Status};

    fn session(title: &str, source: Source, status: Status) -> Session {
        Session {
            source,
            session_id: format!("id-{title}"),
            title: title.into(),
            status,
            cwd: Some(PathBuf::from("/Users/me/code_hub/finder")),
            branch: Some("main".into()),
            last_activity_ms: now_ms() - 10 * 60_000,
            last_message: Some("需要你确认：是否允许运行 npm test？".into()),
            jump: Jump::DeepLink("claude://claude.ai/epitaxy/x".into()),
        }
    }

    fn provider(sessions: Vec<Session>) -> Agents {
        Agents {
            sessions: Arc::new(RwLock::new(sessions)),
            home: Some(PathBuf::from("/Users/me")),
            claude_icon: Icon::File(PathBuf::from("/Applications/Claude.app")),
            codex_icon: Icon::File(PathBuf::from("/Applications/ChatGPT.app")),
        }
    }

    fn fixture() -> Agents {
        provider(vec![
            session(
                "Finder daily radar",
                Source::Claude,
                Status::NeedsYou("等你确认权限".into()),
            ),
            session("Helmp 启动器开发", Source::Claude, Status::Running),
            session("VPC 接入排查", Source::Codex, Status::Done),
        ])
    }

    #[test]
    fn item_matches_agents_design() {
        // 设计稿 Agents / Home：来源 App 图标、彩色状态标签、副标题「~/项目 · 分支 · 10 分钟前」、回车「跳转到会话」。
        let items = fixture().query("radar");
        let item = items.first().unwrap();
        assert_eq!(
            item.icon,
            Icon::File(PathBuf::from("/Applications/Claude.app"))
        );
        assert_eq!(
            item.accessories,
            [Accessory::Tag {
                text: "等你确认".into(),
                tone: Tone::Waiting
            }]
        );
        assert_eq!(item.subtitle, "~/code_hub/finder · main · 10 分钟前");
        assert_eq!(item.action_title, "跳转到会话");
        assert_eq!(item.badge, global_section());
        let shortcuts: Vec<_> = item
            .actions
            .iter()
            .map(|a| (a.title.as_str(), a.shortcut.as_deref()))
            .collect();
        assert_eq!(
            shortcuts,
            [
                ("在访达中显示项目", Some("⌘↵")),
                ("复制项目路径", Some("⌘⇧C")),
                ("复制会话 ID", None),
            ]
        );
    }

    #[test]
    fn detail_has_status_quote_and_metadata() {
        // 详情要回答「在等我什么、在哪个项目、回车会切到哪个 App」，不切过去就能判断要不要马上处理。
        let items = fixture().query("radar");
        let detail = items.first().unwrap().detail.clone().unwrap();
        assert_eq!(detail.title, "Finder daily radar");
        assert_eq!(detail.tag, Some(("等你确认权限".into(), Tone::Waiting)));
        assert_eq!(
            detail.blocks,
            [Block::Quote {
                label: "最后一条消息".into(),
                text: "需要你确认：是否允许运行 npm test？".into()
            }]
        );
        assert_eq!(
            detail.metadata,
            [
                ("项目".to_string(), "~/code_hub/finder".to_string()),
                ("分支".to_string(), "main".to_string()),
                ("来源".to_string(), "Claude 桌面版".to_string()),
                ("最后活动".to_string(), "10 分钟前".to_string()),
            ]
        );
        // 拿不到最后一条消息时整块省略，而不是显示一个空引用框。
        let mut quiet = session("安静", Source::Codex, Status::Idle);
        quiet.last_message = None;
        quiet.cwd = None;
        quiet.branch = None;
        let items = provider(vec![quiet]).query("安静");
        let detail = items.first().unwrap().detail.clone().unwrap();
        assert!(detail.blocks.is_empty());
        assert_eq!(
            detail.metadata.first().map(|(k, _)| k.as_str()),
            Some("来源")
        );
    }

    #[test]
    fn scope_declares_prefixes_and_filters() {
        // 「ag 」进入范围、分段筛选 全部 / Claude / Codex 是设计稿约定；第一个筛选是默认值。
        let scope = fixture().scope().unwrap();
        assert_eq!(scope.id, "agents");
        assert_eq!(scope.label, "Agent 会话");
        assert_eq!(scope.tint, Tint::Purple);
        assert_eq!(scope.prefixes, ["ag ", "agent "]);
        assert_eq!(scope.filters, ["全部", "Claude", "Codex"]);
    }

    #[test]
    fn scoped_query_filters_by_source_and_groups_by_status() {
        let agents = fixture();
        let codex: Vec<_> = agents
            .query_scoped("", "Codex")
            .into_iter()
            .map(|i| (i.title, i.badge))
            .collect();
        assert_eq!(
            codex,
            [("VPC 接入排查".to_string(), "最近完成".to_string())]
        );
        let all = agents.query_scoped("", "全部");
        assert_eq!(all.len(), 3);
        let waiting = all
            .iter()
            .find(|i| i.title == "Finder daily radar")
            .unwrap();
        assert_eq!(waiting.badge, waiting_section());
    }

    #[test]
    fn suggestions_are_waiting_sessions_under_waiting_section() {
        // 菜单栏按 waiting_section() 这个分组名读取；名字或内容不对，菜单栏的「等你处理」就是空的或错的。
        let items = fixture().suggestions();
        let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["Finder daily radar"]);
        assert!(
            items
                .iter()
                .all(|i| i.badge == waiting_section() && i.score >= 900)
        );
    }

    #[test]
    fn global_keyword_still_lists_sessions() {
        // 全局输入 agent / 会话 / claude / codex 列出会话是已有行为，升级 v2 不能丢。
        let agents = fixture();
        assert_eq!(agents.query("agent").len(), 3);
        assert_eq!(agents.query("会话").len(), 3);
        assert_eq!(agents.query("codex").len(), 1);
    }

    #[test]
    fn english_ui_translates_labels_but_keeps_keywords_and_filters() {
        // 英文界面下用户看到的分组、状态、操作、详情都应是英文，否则切了语言还是满屏中文。
        // 范围筛选名会原样传回，进入范围后才切语言时传回的可能是中文「全部」，两种都必须照常列出；
        // 全局关键词（会话 / agent）属于搜索逻辑，跟界面语言无关。
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let agents = fixture();
            let items = agents.query("radar");
            let item = items.first().unwrap();
            assert_eq!(item.badge, "Agent Sessions");
            assert_eq!(item.action_title, "Switch to Session");
            assert_eq!(
                item.accessories,
                [Accessory::Tag {
                    text: "Needs permission".into(),
                    tone: Tone::Waiting
                }]
            );
            assert_eq!(item.subtitle, "~/code_hub/finder · main · 10 min ago");
            let titles: Vec<_> = item.actions.iter().map(|a| a.title.as_str()).collect();
            assert_eq!(
                titles,
                [
                    "Show Project in Finder",
                    "Copy Project Path",
                    "Copy Session ID"
                ]
            );
            let detail = item.detail.clone().unwrap();
            assert_eq!(
                detail.tag,
                Some(("Waiting for permission".into(), Tone::Waiting))
            );
            let keys: Vec<_> = detail.metadata.iter().map(|(k, _)| k.as_str()).collect();
            assert_eq!(keys, ["Project", "Branch", "Source", "Last Activity"]);
            assert_eq!(detail.metadata[2].1, "Claude Desktop");

            let scope = agents.scope().unwrap();
            assert_eq!(scope.label, "Agent Sessions");
            assert_eq!(scope.filters, ["All", "Claude", "Codex"]);
            assert_eq!(agents.query_scoped("", "All").len(), 3);
            assert_eq!(agents.query_scoped("", "全部").len(), 3);
            let codex: Vec<_> = agents
                .query_scoped("", "Codex")
                .into_iter()
                .map(|i| i.badge)
                .collect();
            assert_eq!(codex, ["Recently Completed"]);

            assert_eq!(agents.query("会话").len(), 3);
            assert_eq!(agents.query("sessions").len(), 3);
            // 菜单栏按 waiting_section() 读首页等待会话：英文下也必须是同一个名字。
            assert!(
                agents
                    .suggestions()
                    .iter()
                    .all(|i| i.badge == waiting_section())
            );
        });
    }

    /// 本机真实会话的列表与详情（只读 ~/.claude、~/.codex）：
    /// `cargo test agents::tests::dump_real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_real_sessions() {
        let agents = Agents::default();
        agents.start().unwrap();
        for item in agents.query_scoped("", "全部") {
            let detail = item.detail.unwrap();
            println!(
                "[{}] {} | {} | {:?} | {:?}",
                item.badge, item.title, item.subtitle, item.accessories, item.icon
            );
            println!("    tag={:?}", detail.tag);
            for block in &detail.blocks {
                println!("    {block:?}");
            }
            println!("    {:?}", detail.metadata);
        }
    }

    #[test]
    fn tail_read_drops_partial_first_line() {
        // 从文件中间开始读，第一行必然是残的；拿残行去解析会把别的字段误认成事件。
        let dir = std::env::temp_dir().join(format!("helmp-tail-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        std::fs::write(&path, b"{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n").unwrap();

        let (bytes, truncated) = read_tail(&path, 12).unwrap();
        assert!(truncated);
        let lines: Vec<_> = tail_lines(&bytes, truncated).collect();
        assert_eq!(lines, [b"{\"c\":3}".as_slice()]);

        let (bytes, truncated) = read_tail(&path, 1024).unwrap();
        assert!(!truncated);
        assert_eq!(tail_lines(&bytes, truncated).count(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
