//! 更新源 `latest.json` 的格式（见 docs/updates.md）：解析、校验、选安装包。
//!
//! ```json
//! {"version": "0.4.0", "published_at": "2026-11-20T08:00:00Z",
//!  "notes": {"en": "...", "zh-Hans": "..."}, "minimum_macos": "15.0",
//!  "assets": [{"name": "Helmp-0.4.0.zip", "kind": "zip", "size": 123, "sha256": "<64 hex>",
//!              "urls": ["https://blog.peterchen97.cn/helmp/releases/0.4.0/Helmp-0.4.0.zip", "https://github.com/..."]}]}
//! ```
//!
//! 不认识的 `kind` 原样保留但不会被选中（以后加新格式时，旧版本照样能读这份清单）。

use anyhow::{Result, bail};
use serde::Deserialize;

use super::version::Version;
use crate::i18n::{tr, trf};

/// 安装包大小上限：防止被篡改的清单让我们下载一个巨大的文件。
pub const MAX_ASSET_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Zip,
    Dmg,
}

impl AssetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AssetKind::Zip => "zip",
            AssetKind::Dmg => "dmg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    /// None = 不认识的格式（保留，不会被选中）。
    pub kind: Option<AssetKind>,
    pub size: u64,
    pub sha256: String,
    pub urls: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: Version,
    pub published_at: String,
    pub notes_en: Option<String>,
    pub notes_zh: Option<String>,
    pub minimum_macos: Option<String>,
    pub assets: Vec<Asset>,
}

impl Manifest {
    pub fn asset(&self, kind: AssetKind) -> Option<&Asset> {
        self.assets.iter().find(|asset| asset.kind == Some(kind))
    }

    /// 按界面语言取更新说明，缺了用另一种语言的。
    pub fn notes(&self) -> Option<&str> {
        let (first, second) = if crate::i18n::is_en() {
            (&self.notes_en, &self.notes_zh)
        } else {
            (&self.notes_zh, &self.notes_en)
        };
        first
            .as_deref()
            .or(second.as_deref())
            .map(str::trim)
            .filter(|text| !text.is_empty())
    }
}

#[derive(Deserialize)]
struct RawNotes {
    en: Option<String>,
    #[serde(rename = "zh-Hans")]
    zh_hans: Option<String>,
}

#[derive(Deserialize)]
struct RawAsset {
    name: String,
    kind: String,
    size: u64,
    sha256: String,
    urls: Vec<String>,
}

#[derive(Deserialize)]
struct RawManifest {
    version: String,
    published_at: String,
    #[serde(default)]
    notes: Option<RawNotes>,
    #[serde(default)]
    minimum_macos: Option<String>,
    assets: Vec<RawAsset>,
}

pub fn valid_sha256(hex: &str) -> bool {
    hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// 解析并校验清单。所有问题一次列出。
pub fn parse(text: &str) -> Result<Manifest> {
    let raw: RawManifest = serde_json::from_str(text).map_err(|err| {
        anyhow::anyhow!(trf!(
            "更新清单格式不对：{err}",
            "The update manifest is malformed: {err}"
        ))
    })?;
    let mut problems = Vec::new();
    let version = match Version::parse(&raw.version) {
        Ok(version) => Some(version),
        Err(err) => {
            problems.push(format!("{err:#}"));
            None
        }
    };
    if raw.published_at.trim().is_empty() {
        problems.push(tr("缺少 published_at", "published_at is missing").to_string());
    }
    if let Some(min) = &raw.minimum_macos
        && super::version::macos_at_least(min, "0").is_none()
    {
        problems.push(trf!(
            "minimum_macos 不是有效的版本：{min:?}",
            "minimum_macos isn't a valid version: {min:?}"
        ));
    }
    let mut assets = Vec::new();
    for asset in raw.assets {
        let name = asset.name.trim().to_string();
        if name.is_empty() || name.contains('/') || name.contains("..") {
            problems.push(trf!(
                "安装包名字不安全：{name:?}",
                "Unsafe asset name: {name:?}"
            ));
        }
        if !valid_sha256(&asset.sha256) {
            problems.push(trf!(
                "{name} 没有有效的 sha256（64 位小写十六进制）",
                "{name} has no valid sha256 (64 lowercase hex digits)"
            ));
        }
        if asset.size == 0 || asset.size > MAX_ASSET_BYTES {
            problems.push(trf!(
                "{name} 的大小不合理：{} 字节",
                "{name} has an implausible size: {} bytes",
                asset.size
            ));
        }
        if asset.urls.is_empty() {
            problems.push(trf!("{name} 没有下载地址", "{name} has no download URLs"));
        }
        for url in &asset.urls {
            if !valid_url(url) {
                problems.push(trf!(
                    "{name} 的下载地址必须是 http(s)：{url}",
                    "{name}: download URLs must be http(s): {url}"
                ));
            }
        }
        let kind = match asset.kind.as_str() {
            "zip" => Some(AssetKind::Zip),
            "dmg" => Some(AssetKind::Dmg),
            _ => None,
        };
        assets.push(Asset {
            name,
            kind,
            size: asset.size,
            sha256: asset.sha256,
            urls: asset.urls,
        });
    }
    if !assets.iter().any(|asset| asset.kind == Some(AssetKind::Zip)) {
        problems.push(tr("没有 zip 安装包", "There is no zip asset").to_string());
    }
    match (version, problems.is_empty()) {
        (Some(version), true) => {
            let notes = raw.notes.unwrap_or(RawNotes { en: None, zh_hans: None });
            Ok(Manifest {
                version,
                published_at: raw.published_at.trim().to_string(),
                notes_en: notes.en,
                notes_zh: notes.zh_hans,
                minimum_macos: raw.minimum_macos,
                assets,
            })
        }
        _ => bail!(trf!(
            "更新清单无效：{}",
            "The update manifest is invalid: {}",
            problems.join("; ")
        )),
    }
}

/// GitHub Releases API 的回复（`/releases/latest`）不是清单本身：找出附在发布上的 `latest.json` 的下载地址。
/// 不是 GitHub 发布的 JSON 时返回 None。
pub fn github_manifest_url(text: &str) -> Option<Result<String>> {
    #[derive(Deserialize)]
    struct GhAsset {
        name: String,
        browser_download_url: String,
    }
    #[derive(Deserialize)]
    struct GhRelease {
        tag_name: String,
        assets: Vec<GhAsset>,
    }
    let release: GhRelease = serde_json::from_str(text).ok()?;
    Some(
        release
            .assets
            .into_iter()
            .find(|asset| asset.name == "latest.json")
            .map(|asset| asset.browser_download_url)
            .ok_or_else(|| {
                anyhow::anyhow!(trf!(
                    "GitHub 发布 {} 里没有 latest.json",
                    "GitHub release {} has no latest.json attached",
                    release.tag_name
                ))
            }),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    pub fn sample(version: &str) -> String {
        serde_json::json!({
            "version": version,
            "published_at": "2026-11-20T08:00:00Z",
            "notes": {"en": "Faster search", "zh-Hans": "搜索更快"},
            "minimum_macos": "15.0",
            "assets": [
                {"name": format!("Helmp-{version}.dmg"), "kind": "dmg", "size": 20, "sha256": SHA,
                 "urls": ["https://example.com/a.dmg"]},
                {"name": format!("Helmp-{version}.zip"), "kind": "zip", "size": 10, "sha256": SHA,
                 "urls": ["https://cdn.example.com/a.zip", "https://github.com/a.zip"]},
                {"name": "Helmp.delta", "kind": "delta", "size": 5, "sha256": SHA, "urls": ["https://x/y"]}
            ]
        })
        .to_string()
    }

    #[test]
    fn parses_a_valid_manifest_and_picks_assets_by_kind() {
        let manifest = parse(&sample("0.4.0")).unwrap();
        assert_eq!(manifest.version.to_string(), "0.4.0");
        let zip = manifest.asset(AssetKind::Zip).unwrap();
        assert_eq!(zip.name, "Helmp-0.4.0.zip");
        // 下载地址的顺序就是尝试的顺序（七牛在前，GitHub 在后）。
        assert_eq!(zip.urls[0], "https://cdn.example.com/a.zip");
        assert_eq!(manifest.asset(AssetKind::Dmg).unwrap().size, 20);
        // 不认识的格式保留下来，但不会被选中。
        assert_eq!(manifest.assets.len(), 3);
        assert!(manifest.assets.iter().any(|a| a.kind.is_none()));
    }

    #[test]
    fn notes_follow_the_interface_language_with_fallback() {
        let manifest = parse(&sample("0.4.0")).unwrap();
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            assert_eq!(manifest.notes(), Some("Faster search"));
        });
        crate::i18n::with_lang(crate::i18n::Lang::Zh, || {
            assert_eq!(manifest.notes(), Some("搜索更快"));
            let mut only_en = manifest.clone();
            only_en.notes_zh = None;
            assert_eq!(only_en.notes(), Some("Faster search"));
        });
    }

    #[test]
    fn invalid_fields_are_all_reported() {
        let text = serde_json::json!({
            "version": "latest", "published_at": "",
            "assets": [{"name": "../evil.zip", "kind": "zip", "size": 0, "sha256": "XYZ",
                        "urls": ["file:///etc/passwd"]}]
        })
        .to_string();
        // 断言按英文文案写：测试进程的界面语言随系统，固定下来才稳定。
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let err = format!("{:#}", parse(&text).unwrap_err());
            for needle in ["latest", "published_at", "evil", "sha256", "size", "http(s)"] {
                assert!(err.contains(needle), "缺少 {needle}：{err}");
            }
        });
    }

    #[test]
    fn manifest_without_zip_is_rejected() {
        // 没有 zip 就无法原地替换：宁可报错，也不要「检查到更新但装不了」。
        let mut value: serde_json::Value = serde_json::from_str(&sample("0.4.0")).unwrap();
        value["assets"] = serde_json::json!([value["assets"][0].clone()]);
        let err = format!("{:#}", parse(&value.to_string()).unwrap_err());
        assert!(err.contains("zip"), "{err}");
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(parse("<html>404</html>").is_err());
        assert!(parse("{\"version\": \"1.0.0\"}").is_err());
    }

    #[test]
    fn github_release_points_to_attached_latest_json() {
        let release = serde_json::json!({
            "tag_name": "v0.4.0",
            "assets": [
                {"name": "Helmp-0.4.0.zip", "browser_download_url": "https://github.com/x/Helmp-0.4.0.zip"},
                {"name": "latest.json", "browser_download_url": "https://github.com/x/latest.json"}
            ]
        })
        .to_string();
        assert_eq!(
            github_manifest_url(&release).unwrap().unwrap(),
            "https://github.com/x/latest.json"
        );
        let without = serde_json::json!({"tag_name": "v0.4.0", "assets": []}).to_string();
        assert!(github_manifest_url(&without).unwrap().is_err());
        // 清单本身不是 GitHub 发布。
        assert!(github_manifest_url(&sample("0.4.0")).is_none());
    }
}
