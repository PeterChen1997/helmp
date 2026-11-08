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

use crate::paths;
use crate::provider::{Item, Provider};

use self::state::Session;

/// 任何 jsonl 只读尾部这么多字节（本机 Claude 1,514 个 jsonl / 7.2 GB，绝不能全量读）。
const TAIL_BYTES: u64 = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// 一轮扫描里遇到的问题。每条只在首次出现时打印到 stderr，避免每秒刷屏。
type Warnings = Vec<String>;

pub struct Agents {
    sessions: Arc<RwLock<Vec<Session>>>,
    home: Option<PathBuf>,
}

impl Default for Agents {
    fn default() -> Self {
        Self {
            sessions: Arc::default(),
            home: paths::home_dir().ok(),
        }
    }
}

struct Poller {
    claude: claude::ClaudeScanner,
    codex: codex::CodexScanner,
    reported: HashSet<String>,
}

impl Poller {
    fn refresh(&mut self, sessions: &RwLock<Vec<Session>>) {
        let mut warnings = Warnings::new();
        let mut found = self.claude.scan(&mut warnings);
        found.extend(self.codex.scan(&mut warnings));
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

    fn query(&self, query: &str) -> Vec<Item> {
        let sessions = self.sessions.read().expect("agents 缓存锁中毒");
        let home = self.home.as_deref();
        let now = now_ms();
        state::rank(&sessions, query, home)
            .into_iter()
            .map(|(i, score)| {
                let session = &sessions[i];
                Item {
                    id: format!("agents:{}", session.domain_id()),
                    title: session.title.clone(),
                    subtitle: session.subtitle(home, now),
                    badge: session.source.badge().into(),
                    score,
                    action: jump::action(session.jump.clone()),
                    ..Item::blank()
                }
            })
            .collect()
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
