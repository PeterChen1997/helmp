//! Provider 注册表。新增模块：在这里加 `mod xxx;`，并在 `all()` 里加一行。

mod agents;
mod apps;
mod bookmarks;
mod builtin;
mod calculator;
mod processes;
pub mod clipboard;
mod dictionary;
mod files;
pub mod plugins;
pub mod mcp;
mod color;
mod devtools;
mod emoji;
pub(crate) mod screenshot;
mod scripts;
mod settings_panes;
pub mod snippets;
mod system;
mod toggles;
mod time;
mod translate;
mod window;
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
        Arc::new(plugins::Plugins::new()),
        Arc::new(mcp::Mcp),
        Arc::new(agents::Agents::default()),
        Arc::new(translate::Translate::default()),
        Arc::new(processes::Processes::default()),
        Arc::new(window::Window),
        Arc::new(snippets::Snippets::default()),
        Arc::new(websearch::WebSearch::new()),
        Arc::new(files::Files::new()),
        Arc::new(settings_panes::SettingsPanes),
        Arc::new(toggles::Toggles::new()),
        Arc::new(bookmarks::Bookmarks::default()),
        Arc::new(dictionary::Dictionary::new()),
        Arc::new(time::Time),
        Arc::new(devtools::DevTools),
        Arc::new(emoji::Emoji),
        Arc::new(color::Color),
    ]
}
