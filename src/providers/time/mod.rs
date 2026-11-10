//! 时间、时区与时间戳：`now` / 「时间」、`time tokyo` / 「东京时间」、`1731234567`（秒或毫秒）、
//! `2026-11-10 10:00 pst to beijing`、`3pm 北京 in 纽约`、`in 3 days` / 「3天后」/ `next friday`。
//!
//! 全部离线：时区规则来自编译进来的 chrono-tz（IANA tzdb），本地时区读 `TZ` 或 `/etc/localtime`。
//! 每一行是一种写法（本地时间、UTC、ISO 8601、Unix 秒 / 毫秒、目标时区），回车复制这一行。
//!
//! 测试接缝：`HELMP_NOW`（RFC 3339）固定「现在」，只在本模块读取。

mod zones;

use chrono::{
    DateTime, Datelike, Days, Duration, Months, NaiveDate, NaiveDateTime, NaiveTime, Offset,
    TimeZone, Utc, Weekday,
};
use chrono_tz::Tz;

use crate::i18n::{tr, trf};
use crate::matcher;
use crate::provider::{Accessory, Action, Icon, Item, NamedAction, Provider, Tint, Tone};

const ICON: Icon = Icon::Symbol {
    name: "clock",
    tint: Tint::Blue,
};
/// 明确的时间写法（时间戳、换算、相对时间、「xx 时间」）排第一。
const SCORE: u32 = 1050;
/// 只打了城市名（`tokyo`、「东京」）：低于应用的完全匹配。
const BARE_CITY_SCORE: u32 = 850;

pub struct Time;

impl Provider for Time {
    fn id(&self) -> &'static str {
        "time"
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let now = now();
        let Some((request, score)) = parse(query, now) else {
            return Vec::new();
        };
        rows(&request, now)
            .into_iter()
            .enumerate()
            .map(|(rank, row)| row.into_item(score.saturating_sub(rank as u32)))
            .collect()
    }
}

/// 「现在」：`HELMP_NOW` 设置了就用它（测试），否则系统时间。
fn now() -> DateTime<Utc> {
    std::env::var("HELMP_NOW")
        .ok()
        .and_then(|text| DateTime::parse_from_rfc3339(text.trim()).ok())
        .map(|time| time.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}

/// 本机时区：`TZ` 环境变量，否则 `/etc/localtime` 指向的 zoneinfo 文件名；都读不出时用 UTC。
pub fn local_zone() -> Tz {
    let from_env = std::env::var("TZ")
        .ok()
        .map(|tz| tz.trim_start_matches(':').to_string());
    let from_link = || {
        let target = std::fs::read_link("/etc/localtime").ok()?;
        let text = target.to_string_lossy().into_owned();
        Some(text.split_once("zoneinfo/")?.1.to_string())
    };
    from_env
        .or_else(from_link)
        .and_then(|name| zones::by_iana(&name))
        .unwrap_or(Tz::UTC)
}

#[derive(Debug, Clone, PartialEq)]
enum Request {
    /// 现在：本地、UTC、ISO、Unix，加几个常用时区。
    Now,
    /// 某个时区现在几点。
    Zone(Tz),
    /// Unix 时间戳；`millis` 表示输入是毫秒。
    Timestamp { at: DateTime<Utc>, millis: bool },
    /// 某个时刻（可带源时区），换到目标时区或 Unix。
    Convert {
        at: DateTime<Utc>,
        from: Tz,
        to: Target,
    },
    /// 相对现在的时刻（`in 3 days`、「3天前」、`next friday`）。
    Relative(DateTime<Utc>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Zone(Tz),
    Unix,
    /// 没写目标：列本地、UTC 和 Unix。
    Local,
}

const NOW_WORDS: &[&str] = &[
    "now",
    "time",
    "date",
    "today",
    "timestamp",
    "unix",
    "unix time",
    "current time",
    "时间",
    "现在",
    "当前时间",
    "现在几点",
    "几点了",
    "日期",
    "今天",
    "时间戳",
    "时区",
    "timezone",
];

/// 解析查询，返回请求和分数。认不出时返回 `None`（不出任何行）。
fn parse(query: &str, now: DateTime<Utc>) -> Option<(Request, u32)> {
    let lower = query.to_lowercase();
    if let Some(request) = timestamp(&lower) {
        return Some((request, SCORE));
    }
    if let Some(request) = relative(&lower, now) {
        return Some((request, SCORE));
    }
    if let Some(request) = conversion(&lower, now) {
        return Some((request, SCORE));
    }
    if let Some(request) = zone_query(&lower) {
        return Some(request);
    }
    let score = NOW_WORDS
        .iter()
        .filter_map(|word| matcher::score(&lower, word))
        .max()
        .filter(|score| *score >= 800)?;
    // 只打了前几个字母（`ti`、`no`）时不压过应用的前缀匹配。
    Some((Request::Now, if score >= 1000 { SCORE } else { 800 }))
}

/// 9–10 位数字是秒，12–13 位是毫秒（覆盖 1973 到 2286 年）。
fn timestamp(text: &str) -> Option<Request> {
    let digits = text.trim();
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let value: i64 = digits.parse().ok()?;
    let (at, millis) = match digits.len() {
        9 | 10 => (DateTime::from_timestamp(value, 0)?, false),
        12 | 13 => (DateTime::from_timestamp_millis(value)?, true),
        _ => return None,
    };
    Some(Request::Timestamp { at, millis })
}

/// 「东京时间」、`time tokyo`、`tokyo time`、`time in tokyo`、「东京现在几点」；只打城市名分数低一些。
fn zone_query(text: &str) -> Option<(Request, u32)> {
    let mut rest = text.trim().to_string();
    let mut marked = false;
    for word in [
        "what time is it in ",
        "time in ",
        "time ",
        " time",
        "现在几点",
        "几点了",
        "几点",
        "现在",
        "时间",
        "的",
    ] {
        if rest.contains(word) {
            rest = rest.replacen(word, " ", 1);
            marked = true;
        }
    }
    let name = rest.trim();
    // 只打了名字时要像个地名：`tokyo`、「东京」算，`la`、`pst` 这类缩写必须带上 time / 时间。
    let long_enough = if name.is_ascii() {
        name.len() >= 4
    } else {
        name.chars().count() >= 2
    };
    if !marked && !long_enough {
        return None;
    }
    let zone = zones::lookup(name)?;
    Some((
        Request::Zone(zone),
        if marked { SCORE } else { BARE_CITY_SCORE },
    ))
}

/// `<时刻> [源时区] (to|in|→|转|换成) <目标时区|unix>`，或者只写一个完整的日期时间。
fn conversion(text: &str, now: DateTime<Utc>) -> Option<Request> {
    let local = local_zone();
    let padded = format!(" {text} ");
    let split = [
        " to ",
        " in ",
        " into ",
        "→",
        "->",
        "换成",
        "转换成",
        "转成",
        "转",
        "等于",
    ]
    .iter()
    .find_map(|connector| {
        let (left, right) = padded.rsplit_once(connector)?;
        Some((left.trim().to_string(), right.trim().to_string()))
    });
    let (left, target) = match split {
        Some((left, right)) => {
            let target = match right.as_str() {
                "unix" | "timestamp" | "epoch" | "时间戳" => Target::Unix,
                _ => Target::Zone(zones::lookup(&right)?),
            };
            (left, target)
        }
        None => (text.trim().to_string(), Target::Local),
    };
    let (moment, zone_text) = moment(&left)?;
    let from = if zone_text.is_empty() {
        local
    } else {
        zones::lookup(&zone_text)?
    };
    // 只写了时间、没写目标（`3pm`）不算：太容易和别的搜索混淆；写了日期才算。
    if target == Target::Local && moment.date.is_none() {
        return None;
    }
    let date = moment
        .date
        .unwrap_or_else(|| now.with_timezone(&from).date_naive());
    let naive = NaiveDateTime::new(date, moment.time.unwrap_or_default());
    let at = from
        .from_local_datetime(&naive)
        .earliest()?
        .with_timezone(&Utc);
    Some(Request::Convert {
        at,
        from,
        to: target,
    })
}

struct Moment {
    date: Option<NaiveDate>,
    time: Option<NaiveTime>,
}

/// 开头的日期和 / 或时间，返回剩下的文字（当作源时区）。
fn moment(text: &str) -> Option<(Moment, String)> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let first = *tokens.first()?;
    // 2026-11-10T10:00 写在一起。
    if let Some((date, time)) = first.split_once('t')
        && let (Some(date), Some(time)) = (parse_date(date), parse_time(time))
    {
        let rest = tokens.get(1..).unwrap_or_default().join(" ");
        return Some((
            Moment {
                date: Some(date),
                time: Some(time),
            },
            rest,
        ));
    }
    if let Some(date) = parse_date(first) {
        if let Some(time) = tokens.get(1).and_then(|t| parse_time(t)) {
            let rest = tokens.get(2..).unwrap_or_default().join(" ");
            return Some((
                Moment {
                    date: Some(date),
                    time: Some(time),
                },
                rest,
            ));
        }
        let rest = tokens.get(1..).unwrap_or_default().join(" ");
        return Some((
            Moment {
                date: Some(date),
                time: None,
            },
            rest,
        ));
    }
    let time = parse_time(first)?;
    let rest = tokens.get(1..).unwrap_or_default().join(" ");
    Some((
        Moment {
            date: None,
            time: Some(time),
        },
        rest,
    ))
}

fn parse_date(text: &str) -> Option<NaiveDate> {
    ["%Y-%m-%d", "%Y/%m/%d", "%Y.%m.%d"]
        .iter()
        .find_map(|format| NaiveDate::parse_from_str(text, format).ok())
}

/// `15:00`、`15:00:30`、`3pm`、`3:30pm`、`15点`、`15点30`。
fn parse_time(text: &str) -> Option<NaiveTime> {
    let text = text.trim();
    for format in ["%H:%M:%S", "%H:%M"] {
        if let Ok(time) = NaiveTime::parse_from_str(text, format) {
            return Some(time);
        }
    }
    let (clock, pm) = if let Some(rest) = text.strip_suffix("pm") {
        (rest, Some(true))
    } else if let Some(rest) = text.strip_suffix("am") {
        (rest, Some(false))
    } else if let Some((hour, minute)) = text.split_once('点') {
        let minute = minute.trim_end_matches('分');
        let minute: u32 = if minute.is_empty() {
            0
        } else {
            minute.parse().ok()?
        };
        return NaiveTime::from_hms_opt(hour.parse().ok()?, minute, 0);
    } else {
        return None;
    };
    let (hour, minute) = match clock.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (clock.parse::<u32>().ok()?, 0),
    };
    if !(1..=12).contains(&hour) {
        return None;
    }
    let hour = match (hour, pm) {
        (12, Some(false)) => 0,
        (12, _) => 12,
        (h, Some(true)) => h + 12,
        (h, _) => h,
    };
    NaiveTime::from_hms_opt(hour, minute, 0)
}

/// `in 3 days`、`3 days ago`、`+3d`、`-2h`、「3天后」「2小时前」、`next friday`、「下周五」。
fn relative(text: &str, now: DateTime<Utc>) -> Option<Request> {
    let text = text.trim();
    if let Some(at) = weekday(text, now) {
        return Some(Request::Relative(at));
    }
    let (body, sign) = if let Some(rest) = text.strip_prefix("in ") {
        (rest, 1)
    } else if let Some(rest) = text.strip_suffix(" ago") {
        (rest, -1)
    } else if let Some(rest) = text
        .strip_suffix("以后")
        .or_else(|| text.strip_suffix("后"))
    {
        (rest, 1)
    } else if let Some(rest) = text
        .strip_suffix("以前")
        .or_else(|| text.strip_suffix("前"))
    {
        (rest, -1)
    } else if let Some(rest) = text.strip_prefix('+') {
        (rest, 1)
    } else {
        (text.strip_prefix('-')?, -1)
    };
    let body = body.trim();
    let split = body
        .find(|c: char| !c.is_ascii_digit())
        .filter(|index| *index > 0)?;
    let amount: i64 = body.get(..split)?.parse().ok()?;
    let unit = body.get(split..)?.trim();
    let amount = amount.checked_mul(sign)?;
    let local = local_zone();
    let at = match unit {
        "s" | "sec" | "secs" | "second" | "seconds" | "秒" | "秒钟" => {
            now + Duration::seconds(amount)
        }
        "m" | "min" | "mins" | "minute" | "minutes" | "分钟" => now + Duration::minutes(amount),
        "h" | "hr" | "hrs" | "hour" | "hours" | "小时" | "个小时" => {
            now + Duration::hours(amount)
        }
        "d" | "day" | "days" | "天" => now + Duration::days(amount),
        "w" | "wk" | "week" | "weeks" | "周" | "星期" | "个星期" => {
            now + Duration::weeks(amount)
        }
        "mo" | "month" | "months" | "个月" | "月" => {
            let months = Months::new(u32::try_from(amount.unsigned_abs()).ok()?);
            let base = now.with_timezone(&local);
            let moved = if amount >= 0 {
                base.checked_add_months(months)?
            } else {
                base.checked_sub_months(months)?
            };
            moved.with_timezone(&Utc)
        }
        "y" | "yr" | "year" | "years" | "年" => {
            let months = Months::new(u32::try_from(amount.unsigned_abs().checked_mul(12)?).ok()?);
            let base = now.with_timezone(&local);
            let moved = if amount >= 0 {
                base.checked_add_months(months)?
            } else {
                base.checked_sub_months(months)?
            };
            moved.with_timezone(&Utc)
        }
        _ => return None,
    };
    Some(Request::Relative(at))
}

/// `next friday` / 「下周五」：下一周的那一天；`friday` / `this friday` / 「周五」「这周五」：今天之后最近的那一天。
fn weekday(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    const DAYS: [(Weekday, &str, &str); 7] = [
        (Weekday::Mon, "monday", "一"),
        (Weekday::Tue, "tuesday", "二"),
        (Weekday::Wed, "wednesday", "三"),
        (Weekday::Thu, "thursday", "四"),
        (Weekday::Fri, "friday", "五"),
        (Weekday::Sat, "saturday", "六"),
        (Weekday::Sun, "sunday", "日"),
    ];
    let (next, name) = if let Some(rest) = text.strip_prefix("next ") {
        (true, rest)
    } else if let Some(rest) = text
        .strip_prefix("下周")
        .or_else(|| text.strip_prefix("下星期"))
    {
        (true, rest)
    } else {
        let rest = text
            .strip_prefix("this ")
            .or_else(|| text.strip_prefix("这周"))
            .or_else(|| text.strip_prefix("本周"))
            .or_else(|| text.strip_prefix("周"))
            .or_else(|| text.strip_prefix("星期"))?;
        (false, rest)
    };
    let name = name.trim();
    let target = DAYS.iter().find_map(|(day, en, zh)| {
        let short = en.get(..3).unwrap_or(en);
        (name == *en || name == short || name == *zh || (*zh == "日" && name == "天"))
            .then_some(*day)
    })?;
    let local = local_zone();
    let today = now.with_timezone(&local).date_naive();
    let from_monday = |day: Weekday| day.num_days_from_monday() as i64;
    let date = if next {
        // 下周一 = 本周一 + 7 天，再加上目标是周几。
        let monday = today - Days::new(from_monday(today.weekday()) as u64);
        monday + Days::new(7 + from_monday(target) as u64)
    } else {
        let ahead = (from_monday(target) - from_monday(today.weekday())).rem_euclid(7);
        today + Days::new(ahead as u64)
    };
    let time = now.with_timezone(&local).time();
    let naive = NaiveDateTime::new(date, time);
    Some(
        local
            .from_local_datetime(&naive)
            .earliest()?
            .with_timezone(&Utc),
    )
}

/// 一行结果：标签（副标题）、值（标题，回车复制）、右侧时区偏移。
struct Row {
    key: String,
    label: String,
    value: String,
    offset: Option<String>,
    iso: String,
    unix: i64,
}

impl Row {
    fn into_item(self, score: u32) -> Item {
        Item {
            id: format!("time:{}", self.key),
            title: self.value.clone(),
            subtitle: self.label,
            badge: tr("时间", "Time").into(),
            score,
            icon: ICON,
            accessories: self
                .offset
                .map(|text| Accessory::Tag {
                    text,
                    tone: Tone::Neutral,
                })
                .into_iter()
                .collect(),
            action: Action::copy(self.value),
            action_title: tr("复制", "Copy").into(),
            actions: vec![
                NamedAction::new(
                    tr("复制 ISO 8601", "Copy ISO 8601"),
                    Some("⌘↵"),
                    Action::copy(self.iso),
                ),
                NamedAction::new(
                    tr("复制 Unix 时间戳", "Copy Unix Timestamp"),
                    Some("⌘⇧C"),
                    Action::copy(self.unix.to_string()),
                ),
            ],
            ..Item::blank()
        }
    }
}

fn rows(request: &Request, now: DateTime<Utc>) -> Vec<Row> {
    let local = local_zone();
    match *request {
        Request::Now => {
            let mut rows = instant_rows(now, local);
            for zone in zones::COMMON {
                if *zone != local {
                    rows.push(zone_row(now, *zone));
                }
            }
            rows
        }
        Request::Zone(zone) => {
            let mut rows = vec![zone_row(now, zone)];
            if zone != local {
                rows.push(local_row(now, local));
            }
            rows.push(utc_row(now));
            rows
        }
        Request::Timestamp { at, millis } => {
            let mut rows = vec![
                local_row(at, local),
                utc_row(at),
                iso_row(at, local),
                relative_row(at, now),
            ];
            rows.push(if millis {
                unix_row(at)
            } else {
                unix_ms_row(at)
            });
            rows
        }
        Request::Convert { at, from, to } => {
            let mut rows = Vec::new();
            match to {
                Target::Zone(zone) => rows.push(zone_row(at, zone)),
                Target::Unix => rows.push(unix_row(at)),
                Target::Local => {}
            }
            if from != local && to != Target::Zone(from) {
                rows.push(zone_row(at, from));
            }
            rows.push(local_row(at, local));
            rows.push(utc_row(at));
            if to != Target::Unix {
                rows.push(unix_row(at));
            }
            rows.push(relative_row(at, now));
            rows
        }
        Request::Relative(at) => {
            let mut rows = vec![local_row(at, local), relative_row(at, now)];
            rows.extend(instant_rows(at, local).into_iter().skip(1));
            rows
        }
    }
}

/// 某个时刻的一组标准写法：本地、UTC、ISO、Unix 秒、Unix 毫秒。
fn instant_rows(at: DateTime<Utc>, local: Tz) -> Vec<Row> {
    vec![
        local_row(at, local),
        utc_row(at),
        iso_row(at, local),
        unix_row(at),
        unix_ms_row(at),
    ]
}

fn base(at: DateTime<Utc>, local: Tz) -> (String, i64) {
    (iso(at, local), at.timestamp())
}

fn iso(at: DateTime<Utc>, zone: Tz) -> String {
    at.with_timezone(&zone)
        .format("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}

fn weekday_name(day: Weekday) -> &'static str {
    let names = [
        ("星期一", "Mon"),
        ("星期二", "Tue"),
        ("星期三", "Wed"),
        ("星期四", "Thu"),
        ("星期五", "Fri"),
        ("星期六", "Sat"),
        ("星期日", "Sun"),
    ];
    let (zh, en) = names
        .get(day.num_days_from_monday() as usize)
        .copied()
        .unwrap_or(("", ""));
    tr(zh, en)
}

/// `UTC+8`、`UTC+5:30`、`UTC-5`。
fn offset_label(at: DateTime<Utc>, zone: Tz) -> String {
    let seconds = at.with_timezone(&zone).offset().fix().local_minus_utc();
    let sign = if seconds < 0 { '-' } else { '+' };
    let (hours, minutes) = (seconds.abs() / 3600, seconds.abs() % 3600 / 60);
    if minutes == 0 {
        format!("UTC{sign}{hours}")
    } else {
        format!("UTC{sign}{hours}:{minutes:02}")
    }
}

fn clock(at: DateTime<Utc>, zone: Tz) -> String {
    let time = at.with_timezone(&zone);
    format!(
        "{} {}",
        time.format("%Y-%m-%d %H:%M:%S"),
        weekday_name(time.weekday())
    )
}

fn local_row(at: DateTime<Utc>, local: Tz) -> Row {
    let (iso, unix) = base(at, local);
    Row {
        key: "local".into(),
        label: trf!(
            "本地时间 · {}",
            "Local time · {}",
            zones::display_name(local)
        ),
        value: clock(at, local),
        offset: Some(offset_label(at, local)),
        iso,
        unix,
    }
}

fn zone_row(at: DateTime<Utc>, zone: Tz) -> Row {
    let (iso, unix) = base(at, zone);
    Row {
        key: format!("zone:{}", zone.name()),
        label: zones::display_name(zone),
        value: clock(at, zone),
        offset: Some(offset_label(at, zone)),
        iso,
        unix,
    }
}

fn utc_row(at: DateTime<Utc>) -> Row {
    let (iso, unix) = base(at, Tz::UTC);
    Row {
        key: "utc".into(),
        label: "UTC".into(),
        value: format!("{} UTC", at.format("%Y-%m-%d %H:%M:%S")),
        offset: Some("UTC+0".into()),
        iso,
        unix,
    }
}

fn iso_row(at: DateTime<Utc>, local: Tz) -> Row {
    let (iso, unix) = base(at, local);
    Row {
        key: "iso".into(),
        label: "ISO 8601".into(),
        value: iso.clone(),
        offset: None,
        iso,
        unix,
    }
}

fn unix_row(at: DateTime<Utc>) -> Row {
    let (iso, unix) = base(at, Tz::UTC);
    Row {
        key: "unix".into(),
        label: tr("Unix 时间戳（秒）", "Unix timestamp (seconds)").into(),
        value: unix.to_string(),
        offset: None,
        iso,
        unix,
    }
}

fn unix_ms_row(at: DateTime<Utc>) -> Row {
    let (iso, unix) = base(at, Tz::UTC);
    Row {
        key: "unix-ms".into(),
        label: tr("Unix 时间戳（毫秒）", "Unix timestamp (milliseconds)").into(),
        value: at.timestamp_millis().to_string(),
        offset: None,
        iso,
        unix,
    }
}

fn relative_row(at: DateTime<Utc>, now: DateTime<Utc>) -> Row {
    let (iso, unix) = base(at, local_zone());
    Row {
        key: "relative".into(),
        label: tr("距现在", "Relative to now").into(),
        value: humanize(at - now),
        offset: None,
        iso,
        unix,
    }
}

/// 「3 天后」「2 年前」「刚刚」。按最大的整单位取整。
fn humanize(delta: Duration) -> String {
    let seconds = delta.num_seconds();
    let magnitude = seconds.unsigned_abs();
    if magnitude < 60 {
        return tr("现在", "now").into();
    }
    let units: [(u64, &str, &str); 6] = [
        (365 * 86400, "年", "year"),
        (30 * 86400, "个月", "month"),
        (7 * 86400, "周", "week"),
        (86400, "天", "day"),
        (3600, "小时", "hour"),
        (60, "分钟", "minute"),
    ];
    let (size, zh, en) = units
        .iter()
        .find(|(size, ..)| magnitude >= *size)
        .copied()
        .unwrap_or((60, "分钟", "minute"));
    let count = magnitude / size;
    let plural = if count == 1 { "" } else { "s" };
    match (seconds > 0, crate::i18n::is_en()) {
        (true, false) => format!("{count} {zh}后"),
        (false, false) => format!("{count} {zh}前"),
        (true, true) => format!("in {count} {en}{plural}"),
        (false, true) => format!("{count} {en}{plural} ago"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-10T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn timestamps_in_seconds_and_millis_are_recognized() {
        // 日志里的时间戳有秒也有毫秒，判断错一位就差一千倍。
        assert_eq!(
            timestamp("1731234567"),
            Some(Request::Timestamp {
                at: at("2024-11-10T10:29:27Z"),
                millis: false
            })
        );
        assert_eq!(
            timestamp("1731234567890"),
            Some(Request::Timestamp {
                at: at("2024-11-10T10:29:27.890Z"),
                millis: true
            })
        );
        assert_eq!(timestamp("2026"), None, "年份不是时间戳");
        assert_eq!(timestamp("13812345678"), None, "11 位是手机号");
    }

    #[test]
    fn converts_between_zones_with_daylight_saving() {
        // 11 月纽约已是冬令时（UTC-5），洛杉矶 UTC-8：夏令时算错，约会就错一小时。
        let now = fixed();
        let Some(Request::Convert { at: when, to, .. }) = conversion("3pm 北京 in 纽约", now)
        else {
            panic!("应能解析");
        };
        assert_eq!(when, at("2026-11-10T07:00:00Z"));
        let Target::Zone(zone) = to else { panic!() };
        assert_eq!(clock(when, zone), "2026-11-10 02:00:00 星期二");
        let Some(Request::Convert { at: when, .. }) =
            conversion("2026-11-10 10:00 pst to beijing", now)
        else {
            panic!("应能解析");
        };
        assert_eq!(when, at("2026-11-10T18:00:00Z"));
        let Some(Request::Convert { at: when, to, .. }) =
            conversion("2026-07-01 10:00 pst to unix", now)
        else {
            panic!("应能解析");
        };
        assert_eq!(to, Target::Unix);
        assert_eq!(
            when,
            at("2026-07-01T17:00:00Z"),
            "七月的太平洋时间是 PDT（UTC-7）"
        );
    }

    #[test]
    fn relative_times_count_from_now() {
        // 「3 天后」「下周五」都以 HELMP_NOW 为准，这样测试和用户看到的一致。
        let now = fixed();
        assert_eq!(
            relative("in 3 days", now),
            Some(Request::Relative(at("2026-11-13T12:00:00Z")))
        );
        assert_eq!(
            relative("3天后", now),
            Some(Request::Relative(at("2026-11-13T12:00:00Z")))
        );
        assert_eq!(
            relative("2 hours ago", now),
            Some(Request::Relative(at("2026-11-10T10:00:00Z")))
        );
        assert_eq!(
            relative("+90m", now),
            Some(Request::Relative(at("2026-11-10T13:30:00Z")))
        );
        assert_eq!(relative("hello", now), None);
        assert_eq!(humanize(Duration::days(3)), "3 天后");
        assert_eq!(humanize(Duration::days(-800)), "2 年前");
    }

    #[test]
    fn zone_names_in_chinese_and_english_resolve() {
        // 用户说「东京时间」「time tokyo」「pst」，都要落到同一个 IANA 时区。
        for (text, zone) in [
            ("东京时间", "Asia/Tokyo"),
            ("time tokyo", "Asia/Tokyo"),
            ("tokyo time", "Asia/Tokyo"),
            ("纽约现在几点", "America/New_York"),
            ("time in london", "Europe/London"),
            ("time europe/paris", "Europe/Paris"),
        ] {
            let Some((Request::Zone(found), score)) = zone_query(text) else {
                panic!("{text:?} 应能解析");
            };
            assert_eq!(found.name(), zone, "{text:?}");
            assert_eq!(score, SCORE, "{text:?}");
        }
        let Some((_, score)) = zone_query("tokyo") else {
            panic!()
        };
        assert_eq!(score, BARE_CITY_SCORE, "只打城市名不该压过同名应用");
        assert!(zone_query("wx").is_none());
        assert!(zone_query("微信").is_none());
    }

    #[test]
    fn plain_searches_produce_no_time_rows() {
        // 普通搜索词不能冒出一堆时间行。
        let now = fixed();
        for text in ["wx", "微信", "safari", "2026", "3pm", "c++", "1password"] {
            assert!(parse(text, now).is_none(), "{text:?}");
        }
    }

    #[test]
    fn offsets_read_like_utc_plus() {
        let when = fixed();
        assert_eq!(
            offset_label(when, "Asia/Kolkata".parse().unwrap()),
            "UTC+5:30"
        );
        assert_eq!(
            offset_label(when, "America/New_York".parse().unwrap()),
            "UTC-5"
        );
        assert_eq!(offset_label(when, Tz::Asia__Shanghai), "UTC+8");
    }

    #[test]
    fn clock_icon_exists() {
        use gpui_kit::AssetSource as _;
        let Icon::Symbol { name, .. } = ICON else {
            unreachable!()
        };
        assert!(
            gpui_kit::assets::AllAssets
                .load(&format!("icons/{name}.svg"))
                .unwrap()
                .is_some()
        );
    }
}
