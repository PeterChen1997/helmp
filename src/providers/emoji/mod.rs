//! Emoji 与符号：按中文、英文名称和关键词搜索 emoji 和常用符号（箭头、数学、货币、⌘⌥⇧…）。
//!
//! - 数据是编译期嵌入的 CLDR 注释表（`data.tsv`，由 `script/gen-emoji.py` 生成，Unicode License v3，
//!   署名见 docs/THIRD_PARTY.md）。中文名同样能用拼音全拼和首字母搜（`xiao` → 笑）。
//! - 范围「Emoji」：前缀 `emoji ` / `表情 ` / `符号 `；空查询时最近用过的排最前。
//!   根搜索里以 `:` 开头（`:xiao`，Slack 习惯）只搜 emoji；其他查询最多混入 3 条名称命中、排在本地结果后面。
//!   `:` 后面是数字时不算（留给端口查询 `:3000`）。
//! - 回车粘贴到前台 App（先复制再模拟 ⌘V），⌘C 只复制；⌘K 里可以复制码点、选肤色。

use std::sync::{Mutex, OnceLock};

use crate::i18n::{tr, trf};
use crate::matcher::{self, Candidate};
use crate::provider::{Action, Icon, Item, NamedAction, Outcome, Provider, Scope, Tint};

const PROVIDER_ID: &str = "emoji";
const DATA: &str = include_str!("data.tsv");
/// 范围内一次最多列这么多条（空查询时整张表太长，列表也没法看）。
const SCOPE_LIMIT: usize = 200;
/// 根搜索里最多混入几条。
const ROOT_LIMIT: usize = 3;
/// 最近使用最多记几个。
const RECENT_LIMIT: usize = 24;
/// 关键词命中比名称命中低这么多：「笑」先出名字就叫「大笑」的，再出关键词里有「笑」的。
const KEYWORD_PENALTY: u32 = 60;

/// 肤色修饰符：(修饰符, 中文名, 英文名)。
const SKIN_TONES: [(&str, &str, &str); 5] = [
    ("\u{1F3FB}", "浅肤色", "light skin tone"),
    ("\u{1F3FC}", "中等浅肤色", "medium-light skin tone"),
    ("\u{1F3FD}", "中等肤色", "medium skin tone"),
    ("\u{1F3FE}", "中等深肤色", "medium-dark skin tone"),
    ("\u{1F3FF}", "深肤色", "dark skin tone"),
];

struct Entry {
    text: &'static str,
    symbol: bool,
    skin: bool,
    en_name: &'static str,
    zh_name: &'static str,
    en_keywords: Vec<&'static str>,
    zh_keywords: Vec<&'static str>,
    names: [Candidate; 2],
    keywords: Vec<Candidate>,
}

impl Entry {
    fn parse(line: &'static str) -> Option<Self> {
        let mut cols = line.split('\t');
        let text = cols.next()?;
        let flags = cols.next()?;
        let en_name = cols.next()?;
        let en_keywords: Vec<&str> = cols.next()?.split('|').filter(|k| !k.is_empty()).collect();
        let zh_name = cols.next()?;
        let zh_keywords: Vec<&str> = cols.next()?.split('|').filter(|k| !k.is_empty()).collect();
        let keywords = en_keywords
            .iter()
            .chain(zh_keywords.iter())
            .map(|k| Candidate::new(k))
            .collect();
        Some(Self {
            text,
            symbol: flags.contains('s'),
            skin: flags.contains('t'),
            en_name,
            zh_name,
            names: [Candidate::new(en_name), Candidate::new(zh_name)],
            en_keywords,
            zh_keywords,
            keywords,
        })
    }

    /// 名称命中的分数，以及（低一档的）关键词命中分数，取高者。`names_only` 时只看名称。
    fn score(&self, query: &str, names_only: bool) -> Option<u32> {
        if query == self.text || query == self.text.replace('\u{FE0F}', "") {
            return Some(1000);
        }
        let name = self
            .names
            .iter()
            .filter_map(|name| matcher::score_candidate(query, name))
            .max();
        if names_only {
            return name;
        }
        let keyword = self
            .keywords
            .iter()
            .filter_map(|keyword| matcher::score_candidate(query, keyword))
            .max()
            .map(|score| score.saturating_sub(KEYWORD_PENALTY));
        name.max(keyword)
    }
}

fn table() -> &'static Vec<Entry> {
    static TABLE: OnceLock<Vec<Entry>> = OnceLock::new();
    TABLE.get_or_init(|| {
        DATA.lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .filter_map(Entry::parse)
            .collect()
    })
}

// ---------------------------------------------------------------- 最近使用

fn recent_store() -> &'static Mutex<Option<Vec<String>>> {
    static RECENT: OnceLock<Mutex<Option<Vec<String>>>> = OnceLock::new();
    RECENT.get_or_init(Mutex::default)
}

fn recent_path() -> Option<std::path::PathBuf> {
    crate::paths::data_dir()
        .ok()
        .map(|dir| dir.join("emoji-recent.json"))
}

/// 最近用过的 emoji，最新的在前。
fn recent() -> Vec<String> {
    let mut guard = recent_store().lock().expect("emoji recent poisoned");
    guard
        .get_or_insert_with(|| {
            recent_path()
                .and_then(|path| std::fs::read_to_string(path).ok())
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default()
        })
        .clone()
}

fn remember(text: &str) {
    let mut list = recent();
    list.retain(|item| item != text);
    list.insert(0, text.to_string());
    list.truncate(RECENT_LIMIT);
    if let Some(path) = recent_path()
        && let Ok(json) = serde_json::to_string(&list)
        && let Err(err) = std::fs::write(&path, json)
    {
        eprintln!("helmp: 保存最近使用的 emoji 失败：{err}");
    }
    *recent_store().lock().expect("emoji recent poisoned") = Some(list);
}

// ---------------------------------------------------------------- 动作

/// 粘贴到前台 App：写剪贴板再模拟 ⌘V（面板执行动作前已经隐藏）。没有辅助功能权限时只复制并说明。
pub(crate) fn paste_text(text: String) -> Action {
    Action::new(move || {
        if let Some(outcome) = crate::effects::dry("paste", &text) {
            return Ok(outcome);
        }
        Action::copy(text.clone()).run()?;
        if !crate::providers::clipboard::macos::accessibility_trusted() {
            return Ok(Outcome::Message(
                tr(
                    "已复制。没有辅助功能权限，无法自动粘贴，请手动粘贴（⌘V）。",
                    "Copied. Helmp can’t paste without Accessibility permission, so paste it yourself (⌘V).",
                )
                .into(),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
        crate::providers::clipboard::macos::send_paste()?;
        Ok(Outcome::Done)
    })
}

/// 先记进「最近使用」再执行。
fn remembering(text: &str, action: Action) -> Action {
    let text = text.to_string();
    Action::new(move || {
        remember(&text);
        action.run()
    })
}

fn code_points(text: &str) -> String {
    text.chars()
        .filter(|c| *c != '\u{FE0F}')
        .map(|c| format!("U+{:04X}", c as u32))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 带肤色的写法：去掉变体选择符，修饰符紧跟第一个字符（`☝️` → `☝🏽`）。
fn with_skin(text: &str, modifier: &str) -> String {
    let base: String = text.chars().filter(|c| *c != '\u{FE0F}').collect();
    let mut chars = base.chars();
    let first = chars.next().map(String::from).unwrap_or_default();
    format!("{first}{modifier}{}", chars.as_str())
}

fn item(entry: &Entry, score: u32) -> Item {
    let (name, keywords) = if crate::i18n::is_en() {
        (entry.en_name, &entry.en_keywords)
    } else {
        (entry.zh_name, &entry.zh_keywords)
    };
    let mut actions = vec![
        NamedAction::new(
            tr("复制", "Copy"),
            Some("⌘C"),
            remembering(entry.text, Action::copy(entry.text)),
        ),
        NamedAction::new(
            tr("复制码点", "Copy Code Point"),
            Some("⌘⇧C"),
            Action::copy(code_points(entry.text)),
        ),
    ];
    if entry.skin {
        for (modifier, zh, en) in SKIN_TONES {
            let variant = with_skin(entry.text, modifier);
            actions.push(NamedAction::new(
                trf!("粘贴 {variant}（{zh}）", "Paste {variant} ({en})"),
                None,
                // 最近使用记基础字符：表里只有基础字符，记肤色变体在首页找不到。
                remembering(entry.text, paste_text(variant.clone())),
            ));
        }
    }
    let id = entry
        .text
        .chars()
        .filter(|c| *c != '\u{FE0F}')
        .map(|c| format!("{:x}", c as u32))
        .collect::<Vec<_>>()
        .join("-");
    Item {
        id: format!("{PROVIDER_ID}:{id}"),
        // 列表图标位暂时画不了任意字符（Icon 只有图片和内置符号），字符放在标题最前面。
        title: format!("{}  {name}", entry.text),
        subtitle: keywords
            .iter()
            .take(6)
            .copied()
            .collect::<Vec<_>>()
            .join(" · "),
        badge: if entry.symbol {
            tr("符号", "Symbols").into()
        } else {
            "Emoji".into()
        },
        score,
        icon: Icon::None,
        action: remembering(entry.text, paste_text(entry.text.to_string())),
        action_title: tr("粘贴", "Paste").into(),
        actions,
        ..Item::blank()
    }
}

/// 搜索：分数相同时最近用过的在前，再按表的顺序（emoji 标准顺序，常用的靠前）。
fn search(query: &str, names_only: bool, limit: usize) -> Vec<(&'static Entry, u32)> {
    let recent = recent();
    let rank = |entry: &Entry| {
        recent
            .iter()
            .position(|r| r == entry.text)
            .unwrap_or(usize::MAX)
    };
    let mut hits: Vec<(usize, &Entry, u32)> = table()
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| Some((index, entry, entry.score(query, names_only)?)))
        .collect();
    hits.sort_by(|(ia, a, sa), (ib, b, sb)| {
        sb.cmp(sa).then(rank(a).cmp(&rank(b))).then(ia.cmp(ib))
    });
    hits.into_iter()
        .take(limit)
        .map(|(_, entry, score)| (entry, score))
        .collect()
}

pub struct Emoji;

impl Provider for Emoji {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn start(&self) -> anyhow::Result<()> {
        // 建索引（中文名的拼音）放在后台，第一次查询不用等。
        table();
        Ok(())
    }

    fn scope(&self) -> Option<Scope> {
        Some(Scope {
            id: PROVIDER_ID,
            label: tr("Emoji 与符号", "Emoji & Symbols").into(),
            tint: Tint::Orange,
            prefixes: vec!["emoji ".into(), "表情 ".into(), "符号 ".into()],
            placeholder: tr(
                "搜索 emoji 和符号（中文、拼音、英文）",
                "Search emoji and symbols",
            )
            .into(),
            filters: Vec::new(),
        })
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let query = query.trim();
        // `:xiao` 只搜 emoji；`:3000` 留给端口查询。
        if let Some(rest) = query.strip_prefix(':')
            && rest
                .chars()
                .next()
                .is_some_and(|c| !c.is_ascii_digit() && !c.is_whitespace())
        {
            return search(rest, false, SCOPE_LIMIT)
                .into_iter()
                .map(|(entry, score)| item(entry, score))
                .collect();
        }
        let mut items: Vec<Item> = Vec::new();
        if query.chars().count() >= 2 {
            items.extend(
                search(query, true, ROOT_LIMIT)
                    .into_iter()
                    .filter(|(_, score)| *score >= 800)
                    // 排在应用、命令这些本地结果后面。
                    .map(|(entry, score)| item(entry, score / 4)),
            );
        }
        items.extend(entry_command(query));
        items
    }

    fn query_scoped(&self, query: &str, _filter: &str) -> Vec<Item> {
        let query = query.trim();
        if query.is_empty() {
            let recent = recent();
            let recent_entries = recent
                .iter()
                .filter_map(|text| table().iter().find(|entry| entry.text == text));
            let rest = table()
                .iter()
                .filter(|entry| !recent.iter().any(|r| r == entry.text));
            return recent_entries
                .chain(rest)
                .take(SCOPE_LIMIT)
                .enumerate()
                .map(|(rank, entry)| item(entry, 1000u32.saturating_sub(rank as u32)))
                .collect();
        }
        search(query, false, SCOPE_LIMIT)
            .into_iter()
            .map(|(entry, score)| item(entry, score))
            .collect()
    }
}

/// 「表情」「符号」「emoji」本身：进入范围。
fn entry_command(query: &str) -> Option<Item> {
    let score = ["表情", "emoji", "符号", "symbols", "表情符号", "颜文字"]
        .iter()
        .filter_map(|keyword| matcher::score(query, keyword))
        .max()
        .filter(|score| *score >= 800 && query.chars().count() >= 2)?;
    Some(Item {
        id: format!("{PROVIDER_ID}:scope"),
        title: tr("Emoji 与符号", "Emoji & Symbols").into(),
        subtitle: tr(
            "按中文、拼音或英文搜索，回车粘贴",
            "Search by name or keyword; Return pastes",
        )
        .into(),
        badge: tr("命令", "Commands").into(),
        score,
        icon: Icon::Symbol {
            name: "face-grinning",
            tint: Tint::Orange,
        },
        action: Action::enter_scope(PROVIDER_ID),
        action_title: tr("打开", "Open").into(),
        ..Item::blank()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(items: &[Item]) -> Vec<String> {
        items.iter().map(|item| item.title.clone()).collect()
    }

    #[test]
    fn table_is_complete_and_parses() {
        // 生成脚本出错（列数不对、漏了中文）时整张表会悄悄变小。
        let table = table();
        assert!(table.len() > 1900, "{}", table.len());
        assert_eq!(
            DATA.lines().filter(|l| !l.starts_with('#')).count(),
            table.len()
        );
        assert!(
            table
                .iter()
                .all(|e| !e.zh_name.is_empty() && !e.en_name.is_empty())
        );
        assert!(DATA.starts_with("# CLDR "), "第一行记录数据版本");
    }

    #[test]
    fn finds_emoji_by_chinese_pinyin_and_english() {
        // 中文用户打「笑」「xiao」，英文用户打 smile，都要找到笑脸；符号也能按中文找到。
        for (query, expected) in [
            ("大笑", "😄"),
            ("dax", "😄"),
            ("smile", "😄"),
            ("猫", "🐱"),
            ("向右的箭头", "→"),
            ("箭头", "➡"),
            ("cmd", "⌘"),
            ("约等于", "≈"),
        ] {
            let items = Emoji.query_scoped(query, "");
            assert!(
                items
                    .iter()
                    .take(8)
                    .any(|item| item.title.starts_with(expected)),
                "{query:?}: {:?}",
                titles(&items).into_iter().take(8).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn root_search_mixes_in_only_a_few_low_ranked_hits() {
        // 根搜索里 emoji 只是顺带：最多三条、分数低于应用的模糊匹配，`wx` 不能被「微笑」抢走。
        let items = Emoji.query("smile");
        assert!(items.len() <= ROOT_LIMIT, "{:?}", titles(&items));
        assert!(items.iter().all(|item| item.score < 300));
        let colon = Emoji.query(":xiao");
        assert!(colon.len() > ROOT_LIMIT, "冒号开头是专门搜 emoji");
        assert!(Emoji.query(":3000").is_empty(), "留给端口查询");
    }

    #[test]
    fn skin_tones_attach_to_the_base_character() {
        assert_eq!(with_skin("👍", "\u{1F3FD}"), "👍🏽");
        assert_eq!(with_skin("☝\u{FE0F}", "\u{1F3FD}"), "☝🏽");
        assert_eq!(code_points("❤\u{FE0F}"), "U+2764");
        let thumbs = table().iter().find(|e| e.text == "👍").unwrap();
        assert!(thumbs.skin);
        assert_eq!(item(thumbs, 1).actions.len(), 7);
    }

    #[test]
    fn icon_exists() {
        use gpui_kit::AssetSource as _;
        assert!(
            gpui_kit::assets::AllAssets
                .load("icons/face-grinning.svg")
                .unwrap()
                .is_some()
        );
    }
}
