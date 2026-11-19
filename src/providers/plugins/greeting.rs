//! 搜索框寄语：插件在清单里声明一组文案（`greetings`），全局搜索、输入框为空时代替默认的占位文字。
//!
//! - 纯声明：文案在清单里，打开面板时只在内存里挑一条，不联网、不跑脚本、不读盘。
//! - 挑哪条：当天有按日期指定的（`YYYY-MM-DD` 优先于每年重复的 `MM-DD`）就用它，否则按 `rotation`
//!   轮换平常的 `items`：`open` 每次打开换下一条，`daily` 每天一条。日期按本机时区算；
//!   每年重复的 `02-29` 在平年落到 2 月 28 日（那天没有自己的 `02-28` 时）。
//! - 多个插件都提供时只显示一个：今天有按日期指定文案的插件优先，否则取插件 id 排序最前的那个。
//!   用户可以在设置 › 插件里按插件关掉寄语（`PluginSettings::greeting`）。
//! - 进入命令范围、正在建立应用索引时，原有的占位文字优先（见 `launcher::placeholder_for`）。

use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Datelike as _, NaiveDate, TimeZone};

use super::manifest::{Greetings, Rotation, Text};
use super::registry::{self, State};
use crate::config::Config;

/// `dates[].date` 的两种写法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateRule {
    /// `MM-DD`：每年这一天。
    Yearly { month: u32, day: u32 },
    /// `YYYY-MM-DD`：只那一天。
    Once(NaiveDate),
}

impl DateRule {
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.split('-').collect();
        let number = |part: &str, len: usize| {
            (part.len() == len && part.chars().all(|c| c.is_ascii_digit()))
                .then(|| part.parse::<u32>().ok())
                .flatten()
        };
        match parts.as_slice() {
            [month, day] => {
                let (month, day) = (number(month, 2)?, number(day, 2)?);
                // 2000 是闰年：02-29 也是合法的每年日期。
                NaiveDate::from_ymd_opt(2000, month, day)?;
                Some(DateRule::Yearly { month, day })
            }
            [year, month, day] => {
                let year = i32::try_from(number(year, 4)?).ok()?;
                NaiveDate::from_ymd_opt(year, number(month, 2)?, number(day, 2)?).map(DateRule::Once)
            }
            _ => None,
        }
    }
}

/// 本机时区下「今天」是哪天（测试里换成别的时区）。
pub fn local_date<Tz: TimeZone>(now: DateTime<chrono::Utc>, tz: &Tz) -> NaiveDate {
    now.with_timezone(tz).date_naive()
}

/// 今天按日期指定的文案：只那一天的 > 每年这一天的 > 平年 2 月 28 日补上每年 02-29 的。
fn dated_for(greetings: &Greetings, today: NaiveDate) -> Vec<&Text> {
    let rules: Vec<(Option<DateRule>, &Text)> = greetings
        .dates
        .iter()
        .map(|dated| (DateRule::parse(&dated.date), &dated.text))
        .collect();
    let matching = |want: &dyn Fn(DateRule) -> bool| -> Vec<&Text> {
        rules
            .iter()
            .filter(|(rule, _)| rule.is_some_and(want))
            .map(|(_, text)| *text)
            .collect()
    };
    let once = matching(&|rule| rule == DateRule::Once(today));
    if !once.is_empty() {
        return once;
    }
    let yearly = |month: u32, day: u32| move |rule| rule == DateRule::Yearly { month, day };
    let exact = matching(&yearly(today.month(), today.day()));
    if !exact.is_empty() {
        return exact;
    }
    let leap_fallback = today.month() == 2 && today.day() == 28 && !today.leap_year();
    if leap_fallback {
        return matching(&yearly(2, 29));
    }
    Vec::new()
}

/// 挑中的一条寄语。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chosen<'a> {
    pub text: &'a Text,
    pub rotation: Rotation,
    /// 是按日期指定的那一条。
    pub dated: bool,
}

/// 一个插件今天显示哪条：`opens` 是面板打开的次数（`rotation: open` 用），`daily` 按日期序号轮换。
pub fn pick(greetings: &Greetings, today: NaiveDate, opens: u64) -> Option<Chosen<'_>> {
    let dated = dated_for(greetings, today);
    let is_dated = !dated.is_empty();
    let pool: Vec<&Text> = if is_dated {
        dated
    } else {
        greetings.items.iter().collect()
    };
    let turn = match greetings.rotation {
        Rotation::Open => opens,
        // 从公元元年起的连续天数：跨月、跨年都只是加一，不会在 1 月 1 日跳回第一条。
        Rotation::Daily => u64::try_from(today.num_days_from_ce()).unwrap_or_default(),
    };
    let len = u64::try_from(pool.len()).ok().filter(|len| *len > 0)?;
    let index = usize::try_from(turn % len).ok()?;
    pool.get(index).map(|text| Chosen {
        text,
        rotation: greetings.rotation,
        dated: is_dated,
    })
}

/// 多个插件都提供寄语时选哪个（规则见模块说明）。`plugins` 是 (插件 id, 寄语)，顺序不限。
pub fn choose<'a>(
    plugins: &[(&str, &'a Greetings)],
    today: NaiveDate,
    opens: u64,
) -> Option<Chosen<'a>> {
    let mut sorted: Vec<&(&str, &'a Greetings)> = plugins.iter().collect();
    sorted.sort_by_key(|(id, _)| *id);
    let dated = sorted
        .iter()
        .find(|(_, greetings)| !dated_for(greetings, today).is_empty());
    match dated {
        Some((_, greetings)) => pick(greetings, today, opens),
        None => sorted
            .iter()
            .find_map(|(_, greetings)| pick(greetings, today, opens)),
    }
}

/// 用户在设置里有没有关掉这个插件的寄语（缺省开）。
pub fn enabled(config: &Config, id: &str) -> bool {
    config
        .plugins
        .get(id)
        .and_then(|settings| settings.greeting)
        .unwrap_or(true)
}

/// 面板打开次数。从启动时刻起算，免得每次启动 Helmp 后第一次打开总是第一条。
static OPENS: LazyLock<AtomicU64> = LazyLock::new(|| {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    AtomicU64::new(seed)
});

/// 上一次上报「显示了寄语」的日期：一天最多报一次。
static REPORTED: Mutex<Option<NaiveDate>> = Mutex::new(None);

/// 面板打开时调用：这一次搜索框显示哪句寄语（没有插件提供、或都被关掉时为 None）。
/// 只读内存里的插件列表和配置，开销是微秒级。
pub fn for_this_open() -> Option<String> {
    let config = crate::config::get();
    let entries = registry::current();
    let plugins: Vec<(&str, &Greetings)> = entries
        .iter()
        .filter(|entry| entry.state(&config) == State::Enabled && enabled(&config, &entry.id))
        .filter_map(|entry| {
            let plugin = entry.plugin()?;
            Some((plugin.id(), plugin.manifest.greetings.as_ref()?))
        })
        .collect();
    if plugins.is_empty() {
        return None;
    }
    let today = local_date(chrono::Utc::now(), &chrono::Local);
    let opens = OPENS.fetch_add(1, Ordering::Relaxed);
    let chosen = choose(&plugins, today, opens)?;
    let mut reported = REPORTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *reported != Some(today) {
        *reported = Some(today);
        crate::telemetry::plugin_greeting_shown(chosen.rotation, chosen.dated);
    }
    Some(chosen.text.get().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::plugins::manifest;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// 只带寄语的清单。
    fn greetings(json: &str) -> Greetings {
        let text = format!(
            r#"{{"schemaVersion": 1, "id": "com.example.quotes", "name": "Quotes", "version": "1.0.0",
                "author": "Example", "greetings": {json}}}"#
        );
        manifest::parse(&text, None)
            .unwrap_or_else(|errors| panic!("{errors:?}"))
            .greetings
            .unwrap()
    }

    fn shown(chosen: Option<Chosen<'_>>) -> String {
        chosen.map(|c| c.text.get().to_string()).unwrap_or_default()
    }

    #[test]
    fn date_rules_accept_only_the_two_documented_forms() {
        assert_eq!(
            DateRule::parse("12-25"),
            Some(DateRule::Yearly { month: 12, day: 25 })
        );
        // 闰日可以每年重复（平年怎么落见下面的测试）。
        assert_eq!(
            DateRule::parse("02-29"),
            Some(DateRule::Yearly { month: 2, day: 29 })
        );
        assert_eq!(
            DateRule::parse("2028-02-29"),
            Some(DateRule::Once(date(2028, 2, 29)))
        );
        // 不存在的日子、不补零、别的分隔符都拒绝：猜错了会在错的那天显示。
        for bad in [
            "2027-02-29",
            "02-30",
            "13-01",
            "1-5",
            "2026-1-05",
            "12/25",
            "",
            "26-12-25",
            "2026-12-25-1",
        ] {
            assert_eq!(DateRule::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn bad_greetings_are_rejected_with_readable_reasons() {
        // 写错的日期会在错的那天显示（或永远不显示），过长、换行的文案在一行的搜索框里会被截断：都在校验时拦下。
        let errors = |json: &str| {
            let text = format!(
                r#"{{"schemaVersion": 1, "id": "com.example.quotes", "name": "Quotes", "version": "1.0.0",
                    "author": "Example", "greetings": {json}}}"#
            );
            crate::i18n::with_lang(crate::i18n::Lang::En, || {
                manifest::parse(&text, None).err().unwrap_or_default().join("\n")
            })
        };
        assert!(errors(r#"{}"#).contains("greetings: needs at least one entry"));
        assert!(errors(r#"{"dates": [{"date": "2027-02-29", "text": "x"}]}"#)
            .contains("greetings.dates[0].date: \"2027-02-29\" isn't a valid date"));
        assert!(errors(r#"{"items": ["a\nb"]}"#).contains("greetings.items[0]: must not contain line breaks"));
        let long = "x".repeat(manifest::MAX_GREETING_CHARS + 1);
        assert!(errors(&format!(r#"{{"items": ["{long}"]}}"#)).contains("longer than 120"));
        assert!(errors(r#"{"rotation": "weekly", "items": ["a"]}"#).contains("weekly"));
        assert!(errors(r#"{"items": ["a"], "random": true}"#).contains("random"));
        let many: Vec<&str> = vec!["\"a\""; manifest::MAX_GREETINGS + 1];
        assert!(errors(&format!(r#"{{"items": [{}]}}"#, many.join(","))).contains("at most 1000"));
        // 只有寄语、没有命令的插件是合法的。
        assert_eq!(errors(r#"{"items": ["a"]}"#), "");
    }

    #[test]
    fn open_rotation_moves_on_every_open_and_wraps() {
        let g = greetings(r#"{"items": ["A", "B", "C"]}"#);
        let today = date(2026, 6, 1);
        let seen: Vec<String> = (0..4).map(|n| shown(pick(&g, today, n))).collect();
        assert_eq!(seen, ["A", "B", "C", "A"]);
    }

    #[test]
    fn daily_rotation_is_stable_within_a_day_and_steps_across_years() {
        let g = greetings(r#"{"rotation": "daily", "items": ["A", "B", "C"]}"#);
        // 同一天打开多少次都是同一条。
        let dec31 = date(2026, 12, 31);
        let first = shown(pick(&g, dec31, 0));
        for opens in 1..10 {
            assert_eq!(shown(pick(&g, dec31, opens)), first);
        }
        // 跨年只往后走一条，不会因为「一年中的第几天」归零而重复或跳回。
        let order = ["A", "B", "C"];
        let at = |text: &str| order.iter().position(|o| *o == text).unwrap();
        let next = shown(pick(&g, date(2027, 1, 1), 0));
        assert_eq!(at(&next), (at(&first) + 1) % 3);
        // 闰年的 12-31 → 1-1 同样。
        let leap_last = shown(pick(&g, date(2028, 12, 31), 0));
        let after = shown(pick(&g, date(2029, 1, 1), 0));
        assert_eq!(at(&after), (at(&leap_last) + 1) % 3);
    }

    #[test]
    fn dated_entries_replace_the_rotation_on_their_day() {
        let g = greetings(
            r#"{"items": ["plain"], "dates": [
                {"date": "12-25", "text": {"en": "Merry Christmas", "zh-Hans": "圣诞快乐"}},
                {"date": "2026-12-25", "text": "Only in 2026"},
                {"date": "01-01", "text": "Happy new year"}
            ]}"#,
        );
        // 只那一天的优先于每年重复的。
        let chosen = pick(&g, date(2026, 12, 25), 0).unwrap();
        assert!(chosen.dated);
        assert_eq!(chosen.text.get(), "Only in 2026");
        // 其他年份落回每年重复的那条。
        let other_year = crate::i18n::with_lang(crate::i18n::Lang::En, || {
            shown(pick(&g, date(2027, 12, 25), 0))
        });
        assert_eq!(other_year, "Merry Christmas");
        // 跨年那一天也只看本机日期。
        assert_eq!(shown(pick(&g, date(2027, 1, 1), 5)), "Happy new year");
        // 平常日子用轮换，且不算 dated。
        let plain = pick(&g, date(2027, 3, 3), 0).unwrap();
        assert!(!plain.dated);
        assert_eq!(plain.text.get(), "plain");
    }

    #[test]
    fn a_leap_day_entry_shows_on_feb_28_in_common_years() {
        let g = greetings(r#"{"items": ["plain"], "dates": [{"date": "02-29", "text": "leap"}]}"#);
        assert_eq!(shown(pick(&g, date(2028, 2, 29), 0)), "leap");
        // 闰年 2-28 还是平常的一天：不能提前一天显示。
        assert_eq!(shown(pick(&g, date(2028, 2, 28), 0)), "plain");
        // 平年没有 2-29，落到 2-28，而不是整整三年都不显示。
        assert_eq!(shown(pick(&g, date(2027, 2, 28), 0)), "leap");
        assert_eq!(shown(pick(&g, date(2027, 3, 1), 0)), "plain");
        // 那天已经有自己的 02-28 时，不再挤进来。
        let both = greetings(
            r#"{"dates": [{"date": "02-29", "text": "leap"}, {"date": "02-28", "text": "28th"}]}"#,
        );
        assert_eq!(shown(pick(&both, date(2027, 2, 28), 0)), "28th");
    }

    #[test]
    fn only_dates_and_no_match_means_no_greeting() {
        // 只有按日期的文案、今天又不是那天：没有寄语，搜索框用默认占位。
        let g = greetings(r#"{"dates": [{"date": "07-04", "text": "x"}]}"#);
        assert_eq!(pick(&g, date(2026, 7, 5), 0), None);
    }

    #[test]
    fn the_day_follows_the_local_time_zone() {
        // 2026-12-31 20:30 UTC：东八区已经是 1 月 1 日，洛杉矶还是 12 月 31 日——按日期的文案跟着本机走。
        let instant = chrono::Utc.with_ymd_and_hms(2026, 12, 31, 20, 30, 0).unwrap();
        let shanghai = local_date(instant, &chrono_tz::Asia::Shanghai);
        let los_angeles = local_date(instant, &chrono_tz::America::Los_Angeles);
        assert_eq!(shanghai, date(2027, 1, 1));
        assert_eq!(los_angeles, date(2026, 12, 31));
        let g = greetings(r#"{"items": ["plain"], "dates": [{"date": "01-01", "text": "new year"}]}"#);
        assert_eq!(shown(pick(&g, shanghai, 0)), "new year");
        assert_eq!(shown(pick(&g, los_angeles, 0)), "plain");
        // 夏令时切换那天（美国 3 月第二个周日）日期照常。
        let dst = chrono::Utc.with_ymd_and_hms(2027, 3, 14, 10, 0, 0).unwrap();
        assert_eq!(local_date(dst, &chrono_tz::America::New_York), date(2027, 3, 14));
    }

    #[test]
    fn several_plugins_show_one_and_dated_wins() {
        let a = greetings(r#"{"items": ["from a"]}"#);
        let b = greetings(r#"{"items": ["from b"], "dates": [{"date": "05-20", "text": "b today"}]}"#);
        let plugins = [("com.example.b", &b), ("com.example.a", &a)];
        // 平常：id 排序最前的插件，与传入顺序无关。
        assert_eq!(shown(choose(&plugins, date(2026, 5, 19), 0)), "from a");
        // 今天有按日期文案的插件优先。
        assert_eq!(shown(choose(&plugins, date(2026, 5, 20), 0)), "b today");
        assert_eq!(choose(&[], date(2026, 5, 20), 0), None);
        // 排前面的插件今天没有可显示的（只有别的日子的 dates）时，轮到下一个。
        let dates_only = greetings(r#"{"dates": [{"date": "09-09", "text": "x"}]}"#);
        let plugins = [("com.example.a", &dates_only), ("com.example.b", &a)];
        assert_eq!(shown(choose(&plugins, date(2026, 5, 19), 0)), "from a");
    }

    #[test]
    fn choosing_is_cheap_enough_for_every_panel_open() {
        // 打开面板时同步调用：20 个插件、每个 1000 条也要远低于一帧（16 ms）。
        let items: Vec<String> = (0..980).map(|n| format!("\"item {n}\"")).collect();
        let dates: Vec<String> = (1..=20)
            .map(|d| format!(r#"{{"date": "03-{d:02}", "text": "d{d}"}}"#))
            .collect();
        let g = greetings(&format!(
            r#"{{"items": [{}], "dates": [{}]}}"#,
            items.join(","),
            dates.join(",")
        ));
        let ids: Vec<String> = (0..20).map(|n| format!("com.example.p{n:02}")).collect();
        let plugins: Vec<(&str, &Greetings)> = ids.iter().map(|id| (id.as_str(), &g)).collect();
        let start = std::time::Instant::now();
        for opens in 0..100 {
            assert!(choose(&plugins, date(2026, 6, 1), opens).is_some());
        }
        let each = start.elapsed() / 100;
        println!("greeting choose: {each:?} per open (20 plugins × 1000 entries)");
        assert!(each < std::time::Duration::from_millis(2), "{each:?}");
    }

    #[test]
    fn the_example_plugin_validates() {
        let text = include_str!("../../../examples/plugins/com.example.daily-quote/plugin.json");
        let manifest = manifest::parse(text, None).unwrap_or_else(|errors| panic!("{errors:?}"));
        assert!(manifest.greetings.is_some());
    }
}
