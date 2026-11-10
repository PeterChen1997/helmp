//! MCP 服务器的配置与解析：
//! - 用户自己加的：`config.mcp.servers`（名字、stdio 命令或 http 地址、是否启用、是否常驻）。
//! - 插件带的：清单里的 `mcpServers`，只有插件启用后才算数，键为 `<插件 id>/<名字>`。
//!
//! 密钥（环境变量 / 请求头的值）只存钥匙串：服务名 `dev.helmp.mcp`，账户 `<服务器名>/env:<变量名>`
//! 或 `<服务器名>/header:<请求头名>`；config.json 里只记「哪些键在钥匙串里」（`secrets`）。
//! 插件带的服务器用 `{{pref.<id>}}` 引用插件的偏好设置（secret 偏好设置照旧在插件的钥匙串条目里）。
//!
//! 测试模式（`HELMP_DATA_DIR` + `HELMP_DRY_RUN=1`，见 `plugins::secrets::test_mode`）绝不碰钥匙串，
//! 改读写数据目录里的 `test-keychain.json`，端到端测试靠它确认「密钥没进 config.json」。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::config::Config;
use crate::i18n::{tr, trf};
use crate::providers::plugins::registry::{self, Plugin, State};
use crate::providers::plugins::secrets as plugin_secrets;
use crate::providers::plugins::template::{Template, Var};

pub const SERVICE: &str = "dev.helmp.mcp";
/// 非常驻服务器空闲多久后停掉。
pub const DEFAULT_IDLE_SECS: u64 = 600;
/// 单次请求（tools/call）默认超时。
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct McpConfig {
    pub servers: Vec<ServerConfig>,
    /// 非常驻服务器空闲这么多秒后停掉。
    pub idle_secs: u64,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            servers: Vec::new(),
            idle_secs: DEFAULT_IDLE_SECS,
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerConfig {
    /// 唯一名字，也是钥匙串账户前缀和工具结果 id 的一部分。
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// 常驻：打开面板时就启动、不因空闲停掉，崩溃后自动重启。
    #[serde(default)]
    pub resident: bool,
    pub transport: Transport,
    /// 值存在钥匙串里的键：`env:<变量名>` 或 `header:<请求头名>`。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
    /// tools/call 的超时秒数；缺省 60。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Transport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        /// 非密钥的环境变量。
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    Http {
        url: String,
        /// 非密钥的请求头。
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

impl Transport {
    /// 列表里的一行说明：`npx -y @x/y` 或网址。
    pub fn summary(&self) -> String {
        match self {
            Transport::Stdio { command, args, .. } => {
                let mut parts = vec![command.clone()];
                parts.extend(args.iter().map(|arg| shell_quote(arg)));
                parts.join(" ")
            }
            Transport::Http { url, .. } => url.clone(),
        }
    }
}

/// 显示用的参数引号：含空格等字符时加单引号。
pub fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@=+,%~".contains(c))
    {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// 把「npx -y "a b"」拆成参数（设置页的参数输入框用）：空白分隔，支持单双引号与反斜杠转义。
pub fn split_args(text: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    in_word = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            (None, c) => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err(tr("引号没有闭合", "Unclosed quote").into());
    }
    if in_word {
        out.push(current);
    }
    Ok(out)
}

/// 名字看起来像密钥的环境变量 / 请求头：值自动存进钥匙串，不写进 config.json。
pub fn looks_secret(name: &str) -> bool {
    const WORDS: [&str; 16] = [
        "KEY", "TOKEN", "SECRET", "PASSWORD", "PASSWD", "PASS", "AUTH", "AUTHORIZATION",
        "CREDENTIAL", "CREDENTIALS", "COOKIE", "SESSION", "PAT", "APIKEY", "ACCESSKEY", "BEARER",
    ];
    name.to_ascii_uppercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|part| WORDS.contains(&part))
}

// ---------------------------------------------------------------- 钥匙串

fn account(server: &str, key: &str) -> String {
    format!("{server}/{key}")
}

fn test_keychain_path() -> Result<PathBuf> {
    Ok(crate::paths::data_dir()?.join("test-keychain.json"))
}

fn read_test_keychain() -> Result<BTreeMap<String, String>> {
    match std::fs::read_to_string(test_keychain_path()?) {
        Ok(text) => Ok(serde_json::from_str(&text).unwrap_or_default()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(err) => Err(err.into()),
    }
}

pub fn secret_get(server: &str, key: &str) -> Result<Option<String>> {
    let account = account(server, key);
    if plugin_secrets::test_mode() {
        return Ok(read_test_keychain()?.get(&account).cloned());
    }
    crate::models::keychain::get_in(SERVICE, &account)
}

/// 保存；空字符串等于删除。
pub fn secret_set(server: &str, key: &str, value: &str) -> Result<()> {
    let account = account(server, key);
    let value = value.trim();
    if plugin_secrets::test_mode() {
        let mut map = read_test_keychain()?;
        if value.is_empty() {
            map.remove(&account);
        } else {
            map.insert(account, value.to_string());
        }
        std::fs::write(test_keychain_path()?, serde_json::to_string_pretty(&map)?)?;
        return Ok(());
    }
    if value.is_empty() {
        crate::models::keychain::delete_in(SERVICE, &account)
    } else {
        crate::models::keychain::set_in(SERVICE, &account, value)
    }
}

/// 删除一个服务器的全部密钥（删除服务器时）。
pub fn forget_secrets(server: &ServerConfig) -> Result<()> {
    for key in &server.secrets {
        secret_set(&server.name, key, "")?;
    }
    Ok(())
}

/// 存配置前的整理：看起来像密钥的环境变量 / 请求头挪进钥匙串，config 里只留键名。
/// `force_secret` 里的键无论名字如何都当密钥。返回可以写进 config.json 的版本。
pub fn store_secrets(mut server: ServerConfig, force_secret: &[String]) -> Result<ServerConfig> {
    let name = server.name.clone();
    let mut moved = Vec::new();
    match &mut server.transport {
        Transport::Stdio { env, .. } => {
            for (key, value) in std::mem::take(env) {
                let tagged = format!("env:{key}");
                if looks_secret(&key) || force_secret.contains(&tagged) {
                    moved.push((tagged, value));
                } else {
                    env.insert(key, value);
                }
            }
        }
        Transport::Http { headers, .. } => {
            for (key, value) in std::mem::take(headers) {
                let tagged = format!("header:{key}");
                if looks_secret(&key) || force_secret.contains(&tagged) {
                    moved.push((tagged, value));
                } else {
                    headers.insert(key, value);
                }
            }
        }
    }
    for (key, value) in moved {
        if value.trim().is_empty() {
            continue;
        }
        secret_set(&name, &key, &value)?;
        if !server.secrets.contains(&key) {
            server.secrets.push(key);
        }
    }
    server.secrets.sort();
    Ok(server)
}

// ---------------------------------------------------------------- 校验

pub fn valid_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.chars().count() <= 64
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || "-_. ".contains(c))
        && name.trim() == name
}

/// 新增 / 编辑时的校验。`previous` 是编辑前的名字（改名时允许和自己同名）。
pub fn validate(config: &Config, server: &ServerConfig, previous: Option<&str>) -> Result<()> {
    if !valid_name(&server.name) {
        bail!(trf!(
            "名字 {:?} 不合法：只能用字母、数字、中文、空格、- _ .，最长 64 个字",
            "Invalid name {:?}: use letters, digits, spaces, - _ . (max 64)",
            server.name
        ));
    }
    let taken = config
        .mcp
        .servers
        .iter()
        .any(|other| other.name == server.name && Some(other.name.as_str()) != previous);
    if taken {
        bail!(trf!(
            "已经有叫「{}」的服务器了",
            "A server named “{}” already exists",
            server.name
        ));
    }
    match &server.transport {
        Transport::Stdio { command, env, .. } => {
            if command.trim().is_empty() {
                bail!(tr("命令不能为空", "The command can't be empty"));
            }
            for key in env.keys() {
                if !valid_env_name(key) {
                    bail!(trf!(
                        "环境变量名 {key:?} 只能用字母、数字和 _",
                        "Environment variable {key:?} may only use letters, digits and _"
                    ));
                }
            }
        }
        Transport::Http { url, .. } => {
            host_of_url(url)?;
        }
    }
    Ok(())
}

pub fn valid_env_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// http(s) 地址的主机名（小写，不含端口）。
pub fn host_of_url(url: &str) -> Result<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .with_context(|| {
            trf!(
                "地址 {url:?} 要以 https:// 或 http:// 开头",
                "URL {url:?} must start with https:// or http://"
            )
        })?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(v6) = authority.strip_prefix('[') {
        format!("[{}]", v6.split(']').next().unwrap_or_default())
    } else {
        authority.split(':').next().unwrap_or_default().to_string()
    };
    if host.is_empty() {
        bail!(trf!("地址 {url:?} 没有主机名", "URL {url:?} has no host"));
    }
    Ok(host.to_ascii_lowercase())
}

pub fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

// ---------------------------------------------------------------- 运行时规格

#[derive(Debug, Clone)]
pub enum Origin {
    User,
    Plugin { id: String, name: String },
}

/// 一个可以启动的服务器（用户的或插件带的）。
#[derive(Debug, Clone)]
pub struct Spec {
    /// 用户服务器是名字，插件的是 `<插件 id>/<名字>`。
    pub key: String,
    pub name: String,
    pub origin: Origin,
    pub enabled: bool,
    pub resident: bool,
    pub transport: Transport,
    pub secrets: Vec<String>,
    pub timeout: Duration,
    /// 插件带的服务器：环境变量 / 请求头里的 `{{pref.*}}` 从这个插件取值，相对命令在它的目录里。
    pub plugin: Option<Arc<Plugin>>,
}

impl Spec {
    pub fn from_config(server: &ServerConfig) -> Self {
        Spec {
            key: server.name.clone(),
            name: server.name.clone(),
            origin: Origin::User,
            enabled: server.enabled,
            resident: server.resident,
            transport: server.transport.clone(),
            secrets: server.secrets.clone(),
            timeout: Duration::from_secs(server.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).max(1)),
            plugin: None,
        }
    }

    /// 配置指纹：命令、参数、地址、键名变了，缓存的工具列表就作废（不含密钥的值）。
    pub fn fingerprint(&self) -> String {
        let text = serde_json::json!({
            "transport": self.transport,
            "secrets": self.secrets,
            "plugin": self.plugin.as_ref().map(|p| p.manifest.version.clone()),
        })
        .to_string();
        let digest = Sha256::digest(text.as_bytes());
        digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
    }

    /// 来源说明（设置页、`helmp mcp-client list`）。
    pub fn origin_label(&self) -> String {
        match &self.origin {
            Origin::User => tr("我的服务器", "My server").into(),
            Origin::Plugin { name, .. } => trf!("插件「{name}」", "Plugin “{name}”"),
        }
    }

    /// 实际启动用的环境变量 / 请求头：密钥从钥匙串取，插件模板按偏好设置渲染。
    pub fn resolved(&self) -> Result<Transport> {
        let mut transport = self.transport.clone();
        let map = match &mut transport {
            Transport::Stdio { env, .. } => (env, "env:"),
            Transport::Http { headers, .. } => (headers, "header:"),
        };
        let (values, prefix) = map;
        if let Some(plugin) = &self.plugin {
            for value in values.values_mut() {
                *value = render_pref_template(plugin, value)?;
            }
        } else {
            for key in &self.secrets {
                let Some(name) = key.strip_prefix(prefix) else {
                    continue;
                };
                let value = secret_get(&self.name, key)?.unwrap_or_default();
                if !value.is_empty() {
                    values.insert(name.to_string(), value);
                }
            }
        }
        if let (Some(plugin), Transport::Stdio { command, cwd, .. }) = (&self.plugin, &mut transport)
            && let Some(dir) = &plugin.dir
        {
            if command.starts_with("./") {
                *command = dir.join(&command[2..]).display().to_string();
            }
            let base = cwd.clone().unwrap_or_default();
            *cwd = Some(if base.is_empty() {
                dir.display().to_string()
            } else {
                dir.join(base).display().to_string()
            });
        }
        Ok(transport)
    }

    /// 解析后的密钥值（错误信息、日志里打码用）。
    pub fn secret_values(&self, transport: &Transport) -> Vec<String> {
        let values = match transport {
            Transport::Stdio { env, .. } => env,
            Transport::Http { headers, .. } => headers,
        };
        let mut out = Vec::new();
        for (key, value) in values {
            let secret = if self.plugin.is_some() {
                looks_secret(key)
                    || match &self.transport {
                        Transport::Stdio { env, .. } => env.get(key),
                        Transport::Http { headers, .. } => headers.get(key),
                    }
                    .is_some_and(|raw| raw.contains("{{"))
            } else {
                self.secrets.iter().any(|tagged| tagged.ends_with(&format!(":{key}")))
            };
            if secret && value.chars().count() >= 4 {
                out.push(value.clone());
            }
        }
        out
    }
}

/// 只允许 `{{pref.<id>}}` 的模板（插件清单的 mcpServers 校验时已经保证）。
fn render_pref_template(plugin: &Plugin, text: &str) -> Result<String> {
    let template = Template::parse(text)?;
    // 只认 pref 变量。
    struct Prefs<'a>(&'a Plugin);
    impl crate::providers::plugins::template::Values for Prefs<'_> {
        fn value(&self, var: &Var) -> Result<String> {
            match var {
                Var::Pref(key) => crate::providers::plugins::exec::pref_value(self.0, key),
                _ => bail!(tr(
                    "MCP 服务器的环境变量只能引用 {{pref.*}}",
                    "MCP server values may only use {{pref.*}}"
                )),
            }
        }
    }
    template.render(&Prefs(plugin))
}

/// 当前所有服务器：用户的（含停用的）+ 已启用插件带的。
pub fn all(config: &Config) -> Vec<Spec> {
    let mut specs: Vec<Spec> = config.mcp.servers.iter().map(Spec::from_config).collect();
    for entry in registry::current().iter() {
        if entry.state(config) != State::Enabled {
            continue;
        }
        let Some(plugin) = entry.plugin() else {
            continue;
        };
        for (name, server) in &plugin.manifest.mcp_servers {
            let transport = match (&server.command, &server.url) {
                (Some(command), None) => Transport::Stdio {
                    command: command.clone(),
                    args: server.args.clone(),
                    env: server.env.clone(),
                    cwd: server.cwd.clone(),
                },
                (None, Some(url)) => Transport::Http {
                    url: url.clone(),
                    headers: server.headers.clone(),
                },
                // 校验阶段已经拒绝。
                _ => continue,
            };
            specs.push(Spec {
                key: format!("{}/{name}", plugin.id()),
                name: name.clone(),
                origin: Origin::Plugin {
                    id: plugin.id().to_string(),
                    name: plugin.manifest.name.get().to_string(),
                },
                enabled: true,
                resident: false,
                transport,
                secrets: Vec::new(),
                timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
                plugin: Some(Arc::clone(plugin)),
            });
        }
    }
    specs
}

pub fn find(config: &Config, key: &str) -> Option<Spec> {
    all(config).into_iter().find(|spec| spec.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_looking_names_are_detected() {
        // 名字像密钥的值不能落进 config.json：config.json 会被同步、备份、贴进 issue。
        for name in [
            "GITHUB_PERSONAL_ACCESS_TOKEN",
            "OPENAI_API_KEY",
            "Authorization",
            "X-Api-Key",
            "CONTEXT7_API_KEY",
            "Context7-API-Key",
            "DB_PASSWORD",
            "client_secret",
        ] {
            assert!(looks_secret(name), "{name}");
        }
        for name in ["PATH", "NODE_ENV", "DEBUG", "Accept", "MONKEY_ISLAND_PATHS", "LOG_LEVEL"] {
            assert!(!looks_secret(name), "{name}");
        }
    }

    #[test]
    fn args_split_like_a_shell_without_running_one() {
        assert_eq!(
            split_args(r#"-y @x/server "/Users/me/My Docs" 'a b' c\ d"#).unwrap(),
            ["-y", "@x/server", "/Users/me/My Docs", "a b", "c d"]
        );
        assert_eq!(split_args("  ").unwrap(), Vec::<String>::new());
        assert_eq!(split_args(r#""""#).unwrap(), [""]);
        assert!(split_args("'open").is_err());
        assert_eq!(shell_quote("/My Docs"), "'/My Docs'");
        assert_eq!(shell_quote("@x/y"), "@x/y");
    }

    #[test]
    fn hosts_are_parsed_without_ports_or_userinfo() {
        assert_eq!(
            host_of_url("https://user@MCP.Example.com:8443/mcp?x").unwrap(),
            "mcp.example.com"
        );
        assert_eq!(host_of_url("http://127.0.0.1:9/mcp").unwrap(), "127.0.0.1");
        assert_eq!(host_of_url("http://[::1]:9/mcp").unwrap(), "[::1]");
        assert!(host_of_url("ftp://x").is_err());
        assert!(host_of_url("https:///x").is_err());
    }

    #[test]
    fn config_round_trips_in_the_documented_shape() {
        // 文档里写的 config.json 形状要能读：stdio / http 两种传输，enabled 缺省为开。
        let config: McpConfig = serde_json::from_str(
            r#"{"servers":[
                {"name":"fs","transport":{"type":"stdio","command":"npx","args":["-y","x"],"env":{"A":"1"}}},
                {"name":"web","enabled":false,"resident":true,"transport":{"type":"http","url":"https://e.com/mcp"},"secrets":["header:Authorization"]}
            ]}"#,
        )
        .unwrap();
        assert_eq!(config.idle_secs, DEFAULT_IDLE_SECS);
        assert!(config.servers[0].enabled);
        assert!(!config.servers[1].enabled && config.servers[1].resident);
        assert_eq!(config.servers[0].transport.summary(), "npx -y x");
        let again: McpConfig = serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert_eq!(again, config);
    }

    #[test]
    fn names_are_checked() {
        assert!(valid_name("github"));
        assert!(valid_name("我的 文件"));
        assert!(!valid_name(""));
        assert!(!valid_name("a/b"), "插件服务器的键用 / 分隔，用户名字里不能有");
        assert!(!valid_name(" pad"));
    }
}
