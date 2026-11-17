//! 便签持久化及与窗口无关的吸边、耳标排队计算。
//! 运行时先 load 再 save，由调用方串行管理便签快照；写保护状态不替代快照的同步。

use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

pub const FILE: &str = "notes.json";
pub const TAB_W: f64 = 24.0;
pub const TAB_H: f64 = 72.0;
pub const TAB_GAP: f64 = 8.0;
pub const TAB_MIN_H: f64 = 28.0;

static WRITE_BLOCKED: AtomicBool = AtomicBool::new(true);

/// 全局屏幕坐标，单位 point，左上角原点，与 GPUI 窗口坐标一致。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Frame {
    fn valid(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.w.is_finite()
            && self.h.is_finite()
            && self.w > 0.0
            && self.h > 0.0
            && (self.x + self.w).is_finite()
            && (self.y + self.h).is_finite()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Placement {
    Floating {
        frame: Frame,
    },
    Docked {
        edge: Edge,
        display: String,
        order: u32,
    },
    /// 从桌面关闭，仍可在便签 Scope 搜索。
    Archived,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub text: String,
    /// Unix 秒。
    pub created_at: i64,
    pub updated_at: i64,
    pub placement: Placement,
    /// 展开时卡片宽高，None 使用界面默认值。
    #[serde(default)]
    pub size: Option<(f64, f64)>,
}

#[derive(Serialize, Deserialize)]
struct Document {
    // 不设 default：缺少 notes 字段的文件也是坏文件，不能被当成空列表。
    notes: Vec<Note>,
}

pub fn path() -> Result<PathBuf> {
    Ok(crate::paths::data_dir()?.join(FILE))
}

/// 文件不存在才返回空；损坏或读取失败必须报错，避免后续覆盖用户便签。
#[cfg(test)]
pub fn load_from(path: &Path) -> Result<Vec<Note>> {
    load_from_inner(path, false)
}

fn load_from_inner(path: &Path, recover_corruption: bool) -> Result<Vec<Note>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).context("Couldn’t read notes file"),
    };
    let document = serde_json::from_slice::<Document>(&bytes).map_err(|err| {
        // serde 的错误可能引用错误字段中的正文；Sentry 只接收类别和行列号。
        anyhow::anyhow!(
            "Invalid notes file ({:?}, line {}, column {})",
            err.classify(),
            err.line(),
            err.column()
        )
    });
    match document {
        Ok(document) => Ok(document.notes),
        Err(err) if recover_corruption => {
            eprintln!("helmp: 便签文件损坏，将备份后重建：{err:#}");
            crate::telemetry::report("notes.load", &err);
            // 用户新记的便签不能因为旧文件坏了而静默丢失；只有备份成功才能覆盖原文件。
            // 固定文件名，避免每次启动都积累一份坏文件。
            std::fs::copy(path, path.with_extension("json.unreadable"))
                .context("Couldn’t back up unreadable notes file")?;
            Ok(Vec::new())
        }
        Err(err) => Err(err),
    }
}

/// 同目录临时文件 + rename，读者只会看见完整的新文件或旧文件。
pub fn save_to(path: &Path, notes: &[Note]) -> Result<()> {
    // serde_json 会把 NaN/Infinity 静默写成 null，下一次读取 f64 就会失败。
    anyhow::ensure!(
        notes.iter().all(|note| {
            let frame_finite = match note.placement {
                Placement::Floating { frame } => [frame.x, frame.y, frame.w, frame.h]
                    .into_iter()
                    .all(f64::is_finite),
                _ => true,
            };
            frame_finite
                && note
                    .size
                    .is_none_or(|(w, h)| w.is_finite() && h.is_finite())
        }),
        "Notes contain non-finite geometry"
    );
    let bytes = serde_json::to_vec_pretty(&Document {
        notes: notes.to_vec(),
    })
    .context("Couldn’t serialize notes")?;
    let tmp = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .context("Couldn’t create temporary notes file")?;
    let result = (|| {
        file.write_all(&bytes)
            .context("Couldn’t write notes file")?;
        file.sync_all().context("Couldn’t sync notes file")?;
        std::fs::rename(&tmp, path).context("Couldn’t replace notes file")
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

pub fn load() -> Vec<Note> {
    match path().and_then(|path| load_recovering_from(&path, &WRITE_BLOCKED)) {
        Ok(notes) => notes,
        Err(err) => {
            WRITE_BLOCKED.store(true, Ordering::Relaxed);
            eprintln!("helmp: 读取便签失败：{err:#}");
            crate::telemetry::report("notes.load", &err);
            Vec::new()
        }
    }
}

/// 返回是否写盘成功，调用方据此决定要不要保留未保存标记。
pub fn save(notes: &[Note]) -> bool {
    if let Err(err) = path().and_then(|path| save_checked_to(&path, notes, &WRITE_BLOCKED)) {
        eprintln!("helmp: 保存便签失败：{err:#}");
        crate::telemetry::report("notes.save", &err);
        return false;
    }
    true
}

pub(super) fn load_recovering_from(path: &Path, write_blocked: &AtomicBool) -> Result<Vec<Note>> {
    write_blocked.store(true, Ordering::Relaxed);
    let result = load_from_inner(path, true);
    write_blocked.store(result.is_err(), Ordering::Relaxed);
    result
}

pub(super) fn save_checked_to(
    path: &Path,
    notes: &[Note],
    write_blocked: &AtomicBool,
) -> Result<()> {
    // 读取 IO 错误或坏文件备份失败时持续拒写；不在每次保存时重读、反序列化整个文件。
    anyhow::ensure!(
        !write_blocked.load(Ordering::Relaxed),
        "Notes writes blocked until a successful load or corruption backup"
    );
    save_to(path, notes)
}

pub fn new_note(text: &str, now: i64, placement: Placement) -> Note {
    Note {
        id: uuid::Uuid::new_v4().to_string(),
        text: text.to_string(),
        created_at: now,
        updated_at: now,
        placement,
        size: None,
    }
}

/// 比较对应左右边缘的绝对距离；同时满足阈值时取更近侧，相同距离取左侧。
pub fn snap_edge(frame: Frame, visible: Frame, threshold: f64) -> Option<Edge> {
    if !frame.valid() || !visible.valid() || !threshold.is_finite() || threshold < 0.0 {
        return None;
    }
    let left = (frame.x - visible.x).abs();
    let right = (frame.x + frame.w - (visible.x + visible.w)).abs();
    if left <= threshold && (right > threshold || left <= right) {
        Some(Edge::Left)
    } else if right <= threshold {
        Some(Edge::Right)
    } else {
        None
    }
}

/// ids 由调用方按 order 升序传入；整列垂直居中，顶部到底部保持该顺序。
/// 先等比压缩耳标高度，再在最小高度下截断队尾；无足够空间时返回空。
pub fn dock_layout(ids: &[&str], edge: Edge, visible: Frame) -> Vec<(String, Frame)> {
    if ids.is_empty() || !visible.valid() || visible.w < TAB_W || visible.h < TAB_MIN_H {
        return Vec::new();
    }
    let capacity = ((visible.h + TAB_GAP) / (TAB_MIN_H + TAB_GAP)).floor() as usize;
    let mut count = ids.len().min(capacity);
    // h + TAB_GAP 可能向上舍入，容量临界点下方也必须装得下最小高度。
    if count as f64 * TAB_MIN_H + (count - 1) as f64 * TAB_GAP > visible.h {
        count -= 1;
    }
    // 按实际显示数量计算，向下取整到整数 pt，避免压缩后的浮点累加使首尾越界。
    let height = ((visible.h - TAB_GAP * (count - 1) as f64) / count as f64)
        .floor()
        .clamp(TAB_MIN_H, TAB_H);
    let total = count as f64 * height + (count - 1) as f64 * TAB_GAP;
    let start_y = visible.y + (visible.h - total) / 2.0;
    let x = match edge {
        Edge::Left => visible.x,
        Edge::Right => visible.x + visible.w - TAB_W,
    };
    ids.iter()
        .take(count)
        .enumerate()
        .map(|(index, id)| {
            (
                (*id).to_string(),
                Frame {
                    x,
                    y: start_y + index as f64 * (height + TAB_GAP),
                    w: TAB_W,
                    h: height,
                },
            )
        })
        .collect()
}

/// 只计算同显示器同侧；极限值饱和，避免损坏数据导致溢出 panic。
pub fn next_order(notes: &[Note], edge: Edge, display: &str) -> u32 {
    notes
        .iter()
        .filter_map(|note| match &note.placement {
            Placement::Docked {
                edge: side,
                display: screen,
                order,
            } if *side == edge && screen == display => Some(*order),
            _ => None,
        })
        .max()
        .map_or(0, |order| order.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("helmp-notes-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join(FILE)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn visible() -> Frame {
        // 非零、负坐标覆盖主显示器左侧或上方的外接屏，而非假设所有屏幕从 (0, 0) 开始。
        Frame {
            x: -1200.0,
            y: -100.0,
            w: 1200.0,
            h: 900.0,
        }
    }

    fn docked(edge: Edge, display: &str, order: u32) -> Note {
        new_note(
            "便签",
            123,
            Placement::Docked {
                edge,
                display: display.into(),
                order,
            },
        )
    }

    fn assert_inside_and_separated(layout: &[(String, Frame)], visible: Frame) {
        for (_, frame) in layout {
            assert!(frame.x >= visible.x && frame.x + frame.w <= visible.x + visible.w);
            assert!(frame.y >= visible.y && frame.y + frame.h <= visible.y + visible.h);
            assert!((TAB_MIN_H..=TAB_H).contains(&frame.h));
        }
        for pair in layout.windows(2) {
            assert!(pair[1].1.y >= pair[0].1.y + pair[0].1.h + TAB_GAP - 1e-9);
        }
    }

    #[test]
    fn round_trip_preserves_all_placements_and_replaces_existing_file() {
        // 重启必须恢复自由浮窗、吸边和归档状态，以及中文正文、卡片尺寸和时间戳。
        let dir = TempDir::new();
        let mut notes = vec![
            new_note(
                "第一行\n中文 📝",
                123,
                Placement::Floating { frame: visible() },
            ),
            docked(Edge::Right, "display-uuid", 7),
            new_note("归档", 456, Placement::Archived),
        ];
        notes[0].size = Some((320.0, 240.0));
        notes[0].updated_at = 789;
        save_to(&dir.file(), &notes).unwrap();
        assert_eq!(load_from(&dir.file()).unwrap(), notes);
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.file()).unwrap()).unwrap();
        assert_eq!(json["notes"][0]["placement"]["kind"], "floating");
        assert_eq!(json["notes"][1]["placement"]["kind"], "docked");
        assert_eq!(json["notes"][1]["placement"]["edge"], "right");
        assert_eq!(json["notes"][2]["placement"]["kind"], "archived");
        save_to(&dir.file(), &notes[1..]).unwrap();
        assert_eq!(load_from(&dir.file()).unwrap(), notes[1..]);
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn missing_file_is_empty_but_corruption_and_io_errors_are_not() {
        // 只有首次运行可以是空；坏 JSON、缺少根字段和 IO 错误不能伪装成没有便签。
        let dir = TempDir::new();
        assert!(load_from(&dir.file()).unwrap().is_empty());
        for broken in ["", "{broken", "{}", r#"{"notes":null}"#] {
            std::fs::write(dir.file(), broken).unwrap();
            assert!(load_from(&dir.file()).is_err());
            assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), broken);
        }
        assert!(load_from(&dir.0).is_err());
    }

    #[test]
    fn missing_size_uses_default_without_losing_the_note() {
        // size 是可选的演进字段，早期数据没有它时仍应能恢复便签。
        let dir = TempDir::new();
        let note = new_note("旧便签", 123, Placement::Archived);
        let mut json = serde_json::to_value(&note).unwrap();
        json.as_object_mut().unwrap().remove("size");
        std::fs::write(dir.file(), serde_json::json!({"notes": [json]}).to_string()).unwrap();
        assert_eq!(load_from(&dir.file()).unwrap(), vec![note]);
    }

    #[test]
    fn parse_error_does_not_expose_note_content() {
        // 错误会送到 Sentry；serde 对未知枚举的默认错误会泄露错误值本身。
        let dir = TempDir::new();
        let note = new_note("私密正文", 123, Placement::Archived);
        let mut json = serde_json::to_value(note).unwrap();
        json["placement"]["kind"] = "私密正文".into();
        std::fs::write(dir.file(), serde_json::json!({"notes": [json]}).to_string()).unwrap();
        let error = load_from(&dir.file()).unwrap_err();
        assert!(!format!("{error:#}").contains("私密正文"));
    }

    #[test]
    fn backup_failure_blocks_writes_until_a_successful_load() {
        // 固定备份路径被目录占用，稳定模拟 copy 失败（不依赖运行用户的文件权限）。
        let dir = TempDir::new();
        let backup = dir.file().with_extension("json.unreadable");
        std::fs::create_dir(&backup).unwrap();
        std::fs::write(dir.file(), "{broken").unwrap();
        let write_blocked = AtomicBool::new(false);
        assert!(load_recovering_from(&dir.file(), &write_blocked).is_err());
        assert!(write_blocked.load(Ordering::Relaxed));
        let notes = vec![new_note("便签", 123, Placement::Archived)];
        assert!(save_checked_to(&dir.file(), &notes, &write_blocked).is_err());
        assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), "{broken");

        // 即使外部修复了文件，也要重新 load 才能解除保护，避免把旧的空快照写进去。
        save_to(&dir.file(), &notes).unwrap();
        assert!(save_checked_to(&dir.file(), &[], &write_blocked).is_err());
        assert_eq!(load_from(&dir.file()).unwrap(), notes);
        assert_eq!(
            load_recovering_from(&dir.file(), &write_blocked).unwrap(),
            notes
        );
        assert!(!write_blocked.load(Ordering::Relaxed));
        save_checked_to(&dir.file(), &notes, &write_blocked).unwrap();
    }

    #[test]
    fn read_io_error_blocks_writes_without_creating_a_backup() {
        let dir = TempDir::new();
        std::fs::create_dir(dir.file()).unwrap();
        let original = dir.file().join("original");
        std::fs::write(&original, "keep").unwrap();
        let write_blocked = AtomicBool::new(false);
        assert!(load_recovering_from(&dir.file(), &write_blocked).is_err());
        assert!(write_blocked.load(Ordering::Relaxed));
        assert!(save_checked_to(&dir.file(), &[], &write_blocked).is_err());
        assert!(!dir.file().with_extension("json.unreadable").exists());
        assert_eq!(std::fs::read_to_string(original).unwrap(), "keep");
    }

    #[test]
    fn missing_file_load_allows_first_save() {
        let dir = TempDir::new();
        let write_blocked = AtomicBool::new(true);
        assert!(save_checked_to(&dir.file(), &[], &write_blocked).is_err());
        assert!(
            load_recovering_from(&dir.file(), &write_blocked)
                .unwrap()
                .is_empty()
        );
        assert!(!write_blocked.load(Ordering::Relaxed));
        let notes = vec![new_note("首次便签", 123, Placement::Archived)];
        save_checked_to(&dir.file(), &notes, &write_blocked).unwrap();
        assert_eq!(load_from(&dir.file()).unwrap(), notes);
    }

    #[test]
    fn corruption_recovery_preserves_backup_and_saves_new_notes() {
        let dir = TempDir::new();
        let write_blocked = AtomicBool::new(true);
        let broken = "{broken";
        std::fs::write(dir.file(), broken).unwrap();
        assert!(
            load_recovering_from(&dir.file(), &write_blocked)
                .unwrap()
                .is_empty()
        );
        let backup = dir.file().with_extension("json.unreadable");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), broken);
        assert!(!write_blocked.load(Ordering::Relaxed));
        let notes = vec![new_note("新便签", 123, Placement::Archived)];
        save_checked_to(&dir.file(), &notes, &write_blocked).unwrap();
        assert_eq!(
            load_recovering_from(&dir.file(), &write_blocked).unwrap(),
            notes
        );
        assert_eq!(std::fs::read_to_string(backup).unwrap(), broken);
    }

    #[test]
    fn non_finite_geometry_cannot_corrupt_a_readable_file() {
        // 无效窗口坐标或尺寸不能被静默写成 null，导致重启时所有便签无法读取。
        let dir = TempDir::new();
        let original = vec![new_note("便签", 123, Placement::Archived)];
        save_to(&dir.file(), &original).unwrap();
        let mut invalid = original.clone();
        invalid[0].size = Some((f64::NAN, 240.0));
        assert!(save_to(&dir.file(), &invalid).is_err());
        invalid[0].size = None;
        invalid[0].placement = Placement::Floating {
            frame: Frame {
                x: f64::INFINITY,
                ..visible()
            },
        };
        assert!(save_to(&dir.file(), &invalid).is_err());
        assert_eq!(load_from(&dir.file()).unwrap(), original);
    }

    #[test]
    fn failed_replace_preserves_destination_and_removes_temp_file() {
        // rename 失败也不能残留正文临时文件，或动到原来的目标目录。
        let dir = TempDir::new();
        std::fs::create_dir(dir.file()).unwrap();
        let original = dir.file().join("original");
        std::fs::write(&original, "keep").unwrap();
        assert!(save_to(&dir.file(), &[new_note("x", 1, Placement::Archived)]).is_err());
        assert_eq!(std::fs::read_to_string(original).unwrap(), "keep");
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn new_notes_have_unique_v4_ids_and_preserve_input() {
        // 同一秒内新建多张也不能撞 id；正文和调用者给定的时间、状态不能被隐式改写。
        let a = new_note("  中文\n", 123, Placement::Archived);
        let b = new_note("  中文\n", 123, Placement::Archived);
        assert_ne!(a.id, b.id);
        assert_eq!(uuid::Uuid::parse_str(&a.id).unwrap().get_version_num(), 4);
        assert_eq!(a.text, "  中文\n");
        assert_eq!((a.created_at, a.updated_at), (123, 123));
        assert_eq!(a.placement, Placement::Archived);
        assert_eq!(a.size, None);
    }

    #[test]
    fn snap_selects_nearest_side_including_threshold_and_overshoot() {
        // 拖拽可以略微越过边界；使用绝对距离且包含阈值，避免左右侧手感不一致。
        let screen = visible();
        let frame = Frame {
            x: screen.x + 16.0,
            y: 0.0,
            w: 320.0,
            h: 240.0,
        };
        assert_eq!(snap_edge(frame, screen, 16.0), Some(Edge::Left));
        assert_eq!(
            snap_edge(Frame { x: -336.0, ..frame }, screen, 16.0),
            Some(Edge::Right)
        );
        assert_eq!(snap_edge(Frame { x: -600.0, ..frame }, screen, 16.0), None);
        assert_eq!(
            snap_edge(
                Frame {
                    x: screen.x - 8.0,
                    ..frame
                },
                screen,
                16.0
            ),
            Some(Edge::Left)
        );
        assert_eq!(
            snap_edge(Frame { x: -312.0, ..frame }, screen, 16.0),
            Some(Edge::Right)
        );
        assert_eq!(
            snap_edge(
                Frame {
                    x: screen.x - 17.0,
                    ..frame
                },
                screen,
                16.0
            ),
            None
        );

        // 几乎占满屏幕的便签两侧都靠近时，不能简单地永远优先左侧。
        assert_eq!(
            snap_edge(
                Frame {
                    x: -1190.0,
                    w: 1188.0,
                    ..frame
                },
                screen,
                16.0
            ),
            Some(Edge::Right)
        );
        assert_eq!(
            snap_edge(
                Frame {
                    x: -1198.0,
                    w: 1188.0,
                    ..frame
                },
                screen,
                16.0
            ),
            Some(Edge::Left)
        );
        assert_eq!(
            snap_edge(
                Frame {
                    x: -1198.0,
                    w: 1196.0,
                    ..frame
                },
                screen,
                16.0
            ),
            Some(Edge::Left)
        );
        assert_eq!(
            snap_edge(
                Frame {
                    x: screen.x,
                    ..frame
                },
                screen,
                0.0
            ),
            Some(Edge::Left)
        );
    }

    #[test]
    fn layout_centers_three_tabs_in_order_on_either_edge() {
        // 多张耳标应可分别命中，并且右侧不能跨到相邻显示器。
        let screen = visible();
        for edge in [Edge::Left, Edge::Right] {
            let layout = dock_layout(&["a", "b", "c"], edge, screen);
            assert_eq!(
                layout.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
                ["a", "b", "c"]
            );
            assert_inside_and_separated(&layout, screen);
            assert!(layout.iter().all(|(_, frame)| frame.h == TAB_H));
            let expected_x = if edge == Edge::Left {
                screen.x
            } else {
                screen.x + screen.w - TAB_W
            };
            assert!(layout.iter().all(|(_, frame)| frame.x == expected_x));
            assert_eq!(
                layout[0].1.y - screen.y,
                screen.y + screen.h - (layout[2].1.y + TAB_H)
            );
        }
    }

    #[test]
    fn layout_compresses_before_truncating_at_minimum_height() {
        // 空间不足先压缩，不能直接隐藏便签；超载时保留队首且最小高度仍可点击。
        let screen = Frame {
            h: 160.0,
            ..visible()
        };
        let layout = dock_layout(&["a", "b", "c"], Edge::Left, screen);
        assert_eq!(layout.len(), 3);
        assert!(layout.iter().all(|(_, frame)| frame.h == 48.0));
        assert_inside_and_separated(&layout, screen);

        let ids: Vec<String> = (0..50).map(|n| n.to_string()).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let screen = visible();
        let layout = dock_layout(&refs, Edge::Right, screen);
        assert_eq!(layout.len(), 25);
        assert_eq!(layout.last().unwrap().0, "24");
        assert!(layout.iter().all(|(_, frame)| frame.h == TAB_MIN_H));
        assert_inside_and_separated(&layout, screen);
    }

    #[test]
    fn compressed_layout_stays_strictly_inside_screen() {
        let ids: Vec<String> = (0..25).map(|n| n.to_string()).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        for count in [14, 25] {
            for edge in [Edge::Left, Edge::Right] {
                let layout = dock_layout(&refs[..count], edge, visible());
                assert_eq!(layout.len(), count);
                assert_inside_and_separated(&layout, visible());
            }
        }
    }

    #[test]
    fn truncated_layout_uses_remaining_space_and_checks_capacity_rounding() {
        let screen = Frame {
            h: 70.0,
            ..visible()
        };
        let layout = dock_layout(&["a", "b", "c"], Edge::Left, screen);
        assert_eq!(layout.len(), 2);
        assert!(layout.iter().all(|(_, frame)| frame.h == 31.0));
        assert_inside_and_separated(&layout, screen);

        let screen = Frame {
            h: 64.0_f64.next_down(),
            ..visible()
        };
        let layout = dock_layout(&["a", "b"], Edge::Right, screen);
        assert_eq!(layout.len(), 1);
        assert_inside_and_separated(&layout, screen);
    }

    #[test]
    fn invalid_geometry_and_tiny_displays_do_not_produce_offscreen_tabs() {
        // 断屏或屏幕参数尚未就绪时，不允许生成越界或 NaN 的窗口坐标。
        assert!(dock_layout(&[], Edge::Left, visible()).is_empty());
        for frame in [
            Frame {
                w: TAB_W - 1.0,
                ..visible()
            },
            Frame {
                h: TAB_MIN_H - 1.0,
                ..visible()
            },
            Frame {
                h: 0.0,
                ..visible()
            },
            Frame {
                x: f64::NAN,
                ..visible()
            },
            Frame {
                w: f64::INFINITY,
                ..visible()
            },
        ] {
            assert!(dock_layout(&["a"], Edge::Right, frame).is_empty());
        }
        for threshold in [-1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(snap_edge(visible(), visible(), threshold), None);
        }
        assert_eq!(
            snap_edge(
                Frame {
                    w: -1.0,
                    ..visible()
                },
                visible(),
                16.0
            ),
            None
        );
        let screen = Frame {
            w: TAB_W,
            h: TAB_MIN_H,
            ..visible()
        };
        let layout = dock_layout(&["a", "b"], Edge::Right, screen);
        assert_eq!(layout.len(), 1);
        assert_eq!(layout[0].1, screen);
    }

    #[test]
    fn next_order_only_counts_matching_display_and_edge() {
        // 两个显示器、左右两侧是独立队列；归档/浮窗不能改变入队顺序。
        assert_eq!(next_order(&[], Edge::Left, "a"), 0);
        let mut notes = vec![
            docked(Edge::Left, "a", 2),
            docked(Edge::Left, "a", 7),
            docked(Edge::Left, "b", 100),
            docked(Edge::Right, "a", 200),
            new_note("归档", 1, Placement::Archived),
            new_note("浮窗", 1, Placement::Floating { frame: visible() }),
        ];
        assert_eq!(next_order(&notes, Edge::Left, "a"), 8);
        assert_eq!(next_order(&notes, Edge::Right, "a"), 201);
        assert_eq!(next_order(&notes, Edge::Right, "b"), 0);
        notes.push(docked(Edge::Left, "a", u32::MAX));
        assert_eq!(next_order(&notes, Edge::Left, "a"), u32::MAX);
    }
}
