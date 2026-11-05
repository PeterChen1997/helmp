//! 文本匹配打分。所有 provider 共用，返回 `None` 表示不匹配。
//!
//! 当前规则（大小写不敏感）：完全相等 > 前缀 > 单词首字母 > 子串 > 子序列。

pub fn score(query: &str, candidate: &str) -> Option<u32> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let candidate = candidate.to_lowercase();

    if candidate == query {
        return Some(1000);
    }
    if candidate.starts_with(&query) {
        return Some(900);
    }
    if initials(&candidate).starts_with(&query) {
        return Some(800);
    }
    if candidate.contains(&query) {
        return Some(700);
    }
    subsequence(&query, &candidate).then_some(400)
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
}
