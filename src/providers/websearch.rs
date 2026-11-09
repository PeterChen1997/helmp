//! 网页搜索兜底（HapiGo 的「默认搜索」）：查询非空时给出「在 Google 搜索 “xxx”」等项。
//!
//! - 引擎列表、顺序与开关在 `config.web_search`（设置 › 扩展 › 网页搜索），默认启用 Google、百度、GitHub。
//! - 网址模板里的 `{query}` 换成 URL 编码后的查询；回车用系统默认浏览器打开（`open`）。
//! - 分数低于所有本地结果：有本地结果时排在末尾不抢位置，没有时自然排在最前。

use crate::config::{self, WebSearchEngine};
use crate::i18n::{tr, trf};
use crate::provider::{Action, Icon, Item, NamedAction, Provider, Tint};

use super::scripts::args::percent_encode;

/// 最低的本地结果是剪贴板顺带给的几条（248 起），兜底项必须比它们还低。
const BASE_SCORE: u32 = 100;

fn badge() -> &'static str {
    tr("网页搜索", "Web Search")
}

type Engines = Box<dyn Fn() -> Vec<WebSearchEngine> + Send + Sync>;

pub struct WebSearch {
    engines: Engines,
}

impl WebSearch {
    pub fn new() -> Self {
        Self {
            engines: Box::new(|| config::get().web_search.engines()),
        }
    }
}

impl Provider for WebSearch {
    fn id(&self) -> &'static str {
        "websearch"
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        (self.engines)()
            .iter()
            .filter(|engine| engine.enabled && !engine.url.trim().is_empty())
            .enumerate()
            .map(|(rank, engine)| item(engine, query, rank))
            .collect()
    }
}

fn item(engine: &WebSearchEngine, query: &str, rank: usize) -> Item {
    let name = engine_name(engine);
    let url = expand(&engine.url, query);
    let (icon, tint) = engine_icon(&engine.id);
    Item {
        id: format!("websearch:{}", engine.id),
        title: trf!("在 {name} 搜索“{query}”", "Search {name} for “{query}”"),
        subtitle: url.clone(),
        badge: badge().into(),
        // 按设置里的顺序递减；同分时引擎会按标题排序，打乱用户定的顺序。
        score: BASE_SCORE.saturating_sub(u32::try_from(rank).unwrap_or(BASE_SCORE)),
        icon: Icon::Symbol { name: icon, tint },
        action: Action::open(url.clone()),
        action_title: tr("在浏览器中打开", "Open in Browser").into(),
        actions: vec![NamedAction::new(
            tr("复制链接", "Copy Link"),
            Some("⌘⇧C"),
            Action::copy(url),
        )],
        ..Item::blank()
    }
}

/// 把模板里的每个 `{query}` 换成 URL 编码后的查询。
pub fn expand(template: &str, query: &str) -> String {
    template.trim().replace("{query}", &percent_encode(query))
}

/// 显示名：自定义引擎用配置里的名字；内置引擎按界面语言取（不存进配置，切语言才能跟着变）。
pub fn engine_name(engine: &WebSearchEngine) -> String {
    if !engine.name.trim().is_empty() {
        return engine.name.trim().to_string();
    }
    match engine.id.as_str() {
        "google" => "Google",
        "baidu" => tr("百度", "Baidu"),
        "bing" => "Bing",
        "github" => "GitHub",
        "bing-translate" => tr("必应翻译", "Bing Translator"),
        other => other,
    }
    .to_string()
}

/// Lucide 图标名与底块颜色。
pub fn engine_icon(id: &str) -> (&'static str, Tint) {
    match id {
        "github" => ("github", Tint::Graphite),
        "bing-translate" => ("languages", Tint::Teal),
        "google" | "baidu" | "bing" => ("search", Tint::Blue),
        _ => ("globe", Tint::Gray),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::config::WebSearchConfig;
    use crate::engine::Engine;

    fn defaults() -> WebSearch {
        WebSearch {
            engines: Box::new(|| WebSearchConfig::default().engines()),
        }
    }

    fn ids(items: &[Item]) -> Vec<&str> {
        items.iter().map(|item| item.id.as_str()).collect()
    }

    /// 一个只返回一条低分本地结果的插件，模拟「本地有命中但匹配得不太好」。
    struct Local(u32);

    impl Provider for Local {
        fn id(&self) -> &'static str {
            "test-local"
        }

        fn query(&self, _query: &str) -> Vec<Item> {
            vec![Item {
                id: "test-local:hit".into(),
                title: "zzz 本地结果".into(),
                badge: "应用".into(),
                score: self.0,
                ..Item::blank()
            }]
        }
    }

    #[test]
    fn defaults_to_google_baidu_github_in_order() {
        // 默认只开这三个，且顺序就是结果里的顺序。
        let items = defaults().query("rust gpui");
        assert_eq!(
            ids(&items),
            ["websearch:google", "websearch:baidu", "websearch:github"]
        );
        assert_eq!(items[0].title, "在 Google 搜索“rust gpui”");
        assert!(items[0].score > items[1].score && items[1].score > items[2].score);
    }

    #[test]
    fn nothing_for_an_empty_query() {
        // 首页（空查询）不能出现「在 Google 搜索 “”」。
        assert!(defaults().query("   ").is_empty());
    }

    #[test]
    fn queries_are_url_encoded() {
        // 不编码的话 `c++`、`a&b`、中文会在网址里变成别的意思（+ 是空格、& 截断参数）。
        assert_eq!(
            expand("https://www.google.com/search?q={query}", "c++ a&b 微信"),
            "https://www.google.com/search?q=c%2B%2B%20a%26b%20%E5%BE%AE%E4%BF%A1"
        );
        let item = &defaults().query("c++")[0];
        assert_eq!(item.subtitle, "https://www.google.com/search?q=c%2B%2B");
    }

    #[test]
    fn order_and_switches_come_from_config() {
        // 设置里关掉的不出现；用户把 GitHub 调到最前，结果里它就在最前。
        let mut engines = WebSearchConfig::default().engines();
        engines.retain(|engine| engine.id != "google");
        engines.rotate_right(1);
        for engine in &mut engines {
            engine.enabled = true;
        }
        engines.push(WebSearchEngine {
            id: "zhihu".into(),
            name: "知乎".into(),
            url: "https://www.zhihu.com/search?q={query}".into(),
            builtin: false,
            enabled: true,
        });
        let custom = WebSearch {
            engines: Box::new(move || engines.clone()),
        };
        let items = custom.query("wx");
        assert_eq!(
            ids(&items),
            [
                "websearch:bing-translate",
                "websearch:baidu",
                "websearch:github",
                "websearch:bing",
                "websearch:zhihu"
            ]
        );
        assert_eq!(items[4].title, "在 知乎 搜索“wx”");
        assert_eq!(
            items[4].icon,
            Icon::Symbol {
                name: "globe",
                tint: Tint::Gray
            }
        );
    }

    #[test]
    fn never_outranks_local_results_but_leads_when_alone() {
        // 有本地结果时网页搜索排在末尾，不能抢第一；本地什么都没有时它们就是第一批结果。
        // 248 是剪贴板顺带结果的最低分，本地结果不会更低。
        let engine = Engine::new(vec![Arc::new(Local(248)), Arc::new(defaults())]);
        let items = engine.query("zzz");
        assert_eq!(items[0].id, "test-local:hit");
        assert_eq!(items.last().unwrap().id, "websearch:github");

        let alone = Engine::new(vec![Arc::new(defaults())]);
        assert_eq!(alone.query("zzz")[0].id, "websearch:google");
    }

    #[test]
    fn builtin_names_follow_the_interface_language() {
        // 内置引擎名不存进配置，否则切到英文后仍显示「百度」。
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let items = defaults().query("wx");
            assert_eq!(items[1].title, "Search Baidu for “wx”");
            assert_eq!(items[1].badge, "Web Search");
        });
    }

    #[test]
    fn symbol_icons_exist_in_bundled_assets() {
        // 图标名拼错时界面只会画一个空白方块，不会报错。
        use gpui_kit::AssetSource as _;
        for id in [
            "google",
            "baidu",
            "bing",
            "github",
            "bing-translate",
            "custom",
        ] {
            let (name, _) = engine_icon(id);
            let path = format!("icons/{name}.svg");
            assert!(gpui_kit::assets::AllAssets.load(&path).unwrap().is_some());
        }
    }
}
