//! 城市名、缩写 → IANA 时区。中文名、英文名、常见缩写都登记在这里；也可以直接写 IANA 名（`europe/paris`）。
//!
//! 缩写取用户通常的意思而不是严格定义：`pst` 指太平洋时间（夏天自动是 PDT），`est` 指美东时间。
//! `cst` 有歧义（中国标准时间 / 美国中部时间），不登记。

use chrono_tz::Tz;

use crate::i18n::tr;

/// (IANA 名, 中文显示名, 英文显示名, 别名（小写）)
const CITIES: &[(&str, &str, &str, &[&str])] = &[
    (
        "Asia/Shanghai",
        "北京",
        "Beijing",
        &[
            "北京", "上海", "中国", "beijing", "shanghai", "china", "bjt", "深圳", "广州", "杭州",
            "成都",
        ],
    ),
    (
        "Asia/Hong_Kong",
        "香港",
        "Hong Kong",
        &["香港", "hong kong", "hongkong", "hk", "hkt"],
    ),
    (
        "Asia/Taipei",
        "台北",
        "Taipei",
        &["台北", "台湾", "taipei", "taiwan"],
    ),
    (
        "Asia/Tokyo",
        "东京",
        "Tokyo",
        &["东京", "日本", "大阪", "tokyo", "japan", "osaka", "jst"],
    ),
    (
        "Asia/Seoul",
        "首尔",
        "Seoul",
        &["首尔", "韩国", "seoul", "korea", "kst"],
    ),
    (
        "Asia/Singapore",
        "新加坡",
        "Singapore",
        &["新加坡", "singapore", "sgt"],
    ),
    (
        "Asia/Bangkok",
        "曼谷",
        "Bangkok",
        &["曼谷", "泰国", "bangkok", "thailand"],
    ),
    (
        "Asia/Kolkata",
        "孟买",
        "Mumbai",
        &[
            "孟买",
            "新德里",
            "印度",
            "mumbai",
            "delhi",
            "new delhi",
            "india",
            "ist",
            "kolkata",
        ],
    ),
    ("Asia/Dubai", "迪拜", "Dubai", &["迪拜", "dubai", "gst"]),
    (
        "Europe/London",
        "伦敦",
        "London",
        &["伦敦", "英国", "london", "uk", "bst"],
    ),
    (
        "Europe/Paris",
        "巴黎",
        "Paris",
        &["巴黎", "法国", "paris", "france", "cet", "cest"],
    ),
    (
        "Europe/Berlin",
        "柏林",
        "Berlin",
        &["柏林", "德国", "berlin", "germany"],
    ),
    (
        "Europe/Moscow",
        "莫斯科",
        "Moscow",
        &["莫斯科", "俄罗斯", "moscow", "russia", "msk"],
    ),
    (
        "America/New_York",
        "纽约",
        "New York",
        &[
            "纽约",
            "华盛顿",
            "美东",
            "new york",
            "nyc",
            "ny",
            "washington",
            "boston",
            "est",
            "edt",
            "et",
        ],
    ),
    (
        "America/Chicago",
        "芝加哥",
        "Chicago",
        &["芝加哥", "chicago", "cdt"],
    ),
    (
        "America/Denver",
        "丹佛",
        "Denver",
        &["丹佛", "denver", "mst", "mdt"],
    ),
    (
        "America/Los_Angeles",
        "旧金山",
        "San Francisco",
        &[
            "旧金山",
            "洛杉矶",
            "硅谷",
            "西雅图",
            "美西",
            "san francisco",
            "sf",
            "los angeles",
            "la",
            "seattle",
            "silicon valley",
            "pst",
            "pdt",
            "pt",
        ],
    ),
    (
        "America/Toronto",
        "多伦多",
        "Toronto",
        &["多伦多", "toronto"],
    ),
    (
        "America/Vancouver",
        "温哥华",
        "Vancouver",
        &["温哥华", "vancouver"],
    ),
    (
        "America/Sao_Paulo",
        "圣保罗",
        "São Paulo",
        &["圣保罗", "巴西", "sao paulo", "são paulo", "brazil"],
    ),
    (
        "Australia/Sydney",
        "悉尼",
        "Sydney",
        &[
            "悉尼",
            "墨尔本",
            "澳大利亚",
            "sydney",
            "melbourne",
            "australia",
            "aest",
            "aedt",
        ],
    ),
    (
        "Pacific/Auckland",
        "奥克兰",
        "Auckland",
        &[
            "奥克兰",
            "新西兰",
            "auckland",
            "new zealand",
            "nzst",
            "nzdt",
        ],
    ),
    (
        "UTC",
        "UTC",
        "UTC",
        &["utc", "gmt", "z", "zulu", "世界协调时", "格林尼治"],
    ),
];

/// 「现在」下面顺带列出的常用时区（跳过本地时区）。
pub const COMMON: &[Tz] = &[
    Tz::Asia__Shanghai,
    Tz::Asia__Tokyo,
    Tz::Europe__London,
    Tz::America__New_York,
    Tz::America__Los_Angeles,
];

/// 按别名或 IANA 名找时区（精确匹配，大小写不敏感）。
pub fn lookup(name: &str) -> Option<Tz> {
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return None;
    }
    CITIES
        .iter()
        .find(|(_, _, _, aliases)| aliases.contains(&name.as_str()))
        .and_then(|(iana, ..)| iana.parse().ok())
        .or_else(|| by_iana(&name))
}

/// IANA 名（大小写不敏感，空格当下划线）。
pub fn by_iana(name: &str) -> Option<Tz> {
    let name = name.trim().replace(' ', "_");
    if !name.contains('/') && !name.eq_ignore_ascii_case("utc") {
        return None;
    }
    chrono_tz::TZ_VARIANTS
        .iter()
        .find(|tz| tz.name().eq_ignore_ascii_case(&name))
        .copied()
}

/// 给人看的名字：登记过的用城市名（「东京 · Asia/Tokyo」），否则 IANA 名。
pub fn display_name(zone: Tz) -> String {
    match CITIES.iter().find(|(iana, ..)| *iana == zone.name()) {
        Some((iana, _, _, _)) if *iana == "UTC" => "UTC".into(),
        Some((iana, zh, en, _)) => format!("{} · {iana}", tr(zh, en)),
        None => zone.name().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_city_names_a_real_zone_and_aliases_are_unique() {
        // IANA 名拼错时这个城市永远查不到；别名重复时用户得到哪个全看顺序。
        let mut seen = std::collections::HashSet::new();
        for (iana, _, _, aliases) in CITIES {
            assert!(iana.parse::<Tz>().is_ok(), "{iana}");
            for alias in *aliases {
                assert_eq!(alias.to_lowercase(), *alias);
                assert!(seen.insert(*alias), "{alias} 重复");
            }
        }
    }
}
