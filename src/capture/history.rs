//! 截图历史：最近完成的 [`KEEP`] 张截图（复制、保存、钉图、识别文字之后的最终图，含标注和马赛克）。
//!
//! 存在 `<data_dir>/capture-history/`：每张一对文件，`<id>.png` 是图、`<id>.json` 是元数据
//! （时间、像素尺寸、怎么结束的、在屏幕上的位置）。`id` 是记录时的毫秒时间戳，越大越新。
//! 每记一张就修剪到最新的 [`KEEP`] 张，多出来的连图带元数据一起删掉。
//!
//! 设置 › 截图里可以关掉（`config.screenshot.history`，默认开）；关掉时不再记录，并清空已有的历史。
//! 启动器里的「截图历史」（`providers::screenshot`）从这里读，可以再钉到屏幕上、复制、另存为、识别文字。
//!
//! 这里只做文件读写，不碰界面：截图界面在后台线程调用 [`record`]，启动器在查询线程调用 [`list`]。

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use super::geometry::Rect;
use crate::i18n::{tr, trf};

/// 保留的张数。
pub const KEEP: usize = 10;

/// 一张截图最后是怎么用掉的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ended {
    Copied,
    Saved,
    Pinned,
    Recognized,
}

impl Ended {
    pub fn label(self) -> &'static str {
        match self {
            Ended::Copied => tr("已复制", "Copied"),
            Ended::Saved => tr("已保存", "Saved"),
            Ended::Pinned => tr("已钉图", "Pinned"),
            Ended::Recognized => tr("已识别文字", "Text recognized"),
        }
    }
}

/// 一条历史的元数据（`<id>.json`）。位置用于「再钉到屏幕上」时回到原处。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// 记录时的毫秒时间戳，也是文件名。
    pub id: u64,
    /// 像素尺寸。
    pub width: u32,
    pub height: u32,
    pub ended: Ended,
    /// 选区（本屏坐标，点）。
    pub rect: [f64; 4],
    /// 所在屏的 frame（AppKit 全局坐标）。
    pub screen: [f64; 4],
    /// 所在屏的可见区域（本屏坐标）。
    pub visible: [f64; 4],
}

impl Entry {
    pub fn png(&self, dir: &Path) -> PathBuf {
        dir.join(format!("{}.png", self.id))
    }

    pub fn rect(&self) -> Rect {
        to_rect(self.rect)
    }

    pub fn screen(&self) -> Rect {
        to_rect(self.screen)
    }

    pub fn visible(&self) -> Rect {
        to_rect(self.visible)
    }
}

fn to_rect([x, y, w, h]: [f64; 4]) -> Rect {
    Rect::new(x, y, w, h)
}

fn from_rect(rect: Rect) -> [f64; 4] {
    [rect.x, rect.y, rect.w, rect.h]
}

/// 历史目录。
pub fn dir() -> Result<PathBuf> {
    Ok(crate::paths::data_dir()?.join("capture-history"))
}

/// 设置里是否开着截图历史。
pub fn enabled() -> bool {
    crate::config::get().screenshot.history
}

/// 要记下的一张截图（从 `capture::Shot` 取）。
pub struct Record<'a> {
    pub png: &'a [u8],
    pub pixels: (u32, u32),
    pub rect: Rect,
    pub screen: Rect,
    pub visible: Rect,
    pub ended: Ended,
}

/// 记下一张截图并修剪到最新的 [`KEEP`] 张。`now_ms` 是此刻的毫秒时间戳（同一毫秒记两张时往后顺延）。
/// 先写图、后写元数据：元数据在就说明图完整，[`list`] 只认有元数据的。
pub fn record(dir: &Path, shot: &Record, now_ms: u64) -> Result<Entry> {
    std::fs::create_dir_all(dir)
        .with_context(|| trf!("创建 {}", "Couldn't create {}", dir.display()))?;
    let mut id = now_ms;
    while dir.join(format!("{id}.png")).exists() || dir.join(format!("{id}.json")).exists() {
        id += 1;
    }
    let entry = Entry {
        id,
        width: shot.pixels.0,
        height: shot.pixels.1,
        ended: shot.ended,
        rect: from_rect(shot.rect),
        screen: from_rect(shot.screen),
        visible: from_rect(shot.visible),
    };
    let png = entry.png(dir);
    std::fs::write(&png, shot.png)
        .with_context(|| trf!("写入 {}", "Couldn't write {}", png.display()))?;
    let json = dir.join(format!("{id}.json"));
    std::fs::write(&json, serde_json::to_vec_pretty(&entry)?)
        .with_context(|| trf!("写入 {}", "Couldn't write {}", json.display()))?;
    prune(dir, KEEP)?;
    Ok(entry)
}

/// 历史里的各条，最新的在前。读不了的元数据、缺图的条目跳过（下次修剪时删掉）。
pub fn list(dir: &Path) -> Vec<Entry> {
    let mut entries: Vec<Entry> = ids(dir)
        .into_iter()
        .filter_map(|id| load_entry(dir, id).ok())
        .filter(|entry| entry.png(dir).is_file())
        .collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.id));
    entries
}

/// 读一条历史的元数据。
pub fn load_entry(dir: &Path, id: u64) -> Result<Entry> {
    let path = dir.join(format!("{id}.json"));
    let bytes = std::fs::read(&path).with_context(|| {
        trf!(
            "这张截图已经不在历史里了（{}）",
            "This screenshot is no longer in the history ({})",
            path.display()
        )
    })?;
    let entry: Entry = serde_json::from_slice(&bytes).with_context(|| {
        trf!(
            "截图历史记录损坏：{}",
            "Damaged screenshot history record: {}",
            path.display()
        )
    })?;
    anyhow::ensure!(
        entry.id == id,
        "{}",
        trf!(
            "截图历史记录损坏：{}",
            "Damaged screenshot history record: {}",
            path.display()
        )
    );
    Ok(entry)
}

/// 删除一条（图和元数据）。
pub fn delete(dir: &Path, id: u64) -> Result<()> {
    for ext in ["png", "json"] {
        let path = dir.join(format!("{id}.{ext}"));
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| trf!("删除 {}", "Couldn't delete {}", path.display()));
            }
        }
    }
    Ok(())
}

/// 清空整个历史（设置里关掉截图历史时）。
pub fn clear(dir: &Path) -> Result<()> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| trf!("删除 {}", "Couldn't delete {}", dir.display())),
    }
}

/// 只留最新的 `keep` 条完整的记录：更早的、缺图或缺元数据的都删掉（图和元数据一起）。
/// 只删文件名是数字 id 的 `.png` / `.json`，目录里别的文件不动。
pub fn prune(dir: &Path, keep: usize) -> Result<()> {
    let mut complete: Vec<u64> = ids(dir)
        .into_iter()
        .filter(|id| dir.join(format!("{id}.png")).is_file() && load_entry(dir, *id).is_ok())
        .collect();
    complete.sort_unstable_by(|a, b| b.cmp(a));
    complete.truncate(keep);
    let Ok(read) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for file in read.flatten() {
        let path = file.path();
        let Some(id) = numeric_stem(&path) else {
            continue;
        };
        if !complete.contains(&id) {
            std::fs::remove_file(&path)
                .with_context(|| trf!("删除 {}", "Couldn't delete {}", path.display()))?;
        }
    }
    Ok(())
}

/// 目录里出现过的 id（`.png` 或 `.json`），去重。
fn ids(dir: &Path) -> Vec<u64> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut ids: Vec<u64> = read
        .flatten()
        .filter_map(|file| numeric_stem(&file.path()))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// `<数字>.png` / `<数字>.json` → 数字。
fn numeric_stem(path: &Path) -> Option<u64> {
    let ext = path.extension()?.to_str()?;
    if ext != "png" && ext != "json" {
        return None;
    }
    path.file_stem()?.to_str()?.parse().ok()
}

/// 重新打开（再钉到屏幕上）时摆在哪：返回 (屏的 frame, 可见区域（本屏坐标）, 选区（本屏坐标）)。
/// `screens` 是此刻各块屏的 (frame, visibleFrame)（AppKit 全局坐标），`mouse` 是鼠标位置（同上）。
///
/// 原来那块屏还在（frame 一样）就回到原处；屏换了（外接屏拔了、分辨率改了）就放到鼠标所在屏
/// （都不含鼠标时第一块屏）可见区域的正中，太大就等比缩小到可见区域的 90% 以内。没有屏幕信息时照原样。
pub fn placement(entry: &Entry, screens: &[(Rect, Rect)], mouse: (f64, f64)) -> (Rect, Rect, Rect) {
    let original = (entry.screen(), entry.visible(), entry.rect());
    if screens.iter().any(|(frame, _)| *frame == original.0) {
        return original;
    }
    let contains = |frame: &Rect| {
        mouse.0 >= frame.x
            && mouse.0 <= frame.x + frame.w
            && mouse.1 >= frame.y
            && mouse.1 <= frame.y + frame.h
    };
    let Some((frame, visible)) = screens
        .iter()
        .find(|(frame, _)| contains(frame))
        .or_else(|| screens.first())
    else {
        return original;
    };
    let visible = super::geometry::from_appkit(*frame, *visible);
    let (w, h) = (original.2.w.max(1.0), original.2.h.max(1.0));
    let fit = (0.9 * visible.w / w).min(0.9 * visible.h / h).min(1.0);
    let (w, h) = ((w * fit).round(), (h * fit).round());
    let rect = Rect::new(
        (visible.x + (visible.w - w) / 2.0).round(),
        (visible.y + (visible.h - h) / 2.0).round(),
        w,
        h,
    );
    (*frame, visible, rect)
}

/// 此刻的毫秒时间戳。
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "helmp-capture-history-{tag}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn shot(png: &[u8], ended: Ended) -> Record<'_> {
        Record {
            png,
            pixels: (200, 100),
            rect: Rect::new(10.0, 20.0, 100.0, 50.0),
            screen: Rect::new(0.0, 0.0, 1512.0, 982.0),
            visible: Rect::new(0.0, 37.0, 1512.0, 880.0),
            ended,
        }
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|file| file.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn keeps_exactly_the_newest_ten_and_deletes_older_files() {
        // 隐私承诺是「只留最近 10 张」：第 11 张进来时最旧那张的图和元数据都得真的从磁盘上消失，
        // 不能只是不显示。
        let dir = TempDir::new("prune");
        for n in 0..12u64 {
            record(
                &dir.0,
                &shot(format!("png{n}").as_bytes(), Ended::Copied),
                1_000 + n,
            )
            .unwrap();
        }
        let entries = list(&dir.0);
        assert_eq!(entries.len(), KEEP);
        let ids: Vec<u64> = entries.iter().map(|entry| entry.id).collect();
        assert_eq!(ids, (1_002..1_012).rev().collect::<Vec<_>>(), "最新的在前");
        let names = files(&dir.0);
        assert_eq!(names.len(), KEEP * 2, "{names:?}");
        for gone in ["1000.png", "1000.json", "1001.png", "1001.json"] {
            assert!(
                !names.contains(&gone.to_string()),
                "{gone} 应该被删掉：{names:?}"
            );
        }
        assert_eq!(std::fs::read(dir.0.join("1011.png")).unwrap(), b"png11");
    }

    #[test]
    fn same_millisecond_records_do_not_overwrite_each_other() {
        // 连按两次复制可能落在同一毫秒：第二张不能覆盖第一张。
        let dir = TempDir::new("same-ms");
        let a = record(&dir.0, &shot(b"a", Ended::Copied), 5).unwrap();
        let b = record(&dir.0, &shot(b"b", Ended::Pinned), 5).unwrap();
        assert_eq!((a.id, b.id), (5, 6));
        assert_eq!(list(&dir.0).len(), 2);
    }

    #[test]
    fn metadata_round_trips_with_position_and_how_it_ended() {
        // 「再钉到屏幕上」要回到原来的位置，「已识别文字」等标签要说出它当时是怎么用掉的。
        let dir = TempDir::new("roundtrip");
        let entry = record(&dir.0, &shot(b"img", Ended::Recognized), 42).unwrap();
        let loaded = load_entry(&dir.0, 42).unwrap();
        assert_eq!(loaded, entry);
        assert_eq!(loaded.rect(), Rect::new(10.0, 20.0, 100.0, 50.0));
        assert_eq!(loaded.ended, Ended::Recognized);
        let json = std::fs::read_to_string(dir.0.join("42.json")).unwrap();
        assert!(json.contains("\"recognized\""), "{json}");
    }

    #[test]
    fn broken_or_half_written_entries_are_skipped_then_pruned() {
        // 写到一半退出（只有图没有元数据）或元数据坏了：列表里不出现，修剪时连同残留的文件删掉；
        // 目录里不是历史文件的东西不动。
        let dir = TempDir::new("broken");
        record(&dir.0, &shot(b"ok", Ended::Saved), 10).unwrap();
        std::fs::write(dir.0.join("20.png"), b"orphan").unwrap();
        std::fs::write(dir.0.join("30.json"), b"{not json").unwrap();
        std::fs::write(dir.0.join("30.png"), b"x").unwrap();
        std::fs::write(dir.0.join("notes.txt"), b"keep me").unwrap();
        assert_eq!(list(&dir.0).iter().map(|e| e.id).collect::<Vec<_>>(), [10]);
        prune(&dir.0, KEEP).unwrap();
        assert_eq!(files(&dir.0), ["10.json", "10.png", "notes.txt"]);
    }

    #[test]
    fn reopened_pin_goes_back_to_its_spot_or_the_center_of_the_current_screen() {
        // 「再钉到屏幕上」：屏幕没变就回到原来的位置（和钉图时一样原地浮起）；那块屏不在了，
        // 不能钉到看不见的地方，改放到鼠标所在屏的正中，太大就缩小到放得下。
        let dir = TempDir::new("placement");
        let entry = record(&dir.0, &shot(b"a", Ended::Pinned), 1).unwrap();
        let main = (
            Rect::new(0.0, 0.0, 1512.0, 982.0),
            Rect::new(0.0, 65.0, 1512.0, 880.0),
        );
        let (screen, _, rect) = placement(&entry, &[main], (10.0, 10.0));
        assert_eq!((screen, rect), (entry.screen(), entry.rect()));

        let external = (
            Rect::new(1512.0, 0.0, 400.0, 300.0),
            Rect::new(1512.0, 0.0, 400.0, 280.0),
        );
        let other = (
            Rect::new(-800.0, 0.0, 800.0, 600.0),
            Rect::new(-800.0, 0.0, 800.0, 600.0),
        );
        let (screen, visible, rect) = placement(&entry, &[other, external], (1600.0, 100.0));
        assert_eq!(screen, external.0, "鼠标在哪块屏就放哪块");
        // 可见区域本屏坐标：顶上 20 点是菜单栏。
        assert_eq!(visible, Rect::new(0.0, 20.0, 400.0, 280.0));
        assert_eq!(rect, Rect::new(150.0, 135.0, 100.0, 50.0), "正中，原大小");

        let mut big = entry.clone();
        big.rect = [0.0, 0.0, 1000.0, 500.0];
        let (_, _, rect) = placement(&big, &[external], (0.0, 0.0));
        assert!(rect.w <= 360.0 && rect.h <= 252.0, "{rect:?}");
        assert!((rect.w / rect.h - 2.0).abs() < 0.02, "等比缩小：{rect:?}");

        // 一块屏都不知道（取不到屏幕信息）：照原样。
        assert_eq!(placement(&entry, &[], (0.0, 0.0)).2, entry.rect());
    }

    #[test]
    fn delete_and_clear_remove_files() {
        let dir = TempDir::new("delete");
        record(&dir.0, &shot(b"a", Ended::Copied), 1).unwrap();
        record(&dir.0, &shot(b"b", Ended::Copied), 2).unwrap();
        delete(&dir.0, 1).unwrap();
        assert_eq!(files(&dir.0), ["2.json", "2.png"]);
        // 删已经不在的：不算错（列表可能是旧的）。
        delete(&dir.0, 1).unwrap();
        assert!(load_entry(&dir.0, 1).is_err());
        clear(&dir.0).unwrap();
        assert!(!dir.0.exists());
        clear(&dir.0).unwrap();
        assert!(list(&dir.0).is_empty());
    }
}
