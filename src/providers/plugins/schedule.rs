//! 插件定时任务：清单里的 `schedules` 按间隔、每天固定时刻或指定日期，在后台运行插件目录里的脚本。
//!
//! - 只在 Helmp 运行时触发。每天 / 指定日期的任务：当天的时刻过了还没跑过就跑一次（启动、合盖唤醒后
//!   补跑当天错过的那一次），一天只跑一次；前几天错过的不补。按间隔的任务：距上次运行满间隔就跑，
//!   离开很久回来也只跑一次。日期与钟面时间按本机时区算（夏令时切换当天照常），日期写法与寄语相同。
//! - 上次运行时间与结果记在 `<data_dir>/plugin-schedules.json`（Helmp 自己的状态，不在插件数据目录里，
//!   插件更新、卸载重装都不影响）。
//! - 执行沿用脚本动作的约束（`exec::run_scheduled`）：按 argv 传参不经 shell、限时、超时杀整个进程组、
//!   运行期间登记为忙（自动更新跳过它）。同时最多跑 `MAX_CONCURRENT` 个，同一个任务不会叠着跑。
//! - 插件被停用、卸载、权限待确认，或用户在设置里单独停用某个任务时，正在跑的脚本立即被结束；
//!   退出 Helmp 时（`stop_all`）同样。
//! - 脚本的 stdout 为空，或是一个 JSON 对象：`{"notify": {"title": …, "body": …, "url": …}}` 请求发系统通知
//!   （插件要声明 `notifications` 权限，见 `notify`）。其他输出算失败并写明原因——日志请写 stderr。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use chrono::{DateTime, Datelike as _, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use super::greeting::DateRule;
use super::manifest::ScheduleSpec;
use super::notify::{self, Notice};
use super::registry::{self, Plugin, State};
use crate::config::Config;
use crate::i18n::{tr, trf};

/// 同时最多运行几个定时脚本（所有插件合计）。
pub const MAX_CONCURRENT: usize = 2;
/// 启动后多久开始第一次检查（不和应用索引抢启动时的资源）。
const FIRST_DELAY: Duration = Duration::from_secs(10);
/// 多久检查一次。合盖唤醒后最晚这么久补跑。
const TICK: Duration = Duration::from_secs(15);
/// 通知标题、正文最长多少个字。
pub const MAX_TITLE_CHARS: usize = 100;
pub const MAX_BODY_CHARS: usize = 500;

// ---------------------------------------------------------------- 触发规则

/// 一个任务什么时候运行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// 每隔这么多分钟。
    Every(u64),
    /// 每天这个时刻。
    Daily(NaiveTime),
    /// 指定日期（每年重复或只那一天）的这个时刻。
    Date(DateRule, NaiveTime),
}

/// `HH:MM`（24 小时制，补零）。
fn parse_time(text: &str) -> Option<NaiveTime> {
    let (hour, minute) = text.split_once(':')?;
    let digits = |part: &str| part.len() == 2 && part.chars().all(|c| c.is_ascii_digit());
    if !digits(hour) || !digits(minute) {
        return None;
    }
    NaiveTime::from_hms_opt(hour.parse().ok()?, minute.parse().ok()?, 0)
}

impl Trigger {
    /// 从清单解析；Err 是给插件作者看的原因。
    pub fn from_spec(spec: &ScheduleSpec) -> Result<Self, String> {
        use super::manifest::{MAX_SCHEDULE_MINUTES, MIN_SCHEDULE_MINUTES};
        let time = match &spec.at {
            Some(at) => Some(parse_time(at).ok_or_else(|| {
                trf!(
                    "at {at:?} 不是 24 小时制的 HH:MM（如 08:30）",
                    "at {at:?} isn't a 24-hour HH:MM time (e.g. 08:30)"
                )
            })?),
            None => None,
        };
        match (spec.every_minutes, &spec.date, time) {
            (Some(minutes), None, None) => {
                if (MIN_SCHEDULE_MINUTES..=MAX_SCHEDULE_MINUTES).contains(&minutes) {
                    Ok(Trigger::Every(minutes))
                } else {
                    Err(trf!(
                        "everyMinutes 要在 {MIN_SCHEDULE_MINUTES} 到 {MAX_SCHEDULE_MINUTES} 之间",
                        "everyMinutes must be between {MIN_SCHEDULE_MINUTES} and {MAX_SCHEDULE_MINUTES}"
                    ))
                }
            }
            (Some(_), _, _) => Err(tr(
                "everyMinutes 不能和 at、date 一起用",
                "everyMinutes can't be combined with at or date",
            )
            .into()),
            (None, Some(date), time) => {
                let rule = DateRule::parse(date).ok_or_else(|| {
                    trf!(
                        "date {date:?} 不是有效日期：写成 MM-DD（每年重复，如 12-25）或 YYYY-MM-DD（只那一天）",
                        "date {date:?} isn't a valid date: use MM-DD (every year, e.g. 12-25) or YYYY-MM-DD (that day only)"
                    )
                })?;
                Ok(Trigger::Date(rule, time.unwrap_or(NaiveTime::MIN)))
            }
            (None, None, Some(time)) => Ok(Trigger::Daily(time)),
            (None, None, None) => Err(tr(
                "要写 everyMinutes、at 或 date 其中之一",
                "needs one of everyMinutes, at or date",
            )
            .into()),
        }
    }

    /// 埋点用的种类。
    pub fn kind(&self) -> &'static str {
        match self {
            Trigger::Every(_) => "interval",
            Trigger::Daily(_) => "daily",
            Trigger::Date(..) => "date",
        }
    }

    /// 设置里显示的一句话。
    pub fn describe(&self) -> String {
        match self {
            Trigger::Every(minutes) if minutes % 60 == 0 => {
                let hours = minutes / 60;
                trf!("每 {hours} 小时", "Every {hours} h")
            }
            Trigger::Every(minutes) => trf!("每 {minutes} 分钟", "Every {minutes} min"),
            Trigger::Daily(time) => {
                let time = time.format("%H:%M");
                trf!("每天 {time}", "Daily at {time}")
            }
            Trigger::Date(DateRule::Yearly { month, day }, time) => {
                let time = time.format("%H:%M");
                trf!(
                    "每年 {month} 月 {day} 日 {time}",
                    "Every year on {month:02}-{day:02} at {time}"
                )
            }
            Trigger::Date(DateRule::Once(date), time) => {
                let (date, time) = (date.format("%Y-%m-%d"), time.format("%H:%M"));
                trf!("{date} {time}", "On {date} at {time}")
            }
        }
    }
}

/// 上次运行：Unix 毫秒，以及按本机时区是哪天（每天 / 指定日期的任务据此判断「今天跑过没有」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastRun {
    pub at_ms: i64,
    pub day: NaiveDate,
}

/// 每年 `month-day` 在 `year` 落在哪天：平年的 02-29 落到 2 月 28 日（与寄语相同）。
fn yearly_in(year: i32, month: u32, day: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month, day).or_else(|| {
        (month == 2 && day == 29)
            .then(|| NaiveDate::from_ymd_opt(year, 2, 28))
            .flatten()
    })
}

/// 下一次运行的钟面时间（本机时区）。已经该跑了时返回的时间不晚于现在；只那一天的任务过了那天返回 None。
/// `minute` 是一分钟有多长（端到端测试里调短）。
pub fn next_due<Tz: TimeZone>(
    trigger: &Trigger,
    last: Option<&LastRun>,
    now: DateTime<Utc>,
    tz: &Tz,
    minute: Duration,
) -> Option<NaiveDateTime> {
    let local = now.with_timezone(tz).naive_local();
    let today = local.date();
    // 今天已经跑过（或上次运行的日子在「今天」之后——比如往西飞改了时区）：从明天开始找。
    let start = match last {
        Some(last) if last.day >= today => today.succ_opt()?,
        _ => today,
    };
    match trigger {
        Trigger::Every(minutes) => {
            let Some(last) = last else {
                return Some(local);
            };
            let step = i64::try_from(minute.as_millis().saturating_mul(u128::from(*minutes))).ok()?;
            let next = DateTime::from_timestamp_millis(last.at_ms.saturating_add(step))?;
            Some(next.with_timezone(tz).naive_local())
        }
        Trigger::Daily(time) => Some(start.and_time(*time)),
        Trigger::Date(DateRule::Once(date), time) => (*date >= start).then(|| date.and_time(*time)),
        Trigger::Date(DateRule::Yearly { month, day }, time) => (start.year()..=start.year() + 1)
            .filter_map(|year| yearly_in(year, *month, *day))
            .find(|date| *date >= start)
            .map(|date| date.and_time(*time)),
    }
}

/// 现在该不该运行。
pub fn is_due<Tz: TimeZone>(
    trigger: &Trigger,
    last: Option<&LastRun>,
    now: DateTime<Utc>,
    tz: &Tz,
    minute: Duration,
) -> bool {
    if let (Trigger::Every(minutes), Some(last)) = (trigger, last) {
        // 间隔按绝对时间算：夏令时回拨那一小时里钟面时间会倒退，不能拿钟面时间比。
        let step = i64::try_from(minute.as_millis().saturating_mul(u128::from(*minutes))).unwrap_or(i64::MAX);
        let elapsed = now.timestamp_millis() - last.at_ms;
        // 系统时间被往回调过（上次运行在「未来」）：直接跑，免得一停就是好几天。
        return elapsed < 0 || elapsed >= step;
    }
    let local = now.with_timezone(tz).naive_local();
    next_due(trigger, last, now, tz, minute).is_some_and(|next| next <= local)
}

// ---------------------------------------------------------------- 脚本输出

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    #[serde(default)]
    notify: Option<NotifyRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotifyRequest {
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    url: Option<String>,
}

/// 解析脚本的 stdout：空 = 什么也不做；JSON 对象里的 `notify` = 发一条通知。Err 是给插件作者看的原因。
pub fn parse_output(stdout: &str) -> Result<Option<Notice>> {
    let text = stdout.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let output: Output = serde_json::from_str(text).map_err(|err| {
        let head: String = text.chars().take(80).collect();
        anyhow!(trf!(
            "脚本的输出不是约定的 JSON（为空，或 {{\"notify\": {{\"title\": …}}}}；日志请写到 stderr）：{err}；输出开头：{head}",
            "The script's output isn't the expected JSON (empty, or {{\"notify\": {{\"title\": …}}}}; write logs to stderr): {err}; output starts with: {head}"
        ))
    })?;
    let Some(request) = output.notify else {
        return Ok(None);
    };
    let title = request.title.trim().to_string();
    anyhow::ensure!(
        !title.is_empty(),
        tr("notify.title 不能为空", "notify.title must not be empty")
    );
    anyhow::ensure!(
        title.chars().count() <= MAX_TITLE_CHARS,
        trf!(
            "notify.title 超过 {MAX_TITLE_CHARS} 个字",
            "notify.title is longer than {MAX_TITLE_CHARS} characters"
        )
    );
    anyhow::ensure!(
        request.body.chars().count() <= MAX_BODY_CHARS,
        trf!(
            "notify.body 超过 {MAX_BODY_CHARS} 个字",
            "notify.body is longer than {MAX_BODY_CHARS} characters"
        )
    );
    if let Some(url) = &request.url {
        anyhow::ensure!(
            notify::clickable(url),
            tr(
                "notify.url 只能是 http:// 或 https:// 链接",
                "notify.url must be an http:// or https:// link"
            )
        );
    }
    Ok(Some(Notice {
        title,
        body: request.body.trim().to_string(),
        url: request.url,
    }))
}

// ---------------------------------------------------------------- 状态

/// 一个任务上次运行的结果（存在 `plugin-schedules.json`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    /// 开始运行的时间（Unix 毫秒）。
    pub last_run_ms: i64,
    /// 开始运行时本机的日期（YYYY-MM-DD）。
    pub day: String,
    pub ok: bool,
    /// 失败原因；成功时为空。
    pub error: String,
    /// 这次发了通知。
    pub notified: bool,
}

impl Record {
    pub fn last(&self) -> Option<LastRun> {
        Some(LastRun {
            at_ms: self.last_run_ms,
            day: NaiveDate::parse_from_str(&self.day, "%Y-%m-%d").ok()?,
        })
    }

    /// 设置页、命令行用的一句话结果。
    pub fn summary(&self) -> String {
        match (self.ok, self.notified) {
            (true, true) => tr("成功，已发通知", "OK, notification sent").into(),
            (true, false) => tr("成功", "OK").into(),
            (false, _) => trf!("失败：{}", "Failed: {}", self.error),
        }
    }
}

const STATE_FILE: &str = "plugin-schedules.json";

/// `<插件 id>/<任务 id>`。
pub fn key(plugin: &str, schedule: &str) -> String {
    format!("{plugin}/{schedule}")
}

static RECORDS: LazyLock<Mutex<BTreeMap<String, Record>>> = LazyLock::new(|| {
    let records = crate::paths::data_dir()
        .ok()
        .and_then(|dir| std::fs::read_to_string(dir.join(STATE_FILE)).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    Mutex::new(records)
});
/// 状态每变一次加一：设置页据此重绘。
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

fn records() -> std::sync::MutexGuard<'static, BTreeMap<String, Record>> {
    RECORDS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 某个任务上次运行的结果。
pub fn record(plugin: &str, schedule: &str) -> Option<Record> {
    records().get(&key(plugin, schedule)).cloned()
}

fn save_record(key: &str, record: Record) {
    let mut all = records();
    all.insert(key.to_string(), record);
    let saved = crate::paths::data_dir().and_then(|dir| {
        let path = dir.join(STATE_FILE);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&*all)?)
            .with_context(|| trf!("写入 {}", "Couldn't write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| trf!("保存 {}", "Couldn't save {}", path.display()))
    });
    drop(all);
    if let Err(err) = saved {
        eprintln!("helmp: 保存定时任务状态失败：{err:#}");
        crate::telemetry::report("plugins.schedule_state", &anyhow!("couldn't save the schedule state"));
    }
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

// ---------------------------------------------------------------- 运行

/// 一个能运行的任务。
#[derive(Clone)]
pub struct Job {
    pub plugin: Arc<Plugin>,
    pub spec: ScheduleSpec,
    pub trigger: Trigger,
}

impl Job {
    pub fn key(&self) -> String {
        key(self.plugin.id(), &self.spec.id)
    }
}

/// 用户有没有在设置里单独停用这个任务。
pub fn task_enabled(config: &Config, plugin: &str, schedule: &str) -> bool {
    config
        .plugins
        .get(plugin)
        .is_none_or(|settings| !settings.schedules_off.contains(schedule))
}

/// 现在允许运行的任务：插件已启用（权限确认过）、任务没被单独停用、触发规则有效。
pub fn jobs(config: &Config) -> Vec<Job> {
    registry::current()
        .iter()
        .filter(|entry| entry.state(config) == State::Enabled)
        .filter_map(|entry| entry.plugin())
        .flat_map(|plugin| {
            plugin
                .manifest
                .schedules
                .iter()
                .filter(|spec| task_enabled(config, plugin.id(), &spec.id))
                .filter_map(|spec| {
                    Some(Job {
                        plugin: Arc::clone(plugin),
                        spec: spec.clone(),
                        trigger: Trigger::from_spec(spec).ok()?,
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// 正在运行的任务：进程组号（脚本启动后才有）和「被叫停」标记。
struct Running {
    pgid: Arc<Mutex<Option<i32>>>,
    stopped: Arc<AtomicBool>,
}

static RUNNING: Mutex<BTreeMap<String, Running>> = Mutex::new(BTreeMap::new());
/// 正在退出：不再启动新任务。
static QUITTING: AtomicBool = AtomicBool::new(false);

fn running() -> std::sync::MutexGuard<'static, BTreeMap<String, Running>> {
    RUNNING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 这个任务正在运行。
pub fn is_running(plugin: &str, schedule: &str) -> bool {
    running().contains_key(&key(plugin, schedule))
}

fn stop(run: &Running) {
    run.stopped.store(true, Ordering::SeqCst);
    if let Some(pgid) = *run.pgid.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
        crate::providers::scripts::runner::kill_group(pgid);
    }
}

/// 退出 Helmp 时调用：结束所有正在跑的定时脚本（连同它们派生的子进程），不再启动新的。
pub fn stop_all() {
    QUITTING.store(true, Ordering::SeqCst);
    for run in running().values() {
        stop(run);
    }
}

/// 一分钟有多长：正常 60 秒；端到端测试（测试模式下）用 `HELMP_SCHEDULE_MINUTE_MS` 调短。
fn minute() -> Duration {
    test_override("HELMP_SCHEDULE_MINUTE_MS").unwrap_or(Duration::from_secs(60))
}

fn test_override(name: &str) -> Option<Duration> {
    if !super::secrets::test_mode() {
        return None;
    }
    std::env::var(name)
        .ok()?
        .parse()
        .ok()
        .map(Duration::from_millis)
}

/// 一次运行的结果（已写进状态）。
#[derive(Debug, Clone)]
pub struct Finished {
    pub key: String,
    pub record: Record,
}

/// 运行一次（阻塞）：执行脚本、按输出发通知、记下结果。被 `stop` 叫停的记为失败「已停止」。
fn run_job(job: &Job, pgid: &Arc<Mutex<Option<i32>>>, stopped: &AtomicBool) -> Record {
    let started = chrono::Local::now();
    let mut record = Record {
        last_run_ms: started.timestamp_millis(),
        day: started.date_naive().format("%Y-%m-%d").to_string(),
        ..Record::default()
    };
    let on_spawn = |id: i32| {
        *pgid.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(id);
        // 叫停发生在脚本刚启动、还没登记进程组的那一刻：补杀一次。
        if stopped.load(Ordering::SeqCst) {
            crate::providers::scripts::runner::kill_group(id);
        }
    };
    let outcome = super::exec::run_scheduled(&job.plugin, &job.spec, &on_spawn)
        .and_then(|stdout| parse_output(&stdout));
    let result = if stopped.load(Ordering::SeqCst) {
        Err(anyhow!(tr(
            "已停止（插件或任务被停用，或 Helmp 退出）",
            "Stopped (the plugin or task was turned off, or Helmp quit)"
        )))
    } else {
        match outcome {
            Ok(Some(_)) if !job.plugin.manifest.sends_notifications() => Err(anyhow!(tr(
                "脚本请求发通知，但插件没有在 permissions 里声明 \"notifications\"",
                "The script asked for a notification, but the plugin doesn't declare \"notifications\" in permissions"
            ))),
            Ok(Some(notice)) => notify::send(&notice).map(|()| record.notified = true),
            Ok(None) => Ok(()),
            Err(err) => Err(err),
        }
    };
    match result {
        Ok(()) => record.ok = true,
        Err(err) => {
            let text = format!("{err:#}").replace('\n', " ");
            record.error = text.chars().take(300).collect();
        }
    }
    let timed_out = record.error.contains(tr("已强制停止", "was stopped"));
    track(job.trigger.kind(), record.ok, record.notified, timed_out);
    record
}

/// 埋点：每种触发方式、成败组合每天最多一次（按间隔的任务一天可能跑几百次）。不带插件 id 和内容。
fn track(kind: &'static str, ok: bool, notified: bool, timed_out: bool) {
    static SENT: Mutex<BTreeSet<(NaiveDate, &'static str, bool)>> = Mutex::new(BTreeSet::new());
    let today = chrono::Local::now().date_naive();
    let mut sent = SENT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    sent.retain(|(day, _, _)| *day == today);
    if sent.insert((today, kind, ok)) {
        crate::telemetry::plugin_schedule_run(kind, ok, notified, timed_out);
    }
}

/// 在新线程里开始运行一个任务；结束时写状态并调用 `done`。
fn spawn(job: Job, done: impl FnOnce(Finished) + Send + 'static) {
    let key = job.key();
    let pgid = Arc::new(Mutex::new(None));
    let stopped = Arc::new(AtomicBool::new(false));
    running().insert(
        key.clone(),
        Running {
            pgid: Arc::clone(&pgid),
            stopped: Arc::clone(&stopped),
        },
    );
    let thread_key = key.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("schedule {key}"))
        .spawn(move || {
            let record = run_job(&job, &pgid, &stopped);
            save_record(&thread_key, record.clone());
            running().remove(&thread_key);
            WAKE.notify();
            done(Finished {
                key: thread_key,
                record,
            });
        });
    if let Err(err) = spawned {
        running().remove(&key);
        eprintln!("helmp: 启动定时任务线程失败：{err}");
        crate::telemetry::report("plugins.schedule_spawn", &anyhow!("couldn't spawn a schedule thread"));
    }
}

/// 检查一次：叫停不再允许运行的任务，启动到点的任务。返回这次启动的任务（`<插件 id>/<任务 id>`）。
pub fn tick(now: DateTime<Utc>, done: &(impl Fn(Finished) + Clone + Send + 'static)) -> Vec<String> {
    let config = crate::config::get();
    let jobs = jobs(&config);
    let allowed: BTreeSet<String> = jobs.iter().map(Job::key).collect();
    for (key, run) in running().iter() {
        if !allowed.contains(key) {
            stop(run);
        }
    }
    if QUITTING.load(Ordering::SeqCst) {
        return Vec::new();
    }
    let minute = minute();
    let mut started = Vec::new();
    for job in jobs {
        let key = job.key();
        if running().len() >= MAX_CONCURRENT {
            break;
        }
        if running().contains_key(&key) {
            continue;
        }
        let last = records().get(&key).and_then(Record::last);
        if is_due(&job.trigger, last.as_ref(), now, &chrono::Local, minute) {
            let done = done.clone();
            spawn(job, done);
            started.push(key);
        }
    }
    started
}

/// 唤醒调度循环（配置变了、任务结束了）：停用插件后不用等下一拍就叫停它的脚本。
struct Wake {
    flag: Mutex<bool>,
    cond: Condvar,
}

impl Wake {
    fn notify(&self) {
        *self.flag.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.cond.notify_all();
    }

    fn wait(&self, timeout: Duration) {
        let flag = self.flag.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut flag, _) = self
            .cond
            .wait_timeout_while(flag, timeout, |woken| !*woken)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *flag = false;
    }
}

static WAKE: Wake = Wake {
    flag: Mutex::new(false),
    cond: Condvar::new(),
};

/// 调度循环（阻塞），到 `deadline`（None = 一直跑）或 `stop_all` 后结束。
/// `done` 在每次运行结束时调用（命令行 `watch` 打印用）。
pub fn serve(deadline: Option<Instant>, done: impl Fn(Finished) + Clone + Send + 'static) {
    let tick_every = test_override("HELMP_SCHEDULE_TICK_MS").unwrap_or(TICK);
    let first = test_override("HELMP_SCHEDULE_TICK_MS").unwrap_or(FIRST_DELAY);
    let wait = |step: Duration| {
        let left = deadline.map_or(step, |deadline| deadline.saturating_duration_since(Instant::now()));
        WAKE.wait(step.min(left));
    };
    let over = || QUITTING.load(Ordering::SeqCst) || deadline.is_some_and(|deadline| Instant::now() >= deadline);
    wait(first);
    while !over() {
        tick(Utc::now(), &done);
        wait(tick_every);
    }
}

/// 等正在跑的任务都结束（最多 `timeout`）。
pub fn wait_idle(timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !running().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 图形界面启动时调用：在后台线程跑调度循环，配置一变就醒来检查。
pub fn start() {
    crate::config::subscribe(|_| WAKE.notify());
    let spawned = std::thread::Builder::new()
        .name("plugin schedules".into())
        .spawn(|| serve(None, |_| {}));
    if let Err(err) = spawned {
        eprintln!("helmp: 启动插件定时任务失败：{err}");
        crate::telemetry::report("plugins.schedule_spawn", &anyhow!("couldn't spawn the scheduler"));
    }
}

/// 立即运行一次（命令行 `helmp plugins schedules run`，给插件作者调试用）：不管时间到没到，
/// 但插件仍要已启用。阻塞到结束。
pub fn run_now(plugin: &str, schedule: &str) -> Result<Record> {
    let config = crate::config::get();
    let job = jobs(&config)
        .into_iter()
        .find(|job| job.plugin.id() == plugin && job.spec.id == schedule)
        .with_context(|| {
            trf!(
                "没有可运行的定时任务 {plugin}/{schedule}（插件要已启用、任务没被停用）",
                "No runnable scheduled task {plugin}/{schedule} (the plugin must be enabled and the task turned on)"
            )
        })?;
    let (tx, rx) = std::sync::mpsc::channel();
    spawn(job, move |finished| {
        let _ = tx.send(finished.record);
    });
    rx.recv()
        .map_err(|_| anyhow!(tr("定时任务没有结束", "The scheduled task didn't finish")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::plugins::manifest;

    const MINUTE: Duration = Duration::from_secs(60);

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn time(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    fn spec(json: &str) -> ScheduleSpec {
        let text = format!(
            r#"{{"schemaVersion": 1, "id": "com.example.tasks", "name": "Tasks", "version": "1.0.0",
                "author": "Example", "schedules": [{json}]}}"#
        );
        manifest::parse(&text, None)
            .unwrap_or_else(|errors| panic!("{errors:?}"))
            .schedules
            .remove(0)
    }

    fn trigger(json: &str) -> Trigger {
        Trigger::from_spec(&spec(json)).unwrap()
    }

    /// 上海时间（无夏令时）的某一刻。
    fn shanghai(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        chrono_tz::Asia::Shanghai
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn ran(at: DateTime<Utc>) -> LastRun {
        LastRun {
            at_ms: at.timestamp_millis(),
            day: at.with_timezone(&chrono_tz::Asia::Shanghai).date_naive(),
        }
    }

    fn due(trigger: &Trigger, last: Option<&LastRun>, now: DateTime<Utc>) -> bool {
        is_due(trigger, last, now, &chrono_tz::Asia::Shanghai, MINUTE)
    }

    fn next(trigger: &Trigger, last: Option<&LastRun>, now: DateTime<Utc>) -> Option<NaiveDateTime> {
        next_due(trigger, last, now, &chrono_tz::Asia::Shanghai, MINUTE)
    }

    #[test]
    fn triggers_accept_only_the_documented_forms() {
        assert_eq!(trigger(r#"{"id": "a", "everyMinutes": 30, "script": {"path": "a.sh"}}"#), Trigger::Every(30));
        assert_eq!(trigger(r#"{"id": "a", "at": "08:05", "script": {"path": "a.sh"}}"#), Trigger::Daily(time(8, 5)));
        assert_eq!(
            trigger(r#"{"id": "a", "date": "12-25", "at": "09:00", "script": {"path": "a.sh"}}"#),
            Trigger::Date(DateRule::Yearly { month: 12, day: 25 }, time(9, 0))
        );
        // 只写日期：那天 00:00 起（也就是那天 Helmp 一运行就跑）。
        assert_eq!(
            trigger(r#"{"id": "a", "date": "2027-01-01", "script": {"path": "a.sh"}}"#),
            Trigger::Date(DateRule::Once(date(2027, 1, 1)), NaiveTime::MIN)
        );
        // 写错的要在校验时拦下：猜错了会在错的时间跑后台脚本。
        let errors = |json: &str| {
            let text = format!(
                r#"{{"schemaVersion": 1, "id": "com.example.tasks", "name": "Tasks", "version": "1.0.0",
                    "author": "Example", "schedules": [{json}]}}"#
            );
            crate::i18n::with_lang(crate::i18n::Lang::En, || {
                manifest::parse(&text, None).err().unwrap_or_default().join("\n")
            })
        };
        assert!(errors(r#"{"id": "a", "everyMinutes": 1, "script": {"path": "a.sh"}}"#).contains("between 5 and"));
        assert!(errors(r#"{"id": "a", "everyMinutes": 30, "at": "08:00", "script": {"path": "a.sh"}}"#)
            .contains("can't be combined"));
        assert!(errors(r#"{"id": "a", "script": {"path": "a.sh"}}"#).contains("needs one of"));
        for bad in ["8:00", "24:00", "08:60", "0800", "08:00:00"] {
            let json = format!(r#"{{"id": "a", "at": "{bad}", "script": {{"path": "a.sh"}}}}"#);
            assert!(errors(&json).contains("HH:MM"), "{bad}");
        }
        assert!(errors(r#"{"id": "a", "date": "2027-02-29", "script": {"path": "a.sh"}}"#).contains("valid date"));
        assert!(errors(r#"{"id": "a", "at": "08:00", "script": {"path": "/bin/sh"}}"#).contains("relative"));
        assert!(errors(r#"{"id": "a", "at": "08:00", "script": {"path": "a.sh", "args": ["{{clipboard}}"]}}"#)
            .contains("background"));
        assert!(errors(r#"{"id": "a", "at": "08:00", "script": {"path": "a.sh", "args": ["{{query}}"]}}"#)
            .contains("no input"));
        assert!(errors(r#"{"id": "a", "at": "08:00", "script": {"path": "a.sh", "timeoutMs": 999999}}"#)
            .contains("milliseconds"));
        assert!(errors(r#"{"id": "a", "at": "08:00", "script": {"path": "a.sh"}}, {"id": "a", "at": "09:00", "script": {"path": "a.sh"}}"#)
            .contains("duplicate"));
        // 只有定时任务的插件合法；能用偏好设置和日期变量。
        assert_eq!(
            errors(r#"{"id": "a", "at": "08:00", "script": {"path": "a.sh", "args": ["{{date}}"]}}"#),
            ""
        );
    }

    #[test]
    fn notifications_must_be_declared_and_need_a_schedule() {
        let text = r#"{"schemaVersion": 1, "id": "com.example.tasks", "name": "Tasks", "version": "1.0.0",
            "author": "Example", "permissions": ["notifications"],
            "commands": [{"id": "a", "title": "A", "action": {"type": "open", "url": "https://example.com"}}]}"#;
        let errors = crate::i18n::with_lang(crate::i18n::Lang::En, || {
            manifest::parse(text, None).err().unwrap_or_default().join("\n")
        });
        assert!(errors.contains("only scheduled tasks can send notifications"), "{errors}");
    }

    #[test]
    fn interval_tasks_run_once_per_interval_even_after_a_long_absence() {
        let every = trigger(r#"{"id": "a", "everyMinutes": 30, "script": {"path": "a.sh"}}"#);
        let t0 = shanghai(2026, 6, 1, 9, 0);
        // 从没跑过：马上跑。
        assert!(due(&every, None, t0));
        let last = ran(t0);
        assert!(!due(&every, Some(&last), t0 + chrono::Duration::minutes(29)));
        assert!(due(&every, Some(&last), t0 + chrono::Duration::minutes(30)));
        assert_eq!(next(&every, Some(&last), t0), Some(date(2026, 6, 1).and_time(time(9, 30))));
        // 合盖三天回来：只是「到点了」，跑一次后重新计时，不会把错过的 144 次补跑。
        let back = t0 + chrono::Duration::days(3);
        assert!(due(&every, Some(&last), back));
        assert!(!due(&every, Some(&ran(back)), back + chrono::Duration::minutes(1)));
        // 系统时间被往回调：不能干等到「未来」的上次运行时间之后。
        assert!(due(&every, Some(&last), t0 - chrono::Duration::days(1)));
    }

    #[test]
    fn daily_tasks_catch_up_once_on_the_same_day_only() {
        let daily = trigger(r#"{"id": "a", "at": "08:00", "script": {"path": "a.sh"}}"#);
        let yesterday = ran(shanghai(2026, 6, 1, 8, 0));
        // 7:59 还没到点。
        assert!(!due(&daily, Some(&yesterday), shanghai(2026, 6, 2, 7, 59)));
        assert_eq!(
            next(&daily, Some(&yesterday), shanghai(2026, 6, 2, 7, 59)),
            Some(date(2026, 6, 2).and_time(time(8, 0)))
        );
        // 8 点时 Mac 在睡眠，10 点唤醒（或 10 点才启动 Helmp）：补跑一次。
        let woke = shanghai(2026, 6, 2, 10, 0);
        assert!(due(&daily, Some(&yesterday), woke));
        // 补跑过了就不再跑，今天剩下的时间都不跑；下一次是明天 8 点。
        let caught_up = ran(woke);
        assert!(!due(&daily, Some(&caught_up), shanghai(2026, 6, 2, 23, 59)));
        assert_eq!(
            next(&daily, Some(&caught_up), woke),
            Some(date(2026, 6, 3).and_time(time(8, 0)))
        );
        // 几天没开 Helmp：今天 6 点打开时不补前几天的，等今天 8 点。
        let long_ago = ran(shanghai(2026, 5, 20, 8, 0));
        assert!(!due(&daily, Some(&long_ago), shanghai(2026, 6, 2, 6, 0)));
        // 新装的插件 9 点启用：今天 8 点已过，当天补一次。
        assert!(due(&daily, None, shanghai(2026, 6, 2, 9, 0)));
    }

    #[test]
    fn dated_tasks_repeat_yearly_and_one_off_dates_end() {
        let yearly = trigger(r#"{"id": "a", "date": "12-25", "at": "09:00", "script": {"path": "a.sh"}}"#);
        assert!(!due(&yearly, None, shanghai(2026, 12, 24, 23, 0)));
        assert!(!due(&yearly, None, shanghai(2026, 12, 25, 8, 59)));
        assert!(due(&yearly, None, shanghai(2026, 12, 25, 9, 0)));
        let done = ran(shanghai(2026, 12, 25, 9, 0));
        assert!(!due(&yearly, Some(&done), shanghai(2026, 12, 25, 22, 0)));
        // 跑过之后，下一次是明年同一天。
        assert_eq!(
            next(&yearly, Some(&done), shanghai(2026, 12, 25, 22, 0)),
            Some(date(2027, 12, 25).and_time(time(9, 0)))
        );
        assert!(due(&yearly, Some(&done), shanghai(2027, 12, 25, 9, 30)));
        // 那天 Helmp 没开：第二天不补。
        assert!(!due(&yearly, None, shanghai(2026, 12, 26, 9, 0)));

        let once = trigger(r#"{"id": "a", "date": "2027-01-01", "at": "00:30", "script": {"path": "a.sh"}}"#);
        assert!(due(&once, None, shanghai(2027, 1, 1, 0, 30)));
        let done = ran(shanghai(2027, 1, 1, 0, 30));
        assert_eq!(next(&once, Some(&done), shanghai(2027, 1, 1, 1, 0)), None, "只那一天的任务跑过就结束");
        assert_eq!(next(&once, None, shanghai(2027, 1, 2, 0, 0)), None);
    }

    #[test]
    fn a_leap_day_task_runs_on_feb_28_in_common_years() {
        let leap = trigger(r#"{"id": "a", "date": "02-29", "at": "09:00", "script": {"path": "a.sh"}}"#);
        assert!(due(&leap, None, shanghai(2028, 2, 29, 9, 0)));
        assert!(!due(&leap, None, shanghai(2028, 2, 28, 9, 0)), "闰年不能提前一天");
        assert!(due(&leap, None, shanghai(2027, 2, 28, 9, 0)), "平年落到 2 月 28 日");
        assert_eq!(
            next(&leap, Some(&ran(shanghai(2027, 2, 28, 9, 0))), shanghai(2027, 3, 1, 0, 0)),
            Some(date(2028, 2, 29).and_time(time(9, 0)))
        );
    }

    #[test]
    fn times_follow_the_local_time_zone_and_survive_dst() {
        let daily = trigger(r#"{"id": "a", "at": "08:00", "script": {"path": "a.sh"}}"#);
        // 同一刻：上海已经 8 点过，洛杉矶还是前一天下午——按本机时区走。
        let instant = Utc.with_ymd_and_hms(2026, 6, 2, 0, 30, 0).unwrap();
        assert!(is_due(&daily, None, instant, &chrono_tz::Asia::Shanghai, MINUTE));
        assert!(is_due(&daily, None, instant, &chrono_tz::America::Los_Angeles, MINUTE));
        let la_ran = LastRun {
            at_ms: instant.timestamp_millis(),
            day: instant.with_timezone(&chrono_tz::America::Los_Angeles).date_naive(),
        };
        assert!(!is_due(&daily, Some(&la_ran), instant, &chrono_tz::America::Los_Angeles, MINUTE));

        let ny = chrono_tz::America::New_York;
        let at = |y, m, d, h, min| ny.with_ymd_and_hms(y, m, d, h, min, 0).single().unwrap().with_timezone(&Utc);
        // 春季拨快（2027-03-14 2:00 → 3:00）：02:30 这个钟面时间不存在，3 点后照样跑一次，不会整天跳过。
        let gap = trigger(r#"{"id": "a", "at": "02:30", "script": {"path": "a.sh"}}"#);
        let before = LastRun { at_ms: at(2027, 3, 13, 2, 30).timestamp_millis(), day: date(2027, 3, 13) };
        assert!(!is_due(&gap, Some(&before), at(2027, 3, 14, 1, 59), &ny, MINUTE));
        assert!(is_due(&gap, Some(&before), at(2027, 3, 14, 3, 0), &ny, MINUTE));
        // 秋季拨慢（2026-11-01 1:00–2:00 走两遍）：01:30 只跑一次。
        let fold = trigger(r#"{"id": "a", "at": "01:30", "script": {"path": "a.sh"}}"#);
        let first = ny.with_ymd_and_hms(2026, 11, 1, 1, 30, 0).earliest().unwrap().with_timezone(&Utc);
        assert!(is_due(&fold, None, first, &ny, MINUTE));
        let once = LastRun { at_ms: first.timestamp_millis(), day: date(2026, 11, 1) };
        let second = first + chrono::Duration::hours(1);
        assert_eq!(second.with_timezone(&ny).format("%H:%M").to_string(), "01:30");
        assert!(!is_due(&fold, Some(&once), second, &ny, MINUTE), "回拨的那一小时不能再跑一遍");
        // 间隔按绝对时间：回拨那一小时里钟面倒退，也不会提前跑。
        let every = trigger(r#"{"id": "a", "everyMinutes": 30, "script": {"path": "a.sh"}}"#);
        let late = ny.with_ymd_and_hms(2026, 11, 1, 1, 50, 0).earliest().unwrap().with_timezone(&Utc);
        let last = LastRun { at_ms: late.timestamp_millis(), day: date(2026, 11, 1) };
        assert!(!is_due(&every, Some(&last), late + chrono::Duration::minutes(10), &ny, MINUTE));
        assert!(is_due(&every, Some(&last), late + chrono::Duration::minutes(30), &ny, MINUTE));
    }

    #[test]
    fn script_output_is_empty_or_a_notify_request() {
        assert_eq!(parse_output("  \n").unwrap(), None);
        assert_eq!(parse_output("{}").unwrap(), None);
        let notice = parse_output(
            r#"{"notify": {"title": " Rain later ", "body": "Take an umbrella", "url": "https://example.com/today"}}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(notice.title, "Rain later");
        assert_eq!(notice.body, "Take an umbrella");
        assert_eq!(notice.url.as_deref(), Some("https://example.com/today"));
        // 输出不合约定要说出来（作者会以为通知被吞了），并提示日志写 stderr。
        let err = |text: &str| {
            crate::i18n::with_lang(crate::i18n::Lang::En, || format!("{:#}", parse_output(text).unwrap_err()))
        };
        assert!(err("checking weather...").contains("stderr"));
        assert!(err(r#"{"notify": {"title": ""}}"#).contains("must not be empty"));
        assert!(err(r#"{"notify": {"title": "x", "url": "file:///etc/hosts"}}"#).contains("https://"));
        assert!(err(r#"{"notify": {"title": "x", "sound": "x"}}"#).contains("sound"));
        assert!(err(&format!(r#"{{"notify": {{"title": "{}"}}}}"#, "x".repeat(MAX_TITLE_CHARS + 1))).contains("longer"));
    }

    #[test]
    fn the_example_plugin_validates() {
        let text = include_str!("../../../examples/plugins/com.example.weather-reminder/plugin.json");
        let manifest = manifest::parse(text, None).unwrap_or_else(|errors| panic!("{errors:?}"));
        assert_eq!(manifest.schedules.len(), 2);
        assert!(manifest.sends_notifications());
    }

    #[test]
    fn stopping_a_running_task_kills_its_whole_process_group() {
        // 插件被停用、Helmp 退出时，正在跑的脚本和它派生的子进程都要结束，不能留在后台。
        let dir = crate::providers::scripts::tests::TempDir::new("schedule-stop");
        let pid_file = dir.path().join("child.pid");
        let script = format!("#!/bin/sh\nsleep 300 &\necho $! > '{}'\nwait\n", pid_file.display());
        crate::providers::scripts::tests::write_script(&dir, "hang.sh", &script, true);
        let manifest = r#"{"schemaVersion": 1, "id": "com.example.stop", "name": "Stop", "version": "1.0.0",
            "author": "Example", "schedules": [{"id": "hang", "everyMinutes": 5, "script": {"path": "hang.sh"}}]}"#;
        std::fs::write(dir.path().join("plugin.json"), manifest).unwrap();
        let plugin = registry::load_dir(dir.path(), registry::Source::User).loaded.unwrap();
        let job = Job {
            spec: plugin.manifest.schedules.first().unwrap().clone(),
            trigger: Trigger::Every(5),
            plugin,
        };
        let pgid = Arc::new(Mutex::new(None));
        let stopped = Arc::new(AtomicBool::new(false));
        let worker = {
            let (job, pgid, stopped) = (job.clone(), Arc::clone(&pgid), Arc::clone(&stopped));
            std::thread::spawn(move || run_job(&job, &pgid, &stopped))
        };
        let child: i32 = (0..250)
            .find_map(|_| {
                std::thread::sleep(Duration::from_millis(20));
                std::fs::read_to_string(&pid_file).ok()?.trim().parse().ok()
            })
            .expect("脚本没启动");
        let started = std::time::Instant::now();
        stop(&Running { pgid, stopped });
        let record = worker.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(5), "叫停后要马上结束");
        assert!(!record.ok);
        let gone = (0..100).any(|_| {
            // SAFETY: 信号 0 只检查进程是否存在。
            let alive = unsafe { libc::kill(child, 0) } == 0;
            if alive {
                std::thread::sleep(Duration::from_millis(20));
            }
            !alive
        });
        assert!(gone, "子进程 {child} 还活着");
    }
}
