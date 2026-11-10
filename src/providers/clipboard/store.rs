//! 剪贴板历史的持久化：`clipboard.sqlite` + 图片文件目录。
//!
//! - 不设条数上限（「无上限」是剪贴板用户的第一诉求）。
//! - 同内容（按 sha256 判重）再次复制时不新增行，而是把原行移到最前（`seq` 取新值），
//!   这样 `clipboard:<行 id>` 在整个生命周期内保持稳定。
//! - 图片落盘到 `clipboard-images/<hash>.<png|tiff>`，库里只存文件名。
//! - `pinned = 1` 的条目是用户置顶的：浏览时排在最前，按保留天数清理时跳过。旧库打开时自动补这一列。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use rusqlite::{Connection, OptionalExtension as _, Row, params};
use sha2::{Digest as _, Sha256};

use crate::i18n::{tr, trf};

pub const DB_FILE: &str = "clipboard.sqlite";
pub const IMAGES_DIR: &str = "clipboard-images";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Image,
    Files,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Image => "image",
            Kind::Files => "files",
        }
    }

    fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "text" => Kind::Text,
            "image" => Kind::Image,
            "files" => Kind::Files,
            other => bail!(trf!(
                "剪贴板库里有未知类型 {other:?}",
                "Unknown item type {other:?} in the clipboard database"
            )),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Tiff,
}

impl ImageFormat {
    pub fn ext(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Tiff => "tiff",
        }
    }

    pub fn from_file_name(name: &str) -> Result<Self> {
        match Path::new(name).extension().and_then(|ext| ext.to_str()) {
            Some("png") => Ok(ImageFormat::Png),
            Some("tiff") => Ok(ImageFormat::Tiff),
            _ => bail!(trf!(
                "剪贴板图片 {name:?} 扩展名不是 png/tiff",
                "Clipboard image {name:?} isn’t a .png or .tiff file"
            )),
        }
    }
}

/// 一次采集到的新内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewClip {
    Text(String),
    Files(Vec<PathBuf>),
    Image {
        bytes: Vec<u8>,
        format: ImageFormat,
        width: u32,
        height: u32,
    },
}

/// 复制发生时的前台 App（NSPasteboard 不暴露写入者，只能这样推断）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub bundle_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: i64,
    pub kind: Kind,
    /// 文本：原文；文件：每行一个绝对路径；图片：图片目录下的文件名。
    /// `Store::load_all` 返回的是截断后的预览，`Store::get` 返回完整内容。
    pub content: String,
    /// 完整内容的字符数（截断前）。
    pub chars: usize,
    pub width: u32,
    pub height: u32,
    pub source: Option<Source>,
    /// 最近一次被复制（或被选中写回）的时间，Unix 毫秒。
    pub copied_at_ms: i64,
    /// 越大越新，决定历史顺序。
    pub seq: i64,
    /// 用户置顶：浏览时排最前，不受保留天数清理。
    pub pinned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Added {
    New,
    /// 已有相同内容，原行被移到最前。
    Moved,
}

pub struct Store {
    conn: Connection,
    images_dir: PathBuf,
    next_seq: i64,
}

const COLUMNS: &str = "id, kind, content, length(content), width, height, source_name, source_bundle, copied_at, seq, pinned";

impl Store {
    /// 打开（必要时创建）`<dir>/clipboard.sqlite` 与图片目录。
    pub fn open(dir: &Path) -> Result<Self> {
        let images_dir = dir.join(IMAGES_DIR);
        fs::create_dir_all(&images_dir).with_context(|| {
            trf!(
                "创建剪贴板图片目录 {}",
                "Couldn’t create the clipboard image folder {}",
                images_dir.display()
            )
        })?;
        let db_path = dir.join(DB_FILE);
        let conn = Connection::open(&db_path).with_context(|| {
            trf!(
                "打开剪贴板数据库 {}",
                "Couldn’t open the clipboard database {}",
                db_path.display()
            )
        })?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS clips (
                id            INTEGER PRIMARY KEY,
                kind          TEXT    NOT NULL,
                content       TEXT    NOT NULL,
                hash          TEXT    NOT NULL UNIQUE,
                width         INTEGER NOT NULL DEFAULT 0,
                height        INTEGER NOT NULL DEFAULT 0,
                source_name   TEXT,
                source_bundle TEXT,
                copied_at     INTEGER NOT NULL,
                seq           INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS clips_seq ON clips(seq DESC);",
        )
        .context(tr(
            "初始化剪贴板数据库表",
            "Couldn’t set up the clipboard database tables",
        ))?;
        add_pinned_column(&conn)?;
        let max_seq: i64 =
            conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM clips", [], |row| {
                row.get(0)
            })?;
        Ok(Self {
            conn,
            images_dir,
            next_seq: max_seq + 1,
        })
    }

    /// 记录一次复制。相同内容不新增，而是移到最前；`source` 为 `None` 时保留原来源。
    pub fn add(
        &mut self,
        clip: NewClip,
        source: Option<&Source>,
        now_ms: i64,
    ) -> Result<(Entry, Added)> {
        let (kind, content, hash, width, height) = match clip {
            NewClip::Text(text) => {
                let hash = digest(Kind::Text, text.as_bytes());
                (Kind::Text, text, hash, 0, 0)
            }
            NewClip::Files(paths) => {
                let lines: Vec<String> = paths
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect();
                let content = lines.join("\n");
                let hash = digest(Kind::Files, content.as_bytes());
                (Kind::Files, content, hash, 0, 0)
            }
            NewClip::Image {
                bytes,
                format,
                width,
                height,
            } => {
                let hash = digest(Kind::Image, &bytes);
                let name = format!("{}.{}", &hash[..32], format.ext());
                self.write_image(&name, &bytes)?;
                (Kind::Image, name, hash, width, height)
            }
        };

        let seq = self.next_seq;
        let (source_name, source_bundle) = match source {
            Some(source) => (Some(source.name.as_str()), source.bundle_id.as_deref()),
            None => (None, None),
        };
        let existing: Option<i64> = self
            .conn
            .query_row("SELECT id FROM clips WHERE hash = ?1", [&hash], |row| {
                row.get(0)
            })
            .optional()?;
        let (id, added) = match existing {
            Some(id) => {
                if source.is_some() {
                    self.conn.execute(
                        "UPDATE clips SET seq = ?1, copied_at = ?2, source_name = ?3, source_bundle = ?4 WHERE id = ?5",
                        params![seq, now_ms, source_name, source_bundle, id],
                    )?;
                } else {
                    self.conn.execute(
                        "UPDATE clips SET seq = ?1, copied_at = ?2 WHERE id = ?3",
                        params![seq, now_ms, id],
                    )?;
                }
                (id, Added::Moved)
            }
            None => {
                self.conn.execute(
                    "INSERT INTO clips (kind, content, hash, width, height, source_name, source_bundle, copied_at, seq)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        kind.as_str(),
                        content,
                        hash,
                        width,
                        height,
                        source_name,
                        source_bundle,
                        now_ms,
                        seq
                    ],
                )?;
                (self.conn.last_insert_rowid(), Added::New)
            }
        };
        self.next_seq += 1;
        let entry = self.get(id)?.with_context(|| {
            trf!(
                "剪贴板第 {id} 条写入后读不回来",
                "Clipboard item #{id} couldn’t be read back after saving"
            )
        })?;
        Ok((entry, added))
    }

    /// 把某条移到最前（用户从历史里选中写回时调用）。
    pub fn touch(&mut self, id: i64, now_ms: i64) -> Result<Entry> {
        let changed = self.conn.execute(
            "UPDATE clips SET seq = ?1, copied_at = ?2 WHERE id = ?3",
            params![self.next_seq, now_ms, id],
        )?;
        anyhow::ensure!(
            changed == 1,
            trf!(
                "剪贴板历史里没有第 {id} 条",
                "Clipboard history has no item #{id}"
            )
        );
        self.next_seq += 1;
        self.get(id)?.with_context(|| {
            trf!(
                "剪贴板第 {id} 条更新后读不回来",
                "Clipboard item #{id} couldn’t be read back after updating"
            )
        })
    }

    /// 置顶 / 取消置顶。
    pub fn set_pinned(&mut self, id: i64, pinned: bool) -> Result<Entry> {
        let changed = self.conn.execute(
            "UPDATE clips SET pinned = ?1 WHERE id = ?2",
            params![pinned, id],
        )?;
        anyhow::ensure!(
            changed == 1,
            trf!(
                "剪贴板历史里没有第 {id} 条",
                "Clipboard history has no item #{id}"
            )
        );
        self.get(id)?.with_context(|| {
            trf!(
                "剪贴板第 {id} 条更新后读不回来",
                "Clipboard item #{id} couldn’t be read back after updating"
            )
        })
    }

    /// 完整内容。
    pub fn get(&self, id: i64) -> Result<Option<Entry>> {
        let sql = format!("SELECT {COLUMNS} FROM clips WHERE id = ?1");
        self.conn
            .query_row(&sql, [id], |row| Ok(raw_entry(row)))
            .optional()?
            .transpose()
    }

    /// 全部历史，新的在前；`content` 截断到 `preview_chars` 个字符（内存索引只需要这么多）。
    pub fn load_all(&self, preview_chars: usize) -> Result<Vec<Entry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, substr(content, 1, ?1), length(content), width, height, source_name, source_bundle, copied_at, seq, pinned
             FROM clips ORDER BY seq DESC",
        )?;
        let limit = i64::try_from(preview_chars).unwrap_or(i64::MAX);
        let rows = stmt.query_map([limit], |row| Ok(raw_entry(row)))?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row??);
        }
        Ok(entries)
    }

    /// 删除一条；图片条目连同图片文件一起删。
    pub fn delete(&mut self, id: i64) -> Result<bool> {
        let Some(entry) = self.get(id)? else {
            return Ok(false);
        };
        self.conn.execute("DELETE FROM clips WHERE id = ?1", [id])?;
        if entry.kind == Kind::Image {
            remove_image(&self.image_path(&entry))?;
        }
        Ok(true)
    }

    /// 删除最近一次复制早于 `cutoff_ms` 的条目（保留天数到期），返回被删的 id。置顶的不删。
    pub fn prune_before(&mut self, cutoff_ms: i64) -> Result<Vec<i64>> {
        let expired: Vec<(i64, Option<String>)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, kind, content FROM clips WHERE copied_at < ?1 AND pinned = 0")?;
            let rows = stmt.query_map([cutoff_ms], |row| {
                let kind: String = row.get(1)?;
                let image = if kind == Kind::Image.as_str() {
                    Some(row.get::<_, String>(2)?)
                } else {
                    None
                };
                Ok((row.get::<_, i64>(0)?, image))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        self.conn
            .execute(
                "DELETE FROM clips WHERE copied_at < ?1 AND pinned = 0",
                [cutoff_ms],
            )
            .context(tr(
                "清理过期剪贴板历史",
                "Couldn’t remove expired clipboard history",
            ))?;
        for image in expired.iter().filter_map(|(_, image)| image.as_deref()) {
            remove_image(&self.images_dir.join(image))?;
        }
        Ok(expired.into_iter().map(|(id, _)| id).collect())
    }

    /// 清空全部历史（含图片文件），返回删除的条数。
    pub fn clear(&mut self) -> Result<usize> {
        let removed = self
            .conn
            .execute("DELETE FROM clips", [])
            .context(tr("清空剪贴板历史", "Couldn’t clear clipboard history"))?;
        let entries = match fs::read_dir(&self.images_dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(removed),
            Err(err) => {
                return Err(err).with_context(|| {
                    trf!("读取 {}", "Couldn’t read {}", self.images_dir.display())
                });
            }
        };
        for entry in entries {
            let entry = entry
                .with_context(|| trf!("读取 {}", "Couldn’t read {}", self.images_dir.display()))?;
            remove_image(&entry.path())?;
        }
        Ok(removed)
    }

    /// 条数与占用的磁盘字节数（数据库 + WAL + 图片目录），给设置窗口显示。
    pub fn stats(&self, dir: &Path) -> Result<(usize, u64)> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM clips", [], |row| row.get(0))?;
        let mut bytes = 0;
        for name in [DB_FILE.to_string(), format!("{DB_FILE}-wal")] {
            bytes += fs::metadata(dir.join(name)).map_or(0, |m| m.len());
        }
        if let Ok(entries) = fs::read_dir(&self.images_dir) {
            for entry in entries.flatten() {
                bytes += entry.metadata().map_or(0, |m| m.len());
            }
        }
        Ok((usize::try_from(count).unwrap_or(0), bytes))
    }

    pub fn image_path(&self, entry: &Entry) -> PathBuf {
        self.images_dir.join(&entry.content)
    }

    fn write_image(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let path = self.images_dir.join(name);
        if path.exists() {
            return Ok(());
        }
        // 先写临时文件再改名，避免进程中途退出留下半张图。
        let tmp = self.images_dir.join(format!("{name}.tmp"));
        fs::write(&tmp, bytes).with_context(|| {
            trf!(
                "写剪贴板图片 {}",
                "Couldn’t write clipboard image {}",
                tmp.display()
            )
        })?;
        fs::rename(&tmp, &path).with_context(|| {
            trf!(
                "保存剪贴板图片 {}",
                "Couldn’t save clipboard image {}",
                path.display()
            )
        })?;
        Ok(())
    }
}

/// 旧版本的库没有 `pinned` 列：补上（默认不置顶）。
fn add_pinned_column(conn: &Connection) -> Result<()> {
    let has_column = conn
        .prepare("SELECT 1 FROM pragma_table_info('clips') WHERE name = 'pinned'")?
        .exists([])?;
    if !has_column {
        conn.execute_batch("ALTER TABLE clips ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0")
            .context(tr(
                "给剪贴板数据库补置顶列",
                "Couldn’t add the pinned column to the clipboard database",
            ))?;
    }
    Ok(())
}

/// 删图片文件；文件已经不在（用户手动清过目录）不算错误，其他 IO 错误照常上报。
fn remove_image(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err).with_context(|| {
            trf!(
                "删除剪贴板图片 {}",
                "Couldn’t delete clipboard image {}",
                path.display()
            )
        }),
        _ => Ok(()),
    }
}

fn raw_entry(row: &Row<'_>) -> Result<Entry> {
    let kind: String = row.get(1)?;
    let chars: i64 = row.get(3)?;
    let source_name: Option<String> = row.get(6)?;
    let source_bundle: Option<String> = row.get(7)?;
    Ok(Entry {
        id: row.get(0)?,
        kind: Kind::parse(&kind)?,
        content: row.get(2)?,
        chars: usize::try_from(chars).unwrap_or(0),
        width: row.get(4)?,
        height: row.get(5)?,
        source: source_name.map(|name| Source {
            name,
            bundle_id: source_bundle,
        }),
        copied_at_ms: row.get(8)?,
        seq: row.get(9)?,
        pinned: row.get(10)?,
    })
}

fn digest(kind: Kind, bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 测试专用临时目录，析构时删除。
    pub(crate) struct TempDir(pub PathBuf);

    impl TempDir {
        pub(crate) fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "helmp-clipboard-{tag}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn text(s: &str) -> NewClip {
        NewClip::Text(s.to_string())
    }

    #[test]
    fn duplicate_moves_to_front_and_keeps_id() {
        // 同一段文字反复复制是常态：若每次都新增一行，历史会被重复项淹没；
        // id 不变则面板上已经显示的结果在选中时仍然有效。
        let dir = TempDir::new("dup");
        let mut store = Store::open(&dir.0).unwrap();
        let (a, added_a) = store.add(text("alpha"), None, 1).unwrap();
        let (b, _) = store.add(text("beta"), None, 2).unwrap();
        assert_eq!(added_a, Added::New);

        let (again, added) = store.add(text("alpha"), None, 3).unwrap();
        assert_eq!(added, Added::Moved);
        assert_eq!(again.id, a.id);
        assert_eq!(again.copied_at_ms, 3);

        let order: Vec<i64> = store.load_all(100).unwrap().iter().map(|e| e.id).collect();
        assert_eq!(order, vec![a.id, b.id]);
    }

    #[test]
    fn duplicate_without_source_keeps_original_source() {
        // 启动时的补采集拿不到真实来源（此时前台是终端或启动器本身），不能覆盖掉原来记下的来源。
        let dir = TempDir::new("src");
        let mut store = Store::open(&dir.0).unwrap();
        let safari = Source {
            name: "Safari".into(),
            bundle_id: Some("com.apple.Safari".into()),
        };
        store.add(text("x"), Some(&safari), 1).unwrap();
        let (entry, _) = store.add(text("x"), None, 2).unwrap();
        assert_eq!(entry.source, Some(safari));
    }

    #[test]
    fn add_get_delete_round_trip() {
        let dir = TempDir::new("crud");
        let mut store = Store::open(&dir.0).unwrap();
        let (entry, _) = store.add(text("你好\n世界"), None, 10).unwrap();
        assert_eq!(entry.chars, 5);
        assert_eq!(store.get(entry.id).unwrap().unwrap().content, "你好\n世界");
        assert!(store.delete(entry.id).unwrap());
        assert!(store.get(entry.id).unwrap().is_none());
        assert!(!store.delete(entry.id).unwrap());
    }

    #[test]
    fn history_survives_reopen_and_seq_keeps_growing() {
        // 历史要跨重启保留；重开后新复制的内容必须排在旧内容前面。
        let dir = TempDir::new("reopen");
        let old_id = {
            let mut store = Store::open(&dir.0).unwrap();
            store.add(text("old"), None, 1).unwrap().0.id
        };
        let mut store = Store::open(&dir.0).unwrap();
        let (new, _) = store.add(text("new"), None, 2).unwrap();
        let order: Vec<i64> = store.load_all(100).unwrap().iter().map(|e| e.id).collect();
        assert_eq!(order, vec![new.id, old_id]);
    }

    #[test]
    fn load_all_truncates_preview_but_get_returns_full() {
        let dir = TempDir::new("preview");
        let mut store = Store::open(&dir.0).unwrap();
        let long = "长".repeat(50);
        let (entry, _) = store.add(NewClip::Text(long.clone()), None, 1).unwrap();
        let preview = store.load_all(10).unwrap().remove(0);
        assert_eq!(preview.content, "长".repeat(10));
        assert_eq!(preview.chars, 50);
        assert_eq!(store.get(entry.id).unwrap().unwrap().content, long);
    }

    fn image(bytes: &[u8]) -> NewClip {
        NewClip::Image {
            bytes: bytes.to_vec(),
            format: ImageFormat::Png,
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn prune_removes_only_expired_entries_and_their_images() {
        // 「保留 N 天」是隐私承诺：过期内容（含图片文件）必须真的从磁盘消失，未过期的不能误删。
        let dir = TempDir::new("prune");
        let mut store = Store::open(&dir.0).unwrap();
        let (old, _) = store.add(text("old"), None, 1_000).unwrap();
        let (old_image, _) = store.add(image(b"img"), None, 1_500).unwrap();
        let image_file = store.image_path(&old_image);
        let (fresh, _) = store.add(text("fresh"), None, 5_000).unwrap();
        assert!(image_file.exists());

        let mut removed = store.prune_before(2_000).unwrap();
        removed.sort();
        assert_eq!(removed, vec![old.id, old_image.id]);
        assert!(!image_file.exists(), "过期图片文件必须删掉");
        let left: Vec<i64> = store.load_all(10).unwrap().iter().map(|e| e.id).collect();
        assert_eq!(left, vec![fresh.id]);
    }

    #[test]
    fn clear_wipes_rows_and_image_files() {
        // 设置里的「清空全部历史」：库和图片目录都要清干净，否则旧截图还躺在磁盘上。
        let dir = TempDir::new("clear");
        let mut store = Store::open(&dir.0).unwrap();
        store.add(text("a"), None, 1).unwrap();
        let (img, _) = store.add(image(b"img"), None, 2).unwrap();
        assert_eq!(store.stats(&dir.0).unwrap().0, 2);
        assert_eq!(store.clear().unwrap(), 2);
        assert!(store.load_all(10).unwrap().is_empty());
        assert!(!store.image_path(&img).exists());
        assert_eq!(store.stats(&dir.0).unwrap().0, 0);
    }

    #[test]
    fn pinned_entries_survive_retention_and_old_databases_migrate() {
        // 置顶就是「这条我要一直留着」：保留天数到期也不能删；老版本的库没有置顶列，打开时要补上而不是报错。
        let dir = TempDir::new("pin");
        {
            let conn = Connection::open(dir.0.join(DB_FILE)).unwrap();
            conn.execute_batch(
                "CREATE TABLE clips (id INTEGER PRIMARY KEY, kind TEXT NOT NULL, content TEXT NOT NULL,
                 hash TEXT NOT NULL UNIQUE, width INTEGER NOT NULL DEFAULT 0, height INTEGER NOT NULL DEFAULT 0,
                 source_name TEXT, source_bundle TEXT, copied_at INTEGER NOT NULL, seq INTEGER NOT NULL);
                 INSERT INTO clips (kind, content, hash, copied_at, seq) VALUES ('text', 'legacy', 'h1', 1, 1);",
            )
            .unwrap();
        }
        let mut store = Store::open(&dir.0).unwrap();
        let legacy = store.load_all(10).unwrap().remove(0);
        assert!(!legacy.pinned);
        let (old, _) = store.add(text("old"), None, 2).unwrap();
        assert!(store.set_pinned(old.id, true).unwrap().pinned);
        let removed = store.prune_before(1_000).unwrap();
        assert_eq!(removed, vec![legacy.id]);
        assert!(store.get(old.id).unwrap().is_some());
        assert!(!store.set_pinned(old.id, false).unwrap().pinned);
        assert!(store.set_pinned(9999, true).is_err());
    }

    #[test]
    fn touch_moves_to_front() {
        let dir = TempDir::new("touch");
        let mut store = Store::open(&dir.0).unwrap();
        let (a, _) = store.add(text("a"), None, 1).unwrap();
        store.add(text("b"), None, 2).unwrap();
        store.touch(a.id, 3).unwrap();
        assert_eq!(store.load_all(10).unwrap()[0].id, a.id);
        assert!(store.touch(9999, 4).is_err());
    }
}
