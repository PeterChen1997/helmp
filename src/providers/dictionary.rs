//! 词典：`define apple`、`dict apple`、`查 苹果`（范围前缀 `d ` / `dict ` / `define ` / `词典 ` / `查 `），
//! 用系统词典离线查词（CoreServices 的 `DCSCopyTextDefinition`），释义放在详情里。
//!
//! - 用的是用户在「词典.app」里勾选的词典；系统词典要第一次打开「词典.app」后才会下载，
//!   没下载或查不到时明确说「没有找到释义」，并给出「在词典中打开」。
//! - 回车复制释义，⌘↵ 在词典.app 中打开（`dict://<词>`），⌘T 送去翻译。
//! - 查词是本地调用，毫秒级；同一个词的结果缓存起来，查询时不重复调用。

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;

use objc2::rc::Retained;
use objc2_foundation::NSString;

use crate::i18n::{tr, trf};
use crate::matcher;
use crate::provider::{
    Action, Block, Detail, Icon, Item, NamedAction, Outcome, Provider, Scope, Tint,
};

use super::scripts::args::percent_encode;

const SCOPE: &str = "dictionary";
const ICON: Icon = Icon::Symbol {
    name: "book-a",
    tint: Tint::Red,
};
/// 缓存上限：超过就清空重来（查词是低频操作，不值得做 LRU）。
const CACHE_LIMIT: usize = 256;

#[repr(C)]
struct CFRange {
    location: isize,
    length: isize,
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    /// 返回 +1 引用的 CFStringRef，查不到时为 NULL。`dictionary` 传 NULL 表示用户启用的全部词典。
    fn DCSCopyTextDefinition(
        dictionary: *const c_void,
        text: *const c_void,
        range: CFRange,
    ) -> *const c_void;
}

/// 用系统词典查 `word`；查不到（或系统词典还没下载）返回 None。
pub fn define(word: &str) -> Option<String> {
    let word = word.trim();
    if word.is_empty() {
        return None;
    }
    objc2::rc::autoreleasepool(|_| {
        let text = NSString::from_str(word);
        let length = isize::try_from(word.encode_utf16().count()).ok()?;
        let text_ptr: *const NSString = Retained::as_ptr(&text);
        // SAFETY: NSString 与 CFString 免费桥接；range 覆盖整个字符串（UTF-16 单位）；
        // 返回值按 Copy 规则是 +1 引用，交给 Retained 负责释放。
        let result = unsafe {
            DCSCopyTextDefinition(
                std::ptr::null(),
                text_ptr.cast(),
                CFRange {
                    location: 0,
                    length,
                },
            )
        };
        if result.is_null() {
            return None;
        }
        // SAFETY: 非空的 CFStringRef 即 NSString（免费桥接），所有权（+1）转交给 Retained。
        let definition = unsafe { Retained::from_raw(result.cast_mut().cast::<NSString>()) }?;
        let text = definition.to_string();
        (!text.trim().is_empty()).then_some(text)
    })
}

type Lookup = fn(&str) -> Option<String>;

pub struct Dictionary {
    lookup: Lookup,
    cache: Mutex<HashMap<String, Option<String>>>,
}

impl Dictionary {
    pub fn new() -> Self {
        Self {
            lookup: define,
            cache: Mutex::default(),
        }
    }

    fn definition(&self, word: &str) -> Option<String> {
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(hit) = cache.get(word) {
            return hit.clone();
        }
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        let result = (self.lookup)(word);
        cache.insert(word.to_string(), result.clone());
        result
    }
}

fn badge() -> &'static str {
    tr("词典", "Dictionary")
}

impl Provider for Dictionary {
    fn id(&self) -> &'static str {
        "dictionary"
    }

    /// 根搜索只给一个入口，不在每次按键时查词。
    fn query(&self, query: &str) -> Vec<Item> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        ["词典", "查词", "字典", "dictionary", "define", "dict"]
            .iter()
            .filter_map(|keyword| matcher::score(query, keyword))
            .max()
            .map(|score| {
                vec![Item {
                    id: "dictionary:open".into(),
                    title: tr("词典", "Dictionary").into(),
                    subtitle: tr(
                        "用系统词典离线查词，如「define apple」",
                        "Look up words offline, e.g. “define apple”",
                    )
                    .into(),
                    badge: crate::engine::commands_section().into(),
                    score,
                    icon: ICON,
                    action: Action::enter_scope(SCOPE),
                    action_title: tr("进入", "Open").into(),
                    ..Item::blank()
                }]
            })
            .unwrap_or_default()
    }

    fn scope(&self) -> Option<Scope> {
        Some(Scope {
            id: SCOPE.into(),
            label: tr("词典", "Dictionary").into(),
            tint: Tint::Red,
            prefixes: ["define ", "dict ", "d ", "词典 ", "查 "]
                .map(str::to_string)
                .to_vec(),
            placeholder: tr("输入要查的单词", "Type a word to look up").into(),
            filters: Vec::new(),
        })
    }

    fn query_scoped(&self, query: &str, _filter: &str) -> Vec<Item> {
        let word = query.trim();
        if word.is_empty() {
            return Vec::new();
        }
        let item = match self.definition(word) {
            Some(definition) => found(word, definition),
            None => not_found(word),
        };
        vec![item]
    }
}

fn dict_url(word: &str) -> String {
    format!("dict://{}", percent_encode(word))
}

fn send_to_translate(word: &str) -> NamedAction {
    let word = word.to_string();
    NamedAction::new(
        tr("送去翻译", "Translate"),
        Some("⌘T"),
        Action::new(move || Ok(Outcome::EnterScopeWith("translate".into(), word.clone()))),
    )
}

/// 系统词典的纯文本把所有义项挤在一行：在义项编号、「•」和 PHRASES 这类小标题前断行，详情里好读一些。
/// 复制出去的仍是原文。
fn format_definition(text: &str) -> String {
    let mut out = text.replace(" • ", "\n• ");
    for n in 2..=30 {
        out = out.replace(&format!(". {n} "), &format!(".\n{n} "));
    }
    for heading in ["PHRASAL VERBS", "PHRASES", "DERIVATIVES", "ORIGIN", "USAGE"] {
        out = out.replace(&format!(" {heading} "), &format!("\n\n{heading}\n"));
    }
    out
}

fn found(word: &str, definition: String) -> Item {
    let summary: String = definition.chars().take(120).collect();
    Item {
        id: format!("dictionary:{word}"),
        title: word.to_string(),
        subtitle: summary,
        badge: badge().into(),
        score: 1000,
        icon: ICON,
        action: Action::copy(definition.clone()),
        action_title: tr("复制释义", "Copy Definition").into(),
        actions: vec![
            NamedAction::new(
                tr("在词典中打开", "Open in Dictionary"),
                Some("⌘↵"),
                Action::open(dict_url(word)),
            ),
            NamedAction::new(tr("复制单词", "Copy Word"), Some("⌘C"), Action::copy(word)),
            send_to_translate(word),
        ],
        detail: Some(Detail {
            icon: ICON,
            title: word.to_string(),
            tag: None,
            blocks: vec![
                Block::Result {
                    text: format_definition(&definition),
                    large: false,
                },
                Block::Caption(tr("macOS 词典 · 离线", "macOS Dictionary · offline").into()),
            ],
            metadata: Vec::new(),
            actions: Vec::new(),
        }),
        ..Item::blank()
    }
}

fn not_found(word: &str) -> Item {
    Item {
        id: format!("dictionary:none:{word}"),
        title: trf!("没有找到“{word}”的释义", "No definition for “{word}”"),
        subtitle: tr(
            "系统词典里没有这个词，或词典还没下载：回车在「词典」App 中查（首次打开会下载词典）",
            "Not in the system dictionaries, or they aren’t downloaded yet. Press Return to look it up in the Dictionary app (it downloads them on first launch).",
        )
        .into(),
        badge: badge().into(),
        score: 1000,
        icon: ICON,
        action: Action::open(dict_url(word)),
        action_title: tr("在词典中打开", "Open in Dictionary").into(),
        actions: vec![send_to_translate(word)],
        ..Item::blank()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(word: &str) -> Option<String> {
        (word == "apple").then(|| "apple | ˈap(ə)l | noun the round fruit of a tree".to_string())
    }

    fn provider() -> Dictionary {
        Dictionary {
            lookup: fake,
            cache: Mutex::default(),
        }
    }

    #[test]
    fn shows_the_definition_in_the_detail_and_copies_it() {
        // 查到时释义在详情里，回车复制的是释义本身。
        let items = provider().query_scoped("apple", "");
        let item = &items[0];
        assert_eq!(item.id, "dictionary:apple");
        let detail = item.detail.as_ref().unwrap();
        assert!(
            matches!(&detail.blocks[0], Block::Result { text, .. } if text.contains("round fruit"))
        );
        assert_eq!(item.action_title, "复制释义");
        let titles: Vec<_> = item.actions.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, ["在词典中打开", "复制单词", "送去翻译"]);
        assert_eq!(
            item.actions[2].action.run().unwrap(),
            Outcome::EnterScopeWith("translate".into(), "apple".into())
        );
    }

    #[test]
    fn says_so_when_nothing_is_found() {
        // 查不到不能给空白：要说清楚，并给出去「词典」App 查的路。
        let items = provider().query_scoped("qwzx", "");
        assert_eq!(items[0].id, "dictionary:none:qwzx");
        assert!(items[0].title.contains("没有找到"));
        assert!(provider().query_scoped("  ", "").is_empty());
    }

    #[test]
    fn long_definitions_are_broken_into_senses() {
        // 原文是一整行，几十个义项挤在一起没法读；断行只发生在义项、要点和小标题前。
        let text = "apple | ˈap(ə)l | noun 1 the round fruit. • an unrelated fruit. 2 the tree. PHRASES the apple never falls";
        assert_eq!(
            format_definition(text),
            "apple | ˈap(ə)l | noun 1 the round fruit.\n• an unrelated fruit.\n2 the tree.\n\nPHRASES\nthe apple never falls"
        );
        assert_eq!(
            format_definition("苹果 píngguǒ 名 落叶乔木。"),
            "苹果 píngguǒ 名 落叶乔木。"
        );
    }

    #[test]
    fn dict_url_is_encoded() {
        assert_eq!(dict_url("ice cream"), "dict://ice%20cream");
    }

    #[test]
    fn root_search_only_offers_an_entry() {
        // 根搜索每次按键都会调用，不能每次都查词；只给「词典」入口。
        let items = provider().query("cd");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "dictionary:open");
        assert!(provider().query("apple").is_empty());
    }

    #[test]
    fn system_lookup_is_safe_for_unknown_words() {
        // 真实的系统调用：查不到时返回 None 而不是崩溃（词典没下载时所有词都查不到）。
        assert_eq!(define(""), None);
        assert_eq!(define("qwzxvbnmqwzx"), None);
    }

    #[test]
    fn icon_exists_in_bundled_assets() {
        use gpui_kit::AssetSource as _;
        assert!(
            gpui_kit::assets::AllAssets
                .load("icons/book-a.svg")
                .unwrap()
                .is_some()
        );
    }
}
