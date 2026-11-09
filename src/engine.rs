//! 汇总所有 provider：启动、查询、范围（Scope）解析、首页内容与结果分组。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::history;
use crate::i18n::{self, tr};
use crate::provider::{Action, Icon, Item, Provider, Scope, Tint};

/// 一次查询最多返回的结果数。剪贴板历史不设上限，但列表只展示前这么多条。
pub const MAX_RESULTS: usize = 200;

/// 首页「最近使用」最多展示几项。
const RECENT_ON_HOME: usize = 6;

/// 首页「最近使用」的分组名。界面对这一组用图标平铺而不是列表（按 id 前缀 `RECENT_ID_PREFIX` 认，
/// 不按分组名认：分组名随语言变）。
pub fn recent_section() -> &'static str {
    tr("最近使用", "Recent")
}

/// 「最近使用」结果的 id 前缀。
pub const RECENT_ID_PREFIX: &str = "recent:";

/// 首页「等你处理」：等待用户的 agent 会话。菜单栏也按这个分组名读取，score 应 ≥ 900。
///
/// 分组名既是显示文字也是分组键：取值随语言变，同一次查询内由 `i18n::pinned` 保证一致，
/// 跨查询比较时要和查询在同一个 `pinned` 里取（见菜单栏）。
pub fn waiting_section() -> &'static str {
    tr("等你处理", "Waiting for You")
}

/// 首页「快捷命令」：各插件的入口命令，score 应 < 800。
pub fn commands_section() -> &'static str {
    tr("快捷命令", "Commands")
}

/// 全局可达的引擎：设置窗口、菜单栏、快捷键查询用。
/// 不要经由启动器实体去拿：在启动器自身更新期间读它会让 GPUI 直接中止进程。
pub struct EngineGlobal(pub Engine);

impl gpui_kit::Global for EngineGlobal {}

#[derive(Clone)]
pub struct Engine {
    providers: Arc<Vec<Arc<dyn Provider>>>,
    /// `start` 已经返回（成功或失败）的 provider。搜索框据此提示「正在建立应用索引…」。
    started: Arc<Mutex<HashSet<&'static str>>>,
}

impl Engine {
    pub fn new(providers: Vec<Arc<dyn Provider>>) -> Self {
        Self {
            providers: Arc::new(providers),
            started: Arc::default(),
        }
    }

    fn mark_started(&self, id: &'static str) {
        self.started
            .lock()
            .expect("engine started set poisoned")
            .insert(id);
    }

    /// 某个 provider 的 `start` 是否已经返回。没有这个 provider（或被停用）时视为已完成。
    pub fn is_started(&self, id: &str) -> bool {
        !self.enabled().any(|provider| provider.id() == id)
            || self
                .started
                .lock()
                .expect("engine started set poisoned")
                .contains(id)
    }

    /// 每个 provider 一个后台线程跑 `start`；失败打印到 stderr，不拖垮其他模块。
    pub fn start_in_background(&self) {
        for provider in self.providers.iter() {
            let provider = Arc::clone(provider);
            let engine = self.clone();
            std::thread::Builder::new()
                .name(format!("helmp-{}", provider.id()))
                .spawn(move || {
                    if let Err(err) = provider.start() {
                        eprintln!("helmp: provider {} 启动失败：{err:#}", provider.id());
                    }
                    engine.mark_started(provider.id());
                })
                .expect("spawn provider thread");
        }
    }

    /// 同步启动全部 provider（CLI 用），返回失败列表。
    pub fn start_blocking(&self) -> Vec<(&'static str, anyhow::Error)> {
        let handles: Vec<_> = self
            .providers
            .iter()
            .cloned()
            .map(|provider| std::thread::spawn(move || (provider.id(), provider.start())))
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| {
                let (id, result) = handle.join().expect("provider start panicked");
                self.mark_started(id);
                result.err().map(|err| (id, err))
            })
            .collect()
    }

    fn enabled(&self) -> impl Iterator<Item = &Arc<dyn Provider>> {
        self.providers
            .iter()
            .filter(|provider| crate::config::extension_enabled(provider.id()))
    }

    /// 全局搜索；空查询返回首页（推荐项 + 最近使用）。
    pub fn query(&self, query: &str) -> Vec<Item> {
        // 整次查询固定一种语言：分组名同时是分组键，不能一半中文一半英文。
        i18n::pinned(|| {
            if query.trim().is_empty() {
                return self.home();
            }
            let items = self
                .enabled()
                .flat_map(|provider| provider.query(query))
                .collect();
            let recent: Vec<String> = history::recent().into_iter().map(|e| e.id).collect();
            group_preferring(items, &recent)
        })
    }

    /// 某个范围内的查询。
    pub fn query_scoped(&self, scope: &str, query: &str, filter: &str) -> Vec<Item> {
        i18n::pinned(|| {
            let items = self
                .enabled()
                .filter(|provider| provider.scope().is_some_and(|s| s.id == scope))
                .flat_map(|provider| provider.query_scoped(query, filter))
                .collect();
            group(items)
        })
    }

    /// 首页：各 provider 的推荐项（等你处理、快捷命令）加最近使用。
    fn home(&self) -> Vec<Item> {
        let mut items: Vec<Item> = self
            .enabled()
            .flat_map(|provider| provider.suggestions())
            .collect();
        for entry in history::recent().into_iter().take(RECENT_ON_HOME) {
            let replayable = !(entry.query.is_empty() && entry.scope.is_none());
            let (title, subtitle) = tile_text(
                &entry,
                history::lang_code(),
                || {
                    let item = self.find(&entry.id, &entry.query, entry.scope.as_deref());
                    let (title, subtitle) = match &item {
                        Some(item) => (item.title.as_str(), item.subtitle.as_str()),
                        None => (entry.title.as_str(), entry.subtitle.as_str()),
                    };
                    history::backfill_text(&entry.id, title, subtitle);
                    item
                },
                replayable,
            );
            let icon = tile_icon(entry.icon(), || {
                // 空查询且不在范围里时重查会回到首页本身，跳过。
                if entry.query.is_empty() && entry.scope.is_none() {
                    return None;
                }
                let icon = self
                    .find(&entry.id, &entry.query, entry.scope.as_deref())?
                    .icon;
                history::backfill_icon(&entry.id, &icon);
                Some(icon)
            });
            let engine = self.clone();
            let (id, query, scope) = (entry.id.clone(), entry.query.clone(), entry.scope.clone());
            items.push(Item {
                id: format!("{RECENT_ID_PREFIX}{}", entry.id),
                title,
                subtitle,
                badge: recent_section().into(),
                // 位于「等你处理」（900+）与「快捷命令」（<800）之间。
                score: 850,
                icon,
                action: Action::new(move || {
                    let item = engine.find(&id, &query, scope.as_deref()).ok_or_else(|| {
                        anyhow::anyhow!(i18n::trf!(
                            "「{id}」已经找不到了",
                            "\"{id}\" is no longer available"
                        ))
                    })?;
                    item.action.run()
                }),
                ..Item::blank()
            });
        }
        group(items)
    }

    /// 按 id 重新查到一个结果（CLI `run`、最近使用重放）。
    pub fn find(&self, id: &str, query: &str, scope: Option<&str>) -> Option<Item> {
        let items = match scope {
            Some(scope) => self.query_scoped(scope, query, ""),
            None => self.query(query),
        };
        items.into_iter().find(|item| item.id == id)
    }

    pub fn scopes(&self) -> Vec<Scope> {
        self.enabled()
            .filter_map(|provider| provider.scope())
            .collect()
    }

    pub fn scope(&self, id: &str) -> Option<Scope> {
        self.scopes().into_iter().find(|scope| scope.id == id)
    }

    fn scope_provider(&self, scope: &str) -> Option<&Arc<dyn Provider>> {
        self.enabled()
            .find(|provider| provider.scope().is_some_and(|s| s.id == scope))
    }

    /// 进入范围之前通知对应插件（见 `Provider::will_enter`）。
    pub fn will_enter(&self, scope: &str, app_in_front: bool) {
        if let Some(provider) = self.scope_provider(scope) {
            i18n::pinned(|| provider.will_enter(app_in_front));
        }
    }

    /// 范围内 ↑ 能调出的历史输入（见 `Provider::recall`）。
    pub fn recall(&self, scope: &str) -> Vec<String> {
        self.scope_provider(scope)
            .map(|provider| provider.recall())
            .unwrap_or_default()
    }

    /// 查询是否以某个范围的前缀开头（如 `cb `），返回范围与去掉前缀后的文本。
    pub fn resolve_prefix(&self, query: &str) -> Option<(Scope, String)> {
        let lower = query.to_lowercase();
        let config = crate::config::get();
        self.enabled()
            .filter_map(|provider| provider.scope())
            .find_map(|scope| {
                // 设置 › 扩展里的别名也能当前缀用：别名 `j` → 输入「j 」进入。
                let alias = self
                    .providers
                    .iter()
                    .find(|p| p.scope().is_some_and(|s| s.id == scope.id))
                    .and_then(|p| config.extensions.get(p.id()))
                    .and_then(|ext| ext.alias.clone())
                    .filter(|alias| !alias.trim().is_empty())
                    .map(|alias| format!("{} ", alias.trim().to_lowercase()));
                let prefix = scope
                    .prefixes
                    .iter()
                    .chain(alias.iter())
                    .find(|prefix| lower.starts_with(prefix.as_str()))?;
                let rest = query.get(prefix.len()..).unwrap_or_default().to_string();
                Some((scope, rest))
            })
    }

    pub fn provider_ids(&self) -> Vec<&'static str> {
        self.providers
            .iter()
            .map(|provider| provider.id())
            .collect()
    }
}

/// 「最近使用」格子的图标：记录里存的优先；旧记录没存就按原结果现查一次（查到后写回历史）；
/// 都没有时用通用图标，格子里不能是一块空白。
/// 「最近使用」格子的标题与副标题。记录是另一种界面语言写下的（切换过语言，或旧记录没记语言）时，
/// 按当前语言把原结果重新查一次；查不到或不能重放就用记录里的。
fn tile_text(
    entry: &history::Entry,
    current: &str,
    live: impl FnOnce() -> Option<Item>,
    replayable: bool,
) -> (String, String) {
    if entry.lang.as_deref() != Some(current)
        && replayable
        && let Some(item) = live()
    {
        return (item.title, item.subtitle);
    }
    (entry.title.clone(), entry.subtitle.clone())
}

fn tile_icon(stored: Icon, live: impl FnOnce() -> Option<Icon>) -> Icon {
    if stored != Icon::None {
        return stored;
    }
    match live() {
        Some(icon) if icon != Icon::None => icon,
        _ => Icon::Symbol {
            name: "rotate-ccw",
            tint: Tint::Graphite,
        },
    }
}

/// 按分数排序后按分组聚拢：分组的先后由组内最高分决定，组内按分数、再按标题。
fn group(items: Vec<Item>) -> Vec<Item> {
    group_preferring(items, &[])
}

/// 同 [`group`]，但分数相同的结果里，最近用过的排前（`recent` 新的在前）。
/// 只在同一档内调整：字面前缀永远压过拼音模糊之类的档位规则不变，
/// 但打 `term` 时常用的「终端」会排在没用过的 Termius 前面。
fn group_preferring(mut items: Vec<Item>, recent: &[String]) -> Vec<Item> {
    let used = |item: &Item| {
        recent
            .iter()
            .position(|id| id == &item.id)
            .unwrap_or(usize::MAX)
    };
    items.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| used(a).cmp(&used(b)))
            .then_with(|| a.title.cmp(&b.title))
    });
    let mut order: Vec<String> = Vec::new();
    for item in &items {
        if !order.contains(&item.badge) {
            order.push(item.badge.clone());
        }
    }
    // 稳定排序：组内保持上面的分数顺序。
    items.sort_by_key(|item| order.iter().position(|badge| badge == &item.badge));
    items.truncate(MAX_RESULTS);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, badge: &str, score: u32) -> Item {
        Item {
            title: title.into(),
            badge: badge.into(),
            score,
            ..Item::blank()
        }
    }

    #[test]
    fn recent_tiles_follow_the_interface_language() {
        // 「最近使用」存的是执行时的标题：中文界面下用过「锁屏」，切到英文后首页还写着「锁屏 /
        // 锁定屏幕，回来时需要密码」，就是英文界面里漏翻的中文。语言不同时按当前语言重取。
        let entry = |lang: Option<&str>| history::Entry {
            id: "system:lock".into(),
            title: "锁屏".into(),
            subtitle: "锁定屏幕，回来时需要密码".into(),
            badge: "系统命令".into(),
            icon: None,
            icon_is_file: false,
            symbol: None,
            query: "lock".into(),
            scope: None,
            at_ms: 0,
            lang: lang.map(Into::into),
        };
        let live = || {
            let mut item = item("Lock Screen", "System", 900);
            item.subtitle = "Lock the screen".into();
            Some(item)
        };
        let english = ("Lock Screen".to_string(), "Lock the screen".to_string());
        assert_eq!(tile_text(&entry(Some("zh")), "en", live, true), english);
        // 旧记录没有语言字段，也按当前语言重取。
        assert_eq!(tile_text(&entry(None), "en", live, true), english);
        // 语言没变就用记录里的，不重查。
        let stored = ("锁屏".to_string(), "锁定屏幕，回来时需要密码".to_string());
        assert_eq!(
            tile_text(&entry(Some("zh")), "zh", || panic!("不该重查"), true),
            stored
        );
        // 查不到（应用被卸载）或不能重放时，退回记录里的标题。
        assert_eq!(tile_text(&entry(Some("zh")), "en", || None, true), stored);
        assert_eq!(
            tile_text(&entry(Some("zh")), "en", || panic!("不能重放"), false),
            stored
        );
    }

    #[test]
    fn recent_tiles_never_render_blank() {
        // 首页「最近使用」是图标平铺：升级前的旧记录没存图标，格子里会是一块空白。
        let file = Icon::File("/Applications/WeChat.app".into());
        let symbol = Icon::Symbol {
            name: "languages",
            tint: Tint::Teal,
        };
        // 存了图标就不再现查（现查要跑一次完整查询）。
        assert_eq!(tile_icon(file.clone(), || panic!("不该现查")), file);
        // 旧记录：按原结果现查到的图标补上。
        assert_eq!(tile_icon(Icon::None, || Some(symbol.clone())), symbol);
        // 原结果已经找不到，或者它本身就没图标：用通用图标，不能是空白。
        for live in [None, Some(Icon::None)] {
            assert!(matches!(
                tile_icon(Icon::None, || live),
                Icon::Symbol { .. }
            ));
        }
    }

    #[test]
    fn fallback_tile_icon_exists_in_bundled_assets() {
        use gpui_kit::AssetSource as _;
        let Icon::Symbol { name, .. } = tile_icon(Icon::None, || None) else {
            unreachable!()
        };
        let path = format!("icons/{name}.svg");
        assert!(gpui_kit::assets::AllAssets.load(&path).unwrap().is_some());
    }

    #[test]
    fn home_sections_follow_the_interface_language() {
        // 首页小节标题就是分组名：英文界面不能冒出中文标题；三个分组名也不能相同，否则会被并成一组。
        use crate::i18n::{Lang, with_lang};
        let names = || [recent_section(), waiting_section(), commands_section()];
        assert_eq!(
            with_lang(Lang::Zh, names),
            ["最近使用", "等你处理", "快捷命令"]
        );
        let en = with_lang(Lang::En, names);
        assert_eq!(en, ["Recent", "Waiting for You", "Commands"]);
        assert!(en[0] != en[1] && en[1] != en[2] && en[0] != en[2]);
    }

    struct Slow;

    impl Provider for Slow {
        fn id(&self) -> &'static str {
            "slow-index"
        }
        fn query(&self, _: &str) -> Vec<Item> {
            Vec::new()
        }
    }

    #[test]
    fn reports_when_a_provider_has_started() {
        // 搜索框据此提示「正在建立应用索引…」：start 返回前必须报未完成，返回后（哪怕失败）必须报完成，
        // 否则提示要么从不出现，要么永远不消失。
        let engine = Engine::new(vec![Arc::new(Slow)]);
        assert!(!engine.is_started("slow-index"));
        assert!(engine.start_blocking().is_empty());
        assert!(engine.is_started("slow-index"));
        // 没有这个插件（如被停用）时不能一直显示「正在索引」。
        assert!(engine.is_started("not-registered"));
    }

    #[test]
    fn recently_used_wins_ties_but_never_beats_a_better_match() {
        // 「常用的排前」：同样是前缀命中，用户天天开的「终端」不该因为字母序排在没用过的 Termius 后面；
        // 但最近用过不能压过更好的匹配，否则打 wx 会先出微信开发者工具。
        let mut termius = item("Termius", "应用", 900);
        termius.id = "apps:termius".into();
        let mut terminal = item("终端", "应用", 900);
        terminal.id = "apps:terminal".into();
        let mut devtools = item("微信开发者工具", "应用", 750);
        devtools.id = "apps:devtools".into();
        let recent = ["apps:devtools".to_string(), "apps:terminal".to_string()];
        let grouped = group_preferring(vec![termius, devtools, terminal], &recent);
        let titles: Vec<_> = grouped.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["终端", "Termius", "微信开发者工具"]);
        // 没有使用记录时仍按标题排，结果稳定。
        let plain = group_preferring(vec![item("b", "应用", 900), item("a", "应用", 900)], &[]);
        let titles: Vec<_> = plain.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["a", "b"]);
    }

    #[test]
    fn groups_sections_by_their_best_item() {
        // 列表按小节显示：同一组必须连在一起，否则「应用」标题会出现两次；
        // 组的先后由最好的那条决定，保证第一条仍是全局最佳匹配。
        let grouped = group(vec![
            item("微信", "应用", 900),
            item("修复 wx 登录", "Agent 会话", 700),
            item("微信开发者工具", "应用", 600),
            item("wx_appid", "剪贴板", 650),
        ]);
        let titles: Vec<_> = grouped.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(
            titles,
            ["微信", "微信开发者工具", "修复 wx 登录", "wx_appid"]
        );
    }
}
