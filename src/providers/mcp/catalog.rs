//! 「精选」：编译进来的一小份常用服务器。命令照抄各自官方 README（2026-11 核对）：
//! - modelcontextprotocol/servers：filesystem、memory、sequential-thinking（npx -y），fetch、git、time（uvx）。
//! - microsoft/playwright-mcp：`npx @playwright/mcp@latest`。
//! - upstash/context7：远程 `https://mcp.context7.com/mcp`，API Key 可选（请求头 `Context7-API-Key`）。
//!
//! 添加前设置页会把要运行的完整命令、要填的环境变量列出来给用户看。

use std::collections::BTreeMap;

use super::servers::{ServerConfig, Transport};
use crate::i18n::tr;

/// 需要用户填的值（环境变量或请求头）。
#[derive(Debug, Clone, Copy)]
pub struct Need {
    /// `env:NAME` / `header:Name` / `arg`（替换参数里的 `{value}`）。
    pub key: &'static str,
    pub label_zh: &'static str,
    pub label_en: &'static str,
    pub required: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub id: &'static str,
    pub title: &'static str,
    pub summary_zh: &'static str,
    pub summary_en: &'static str,
    /// 要先装好的东西（npx 要 Node.js，uvx 要 uv）。
    pub requires: &'static str,
    pub command: Option<&'static str>,
    pub args: &'static [&'static str],
    pub url: Option<&'static str>,
    pub needs: &'static [Need],
    pub homepage: &'static str,
}

const REFERENCE: &str = "https://github.com/modelcontextprotocol/servers";

pub const ENTRIES: &[Entry] = &[
    Entry {
        id: "filesystem",
        title: "Filesystem",
        summary_zh: "读写指定文件夹里的文件（只能访问你给的目录）",
        summary_en: "Read and write files in the folder you allow",
        requires: "Node.js (npx)",
        command: Some("npx"),
        args: &["-y", "@modelcontextprotocol/server-filesystem", "{value}"],
        url: None,
        needs: &[Need {
            key: "arg",
            label_zh: "允许访问的文件夹",
            label_en: "Folder to allow",
            required: true,
        }],
        homepage: REFERENCE,
    },
    Entry {
        id: "fetch",
        title: "Fetch",
        summary_zh: "抓取网页并转成 Markdown",
        summary_en: "Fetch web pages as Markdown",
        requires: "uv (uvx)",
        command: Some("uvx"),
        args: &["mcp-server-fetch"],
        url: None,
        needs: &[],
        homepage: REFERENCE,
    },
    Entry {
        id: "git",
        title: "Git",
        summary_zh: "查看与操作一个 Git 仓库（状态、diff、日志、提交）",
        summary_en: "Inspect and work with a Git repository",
        requires: "uv (uvx)",
        command: Some("uvx"),
        args: &["mcp-server-git", "--repository", "{value}"],
        url: None,
        needs: &[Need {
            key: "arg",
            label_zh: "仓库路径",
            label_en: "Repository path",
            required: true,
        }],
        homepage: REFERENCE,
    },
    Entry {
        id: "memory",
        title: "Memory",
        summary_zh: "基于知识图谱的长期记忆",
        summary_en: "Knowledge-graph based persistent memory",
        requires: "Node.js (npx)",
        command: Some("npx"),
        args: &["-y", "@modelcontextprotocol/server-memory"],
        url: None,
        needs: &[],
        homepage: REFERENCE,
    },
    Entry {
        id: "sequential-thinking",
        title: "Sequential Thinking",
        summary_zh: "分步骤思考、可修订的推理工具",
        summary_en: "Step-by-step, revisable reasoning",
        requires: "Node.js (npx)",
        command: Some("npx"),
        args: &["-y", "@modelcontextprotocol/server-sequential-thinking"],
        url: None,
        needs: &[],
        homepage: REFERENCE,
    },
    Entry {
        id: "time",
        title: "Time",
        summary_zh: "当前时间与时区换算",
        summary_en: "Current time and time zone conversion",
        requires: "uv (uvx)",
        command: Some("uvx"),
        args: &["mcp-server-time"],
        url: None,
        needs: &[],
        homepage: REFERENCE,
    },
    Entry {
        id: "playwright",
        title: "Playwright",
        summary_zh: "用浏览器打开网页、点击、填表、截图",
        summary_en: "Drive a browser: open pages, click, fill forms, take screenshots",
        requires: "Node.js (npx)",
        command: Some("npx"),
        args: &["@playwright/mcp@latest"],
        url: None,
        needs: &[],
        homepage: "https://github.com/microsoft/playwright-mcp",
    },
    Entry {
        id: "context7",
        title: "Context7",
        summary_zh: "查询各类库与框架的最新文档（远程服务）",
        summary_en: "Up-to-date docs for libraries and frameworks (remote)",
        requires: "",
        command: None,
        args: &[],
        url: Some("https://mcp.context7.com/mcp"),
        needs: &[Need {
            key: "header:Context7-API-Key",
            label_zh: "API Key（可选，提高限额）",
            label_en: "API key (optional, raises limits)",
            required: false,
        }],
        homepage: "https://github.com/upstash/context7",
    },
];

impl Entry {
    pub fn summary(&self) -> &'static str {
        tr(self.summary_zh, self.summary_en)
    }

    /// 要运行的完整命令（或地址），添加前给用户看。
    pub fn command_line(&self, value: &str) -> String {
        match (self.command, self.url) {
            (Some(command), _) => {
                let mut parts = vec![command.to_string()];
                for arg in self.args {
                    let arg = if *arg == "{value}" {
                        if value.is_empty() {
                            "<…>".to_string()
                        } else {
                            super::servers::shell_quote(value)
                        }
                    } else {
                        (*arg).to_string()
                    };
                    parts.push(arg);
                }
                parts.join(" ")
            }
            (None, Some(url)) => url.to_string(),
            (None, None) => String::new(),
        }
    }

    /// 生成配置。`values`：need.key → 用户填的值（`~` 开头的路径展开）。缺必填项返回其名字。
    pub fn server(&self, values: &BTreeMap<String, String>) -> Result<ServerConfig, String> {
        for need in self.needs {
            if need.required && values.get(need.key).is_none_or(|v| v.trim().is_empty()) {
                return Err(tr(need.label_zh, need.label_en).to_string());
            }
        }
        let expand = |value: &str| -> String {
            match value.strip_prefix("~/") {
                Some(rest) => crate::paths::home_dir()
                    .map(|home| home.join(rest).display().to_string())
                    .unwrap_or_else(|_| value.to_string()),
                None => value.to_string(),
            }
        };
        let transport = match (self.command, self.url) {
            (Some(command), _) => {
                let mut env = BTreeMap::new();
                for need in self.needs {
                    if let Some(name) = need.key.strip_prefix("env:")
                        && let Some(value) = values.get(need.key).filter(|v| !v.trim().is_empty())
                    {
                        env.insert(name.to_string(), value.trim().to_string());
                    }
                }
                Transport::Stdio {
                    command: command.to_string(),
                    args: self
                        .args
                        .iter()
                        .map(|arg| {
                            if *arg == "{value}" {
                                expand(values.get("arg").map(|v| v.trim()).unwrap_or_default())
                            } else {
                                (*arg).to_string()
                            }
                        })
                        .collect(),
                    env,
                    cwd: None,
                }
            }
            (None, Some(url)) => {
                let mut headers = BTreeMap::new();
                for need in self.needs {
                    if let Some(name) = need.key.strip_prefix("header:")
                        && let Some(value) = values.get(need.key).filter(|v| !v.trim().is_empty())
                    {
                        headers.insert(name.to_string(), value.trim().to_string());
                    }
                }
                Transport::Http {
                    url: url.to_string(),
                    headers,
                }
            }
            (None, None) => return Err(self.id.to_string()),
        };
        Ok(ServerConfig {
            name: self.id.to_string(),
            enabled: true,
            resident: false,
            transport,
            secrets: Vec::new(),
            timeout_secs: None,
        })
    }
}

pub fn find(id: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|entry| entry.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_entries_build_valid_configs() {
        // 一键添加生成的配置必须能通过和手动添加一样的校验，命令行与 README 一致。
        let config = crate::config::Config::default();
        for entry in ENTRIES {
            let mut values = BTreeMap::new();
            values.insert("arg".to_string(), "/tmp/x".to_string());
            let server = entry.server(&values).unwrap();
            super::super::servers::validate(&config, &server, None)
                .unwrap_or_else(|err| panic!("{}: {err:#}", entry.id));
        }
        let fs = find("filesystem").unwrap();
        assert_eq!(
            fs.command_line("/Users/me/My Docs"),
            "npx -y @modelcontextprotocol/server-filesystem '/Users/me/My Docs'"
        );
        assert!(fs.server(&BTreeMap::new()).is_err(), "缺必填的目录");
        assert_eq!(find("time").unwrap().command_line(""), "uvx mcp-server-time");
    }

    #[test]
    fn optional_keys_become_secret_headers() {
        let mut values = BTreeMap::new();
        values.insert("header:Context7-API-Key".to_string(), "ctx7sk-1".to_string());
        let server = find("context7").unwrap().server(&values).unwrap();
        assert!(matches!(&server.transport, Transport::Http { headers, .. }
            if headers["Context7-API-Key"] == "ctx7sk-1"));
        assert!(super::super::servers::looks_secret("Context7-API-Key"));
    }
}
