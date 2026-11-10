//! 插件发现与状态：
//! - 自带插件：`resources/plugins/<id>/plugin.json` 编译进二进制，只读，可以停用，默认启用。
//! - 用户插件：`<data_dir>/plugins/<文件夹>/plugin.json`，默认停用；用户在设置里看过权限并启用后才生效，
//!   之后清单的权限变了（比如多了一个主机名）要重新确认。
//! - 校验失败的插件不跳过：以「出错」状态留在列表里，设置页和 `helmp plugins` 都能看到原因。
//! - 每 3 秒检查用户插件目录的文件指纹，变了就整体重载（与脚本插件一致的热重载）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result};

use super::manifest::{self, Manifest};
use super::permissions::Permissions;
use crate::config::Config;
use crate::i18n::{tr, trf};

const POLL_INTERVAL: Duration = Duration::from_secs(3);
/// 指纹最多看这么深、这么多个文件，防止有人把整个仓库链进来拖慢轮询。
const FINGERPRINT_DEPTH: usize = 3;
const FINGERPRINT_FILES: usize = 2000;

/// 自带插件（id, 清单）。新增：放进 resources/plugins/<id>/plugin.json，再在这里加一行。
pub const BUNDLED: &[(&str, &str)] = &[
    (
        "dev.helmp.flomo",
        include_str!("../../../resources/plugins/dev.helmp.flomo/plugin.json"),
    ),
    (
        "dev.helmp.quicklinks",
        include_str!("../../../resources/plugins/dev.helmp.quicklinks/plugin.json"),
    ),
    (
        "dev.helmp.obsidian",
        include_str!("../../../resources/plugins/dev.helmp.obsidian/plugin.json"),
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Bundled,
    User,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Bundled => tr("自带", "Bundled"),
            Source::User => tr("用户插件", "User plugin"),
        }
    }
}

/// 校验通过的插件。
#[derive(Debug)]
pub struct Plugin {
    pub manifest: Manifest,
    /// 插件目录（脚本、png 图标相对它）；自带插件没有目录。
    pub dir: Option<PathBuf>,
    pub permissions: Permissions,
}

impl Plugin {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }
}

/// 列表里的一项：可能加载失败。
#[derive(Debug, Clone)]
pub struct Entry {
    /// 清单里的 id；读不出清单时是文件夹名。
    pub id: String,
    pub source: Source,
    pub dir: Option<PathBuf>,
    pub loaded: Result<Arc<Plugin>, Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Enabled,
    Disabled,
    /// 用户启用过，但清单的权限变了，要重新确认才生效。
    NeedsConsent,
    Broken,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Enabled => tr("已启用", "enabled"),
            State::Disabled => tr("已停用", "disabled"),
            State::NeedsConsent => tr("权限有变化，待确认", "needs permission review"),
            State::Broken => tr("出错", "error"),
        }
    }
}

impl Entry {
    pub fn plugin(&self) -> Option<&Arc<Plugin>> {
        self.loaded.as_ref().ok()
    }

    pub fn state(&self, config: &Config) -> State {
        let Ok(plugin) = &self.loaded else {
            return State::Broken;
        };
        let settings = config.plugins.get(&self.id);
        let enabled = settings.and_then(|s| s.enabled);
        match self.source {
            Source::Bundled => {
                if enabled.unwrap_or(true) {
                    State::Enabled
                } else {
                    State::Disabled
                }
            }
            Source::User => match enabled {
                Some(true) => {
                    let consent = settings.map(|s| s.consent.as_slice()).unwrap_or_default();
                    if consent == plugin.permissions.keys().as_slice() {
                        State::Enabled
                    } else {
                        State::NeedsConsent
                    }
                }
                _ => State::Disabled,
            },
        }
    }
}

/// 启用（同时记下用户确认过的权限）或停用。
pub fn set_enabled(config: &mut Config, entry: &Entry, on: bool) {
    let settings = config.plugins.entry(entry.id.clone()).or_default();
    settings.enabled = Some(on);
    if on && let Some(plugin) = entry.plugin() {
        settings.consent = plugin.permissions.keys();
    }
}

pub fn user_dir() -> Result<PathBuf> {
    let dir = crate::paths::data_dir()?.join("plugins");
    std::fs::create_dir_all(&dir)
        .with_context(|| trf!("创建 {}", "Couldn't create {}", dir.display()))?;
    Ok(dir)
}

pub fn load_bundled() -> Vec<Entry> {
    BUNDLED
        .iter()
        .map(|(id, text)| Entry {
            id: (*id).to_string(),
            source: Source::Bundled,
            dir: None,
            loaded: manifest::parse(text, None).map(|manifest| {
                Arc::new(Plugin {
                    permissions: Permissions::derive(&manifest),
                    manifest,
                    dir: None,
                })
            }),
        })
        .collect()
}

/// 读取并校验一个插件目录（`helmp plugins validate` 也用它）。
pub fn load_dir(dir: &Path, source: Source) -> Entry {
    let folder = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let path = dir.join("plugin.json");
    let loaded = match std::fs::read_to_string(&path) {
        Ok(text) => manifest::parse(&text, Some(dir)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(vec![trf!(
            "{} 里没有 plugin.json",
            "{} has no plugin.json",
            dir.display()
        )]),
        Err(err) => Err(vec![trf!(
            "读取 {} 失败：{err}",
            "Couldn't read {}: {err}",
            path.display()
        )]),
    };
    let id = loaded
        .as_ref()
        .map(|manifest| manifest.id.clone())
        .unwrap_or(folder);
    Entry {
        id,
        source,
        dir: Some(dir.to_path_buf()),
        loaded: loaded.map(|manifest| {
            Arc::new(Plugin {
                permissions: Permissions::derive(&manifest),
                manifest,
                dir: Some(dir.to_path_buf()),
            })
        }),
    }
}

fn user_folders(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// 自带插件 + 用户插件。id 重复的后来者标为出错（不能悄悄覆盖自带插件）。
pub fn load_all() -> Vec<Entry> {
    let mut entries = load_bundled();
    match user_dir() {
        Ok(root) => {
            for dir in user_folders(&root) {
                let mut entry = load_dir(&dir, Source::User);
                if entry.loaded.is_ok() && entries.iter().any(|other| other.id == entry.id) {
                    entry.loaded = Err(vec![trf!(
                        "id {} 已被别的插件使用（自带插件的 id 不能复用）",
                        "id {} is already used by another plugin (bundled plugin ids can't be reused)",
                        entry.id
                    )]);
                }
                entries.push(entry);
            }
        }
        Err(err) => eprintln!("helmp: plugins 插件目录不可用：{err:#}"),
    }
    entries
}

fn store() -> &'static RwLock<Arc<Vec<Entry>>> {
    static STORE: OnceLock<RwLock<Arc<Vec<Entry>>>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(Arc::new(load_all())))
}

/// 当前插件列表（首次调用时加载）。
pub fn current() -> Arc<Vec<Entry>> {
    Arc::clone(
        &store()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// 重新扫描并替换列表（设置页「重新加载」、目录变化）。
pub fn reload() -> Arc<Vec<Entry>> {
    let entries = Arc::new(load_all());
    report(&entries);
    *store()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::clone(&entries);
    entries
}

/// 出错的插件打到 stderr（同时在设置页里显示）。
pub fn report(entries: &[Entry]) {
    let broken = entries.iter().filter(|entry| entry.loaded.is_err()).count();
    for entry in entries {
        if let Err(errors) = &entry.loaded {
            eprintln!("helmp: 插件 {} 无法加载：{}", entry.id, errors.join("；"));
        }
    }
    eprintln!(
        "helmp: plugins 已加载 {} 个插件，出错 {broken} 个",
        entries.len() - broken
    );
}

/// 用户插件目录的文件指纹（路径 → 修改时间与大小）。
fn fingerprint(root: &Path) -> BTreeMap<PathBuf, (Option<SystemTime>, u64)> {
    fn walk(dir: &Path, depth: usize, out: &mut BTreeMap<PathBuf, (Option<SystemTime>, u64)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            if out.len() >= FINGERPRINT_FILES {
                return;
            }
            let path = entry.path();
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                if depth < FINGERPRINT_DEPTH {
                    walk(&path, depth + 1, out);
                }
            } else {
                out.insert(path, (metadata.modified().ok(), metadata.len()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, 0, &mut out);
    out
}

/// 后台轮询用户插件目录，变化时重载。只启动一次。
pub fn watch() -> Result<()> {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return Ok(());
    }
    let root = user_dir()?;
    report(&current());
    let mut seen = fingerprint(&root);
    std::thread::Builder::new()
        .name("helmp-plugins-watch".into())
        .spawn(move || {
            loop {
                std::thread::sleep(POLL_INTERVAL);
                let now = fingerprint(&root);
                if now != seen {
                    seen = now;
                    reload();
                }
            }
        })
        .context(tr(
            "启动插件目录轮询线程",
            "Couldn't start the plugin folder watcher",
        ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_plugins_all_validate() {
        // 自带插件是示例也是功能：清单写坏了，用户一打开设置就看到「出错」。
        for entry in load_bundled() {
            match &entry.loaded {
                Ok(plugin) => assert_eq!(plugin.id(), entry.id),
                Err(errors) => panic!("{}: {errors:#?}", entry.id),
            }
        }
    }

    fn user_entry(dir: &Path) -> Entry {
        load_dir(dir, Source::User)
    }

    #[test]
    fn user_plugins_need_consent_that_matches_current_permissions() {
        // 用户插件默认不生效；启用时确认的权限与当前清单不一致（比如更新后多了一个主机）就要重新确认，
        // 否则插件更新可以在用户不知情的情况下扩大访问范围。
        let dir = std::env::temp_dir().join(format!("helmp-registry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |host: &str| {
            std::fs::write(
                dir.join("plugin.json"),
                format!(
                    r#"{{"schemaVersion":1,"id":"com.example.p","name":"P","version":"1.0.0","author":"A",
                    "commands":[{{"id":"a","title":"A","action":{{"type":"http","method":"GET","url":"https://{host}/"}}}}]}}"#
                ),
            )
            .unwrap();
        };
        write("a.example.com");
        let mut config = Config::default();
        let entry = user_entry(&dir);
        assert_eq!(entry.state(&config), State::Disabled);
        set_enabled(&mut config, &entry, true);
        assert_eq!(entry.state(&config), State::Enabled);
        assert_eq!(
            config.plugins["com.example.p"].consent,
            ["network:a.example.com"]
        );

        write("b.example.com");
        let updated = user_entry(&dir);
        assert_eq!(updated.state(&config), State::NeedsConsent);
        set_enabled(&mut config, &updated, false);
        assert_eq!(updated.state(&config), State::Disabled);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bundled_plugins_default_on_and_can_be_turned_off() {
        let mut config = Config::default();
        let entry = load_bundled().into_iter().next().unwrap();
        assert_eq!(entry.state(&config), State::Enabled);
        set_enabled(&mut config, &entry, false);
        assert_eq!(entry.state(&config), State::Disabled);
    }

    #[test]
    fn fingerprint_changes_when_a_plugin_file_changes() {
        // 热重载靠指纹比较：改了清单或脚本（大小变化、新增文件）必须能察觉，否则用户要重启才生效。
        let root = std::env::temp_dir().join(format!("helmp-registry-fp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("p/bin")).unwrap();
        std::fs::write(root.join("p/plugin.json"), "{}").unwrap();
        let before = fingerprint(&root);
        assert_eq!(fingerprint(&root), before, "没有变化时不能反复重载");
        std::fs::write(root.join("p/plugin.json"), "{ }").unwrap();
        let edited = fingerprint(&root);
        assert_ne!(edited, before);
        std::fs::write(root.join("p/bin/list.sh"), "#!/bin/sh\n").unwrap();
        assert_ne!(fingerprint(&root), edited);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_or_broken_manifests_stay_listed_with_a_reason() {
        // 坏插件不能被悄悄跳过：用户要知道为什么自己的插件没出现。
        let dir =
            std::env::temp_dir().join(format!("helmp-registry-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let entry = user_entry(&dir);
        assert_eq!(entry.state(&Config::default()), State::Broken);
        assert!(entry.loaded.as_ref().unwrap_err()[0].contains("plugin.json"));
        std::fs::write(dir.join("plugin.json"), "{ nope").unwrap();
        let entry = user_entry(&dir);
        assert_eq!(entry.state(&Config::default()), State::Broken);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
