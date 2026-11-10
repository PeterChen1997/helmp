//! 更新源：按顺序试每个地址取 `latest.json`，下载安装包并校验大小与 sha256。
//!
//! - 默认先试七牛（国内可直连），再试 GitHub Releases（仓库公开后可用）。
//! - `HELMP_UPDATE_FEED`（优先）或配置 `updates.feed_url` 可替换整个列表，逗号或空白分隔多个地址。
//! - 演练模式（`HELMP_DRY_RUN=1`）只允许本机地址，测试不会联网。
//! - 每个失败的地址都记下原因：前面的失败、后面的成功时打印到 stderr，全部失败时一起报出来。

use std::io::Read as _;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};

use super::manifest::{self, Asset, Manifest};
use crate::config::Config;
use crate::i18n::{tr, trf};

pub const PRIMARY_FEED: &str = "https://blog.peterchen97.cn/helmp/releases/latest.json";
/// 直接下载附在最新发布上的 `latest.json`：不走 API，没有匿名每小时 60 次的限制。
/// 若有人把源换成 API 地址（`api.github.com/.../releases/latest`），也照样能解析。
pub const GITHUB_FEED: &str =
    "https://github.com/PeterChen1997/helmp/releases/latest/download/latest.json";
const MAX_FEED_BYTES: u64 = 1024 * 1024;
const FEED_TIMEOUT: Duration = Duration::from_secs(15);

/// 当前生效的更新源列表。
pub fn feed_urls(config: &Config) -> Vec<String> {
    let custom = std::env::var("HELMP_UPDATE_FEED")
        .ok()
        .filter(|text| !text.trim().is_empty())
        .or_else(|| {
            config
                .updates
                .feed_url
                .clone()
                .filter(|text| !text.trim().is_empty())
        });
    match custom {
        Some(text) => text
            .split([',', ' ', '\n', '\t'])
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string)
            .collect(),
        None => vec![PRIMARY_FEED.to_string(), GITHUB_FEED.to_string()],
    }
}

fn is_github(url: &str) -> bool {
    url.starts_with("https://github.com/") || url.starts_with("https://api.github.com/")
}

pub fn is_loopback(url: &str) -> bool {
    let authority = url
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
        .unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = if host.starts_with('[') {
        host.split(']').next().map(|h| format!("{h}]")).unwrap_or_default()
    } else {
        host.split(':').next().unwrap_or_default().to_string()
    };
    matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]")
}

fn agent(global: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(global)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_per_read(Some(Duration::from_secs(30)))
        .user_agent(format!("Helmp/{} (macOS updater)", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// GET 一个地址，最多读 `limit` 字节；`progress(已读, 总长)` 每读一块调用一次。
fn get(
    url: &str,
    limit: u64,
    global_timeout: Option<Duration>,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<Vec<u8>> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        bail!(trf!("地址必须是 http(s)：{url}", "Must be an http(s) URL: {url}"));
    }
    if crate::effects::dry_run() && !is_loopback(url) {
        bail!(trf!(
            "演练模式下不联网：{url}（可用 HELMP_UPDATE_FEED 指向本机地址）",
            "No network in dry-run mode: {url} (point HELMP_UPDATE_FEED at a local address)"
        ));
    }
    let mut response = match agent(global_timeout)
        .get(url)
        .header("Accept", "application/json, application/octet-stream, */*")
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(404)) if is_github(url) => bail!(tr(
            "GitHub 上没有可用的发布（404：仓库可能还是私有的，或还没有发布）",
            "No release available on GitHub (404: the repository may still be private, or nothing is published yet)"
        )),
        Err(ureq::Error::StatusCode(code)) => bail!(trf!("HTTP {code}", "HTTP {code}")),
        Err(err) => return Err(anyhow!(err)),
    };
    let total = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if let Some(total) = total
        && total > limit
    {
        bail!(trf!(
            "文件比预期大（{total} 字节，上限 {limit}）",
            "The file is larger than expected ({total} bytes, limit {limit})"
        ));
    }
    // 多读一个字节：读到了说明超过上限（服务器没给长度时也能发现）。
    let mut reader = response.body_mut().with_config().limit(limit + 1).reader();
    let mut bytes = Vec::with_capacity(total.unwrap_or(0).min(limit) as usize);
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|err| anyhow!(trf!("读取失败：{err}", "Read failed: {err}")))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if bytes.len() as u64 > limit {
            bail!(trf!(
                "文件比预期大（超过 {limit} 字节）",
                "The file is larger than expected (over {limit} bytes)"
            ));
        }
        progress(bytes.len() as u64, total);
    }
    Ok(bytes)
}

fn fetch_one(url: &str) -> Result<Manifest> {
    let bytes = get(url, MAX_FEED_BYTES, Some(FEED_TIMEOUT), &mut |_, _| {})?;
    let text = String::from_utf8(bytes)
        .map_err(|_| anyhow!(tr("回复不是 UTF-8 文本", "The reply isn't UTF-8 text")))?;
    // GitHub Releases API：找出附在发布上的 latest.json 再取一次。
    if let Some(manifest_url) = manifest::github_manifest_url(&text) {
        let manifest_url = manifest_url?;
        let bytes = get(&manifest_url, MAX_FEED_BYTES, Some(FEED_TIMEOUT), &mut |_, _| {})
            .with_context(|| manifest_url.clone())?;
        let text = String::from_utf8(bytes)
            .map_err(|_| anyhow!(tr("回复不是 UTF-8 文本", "The reply isn't UTF-8 text")))?;
        return manifest::parse(&text);
    }
    manifest::parse(&text)
}

/// 一次检查的结果：清单、取到它的地址，以及之前失败的地址与原因。
pub struct Fetched {
    pub manifest: Manifest,
    pub source: String,
    pub skipped: Vec<(String, String)>,
}

/// 按顺序试每个更新源，返回第一个有效的清单；全部失败时列出每个地址的原因。
pub fn fetch(urls: &[String]) -> Result<Fetched> {
    if urls.is_empty() {
        bail!(tr("没有配置更新源", "No update feed is configured"));
    }
    let mut failures = Vec::new();
    for url in urls {
        match fetch_one(url) {
            Ok(manifest) => {
                return Ok(Fetched {
                    manifest,
                    source: url.clone(),
                    skipped: failures,
                });
            }
            Err(err) => failures.push((url.clone(), format!("{err:#}"))),
        }
    }
    let lines: Vec<String> = failures
        .iter()
        .map(|(url, err)| format!("{url}: {err}"))
        .collect();
    bail!(lines.join("\n"))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 下载内容必须与清单完全一致：大小相等、sha256 相等。否则拒绝（可能被篡改或下载不完整）。
pub fn verify_payload(bytes: &[u8], asset: &Asset) -> Result<()> {
    if bytes.len() as u64 != asset.size {
        bail!(trf!(
            "{} 大小不符（应为 {} 字节，实际 {}），已拒绝安装",
            "{}: size mismatch (expected {} bytes, got {}); refusing to install",
            asset.name,
            asset.size,
            bytes.len()
        ));
    }
    let actual = sha256_hex(bytes);
    if actual != asset.sha256 {
        bail!(trf!(
            "{} 的 sha256 校验失败（应为 {}，实际 {actual}），可能被篡改，已拒绝安装",
            "{}: sha256 mismatch (expected {}, got {actual}); it may have been tampered with, refusing to install",
            asset.name,
            asset.sha256
        ));
    }
    Ok(())
}

/// 按顺序试安装包的每个下载地址，返回通过校验的内容与地址。
/// 校验失败（大小或 sha256 不符）不换下一个地址：清单与文件对不上就整体拒绝。
pub fn download(
    asset: &Asset,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(Vec<u8>, String)> {
    let mut failures = Vec::new();
    for url in &asset.urls {
        match get(url, asset.size, None, &mut |done, _| progress(done, asset.size)) {
            Ok(bytes) => {
                verify_payload(&bytes, asset)?;
                for (failed, err) in &failures {
                    eprintln!("helmp: {failed} 下载失败（{err}），已改用 {url}");
                }
                return Ok((bytes, url.clone()));
            }
            Err(err) => failures.push((url.clone(), format!("{err:#}"))),
        }
    }
    let lines: Vec<String> = failures
        .iter()
        .map(|(url, err)| format!("{url}: {err}"))
        .collect();
    bail!(trf!(
        "下载 {} 失败：\n{}",
        "Couldn't download {}:\n{}",
        asset.name,
        lines.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updates::manifest::AssetKind;

    #[test]
    fn default_feeds_try_qiniu_before_github() {
        // SAFETY 说明不需要：只读环境变量。测试进程里没有设置 HELMP_UPDATE_FEED。
        if std::env::var_os("HELMP_UPDATE_FEED").is_some() {
            return;
        }
        let urls = feed_urls(&Config::default());
        assert_eq!(urls, vec![PRIMARY_FEED.to_string(), GITHUB_FEED.to_string()]);
    }

    #[test]
    fn configured_feed_list_replaces_defaults() {
        if std::env::var_os("HELMP_UPDATE_FEED").is_some() {
            return;
        }
        let mut config = Config::default();
        config.updates.feed_url = Some("https://a.example/latest.json, https://b.example/x.json".into());
        assert_eq!(
            feed_urls(&config),
            vec!["https://a.example/latest.json", "https://b.example/x.json"]
        );
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback("http://127.0.0.1:8080/latest.json"));
        assert!(is_loopback("http://localhost/x"));
        assert!(is_loopback("http://[::1]:9/x"));
        assert!(!is_loopback("https://blog.peterchen97.cn/helmp/releases/latest.json"));
        assert!(!is_loopback("http://127.0.0.1.evil.com/x"));
        assert!(!is_loopback("http://localhost@evil.com/x"));
    }

    fn asset(body: &[u8]) -> Asset {
        Asset {
            name: "Helmp-9.9.9.zip".into(),
            kind: Some(AssetKind::Zip),
            size: body.len() as u64,
            sha256: sha256_hex(body),
            urls: vec!["https://example.com/a.zip".into()],
        }
    }

    #[test]
    fn payload_must_match_size_and_hash() {
        let good = b"helmp zip bytes";
        verify_payload(good, &asset(good)).unwrap();
        let mut tampered = asset(good);
        tampered.sha256 = sha256_hex(b"something else");
        let err = format!("{:#}", verify_payload(good, &tampered).unwrap_err());
        assert!(err.contains("sha256"), "{err}");
        let mut short = asset(good);
        short.size += 1;
        let err = format!("{:#}", verify_payload(good, &short).unwrap_err());
        assert!(err.contains("size") || err.contains("大小"), "{err}");
    }
}
