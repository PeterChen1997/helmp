//! 自定义插件源：公开插件中心之外、目录结构相同的注册表（团队或个人自己托管，类似 Homebrew tap）。
//!
//! - 配置在 `config.plugin_sources`（名称、地址、可选的鉴权请求头名）。公开插件中心总是第一个源，不在这个列表里。
//! - 地址必须是 https（本机回环地址除外，测试用），不能带用户名密码、查询串或片段。
//! - 鉴权请求头的值只存钥匙串：服务名 `dev.helmp.plugin-sources`，账户是源的地址；
//!   从不写进 config.json、日志、报错文案或 Sentry。请求只把它发给这个源自己的主机（见 `center::get`）。
//! - 测试模式（`HELMP_DATA_DIR` + `HELMP_DRY_RUN=1`，见 `secrets::test_mode`）绝不碰钥匙串，
//!   改读写数据目录里的 `test-plugin-sources.json`，端到端测试靠它确认令牌没进 config.json。

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::config::{Config, PluginSource};
use crate::i18n::trf;
use crate::telemetry::SourceEvent;

pub const SERVICE: &str = "dev.helmp.plugin-sources";
/// 没写请求头名、但给了令牌时用的请求头。
pub const DEFAULT_HEADER: &str = "Authorization";

/// 地址里的 `scheme://authority`（小写）；不是 http(s) 地址时 None。
pub fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    (!authority.is_empty()).then(|| format!("{scheme}://{}", authority.to_ascii_lowercase()))
}

/// 主机名（去掉端口）；IPv6 保留方括号。
pub fn host(url: &str) -> String {
    let origin = origin(url).unwrap_or_default();
    let authority = origin.split_once("://").map(|(_, a)| a).unwrap_or_default();
    match authority.strip_prefix('[') {
        Some(rest) => format!("[{}]", rest.split(']').next().unwrap_or_default()),
        None => authority.split(':').next().unwrap_or_default().to_string(),
    }
}

pub fn is_loopback(url: &str) -> bool {
    matches!(host(url).as_str(), "127.0.0.1" | "localhost" | "[::1]")
}

/// 检查并规范化一个源地址（去空白、补结尾的 `/`）。
pub fn normalize_url(url: &str) -> Result<String> {
    let url = url.trim();
    let Some(origin) = origin(url) else {
        bail!(trf!(
            "插件源地址必须以 https:// 开头：{url}",
            "A plugin source URL must start with https://: {url}"
        ));
    };
    if !origin.starts_with("https://") && !is_loopback(url) {
        bail!(trf!(
            "插件源必须用 https（只有本机地址可以用 http）：{url}",
            "Plugin sources must use https (plain http is only allowed for this Mac): {url}"
        ));
    }
    if origin.contains('@') {
        bail!(trf!(
            "插件源地址里不能带用户名或密码，令牌请单独填写：{}",
            "Don't put a user name or password in the URL; enter the token separately: {}",
            host(url)
        ));
    }
    if url.contains(['?', '#']) || url.chars().any(char::is_whitespace) {
        bail!(trf!(
            "插件源地址不能带查询参数、片段或空格（令牌请单独填写）：{}",
            "A plugin source URL can't contain a query, fragment or spaces (enter the token separately): {}",
            host(url)
        ));
    }
    Ok(if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{url}/")
    })
}

/// 请求头名：HTTP token 字符，不能是会被拿来改写请求的那几个。
pub fn valid_header(name: &str) -> bool {
    const RESERVED: [&str; 5] = ["host", "content-length", "transfer-encoding", "connection", "cookie"];
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c))
        && !RESERVED.contains(&name.to_ascii_lowercase().as_str())
}

/// 检查一个新源：名字非空且不重复，地址合法且不重复，请求头名合法。返回规范化后的源。
pub fn check_new(config: &Config, source: PluginSource) -> Result<PluginSource> {
    let name = source.name.trim().to_string();
    if name.is_empty() || name.len() > 64 || name.contains(['\t', '\n']) {
        bail!(trf!(
            "插件源名字不能为空，最多 64 个字符",
            "A plugin source needs a name of up to 64 characters"
        ));
    }
    let url = normalize_url(&source.url)?;
    if config.plugin_sources.iter().any(|s| s.name == name)
        || super::center::PUBLIC_NAMES.contains(&name.as_str())
    {
        bail!(trf!(
            "已经有一个叫「{name}」的插件源",
            "There's already a plugin source named “{name}”"
        ));
    }
    if let Some(existing) = config.plugin_sources.iter().find(|s| s.url == url) {
        bail!(trf!(
            "这个地址已经添加过了（「{}」）",
            "This URL is already added (“{}”)",
            existing.name
        ));
    }
    if url == super::center::registry_base(config) {
        bail!(trf!(
            "这是公开插件中心的地址，不用再添加",
            "That's the public plugin center, which is always included"
        ));
    }
    let auth_header = source
        .auth_header
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty());
    if let Some(header) = &auth_header
        && !valid_header(header)
    {
        bail!(trf!(
            "请求头名 {header:?} 不合法",
            "{header:?} isn't a valid header name"
        ));
    }
    Ok(PluginSource {
        name,
        url,
        auth_header,
    })
}

/// 添加一个源（`token` 非空时存进钥匙串，没写请求头名就用 `Authorization`）。返回存下的源。
/// `via` 是入口（`settings` / `cli`），只用于统计。
pub fn add(source: PluginSource, token: Option<&str>, via: &'static str) -> Result<PluginSource> {
    let mut source = check_new(&crate::config::get(), source)?;
    let token = token.map(str::trim).filter(|t| !t.is_empty());
    if token.is_some() && source.auth_header.is_none() {
        source.auth_header = Some(DEFAULT_HEADER.to_string());
    }
    if let Some(token) = token {
        set_token(&source.url, token)?;
    }
    let saved = source.clone();
    crate::config::update(move |config| config.plugin_sources.push(source))?;
    crate::telemetry::plugin_source(SourceEvent::Added {
        via,
        auth: saved.auth_header.is_some(),
    });
    Ok(saved)
}

/// 删除一个源和它的令牌。已经从它装的插件留着，只是不再有更新。
pub fn remove(name: &str) -> Result<PluginSource> {
    let config = crate::config::get();
    let Some(source) = config.plugin_sources.iter().find(|s| s.name == name).cloned() else {
        bail!(trf!(
            "没有叫「{name}」的插件源",
            "There's no plugin source named “{name}”"
        ));
    };
    set_token(&source.url, "")?;
    let url = source.url.clone();
    crate::config::update(move |config| config.plugin_sources.retain(|s| s.url != url))?;
    crate::telemetry::plugin_source(SourceEvent::Removed);
    Ok(source)
}

/// 换令牌（空 = 删除）。源没设请求头名时补上 `Authorization`。
pub fn update_token(name: &str, token: &str) -> Result<()> {
    let config = crate::config::get();
    let Some(source) = config.plugin_sources.iter().find(|s| s.name == name) else {
        bail!(trf!(
            "没有叫「{name}」的插件源",
            "There's no plugin source named “{name}”"
        ));
    };
    set_token(&source.url, token)?;
    if source.auth_header.is_none() && !token.trim().is_empty() {
        let name = name.to_string();
        crate::config::update(move |config| {
            if let Some(source) = config.plugin_sources.iter_mut().find(|s| s.name == name) {
                source.auth_header = Some(DEFAULT_HEADER.to_string());
            }
        })?;
    }
    crate::telemetry::plugin_source(SourceEvent::TokenChanged);
    Ok(())
}

// ---------------------------------------------------------------- 钥匙串

fn test_store() -> Result<PathBuf> {
    Ok(crate::paths::data_dir()?.join("test-plugin-sources.json"))
}

fn read_test_store() -> Result<BTreeMap<String, String>> {
    match std::fs::read_to_string(test_store()?) {
        Ok(text) => Ok(serde_json::from_str(&text).unwrap_or_default()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(err) => Err(err.into()),
    }
}

/// 读这个源的令牌；没存过返回 None。
pub fn token(url: &str) -> Result<Option<String>> {
    if super::secrets::test_mode() {
        return Ok(read_test_store()?.get(url).cloned());
    }
    crate::models::keychain::get_in(SERVICE, url)
}

/// 存令牌；空字符串等于删除。
pub fn set_token(url: &str, value: &str) -> Result<()> {
    let value = value.trim();
    if super::secrets::test_mode() {
        let mut map = read_test_store()?;
        if value.is_empty() {
            map.remove(url);
        } else {
            map.insert(url.to_string(), value.to_string());
        }
        std::fs::write(test_store()?, serde_json::to_string_pretty(&map)?)?;
        return Ok(());
    }
    if value.is_empty() {
        crate::models::keychain::delete_in(SERVICE, url)
    } else {
        crate::models::keychain::set_in(SERVICE, url, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_sources_must_use_https_except_on_this_mac() {
        // 令牌走请求头：明文 http 会把它交给路上的任何人。本机回环地址只给测试用。
        assert_eq!(
            normalize_url(" https://plugins.example.com/team ").unwrap(),
            "https://plugins.example.com/team/"
        );
        assert_eq!(
            normalize_url("http://127.0.0.1:8080").unwrap(),
            "http://127.0.0.1:8080/"
        );
        assert!(normalize_url("http://localhost/x/").is_ok());
        for url in [
            "http://plugins.example.com/",
            "ftp://plugins.example.com/",
            "plugins.example.com",
            "file:///tmp/registry/",
        ] {
            assert!(normalize_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn credentials_never_ride_in_the_url() {
        // 地址会写进 config.json 和报错：用户名密码、?token= 一律拒绝，令牌只能单独填（进钥匙串）。
        for url in [
            "https://user:secret@plugins.example.com/",
            "https://plugins.example.com/?token=secret",
            "https://plugins.example.com/#secret",
        ] {
            let err = normalize_url(url).unwrap_err().to_string();
            assert!(!err.contains("secret"), "报错里不能回显令牌：{err}");
        }
    }

    #[test]
    fn new_sources_need_unique_names_and_urls() {
        let mut config = Config::default();
        config.plugin_sources.push(PluginSource {
            name: "Team".into(),
            url: "https://plugins.example.com/".into(),
            auth_header: None,
        });
        let new = |name: &str, url: &str| PluginSource {
            name: name.into(),
            url: url.into(),
            auth_header: None,
        };
        assert!(check_new(&config, new("Team", "https://other.example.com/")).is_err());
        assert!(check_new(&config, new("Other", "https://plugins.example.com")).is_err());
        assert!(check_new(&config, new(" ", "https://other.example.com/")).is_err());
        assert!(
            check_new(&config, new("Plugin Center", "https://other.example.com/")).is_err(),
            "不能冒用公开插件中心的名字（--source 会指错）"
        );
        assert!(
            check_new(&config, new("Public", crate::providers::plugins::center::DEFAULT_REGISTRY))
                .is_err(),
            "公开插件中心总在，不能再当自定义源加一遍"
        );
        let ok = check_new(&config, new(" Other ", "https://other.example.com")).unwrap();
        assert_eq!((ok.name.as_str(), ok.url.as_str()), ("Other", "https://other.example.com/"));
    }

    #[test]
    fn header_names_are_plain_tokens() {
        assert!(valid_header("Authorization"));
        assert!(valid_header("X-Api-Key"));
        for bad in ["", "Bad Header", "X:Y", "Host", "Cookie", "a\r\nb"] {
            assert!(!valid_header(bad), "{bad:?}");
        }
    }

    #[test]
    fn origin_and_host_are_parsed_case_insensitively() {
        assert_eq!(
            origin("HTTPS://Plugins.Example.com:8443/a/b").as_deref(),
            Some("https://plugins.example.com:8443")
        );
        assert_eq!(host("https://plugins.example.com:8443/a"), "plugins.example.com");
        assert_eq!(host("http://[::1]:9/"), "[::1]");
        assert!(is_loopback("http://[::1]:9/"));
        assert!(!is_loopback("https://127.0.0.1.example.com/"));
    }
}
