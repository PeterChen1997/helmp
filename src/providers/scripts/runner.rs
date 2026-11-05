//! 执行脚本：选解释器、限时运行、按 mode 把输出变成 `Outcome`。
//!
//! 熔断参考 `_research/launcher/08-plugin-system-best-practices.md`：
//! 脚本是一次性命令（非每键查询），给 30 秒（与 Steward net 请求上限同值）；
//! 超时对整个进程组发 SIGKILL，连脚本派生的子进程一起清掉，不留孤儿。
//! 输出每路最多保留末尾 1 MiB，防止失控脚本吃光内存。

use std::fs;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};

use super::header::Mode;
use crate::provider::Outcome;

/// 单次执行的硬上限。
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// 每路输出最多保留的字节数（保留末尾）。
const OUTPUT_CAP: usize = 1024 * 1024;
/// 报错时附带的 stderr 尾部行数。
const ERROR_TAIL_LINES: usize = 10;

#[derive(Debug)]
pub struct RunOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// 可执行文件直接 exec；否则按 shebang 调解释器。不会 chmod 用户文件。
pub fn command_for(path: &Path) -> Result<Command> {
    let metadata = fs::metadata(path).with_context(|| format!("读取 {}", path.display()))?;
    if metadata.permissions().mode() & 0o111 != 0 {
        return Ok(Command::new(path));
    }
    let mut first_line = String::new();
    fs::File::open(path)
        .with_context(|| format!("打开 {}", path.display()))?
        .take(4096)
        .read_to_string(&mut first_line)
        .ok();
    let shebang = first_line
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("#!"))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .with_context(|| {
            format!(
                "{} 既不可执行也没有 shebang 行，无法确定解释器（可以 chmod +x 或加 #!）",
                path.display()
            )
        })?;
    let mut parts = shebang.split_whitespace();
    let program = parts.next().context("shebang 为空")?;
    let mut command = Command::new(program);
    command.args(parts).arg(path);
    Ok(command)
}

/// `currentDirectoryPath`（支持 `~`，相对路径相对脚本所在目录），缺省为脚本所在目录。
pub fn working_dir(configured: Option<&str>, script: &Path) -> Result<PathBuf> {
    let script_dir = script
        .parent()
        .map(Path::to_path_buf)
        .context("脚本路径没有上级目录")?;
    let Some(configured) = configured else {
        return Ok(script_dir);
    };
    let dir = if configured == "~" {
        crate::paths::home_dir()?
    } else if let Some(rest) = configured.strip_prefix("~/") {
        crate::paths::home_dir()?.join(rest)
    } else {
        script_dir.join(configured)
    };
    anyhow::ensure!(
        dir.is_dir(),
        "currentDirectoryPath {configured} 不是存在的目录（{}）",
        dir.display()
    );
    Ok(dir)
}

/// 运行脚本直到退出且输出管道关闭，超过 `timeout` 杀掉整个进程组并报错。
pub fn execute(path: &Path, args: &[String], cwd: &Path, timeout: Duration) -> Result<RunOutput> {
    let mut command = command_for(path)?;
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 新进程组，组号 = 子进程 pid，超时时整组一起杀。
        .process_group(0);
    let mut child = command
        .spawn()
        .with_context(|| format!("启动脚本 {}", path.display()))?;
    let pgid = i32::try_from(child.id()).context("pid 超出 i32")?;

    let (sender, receiver) = mpsc::channel();
    spawn_reader(
        0,
        child.stdout.take().context("stdout 管道未创建")?,
        sender.clone(),
    );
    spawn_reader(1, child.stderr.take().context("stderr 管道未创建")?, sender);

    let deadline = Instant::now() + timeout;
    let mut status = None;
    let mut outputs: [Option<Vec<u8>>; 2] = [None, None];
    loop {
        if status.is_none() {
            status = child.try_wait().context("等待脚本退出")?;
        }
        if let (Some(status), [Some(_), Some(_)]) = (status, &outputs) {
            let [stdout, stderr] = outputs
                .map(|bytes| String::from_utf8_lossy(&bytes.unwrap_or_default()).into_owned());
            return Ok(RunOutput {
                status,
                stdout,
                stderr,
            });
        }
        let now = Instant::now();
        if now >= deadline {
            kill_group(pgid);
            let _ = child.wait();
            bail!(
                "脚本 {} 超过 {} 秒未结束，已终止其进程组",
                path.display(),
                timeout.as_secs_f32()
            );
        }
        let wait = (deadline - now).min(Duration::from_millis(10));
        if let Ok((slot, bytes)) = receiver.recv_timeout(wait)
            && let Some(output) = outputs.get_mut(slot)
        {
            *output = Some(bytes);
        }
    }
}

fn kill_group(pgid: i32) {
    // SAFETY: kill(2) 只读参数，负 pid 表示向整个进程组发信号。
    let result = unsafe { libc::kill(-pgid, libc::SIGKILL) };
    if result != 0 {
        // ESRCH 表示整组已经退出，不算错误；其他情况要说出来。
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            eprintln!("huan: scripts 终止进程组 {pgid} 失败：{err}");
        }
    }
}

fn spawn_reader(
    slot: usize,
    pipe: impl std::io::Read + Send + 'static,
    sender: mpsc::Sender<(usize, Vec<u8>)>,
) {
    std::thread::spawn(move || {
        // 发送失败只可能是 execute 已经返回（超时），那时结果没人要了。
        let _ = sender.send((slot, read_tail(pipe)));
    });
}

/// 读完整个管道，只保留末尾 `OUTPUT_CAP` 字节。
fn read_tail(mut pipe: impl std::io::Read) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                kept.extend_from_slice(chunk.get(..n).unwrap_or_default());
                // 攒到两倍再裁，避免每读一块都整体搬移。
                if kept.len() > 2 * OUTPUT_CAP {
                    kept.drain(..kept.len() - OUTPUT_CAP);
                    truncated = true;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(err) => {
                kept.extend_from_slice(format!("\n[读取输出失败：{err}]").as_bytes());
                break;
            }
        }
    }
    if kept.len() > OUTPUT_CAP {
        kept.drain(..kept.len() - OUTPUT_CAP);
        truncated = true;
    }
    if truncated {
        let mut marked = "[...前面的输出已截断]\n".as_bytes().to_vec();
        marked.extend(kept);
        return marked;
    }
    kept
}

fn last_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines
        .get(lines.len().saturating_sub(count)..)
        .unwrap_or_default()
        .join("\n")
}

/// 按 mode 处理输出；非零退出码一律报错并带 stderr 尾部。
pub fn outcome(title: &str, mode: Mode, output: &RunOutput) -> Result<Outcome> {
    if !output.status.success() {
        let how = match (output.status.code(), output.status.signal()) {
            (Some(code), _) => format!("退出码 {code}"),
            (None, Some(signal)) => format!("被信号 {signal} 终止"),
            (None, None) => format!("异常退出（{}）", output.status),
        };
        let detail = if output.stderr.trim().is_empty() {
            &output.stdout
        } else {
            &output.stderr
        };
        let tail = last_lines(detail, ERROR_TAIL_LINES);
        return Err(if tail.is_empty() {
            anyhow!("脚本「{title}」失败，{how}")
        } else {
            anyhow!("脚本「{title}」失败，{how}：\n{tail}")
        });
    }
    Ok(match mode {
        Mode::Silent => Outcome::Done,
        Mode::Compact | Mode::Inline => {
            let last = output
                .stdout
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty());
            Outcome::Message(
                last.map_or_else(|| format!("「{title}」已完成（没有输出）"), str::to_string),
            )
        }
        Mode::FullOutput => {
            let parts: Vec<&str> = [output.stdout.trim_end(), output.stderr.trim_end()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect();
            if parts.is_empty() {
                Outcome::Message(format!("「{title}」已完成（没有输出）"))
            } else {
                Outcome::Message(parts.join("\n"))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::scripts::tests::{TempDir, write_script};

    fn output(code: i32, stdout: &str, stderr: &str) -> RunOutput {
        RunOutput {
            // wait(2) 状态编码：退出码在高 8 位。
            status: ExitStatus::from_raw(code << 8),
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn modes_shape_the_output() {
        // mode 决定用户看到什么：silent 不打扰，compact 只要结论，fullOutput 要全文（含 stderr 里的诊断）。
        let ok = output(0, "第一行\n结果 42\n\n", "warn: x\n");
        assert_eq!(outcome("t", Mode::Silent, &ok).unwrap(), Outcome::Done);
        assert_eq!(
            outcome("t", Mode::Compact, &ok).unwrap(),
            Outcome::Message("结果 42".into())
        );
        assert_eq!(
            outcome("t", Mode::Inline, &ok).unwrap(),
            Outcome::Message("结果 42".into())
        );
        assert_eq!(
            outcome("t", Mode::FullOutput, &ok).unwrap(),
            Outcome::Message("第一行\n结果 42\nwarn: x".into())
        );
        assert_eq!(
            outcome("t", Mode::Compact, &output(0, "", "")).unwrap(),
            Outcome::Message("「t」已完成（没有输出）".into())
        );
    }

    #[test]
    fn non_zero_exit_is_an_error_with_stderr_tail() {
        // 失败要大声：即使是 silent 脚本，非零退出也必须让用户看到原因。
        let stderr: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        let err = outcome("t", Mode::Silent, &output(3, "", &stderr)).unwrap_err();
        let message = format!("{err}");
        assert!(message.contains("退出码 3"), "{message}");
        assert!(message.contains("line 30") && message.contains("line 21"));
        assert!(!message.contains("line 20\n"), "只带尾部：{message}");
    }

    #[test]
    fn runs_non_executable_script_through_shebang_in_its_directory() {
        // 用户从网上下载的脚本常常没有 x 权限；不 chmod 用户文件也要能跑。默认 cwd 是脚本目录。
        let dir = TempDir::new("shebang");
        let path = write_script(&dir, "pwd.sh", "#!/bin/sh\npwd\necho \"$1|$2\"\n", false);
        let cwd = working_dir(None, &path).unwrap();
        let out = execute(&path, &["a b".into(), "c".into()], &cwd, TIMEOUT).unwrap();
        assert!(out.status.success(), "{out:?}");
        let canonical = fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            out.stdout,
            format!("{}\na b|c\n", canonical.display()),
            "参数必须原样按位置传入"
        );
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o111, 0);
    }

    #[test]
    fn file_without_shebang_or_exec_bit_fails_loudly() {
        let dir = TempDir::new("noshebang");
        let path = write_script(&dir, "x.txt", "echo hi\n", false);
        let err = command_for(&path).unwrap_err();
        assert!(format!("{err}").contains("shebang"));
    }

    #[test]
    fn current_directory_path_supports_tilde_and_must_exist() {
        let dir = TempDir::new("cwd");
        let script = dir.path().join("s.sh");
        assert_eq!(
            working_dir(Some("~"), &script).unwrap(),
            crate::paths::home_dir().unwrap()
        );
        assert!(working_dir(Some("does-not-exist"), &script).is_err());
    }

    #[test]
    fn timeout_kills_the_whole_process_group() {
        // 卡死的脚本不能拖住启动器的执行线程；它派生的后台子进程也必须一起死，不留孤儿。
        let dir = TempDir::new("timeout");
        let pid_file = dir.path().join("child.pid");
        let path = write_script(
            &dir,
            "hang.sh",
            &format!(
                "#!/bin/sh\nsleep 300 &\necho $! > '{}'\nwait\n",
                pid_file.display()
            ),
            true,
        );
        let started = Instant::now();
        let err = execute(&path, &[], dir.path(), Duration::from_millis(500)).unwrap_err();
        assert!(format!("{err}").contains("超过"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));

        let child_pid: i32 = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // SIGKILL 投递是异步的，给内核一点时间回收。
        let gone = (0..100).any(|_| {
            // SAFETY: 信号 0 只检查进程是否存在。
            let alive = unsafe { libc::kill(child_pid, 0) } == 0;
            if alive {
                std::thread::sleep(Duration::from_millis(20));
            }
            !alive
        });
        assert!(gone, "后台子进程 {child_pid} 仍然存活");
    }
}
