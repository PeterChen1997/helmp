//! 汇总所有 provider：启动、查询、范围（Scope）解析、首页内容与结果分组。

use std::sync::Arc;

use crate::history;
use crate::provider::{Action, Item, Provider, Scope};

/// 一次查询最多返回的结果数。剪贴板历史不设上限，但列表只展示前这么多条。
pub const MAX_RESULTS: usize = 200;

/// 首页「最近使用」最多展示几项。
const RECENT_ON_HOME: usize = 6;

/// 首页「最近使用」的分组名。界面对这一组用图标平铺而不是列表。
pub const RECENT_SECTION: &str = "最近使用";

/// 首页「等你处理」：等待用户的 agent 会话。菜单栏也按这个分组名读取，score 应 ≥ 900。
pub const WAITING_SECTION: &str = "等你处理";

/// 首页「快捷命令」：各插件的入口命令，score 应 < 800。
pub const COMMANDS_SECTION: &str = "快捷命令";

/// 全局可达的引擎：设置窗口、菜单栏、快捷键查询用。
/// 不要经由启动器实体去拿：在启动器自身更新期间读它会让 GPUI 直接中止进程。
pub struct EngineGlobal(pub Engine);

impl gpui_kit::Global for EngineGlobal {}

#[derive(Clone)]
pub struct Engine {
    providers: Arc<Vec<Arc<dyn Provider>>>,
}

impl Engine {
    pub fn new(providers: Vec<Arc<dyn Provider>>) -> Self {
        Self {
            providers: Arc::new(providers),
        }
    }

    /// 每个 provider 一个后台线程跑 `start`；失败打印到 stderr，不拖垮其他模块。
    pub fn start_in_background(&self) {
        for provider in self.providers.iter() {
            let provider = Arc::clone(provider);
            std::thread::Builder::new()
                .name(format!("helmp-{}", provider.id()))
                .spawn(move || {
                    if let Err(err) = provider.start() {
                        eprintln!("helmp: provider {} 启动失败：{err:#}", provider.id());
                    }
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
            .filter_map(
                |handle| match handle.join().expect("provider start panicked") {
                    (id, Err(err)) => Some((id, err)),
                    (_, Ok(())) => None,
                },
            )
            .collect()
    }

    fn enabled(&self) -> impl Iterator<Item = &Arc<dyn Provider>> {
        self.providers
            .iter()
            .filter(|provider| crate::config::extension_enabled(provider.id()))
    }

    /// 全局搜索；空查询返回首页（推荐项 + 最近使用）。
    pub fn query(&self, query: &str) -> Vec<Item> {
        if query.trim().is_empty() {
            return self.home();
        }
        let items = self
            .enabled()
            .flat_map(|provider| provider.query(query))
            .collect();
        group(items)
    }

    /// 某个范围内的查询。
    pub fn query_scoped(&self, scope: &str, query: &str, filter: &str) -> Vec<Item> {
        let items = self
            .enabled()
            .filter(|provider| provider.scope().is_some_and(|s| s.id == scope))
            .flat_map(|provider| provider.query_scoped(query, filter))
            .collect();
        group(items)
    }

    /// 首页：各 provider 的推荐项（等你处理、快捷命令）加最近使用。
    fn home(&self) -> Vec<Item> {
        let mut items: Vec<Item> = self
            .enabled()
            .flat_map(|provider| provider.suggestions())
            .collect();
        for entry in history::recent().into_iter().take(RECENT_ON_HOME) {
            let engine = self.clone();
            let (id, query, scope) = (entry.id.clone(), entry.query.clone(), entry.scope.clone());
            items.push(Item {
                id: format!("recent:{}", entry.id),
                title: entry.title.clone(),
                subtitle: entry.subtitle.clone(),
                badge: RECENT_SECTION.into(),
                // 位于「等你处理」（900+）与「快捷命令」（<800）之间。
                score: 850,
                icon: entry.icon(),
                action: Action::new(move || {
                    let item = engine
                        .find(&id, &query, scope.as_deref())
                        .ok_or_else(|| anyhow::anyhow!("「{id}」已经找不到了"))?;
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

/// 按分数排序后按分组聚拢：分组的先后由组内最高分决定，组内按分数、再按标题。
fn group(mut items: Vec<Item>) -> Vec<Item> {
    items.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.title.cmp(&b.title)));
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
