//! Provider 注册表。新增模块：在这里加 `mod xxx;`，并在 `all()` 里加一行。

mod apps;
mod builtin;
mod screenshot;
mod clipboard;

use std::sync::Arc;

use crate::provider::Provider;

pub fn all() -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(builtin::Builtin),
        Arc::new(apps::Apps::default()),
        Arc::new(screenshot::Screenshot),
        Arc::new(clipboard::Clipboard::new()),
    ]
}
