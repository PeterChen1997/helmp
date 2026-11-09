//! 回车跳转：桌面深链 → iTerm2 / Terminal.app 按 tty 选中 tab → 其他终端只激活宿主 App。
//!
//! 活着的会话绝不 `--resume`（会开第二个进程）。`HELMP_DRY_RUN=1` 时只返回将要执行的命令。

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};

use super::process;
use super::state::Jump;
use crate::i18n::trf;
use crate::provider::{Action, Outcome};

/// 一条要执行的外部命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub program: &'static str,
    pub args: Vec<String>,
}

impl Plan {
    fn open(target: impl Into<String>) -> Self {
        Self {
            program: "open",
            args: vec![target.into()],
        }
    }

    fn osascript(script: String) -> Self {
        Self {
            program: "osascript",
            args: vec!["-e".into(), script],
        }
    }

    /// 像 shell 一样渲染，方便 dry-run 时人看、也能直接复制执行。
    pub fn render(&self) -> String {
        std::iter::once(self.program.to_string())
            .chain(self.args.iter().map(|arg| shell_quote(arg)))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn execute(&self) -> Result<()> {
        let output = command(self.program)
            .args(&self.args)
            .output()
            .with_context(|| trf!("启动 {} 失败", "Couldn’t start {}", self.program))?;
        if !output.status.success() {
            bail!(trf!(
                "{} 失败（{}）：{}",
                "{} failed ({}): {}",
                self.program,
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(())
    }
}

fn shell_quote(arg: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c);
    if !arg.is_empty() && arg.chars().all(safe) {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// 子进程一律用 UTF-8 locale：Helmp 可能从 LANG 为空的环境启动，osascript 会把脚本里的中文按 C locale 解码成乱码。
fn command(program: &str) -> Command {
    let mut command = Command::new(program);
    command
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8");
    command
}

fn dry_run() -> bool {
    std::env::var("HELMP_DRY_RUN").as_deref() == Ok("1")
}

pub fn action(jump: Jump) -> Action {
    Action::new(move || {
        let plan = plan(&jump)?;
        if dry_run() {
            return Ok(Outcome::Message(trf!(
                "将执行：{}",
                "Would run: {}",
                plan.render()
            )));
        }
        plan.execute()?;
        Ok(Outcome::Done)
    })
}

/// 解析跳转目标。终端类在这一刻才查 tty，因为会话可能已经换了终端或退出。
pub fn plan(jump: &Jump) -> Result<Plan> {
    match jump {
        Jump::DeepLink(url) => Ok(Plan::open(url.clone())),
        Jump::Pid(pid) => terminal_plan(*pid),
        Jump::CodexLock(lock) => terminal_plan(lock_holder(lock)?),
    }
}

/// 谁持有 Codex 写锁，谁就是那个会话的进程。
fn lock_holder(lock: &Path) -> Result<i32> {
    let output = command("lsof")
        .arg("-t")
        .arg(lock)
        .output()
        .with_context(|| trf!("运行 lsof 失败", "Couldn’t run lsof"))?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.trim().parse().ok())
        .with_context(|| {
            trf!(
                "没有进程持有 {}，会话可能刚结束",
                "No process holds {}; the session may have just ended",
                lock.display()
            )
        })
}

fn terminal_plan(pid: i32) -> Result<Plan> {
    let tty = ps(pid, &["-o", "tty="])?;
    let env = ps(pid, &["eww", "-o", "command="])?;
    let env_var = |key: &str| {
        let prefix = format!("{key}=");
        env.split_whitespace()
            .find_map(|token| token.strip_prefix(&prefix).map(String::from))
    };
    choose_plan(
        Some(tty.trim()).filter(|t| !t.is_empty() && *t != "??"),
        env_var("TERM_PROGRAM").as_deref(),
        host_app(pid),
        env_var("__CFBundleIdentifier").as_deref(),
    )
    .with_context(|| {
        trf!(
            "定位进程 {pid} 的窗口",
            "Locating the window of process {pid}"
        )
    })
}

/// 纯决策部分，单独拿出来便于测试。
pub fn choose_plan(
    tty: Option<&str>,
    term_program: Option<&str>,
    host_bundle: Option<PathBuf>,
    bundle_id: Option<&str>,
) -> Result<Plan> {
    // 经 /usr/bin/login 起的 shell 环境变量 ps 读不到，这时按宿主 App 认终端。
    let host_name = host_bundle
        .as_deref()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    let terminal = term_program.or(match host_name {
        Some("iTerm.app") => Some("iTerm.app"),
        Some("Terminal.app") => Some("Apple_Terminal"),
        _ => None,
    });
    match (tty, terminal) {
        (Some(tty), Some("iTerm.app")) => return Ok(Plan::osascript(iterm_script(tty))),
        (Some(tty), Some("Apple_Terminal")) => return Ok(Plan::osascript(terminal_script(tty))),
        _ => {}
    }
    if let Some(bundle) = host_bundle {
        return Ok(Plan {
            program: "open",
            args: vec!["-a".into(), bundle.display().to_string()],
        });
    }
    if let Some(id) = bundle_id {
        return Ok(Plan {
            program: "open",
            args: vec!["-b".into(), id.into()],
        });
    }
    bail!(trf!(
        "找不到宿主 App（tty={tty:?}，TERM_PROGRAM={term_program:?}）",
        "Couldn’t find the host app (tty={tty:?}, TERM_PROGRAM={term_program:?})"
    ))
}

fn ps(pid: i32, args: &[&str]) -> Result<String> {
    let output = command("ps")
        .args(args)
        .arg("-p")
        .arg(pid.to_string())
        .output()
        .with_context(|| trf!("运行 ps 失败", "Couldn’t run ps"))?;
    if !output.status.success() {
        bail!(trf!(
            "进程 {pid} 已不存在",
            "Process {pid} no longer exists"
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// 沿 ppid 往上爬，第一个可执行文件在 `.app` 里的祖先就是宿主 GUI App。
/// 从父进程开始：会话进程自己可能也在某个 `.app` 里（桌面版 claude 二进制就是），但那不是窗口所在。
fn host_app(pid: i32) -> Option<PathBuf> {
    let mut current = process::ppid(pid)?;
    for _ in 0..32 {
        if current <= 1 {
            return None;
        }
        if let Some(bundle) = process::path(current).and_then(|p| process::outer_app_bundle(&p)) {
            return Some(bundle);
        }
        current = process::ppid(current)?;
    }
    None
}

fn applescript_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn iterm_script(tty: &str) -> String {
    let dev = applescript_string(&format!("/dev/{tty}"));
    let missing = applescript_string(&trf!(
        "iTerm2 里找不到 tty 为 /dev/{tty} 的会话",
        "No iTerm2 session with tty /dev/{tty}"
    ));
    format!(
        "tell application id \"com.googlecode.iterm2\"
\trepeat with w in windows
\t\trepeat with t in tabs of w
\t\t\trepeat with s in sessions of t
\t\t\t\tif tty of s is {dev} then
\t\t\t\t\tselect w
\t\t\t\t\tselect t
\t\t\t\t\tselect s
\t\t\t\t\tactivate
\t\t\t\t\treturn
\t\t\t\tend if
\t\t\tend repeat
\t\tend repeat
\tend repeat
end tell
error {missing}"
    )
}

fn terminal_script(tty: &str) -> String {
    let dev = applescript_string(&format!("/dev/{tty}"));
    let missing = applescript_string(&trf!(
        "Terminal 里找不到 tty 为 /dev/{tty} 的标签页",
        "No Terminal tab with tty /dev/{tty}"
    ));
    format!(
        "tell application id \"com.apple.Terminal\"
\trepeat with w in windows
\t\trepeat with t in tabs of w
\t\t\tif tty of t is {dev} then
\t\t\t\tset selected tab of w to t
\t\t\t\tset index of w to 1
\t\t\t\tactivate
\t\t\t\treturn
\t\t\tend if
\t\tend repeat
\tend repeat
end tell
error {missing}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_links_open_directly() {
        // 桌面会话走深链，不经过 tty 解析；渲染结果就是 dry-run 给用户看的命令。
        let plan = plan(&Jump::DeepLink("codex://threads/abc".into())).unwrap();
        assert_eq!(plan.render(), "open codex://threads/abc");
    }

    #[test]
    fn iterm_and_terminal_select_by_tty() {
        // 同一项目常开多个会话，只有 tty 能精确区分；选错 tab 等于没跳。
        let iterm = choose_plan(Some("ttys003"), Some("iTerm.app"), None, None).unwrap();
        assert_eq!(iterm.program, "osascript");
        assert!(iterm.args[1].contains("com.googlecode.iterm2"));
        assert!(iterm.args[1].contains("tty of s is \"/dev/ttys003\""));

        let terminal = choose_plan(
            Some("ttys007"),
            Some("Apple_Terminal"),
            Some(PathBuf::from("/x.app")),
            None,
        )
        .unwrap();
        assert!(terminal.args[1].contains("com.apple.Terminal"));
        assert!(terminal.args[1].contains("tty of t is \"/dev/ttys007\""));

        // iTerm2 里经 login 起的 shell，ps 读不到 TERM_PROGRAM；靠宿主 App 认出来仍要精确选 tab。
        let no_env = choose_plan(
            Some("ttys000"),
            None,
            Some(PathBuf::from("/Applications/iTerm.app")),
            None,
        )
        .unwrap();
        assert!(no_env.args[1].contains("tty of s is \"/dev/ttys000\""));
    }

    /// 本机真实 iTerm2 shell 的端到端解析：`HELMP_TEST_TTY_PID=<pid> cargo test -- --ignored`。
    #[test]
    #[ignore = "依赖本机正在运行的终端进程"]
    fn resolves_real_terminal_process() {
        let pid: i32 = std::env::var("HELMP_TEST_TTY_PID")
            .unwrap()
            .parse()
            .unwrap();
        let plan = plan(&Jump::Pid(pid)).unwrap();
        println!("{}", plan.render());
    }

    #[test]
    fn other_terminals_only_activate_host_app() {
        // Ghostty / WezTerm / tmux 第一版不精确选 tab，至少把宿主 App 拉到前台。
        let plan = choose_plan(
            Some("ttys001"),
            Some("ghostty"),
            Some(PathBuf::from("/Applications/Ghostty.app")),
            Some("com.mitchellh.ghostty"),
        )
        .unwrap();
        assert_eq!(plan.render(), "open -a /Applications/Ghostty.app");

        let by_id = choose_plan(
            Some("ttys001"),
            Some("tmux"),
            None,
            Some("com.example.term"),
        );
        assert_eq!(by_id.unwrap().render(), "open -b com.example.term");

        // 什么都找不到时要报错，不能假装跳过去了。
        assert!(choose_plan(None, None, None, None).is_err());
    }

    #[test]
    fn shell_quote_only_when_needed() {
        assert_eq!(
            shell_quote("claude://claude.ai/epitaxy/local_1"),
            "claude://claude.ai/epitaxy/local_1"
        );
        assert_eq!(
            shell_quote("/Applications/My App.app"),
            "'/Applications/My App.app'"
        );
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
