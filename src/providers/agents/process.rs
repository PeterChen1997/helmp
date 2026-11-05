//! 进程信息：直接走 `proc_pidinfo` / `proc_pidpath`，每秒轮询判活时不 spawn `ps`。

use std::path::PathBuf;

/// 进程启动时间（Unix 秒）。完整 BSD 信息只对同用户进程可读，会话进程都是用户自己的。
pub fn start_secs(pid: i32) -> Option<u64> {
    // SAFETY: proc_bsdinfo 是纯 POD，全零是合法初值；缓冲区大小如实传入。
    let mut raw: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    pidinfo(pid, libc::PROC_PIDTBSDINFO, &mut raw).then_some(raw.pbi_start_tvsec)
}

/// 父进程。用 short 版本：`/usr/bin/login` 这类 root 进程的完整信息读不到，short 版本可以。
pub fn ppid(pid: i32) -> Option<i32> {
    // SAFETY: 同上。
    let mut raw: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
    pidinfo(pid, libc::PROC_PIDT_SHORTBSDINFO, &mut raw).then_some(raw.pbsi_ppid as i32)
}

fn pidinfo<T>(pid: i32, flavor: libc::c_int, out: &mut T) -> bool {
    if pid <= 0 {
        return false;
    }
    let size = std::mem::size_of::<T>() as libc::c_int;
    // SAFETY: out 指向一块大小为 size 的可写内存。
    let written = unsafe { libc::proc_pidinfo(pid, flavor, 0, (out as *mut T).cast(), size) };
    written == size
}

/// 可执行文件路径。
pub fn path(pid: i32) -> Option<PathBuf> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: 缓冲区按 PROC_PIDPATHINFO_MAXSIZE 分配，返回值是写入的字节数。
    let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if len <= 0 {
        return None;
    }
    buf.truncate(len as usize);
    String::from_utf8(buf).ok().map(PathBuf::from)
}

/// pid 还活着，且（给了 `started_at_ms` 时）启动时间对得上——防止 pid 被复用后把孤儿注册文件当成活会话。
pub fn alive(pid: i32, started_at_ms: Option<u64>) -> bool {
    let Some(start) = start_secs(pid) else {
        return false;
    };
    match started_at_ms {
        // 注册表的 startedAt 比进程启动晚几秒（实测 1–2 s），放宽到 2 分钟。
        Some(ms) => (ms / 1000).abs_diff(start) <= 120,
        None => true,
    }
}

/// 可执行文件所在的最外层 `.app`：`/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/…` → `/Applications/ChatGPT.app`。
pub fn outer_app_bundle(exe: &std::path::Path) -> Option<PathBuf> {
    let text = exe.to_str()?;
    let end = text.find(".app/")? + ".app".len();
    Some(PathBuf::from(&text[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive_and_start_time_guards_pid_reuse() {
        // 注册表里的 pid 可能已被别的进程复用；启动时间对不上就必须当成已死，否则列表里会出现幽灵会话。
        let pid = std::process::id() as i32;
        let start = start_secs(pid).expect("自己的进程信息");
        assert!(alive(pid, Some(start * 1000 + 1500)));
        assert!(!alive(pid, Some((start - 3600) * 1000)));
        assert!(!alive(-1, None));
        assert_eq!(ppid(pid), Some(std::os::unix::process::parent_id() as i32));
    }

    #[test]
    fn outer_bundle_is_the_gui_app() {
        let exe = std::path::Path::new(
            "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
        );
        assert_eq!(
            outer_app_bundle(exe),
            Some(PathBuf::from("/Applications/ChatGPT.app"))
        );
        assert_eq!(outer_app_bundle(std::path::Path::new("/bin/zsh")), None);
    }
}
