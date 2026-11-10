//! 计算器：输入算式，结果直接排在第一条，回车复制结果。
//!
//! - 支持 `+ - * / % ^ **`、括号、`sqrt` `sin` `ln` `pow` `min` `max` 等函数与 `pi` `e`；
//!   全角符号、中文括号、`×÷` 先规范化成半角（见 `parser::normalize`）。
//! - 只有输入「像算式」时才出结果或错误行（见 `parser::evaluate`）：`wx`、`微信`、`2026` 这类搜索词什么都不出。
//! - 错误细分：除以零、表达式不完整、超出范围、超出定义域、未知函数、参数个数不对、写法有误，写在副标题里。
//! - 不是算式时再看是不是单位或货币换算（`10 km to mi`、`3斤`、`100 usd to cny`，见 `units`），
//!   汇率来自 ECB 并缓存在本地（见 `currency`）；输入「汇率」列出常用币对。

mod currency;
mod parser;
mod units;

use crate::i18n::{tr, trf};
use crate::matcher;
use crate::provider::{Action, Icon, Item, NamedAction, Outcome, Provider, Tint};
use parser::CalcError;
use units::{Category, Unit};

const ICON: Icon = Icon::Symbol {
    name: "calculator",
    tint: Tint::Orange,
};
const ERROR_ICON: Icon = Icon::Symbol {
    name: "calculator",
    tint: Tint::Red,
};
const UNIT_ICON: Icon = Icon::Symbol {
    name: "ruler",
    tint: Tint::Orange,
};
const FX_ICON: Icon = Icon::Symbol {
    name: "banknote",
    tint: Tint::Green,
};
/// 单位换算结果的有效数字（`6.21371 mi`）：再多就是噪音。
const UNIT_SIGNIFICANT: i32 = 6;
/// 高于任何文字匹配（完全相等 1000）：输入是算式时，答案就是用户要的第一条。
const SCORE: u32 = 1100;
/// 有效数字位数：15 位以内 f64 是精确的，0.1+0.2 的残差在第 17 位，这样能显示成 0.3。
const SIGNIFICANT: i32 = 15;
/// 绝对值达到这么大，或小于 `TINY` 时改用科学计数，避免一长串数字或 0.000…。
const HUGE: f64 = 1e15;
const TINY: f64 = 1e-9;

fn badge() -> &'static str {
    tr("计算器", "Calculator")
}

pub struct Calculator;

impl Provider for Calculator {
    fn id(&self) -> &'static str {
        "calculator"
    }

    fn start(&self) -> anyhow::Result<()> {
        currency::start();
        Ok(())
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let expression = expression(query);
        if let Some(result) = parser::evaluate(&expression) {
            // `rgb(255, 136, 0)`、`hsl(…)` 是颜色写法（颜色插件会转换），不是写错的函数。
            if let Err(CalcError::UnknownFunction(name)) = &result
                && ["rgb", "rgba", "hsl", "hsla"].contains(&name.as_str())
            {
                return Vec::new();
            }
            return vec![match result {
                Ok(value) => result_item(&expression, &format_number(value)),
                Err(err) => error_item(&expression, &describe(&err)),
            }];
        }
        if let Some(request) = units::parse(&expression) {
            return conversion_items(&request);
        }
        exchange_rate_items(query)
    }
}

/// 规范化后的算式：去掉首尾空白，以及开头或结尾的 `=`（`=1+2`、`1+2=` 都常见）。
fn expression(query: &str) -> String {
    let text = parser::normalize(query.trim());
    let text = text.trim();
    let text = text.strip_prefix('=').unwrap_or(text);
    let text = text.strip_suffix('=').unwrap_or(text);
    text.trim().to_string()
}

fn result_item(expression: &str, result: &str) -> Item {
    Item {
        id: "calculator:result".into(),
        title: result.to_string(),
        subtitle: format!("{expression} ="),
        badge: badge().into(),
        score: SCORE,
        icon: ICON,
        action: Action::copy(result),
        action_title: tr("复制结果", "Copy Result").into(),
        actions: vec![NamedAction::new(
            tr("复制算式与结果", "Copy Expression and Result"),
            Some("⌘⇧C"),
            Action::copy(format!("{expression} = {result}")),
        )],
        ..Item::blank()
    }
}

/// 错误行：标题是算式，副标题说清楚错在哪。回车只把说明再展示一次，不复制任何东西。
fn error_item(expression: &str, message: &str) -> Item {
    let shown = message.to_string();
    Item {
        id: "calculator:error".into(),
        title: expression.to_string(),
        subtitle: message.to_string(),
        badge: badge().into(),
        score: SCORE,
        icon: ERROR_ICON,
        action: Action::new(move || Ok(Outcome::Message(shown.clone()))).keep_open(),
        action_title: tr("无法计算", "Can’t Calculate").into(),
        ..Item::blank()
    }
}

/// 单位换算：每个目标单位一行，第一条分数最高。数据单位写了目标时另给一行 1024 进制。
fn conversion_items(request: &units::Request) -> Vec<Item> {
    if request.from.category == Category::Currency {
        return currency_items(request);
    }
    let mut items = Vec::new();
    for to in request.targets() {
        let Some(value) = units::convert(request.amount, request.from, to) else {
            continue;
        };
        let rank = items.len() as u32;
        items.push(unit_item(request, to, value, None, rank));
        if request.to.is_some()
            && let Some(binary) = units::convert_binary(request.amount, request.from, to)
            && binary != value
        {
            let note = tr("按 1024 进制", "binary, 1024-based");
            items.push(unit_item(request, to, binary, Some(note), rank + 1));
        }
    }
    items
}

fn unit_item(
    request: &units::Request,
    to: &Unit,
    value: f64,
    note: Option<&str>,
    rank: u32,
) -> Item {
    let number = format_significant(value, UNIT_SIGNIFICANT);
    let with_unit = format!("{number} {}", to.symbol);
    let source = format!("{} {}", format_number(request.amount), request.from.symbol);
    let subtitle = match note {
        Some(note) => format!("{source} · {note}"),
        None => format!("{source} ="),
    };
    let id = match note {
        Some(_) => format!("calculator:unit:{}-1024", to.key),
        None => format!("calculator:unit:{}", to.key),
    };
    Item {
        id,
        title: with_unit.clone(),
        subtitle,
        badge: tr("单位换算", "Units").into(),
        score: SCORE.saturating_sub(rank),
        icon: UNIT_ICON,
        action: Action::copy(number),
        action_title: tr("复制数值", "Copy Value").into(),
        actions: vec![
            NamedAction::new(
                tr("复制带单位的结果", "Copy with Unit"),
                Some("⌘↵"),
                Action::copy(with_unit.clone()),
            ),
            NamedAction::new(
                tr("复制换算式", "Copy Conversion"),
                Some("⌘⇧C"),
                Action::copy(format!("{source} = {with_unit}")),
            ),
        ],
        ..Item::blank()
    }
}

/// 货币换算：有汇率就逐个列出，副标题写汇率日期；没有汇率就给一条说明原因的行，回车立即联网更新。
fn currency_items(request: &units::Request) -> Vec<Item> {
    let source = format!("{} {}", format_number(request.amount), request.from.symbol);
    let Some(rates) = currency::snapshot() else {
        let targets: Vec<_> = request.targets().iter().map(|u| u.symbol).collect();
        return vec![no_rates_item(&format!(
            "{source} → {}",
            targets.join(" / ")
        ))];
    };
    let stamp = rates_stamp(&rates);
    let mut items = Vec::new();
    for to in request.targets() {
        let rank = items.len() as u32;
        let Some(value) = rates.convert(request.amount, request.from.key, to.key) else {
            let missing = [request.from.key, to.key]
                .into_iter()
                .find(|code| !rates.rates.contains_key(*code))
                .unwrap_or(to.key);
            let message = trf!(
                "汇率表里没有 {missing}（{}）",
                "No rate for {missing} ({})",
                rates.source
            );
            let shown = message.clone();
            items.push(Item {
                id: format!("calculator:fx:{}", to.key),
                title: format!("{source} → {}", to.symbol),
                subtitle: message,
                badge: tr("汇率", "Currency").into(),
                score: SCORE.saturating_sub(rank),
                icon: FX_ICON,
                action: Action::new(move || Ok(Outcome::Message(shown.clone()))).keep_open(),
                action_title: tr("无法换算", "Can’t Convert").into(),
                ..Item::blank()
            });
            continue;
        };
        let number = format_money(value);
        let with_code = format!("{number} {}", to.symbol);
        items.push(Item {
            id: format!("calculator:fx:{}", to.key),
            title: with_code.clone(),
            subtitle: format!("{source} · {stamp}"),
            badge: tr("汇率", "Currency").into(),
            score: SCORE.saturating_sub(rank),
            icon: FX_ICON,
            action: Action::copy(number),
            action_title: tr("复制数值", "Copy Value").into(),
            actions: vec![
                NamedAction::new(
                    tr("复制带币种的结果", "Copy with Currency"),
                    Some("⌘↵"),
                    Action::copy(with_code.clone()),
                ),
                NamedAction::new(
                    tr("复制换算式", "Copy Conversion"),
                    Some("⌘⇧C"),
                    Action::copy(format!("{source} = {with_code}")),
                ),
                NamedAction::new(
                    tr("更新汇率", "Update Exchange Rates"),
                    None,
                    update_rates_action(),
                ),
            ],
            ..Item::blank()
        });
    }
    items
}

/// 「汇率更新于 2026-11-09（ECB）」，上次更新失败时注明正在用缓存。
fn rates_stamp(rates: &currency::Rates) -> String {
    let stamp = trf!(
        "汇率更新于 {}（{}）",
        "Rates as of {} ({})",
        rates.date,
        rates.source
    );
    match currency::last_error() {
        Some(_) => format!(
            "{stamp} · {}",
            tr("更新失败，使用缓存", "update failed, using cache")
        ),
        None => stamp,
    }
}

fn no_rates_item(title: &str) -> Item {
    let subtitle = match currency::last_error() {
        Some(err) => trf!(
            "没有汇率数据，联网后自动获取（{err}）",
            "No exchange-rate data yet; it’s fetched once you’re online ({err})"
        ),
        None => tr(
            "没有汇率数据，联网后自动获取",
            "No exchange-rate data yet; it’s fetched once you’re online",
        )
        .into(),
    };
    Item {
        id: "calculator:fx-unavailable".into(),
        title: title.to_string(),
        subtitle,
        badge: tr("汇率", "Currency").into(),
        score: SCORE,
        icon: FX_ICON,
        action: update_rates_action(),
        action_title: tr("更新汇率", "Update Exchange Rates").into(),
        ..Item::blank()
    }
}

/// 立即联网更新汇率，结果用一句话告诉用户（成功写日期，失败写原因）。
fn update_rates_action() -> Action {
    Action::new(|| {
        Ok(Outcome::Message(match currency::refresh() {
            Ok(rates) => trf!(
                "汇率已更新：{}（{}）",
                "Exchange rates updated: {} ({})",
                rates.date,
                rates.source
            ),
            Err(err) => trf!(
                "汇率更新失败：{err:#}",
                "Couldn’t update exchange rates: {err:#}"
            ),
        }))
    })
}

/// 「汇率」命令：列出常用外币对人民币的汇率和「更新汇率」。
fn exchange_rate_items(query: &str) -> Vec<Item> {
    const KEYWORDS: &[&str] = &["汇率", "货币换算", "exchange rate", "currency", "fx"];
    // 至少两个字：只打一个 `f`、`h` 不该冒出整组汇率。
    if query.trim().chars().count() < 2 {
        return Vec::new();
    }
    let Some(score) = KEYWORDS
        .iter()
        .filter_map(|keyword| matcher::score(query, keyword))
        .max()
        .filter(|score| *score >= 700)
    else {
        return Vec::new();
    };
    let Some(rates) = currency::snapshot() else {
        return vec![Item {
            score,
            ..no_rates_item(tr("汇率", "Exchange Rates"))
        }];
    };
    let stamp = rates_stamp(&rates);
    let mut items: Vec<Item> = [
        ("USD", 1.0),
        ("EUR", 1.0),
        ("JPY", 100.0),
        ("HKD", 1.0),
        ("GBP", 1.0),
    ]
    .into_iter()
    .filter_map(|(code, amount)| {
        let value = rates.convert(amount, code, "CNY")?;
        let text = format!(
            "{} {code} = {} CNY",
            format_number(amount),
            format_money(value)
        );
        Some(Item {
            id: format!("calculator:fx-pair:{code}"),
            title: text.clone(),
            subtitle: stamp.clone(),
            badge: tr("汇率", "Currency").into(),
            score,
            icon: FX_ICON,
            action: Action::copy(text),
            action_title: tr("复制", "Copy").into(),
            ..Item::blank()
        })
    })
    .collect();
    items.push(Item {
        id: "calculator:fx-update".into(),
        title: tr("更新汇率", "Update Exchange Rates").into(),
        subtitle: stamp,
        badge: tr("汇率", "Currency").into(),
        score: score.saturating_sub(1),
        icon: FX_ICON,
        action: update_rates_action(),
        action_title: tr("更新", "Update").into(),
        ..Item::blank()
    });
    items
}

/// 金额：一元以上保留两位小数，零点几的保留四位有效数字。
fn format_money(value: f64) -> String {
    if value.abs() >= 1.0 {
        format!("{value:.2}")
    } else {
        format_significant(value, 4)
    }
}

fn describe(err: &CalcError) -> String {
    match err {
        CalcError::DivisionByZero => tr("除数不能为零", "Can’t divide by zero").into(),
        CalcError::Incomplete => tr(
            "表达式不完整：缺少数字或右括号",
            "Incomplete expression: a number or closing parenthesis is missing",
        )
        .into(),
        CalcError::OutOfRange => tr("结果超出计算范围", "The result is out of range").into(),
        CalcError::Domain => tr(
            "超出函数定义域（如负数开平方、负数取对数）",
            "Outside the function’s domain (e.g. square root or log of a negative number)",
        )
        .into(),
        CalcError::UnknownFunction(name) => trf!(
            "未知函数 {name}（支持 sqrt、sin、cos、tan、ln、log、pow、min、max 等）",
            "Unknown function {name} (try sqrt, sin, cos, tan, ln, log, pow, min, max…)"
        ),
        CalcError::WrongArity { name, expected } => trf!(
            "{name} 需要 {expected} 个参数",
            "{name} takes {expected} argument(s)"
        ),
        CalcError::Syntax => tr("表达式有误", "Invalid expression").into(),
    }
}

/// 结果的显示：不加千分位；按 15 位有效数字取整去掉浮点残差（0.1+0.2 → 0.3）；
/// 特别大或特别小用科学计数（`1.5e20`，可以直接复制回来再算）。
fn format_number(value: f64) -> String {
    format_significant(value, SIGNIFICANT)
}

/// 按 `significant` 位有效数字显示（规则同 `format_number`）。
fn format_significant(value: f64, significant: i32) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let magnitude = value.abs();
    if !(TINY..HUGE).contains(&magnitude) {
        let text = format!("{:.*e}", (significant - 4).max(1) as usize, value);
        let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
        return format!("{}e{exponent}", trim_zeros(mantissa));
    }
    let integer_digits = magnitude.log10().floor() as i32 + 1;
    let decimals = (significant - integer_digits).max(0) as usize;
    // 走到这里绝对值不小于 TINY，15 位有效数字之内不会被取整成「-0」。
    trim_zeros(&format!("{value:.decimals$}")).to_string()
}

fn trim_zeros(text: &str) -> &str {
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(query: &str) -> Option<Item> {
        Calculator.query(query).into_iter().next()
    }

    #[test]
    fn answers_appear_as_the_first_result() {
        // 引擎按分数排序：答案必须高过任何文字匹配（完全相等 1000），否则会被同名应用压下去。
        for (query, expected) in [
            ("1+2*3", "7"),
            ("(4-1)^2", "9"),
            ("2**10", "1024"),
            ("10%3", "1"),
            ("sqrt(2)", "1.4142135623731"),
            ("sin(pi/2)", "1"),
            ("1e3/7", "142.857142857143"),
            ("（１＋２）×３", "9"),
            ("1+2=", "3"),
            ("=6÷4", "1.5"),
        ] {
            let item = top(query).unwrap_or_else(|| panic!("{query:?} 应出结果"));
            assert_eq!(item.id, "calculator:result", "{query:?}");
            assert_eq!(item.title, expected, "{query:?}");
            assert!(item.score > 1000);
        }
    }

    #[test]
    fn numbers_read_like_a_calculator() {
        // 用户复制结果后要直接粘进别处：浮点残差、千分位、二十位长串都会让结果没法用。
        assert_eq!(format_number(0.1 + 0.2), "0.3");
        assert_eq!(format_number(1234567.0), "1234567");
        assert_eq!(format_number(-2.5), "-2.5");
        assert_eq!(format_number(1e20), "1e20");
        assert_eq!(format_number(2f64.powi(60)), "1.15292150461e18");
        assert_eq!(format_number(1.5e-12), "1.5e-12");
        assert_eq!(format_number(-1e-20 + 0.0), "-1e-20");
        assert_eq!(format_number(123456789012345.0), "123456789012345");
        // 科学计数的结果能被计算器原样读回去。
        let item = top("1e20*3").unwrap();
        assert_eq!(item.title, "3e20");
        assert_eq!(top(&format!("{}/3", item.title)).unwrap().title, "1e20");
    }

    #[test]
    fn plain_search_terms_produce_no_rows() {
        // 普通搜索词既不能被结果顶到前面，也不能冒出一条「表达式有误」的错误行。
        for query in [
            "wx",
            "微信",
            "1password",
            "2026",
            "Safari",
            "c++",
            "pi",
            "  ",
        ] {
            assert!(Calculator.query(query).is_empty(), "{query:?}");
        }
    }

    #[test]
    fn errors_explain_themselves_in_the_subtitle() {
        // 错误行的标题是算式本身，原因写在副标题；回车不能把错误文字当结果复制走。
        for (query, needle) in [
            ("1/0", "除数不能为零"),
            ("1+", "表达式不完整"),
            ("10^400", "超出计算范围"),
            ("foo(2)", "未知函数 foo"),
            ("sqrt(-4)", "定义域"),
        ] {
            let item = top(query).unwrap_or_else(|| panic!("{query:?} 应出错误行"));
            assert_eq!(item.id, "calculator:error");
            assert_eq!(item.title, query);
            assert!(
                item.subtitle.contains(needle),
                "{query:?}: {}",
                item.subtitle
            );
            assert_eq!(
                item.action.run().unwrap(),
                Outcome::Message(item.subtitle.clone())
            );
        }
    }

    #[test]
    fn english_ui_explains_errors_in_english() {
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let item = top("1/0").unwrap();
            assert_eq!(item.subtitle, "Can’t divide by zero");
            assert_eq!(item.badge, "Calculator");
        });
    }

    #[test]
    fn symbol_icons_exist_in_bundled_assets() {
        // 图标名拼错时界面只会画一个空白方块，不会报错。
        use gpui_kit::AssetSource as _;
        for icon in [ICON, ERROR_ICON, UNIT_ICON, FX_ICON] {
            let Icon::Symbol { name, .. } = icon else {
                unreachable!()
            };
            let path = format!("icons/{name}.svg");
            assert!(gpui_kit::assets::AllAssets.load(&path).unwrap().is_some());
        }
    }
}
