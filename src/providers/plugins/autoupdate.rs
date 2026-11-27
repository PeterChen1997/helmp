//! 插件自动更新：检查从插件中心 / 自定义插件源安装的插件有没有新版本，有就装上。
//!
//! - 定时挂在 App 自动更新的同一个轮询任务上（`updates::start`）：启动 20 秒后一次，之后每 24 小时；
//!   有插件失败或正忙时 1 小时后再试。开关是「设置 › 插件」的「自动更新插件」（`general.auto_update_plugins`）。
//! - 每个源只取一次索引，按安装记录里的源比较版本（带那个源的令牌）。下载、校验、原子替换都走
//!   `center::install_from`：权限没变、也不运行代码的保持启用；权限变了，或会运行代码的插件（脚本、定时任务、
//!   MCP 服务器）换了文件，就成「权限或内容有变化，待确认」，等用户在设置里确认。
//! - 失败不弹提示，旧版本原样保留，下次再试；插件正在运行脚本、或它带的 MCP 服务器在跑时跳过，下次再试。
//! - 每次的结果记在 `<data_dir>/plugin-updates.json`，设置页显示上次检查时间与结果。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, anyhow};
use serde::{Deserialize, Serialize};

use super::center::{self, Client};
use super::registry::{self, Source, State};
use crate::config::RegistryInstall;
use crate::i18n::{tr, trf};

/// 谁发起的检查（埋点用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// 每日定时。
    Scheduled,
    /// 用户点了「现在检查」或运行了 `helmp plugins update`。
    Manual,
}

impl Trigger {
    fn as_str(self) -> &'static str {
        match self {
            Trigger::Scheduled => "scheduled",
            Trigger::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Updated {
    pub id: String,
    pub from: String,
    pub to: String,
    /// 新版本要的权限和用户确认过的不同：已停用，等用户确认。
    pub needs_consent: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Failure {
    pub id: String,
    pub error: String,
}

/// 一次检查的结果。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Report {
    /// 检查时间（Unix 秒）。
    pub checked_at: u64,
    /// 检查了多少个插件。
    pub checked: usize,
    pub updated: Vec<Updated>,
    /// 正在运行、这次没替换的插件。
    pub busy: Vec<String>,
    pub failed: Vec<Failure>,
}

impl Report {
    /// 有插件失败或正忙：1 小时后再试，而不是等一天。
    pub fn retry_soon(&self) -> bool {
        !self.failed.is_empty() || !self.busy.is_empty()
    }

    fn fail(&mut self, id: &str, err: impl std::fmt::Display) {
        self.failed.push(Failure {
            id: id.to_string(),
            error: err.to_string().replace('\n', "; "),
        });
    }

    /// 设置页、命令行用的一句话结果。
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.updated.is_empty() && self.failed.is_empty() && self.busy.is_empty() {
            parts.push(if self.checked == 0 {
                tr("没有从插件中心或插件源安装的插件", "No plugins installed from the plugin center or a source").to_string()
            } else {
                tr("插件都是最新的", "All plugins are up to date").to_string()
            });
        }
        if !self.updated.is_empty() {
            let list: Vec<String> = self
                .updated
                .iter()
                .map(|u| format!("{} {} → {}", u.id, u.from, u.to))
                .collect();
            parts.push(trf!("已更新 {}", "Updated {}", list.join(tr("、", ", "))));
            let consent = self.updated.iter().filter(|u| u.needs_consent).count();
            if consent > 0 {
                parts.push(trf!(
                    "{consent} 个权限或内容有变化，待确认",
                    "{consent} need a permission review"
                ));
            }
        }
        if !self.busy.is_empty() {
            parts.push(trf!(
                "{} 正在运行，稍后再更新",
                "{} running, will update later",
                self.busy.join(tr("、", ", "))
            ));
        }
        if let Some(first) = self.failed.first() {
            parts.push(trf!(
                "{} 个失败（{}：{}），旧版本保持不变",
                "{} failed ({}: {}); the old versions are kept",
                self.failed.len(),
                first.id,
                first.error
            ));
        }
        parts.join(tr("；", "; "))
    }
}

/// 插件正在用（脚本在跑，或它带的 MCP 服务器在运行）：这时不替换它的目录。
fn busy(id: &str) -> bool {
    super::exec::running::is_running(id) || crate::providers::mcp::manager::plugin_running(id)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// 检查并更新（阻塞，联网）。`only`：只看这些 id（命令行 `helmp plugins update <id>`），None = 全部。
pub fn check(only: Option<&[String]>) -> Report {
    let mut report = Report {
        checked_at: now_secs(),
        ..Report::default()
    };
    let config = crate::config::get();
    let installed: Vec<(String, RegistryInstall)> = registry::load_all()
        .into_iter()
        .filter(|entry| entry.source == Source::User)
        .filter(|entry| only.is_none_or(|ids| ids.contains(&entry.id)))
        .filter_map(|entry| {
            let record = center::install_record(&entry.id, &config, entry.dir.as_deref())?;
            Some((entry.id, record))
        })
        .collect();
    if let Some(ids) = only {
        for id in ids.iter().filter(|id| !installed.iter().any(|(i, _)| i == *id)) {
            report.fail(
                id,
                tr(
                    "不是从插件中心或插件源安装的，没有更新可查",
                    "wasn't installed from the plugin center or a plugin source, so there's nothing to update",
                ),
            );
        }
    }
    let registries = center::registries(&config);
    for (id, record) in &installed {
        if !registries.iter().any(|registry| registry.owns(record)) {
            report.checked += 1;
            report.fail(
                id,
                trf!(
                    "安装它的插件源（{}）已经删掉了",
                    "the plugin source it came from ({}) has been removed",
                    super::sources::host(&record.registry)
                ),
            );
        }
    }
    for registry in registries {
        let mine: Vec<&(String, RegistryInstall)> =
            installed.iter().filter(|(_, record)| registry.owns(record)).collect();
        if mine.is_empty() {
            continue;
        }
        report.checked += mine.len();
        let fetched = Client::new(registry).and_then(|client| {
            let index = client.index()?;
            Ok((client, index))
        });
        let (client, index) = match fetched {
            Ok(fetched) => fetched,
            Err(err) => {
                for (id, _) in mine {
                    report.fail(id, format!("{err:#}"));
                }
                continue;
            }
        };
        for (id, record) in mine {
            let Some(listing) = index.get(id) else {
                report.fail(
                    id,
                    tr("源里已经没有这个插件了", "the source no longer lists it"),
                );
                continue;
            };
            if !center::newer(&listing.version, &record.version) {
                continue;
            }
            if busy(id) {
                report.busy.push(id.clone());
                continue;
            }
            match center::install_from(id, &client, &index, Some(record.version.clone())) {
                Ok(done) if !done.up_to_date => {
                    let config = crate::config::get();
                    let needs_consent = registry::current()
                        .iter()
                        .find(|entry| entry.id == *id)
                        .is_some_and(|entry| entry.state(&config) == State::NeedsConsent);
                    report.updated.push(Updated {
                        id: id.clone(),
                        from: done.previous.unwrap_or_default(),
                        to: done.version,
                        needs_consent,
                    });
                }
                Ok(_) => {}
                Err(err) => report.fail(id, format!("{err:#}")),
            }
        }
    }
    report
}

// ---------------------------------------------------------------- 状态

static IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static LAST: Mutex<Option<Report>> = Mutex::new(None);

const STATE_FILE: &str = "plugin-updates.json";

/// 正在检查。
pub fn in_flight() -> bool {
    IN_FLIGHT.load(Ordering::SeqCst)
}

/// 状态每变一次加一：设置页据此重绘。
pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

/// 上一次检查的结果（本次运行没查过时读文件，跨重启保留）。
pub fn last() -> Option<Report> {
    let mut last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if last.is_none() {
        *last = crate::paths::data_dir()
            .ok()
            .and_then(|dir| std::fs::read_to_string(dir.join(STATE_FILE)).ok())
            .and_then(|text| serde_json::from_str(&text).ok());
    }
    last.clone()
}

fn save(report: &Report) -> Result<()> {
    let path = crate::paths::data_dir()?.join(STATE_FILE);
    std::fs::write(&path, serde_json::to_string_pretty(report)?)
        .with_context(|| trf!("写入 {}", "Couldn't write {}", path.display()))
}

/// 检查一次并记下结果、发埋点（阻塞）。已经有一次在进行时返回 None。
pub fn run(trigger: Trigger, only: Option<&[String]>) -> Option<Report> {
    if IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return None;
    }
    GENERATION.fetch_add(1, Ordering::SeqCst);
    let report = finish(trigger, check(only));
    IN_FLIGHT.store(false, Ordering::SeqCst);
    GENERATION.fetch_add(1, Ordering::SeqCst);
    Some(report)
}

fn finish(trigger: Trigger, report: Report) -> Report {
    if let Err(err) = save(&report) {
        eprintln!("helmp: {err:#}");
        crate::telemetry::report("plugins.autoupdate", &anyhow!("couldn't save the update state"));
    }
    for failure in &report.failed {
        eprintln!("helmp: 插件自动更新：{} 失败：{}", failure.id, failure.error);
    }
    crate::telemetry::plugin_autoupdate(
        trigger.as_str(),
        !report.updated.is_empty(),
        report.updated.iter().any(|u| u.needs_consent),
        !report.failed.is_empty(),
    );
    *LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(report.clone());
    report
}

/// 在后台线程检查（定时、设置页「现在检查」）。已经在检查时什么也不做，返回 false。
pub fn start_in_background(trigger: Trigger) -> bool {
    if IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return false;
    }
    GENERATION.fetch_add(1, Ordering::SeqCst);
    std::thread::spawn(move || {
        finish(trigger, check(None));
        IN_FLIGHT.store(false, Ordering::SeqCst);
        GENERATION.fetch_add(1, Ordering::SeqCst);
    });
    true
}

// ---------------------------------------------------------------- 定时

/// 每日检查的节奏（与 App 自动更新相同）：启动后 `first` 一次，成功后隔 `interval`，有失败或正忙隔 `retry`。
/// 开关关着时到点也不查；打开后立即补一次。
#[derive(Debug)]
pub struct Schedule {
    next: Instant,
    /// 已经发起一次，等它结束后安排下一次。
    waiting: bool,
    interval: Duration,
    retry: Duration,
}

impl Schedule {
    pub fn new(started: Instant, first: Duration, interval: Duration, retry: Duration) -> Self {
        Self {
            next: started + first,
            waiting: false,
            interval,
            retry,
        }
    }

    /// 轮询时调用。返回 true = 现在发起一次检查。`in_flight`：有一次检查在进行（定时的或手动的）；
    /// `retry_soon`：最近一次检查有失败或正忙的插件。
    pub fn tick(&mut self, now: Instant, enabled: bool, in_flight: bool, retry_soon: bool) -> bool {
        if self.waiting {
            if !in_flight {
                self.waiting = false;
                self.next = now + if retry_soon { self.retry } else { self.interval };
            }
            return false;
        }
        if !enabled || now < self.next {
            return false;
        }
        // 用户刚好在手动检查：等它结束后照常排下一次。
        self.waiting = true;
        !in_flight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: Duration = Duration::from_secs(20);
    const DAY: Duration = Duration::from_secs(24 * 3600);
    const HOUR: Duration = Duration::from_secs(3600);

    #[test]
    fn schedule_checks_daily_and_retries_failures_within_the_hour() {
        // 和 App 更新一样的节奏：启动 20 秒后一次，成功后隔一天，失败隔一小时，不能一直狂查。
        let t0 = Instant::now();
        let mut s = Schedule::new(t0, FIRST, DAY, HOUR);
        assert!(!s.tick(t0, true, false, false), "启动时不查，免得和索引抢资源");
        assert!(s.tick(t0 + FIRST, true, false, false));
        assert!(!s.tick(t0 + FIRST, true, true, false), "进行中不再发起");
        let done = t0 + FIRST + Duration::from_secs(5);
        assert!(!s.tick(done, true, false, true), "结束这一拍只排下一次");
        assert!(!s.tick(done + HOUR - Duration::from_secs(1), true, false, true));
        assert!(s.tick(done + HOUR, true, false, true), "失败后一小时重试");
        let done = done + HOUR + Duration::from_secs(5);
        assert!(!s.tick(done, true, false, false));
        assert!(!s.tick(done + HOUR, true, false, false), "成功后不会一小时就再查");
        assert!(s.tick(done + DAY, true, false, false));
    }

    #[test]
    fn turning_the_switch_off_stops_scheduled_checks() {
        // 用户关掉「自动更新插件」后，到点也不能再联网检查；重新打开立即补一次。
        let t0 = Instant::now();
        let mut s = Schedule::new(t0, FIRST, DAY, HOUR);
        for hours in 0..72 {
            assert!(!s.tick(t0 + FIRST + HOUR * hours, false, false, false));
        }
        assert!(s.tick(t0 + FIRST + HOUR * 72, true, false, false));
    }

    #[test]
    fn a_manual_check_in_progress_is_waited_for_not_doubled() {
        // 到点时用户正好点了「现在检查」：不并发再查一次，等它结束后按结果排下一次。
        let t0 = Instant::now();
        let mut s = Schedule::new(t0, FIRST, DAY, HOUR);
        assert!(!s.tick(t0 + FIRST, true, true, false));
        assert!(!s.tick(t0 + FIRST + HOUR, true, false, false));
        assert!(!s.tick(t0 + FIRST + HOUR * 2, true, false, false));
        assert!(s.tick(t0 + FIRST + HOUR + DAY, true, false, false));
    }

    #[test]
    fn summary_says_what_happened() {
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let mut report = Report {
                checked: 2,
                ..Report::default()
            };
            assert_eq!(report.summary(), "All plugins are up to date");
            report.updated.push(Updated {
                id: "com.example.memo".into(),
                from: "1.0.0".into(),
                to: "1.1.0".into(),
                needs_consent: true,
            });
            report.fail("com.example.team", "HTTP 401");
            let text = report.summary();
            assert!(text.contains("Updated com.example.memo 1.0.0 → 1.1.0"), "{text}");
            assert!(text.contains("1 need a permission review"), "{text}");
            assert!(text.contains("1 failed (com.example.team: HTTP 401)"), "{text}");
            assert!(report.retry_soon());
        });
    }
}
