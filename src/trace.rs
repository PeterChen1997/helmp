//! `HELMP_TRACE=1` 时把界面关键事件打到 stderr，供端到端验证对照（看不到窗口像素时也能确认行为）。

use std::sync::OnceLock;
use std::time::Instant;

fn start() -> Option<Instant> {
    static START: OnceLock<Option<Instant>> = OnceLock::new();
    *START.get_or_init(|| std::env::var_os("HELMP_TRACE").map(|_| Instant::now()))
}

pub fn log(message: impl AsRef<str>) {
    if let Some(start) = start() {
        eprintln!(
            "trace +{:>6}ms {}",
            start.elapsed().as_millis(),
            message.as_ref()
        );
    }
}
