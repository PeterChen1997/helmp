//! `ps` 与 `lsof` 输出的解析（纯函数，单测覆盖）。

/// `ps -axo pid=,ppid=,uid=,pcpu=,rss=,comm=` 的一行。
#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub uid: u32,
    /// CPU 占用（%，可超过 100）。
    pub cpu: f32,
    /// 常驻内存，KB。
    pub rss_kb: u64,
    /// 可执行文件路径（`comm`，macOS 上通常是完整路径）。
    pub command: String,
}

impl Proc {
    /// 进程名：可执行文件名。
    pub fn name(&self) -> &str {
        self.command
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.command)
    }
}

/// 解析 `ps -axo pid=,ppid=,uid=,pcpu=,rss=,comm=`：前五列是数字，其余（可能含空格）是路径。
/// 解析不了的行跳过（ps 偶尔会输出被截断的行）。
pub fn parse_ps(output: &str) -> Vec<Proc> {
    output
        .lines()
        .filter_map(|line| {
            let mut rest = line.trim_start();
            let mut next = || {
                let end = rest.find(char::is_whitespace)?;
                let (field, tail) = rest.split_at(end);
                rest = tail.trim_start();
                Some(field)
            };
            let pid = next()?.parse().ok()?;
            let ppid = next()?.parse().ok()?;
            let uid = next()?.parse().ok()?;
            let cpu = next()?.replace(',', ".").parse().ok()?;
            let rss_kb = next()?.parse().ok()?;
            let command = rest.trim_end().to_string();
            (!command.is_empty()).then_some(Proc {
                pid,
                ppid,
                uid,
                cpu,
                rss_kb,
                command,
            })
        })
        .collect()
}

/// 一个监听中的 TCP 端口及占用它的进程。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub pid: i32,
    /// lsof 报的进程名（`c` 字段，可能被截断成 15 个字符）。
    pub command: String,
    pub port: u16,
    /// 监听地址，如 `*`、`127.0.0.1`、`[::1]`。
    pub address: String,
}

/// 解析 `lsof -nP -iTCP -sTCP:LISTEN -Fpcn`：`p<pid>` 开始一个进程，`c<名字>`，
/// 之后每个 `n<地址>:<端口>` 是一个监听套接字。同一进程同一端口（IPv4 + IPv6）只算一次。
pub fn parse_lsof(output: &str) -> Vec<Listener> {
    let mut out: Vec<Listener> = Vec::new();
    let mut pid = None;
    let mut command = String::new();
    for line in output.lines() {
        let (tag, value) = match line.chars().next() {
            Some(tag) => (tag, &line[tag.len_utf8()..]),
            None => continue,
        };
        match tag {
            'p' => {
                pid = value.parse().ok();
                command.clear();
            }
            'c' => command = value.to_string(),
            'n' => {
                let Some(pid) = pid else { continue };
                let Some((address, port)) = value.rsplit_once(':') else {
                    continue;
                };
                // 「127.0.0.1:3000->…」是已连接的套接字，监听的不会有箭头。
                let Ok(port) = port.parse() else { continue };
                if out.iter().any(|l| l.pid == pid && l.port == port) {
                    continue;
                }
                out.push(Listener {
                    pid,
                    command: command.clone(),
                    port,
                    address: address.to_string(),
                });
            }
            _ => {}
        }
    }
    out
}

/// 查询里的端口：`:3000`、`3000`、`port 3000`、`端口 3000`。返回 None 表示不是端口查询。
/// 只有 `:` 或 `port` / `端口` 本身（不带数字）返回 `Some(None)`：列出全部监听端口。
pub fn port_query(query: &str) -> Option<Option<u16>> {
    let text = query.trim();
    let rest = ["port", "端口", ":"]
        .iter()
        .find_map(|prefix| {
            text.get(..prefix.len())
                .filter(|head| head.eq_ignore_ascii_case(prefix))
                .map(|_| text[prefix.len()..].trim_start_matches(':').trim())
        })
        .or_else(|| {
            (!text.is_empty() && text.chars().all(|c| c.is_ascii_digit())).then_some(text)
        })?;
    if rest.is_empty() {
        return Some(None);
    }
    rest.parse().ok().map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_lines_keep_paths_with_spaces() {
        // App 的可执行文件路径常带空格（Google Chrome Helper），按空白切会把名字截断、搜不到。
        let out = "    1     0     0   0.0  12345 /sbin/launchd\n\
                   4321     1   501  12,5 204800 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome\n\
                   garbage line\n\
                   77 1 501 0.0 10\n";
        let procs = parse_ps(out);
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0].name(), "launchd");
        assert_eq!(procs[1].pid, 4321);
        assert_eq!(procs[1].uid, 501);
        assert!((procs[1].cpu - 12.5).abs() < 0.01, "逗号小数（部分 locale）也要认");
        assert_eq!(procs[1].rss_kb, 204_800);
        assert_eq!(procs[1].name(), "Google Chrome");
    }

    #[test]
    fn lsof_groups_sockets_by_process_and_dedups_ipv4_ipv6() {
        // node 常同时监听 IPv4 和 IPv6 的同一端口：列表里出现两行一模一样的结果只会让人困惑。
        let out = "p100\ncnode\nf20\nn*:3000\nf21\nn[::1]:3000\np200\ncpython3.12\nn127.0.0.1:8000\nn127.0.0.1:8001\n";
        let listeners = parse_lsof(out);
        assert_eq!(
            listeners,
            vec![
                Listener { pid: 100, command: "node".into(), port: 3000, address: "*".into() },
                Listener { pid: 200, command: "python3.12".into(), port: 8000, address: "127.0.0.1".into() },
                Listener { pid: 200, command: "python3.12".into(), port: 8001, address: "127.0.0.1".into() },
            ]
        );
        assert!(parse_lsof("").is_empty());
    }

    #[test]
    fn port_queries_in_both_languages() {
        // 「port 3000」「端口 3000」「:3000」都是在问谁占了 3000；普通文字不能被当成端口查询。
        assert_eq!(port_query("port 3000"), Some(Some(3000)));
        assert_eq!(port_query("PORT 8080"), Some(Some(8080)));
        assert_eq!(port_query("端口 3000"), Some(Some(3000)));
        assert_eq!(port_query(":3000"), Some(Some(3000)));
        assert_eq!(port_query("3000"), Some(Some(3000)));
        assert_eq!(port_query(":"), Some(None));
        assert_eq!(port_query("端口"), Some(None));
        assert_eq!(port_query("portal"), None);
        assert_eq!(port_query("port 99999"), None);
        assert_eq!(port_query("safari"), None);
        assert_eq!(port_query(""), None, "空查询是列进程，不是列端口");
    }
}
