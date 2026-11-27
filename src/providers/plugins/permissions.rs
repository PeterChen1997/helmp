//! 权限从清单推导，不靠插件自己声明（只有剪贴板读取要显式声明）：
//! - 网络：所有 http 动作的主机名；整段地址来自偏好设置时取它的 allowedHosts，并标明「来自偏好设置」。
//! - 打开：open 动作的 scheme（https、obsidian…）；地址整段是模板时为「任意链接或文件」。
//! - 剪贴板读取（声明了 clipboard.read）、写入（copy / paste）、模拟按键（paste）、运行脚本（script / list）。
//! - 在搜索框显示寄语（声明了 greetings）。
//! - 提供界面主题（声明了 themes；带日期的会自动换上，单独一个权限键）。
//! - 提供界面主题（声明了 themes；带日期的会自动换上）。
//! - 在后台定时运行脚本（声明了 schedules）、发送系统通知（声明了 notifications）。
//!
//! - 运行本地程序（`mcpServers` 的 `command`）：确认的是完整命令行，参数、环境变量、工作目录变了都要重新确认。
//!
//! 用户插件启用前把这些列给用户确认；确认过的权限键（`keys`）存进配置，清单权限变了要重新确认。
//! 会运行代码的插件（脚本、定时任务、MCP 服务器）还带一个 `code:` 键，取自安装器写下的内容指纹
//! （`registry::CODE_STAMP`）：更新或覆盖安装换了文件，权限类别没变也要重新确认。

use std::collections::BTreeSet;

use sha2::{Digest as _, Sha256};

use super::manifest::{ActionSpec, Manifest, McpServerSpec, UrlSource, http_url_source};
use crate::i18n::{tr, trf};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Permissions {
    /// (主机名, 来自哪个偏好设置)。
    pub hosts: BTreeSet<(String, Option<String>)>,
    pub schemes: BTreeSet<String>,
    pub open_any: bool,
    pub clipboard_read: bool,
    pub clipboard_write: bool,
    pub keystrokes: bool,
    pub exec: bool,
    /// 插件带的 MCP 服务器要运行的本地程序（`npx -y pkg`、`./server.py`）。
    pub processes: BTreeSet<Process>,
    /// 安装时的内容指纹（见 `registry::CODE_STAMP`），只有会运行代码的插件才带；手动放进插件文件夹的没有。
    pub code: Option<String>,
    /// 在搜索框显示插件提供的寄语（清单声明了 `greetings`）。
    pub greeting: bool,
    /// 在后台定时运行脚本（清单声明了 `schedules`）。
    pub background: bool,
    /// 发送系统通知（清单的 permissions 声明了 `notifications`）。
    pub notify: bool,
    /// 提供界面主题（清单声明了 `themes`）。
    pub theme: bool,
    /// 有带日期的主题：到日子会自动换上。
    pub dated_theme: bool,
}

/// 一个要运行的本地程序。用户确认的是整条命令行：只认程序名的话，`npx -y a` 换成 `npx -y b`
/// （或 `sh -c …` 换一段脚本）不会回到待确认。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Process {
    /// 给用户看的完整命令行：环境变量、程序、参数。
    pub line: String,
    /// 确认键 `process:<程序>#<命令行与工作目录的摘要>`。
    pub key: String,
}

impl Process {
    fn new(command: &str, server: &McpServerSpec) -> Self {
        fn word(text: &str) -> String {
            if text.is_empty() || text.contains(|c: char| c.is_whitespace() || c == '"') {
                format!("{text:?}")
            } else {
                text.to_string()
            }
        }
        let mut words: Vec<String> = server
            .env
            .iter()
            .map(|(name, value)| format!("{name}={}", word(value)))
            .collect();
        words.push(word(command));
        words.extend(server.args.iter().map(|arg| word(arg)));
        // 摘要按字段分开算（不用拼好的那一行），免得靠空格和引号拼出两条看起来不同、摘要相同的命令。
        let mut hasher = Sha256::new();
        let mut field = |text: &str| {
            hasher.update((text.len() as u64).to_le_bytes());
            hasher.update(text.as_bytes());
        };
        field(command);
        field(&server.args.len().to_string());
        for arg in &server.args {
            field(arg);
        }
        for (name, value) in &server.env {
            field(name);
            field(value);
        }
        field(server.cwd.as_deref().unwrap_or_default());
        let digest: String = hasher
            .finalize()
            .iter()
            .take(6)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Self {
            line: words.join(" "),
            key: format!("process:{command}#{digest}"),
        }
    }
}

impl Permissions {
    /// 会不会运行代码（脚本、后台任务、本地程序）：这类插件换了文件要重新确认。
    pub fn runs_code(&self) -> bool {
        self.exec || self.background || !self.processes.is_empty()
    }

    pub fn derive(manifest: &Manifest) -> Self {
        let mut permissions = Self {
            clipboard_read: manifest.reads_clipboard(),
            greeting: manifest.greetings.is_some(),
            background: !manifest.schedules.is_empty(),
            notify: manifest.sends_notifications(),
            theme: !manifest.themes.is_empty(),
            dated_theme: manifest.themes.iter().any(|theme| !theme.dates.is_empty()),
            ..Self::default()
        };
        for server in manifest.mcp_servers.values() {
            if let Some(command) = &server.command {
                permissions.processes.insert(Process::new(command, server));
            }
            // 地址在校验时已经要求是静态的。
            if let Some(url) = &server.url
                && let Ok(UrlSource::Static { host }) = http_url_source(url, manifest)
            {
                permissions.hosts.insert((host, None));
            }
        }
        for command in &manifest.commands {
            if let Some(action) = &command.action {
                permissions.add(action, manifest);
            }
            if let Some(list) = &command.list {
                permissions.exec = true;
                match &list.action {
                    Some(action) => permissions.add(action, manifest),
                    // 缺省动作：打开列表项的 arg（网址或路径）。
                    None => permissions.open_any = true,
                }
            }
        }
        permissions
    }

    fn add(&mut self, action: &ActionSpec, manifest: &Manifest) {
        match action {
            ActionSpec::Open(spec) => match spec.url.split_once(':') {
                Some((scheme, _)) if !spec.url.starts_with("{{") => {
                    self.schemes.insert(scheme.to_ascii_lowercase());
                }
                _ => self.open_any = true,
            },
            ActionSpec::Http(spec) => match http_url_source(&spec.url, manifest) {
                Ok(UrlSource::Static { host }) => {
                    self.hosts.insert((host, None));
                }
                Ok(UrlSource::Pref { pref, hosts }) => {
                    for host in hosts {
                        self.hosts.insert((host, Some(pref.clone())));
                    }
                }
                // 校验阶段已经拒绝了这种清单，走不到这里。
                Err(_) => {}
            },
            ActionSpec::Script(_) => self.exec = true,
            ActionSpec::Copy(_) => self.clipboard_write = true,
            ActionSpec::Paste(_) => {
                self.clipboard_write = true;
                self.keystrokes = true;
            }
        }
        // script 的输出也可以复制 / 粘贴。
        if let ActionSpec::Script(spec) = action {
            use super::manifest::ScriptOutput;
            match spec.output {
                ScriptOutput::Copy => self.clipboard_write = true,
                ScriptOutput::Paste => {
                    self.clipboard_write = true;
                    self.keystrokes = true;
                }
                ScriptOutput::Message | ScriptOutput::Silent => {}
            }
        }
    }

    /// 允许访问的主机名（运行时检查用）。
    pub fn allows_host(&self, host: &str) -> bool {
        self.hosts
            .iter()
            .any(|(allowed, _)| allowed.eq_ignore_ascii_case(host))
    }

    /// 稳定的权限键，存进配置作为「用户确认过的权限」。
    pub fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .hosts
            .iter()
            .map(|(host, _)| format!("network:{host}"))
            .collect();
        keys.extend(self.schemes.iter().map(|scheme| format!("open:{scheme}")));
        if self.open_any {
            keys.push("open:*".into());
        }
        if self.clipboard_read {
            keys.push("clipboard:read".into());
        }
        if self.clipboard_write {
            keys.push("clipboard:write".into());
        }
        if self.keystrokes {
            keys.push("keystrokes".into());
        }
        if self.exec {
            keys.push("exec".into());
        }
        if self.greeting {
            keys.push("greeting".into());
        }
        if self.background {
            keys.push("background".into());
        }
        if self.notify {
            keys.push("notify".into());
        }
        // 带日期的主题会自己换上：和只提供主题是不同的权限，加了日期要重新确认。
        if self.dated_theme {
            keys.push("theme:dated".into());
        } else if self.theme {
            keys.push("theme".into());
        }
        keys.extend(self.processes.iter().map(|process| process.key.clone()));
        if let Some(code) = &self.code {
            keys.push(format!("code:{code}"));
        }
        keys.sort();
        keys.dedup();
        keys
    }

    /// 给用户看的权限说明，一条一行。没有任何权限时为空。
    pub fn describe(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for (host, pref) in &self.hosts {
            lines.push(match pref {
                None => trf!("访问网络：{host}", "Network access: {host}"),
                Some(pref) => trf!(
                    "访问网络：{host}（地址来自偏好设置 {pref}）",
                    "Network access: {host} (URL comes from preference {pref})"
                ),
            });
        }
        // 网页链接说「在浏览器里打开网页」，其他 scheme 写成 `obsidian://` 并点明会启动对应 App；
        // 直接列「https:」用户看不懂这是什么权限。
        let web = self.schemes.iter().any(|s| s == "http" || s == "https");
        if web {
            lines.push(tr("在浏览器中打开网页", "Open web pages in your browser").into());
        }
        let apps: Vec<String> = self
            .schemes
            .iter()
            .filter(|s| *s != "http" && *s != "https")
            .map(|s| format!("{s}://"))
            .collect();
        if !apps.is_empty() {
            lines.push(trf!(
                "打开 {} 链接（会启动对应的 App）",
                "Open {} links (launches the matching app)",
                apps.join(tr("、", ", "))
            ));
        }
        if self.open_any {
            lines.push(tr("打开任意链接或文件", "Open any link or file").into());
        }
        if self.clipboard_read {
            lines.push(tr("读取剪贴板", "Read the clipboard").into());
        }
        if self.clipboard_write {
            lines.push(tr("写入剪贴板", "Write to the clipboard").into());
        }
        if self.keystrokes {
            lines.push(
                tr(
                    "向前台应用发送 ⌘V（需要辅助功能权限）",
                    "Send ⌘V to the front app (needs Accessibility permission)",
                )
                .into(),
            );
        }
        if self.exec {
            lines.push(
                tr(
                    "运行插件目录里的脚本（以你的用户身份，不受沙盒限制）",
                    "Run scripts from the plugin folder (as you, not sandboxed)",
                )
                .into(),
            );
        }
        for process in &self.processes {
            let command = &process.line;
            lines.push(trf!(
                "运行本地程序：{command}（MCP 服务器，以你的用户身份，不受沙盒限制）",
                "Run a local program: {command} (MCP server, as you, not sandboxed)"
            ));
        }
        if self.greeting {
            lines.push(tr("在搜索框显示提示语", "Show messages in the search box").into());
        }
        if self.background {
            lines.push(
                tr(
                    "在后台定时运行插件目录里的脚本（Helmp 运行时，以你的用户身份，不受沙盒限制）",
                    "Run scripts from the plugin folder in the background on a schedule (while Helmp is running, as you, not sandboxed)",
                )
                .into(),
            );
        }
        if self.notify {
            lines.push(tr("发送通知", "Send notifications").into());
        }
        if self.dated_theme {
            lines.push(
                tr(
                    "提供界面主题，并在指定日期自动换上",
                    "Provide color themes and switch to them on set dates",
                )
                .into(),
            );
        } else if self.theme {
            lines.push(tr("提供界面主题", "Provide color themes").into());
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::plugins::manifest::parse;

    fn derive(commands: &str, extra: &str) -> Permissions {
        let text = format!(
            r#"{{"schemaVersion":1,"id":"com.example.p","name":"P","version":"1.0.0","author":"A",{extra}"commands":{commands}}}"#
        );
        Permissions::derive(&parse(&text, None).unwrap_or_else(|e| panic!("{e:?}")))
    }

    #[test]
    fn network_hosts_come_from_http_urls() {
        // 用户确认的网络权限必须正好是插件会访问的主机：少了就是暗中联网，多了用户无从判断。
        let p = derive(
            r#"[{"id":"a","title":"A","action":{"type":"http","method":"GET","url":"https://API.example.com:8443/v1?q={{query|url}}"}},
                {"id":"b","title":"B","action":{"type":"open","url":"obsidian://new?content={{query|url}}"}}]"#,
            "",
        );
        assert_eq!(p.keys(), ["network:api.example.com", "open:obsidian"]);
        assert!(p.allows_host("api.example.com"));
        assert!(!p.allows_host("evil.com"));
        assert!(!p.exec && !p.clipboard_read && !p.clipboard_write);
    }

    #[test]
    fn hosts_of_a_url_preference_come_from_allowed_hosts() {
        // flomo 这类「整段 webhook 地址是密钥」的插件：主机名由 allowedHosts 锁定，并说明来自偏好设置。
        let p = derive(
            r#"[{"id":"a","title":"A","action":{"type":"http","method":"POST","url":"{{pref.hook}}","json":{"c":"{{query}}"}}}]"#,
            r#""preferences":[{"id":"hook","type":"secret","title":"Hook","allowedHosts":["flomoapp.com"]}],"#,
        );
        assert_eq!(p.keys(), ["network:flomoapp.com"]);
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            assert_eq!(
                p.describe(),
                ["Network access: flomoapp.com (URL comes from preference hook)"]
            );
        });
    }

    #[test]
    fn scripts_clipboard_and_paste_are_flagged() {
        // 运行脚本、读剪贴板、模拟按键都是敏感能力，启用前必须让用户看到。
        let p = derive(
            r#"[{"id":"a","title":"A","action":{"type":"paste","text":"{{clipboard|upper}}"}},
                {"id":"b","title":"B","list":{"script":{"path":"list.sh"}}}]"#,
            r#""permissions":["clipboard.read"],"#,
        );
        assert_eq!(
            p.keys(),
            [
                "clipboard:read",
                "clipboard:write",
                "exec",
                "keystrokes",
                "open:*"
            ]
        );
        assert_eq!(p.describe().len(), 5);
    }

    #[test]
    fn bundled_mcp_servers_need_process_and_network_consent() {
        // 插件带的 MCP 服务器会运行本地程序、访问网络：启用前必须列给用户看。
        let text = r#"{"schemaVersion":1,"id":"com.example.p","name":"P","version":"1.0.0","author":"A",
            "mcpServers":{"gh":{"command":"npx","args":["-y","x"]},"docs":{"url":"https://MCP.example.com/mcp"}}}"#;
        let p = Permissions::derive(&parse(text, None).unwrap());
        let keys = p.keys();
        assert_eq!(keys.len(), 2, "{keys:?}");
        assert_eq!(keys[0], "network:mcp.example.com");
        assert!(keys[1].starts_with("process:npx#"), "{keys:?}");
        let lines = crate::i18n::with_lang(crate::i18n::Lang::Zh, || p.describe());
        assert!(lines.contains(&"访问网络：mcp.example.com".to_string()), "{lines:?}");
    }

    fn mcp(server: &str) -> Permissions {
        let text = format!(
            r#"{{"schemaVersion":1,"id":"com.example.p","name":"P","version":"1.0.0","author":"A","mcpServers":{{"s":{server}}}}}"#
        );
        Permissions::derive(&parse(&text, None).unwrap_or_else(|e| panic!("{e:?}")))
    }

    #[test]
    fn a_different_command_line_needs_consent_again() {
        // 本地程序不受沙盒限制，跑什么全看参数：`npx -y a` 和 `npx -y b` 是两个程序。
        // 只按程序名确认的话，自动更新换掉参数后新命令行不经确认就运行（CARM-713）。
        let confirmed = mcp(r#"{"command":"npx","args":["-y","pkg-a"]}"#).keys();
        assert_eq!(confirmed, mcp(r#"{"command":"npx","args":["-y","pkg-a"]}"#).keys());
        for changed in [
            r#"{"command":"npx","args":["-y","pkg-b"]}"#,
            r#"{"command":"npx","args":["-y","pkg-a","--extra"]}"#,
            r#"{"command":"npx","args":["-y","pkg-a"],"env":{"NODE_OPTIONS":"--require ./x.js"}}"#,
            r#"{"command":"npx","args":["-y","pkg-a"],"cwd":"other"}"#,
            // 参数的切分也是命令行的一部分。
            r#"{"command":"npx","args":["-y pkg-a"]}"#,
        ] {
            assert_ne!(mcp(changed).keys(), confirmed, "{changed}");
        }
    }

    #[test]
    fn the_consent_card_shows_the_whole_command_line() {
        // 用户要能看到确认的到底是什么：只写「运行本地程序：sh」等于什么都没说。
        let p = mcp(
            r#"{"command":"sh","args":["-c","curl example.com | sh"],"env":{"MODE":"fast"}}"#,
        );
        let lines = crate::i18n::with_lang(crate::i18n::Lang::Zh, || p.describe());
        assert_eq!(
            lines,
            [r#"运行本地程序：MODE=fast sh -c "curl example.com | sh"（MCP 服务器，以你的用户身份，不受沙盒限制）"#]
        );
    }

    #[test]
    fn the_install_stamp_is_part_of_what_was_confirmed() {
        // 脚本权限只是一个「能运行脚本」：更新换了脚本内容，权限类别一样，也得让用户再看一眼。
        let mut p = derive(
            r#"[{"id":"a","title":"A","action":{"type":"script","path":"run.sh"}}]"#,
            "",
        );
        assert!(p.runs_code());
        let unstamped = p.keys();
        p.code = Some("aaaa".into());
        let first = p.keys();
        p.code = Some("bbbb".into());
        assert_ne!(first, unstamped);
        assert_ne!(first, p.keys());
        assert!(first.contains(&"code:aaaa".to_string()), "{first:?}");
    }

    #[test]
    fn quicklinks_only_need_open() {
        let p = derive(
            r#"[{"id":"a","title":"A","action":{"type":"open","url":"https://x.com/?q={{query|url}}"}}]"#,
            "",
        );
        assert_eq!(p.keys(), ["open:https"]);
    }

    #[test]
    fn open_permissions_read_as_plain_language() {
        // 权限卡片给普通用户看：写「https:」没人懂；网页说「在浏览器中打开网页」，
        // 其他 scheme 说明会启动对应的 App。
        let mut perms = Permissions::default();
        perms.schemes.insert("https".into());
        perms.schemes.insert("obsidian".into());
        let lines = crate::i18n::with_lang(crate::i18n::Lang::En, || perms.describe());
        assert!(lines.contains(&"Open web pages in your browser".to_string()), "{lines:?}");
        assert!(
            lines.iter().any(|l| l == "Open obsidian:// links (launches the matching app)"),
            "{lines:?}"
        );
    }

    #[test]
    fn greetings_are_a_permission_that_needs_consent() {
        // 往搜索框里放文字也是插件对 Helmp 的改动：启用前要列出来；后来新加了寄语，权限键变了就要重新确认。
        let without = derive(
            r#"[{"id":"a","title":"A","action":{"type":"open","url":"https://example.com"}}]"#,
            "",
        );
        let with = derive(
            r#"[{"id":"a","title":"A","action":{"type":"open","url":"https://example.com"}}]"#,
            r#""greetings":{"items":["Hello"]},"#,
        );
        assert!(!without.keys().contains(&"greeting".to_string()));
        assert_eq!(with.keys(), ["greeting", "open:https"]);
        assert_ne!(with.keys(), without.keys());
        let lines = crate::i18n::with_lang(crate::i18n::Lang::Zh, || with.describe());
        assert!(lines.contains(&"在搜索框显示提示语".to_string()), "{lines:?}");
    }

    #[test]
    fn themes_need_consent_and_dates_need_it_again() {
        // 换界面配色是插件对 Helmp 的改动，到日子自己换上更是：启用前列出来；新版加了日期要重新确认。
        let command = r#"[{"id":"a","title":"A","action":{"type":"open","url":"https://example.com"}}]"#;
        let colors = r##"{"background":"#ffffff","foreground":"#111111","accent":"#0b63e5","selection":"#0b63e5"}"##;
        let dark = r##"{"background":"#1e1e20","foreground":"#f5f5f7","accent":"#3b8bff","selection":"#0a5fd6"}"##;
        let plain = derive(
            command,
            &format!(r#""themes":[{{"id":"t","name":"T","light":{colors},"dark":{dark}}}],"#),
        );
        let dated = derive(
            command,
            &format!(
                r#""themes":[{{"id":"t","name":"T","light":{colors},"dark":{dark},"dates":[{{"from":"12-20","to":"12-26"}}]}}],"#
            ),
        );
        assert!(plain.keys().contains(&"theme".to_string()));
        assert!(dated.keys().contains(&"theme:dated".to_string()));
        assert_ne!(plain.keys(), dated.keys());
        let lines = crate::i18n::with_lang(crate::i18n::Lang::Zh, || dated.describe());
        assert!(lines.contains(&"提供界面主题，并在指定日期自动换上".to_string()), "{lines:?}");
    }
}
