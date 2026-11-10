//! 汇率：欧洲央行（ECB）每日参考汇率，缓存在 `<data_dir>/fx.json`。
//!
//! - 启动时读缓存；缓存超过一天（或没有）就联网更新一次，之后后台每小时检查一次，最多一天拉一次。
//! - 离线时用缓存，结果副标题写「汇率更新于 <日期>」；上次更新失败还会注明「更新失败，使用缓存」。
//! - 没有缓存又拿不到数据时，换算行明确说「没有汇率数据」和原因，不静默不出结果。
//! - 地址可用 `HELMP_FX_URL` 覆盖（端到端测试指向本地假服务）。演练模式（`HELMP_DRY_RUN=1`）下
//!   没设这个变量就不联网，测试永远碰不到真实网络。

use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::i18n::{tr, trf};

const ECB_URL: &str = "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml";
/// 缓存超过这么久才重新拉（ECB 每个工作日更新一次）。
const MAX_AGE_SECS: i64 = 24 * 3600;
/// 失败后至少隔这么久再试，避免离线时反复请求。
const RETRY_SECS: i64 = 3600;
const TIMEOUT: Duration = Duration::from_secs(8);

/// 以欧元为基准的汇率表（含 `EUR = 1`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rates {
    /// ECB 给出的汇率日期，如 `2026-11-09`。
    pub date: String,
    /// 本机拉取时间（Unix 秒）。
    pub fetched_at: i64,
    pub source: String,
    pub rates: BTreeMap<String, f64>,
}

impl Rates {
    /// `amount` 个 `from` 换成多少 `to`。任一币种不在表里时返回 `None`。
    pub fn convert(&self, amount: f64, from: &str, to: &str) -> Option<f64> {
        let from = self.rates.get(from)?;
        let to = self.rates.get(to)?;
        Some(amount / from * to)
    }
}

#[derive(Default)]
struct State {
    loaded: bool,
    rates: Option<Rates>,
    /// 最近一次更新失败的原因（成功后清空）。
    error: Option<String>,
    last_attempt: Option<i64>,
}

fn state() -> &'static RwLock<State> {
    static STATE: OnceLock<RwLock<State>> = OnceLock::new();
    STATE.get_or_init(RwLock::default)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// 汇率地址；演练模式下没指定地址时为 `None`（不联网）。
fn url() -> Option<String> {
    match std::env::var("HELMP_FX_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ if crate::effects::dry_run() => None,
        _ => Some(ECB_URL.to_string()),
    }
}

fn cache_path() -> Result<std::path::PathBuf> {
    Ok(crate::paths::data_dir()?.join("fx.json"))
}

fn load_cache() -> Option<Rates> {
    let text = std::fs::read_to_string(cache_path().ok()?).ok()?;
    match serde_json::from_str(&text) {
        Ok(rates) => Some(rates),
        Err(err) => {
            eprintln!("helmp: 汇率缓存读不出，忽略：{err}");
            None
        }
    }
}

/// 当前汇率（第一次调用时读缓存）。查询路径只读内存。
pub fn snapshot() -> Option<Rates> {
    {
        let guard = state().read().expect("fx state poisoned");
        if guard.loaded {
            return guard.rates.clone();
        }
    }
    let mut guard = state().write().expect("fx state poisoned");
    if !guard.loaded {
        guard.rates = load_cache();
        guard.loaded = true;
    }
    guard.rates.clone()
}

/// 上次更新失败的原因。
pub fn last_error() -> Option<String> {
    state().read().expect("fx state poisoned").error.clone()
}

/// 立即联网更新（「更新汇率」命令、启动时缓存过期）。成功后写缓存。
pub fn refresh() -> Result<Rates> {
    let result = fetch();
    let mut guard = state().write().expect("fx state poisoned");
    guard.loaded = true;
    guard.last_attempt = Some(now());
    match &result {
        Ok(rates) => {
            guard.rates = Some(rates.clone());
            guard.error = None;
        }
        Err(err) => {
            if guard.rates.is_none() {
                guard.rates = load_cache();
            }
            guard.error = Some(format!("{err:#}"));
        }
    }
    result
}

fn fetch() -> Result<Rates> {
    let url = url().context(tr(
        "演练模式下不联网（可设置 HELMP_FX_URL）",
        "No network in dry-run mode (set HELMP_FX_URL)",
    ))?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let text = agent
        .get(&url)
        .call()
        .and_then(|mut response| response.body_mut().read_to_string())
        .with_context(|| {
            trf!(
                "获取汇率失败（{url}）",
                "Couldn't fetch exchange rates ({url})"
            )
        })?;
    let rates = parse_ecb(&text, now())?;
    let path = cache_path()?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&rates)?)
        .with_context(|| trf!("写入 {}", "Couldn't write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| trf!("保存 {}", "Couldn't save {}", path.display()))?;
    Ok(rates)
}

/// 缓存过期（或没有）且离上次尝试超过一小时：该联网了。
fn due() -> bool {
    let fetched = snapshot().map(|rates| rates.fetched_at);
    let guard = state().read().expect("fx state poisoned");
    let now = now();
    let stale = fetched.is_none_or(|at| now - at >= MAX_AGE_SECS);
    let waited = guard.last_attempt.is_none_or(|at| now - at >= RETRY_SECS);
    stale && waited && url().is_some()
}

/// 启动：读缓存，过期就同步更新一次（provider 的 `start` 本来就在后台线程），再起一个每小时检查的线程。
pub fn start() {
    if due()
        && let Err(err) = refresh()
    {
        eprintln!("helmp: {err:#}");
    }
    if url().is_none() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("helmp-fx".into())
        .spawn(|| {
            loop {
                std::thread::sleep(Duration::from_secs(RETRY_SECS as u64));
                if due()
                    && let Err(err) = refresh()
                {
                    eprintln!("helmp: {err:#}");
                }
            }
        });
    if let Err(err) = spawned {
        eprintln!("helmp: 汇率后台更新线程启动失败：{err}");
    }
}

/// 解析 ECB 的 eurofxref XML：`<Cube time='2026-11-09'>` 下若干 `<Cube currency='USD' rate='1.08'/>`。
pub fn parse_ecb(xml: &str, fetched_at: i64) -> Result<Rates> {
    let attr = |tag: &str, name: &str| -> Option<String> {
        let start = tag.find(&format!("{name}="))? + name.len() + 1;
        let quote = tag.get(start..start + 1)?;
        let rest = tag.get(start + 1..)?;
        Some(rest.get(..rest.find(quote)?)?.to_string())
    };
    let mut date = None;
    let mut rates = BTreeMap::from([("EUR".to_string(), 1.0)]);
    for tag in xml.split('<').filter(|tag| tag.starts_with("Cube")) {
        if let Some(time) = attr(tag, "time") {
            date = Some(time);
        }
        if let (Some(code), Some(rate)) = (attr(tag, "currency"), attr(tag, "rate")) {
            let rate: f64 = rate.trim().parse().with_context(|| {
                trf!(
                    "汇率 {code} 的值读不出：{rate}",
                    "Bad rate for {code}: {rate}"
                )
            })?;
            anyhow::ensure!(rate > 0.0 && rate.is_finite(), "rate {code} = {rate}");
            rates.insert(code.to_uppercase(), rate);
        }
    }
    let date = date.context(tr(
        "汇率数据格式不对：没有日期",
        "Unexpected exchange-rate data: no date",
    ))?;
    anyhow::ensure!(
        rates.len() > 1,
        tr(
            "汇率数据格式不对：没有任何币种",
            "Unexpected exchange-rate data: no currencies"
        )
    );
    Ok(Rates {
        date,
        fetched_at,
        source: "ECB".into(),
        rates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gesmes:Envelope xmlns:gesmes="http://www.gesmes.org/xml/2002-08-01" xmlns="http://www.ecb.int/vocabulary/2002-08-01/eurofxref">
	<gesmes:subject>Reference rates</gesmes:subject>
	<Cube>
		<Cube time='2026-11-09'>
			<Cube currency='USD' rate='1.1000'/>
			<Cube currency='JPY' rate='165.00'/>
			<Cube currency='CNY' rate='7.7000'/>
		</Cube>
	</Cube>
</gesmes:Envelope>"#;

    #[test]
    fn parses_the_ecb_daily_feed() {
        // ECB 的格式是汇率的唯一来源：日期和每个币种都要读对，欧元本身补成 1。
        let rates = parse_ecb(SAMPLE, 42).unwrap();
        assert_eq!(rates.date, "2026-11-09");
        assert_eq!(rates.rates["USD"], 1.1);
        assert_eq!(rates.rates["EUR"], 1.0);
        assert_eq!(rates.fetched_at, 42);
        let cny = rates.convert(100.0, "USD", "CNY").unwrap();
        assert!((cny - 700.0).abs() < 1e-9, "{cny}");
        assert!(rates.convert(1.0, "USD", "TWD").is_none());
    }

    #[test]
    fn garbage_is_an_error_not_an_empty_table() {
        // 拿到的是错误页（HTML、空串）时要报错，不能把空表写进缓存覆盖掉好的汇率。
        assert!(parse_ecb("<html>503</html>", 0).is_err());
        assert!(parse_ecb("<Cube time='2026-11-09'></Cube>", 0).is_err());
        assert!(parse_ecb("<Cube time='x'><Cube currency='USD' rate='abc'/>", 0).is_err());
    }
}
