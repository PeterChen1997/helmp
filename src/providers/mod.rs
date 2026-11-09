//! Provider 注册表。新增模块：在这里加 `mod xxx;`，并在 `all()` 里加一行。

mod agents;
mod apps;
mod builtin;
mod calculator;
pub mod clipboard;
mod screenshot;
mod scripts;
mod system;
mod translate;
pub mod websearch;

use std::sync::Arc;

use crate::provider::Provider;

pub fn all() -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(builtin::Builtin),
        Arc::new(calculator::Calculator),
        Arc::new(system::System::new()),
        Arc::new(apps::Apps::default()),
        Arc::new(screenshot::Screenshot),
        Arc::new(clipboard::Clipboard::new()),
        Arc::new(scripts::Scripts::new()),
        Arc::new(agents::Agents::default()),
        Arc::new(translate::Translate::default()),
        Arc::new(websearch::WebSearch::new()),
    ]
}
