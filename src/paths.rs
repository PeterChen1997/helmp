//! 本地数据目录：`~/Library/Application Support/huan/`，可用 `HUAN_DATA_DIR` 覆盖（测试用）。

use std::path::PathBuf;

use anyhow::{Context as _, Result};

pub fn data_dir() -> Result<PathBuf> {
    let dir = match std::env::var_os("HUAN_DATA_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => home_dir()?.join("Library/Application Support/huan"),
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("创建数据目录 {}", dir.display()))?;
    Ok(dir)
}

pub fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME 未设置")
}
