//! 插件中心：从注册表（GitHub 仓库 PeterChen1997/helmp-plugins，经 jsDelivr 分发）安装与更新插件。
//!
//! - 注册表地址：`HELMP_REGISTRY_URL` > `config.plugins_registry_url` > [`DEFAULT_REGISTRY`]。
//!   目录下有 `index.json`（注册表仓库的 `scripts/build-index.py` 生成，列出每个插件的版本、权限和
//!   每个文件的 sha256 / 大小），文件在 `plugins/<id>/<path>`。
//! - 安装：下载索引里列出的每个文件，逐个校验大小与 sha256，写进临时目录并按 Helmp 的规则校验清单
//!   （id、版本必须与索引一致），全部通过后整体改名到 `<data_dir>/plugins/<id>`。任何一步失败都不留下文件。
//! - 装好的插件和手动放进来的用户插件一样：默认停用，用户在设置里看过权限、点了「允许并启用」才生效；
//!   更新后权限变了要重新确认（见 `registry::Entry::state`）。从不自动启用。
//! - 和自带插件同 id 的插件不安装（不能遮住自带插件），给出说明。
//! - 自定义插件源（`config.plugin_sources`，见 `sources`）和公开插件中心结构相同，可以带鉴权请求头；
//!   浏览时合并展示（同 id 公开插件中心优先），更新只从安装记录里的源取。校验、原子安装、默认停用对所有源一样。
//! - 演练模式（`HELMP_DRY_RUN=1`）下只访问本机地址（端到端测试的假服务）。

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use super::manifest::{self, Text};
use super::registry::{self, Source};
use super::sources;
use crate::config::{Config, PluginSource, RegistryInstall};
use crate::i18n::{tr, trf};

pub const DEFAULT_REGISTRY: &str = "https://cdn.jsdelivr.net/gh/PeterChen1997/helmp-plugins@main/";
/// 落地页上的插件中心（设置页「插件中心」按钮）。
pub const CENTER_PAGE: &str = "https://blog.peterchen97.cn/helmp/plugins/";
pub const INDEX_SCHEMA: u32 = 1;
/// 与注册表校验脚本（build-index.py）一致的限制。
pub const MAX_FILE_BYTES: u64 = 512 * 1024;
pub const MAX_PLUGIN_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_FILES: usize = 64;
const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Deserialize)]
pub struct Index {
    pub schema: u32,
    /// 生成索引时注册表仓库的 commit；用来把 jsDelivr 的 `@main` 换成固定版本下载文件。
    #[serde(default)]
    pub commit: Option<String>,
    pub plugins: Vec<Listing>,
}

/// 索引里的一个插件（Helmp 用到的字段；索引里的其他字段如 homepage、icon、keywords 给落地页用，这里忽略）。
#[derive(Debug, Clone, Deserialize)]
pub struct Listing {
    pub id: String,
    pub version: String,
    pub name: Text,
    #[serde(default)]
    pub description: Option<Text>,
    pub author: String,
    /// 权限键（与 `Permissions::keys` 相同）。只用于安装前展示，安装后以 Helmp 自己推导的为准。
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub permissions_text: BTreeMap<String, Vec<String>>,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub executable: bool,
}

impl Listing {
    /// 当前界面语言的权限说明；索引里没有时退回权限键。
    pub fn permission_lines(&self) -> Vec<String> {
        let key = if crate::i18n::is_en() {
            "en"
        } else {
            "zh-Hans"
        };
        self.permissions_text
            .get(key)
            .or_else(|| self.permissions_text.get("en"))
            .cloned()
            .unwrap_or_else(|| self.permissions.clone())
    }
}

impl Index {
    pub fn get(&self, id: &str) -> Option<&Listing> {
        self.plugins.iter().find(|listing| listing.id == id)
    }
}

/// 索引里的文件路径：相对插件目录，只由普通段组成，不含隐藏段、反斜杠。
pub fn safe_file_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.starts_with('/')
        && Path::new(path)
            .components()
            .all(|component| match component {
                Component::Normal(part) => !part.to_string_lossy().starts_with('.'),
                _ => false,
            })
        && !path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
}

fn valid_sha256(hex: &str) -> bool {
    hex.len() == 64
        && hex
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// 解析并检查索引：schema、id、文件列表（路径、哈希格式、大小限制）。有问题就整体拒绝。
pub fn parse_index(text: &str) -> Result<Index> {
    let index: Index = serde_json::from_str(text).map_err(|err| {
        anyhow!(trf!(
            "插件中心索引格式不对：{err}",
            "The plugin index is malformed: {err}"
        ))
    })?;
    if index.schema != INDEX_SCHEMA {
        bail!(trf!(
            "插件中心索引版本 {} 不受支持（这个版本的 Helmp 只认 {INDEX_SCHEMA}），请升级 Helmp",
            "Plugin index schema {} isn't supported (this Helmp reads {INDEX_SCHEMA}); update Helmp",
            index.schema
        ));
    }
    let mut ids = BTreeSet::new();
    for listing in &index.plugins {
        check_listing(listing)?;
        if !ids.insert(listing.id.as_str()) {
            bail!(trf!(
                "插件中心索引里 {} 重复",
                "Plugin {} is listed twice in the index",
                listing.id
            ));
        }
    }
    Ok(index)
}

fn check_listing(listing: &Listing) -> Result<()> {
    let id = &listing.id;
    if !manifest::valid_id(id) {
        bail!(trf!(
            "索引里的插件 id {id:?} 不合法",
            "Invalid plugin id {id:?} in the index"
        ));
    }
    if listing.files.len() > MAX_FILES {
        bail!(trf!(
            "{id} 的文件太多（{} 个，上限 {MAX_FILES}）",
            "{id} has too many files ({}, limit {MAX_FILES})",
            listing.files.len()
        ));
    }
    let mut paths = BTreeSet::new();
    let mut total = 0u64;
    for file in &listing.files {
        if !safe_file_path(&file.path) {
            bail!(trf!(
                "{id} 的文件路径 {:?} 不安全（必须是插件目录里的相对路径，不能含 .. 或隐藏文件）",
                "{id} lists an unsafe file path {:?} (must stay inside the plugin folder, no .. or hidden files)",
                file.path
            ));
        }
        if !paths.insert(file.path.as_str()) {
            bail!(trf!(
                "{id} 的文件 {} 重复",
                "{id} lists {} twice",
                file.path
            ));
        }
        if !valid_sha256(&file.sha256) {
            bail!(trf!(
                "{id} 的文件 {} 没有有效的 sha256",
                "{id}: {} has no valid sha256",
                file.path
            ));
        }
        if file.size > MAX_FILE_BYTES {
            bail!(trf!(
                "{id} 的文件 {} 太大（{} 字节，上限 {MAX_FILE_BYTES}）",
                "{id}: {} is too large ({} bytes, limit {MAX_FILE_BYTES})",
                file.path,
                file.size
            ));
        }
        total += file.size;
    }
    if total > MAX_PLUGIN_BYTES {
        bail!(trf!(
            "{id} 总共 {total} 字节，超过上限 {MAX_PLUGIN_BYTES}",
            "{id} is {total} bytes in total, over the {MAX_PLUGIN_BYTES} limit"
        ));
    }
    if !paths.contains("plugin.json") {
        bail!(trf!(
            "{id} 的文件列表里没有 plugin.json",
            "{id} has no plugin.json in its file list"
        ));
    }
    Ok(())
}

/// `a` 比 `b` 新吗？按点号分段，数字段按数值比，其余按字符串比；段数不同时缺的段当 0。
pub fn newer(a: &str, b: &str) -> bool {
    use std::cmp::Ordering;
    let parts = |v: &str| -> Vec<String> {
        v.trim()
            .split(['.', '-', '+'])
            .map(str::to_string)
            .collect()
    };
    let (pa, pb) = (parts(a), parts(b));
    for i in 0..pa.len().max(pb.len()) {
        let x = pa.get(i).map(String::as_str).unwrap_or("0");
        let y = pb.get(i).map(String::as_str).unwrap_or("0");
        let order = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            _ => x.cmp(y),
        };
        match order {
            Ordering::Greater => return true,
            Ordering::Less => return false,
            Ordering::Equal => {}
        }
    }
    false
}

// ---------------------------------------------------------------- 网络

/// 当前注册表地址（总以 `/` 结尾）。
pub fn registry_base(config: &Config) -> String {
    let base = std::env::var("HELMP_REGISTRY_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .or_else(|| {
            config
                .plugins_registry_url
                .clone()
                .filter(|url| !url.trim().is_empty())
        })
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string());
    let base = base.trim().to_string();
    if base.ends_with('/') {
        base
    } else {
        format!("{base}/")
    }
}

/// 下载文件用的地址：jsDelivr 的分支地址（`@main`）换成索引对应的 commit，
/// 避免 CDN 上索引和文件缓存的版本不一致（哈希对不上）。其他地址原样使用。
pub fn files_base(base: &str, commit: Option<&str>) -> String {
    const JSDELIVR: &str = "https://cdn.jsdelivr.net/gh/";
    let Some(commit) = commit.filter(|c| c.len() == 40 && c.chars().all(|c| c.is_ascii_hexdigit()))
    else {
        return base.to_string();
    };
    let Some(rest) = base.strip_prefix(JSDELIVR) else {
        return base.to_string();
    };
    let Some((owner, rest)) = rest.split_once('/') else {
        return base.to_string();
    };
    let Some((repo_ref, tail)) = rest.split_once('/') else {
        return base.to_string();
    };
    let repo = repo_ref.split('@').next().unwrap_or(repo_ref);
    format!("{JSDELIVR}{owner}/{repo}@{commit}/{tail}")
}

/// 一个注册表：公开插件中心，或用户添加的自定义插件源（见 `sources`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    /// 显示名；公开插件中心是「插件中心」。
    pub name: String,
    /// 目录地址，以 `/` 结尾。
    pub base: String,
    /// 用户添加的源（false = 公开插件中心）。
    pub custom: bool,
    /// 鉴权请求头名；值在钥匙串里。
    pub auth_header: Option<String>,
}

impl Registry {
    pub fn public(config: &Config) -> Self {
        Self {
            name: tr("插件中心", "Plugin Center").to_string(),
            base: registry_base(config),
            custom: false,
            auth_header: None,
        }
    }

    fn from_source(source: &PluginSource) -> Self {
        Self {
            name: source.name.clone(),
            // 手改坏了的地址原样留着，联网时 `check_url` 会说明哪里不对。
            base: sources::normalize_url(&source.url).unwrap_or_else(|_| source.url.clone()),
            custom: true,
            auth_header: source.auth_header.clone(),
        }
    }

    /// 这条安装记录是不是从这个源装的。旧记录（没有 custom 标记）都算公开插件中心，
    /// 哪怕当时的地址和现在不同（换过 `HELMP_REGISTRY_URL` 或 `plugins_registry_url`）。
    pub fn owns(&self, record: &RegistryInstall) -> bool {
        if self.custom {
            record.custom && record.registry == self.base
        } else {
            !record.custom
        }
    }

    fn record(&self, version: &str) -> RegistryInstall {
        RegistryInstall {
            registry: self.base.clone(),
            version: version.to_string(),
            custom: self.custom,
        }
    }
}

/// 全部源：公开插件中心在最前，然后是用户按顺序添加的自定义源。
pub fn registries(config: &Config) -> Vec<Registry> {
    let mut all = vec![Registry::public(config)];
    all.extend(config.plugin_sources.iter().map(Registry::from_source));
    all
}

/// 指代公开插件中心的名字（自定义源不能用）。
pub const PUBLIC_NAMES: [&str; 3] = ["plugin-center", "插件中心", "Plugin Center"];

/// 按名字找源（命令行 `--source`）；公开插件中心的名字不分语言都认。
pub fn find_registry(config: &Config, name: &str) -> Result<Registry> {
    if PUBLIC_NAMES.contains(&name) {
        return Ok(Registry::public(config));
    }
    config
        .plugin_sources
        .iter()
        .find(|source| source.name == name)
        .map(Registry::from_source)
        .with_context(|| {
            trf!(
                "没有叫「{name}」的插件源（helmp plugins source list 查看）",
                "There's no plugin source named “{name}” (see helmp plugins source list)"
            )
        })
}

/// 上报给 Sentry 的失败原因：只有源的类型和固定的原因枚举，不带地址、插件 id 和令牌。
pub fn sentry_message(registry: &Registry, reason: &str) -> String {
    let kind = if registry.custom { "custom" } else { "public" };
    format!("{kind} registry: {reason}")
}

fn report_failure(registry: &Registry, reason: &'static str) {
    crate::telemetry::report("plugins.registry", &anyhow!(sentry_message(registry, reason)));
}

/// 联网用的客户端：一个源一个，令牌只在这里读一次。不实现 Debug，令牌不会被打印出来。
pub struct Client {
    pub registry: Registry,
    token: Option<String>,
}

impl Client {
    pub fn new(registry: Registry) -> Result<Self> {
        let token = match &registry.auth_header {
            Some(_) => sources::token(&registry.base).with_context(|| {
                trf!(
                    "读取插件源「{}」的令牌失败（钥匙串）",
                    "Couldn't read the token for plugin source “{}” from the Keychain",
                    registry.name
                )
            })?,
            None => None,
        };
        Ok(Self { registry, token })
    }

    fn check_url(&self, url: &str) -> Result<()> {
        if !url.starts_with("https://") && !url.starts_with("http://") {
            bail!(trf!(
                "插件中心地址必须是 http(s)：{url}",
                "The plugin registry must be an http(s) URL: {url}"
            ));
        }
        if self.registry.custom
            && let Err(err) = sources::normalize_url(&self.registry.base)
        {
            bail!(trf!(
                "插件源「{}」的地址不可用：{err}",
                "Plugin source “{}” has an unusable URL: {err}",
                self.registry.name
            ));
        }
        if crate::effects::dry_run() && !sources::is_loopback(url) {
            bail!(trf!(
                "演练模式下不联网：{url}（可用 HELMP_REGISTRY_URL 指向本机地址）",
                "No network in dry-run mode: {url} (point HELMP_REGISTRY_URL at a local address)"
            ));
        }
        Ok(())
    }

    /// 取一个文件。自定义源：不自动跟随重定向，只跟随同一主机内的跳转（令牌不会被带到别的主机）；
    /// 鉴权请求头只发给源自己的主机。HTTP 错误换成说得清哪个源、什么原因的提示。
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
        self.check_url(url)?;
        let registry = &self.registry;
        let home = sources::origin(&registry.base);
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .max_redirects(if registry.custom { 0 } else { 10 })
            .build()
            .into();
        let mut url = url.to_string();
        for _ in 0..=MAX_REDIRECTS {
            let mut request = agent.get(&url);
            if let (Some(header), Some(token)) = (&registry.auth_header, &self.token)
                && registry.custom
                && sources::origin(&url) == home
            {
                request = request.header(header.as_str(), token.as_str());
            }
            let mut response = match request.call() {
                Ok(response) => response,
                Err(ureq::Error::StatusCode(code)) => {
                    report_failure(registry, status_reason(code));
                    bail!(status_message(registry, code, self.token.is_some()));
                }
                Err(err) => {
                    report_failure(registry, "unreachable");
                    bail!(trf!(
                        "连不上{}（{}）：{err}",
                        "Couldn't reach {} ({}): {err}",
                        registry.label(),
                        sources::host(&url)
                    ));
                }
            };
            if response.status().is_redirection() {
                let next = response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .map(|location| resolve(&url, location));
                match next {
                    Some(next) if sources::origin(&next) == home => {
                        url = next;
                        continue;
                    }
                    _ => {
                        report_failure(registry, "redirect");
                        bail!(trf!(
                            "{}把请求转到了另一个主机，为保护令牌已停止。请把源地址改成文件实际所在的地址",
                            "{} redirected to another host; stopped so the token isn't sent there. Use the address where the files actually live",
                            registry.label()
                        ));
                    }
                }
            }
            return response
                .body_mut()
                .with_config()
                .limit(limit)
                .read_to_vec()
                .with_context(|| {
                    trf!(
                        "读取 {url} 失败（或超过 {limit} 字节）",
                        "Couldn't read {url} (or it exceeds {limit} bytes)"
                    )
                });
        }
        report_failure(registry, "redirect");
        bail!(trf!(
            "{}重定向次数太多",
            "{} redirected too many times",
            registry.label()
        ))
    }

    pub fn index(&self) -> Result<Index> {
        let bytes = self.get(&format!("{}index.json", self.registry.base), MAX_INDEX_BYTES)?;
        let parsed = String::from_utf8(bytes)
            .map_err(|_| {
                anyhow!(tr(
                    "插件中心索引不是 UTF-8 文本",
                    "The plugin index isn't UTF-8 text"
                ))
            })
            .and_then(|text| parse_index(&text));
        parsed.map_err(|err| {
            report_failure(&self.registry, "invalid_index");
            if self.registry.custom {
                anyhow!(trf!(
                    "插件源「{}」：{err:#}",
                    "Plugin source “{}”: {err:#}",
                    self.registry.name
                ))
            } else {
                err
            }
        })
    }
}

const MAX_REDIRECTS: usize = 5;

impl Registry {
    /// 报错里指代这个源的说法。
    fn label(&self) -> String {
        if self.custom {
            trf!("插件源「{}」", "plugin source “{}”", self.name)
        } else {
            tr("插件中心", "the plugin center").to_string()
        }
    }
}

/// 重定向的 Location 相对当前地址解析（绝对地址、`/路径`、相对路径）。
fn resolve(current: &str, location: &str) -> String {
    if sources::origin(location).is_some() {
        return location.to_string();
    }
    let origin = sources::origin(current).unwrap_or_default();
    if location.starts_with('/') {
        return format!("{origin}{location}");
    }
    let dir = current.rsplit_once('/').map(|(dir, _)| dir).unwrap_or(current);
    format!("{dir}/{location}")
}

fn status_reason(code: u16) -> &'static str {
    match code {
        401 => "http_401",
        403 => "http_403",
        404 => "http_404",
        500..=599 => "http_5xx",
        _ => "http_other",
    }
}

/// `sent_token`：这次请求带了令牌（401 就是令牌不对，而不是没设）。
fn status_message(registry: &Registry, code: u16, sent_token: bool) -> String {
    let source = registry.label();
    match code {
        401 if sent_token => trf!(
            "{source}拒绝了令牌（HTTP 401）：令牌不对或已过期。在「设置 › 插件 › 插件源」里更新令牌",
            "{source} rejected the token (HTTP 401): it's wrong or expired. Update it in Settings › Plugins › Plugin Sources"
        ),
        401 => trf!(
            "{source}需要登录（HTTP 401）：在「设置 › 插件 › 插件源」里给它设置令牌",
            "{source} needs a token (HTTP 401): set one in Settings › Plugins › Plugin Sources"
        ),
        403 => trf!(
            "{source}拒绝访问（HTTP 403）：这个令牌没有读取权限",
            "{source} denied access (HTTP 403): the token isn't allowed to read it"
        ),
        404 => trf!(
            "{source}上找不到文件（HTTP 404）：检查地址是不是指向 index.json 所在的目录",
            "{source} returned HTTP 404: check that the URL points at the folder that holds index.json"
        ),
        _ => trf!("{source}返回了 HTTP {code}", "{source} returned HTTP {code}"),
    }
}

/// 各个源取到的索引（或失败原因），顺序同 [`registries`]。一个源失败不影响其他源。
pub struct Catalog {
    pub sources: Vec<(Registry, Result<Arc<Index>, String>)>,
}

/// 一个插件在合并视图里的一行。
pub struct Entry<'a> {
    pub registry: &'a Registry,
    pub listing: &'a Listing,
    /// 同 id 还出现在这些源里，被这一行盖住了。
    pub shadowed: Vec<&'a str>,
}

impl Catalog {
    /// 并行取全部源的索引。
    pub fn fetch(config: &Config) -> Self {
        let registries = registries(config);
        let results: Vec<Result<Index, String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = registries
                .iter()
                .map(|registry| {
                    scope.spawn(move || {
                        Client::new(registry.clone())
                            .and_then(|client| client.index())
                            .map_err(|err| format!("{err:#}"))
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap_or_else(|_| Err("panicked".into())))
                .collect()
        });
        Self {
            sources: registries
                .into_iter()
                .zip(results)
                .map(|(registry, result)| (registry, result.map(Arc::new)))
                .collect(),
        }
    }

    /// 合并后的插件列表。同 id 出现在几个源里时：装过的按安装记录里的源；没装过的公开插件中心优先，
    /// 然后按源的顺序。其余的记在 `shadowed` 里，界面上明确提示。
    pub fn entries(&self, config: &Config, entries: &[registry::Entry]) -> Vec<Entry<'_>> {
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        let mut out: Vec<Entry<'_>> = Vec::new();
        for (registry, result) in &self.sources {
            let Ok(index) = result else { continue };
            for listing in &index.plugins {
                let Some(&at) = seen.get(listing.id.as_str()) else {
                    seen.insert(&listing.id, out.len());
                    out.push(Entry {
                        registry,
                        listing,
                        shadowed: Vec::new(),
                    });
                    continue;
                };
                let local = entries
                    .iter()
                    .find(|e| e.id == listing.id && e.source == Source::User)
                    .and_then(|e| e.dir.as_deref());
                let owner = install_record(&listing.id, config, local)
                    .filter(|record| registry.owns(record) && !out[at].registry.owns(record));
                let first = &mut out[at];
                if owner.is_some() {
                    let previous = std::mem::replace(first, Entry {
                        registry,
                        listing,
                        shadowed: Vec::new(),
                    });
                    first.shadowed = previous.shadowed;
                    first.shadowed.push(&previous.registry.name);
                } else {
                    first.shadowed.push(&registry.name);
                }
            }
        }
        out
    }

    pub fn failures(&self) -> impl Iterator<Item = (&Registry, &str)> {
        self.sources
            .iter()
            .filter_map(|(registry, result)| result.as_ref().err().map(|err| (registry, err.as_str())))
    }
}

/// 同 id 被别的源盖住时给用户的提示。
pub fn shadowed_message(entry: &Entry<'_>) -> Option<String> {
    (!entry.shadowed.is_empty()).then(|| {
        let others = entry.shadowed.join(tr("、", ", "));
        trf!(
            "「{others}」里也有同 id 的插件，这里用的是「{}」的版本",
            "“{others}” also lists this id; the version from “{}” is used",
            entry.registry.name
        )
    })
}

// ---------------------------------------------------------------- 安装

/// 一次安装的结果。
#[derive(Debug, Clone)]
pub struct Installed {
    pub id: String,
    pub version: String,
    pub dir: PathBuf,
    /// 之前装过的版本（这次是更新）。
    pub previous: Option<String>,
    /// 已是最新，没有重新下载。
    pub up_to_date: bool,
}

pub fn is_bundled(id: &str) -> bool {
    registry::BUNDLED.iter().any(|(bundled, _)| *bundled == id)
}

/// 和自带插件同 id 时给用户的说明。
pub fn bundled_message(id: &str) -> String {
    trf!(
        "{id} 是 Helmp 自带的插件，已经装好了，不需要从插件中心安装：在「设置 › 插件」里启用即可",
        "{id} comes bundled with Helmp, so there's nothing to install: turn it on in Settings › Plugins"
    )
}

/// 插件中心里某个插件相对本机的状态（设置页、`helmp plugins available` 共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Bundled,
    NotInstalled,
    /// 从插件中心装的，已是最新。
    Installed,
    UpdateAvailable {
        installed: String,
    },
    /// 插件文件夹里有同 id 的本地插件（不是从插件中心装的）。
    LocalConflict,
    /// 已经从另一个源装了同 id 的插件（更新只看那个源）。
    OtherSource,
}

/// `listing` 是 `from` 这个源里的条目。
pub fn availability(
    listing: &Listing,
    from: &Registry,
    config: &Config,
    entries: &[registry::Entry],
) -> Availability {
    if is_bundled(&listing.id) {
        return Availability::Bundled;
    }
    let local = entries
        .iter()
        .find(|entry| entry.id == listing.id && entry.source == Source::User);
    let installed = install_record(&listing.id, config, local.and_then(|e| e.dir.as_deref()));
    match installed {
        Some(record) if !from.owns(&record) => Availability::OtherSource,
        Some(record) if newer(&listing.version, &record.version) => Availability::UpdateAvailable {
            installed: record.version.clone(),
        },
        Some(_) => Availability::Installed,
        None if local.is_some() => Availability::LocalConflict,
        None => Availability::NotInstalled,
    }
}

/// 插件目录里的安装记录文件。配置里也记一份，但图形界面开着时，命令行（或 MCP）装的插件
/// 写进 config.json 的记录可能被图形界面下次保存配置时覆盖，所以以插件目录里的这份为准兜底。
pub const MARKER: &str = ".helmp-registry.json";

/// 这个插件是不是从插件中心装的、装的哪个版本：先看配置，再看插件目录里的记录文件。
pub fn install_record(id: &str, config: &Config, dir: Option<&Path>) -> Option<RegistryInstall> {
    config
        .plugins
        .get(id)
        .and_then(|settings| settings.installed.clone())
        .or_else(|| {
            let text = std::fs::read_to_string(dir?.join(MARKER)).ok()?;
            serde_json::from_str(&text).ok()
        })
}

/// 安装或更新 `id`：联网、写插件目录、记进配置、重载插件列表。
///
/// 选源：装过的只从安装记录里的源更新（`source` 给了别的源就拒绝）；没装过的用 `source` 指定的源，
/// 没指定就按顺序找（公开插件中心优先）。
pub fn install(id: &str, source: Option<&str>) -> Result<Installed> {
    if is_bundled(id) {
        bail!(bundled_message(id));
    }
    let config = crate::config::get();
    let root = registry::user_dir()?;
    let record = install_record(id, &config, Some(&root.join(id)));
    let wanted = source.map(|name| find_registry(&config, name)).transpose()?;
    let (client, index) = match (&record, wanted) {
        (Some(record), wanted) => {
            let Some(owner) = registries(&config).into_iter().find(|r| r.owns(record)) else {
                bail!(trf!(
                    "{id} 是从插件源 {} 安装的，这个源已经删掉了：重新添加它才能更新",
                    "{id} was installed from the plugin source at {}, which has been removed; add it back to update",
                    sources::host(&record.registry)
                ));
            };
            if let Some(wanted) = wanted
                && wanted != owner
            {
                bail!(trf!(
                    "{id} 已经从「{}」安装，更新也只从那里取；要换成「{}」的版本，先删掉这个插件",
                    "{id} is installed from “{}” and only updates from there; to switch to “{}”, remove the plugin first",
                    owner.name,
                    wanted.name
                ));
            }
            let client = Client::new(owner)?;
            let index = client.index()?;
            (client, index)
        }
        (None, Some(wanted)) => {
            let client = Client::new(wanted)?;
            let index = client.index()?;
            (client, index)
        }
        (None, None) => find_in_sources(id, &config)?,
    };
    let base = client.registry.base.clone();
    let listing = index.get(id).with_context(|| {
        if client.registry.custom {
            trf!(
                "插件源「{}」里没有 {id}",
                "{id} isn't in plugin source “{}”",
                client.registry.name
            )
        } else {
            trf!(
                "插件中心里没有 {id}（{base}）",
                "{id} isn't in the plugin center ({base})"
            )
        }
    })?;
    let previous = record.map(|record| record.version);
    check_local_conflict(id, &root, previous.is_some(), &registry::load_all())?;
    let target = root.join(id);
    if let Some(version) = &previous
        && !newer(&listing.version, version)
        && target.join("plugin.json").is_file()
    {
        return Ok(Installed {
            id: id.to_string(),
            version: version.clone(),
            dir: target,
            previous,
            up_to_date: true,
        });
    }
    let files = files_base(&base, index.commit.as_deref());
    let record = client.registry.record(&listing.version);
    let dir = install_into(listing, &root, &record, |path| {
        client.get(&format!("{files}plugins/{id}/{path}"), MAX_FILE_BYTES)
    })
    .inspect_err(|_| report_failure(&client.registry, "install_rejected"))?;
    crate::config::update(|config| {
        // 只记来源和版本：enabled / consent 原样保留，新装的插件就是停用状态，
        // 更新后权限变了由 `Entry::state` 判定为待确认。
        config.plugins.entry(id.to_string()).or_default().installed = Some(record);
    })?;
    registry::reload();
    crate::telemetry::plugin_installed(client.registry.custom, previous.is_some());
    Ok(Installed {
        id: id.to_string(),
        version: listing.version.clone(),
        dir,
        previous,
        up_to_date: false,
    })
}

/// 按源的顺序找第一个列出了 `id` 的源。都没有时把失败的源一并说明。
fn find_in_sources(id: &str, config: &Config) -> Result<(Client, Index)> {
    let mut errors = Vec::new();
    for registry in registries(config) {
        let fetched = Client::new(registry).and_then(|client| {
            let index = client.index()?;
            Ok((client, index))
        });
        match fetched {
            Ok((client, index)) if index.get(id).is_some() => return Ok((client, index)),
            Ok(_) => {}
            Err(err) => errors.push(format!("{err:#}")),
        }
    }
    if config.plugin_sources.is_empty()
        && let Some(error) = errors.pop()
    {
        bail!(error);
    }
    let mut message = trf!(
        "插件中心和已添加的插件源里都没有找到 {id}",
        "Couldn't find {id} in the plugin center or any added plugin source"
    );
    for error in errors {
        message.push_str(&format!("\n  - {error}"));
    }
    bail!(message)
}

/// 同 id 的本地插件（手动放进来的、或者文件夹名不同）不能被插件中心覆盖。
fn check_local_conflict(
    id: &str,
    root: &Path,
    from_registry: bool,
    entries: &[registry::Entry],
) -> Result<()> {
    let target = root.join(id);
    // 清单写坏了的插件在列表里用文件夹名当 id：再直接读一下清单里写的 id，正在改的插件也不能被覆盖。
    let declared = |entry: &registry::Entry| {
        entry.dir.as_ref().and_then(|dir| {
            let text = std::fs::read_to_string(dir.join("plugin.json")).ok()?;
            let value: serde_json::Value = serde_json::from_str(&text).ok()?;
            value.get("id")?.as_str().map(str::to_string)
        })
    };
    let same_id = |entry: &&registry::Entry| {
        entry.source == Source::User
            && (entry.id == id || declared(entry).is_some_and(|declared| declared == id))
    };
    for entry in entries.iter().filter(same_id) {
        let dir = entry.dir.clone().unwrap_or_default();
        if dir != target || !from_registry {
            bail!(trf!(
                "插件文件夹里已经有一个 id 为 {id} 的本地插件（{}），它不是从插件中心安装的。为免覆盖你的文件，先把它移走再安装",
                "There's already a local plugin with id {id} ({}) that wasn't installed from the plugin center. Move it away first so it isn't overwritten",
                dir.display()
            ));
        }
    }
    // 文件夹在、但没能加载出同 id（比如清单写坏了），而且不是插件中心装的：同样不覆盖。
    if target.exists() && !from_registry {
        bail!(trf!(
            "{} 已存在，而且不是从插件中心安装的。为免覆盖你的文件，先把它移走再安装",
            "{} already exists and wasn't installed from the plugin center. Move it away first so it isn't overwritten",
            target.display()
        ));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    format!("{}-{nanos}", std::process::id())
}

/// 下载（`fetch(相对路径)`）、校验、写入 `<root>/<id>`。先写进 `root` 下的隐藏临时目录，
/// 全部校验通过后整体改名替换；失败时临时目录删掉，原有的版本不动。
pub fn install_into(
    listing: &Listing,
    root: &Path,
    record: &RegistryInstall,
    fetch: impl Fn(&str) -> Result<Vec<u8>>,
) -> Result<PathBuf> {
    check_listing(listing)?;
    let id = &listing.id;
    let staging = root.join(format!(".install-{id}-{}", unique_suffix()));
    let result = (|| -> Result<()> {
        std::fs::create_dir_all(&staging)
            .with_context(|| trf!("创建 {}", "Couldn't create {}", staging.display()))?;
        for file in &listing.files {
            let bytes = fetch(&file.path)?;
            if bytes.len() as u64 != file.size {
                bail!(trf!(
                    "{id} 的文件 {} 大小不对（应为 {} 字节，下载到 {} 字节），已拒绝安装",
                    "{id}: {} has the wrong size (expected {} bytes, got {}); refusing to install",
                    file.path,
                    file.size,
                    bytes.len()
                ));
            }
            let actual = sha256_hex(&bytes);
            if actual != file.sha256 {
                bail!(trf!(
                    "{id} 的文件 {} 校验失败（sha256 应为 {}，实际 {actual}），可能被篡改，已拒绝安装",
                    "{id}: {} failed verification (sha256 should be {}, got {actual}); it may have been tampered with, refusing to install",
                    file.path,
                    file.sha256
                ));
            }
            let path = staging.join(&file.path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, &bytes)
                .with_context(|| trf!("写入 {}", "Couldn't write {}", path.display()))?;
            let mode = if file.executable { 0o755 } else { 0o644 };
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
        }
        let entry = registry::load_dir(&staging, Source::User);
        let plugin = entry.loaded.map_err(|errors| {
            anyhow!(trf!(
                "{id} 的清单没有通过校验：{}",
                "{id}'s manifest failed validation: {}",
                errors.join(tr("；", "; "))
            ))
        })?;
        if plugin.id() != id {
            bail!(trf!(
                "清单里的 id 是 {}，与插件中心的 {id} 不一致，已拒绝安装",
                "The manifest's id is {}, not {id} as listed; refusing to install",
                plugin.id()
            ));
        }
        std::fs::write(staging.join(MARKER), serde_json::to_string_pretty(record)?)?;
        if plugin.manifest.version != listing.version {
            bail!(trf!(
                "清单版本 {} 与插件中心列出的 {} 不一致，已拒绝安装",
                "The manifest's version {} doesn't match the listed {}; refusing to install",
                plugin.manifest.version,
                listing.version
            ));
        }
        Ok(())
    })();
    if let Err(err) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(err);
    }
    let target = root.join(id);
    let old = root.join(format!(".replaced-{id}-{}", unique_suffix()));
    let had_old = target.exists();
    if had_old {
        std::fs::rename(&target, &old).with_context(|| {
            let _ = std::fs::remove_dir_all(&staging);
            trf!(
                "移开旧版本 {}",
                "Couldn't move the old version {} aside",
                target.display()
            )
        })?;
    }
    if let Err(err) = std::fs::rename(&staging, &target) {
        if had_old {
            let _ = std::fs::rename(&old, &target);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(err)
            .with_context(|| trf!("安装到 {}", "Couldn't install into {}", target.display()));
    }
    if had_old {
        let _ = std::fs::remove_dir_all(&old);
    }
    Ok(target)
}

// ---------------------------------------------------------------- 提交前的文件清单

/// 列出插件目录里的全部文件并按注册表的规则检查：不能有符号链接、隐藏文件、二进制文件（png 除外），
/// 大小与数量不超限。返回按路径排序的清单（含 sha256），问题全部列出。
pub fn collect_files(dir: &Path) -> Result<Vec<FileEntry>, Vec<String>> {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<FileEntry>, errors: &mut Vec<String>) {
        let mut entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(entries) => entries.filter_map(|entry| entry.ok()).collect(),
            Err(err) => {
                errors.push(trf!(
                    "读取 {} 失败：{err}",
                    "Couldn't read {}: {err}",
                    dir.display()
                ));
                return;
            }
        };
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                errors.push(trf!("读取 {path} 失败", "Couldn't read {path}"));
                continue;
            };
            if name.starts_with('.') {
                errors.push(trf!(
                    "不能包含隐藏文件：{path}",
                    "Hidden files aren't allowed: {path}"
                ));
            } else if meta.file_type().is_symlink() {
                errors.push(trf!(
                    "不能包含符号链接：{path}",
                    "Symlinks aren't allowed: {path}"
                ));
            } else if meta.is_dir() {
                walk(&entry.path(), &path, out, errors);
            } else if meta.len() > MAX_FILE_BYTES {
                errors.push(trf!(
                    "{path} 太大（{} 字节，上限 {MAX_FILE_BYTES}）",
                    "{path} is too large ({} bytes, limit {MAX_FILE_BYTES})",
                    meta.len()
                ));
            } else {
                match std::fs::read(entry.path()) {
                    Ok(bytes) => {
                        let png = path.ends_with(".png") && bytes.starts_with(b"\x89PNG\r\n\x1a\n");
                        let text = std::str::from_utf8(&bytes).is_ok() && !bytes.contains(&0);
                        if !png && !text {
                            errors.push(trf!(
                                "{path} 不是文本文件（只允许 UTF-8 文本和 png 图标）",
                                "{path} isn't a text file (only UTF-8 text and png icons are allowed)"
                            ));
                        }
                        out.push(FileEntry {
                            sha256: sha256_hex(&bytes),
                            size: bytes.len() as u64,
                            executable: meta.permissions().mode() & 0o111 != 0,
                            path,
                        });
                    }
                    Err(err) => errors.push(trf!(
                        "读取 {path} 失败：{err}",
                        "Couldn't read {path}: {err}"
                    )),
                }
            }
        }
    }
    let mut files = Vec::new();
    let mut errors = Vec::new();
    walk(dir, "", &mut files, &mut errors);
    if files.len() > MAX_FILES {
        errors.push(trf!(
            "文件太多（{} 个，上限 {MAX_FILES}）",
            "Too many files ({}, limit {MAX_FILES})",
            files.len()
        ));
    }
    let total: u64 = files.iter().map(|file| file.size).sum();
    if total > MAX_PLUGIN_BYTES {
        errors.push(trf!(
            "总共 {total} 字节，超过上限 {MAX_PLUGIN_BYTES}",
            "{total} bytes in total, over the {MAX_PLUGIN_BYTES} limit"
        ));
    }
    if !files.iter().any(|file| file.path == "README.md") {
        errors.push(
            tr(
                "缺少 README.md（说明插件做什么、怎么配置）",
                "README.md is missing (explain what the plugin does and how to set it up)",
            )
            .to_string(),
        );
    }
    if errors.is_empty() {
        Ok(files)
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const MANIFEST: &str = r#"{"schemaVersion":1,"id":"com.example.memo","name":"Memo","version":"1.2.0","author":"A","license":"MIT",
        "commands":[{"id":"a","title":"A","action":{"type":"open","url":"https://example.com/?q={{query|url}}"}}]}"#;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("helmp-center-{name}-{}", unique_suffix()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn listing(files: &[(&str, &[u8])]) -> Listing {
        Listing {
            id: "com.example.memo".into(),
            version: "1.2.0".into(),
            name: Text::Plain("Memo".into()),
            description: None,
            author: "A".into(),
            permissions: vec!["open:https".into()],
            permissions_text: BTreeMap::new(),
            files: files
                .iter()
                .map(|(path, bytes)| FileEntry {
                    path: (*path).into(),
                    sha256: sha256_hex(bytes),
                    size: bytes.len() as u64,
                    executable: path.ends_with(".sh"),
                })
                .collect(),
        }
    }

    fn record() -> RegistryInstall {
        RegistryInstall {
            registry: "http://127.0.0.1:1/".into(),
            version: "1.2.0".into(),
            custom: false,
        }
    }

    fn server(files: &[(&str, &[u8])]) -> impl Fn(&str) -> Result<Vec<u8>> {
        let map: HashMap<String, Vec<u8>> = files
            .iter()
            .map(|(p, b)| ((*p).to_string(), b.to_vec()))
            .collect();
        move |path| map.get(path).cloned().ok_or_else(|| anyhow!("404 {path}"))
    }

    fn index_json(files: &str) -> String {
        format!(
            r#"{{"schema":1,"generated_at":"2026-11-10T00:00:00Z","commit":null,"plugins":[{{"id":"com.example.memo","version":"1.0.0",
            "name":{{"en":"Memo","zh-Hans":"备忘"}},"description":null,"author":"A","permissions":["open:https"],
            "permissions_text":{{"en":["Open web pages in your browser"],"zh-Hans":["在浏览器中打开网页"]}},"files":{files}}}]}}"#
        )
    }

    #[test]
    fn parses_the_index_the_registry_generates() {
        // 注册表脚本生成的格式（多出的字段如 review、keywords 要能读，以后加字段也不能让旧版 Helmp 读不了）。
        let sha = "a".repeat(64);
        let text = index_json(&format!(
            r#"[{{"path":"plugin.json","sha256":"{sha}","size":10,"executable":false}},{{"path":"bin/run.sh","sha256":"{sha}","size":5,"executable":true}}]"#
        ))
        .replace(r#""author":"A","#, r#""author":"A","review":[],"future":{"x":1},"#);
        let index = parse_index(&text).unwrap();
        let memo = index.get("com.example.memo").unwrap();
        assert_eq!(memo.files.len(), 2);
        assert!(memo.files[1].executable);
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            assert_eq!(memo.name.get(), "Memo");
            assert_eq!(memo.permission_lines(), ["Open web pages in your browser"]);
        });
    }

    #[test]
    fn index_with_unsafe_paths_or_unknown_schema_is_refused() {
        // 文件路径决定写到磁盘哪里：`../`、绝对路径、隐藏文件都可能写出插件目录或藏东西，整份索引拒绝。
        let sha = "b".repeat(64);
        for path in [
            "../evil.sh",
            "/etc/x",
            "bin/../../x",
            ".hidden",
            "bin/.x",
            "a\\b",
            "a//b",
            "",
        ] {
            let text = index_json(&format!(
                r#"[{{"path":"plugin.json","sha256":"{sha}","size":1}},{{"path":{},"sha256":"{sha}","size":1}}]"#,
                serde_json::to_string(path).unwrap()
            ));
            assert!(parse_index(&text).is_err(), "{path:?} 应被拒绝");
        }
        let text = index_json(&format!(
            r#"[{{"path":"plugin.json","sha256":"{sha}","size":1}}]"#
        ));
        assert!(parse_index(&text).is_ok());
        assert!(parse_index(&text.replace(r#""schema":1"#, r#""schema":2"#)).is_err());
        // 没有 plugin.json、哈希不是 64 位十六进制、文件超限：同样拒绝。
        assert!(
            parse_index(&index_json(&format!(
                r#"[{{"path":"a.sh","sha256":"{sha}","size":1}}]"#
            )))
            .is_err()
        );
        assert!(
            parse_index(&index_json(
                r#"[{"path":"plugin.json","sha256":"xyz","size":1}]"#
            ))
            .is_err()
        );
        assert!(
            parse_index(&index_json(&format!(
                r#"[{{"path":"plugin.json","sha256":"{sha}","size":{}}}]"#,
                MAX_FILE_BYTES + 1
            )))
            .is_err()
        );
    }

    #[test]
    fn install_writes_verified_files_atomically() {
        // 正常安装：文件内容与权限位都和索引一致，临时目录不留痕。
        let root = tmp("ok");
        let files: &[(&str, &[u8])] = &[
            ("plugin.json", MANIFEST.as_bytes()),
            ("bin/run.sh", b"#!/bin/sh\necho hi\n"),
        ];
        let dir = install_into(&listing(files), &root, &record(), server(files)).unwrap();
        assert_eq!(dir, root.join("com.example.memo"));
        assert_eq!(
            std::fs::read_to_string(dir.join("plugin.json")).unwrap(),
            MANIFEST
        );
        let mode = std::fs::metadata(dir.join("bin/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
        let names: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["com.example.memo"], "不能留下临时目录");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn tampered_file_is_refused_and_nothing_is_written() {
        // CDN 或中间人改了任何一个文件：哈希对不上就整体拒绝，磁盘上不能出现半装的插件。
        let root = tmp("tampered");
        let files: &[(&str, &[u8])] = &[
            ("plugin.json", MANIFEST.as_bytes()),
            ("bin/run.sh", b"#!/bin/sh\necho hi\n"),
        ];
        let served: &[(&str, &[u8])] = &[
            ("plugin.json", MANIFEST.as_bytes()),
            ("bin/run.sh", b"#!/bin/sh\necho XX\n"),
        ];
        let err = install_into(&listing(files), &root, &record(), server(served)).unwrap_err();
        assert!(format!("{err:#}").contains("sha256"), "{err:#}");
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            0,
            "不能留下任何文件"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn failed_update_keeps_the_installed_version() {
        // 更新失败（这里是清单 id 与索引不符）时，原来能用的版本必须原样保留。
        let root = tmp("keep");
        let files: &[(&str, &[u8])] = &[("plugin.json", MANIFEST.as_bytes())];
        install_into(&listing(files), &root, &record(), server(files)).unwrap();
        let wrong = MANIFEST.replace("com.example.memo", "com.example.other");
        let bad: &[(&str, &[u8])] = &[("plugin.json", wrong.as_bytes())];
        let err = install_into(&listing(bad), &root, &record(), server(bad)).unwrap_err();
        assert!(format!("{err:#}").contains("com.example.other"), "{err:#}");
        assert_eq!(
            std::fs::read_to_string(root.join("com.example.memo/plugin.json")).unwrap(),
            MANIFEST
        );
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn traversal_paths_are_refused_even_without_the_index_check() {
        // 纵深防御：即使有人绕过 parse_index 直接调用，`../` 也不能写出插件目录。
        let root = tmp("traversal");
        let files: &[(&str, &[u8])] =
            &[("plugin.json", MANIFEST.as_bytes()), ("../escape.sh", b"x")];
        assert!(install_into(&listing(files), &root, &record(), server(files)).is_err());
        assert!(!root.parent().unwrap().join("escape.sh").exists());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn version_comparison_drives_update_detection() {
        // 「有更新」只在注册表版本真的更新时出现；按数值比，1.10 比 1.9 新。
        assert!(newer("1.10.0", "1.9.0"));
        assert!(newer("2.0.0", "1.99.99"));
        assert!(newer("1.0.1", "1.0"));
        assert!(!newer("1.0.0", "1.0.0"));
        assert!(!newer("1.0", "1.0.0"));
        assert!(!newer("0.9.0", "1.0.0"));
    }

    #[test]
    fn availability_reflects_install_records() {
        let config_with = |version: Option<&str>| {
            let mut config = Config::default();
            if let Some(version) = version {
                config
                    .plugins
                    .entry("com.example.memo".into())
                    .or_default()
                    .installed = Some(RegistryInstall {
                    registry: "x".into(),
                    version: version.into(),
                    custom: false,
                });
            }
            config
        };
        let memo = listing(&[("plugin.json", MANIFEST.as_bytes())]);
        let public = Registry::public(&Config::default());
        assert_eq!(
            availability(&memo, &public, &config_with(None), &[]),
            Availability::NotInstalled
        );
        assert_eq!(
            availability(&memo, &public, &config_with(Some("1.2.0")), &[]),
            Availability::Installed
        );
        assert_eq!(
            availability(&memo, &public, &config_with(Some("1.1.0")), &[]),
            Availability::UpdateAvailable {
                installed: "1.1.0".into()
            }
        );
        let mut flomo = memo.clone();
        flomo.id = "dev.helmp.flomo".into();
        assert_eq!(
            availability(&flomo, &public, &config_with(None), &[]),
            Availability::Bundled
        );
    }

    #[test]
    fn jsdelivr_branch_urls_are_pinned_to_the_index_commit() {
        // 索引与文件分别缓存在 CDN 上：按索引的 commit 取文件，才能保证哈希对得上。
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            files_base(DEFAULT_REGISTRY, Some(sha)),
            format!("https://cdn.jsdelivr.net/gh/PeterChen1997/helmp-plugins@{sha}/")
        );
        assert_eq!(files_base(DEFAULT_REGISTRY, None), DEFAULT_REGISTRY);
        assert_eq!(
            files_base("http://127.0.0.1:9/", Some(sha)),
            "http://127.0.0.1:9/"
        );
        assert_eq!(files_base(DEFAULT_REGISTRY, Some("main")), DEFAULT_REGISTRY);
    }

    #[test]
    fn collect_files_enforces_registry_rules() {
        // 提交前就按注册表 CI 的规则检查：符号链接、隐藏文件、二进制文件会在审核时被拒，早点告诉作者。
        let dir = tmp("collect");
        std::fs::write(dir.join("plugin.json"), MANIFEST).unwrap();
        std::fs::write(dir.join("README.md"), "# Memo\n").unwrap();
        std::fs::write(dir.join("icon.png"), b"\x89PNG\r\n\x1a\nrest").unwrap();
        let files = collect_files(&dir).unwrap();
        let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["README.md", "icon.png", "plugin.json"]);
        std::fs::write(dir.join(".DS_Store"), "x").unwrap();
        std::fs::write(dir.join("blob.bin"), [0u8, 159, 146, 150]).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", dir.join("link")).unwrap();
        let errors = collect_files(&dir).unwrap_err();
        assert_eq!(errors.len(), 3, "{errors:#?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn custom(name: &str, base: &str, auth: bool) -> Registry {
        Registry {
            name: name.into(),
            base: base.into(),
            custom: true,
            auth_header: auth.then(|| "Authorization".into()),
        }
    }

    fn installed_from(registry: &Registry, version: &str) -> Config {
        let mut config = Config::default();
        config
            .plugins
            .entry("com.example.memo".into())
            .or_default()
            .installed = Some(registry.record(version));
        config
    }

    #[test]
    fn updates_only_come_from_the_source_a_plugin_was_installed_from() {
        // 同 id 出现在两个源里：从自定义源装的插件不能被公开插件中心的版本「更新」掉（反过来也一样），
        // 否则任何一个源都能借同名插件替换另一个源装的代码。
        let memo = listing(&[("plugin.json", MANIFEST.as_bytes())]);
        let public = Registry::public(&Config::default());
        let team = custom("Team", "https://plugins.example.com/", true);
        let config = installed_from(&team, "1.0.0");
        assert_eq!(
            availability(&memo, &team, &config, &[]),
            Availability::UpdateAvailable {
                installed: "1.0.0".into()
            }
        );
        assert_eq!(availability(&memo, &public, &config, &[]), Availability::OtherSource);
        let config = installed_from(&public, "1.0.0");
        assert_eq!(availability(&memo, &team, &config, &[]), Availability::OtherSource);
        // 旧版本写的记录没有 custom 标记，地址换过也仍算公开插件中心。
        let mut moved = public.clone();
        moved.base = "http://127.0.0.1:9/".into();
        assert!(moved.owns(&config.plugins["com.example.memo"].installed.clone().unwrap()));
    }

    fn catalog(sources: Vec<(Registry, Vec<Listing>)>) -> Catalog {
        Catalog {
            sources: sources
                .into_iter()
                .map(|(registry, plugins)| {
                    let index = Index {
                        schema: INDEX_SCHEMA,
                        commit: None,
                        plugins,
                    };
                    (registry, Ok(Arc::new(index)))
                })
                .collect(),
        }
    }

    #[test]
    fn merged_listing_prefers_the_public_center_and_says_so() {
        let memo = listing(&[("plugin.json", MANIFEST.as_bytes())]);
        let mut other = memo.clone();
        other.id = "com.example.other".into();
        let public = Registry::public(&Config::default());
        let team = custom("Team", "https://plugins.example.com/", false);
        let mut both = catalog(vec![
            (public.clone(), vec![memo.clone()]),
            (team.clone(), vec![memo.clone(), other]),
        ]);
        both.sources.push((custom("Down", "https://down.example.com/", false), Err("offline".into())));
        let config = Config::default();
        let entries = both.entries(&config, &[]);
        let rows: Vec<_> = entries
            .iter()
            .map(|e| (e.listing.id.as_str(), e.registry.name.as_str(), e.shadowed.clone()))
            .collect();
        assert_eq!(
            rows,
            [
                ("com.example.memo", public.name.as_str(), vec!["Team"]),
                ("com.example.other", "Team", vec![]),
            ]
        );
        assert!(shadowed_message(&entries[0]).unwrap().contains("Team"));
        assert_eq!(both.failures().count(), 1, "一个源失败不影响其他源");
        // 已经从 Team 装了：这一行跟着安装记录走，更新从 Team 取。
        let config = installed_from(&team, "1.0.0");
        let entries = both.entries(&config, &[]);
        assert_eq!(entries[0].registry.name, "Team");
        assert_eq!(entries[0].shadowed, [public.name.as_str()]);
    }

    /// 本机假服务：按路径回复 (状态码, 额外响应头, 正文)，记下每个请求的路径和请求头。
    fn serve(
        reply: impl Fn(&str) -> (u16, String, String) + Send + 'static,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{BufRead as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    request.push_str(&line);
                }
                let path = request.split_whitespace().nth(1).unwrap_or_default().to_string();
                log.lock().unwrap().push(request);
                let (status, headers, body) = reply(&path);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\n{headers}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (url, seen)
    }

    const TOKEN: &str = "Bearer s3cr3t-token-value";

    fn client(registry: Registry, token: Option<&str>) -> Client {
        Client {
            registry,
            token: token.map(str::to_string),
        }
    }

    #[test]
    fn the_token_only_goes_to_the_sources_own_host() {
        // 同主机内的跳转照常跟随并带令牌；跳到别的主机就停下，令牌一次都不能发过去。
        let (elsewhere, elsewhere_seen) = serve(|_| (200, String::new(), "{}".into()));
        let target = format!("{elsewhere}steal");
        let (base, seen) = serve(move |path| match path {
            "/moved" => (302, "location: /index.json\r\n".into(), String::new()),
            "/away" => (302, format!("location: {target}\r\n"), String::new()),
            _ => (200, String::new(), "ok".into()),
        });
        let source = client(custom("Team", &base, true), Some(TOKEN));
        assert_eq!(source.get(&format!("{base}moved"), 100).unwrap(), b"ok");
        let requests = seen.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|r| r.contains(TOKEN)), "{requests:?}");

        let err = crate::i18n::with_lang(crate::i18n::Lang::En, || {
            format!("{:#}", source.get(&format!("{base}away"), 100).unwrap_err())
        });
        assert!(err.contains("another host"), "{err}");
        assert!(elsewhere_seen.lock().unwrap().is_empty(), "不能跟随到别的主机");
        // 文件地址如果不在源自己的主机上（索引写错或被改），同样不带令牌。
        source.get(&format!("{elsewhere}file"), 100).unwrap();
        let other = elsewhere_seen.lock().unwrap().clone();
        assert_eq!(other.len(), 1);
        assert!(!other[0].contains("s3cr3t"), "{other:?}");
    }

    #[test]
    fn auth_failures_name_the_source_and_never_leak_the_token() {
        // 401 / 403 要说清是哪个源、该去哪里改；报错文案和 Sentry 事件里都不能出现令牌和地址。
        let (base, _) = serve(|path| match path {
            "/index.json" => (401, String::new(), "{}".into()),
            _ => (403, String::new(), "{}".into()),
        });
        let events = sentry::test::with_captured_events(|| crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let source = client(custom("Team", &base, true), Some(TOKEN));
            let err = format!("{:#}", source.index().unwrap_err());
            assert!(err.contains("“Team”") && err.contains("401"), "{err}");
            assert!(err.contains("Plugin Sources"), "{err}");
            assert!(!err.contains("s3cr3t"), "{err}");
            let err = format!("{:#}", source.get(&format!("{base}plugins/x"), 10).unwrap_err());
            assert!(err.contains("403"), "{err}");
            let missing = client(custom("Team", &base, true), None);
            let err = format!("{:#}", missing.index().unwrap_err());
            assert!(err.contains("needs a token"), "{err}");
        }));
        assert_eq!(events.len(), 3, "每次失败都上报");
        for event in &events {
            let text = format!("{event:?}");
            assert!(!text.contains("s3cr3t"), "{text}");
            assert!(!text.contains("127.0.0.1"), "Sentry 里不能有源的地址：{text}");
        }
        assert_eq!(
            events[0].message.as_deref(),
            Some("plugins.registry: custom registry: http_401")
        );
    }
}
