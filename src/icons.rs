//! 文件 / 应用图标缓存：用 NSWorkspace 取系统图标，渲染成 128 px PNG 存在
//! `<data_dir>/icon-cache/`。第一次请求时在后台线程生成，期间返回 None（界面先画占位，
//! 定时刷新时就会显示出来）。

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context as _, Result, anyhow};
use objc2::AnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWorkspace};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

const SIZE: f64 = 128.0;

#[derive(Clone)]
enum State {
    Pending,
    Ready(PathBuf),
    Failed,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, State>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, State>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// 返回 `path` 的图标 PNG；还没生成好时返回 None 并在后台开始生成。
pub fn png_for(path: &Path) -> Option<PathBuf> {
    let mut entries = cache().lock().expect("icon cache poisoned");
    match entries.get(path) {
        Some(State::Ready(png)) => return Some(png.clone()),
        Some(State::Pending | State::Failed) => return None,
        None => {}
    }
    entries.insert(path.to_path_buf(), State::Pending);
    drop(entries);

    let source = path.to_path_buf();
    std::thread::spawn(move || {
        let state = match generate(&source) {
            Ok(png) => State::Ready(png),
            Err(err) => {
                eprintln!("helmp: 生成图标失败 {}：{err:#}", source.display());
                State::Failed
            }
        };
        cache()
            .lock()
            .expect("icon cache poisoned")
            .insert(source, state);
    });
    None
}

fn cache_file(path: &Path) -> Result<PathBuf> {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    // 应用更新后图标可能变化：把修改时间也算进文件名。
    if let Ok(modified) = std::fs::metadata(path).and_then(|m| m.modified()) {
        modified.hash(&mut hasher);
    }
    let dir = crate::paths::data_dir()?.join("icon-cache");
    std::fs::create_dir_all(&dir).with_context(|| format!("创建 {}", dir.display()))?;
    Ok(dir.join(format!("{:016x}.png", hasher.finish())))
}

fn generate(path: &Path) -> Result<PathBuf> {
    let out = cache_file(path)?;
    if out.exists() {
        return Ok(out);
    }
    let png = render_png(path)?;
    std::fs::write(&out, png).with_context(|| format!("写入 {}", out.display()))?;
    Ok(out)
}

fn render_png(path: &Path) -> Result<Vec<u8>> {
    let path = path.to_str().ok_or_else(|| anyhow!("路径不是 UTF-8"))?;
    let workspace = NSWorkspace::sharedWorkspace();
    let image = workspace.iconForFile(&NSString::from_str(path));
    let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(SIZE, SIZE));
    // SAFETY: rect 是有效的可写指针；context 与 hints 允许为空。
    let cg = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }
        .ok_or_else(|| anyhow!("取不到 CGImage"))?;
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
    // SAFETY: properties 传空字典即可。
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .ok_or_else(|| anyhow!("PNG 编码失败"))?;
    Ok(data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_real_app_icon() {
        // 结果列表的应用图标全靠这条路径；系统自带的「计算器」一定存在。
        let png = render_png(Path::new("/System/Applications/Calculator.app")).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(png.len() > 1000);
    }
}
