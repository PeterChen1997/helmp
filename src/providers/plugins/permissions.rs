//! 权限从清单推导，不靠插件自己声明（只有剪贴板读取要显式声明）：
//! - 网络：所有 http 动作的主机名；整段地址来自偏好设置时取它的 allowedHosts，并标明「来自偏好设置」。
//! - 打开：open 动作的 scheme（https、obsidian…）；地址整段是模板时为「任意链接或文件」。
//! - 剪贴板读取（声明了 clipboard.read）、写入（copy / paste）、模拟按键（paste）、运行脚本（script / list）。
//!
//! 用户插件启用前把这些列给用户确认；确认过的权限键（`keys`）存进配置，清单权限变了要重新确认。

use std::collections::BTreeSet;

use super::manifest::{ActionSpec, Manifest, UrlSource, http_url_source};
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
}

impl Permissions {
    pub fn derive(manifest: &Manifest) -> Self {
        let mut permissions = Self {
            clipboard_read: manifest.reads_clipboard(),
            ..Self::default()
        };
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
        if !self.schemes.is_empty() {
            let schemes: Vec<String> = self.schemes.iter().map(|s| format!("{s}:")).collect();
            lines.push(trf!(
                "打开链接：{}",
                "Open links: {}",
                schemes.join(tr("、", ", "))
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
    fn quicklinks_only_need_open() {
        let p = derive(
            r#"[{"id":"a","title":"A","action":{"type":"open","url":"https://x.com/?q={{query|url}}"}}]"#,
            "",
        );
        assert_eq!(p.keys(), ["open:https"]);
    }
}
