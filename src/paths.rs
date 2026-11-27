//! 本地数据目录：`~/Library/Application Support/Helmp/`，可用 `HELMP_DATA_DIR` 覆盖（测试用）。
//! 旧版本叫「唤」，数据在 `.../huan/`；首次启动时整体改名迁移过来，剪贴板历史不丢。

use std::path::PathBuf;

use anyhow::{Context as _, Result};

pub fn data_dir() -> Result<PathBuf> {
    let dir = match std::env::var_os("HELMP_DATA_DIR") {
        Some(dir) => PathBuf::from(dir),
        // 单测不读写真实数据目录：没指定时落到本进程专用的临时目录。
        None if cfg!(test) => std::env::temp_dir().join(format!("helmp-test-data-{}", std::process::id())),
        None => {
            let support = home_dir()?.join("Library/Application Support");
            let dir = support.join("Helmp");
            let legacy = support.join("huan");
            if !dir.exists() && legacy.is_dir() {
                // 多个 provider 会并发调用到这里：别的线程抢先迁移完成时 rename 会失败，
                // 只要新目录已经在了就算成功。
                if let Err(err) = std::fs::rename(&legacy, &dir)
                    && !dir.is_dir()
                {
                    return Err(err).with_context(|| {
                        format!("迁移旧数据目录 {} → {}", legacy.display(), dir.display())
                    });
                }
            }
            dir
        }
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("创建数据目录 {}", dir.display()))?;
    Ok(dir)
}

pub fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME 未设置")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_tests_stay_out_of_the_real_data_dir() {
        // `cargo test --release` 在开发者自己的机器上跑：单测碰到真实的
        // `~/Library/Application Support/Helmp`，就可能改掉正在用的配置、定时任务记录和统计状态。
        let dir = data_dir().unwrap();
        if std::env::var_os("HELMP_DATA_DIR").is_none() {
            assert!(dir.starts_with(std::env::temp_dir()), "{}", dir.display());
        }
        assert!(!dir.ends_with("Library/Application Support/Helmp"), "{}", dir.display());
    }
}
