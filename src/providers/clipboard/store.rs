//! 剪贴板历史的持久化：`clipboard.sqlite` + 图片文件目录。
//!
//! - 不设条数上限（「无上限」是剪贴板用户的第一诉求）。
//! - 同内容（按 sha256 判重）再次复制时不新增行，而是把原行移到最前（`seq` 取新值），
//!   这样 `clipboard:<行 id>` 在整个生命周期内保持稳定。
//! - 图片落盘到 `clipboard-images/<hash>.<png|tiff>`，库里只存文件名。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use rusqlite::{Connection, OptionalExtension as _, Row, params};
use sha2::{Digest as _, Sha256};

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
            other => bail!("剪贴板库里有未知类型 {other:?}"),
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
            _ => bail!("剪贴板图片 {name:?} 扩展名不是 png/tiff"),
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

const COLUMNS: &str =
    "id, kind, content, length(content), width, height, source_name, source_bundle, copied_at, seq";

impl Store {
    /// 打开（必要时创建）`<dir>/clipboard.sqlite` 与图片目录。
    pub fn open(dir: &Path) -> Result<Self> {
        let images_dir = dir.join(IMAGES_DIR);
        fs::create_dir_all(&images_dir)
            .with_context(|| format!("创建剪贴板图片目录 {}", images_dir.display()))?;
        let db_path = dir.join(DB_FILE);
        let conn = Connection::open(&db_path)
            .with_context(|| format!("打开剪贴板数据库 {}", db_path.display()))?;
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
        .context("初始化剪贴板数据库表")?;
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
        let entry = self
            .get(id)?
            .with_context(|| format!("剪贴板第 {id} 条写入后读不回来"))?;
        Ok((entry, added))
    }

    /// 把某条移到最前（用户从历史里选中写回时调用）。
    pub fn touch(&mut self, id: i64, now_ms: i64) -> Result<Entry> {
        let changed = self.conn.execute(
            "UPDATE clips SET seq = ?1, copied_at = ?2 WHERE id = ?3",
            params![self.next_seq, now_ms, id],
        )?;
        anyhow::ensure!(changed == 1, "剪贴板历史里没有第 {id} 条");
        self.next_seq += 1;
        self.get(id)?
            .with_context(|| format!("剪贴板第 {id} 条更新后读不回来"))
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
            "SELECT id, kind, substr(content, 1, ?1), length(content), width, height, source_name, source_bundle, copied_at, seq
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
    // 面板里还没有「删除历史条目」的入口（需要 provider 接口支持次级动作），先由测试覆盖。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn delete(&mut self, id: i64) -> Result<bool> {
        let Some(entry) = self.get(id)? else {
            return Ok(false);
        };
        self.conn.execute("DELETE FROM clips WHERE id = ?1", [id])?;
        if entry.kind == Kind::Image {
            let path = self.image_path(&entry);
            fs::remove_file(&path).with_context(|| format!("删除剪贴板图片 {}", path.display()))?;
        }
        Ok(true)
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
        fs::write(&tmp, bytes).with_context(|| format!("写剪贴板图片 {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("保存剪贴板图片 {}", path.display()))?;
        Ok(())
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
