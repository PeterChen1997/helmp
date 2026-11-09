//! 深链接 `helmp://`：脚本、快捷指令、Alfred / Raycast 工作流和端到端验收都能直接调出 Helmp。
//!
//! ```text
//! helmp://show[?q=文字]          唤起面板，可带查询
//! helmp://scope/<id>[?q=文字]    进入某个扩展（translate、clipboard、agents、screenshot…）
//! helmp://settings[/<页面>]      打开设置（general、extensions、models、agents、clipboard、screenshot、appearance、about）
//! helmp://hide                   收起面板
//! ```
//!
//! 验收专用：设置了环境变量 `HELMP_SNAPSHOT_DIR` 时，`helmp://debug/snapshot?q=名字` 把 Helmp
//! 当前可见的每个窗口截成 `<目录>/<名字>-<窗口号>.png`（截图子进程沿用 Helmp 的录屏授权）。
//! 没设这个变量时该链接不被识别，普通用户用不到。

use std::sync::mpsc;
use std::time::Duration;

use gpui_kit::{App, Application, AsyncApp};

use crate::launcher::LauncherHandle;
use crate::settings::Page;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Show { query: Option<String> },
    Scope { id: String, query: Option<String> },
    Settings { page: Option<Page> },
    Hide,
    Snapshot { name: String },
}

/// 解析一个 `helmp://` 链接；不认识的返回 None（由调用方打印原因）。
pub fn parse(url: &str) -> Option<Link> {
    let rest = url.strip_prefix("helmp://")?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let q = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == "q")
        .map(|(_, value)| percent_decode(value))
        .filter(|value| !value.is_empty());
    let mut parts = path.trim_matches('/').split('/');
    match (parts.next()?, parts.next()) {
        ("show" | "", None) => Some(Link::Show { query: q }),
        ("scope", Some(id)) if !id.is_empty() => Some(Link::Scope {
            id: id.to_string(),
            query: q,
        }),
        ("settings", None) => Some(Link::Settings { page: None }),
        ("settings", Some(slug)) => Page::ALL
            .into_iter()
            .find(|page| format!("{page:?}").eq_ignore_ascii_case(slug))
            .map(|page| Link::Settings { page: Some(page) }),
        ("hide", None) => Some(Link::Hide),
        ("debug", Some("snapshot")) if snapshot_dir().is_some() => Some(Link::Snapshot {
            name: q.unwrap_or_else(|| "snapshot".into()),
        }),
        _ => None,
    }
}

/// `%XX` 与 `+` 解码；非法转义原样保留，结果按 UTF-8 宽松解码。
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (byte, hex) {
            (b'%', Some(value)) => {
                out.push(value);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            _ => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 在 `run` 之前注册：系统送来的 URL 先进通道，等应用起来后由 [`install`] 在主线程处理。
pub fn listen(app: &Application) -> mpsc::Receiver<Vec<String>> {
    let (tx, rx) = mpsc::channel();
    app.on_open_urls(move |urls| {
        let _ = tx.send(urls);
    });
    rx
}

pub fn install(rx: mpsc::Receiver<Vec<String>>, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            while let Ok(urls) = rx.try_recv() {
                for url in urls {
                    crate::trace::log(format!("deeplink {url}"));
                    match parse(&url) {
                        Some(link) => cx.update(|cx| open(link, cx)),
                        None => eprintln!("helmp: 不认识的链接 {url}"),
                    }
                }
            }
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
        }
    })
    .detach();
}

fn open(link: Link, cx: &mut App) {
    match link {
        Link::Show { query } => LauncherHandle::update(cx, |launcher, window, cx| {
            launcher.show(window, cx);
            if let Some(query) = query {
                launcher.set_query(&query, window, cx);
            }
        }),
        Link::Scope { id, query } => LauncherHandle::update(cx, |launcher, window, cx| {
            launcher.open_scope_with(&id, query.unwrap_or_default(), window, cx)
        }),
        // 设置窗口会读引擎等全局状态，放到下一轮，避开任何正在进行的实体更新。
        Link::Settings { page } => cx.defer(move |cx| match page {
            Some(page) => crate::settings::open_page(cx, page),
            None => crate::settings::open(cx),
        }),
        Link::Hide => LauncherHandle::update(cx, |launcher, window, cx| launcher.hide(window, cx)),
        Link::Snapshot { name } => snapshot(&name),
    }
}

fn snapshot_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HELMP_SNAPSHOT_DIR").map(Into::into)
}

/// 截下本进程每个可见窗口（含面板、设置、引导），文件名里的名字只保留安全字符。
fn snapshot(name: &str) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    let (Some(dir), Some(mtm)) = (snapshot_dir(), MainThreadMarker::new()) else {
        return;
    };
    let name: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    for window in NSApplication::sharedApplication(mtm).windows() {
        if !window.isVisible() {
            continue;
        }
        let number = window.windowNumber();
        let path = dir.join(format!("{name}-{number}.png"));
        // 按窗口号截（-l）在新系统上对本进程的面板会失败，改按窗口所在区域截。
        // AppKit 坐标原点在主屏左下角，screencapture -R 的原点在左上角。
        let frame = window.frame();
        let screen_height = objc2_app_kit::NSScreen::screens(mtm)
            .firstObject()
            .map(|screen| screen.frame().size.height)
            .unwrap_or_default();
        let top = screen_height - frame.origin.y - frame.size.height;
        let rect = format!(
            "{},{},{},{}",
            frame.origin.x.round(),
            top.round(),
            frame.size.width.round(),
            frame.size.height.round()
        );
        let result = std::process::Command::new("screencapture")
            .args(["-x", "-R", &rect])
            .arg(&path)
            .output();
        let outcome = match result {
            Ok(out) if out.status.success() => "ok".to_string(),
            Ok(out) => format!("失败：{}", String::from_utf8_lossy(&out.stderr).trim()),
            Err(err) => format!("启动失败：{err}"),
        };
        crate::trace::log(format!("snapshot {} → {outcome}", path.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_links() {
        // README 和 docs 里写给用户的链接，必须都能解析成对应动作。
        assert_eq!(
            parse("helmp://show?q=%E5%BE%AE%E4%BF%A1"),
            Some(Link::Show {
                query: Some("微信".into())
            })
        );
        assert_eq!(parse("helmp://"), Some(Link::Show { query: None }));
        assert_eq!(
            parse("helmp://scope/translate?q=hello+world"),
            Some(Link::Scope {
                id: "translate".into(),
                query: Some("hello world".into())
            })
        );
        assert_eq!(
            parse("helmp://settings/extensions"),
            Some(Link::Settings {
                page: Some(Page::Extensions)
            })
        );
        assert_eq!(parse("helmp://settings"), Some(Link::Settings { page: None }));
        assert_eq!(parse("helmp://hide"), Some(Link::Hide));
    }

    #[test]
    fn unknown_links_are_rejected_not_guessed() {
        // 拼错的页面或动作不能悄悄退化成别的操作（比如打开设置首页）。
        for url in [
            "helmp://settings/nope",
            "helmp://scope/",
            "helmp://launch",
            "https://example.com",
        ] {
            assert_eq!(parse(url), None, "{url}");
        }
    }

    #[test]
    fn snapshot_link_needs_the_env_switch() {
        // 验收用的截图链接不能被普通用户（或网页里的链接）触发。
        if std::env::var_os("HELMP_SNAPSHOT_DIR").is_none() {
            assert_eq!(parse("helmp://debug/snapshot?q=x"), None);
        }
    }

    #[test]
    fn malformed_escapes_are_kept_verbatim() {
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%zzb"), "a%zzb");
    }
}
