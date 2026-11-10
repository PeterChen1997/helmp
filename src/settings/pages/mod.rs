//! 各设置页的渲染。每页只读 `config::get()` 并通过 `SettingsView::save` 写回。

mod clipboard;
mod extensions;
mod general;
mod misc;
mod snippets;

pub(super) use snippets::snippets_pane;

use std::path::PathBuf;

use gpui_kit::*;

use super::SettingsView;

impl SettingsView {
    /// 弹出 NSOpenPanel（GPUI 的 `prompt_for_paths`）选一个目录或 App，选中后回调。取消时什么都不做。
    pub(super) fn pick_path(
        &mut self,
        directories: bool,
        prompt: &str,
        cx: &mut Context<Self>,
        on_pick: impl FnOnce(&mut Self, PathBuf, &mut Context<Self>) + 'static,
    ) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: !directories,
            directories,
            multiple: false,
            prompt: Some(prompt.to_string().into()),
        });
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let picked = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) => None,
                Ok(Err(err)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.error = Some(crate::i18n::trf!(
                            "打开选择面板失败：{err:#}",
                            "Couldn't open the file picker: {err:#}"
                        ));
                        cx.notify();
                    });
                    None
                }
                Err(_) => {
                    eprintln!("helmp: 选择面板被意外关闭");
                    None
                }
            };
            if let Some(path) = picked {
                let _ = this.update(cx, |this, cx| on_pick(this, path, cx));
            }
        })
        .detach();
    }
}

/// 路径显示：家目录缩写成 `~`。
pub(super) fn display_path(path: &std::path::Path) -> String {
    let home = crate::paths::home_dir()
        .map(|home| home.to_string_lossy().into_owned())
        .unwrap_or_default();
    super::model::abbreviate_home(&path.to_string_lossy(), &home)
}

/// 默认脚本目录 `<data_dir>/scripts`（脚本插件总是加载它）。
pub(super) fn default_script_dir() -> anyhow::Result<PathBuf> {
    Ok(crate::paths::data_dir()?.join("scripts"))
}
