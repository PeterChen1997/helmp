//! 单位与货币换算的解析：`10 km to mi`、`100f in c`、`3斤`、`1 GB = ? MB`、`100 usd to cny`、`100美元`。
//!
//! 只认「数字 + 单位 [+ 连接词 + 目标单位]」这一种形状，其余一律返回 `None`（不出结果、不出错误行），
//! 免得把 `5g`、`3 days` 这类普通搜索词顶到第一条：
//! - 写了目标单位（`5 g to oz`）：任何单位都换算。
//! - 没写目标单位：只有长度、质量、温度、体积、面积、速度、数据大小和货币会列出常用换算，
//!   而且单位要写得够明确（至少两个字母、或中文、或货币符号），`5g`、`3m` 不算。时间单位必须写目标。
//!
//! 温度不是比例换算，单独走 `convert`；货币的汇率由 `currency` 提供，这里只认货币单位。

/// 单位的类别：只有同类之间能换算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Length,
    Mass,
    Temperature,
    Volume,
    Area,
    Speed,
    Data,
    Time,
    Currency,
}

#[derive(Debug)]
pub struct Unit {
    /// 结果 id 用的 ASCII 键，如 `km`、`jin`、`floz`。货币是 ISO 代码。
    pub key: &'static str,
    /// 显示用的符号，如 `km`、`斤`、`°C`、`USD`。
    pub symbol: &'static str,
    pub category: Category,
    /// 换成本类基准单位（米、千克、升、平方米、米每秒、字节、秒）的系数；温度和货币不用。
    pub factor: f64,
    /// 数据单位按 1024 进制的系数（KB、MB…）；其他单位为 `None`。
    pub binary: Option<f64>,
    /// 用户可能写的形式（小写）。
    pub aliases: &'static [&'static str],
}

const fn unit(
    key: &'static str,
    symbol: &'static str,
    category: Category,
    factor: f64,
    aliases: &'static [&'static str],
) -> Unit {
    Unit {
        key,
        symbol,
        category,
        factor,
        binary: None,
        aliases,
    }
}

const fn data(
    key: &'static str,
    symbol: &'static str,
    factor: f64,
    binary: f64,
    aliases: &'static [&'static str],
) -> Unit {
    Unit {
        key,
        symbol,
        category: Category::Data,
        factor,
        binary: Some(binary),
        aliases,
    }
}

use Category::*;

const KIB: f64 = 1024.0;

/// 换算系数都是定义值（国际码 0.9144 m、常衡磅 0.45359237 kg、美制加仑 3.785411784 L、市制 1 斤 = 0.5 kg）。
pub static UNITS: &[Unit] = &[
    // 长度（基准：米）
    unit(
        "km",
        "km",
        Length,
        1000.0,
        &[
            "km",
            "kms",
            "kilometer",
            "kilometers",
            "kilometre",
            "kilometres",
            "公里",
            "千米",
        ],
    ),
    unit(
        "m",
        "m",
        Length,
        1.0,
        &["m", "meter", "meters", "metre", "metres", "米", "公尺"],
    ),
    unit(
        "dm",
        "dm",
        Length,
        0.1,
        &["dm", "decimeter", "decimeters", "分米"],
    ),
    unit(
        "cm",
        "cm",
        Length,
        0.01,
        &[
            "cm",
            "centimeter",
            "centimeters",
            "centimetre",
            "厘米",
            "公分",
        ],
    ),
    unit(
        "mm",
        "mm",
        Length,
        0.001,
        &["mm", "millimeter", "millimeters", "millimetre", "毫米"],
    ),
    unit(
        "um",
        "µm",
        Length,
        1e-6,
        &["µm", "μm", "um", "micron", "microns", "micrometer", "微米"],
    ),
    unit(
        "nm",
        "nm",
        Length,
        1e-9,
        &["nanometer", "nanometers", "纳米"],
    ),
    unit(
        "mi",
        "mi",
        Length,
        1609.344,
        &["mi", "mile", "miles", "英里"],
    ),
    unit(
        "yd",
        "yd",
        Length,
        0.9144,
        &["yd", "yds", "yard", "yards", "码"],
    ),
    unit(
        "ft",
        "ft",
        Length,
        0.3048,
        &["ft", "feet", "foot", "'", "英尺", "呎"],
    ),
    unit(
        "in",
        "in",
        Length,
        0.0254,
        &["in", "inch", "inches", "\"", "英寸", "吋"],
    ),
    unit(
        "nmi",
        "nmi",
        Length,
        1852.0,
        &["nmi", "nautical mile", "nautical miles", "海里"],
    ),
    unit("li", "里", Length, 500.0, &["里", "市里"]),
    unit("zhang", "丈", Length, 10.0 / 3.0, &["丈"]),
    unit("chi", "尺", Length, 1.0 / 3.0, &["尺", "市尺"]),
    unit("cun", "寸", Length, 1.0 / 30.0, &["寸", "市寸"]),
    // 质量（基准：千克）
    unit(
        "t",
        "t",
        Mass,
        1000.0,
        &["t", "ton", "tons", "tonne", "tonnes", "吨", "公吨"],
    ),
    unit(
        "kg",
        "kg",
        Mass,
        1.0,
        &[
            "kg",
            "kgs",
            "kilogram",
            "kilograms",
            "kilo",
            "kilos",
            "公斤",
            "千克",
        ],
    ),
    unit(
        "g",
        "g",
        Mass,
        0.001,
        &["g", "gram", "grams", "gramme", "克"],
    ),
    unit(
        "mg",
        "mg",
        Mass,
        1e-6,
        &["mg", "milligram", "milligrams", "毫克"],
    ),
    unit(
        "lb",
        "lb",
        Mass,
        0.45359237,
        &["lb", "lbs", "pound", "pounds", "磅"],
    ),
    unit(
        "oz",
        "oz",
        Mass,
        0.028349523125,
        &["oz", "ounce", "ounces", "盎司", "安士"],
    ),
    unit("st", "st", Mass, 6.35029318, &["stone", "stones", "英石"]),
    unit("ct", "ct", Mass, 0.0002, &["ct", "carat", "carats", "克拉"]),
    unit("jin", "斤", Mass, 0.5, &["斤", "市斤"]),
    unit("liang", "两", Mass, 0.05, &["两", "市两"]),
    unit("qian", "钱", Mass, 0.005, &["钱"]),
    // 温度（换算见 `convert`）
    unit(
        "c",
        "°C",
        Temperature,
        1.0,
        &[
            "c",
            "°c",
            "℃",
            "celsius",
            "centigrade",
            "摄氏度",
            "摄氏",
            "度",
        ],
    ),
    unit(
        "f",
        "°F",
        Temperature,
        1.0,
        &["f", "°f", "℉", "fahrenheit", "华氏度", "华氏"],
    ),
    unit(
        "k",
        "K",
        Temperature,
        1.0,
        &["k", "kelvin", "kelvins", "开尔文", "开氏度"],
    ),
    // 体积（基准：升）
    unit(
        "l",
        "L",
        Volume,
        1.0,
        &["l", "liter", "liters", "litre", "litres", "升", "公升"],
    ),
    unit(
        "ml",
        "mL",
        Volume,
        0.001,
        &["ml", "milliliter", "milliliters", "millilitre", "毫升"],
    ),
    unit(
        "m3",
        "m³",
        Volume,
        1000.0,
        &["m3", "m³", "cubic meter", "cubic meters", "立方米", "方"],
    ),
    unit(
        "cm3",
        "cm³",
        Volume,
        0.001,
        &["cm3", "cm³", "cc", "立方厘米"],
    ),
    unit(
        "gal",
        "gal",
        Volume,
        3.785411784,
        &["gal", "gallon", "gallons", "加仑", "美制加仑"],
    ),
    unit(
        "qt",
        "qt",
        Volume,
        0.946352946,
        &["qt", "quart", "quarts", "夸脱"],
    ),
    unit(
        "pt",
        "pt",
        Volume,
        0.473176473,
        &["pt", "pint", "pints", "品脱"],
    ),
    unit("cup", "cup", Volume, 0.2365882365, &["cup", "cups", "杯"]),
    unit(
        "floz",
        "fl oz",
        Volume,
        0.0295735295625,
        &["fl oz", "floz", "fluid ounce", "fluid ounces", "液量盎司"],
    ),
    unit(
        "tbsp",
        "tbsp",
        Volume,
        0.01478676478125,
        &["tbsp", "tablespoon", "tablespoons", "汤匙"],
    ),
    unit(
        "tsp",
        "tsp",
        Volume,
        0.00492892159375,
        &["tsp", "teaspoon", "teaspoons", "茶匙"],
    ),
    // 面积（基准：平方米）
    unit(
        "km2",
        "km²",
        Area,
        1e6,
        &[
            "km2",
            "km²",
            "sq km",
            "square kilometer",
            "square kilometers",
            "平方公里",
            "平方千米",
        ],
    ),
    unit(
        "m2",
        "m²",
        Area,
        1.0,
        &[
            "m2",
            "m²",
            "sqm",
            "sq m",
            "square meter",
            "square meters",
            "平方米",
            "平米",
            "平",
        ],
    ),
    unit(
        "cm2",
        "cm²",
        Area,
        1e-4,
        &[
            "cm2",
            "cm²",
            "square centimeter",
            "square centimeters",
            "平方厘米",
        ],
    ),
    unit(
        "ha",
        "ha",
        Area,
        1e4,
        &["ha", "hectare", "hectares", "公顷"],
    ),
    unit(
        "acre",
        "acre",
        Area,
        4046.8564224,
        &["acre", "acres", "英亩"],
    ),
    unit(
        "ft2",
        "ft²",
        Area,
        0.09290304,
        &[
            "ft2",
            "ft²",
            "sq ft",
            "sqft",
            "square foot",
            "square feet",
            "平方英尺",
        ],
    ),
    unit(
        "mi2",
        "mi²",
        Area,
        2589988.110336,
        &[
            "mi2",
            "mi²",
            "sq mi",
            "square mile",
            "square miles",
            "平方英里",
        ],
    ),
    unit("mu", "亩", Area, 2000.0 / 3.0, &["亩", "市亩"]),
    // 速度（基准：米每秒）
    unit(
        "kmh",
        "km/h",
        Speed,
        1000.0 / 3600.0,
        &[
            "km/h",
            "kmh",
            "kph",
            "kmph",
            "公里每小时",
            "千米每小时",
            "公里/小时",
            "时速",
        ],
    ),
    unit("ms", "m/s", Speed, 1.0, &["m/s", "mps", "米每秒", "米/秒"]),
    unit(
        "mph",
        "mph",
        Speed,
        1609.344 / 3600.0,
        &["mph", "mi/h", "英里每小时"],
    ),
    unit(
        "knot",
        "kn",
        Speed,
        1852.0 / 3600.0,
        &["kn", "knot", "knots", "节"],
    ),
    // 数据大小（基准：字节）。KB/MB… 按 1000 进制（与访达一致），KiB/MiB… 按 1024；写了目标单位时另给一行 1024 进制。
    data("b", "B", 1.0, 1.0, &["b", "byte", "bytes", "字节"]),
    data(
        "kb",
        "KB",
        1e3,
        KIB,
        &["kb", "kilobyte", "kilobytes", "千字节"],
    ),
    data(
        "mb",
        "MB",
        1e6,
        KIB * KIB,
        &["mb", "megabyte", "megabytes", "兆", "兆字节"],
    ),
    data(
        "gb",
        "GB",
        1e9,
        KIB * KIB * KIB,
        &["gb", "gigabyte", "gigabytes", "吉字节"],
    ),
    data(
        "tb",
        "TB",
        1e12,
        KIB * KIB * KIB * KIB,
        &["tb", "terabyte", "terabytes", "太字节"],
    ),
    data(
        "pb",
        "PB",
        1e15,
        KIB * KIB * KIB * KIB * KIB,
        &["pb", "petabyte", "petabytes"],
    ),
    unit("kib", "KiB", Data, KIB, &["kib", "kibibyte", "kibibytes"]),
    unit(
        "mib",
        "MiB",
        Data,
        KIB * KIB,
        &["mib", "mebibyte", "mebibytes"],
    ),
    unit(
        "gib",
        "GiB",
        Data,
        KIB * KIB * KIB,
        &["gib", "gibibyte", "gibibytes"],
    ),
    unit(
        "tib",
        "TiB",
        Data,
        KIB * KIB * KIB * KIB,
        &["tib", "tebibyte", "tebibytes"],
    ),
    // 时间（基准：秒）。年按 365 天、月按 30 天。
    unit(
        "ms-time",
        "ms",
        Time,
        0.001,
        &["ms", "millisecond", "milliseconds", "毫秒"],
    ),
    unit(
        "s",
        "s",
        Time,
        1.0,
        &["s", "sec", "secs", "second", "seconds", "秒", "秒钟"],
    ),
    unit(
        "min",
        "min",
        Time,
        60.0,
        &["min", "mins", "minute", "minutes", "分钟", "分"],
    ),
    unit(
        "h",
        "h",
        Time,
        3600.0,
        &["h", "hr", "hrs", "hour", "hours", "小时", "钟头"],
    ),
    unit(
        "day",
        "day",
        Time,
        86400.0,
        &["d", "day", "days", "天", "日"],
    ),
    unit(
        "week",
        "week",
        Time,
        604800.0,
        &["wk", "week", "weeks", "周", "星期", "礼拜"],
    ),
    unit(
        "month",
        "month",
        Time,
        2592000.0,
        &["month", "months", "月", "个月"],
    ),
    unit(
        "year",
        "year",
        Time,
        31536000.0,
        &["yr", "yrs", "year", "years", "年"],
    ),
];

/// 货币：ISO 代码总能用（`usd`、`cny`），这里登记中文名、俗称与符号。不在汇率表里的代码也能解析，
/// 换算时再说「没有这个币种的汇率」。
pub static CURRENCIES: &[Unit] = &[
    unit(
        "CNY",
        "CNY",
        Currency,
        1.0,
        &[
            "cny",
            "rmb",
            "¥",
            "￥",
            "人民币",
            "元",
            "块",
            "块钱",
            "yuan",
            "软妹币",
        ],
    ),
    unit(
        "USD",
        "USD",
        Currency,
        1.0,
        &[
            "usd", "$", "us$", "美元", "美金", "刀", "美刀", "dollar", "dollars", "bucks",
        ],
    ),
    unit(
        "EUR",
        "EUR",
        Currency,
        1.0,
        &["eur", "€", "欧元", "欧", "euro", "euros"],
    ),
    unit(
        "JPY",
        "JPY",
        Currency,
        1.0,
        &["jpy", "円", "日元", "日币", "日圆", "yen"],
    ),
    unit(
        "HKD",
        "HKD",
        Currency,
        1.0,
        &["hkd", "hk$", "港币", "港元", "港纸"],
    ),
    unit(
        "GBP",
        "GBP",
        Currency,
        1.0,
        &["gbp", "£", "英镑", "pound sterling", "quid"],
    ),
    unit(
        "KRW",
        "KRW",
        Currency,
        1.0,
        &["krw", "₩", "韩元", "韩币", "won"],
    ),
    unit("AUD", "AUD", Currency, 1.0, &["aud", "a$", "澳元", "澳币"]),
    unit("CAD", "CAD", Currency, 1.0, &["cad", "c$", "加元", "加币"]),
    unit("CHF", "CHF", Currency, 1.0, &["chf", "瑞士法郎", "瑞郎"]),
    unit(
        "SGD",
        "SGD",
        Currency,
        1.0,
        &["sgd", "s$", "新加坡元", "新币"],
    ),
    unit("NZD", "NZD", Currency, 1.0, &["nzd", "新西兰元", "纽币"]),
    unit("THB", "THB", Currency, 1.0, &["thb", "฿", "泰铢"]),
    unit(
        "INR",
        "INR",
        Currency,
        1.0,
        &["inr", "₹", "印度卢比", "卢比"],
    ),
    unit("IDR", "IDR", Currency, 1.0, &["idr", "印尼盾"]),
    unit(
        "MYR",
        "MYR",
        Currency,
        1.0,
        &["myr", "马来西亚林吉特", "林吉特", "马币", "令吉"],
    ),
    unit("PHP", "PHP", Currency, 1.0, &["php", "菲律宾比索"]),
    unit("MXN", "MXN", Currency, 1.0, &["mxn", "墨西哥比索"]),
    unit(
        "BRL",
        "BRL",
        Currency,
        1.0,
        &["brl", "巴西雷亚尔", "雷亚尔"],
    ),
    unit("ZAR", "ZAR", Currency, 1.0, &["zar", "南非兰特", "兰特"]),
    unit("TRY", "TRY", Currency, 1.0, &["try", "土耳其里拉", "里拉"]),
    unit("SEK", "SEK", Currency, 1.0, &["sek", "瑞典克朗"]),
    unit("NOK", "NOK", Currency, 1.0, &["nok", "挪威克朗"]),
    unit("DKK", "DKK", Currency, 1.0, &["dkk", "丹麦克朗"]),
    unit(
        "PLN",
        "PLN",
        Currency,
        1.0,
        &["pln", "波兰兹罗提", "兹罗提"],
    ),
    unit("CZK", "CZK", Currency, 1.0, &["czk", "捷克克朗"]),
    unit("HUF", "HUF", Currency, 1.0, &["huf", "匈牙利福林", "福林"]),
    unit(
        "ILS",
        "ILS",
        Currency,
        1.0,
        &["ils", "₪", "以色列新谢克尔", "谢克尔"],
    ),
    unit("ISK", "ISK", Currency, 1.0, &["isk", "冰岛克朗"]),
    unit(
        "RON",
        "RON",
        Currency,
        1.0,
        &["ron", "罗马尼亚列伊", "列伊"],
    ),
];

/// 没写目标单位时列出的常用单位（按顺序，跳过源单位和数值太大太小的）。
fn common_targets(category: Category) -> &'static [&'static str] {
    match category {
        Length => &["m", "km", "cm", "mi", "ft", "in"],
        Mass => &["kg", "g", "jin", "lb", "oz"],
        Temperature => &["c", "f", "k"],
        Volume => &["l", "ml", "gal", "floz", "cup"],
        Area => &["m2", "km2", "mu", "ha", "acre", "ft2"],
        Speed => &["kmh", "ms", "mph", "knot"],
        Data => &["mb", "gb", "kb", "tb", "b"],
        Currency => &["CNY", "USD", "EUR", "JPY", "HKD"],
        Time => &[],
    }
}

/// 解析出的换算请求。
#[derive(Debug, Clone, Copy)]
pub struct Request {
    pub amount: f64,
    pub from: &'static Unit,
    /// 用户写的目标单位；`None` 时列出常用换算。
    pub to: Option<&'static Unit>,
}

impl Request {
    /// 要展示的目标单位：写了就是它，没写就是本类常用单位（最多 4 个，数值在 0.01 到一百万之间优先）。
    pub fn targets(&self) -> Vec<&'static Unit> {
        if let Some(to) = self.to {
            return vec![to];
        }
        let candidates: Vec<&'static Unit> = common_targets(self.from.category)
            .iter()
            .filter_map(|key| by_key(key))
            .filter(|unit| unit.key != self.from.key && !unit.factor_eq(self.from))
            .collect();
        if self.from.category == Currency || self.from.category == Temperature {
            return candidates.into_iter().take(4).collect();
        }
        let readable = |unit: &&Unit| {
            convert(self.amount, self.from, unit)
                .map(f64::abs)
                .is_some_and(|v| v == 0.0 || (0.01..1e6).contains(&v))
        };
        let mut picked: Vec<_> = candidates.iter().copied().filter(readable).collect();
        for unit in &candidates {
            if !picked.iter().any(|p| p.key == unit.key) {
                picked.push(unit);
            }
        }
        picked.truncate(4);
        picked
    }
}

impl Unit {
    /// 同一单位的别名（公斤 = 千克）不该作为「换算目标」再列一遍。
    fn factor_eq(&self, other: &Unit) -> bool {
        self.category != Temperature
            && self.category != Currency
            && self.category == other.category
            && self.factor == other.factor
    }
}

pub fn by_key(key: &str) -> Option<&'static Unit> {
    UNITS
        .iter()
        .chain(CURRENCIES.iter())
        .find(|unit| unit.key == key)
}

/// 按用户写法找单位：先看登记的别名，再把三个字母当货币代码（汇率表里有的话）。
pub fn lookup(text: &str) -> Option<&'static Unit> {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return None;
    }
    UNITS
        .iter()
        .chain(CURRENCIES.iter())
        .find(|unit| unit.aliases.contains(&text.as_str()))
}

/// 比例单位乘系数；温度先换成摄氏度再换出去。类别不同返回 `None`。货币不在这里换算。
pub fn convert(amount: f64, from: &Unit, to: &Unit) -> Option<f64> {
    if from.category != to.category || from.category == Currency {
        return None;
    }
    if from.category == Temperature {
        let celsius = match from.key {
            "f" => (amount - 32.0) * 5.0 / 9.0,
            "k" => amount - 273.15,
            _ => amount,
        };
        return Some(match to.key {
            "f" => celsius * 9.0 / 5.0 + 32.0,
            "k" => celsius + 273.15,
            _ => celsius,
        });
    }
    Some(amount * from.factor / to.factor)
}

/// 数据单位按 1024 进制换算（`1 GB = 1024 MB`）。两边都是 KB/MB… 才有意义。
pub fn convert_binary(amount: f64, from: &Unit, to: &Unit) -> Option<f64> {
    Some(amount * from.binary? / to.binary?)
}

/// 「换成」「to」这类连接词，长的在前（`换算成` 先于 `换成`）。英文词两边要有空格。
const CONNECTORS: &[&str] = &[
    " into ",
    " to ",
    " in ",
    " as ",
    "=>",
    "->",
    "→",
    "=",
    "换算成",
    "转换成",
    "换算为",
    "转换为",
    "换成",
    "转成",
    "兑换成",
    "兑换",
    "折合",
    "等于",
    "是",
    "合",
    "兑",
];

/// 货币符号可以写在数字前面：`$100`、`¥500 to usd`。
const PREFIX_SYMBOLS: &[&str] = &["us$", "hk$", "$", "¥", "￥", "€", "£", "₩", "₹", "฿", "₪"];

pub fn parse(text: &str) -> Option<Request> {
    let mut text = text.trim().to_lowercase();
    for noise in ["是多少", "等于多少", "多少", "几", "?", "？"] {
        text = text.replace(noise, " ");
    }
    let text = text.trim().trim_end_matches('=').trim();
    let (prefix, text) = match PREFIX_SYMBOLS
        .iter()
        .find_map(|symbol| text.strip_prefix(symbol).map(|rest| (*symbol, rest)))
    {
        Some((symbol, rest)) => (lookup(symbol), rest.trim_start()),
        None => (None, text),
    };
    let (amount, rest) = number(text)?;
    let rest = rest.trim();
    let (from, to, written) = match prefix {
        Some(from) => {
            let target = strip_connector(rest);
            let to = if target.is_empty() {
                None
            } else {
                Some(lookup(target)?)
            };
            (from, to, "$")
        }
        None => split_units(rest)?,
    };
    if let Some(to) = to {
        if to.category != from.category {
            return None;
        }
    } else if !bare_allowed(from, written) {
        return None;
    }
    Some(Request { amount, from, to })
}

/// 开头的数字：可带正负号、千分位逗号和小数。返回数值与剩下的文字。
fn number(text: &str) -> Option<(f64, &str)> {
    let bytes = text.as_bytes();
    let mut end = 0;
    if matches!(bytes.first(), Some(b'-' | b'+')) {
        end = 1;
    }
    let digits_start = end;
    while end < bytes.len()
        && (bytes[end].is_ascii_digit() || bytes[end] == b',' || bytes[end] == b'.')
    {
        end += 1;
    }
    let literal: String = text.get(..end)?.chars().filter(|c| *c != ',').collect();
    if !text
        .get(digits_start..end)?
        .bytes()
        .any(|b| b.is_ascii_digit())
    {
        return None;
    }
    let value: f64 = literal.parse().ok()?;
    value.is_finite().then_some((value, text.get(end..)?))
}

fn strip_connector(text: &str) -> &str {
    let padded = format!(" {text}");
    for connector in CONNECTORS {
        if padded.starts_with(connector) {
            return text
                .get(connector.trim_start().len()..)
                .unwrap_or_default()
                .trim();
        }
    }
    text.trim()
}

/// 把 `km to mi`、`公里等于英里`、`usd cny`、`美元人民币` 拆成源单位和目标单位。
/// 第三个返回值是用户写的源单位文字（判断写得够不够明确）。
fn split_units(rest: &str) -> Option<(&'static Unit, Option<&'static Unit>, &str)> {
    if let Some(unit) = lookup(rest) {
        return Some((unit, None, rest));
    }
    let padded = format!(" {rest} ");
    for connector in CONNECTORS {
        for (index, _) in padded.match_indices(connector) {
            let left = padded.get(..index).unwrap_or_default().trim();
            let right = padded
                .get(index + connector.len()..)
                .unwrap_or_default()
                .trim();
            if let (Some(from), Some(to)) = (lookup(left), lookup(right)) {
                let written = rest.trim_start().get(..left.len()).unwrap_or(rest);
                return Some((from, Some(to), written));
            }
        }
    }
    // 没有连接词：在每个字符边界试一刀，两边都是同类单位就算（`usd cny`、`美元人民币`）。
    rest.char_indices().skip(1).find_map(|(index, _)| {
        let (left, right) = rest.split_at(index);
        let from = lookup(left)?;
        let to = lookup(right)?;
        (from.category == to.category).then_some((from, Some(to), left.trim()))
    })
}

/// 只写了源单位时，要写得够明确才出结果：`10km`、`3斤`、`100 usd` 出，`5g`、`3m`、`2 days` 不出。
fn bare_allowed(unit: &Unit, written: &str) -> bool {
    if unit.category == Time {
        return false;
    }
    let written = written.trim();
    written == "$" || !written.is_ascii() || written.chars().count() >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> (f64, &'static str, Option<&'static str>) {
        let request = parse(text).unwrap_or_else(|| panic!("{text:?} 应能解析"));
        (request.amount, request.from.key, request.to.map(|u| u.key))
    }

    #[test]
    fn natural_english_and_chinese_forms_parse() {
        // 用户怎么顺手怎么写：空格、无空格、中文连接词、问号都要认。
        assert_eq!(parsed("10 km to mi"), (10.0, "km", Some("mi")));
        assert_eq!(parsed("10km in m"), (10.0, "km", Some("m")));
        assert_eq!(parsed("100f in c"), (100.0, "f", Some("c")));
        assert_eq!(parsed("1 GB = ? MB"), (1.0, "gb", Some("mb")));
        assert_eq!(parsed("1.5 GB in MB"), (1.5, "gb", Some("mb")));
        assert_eq!(parsed("3斤"), (3.0, "jin", None));
        assert_eq!(parsed("10公里等于多少英里"), (10.0, "km", Some("mi")));
        assert_eq!(parsed("100摄氏度是多少华氏度"), (100.0, "c", Some("f")));
        assert_eq!(parsed("2亩是多少平方米"), (2.0, "mu", Some("m2")));
        assert_eq!(parsed("100 usd to cny"), (100.0, "USD", Some("CNY")));
        assert_eq!(parsed("100美元"), (100.0, "USD", None));
        assert_eq!(parsed("100 刀"), (100.0, "USD", None));
        assert_eq!(parsed("100 美元 人民币"), (100.0, "USD", Some("CNY")));
        assert_eq!(parsed("100美元换成人民币"), (100.0, "USD", Some("CNY")));
        assert_eq!(parsed("¥500 to usd"), (500.0, "CNY", Some("USD")));
        assert_eq!(parsed("$1,200"), (1200.0, "USD", None));
        assert_eq!(parsed("90 min to h"), (90.0, "min", Some("h")));
        assert_eq!(parsed("-40 c to f"), (-40.0, "c", Some("f")));
    }

    #[test]
    fn plain_search_terms_are_not_conversions() {
        // 这些都是用户在搜东西：单字母单位、时间单位不写目标时不能抢第一条。
        for text in [
            "5g",
            "3m",
            "2 days",
            "1password",
            "2026",
            "wx",
            "10 km to kg",
            "3 pm",
            "c++",
            "100",
        ] {
            assert!(parse(text).is_none(), "{text:?}");
        }
    }

    #[test]
    fn conversions_use_exact_definitions() {
        // 换算系数错一点，用户复制走的就是错数。
        let unit = |text| lookup(text).unwrap();
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9 * b.abs().max(1.0);
        assert!(close(
            convert(10.0, unit("km"), unit("mi")).unwrap(),
            6.213711922373339
        ));
        assert!(close(
            convert(100.0, unit("f"), unit("c")).unwrap(),
            37.77777777777778
        ));
        assert!(close(convert(-40.0, unit("c"), unit("f")).unwrap(), -40.0));
        assert!(close(convert(0.0, unit("c"), unit("k")).unwrap(), 273.15));
        assert!(close(convert(3.0, unit("斤"), unit("kg")).unwrap(), 1.5));
        assert!(close(convert(1.0, unit("两"), unit("g")).unwrap(), 50.0));
        assert!(close(
            convert(1.0, unit("亩"), unit("m2")).unwrap(),
            666.6666666666666
        ));
        assert!(close(convert(1.0, unit("gb"), unit("mb")).unwrap(), 1000.0));
        assert!(close(
            convert_binary(1.0, unit("gb"), unit("mb")).unwrap(),
            1024.0
        ));
        assert!(close(
            convert(1.0, unit("gib"), unit("mib")).unwrap(),
            1024.0
        ));
        assert!(close(
            convert(36.0, unit("km/h"), unit("m/s")).unwrap(),
            10.0
        ));
        assert!(convert(1.0, unit("km"), unit("kg")).is_none());
    }

    #[test]
    fn bare_units_list_readable_common_targets() {
        // 只写「3斤」时给出 kg、g、lb 这类常用单位，不列源单位本身，也不列数值离谱的。
        let keys = |text| {
            parse(text)
                .unwrap()
                .targets()
                .iter()
                .map(|u| u.key)
                .collect::<Vec<_>>()
        };
        assert_eq!(keys("3斤"), ["kg", "g", "lb", "oz"]);
        assert_eq!(keys("100美元"), ["CNY", "EUR", "JPY", "HKD"]);
        assert!(!keys("10 km").contains(&"km"));
        assert!(!keys("2 公斤").contains(&"kg"), "公斤和千克是同一单位");
    }

    #[test]
    fn every_alias_is_unique() {
        // 同一个写法对应两个单位时，用户得到哪个全看表的顺序。
        let mut seen = std::collections::HashMap::new();
        for unit in UNITS.iter().chain(CURRENCIES.iter()) {
            for alias in unit.aliases {
                assert_eq!(alias.to_lowercase(), *alias, "{alias} 要写成小写");
                if let Some(other) = seen.insert(*alias, unit.key) {
                    panic!("{alias} 同时属于 {other} 和 {}", unit.key);
                }
            }
        }
    }
}
