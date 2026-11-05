//! 汇总所有 provider：启动、并发查询结果合并排序。

use std::sync::Arc;

use crate::provider::{Item, Provider};

/// 一次查询最多返回的结果数。剪贴板历史不设上限，但列表只展示前这么多条。
pub const MAX_RESULTS: usize = 200;

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
                .name(format!("huan-{}", provider.id()))
                .spawn(move || {
                    if let Err(err) = provider.start() {
                        eprintln!("huan: provider {} 启动失败：{err:#}", provider.id());
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

    pub fn query(&self, query: &str) -> Vec<Item> {
        let mut items: Vec<Item> = self
            .providers
            .iter()
            .flat_map(|provider| provider.query(query))
            .collect();
        // 分数相同按标题，保证同一查询的顺序稳定。
        items.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.title.cmp(&b.title)));
        items.truncate(MAX_RESULTS);
        items
    }

    pub fn provider_ids(&self) -> Vec<&'static str> {
        self.providers
            .iter()
            .map(|provider| provider.id())
            .collect()
    }
}
