//! 文本匹配打分。所有 provider 共用，返回 `None` 表示不匹配。
//!
//! 字面规则（大小写不敏感）：完全相等 1000 > 前缀 900 > 单词首字母 800 > 子串 700 > 子序列 400。
//!
//! 含汉字的候选另走拼音规则，取两者较高分：
//! - 850：从第一个音节起，每个音节取一段非空前缀，恰好用到最后一个音节
//!   （`weixin`、`wx`、`wxin` → 微信）。
//! - 750：从第一个音节起按音节前缀吃完查询，但没用到最后（`wx` → 微信开发者工具）。
//! - 550：同上，但从中间某个音节起（`xin` → 微信）。
//! - 300：查询是默认读音全拼的子序列。故意低于字面子序列，保证字面命中永远排在拼音模糊命中前面。
//!
//! 多音字的每个读音都参与音节匹配（`zhong`/`chong` 都能命中「重」）。
//! 索引型 provider 应该用 [`Candidate`] 预处理一次，查询时调用 [`score_candidate`]，避免每次查询重算拼音。
//! 需要告诉用户「为什么命中」时（应用详情的「匹配方式」）用 [`match_candidate`]，分数与 `score_candidate` 一致。

use pinyin::{ToPinyin, ToPinyinMulti};

use crate::i18n::tr;

const EXACT: u32 = 1000;
const PREFIX: u32 = 900;
const PINYIN_FULL: u32 = 850;
const INITIALS: u32 = 800;
const PINYIN_PREFIX: u32 = 750;
const SUBSTRING: u32 = 700;
const PINYIN_INNER: u32 = 550;
const SUBSEQUENCE: u32 = 400;
const PINYIN_SUBSEQUENCE: u32 = 300;

pub fn score(query: &str, candidate: &str) -> Option<u32> {
    score_candidate(query, &Candidate::new(candidate))
}

/// 预处理过的候选文本：小写形式，以及含汉字时的拼音音节。
#[derive(Debug, Clone)]
pub struct Candidate {
    lower: String,
    pinyin: Option<Syllables>,
}

impl Candidate {
    pub fn new(text: &str) -> Self {
        let lower = text.to_lowercase();
        let pinyin = Syllables::new(&lower);
        Self { lower, pinyin }
    }
}

pub fn score_candidate(query: &str, candidate: &Candidate) -> Option<u32> {
    if query.trim().is_empty() {
        return Some(0);
    }
    match_candidate(query, candidate).map(|m| m.score)
}

/// 命中方式。每种方式对应唯一的分数档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    Exact,
    Prefix,
    /// 英文单词首字母（`vsc` → Visual Studio Code）。
    WordInitials,
    Substring,
    Subsequence,
    /// 每个音节只打了首字母（`wx` → 微信）。
    PinyinInitials,
    /// 每个音节都打了完整的默认读音（`weixin` → 微信）。
    PinyinFull,
    /// 全拼与首字母混打，或用了多音字的其他读音（`wxin` → 微信）。
    PinyinMixed,
    /// 默认读音全拼的子序列。
    PinyinFuzzy,
}

impl MatchKind {
    /// 给人看的名字，用于详情里的「匹配方式」。
    pub fn label(self) -> &'static str {
        match self {
            MatchKind::Exact => tr("名称完全匹配", "Exact name"),
            MatchKind::Prefix => tr("名称开头", "Name prefix"),
            MatchKind::WordInitials => tr("单词首字母", "Word initials"),
            MatchKind::Substring => tr("名称包含", "Part of name"),
            MatchKind::Subsequence => tr("模糊匹配", "Fuzzy"),
            MatchKind::PinyinInitials => tr("拼音首字母", "Pinyin initials"),
            MatchKind::PinyinFull => tr("全拼", "Full pinyin"),
            MatchKind::PinyinMixed => tr("拼音", "Pinyin"),
            MatchKind::PinyinFuzzy => tr("拼音模糊匹配", "Fuzzy pinyin"),
        }
    }
}

/// 拼音命中的那一段音节（默认读音）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinyinSpan {
    /// 全拼，如 `weixin`。
    pub full: String,
    /// 首字母，如 `wx`。
    pub initials: String,
    /// 是否从第一个音节一直覆盖到最后一个（`wx` → 微信 是，`wx` → 微信开发者工具 不是）。
    pub whole: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// 与 [`score_candidate`] 相同的分数。
    pub score: u32,
    pub kind: MatchKind,
    /// 走音节规则命中时才有；拼音模糊匹配与字面命中为 `None`。
    pub pinyin: Option<PinyinSpan>,
}

/// 与 [`score_candidate`] 同样的规则，额外返回命中方式。空查询返回 `None`。
pub fn match_candidate(query: &str, candidate: &Candidate) -> Option<Match> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return None;
    }
    let literal = literal_match(&query, &candidate.lower).map(|(score, kind)| Match {
        score,
        kind,
        pinyin: None,
    });
    let pinyin = candidate.pinyin.as_ref().and_then(|s| s.matches(&query));
    // 各档分数互不相同，取较高的命中。
    match (literal, pinyin) {
        (Some(a), Some(b)) => Some(if b.score > a.score { b } else { a }),
        (a, b) => a.or(b),
    }
}

fn literal_match(query: &str, candidate: &str) -> Option<(u32, MatchKind)> {
    if candidate == query {
        return Some((EXACT, MatchKind::Exact));
    }
    if candidate.starts_with(query) {
        return Some((PREFIX, MatchKind::Prefix));
    }
    if initials(candidate).starts_with(query) {
        return Some((INITIALS, MatchKind::WordInitials));
    }
    if candidate.contains(query) {
        return Some((SUBSTRING, MatchKind::Substring));
    }
    subsequence(query, candidate).then_some((SUBSEQUENCE, MatchKind::Subsequence))
}

/// 取每个单词首字母：`visual studio code` → `vsc`。
fn initials(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || c == '-' || c == '_' || c == '.')
        .filter_map(|word| word.chars().next())
        .collect()
}

fn subsequence(query: &str, candidate: &str) -> bool {
    let mut rest = candidate.chars();
    query.chars().all(|q| rest.any(|c| c == q))
}

/// 候选文本切成的「音节」：每个汉字一个（带全部读音），连续的字母数字算一个。
#[derive(Debug, Clone)]
struct Syllables {
    /// 每个音节的可选拼写，第一个是默认读音。
    tokens: Vec<Vec<String>>,
    /// 默认读音连起来的全拼，用于子序列兜底。
    joined: String,
}

impl Syllables {
    /// 不含汉字时返回 `None`：纯字母候选只走字面规则。
    fn new(lower: &str) -> Option<Self> {
        let mut tokens: Vec<Vec<String>> = Vec::new();
        let mut word = String::new();
        let mut has_han = false;
        for ch in lower.chars() {
            if let Some(default) = ch.to_pinyin() {
                has_han = true;
                if !word.is_empty() {
                    tokens.push(vec![std::mem::take(&mut word)]);
                }
                let mut readings = vec![default.plain().to_string()];
                if let Some(multi) = ch.to_pinyin_multi() {
                    for reading in multi {
                        let plain = reading.plain();
                        if !readings.iter().any(|r| r == plain) {
                            readings.push(plain.to_string());
                        }
                    }
                }
                tokens.push(readings);
            } else if ch.is_alphanumeric() {
                word.push(ch);
            } else if !word.is_empty() {
                tokens.push(vec![std::mem::take(&mut word)]);
            }
        }
        if !word.is_empty() {
            tokens.push(vec![word]);
        }
        if !has_han {
            return None;
        }
        let joined = tokens
            .iter()
            .filter_map(|t| t.first())
            .map(String::as_str)
            .collect();
        Some(Self { tokens, joined })
    }

    fn matches(&self, query: &str) -> Option<Match> {
        // 拼音只由 ASCII 字母数字组成；用户可能用空格分隔音节（`wei xin`）。
        let query: String = query.chars().filter(|c| !c.is_whitespace()).collect();
        if query.is_empty() || !query.is_ascii() {
            return None;
        }
        let total = self.tokens.len();
        // 起点越靠前档位越高（从头 > 从中间），所以第一个能吃完查询的起点就是最好的。
        for start in 0..total {
            let Some(end) = self.consume(query.as_bytes(), start) else {
                continue;
            };
            let score = match (start, end) {
                (0, end) if end == total => PINYIN_FULL,
                (0, _) => PINYIN_PREFIX,
                _ => PINYIN_INNER,
            };
            let span = self.tokens.get(start..end).unwrap_or_default();
            let full: String = span
                .iter()
                .filter_map(|t| t.first())
                .map(String::as_str)
                .collect();
            let initials: String = span
                .iter()
                .filter_map(|t| t.first()?.chars().next())
                .collect();
            // 每个音节至少吃一个字符，长度相等说明每个音节恰好只打了首字母。
            let kind = if query.len() == end - start {
                MatchKind::PinyinInitials
            } else if query == full {
                MatchKind::PinyinFull
            } else {
                MatchKind::PinyinMixed
            };
            return Some(Match {
                score,
                kind,
                pinyin: Some(PinyinSpan {
                    full,
                    initials,
                    whole: start == 0 && end == total,
                }),
            });
        }
        subsequence(&query, &self.joined).then_some(Match {
            score: PINYIN_SUBSEQUENCE,
            kind: MatchKind::PinyinFuzzy,
            pinyin: None,
        })
    }

    /// 从第 `start` 个音节起，每个音节吃掉查询的一段非空前缀，直到查询吃完。
    /// 成功时返回能走到的最远音节下标（不含）。用备忘录保证最坏情况多项式。
    fn consume(&self, query: &[u8], start: usize) -> Option<usize> {
        let width = self.tokens.len() + 1;
        let mut memo: Vec<Option<Option<usize>>> = vec![None; (query.len() + 1) * width];
        self.consume_from(query, 0, start, &mut memo, width)
    }

    fn consume_from(
        &self,
        query: &[u8],
        pos: usize,
        token: usize,
        memo: &mut [Option<Option<usize>>],
        width: usize,
    ) -> Option<usize> {
        if pos == query.len() {
            return Some(token);
        }
        let key = pos * width + token;
        if let Some(&Some(cached)) = memo.get(key) {
            return cached;
        }
        let mut best = None;
        if let Some(readings) = self.tokens.get(token) {
            for reading in readings {
                let shared = reading
                    .bytes()
                    .zip(query.get(pos..).unwrap_or_default())
                    .take_while(|(a, b)| a == *b)
                    .count();
                for len in 1..=shared {
                    let end = self.consume_from(query, pos + len, token + 1, memo, width);
                    best = best.max(end);
                }
            }
        }
        if let Some(slot) = memo.get_mut(key) {
            *slot = Some(best);
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_exact_over_prefix_over_substring() {
        // 排序决定用户回车打开的是不是他想要的那个，所以档位必须严格递减。
        let exact = score("safari", "Safari").unwrap();
        let prefix = score("saf", "Safari").unwrap();
        let initials = score("vsc", "Visual Studio Code").unwrap();
        let substring = score("fari", "Safari").unwrap();
        let subseq = score("sfr", "Safari").unwrap();
        assert!(exact > prefix && prefix > initials && initials > substring && substring > subseq);
    }

    #[test]
    fn rejects_non_matching() {
        assert_eq!(score("xyz", "Safari"), None);
    }

    #[test]
    fn full_pinyin_and_initials_hit_chinese_names() {
        // 中文用户不切输入法就能搜中文应用名，是本项目相对 Raycast/Spotlight 的核心差异（README「MVP 验收」）。
        for query in ["weixin", "wx", "wxin", "weix", "wei xin", "WX"] {
            assert_eq!(score(query, "微信"), Some(PINYIN_FULL), "{query}");
        }
        assert_eq!(score("jsq", "计算器"), Some(PINYIN_FULL));
        assert_eq!(score("fs", "飞书"), Some(PINYIN_FULL));
        assert_eq!(score("feishu", "飞书"), Some(PINYIN_FULL));
        // 汉字本身仍走字面规则，拿最高分。
        assert_eq!(score("微信", "微信"), Some(EXACT));
    }

    #[test]
    fn pinyin_tiers_prefer_complete_abbreviations() {
        // 「wx」打完整个名字（微信）应排在只打了开头（微信开发者工具）之前，否则回车会打开错的应用。
        let whole = score("wx", "微信").unwrap();
        let head = score("wx", "微信开发者工具").unwrap();
        let inner = score("xin", "微信").unwrap();
        let fuzzy = score("wix", "微信").unwrap();
        assert!(whole > head && head > inner && inner > fuzzy);
        assert_eq!(fuzzy, PINYIN_SUBSEQUENCE);
    }

    #[test]
    fn mixed_latin_and_chinese_names() {
        // 「QQ音乐」这类中英混排名很常见：英文单词整体算一个音节，可以和拼音首字母连着打。
        assert_eq!(score("qqyy", "QQ音乐"), Some(PINYIN_FULL));
        assert_eq!(score("qqyinyue", "QQ音乐"), Some(PINYIN_FULL));
        assert_eq!(score("yy", "QQ音乐"), Some(PINYIN_INNER));
        // 中英混排名的英文部分仍走字面前缀。
        assert_eq!(score("safari", "Safari浏览器"), Some(PREFIX));
        assert_eq!(score("safarillq", "Safari浏览器"), Some(PINYIN_FULL));
    }

    #[test]
    fn heteronyms_match_every_reading() {
        // 多音字只认默认读音会让一半用户搜不到（「重庆」有人打 cq，也有人按 zhong 打 zq）。
        assert!(score("cq", "重庆").is_some());
        assert!(score("zq", "重庆").is_some());
    }

    #[test]
    fn literal_prefix_beats_pinyin_fuzzy() {
        // 字面前缀是用户最确定的意图，任何拼音模糊匹配都不能压过它。
        let literal_prefix = score("we", "WeChat").unwrap();
        let pinyin_subseq = score("wexn", "微信").unwrap();
        let literal_subseq = score("wct", "WeChat").unwrap();
        assert!(literal_prefix > pinyin_subseq);
        assert!(literal_subseq > pinyin_subseq);
    }

    #[test]
    fn match_kind_explains_how_the_query_hit() {
        // 应用详情的「匹配方式」要说清楚为什么命中：用户看到「拼音首字母」才知道 wx 能直接搜微信。
        let m = match_candidate("wx", &Candidate::new("微信")).unwrap();
        assert_eq!(m.kind, MatchKind::PinyinInitials);
        assert_eq!(
            m.pinyin,
            Some(PinyinSpan {
                full: "weixin".into(),
                initials: "wx".into(),
                whole: true
            })
        );
        let kind = |q: &str, c: &str| match_candidate(q, &Candidate::new(c)).unwrap().kind;
        assert_eq!(kind("weixin", "微信"), MatchKind::PinyinFull);
        assert_eq!(kind("wxin", "微信"), MatchKind::PinyinMixed);
        assert_eq!(kind("wix", "微信"), MatchKind::PinyinFuzzy);
        assert_eq!(kind("微信", "微信"), MatchKind::Exact);
        assert_eq!(kind("vsc", "Visual Studio Code"), MatchKind::WordInitials);
        // 只命中开头时，span 只覆盖被打到的音节，并标明没有覆盖整个名字。
        let head = match_candidate("wx", &Candidate::new("微信开发者工具")).unwrap();
        assert_eq!(head.kind, MatchKind::PinyinInitials);
        let span = head.pinyin.unwrap();
        assert_eq!(span.full, "weixin");
        assert!(!span.whole);
    }

    #[test]
    fn empty_query_keeps_score_contract() {
        // 别的插件依赖「空查询 score 为 Some(0)」列出全部候选；match_candidate 没有「命中方式」可说，返回 None。
        assert_eq!(score("  ", "微信"), Some(0));
        assert_eq!(match_candidate("  ", &Candidate::new("微信")), None);
    }

    #[test]
    fn pinyin_rejects_unrelated_queries() {
        assert_eq!(score("fs", "微信"), None);
        assert_eq!(score("xw", "微信"), None);
    }
}
