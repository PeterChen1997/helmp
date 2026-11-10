//! 语义化版本（semver 2.0）：解析与比较。预发布版本低于同号正式版，构建元数据不参与比较。

use std::cmp::Ordering;
use std::fmt;

use anyhow::{Result, bail};

use crate::i18n::trf;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Ident {
    Num(u64),
    Alpha(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<Ident>,
    /// 原文（去掉开头的 `v` 与构建元数据），用于显示。
    text: String,
}

impl Version {
    /// 解析 `1.2.3`、`v1.2.3-beta.2`、`1.2.3+build.5`。三段数字缺一不可。
    pub fn parse(input: &str) -> Result<Self> {
        let raw = input.trim();
        let invalid = || trf!("不是有效的版本号：{raw:?}", "Not a valid version: {raw:?}");
        let text = raw.strip_prefix('v').unwrap_or(raw);
        let text = text.split('+').next().unwrap_or_default();
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let numbers: Vec<&str> = core.split('.').collect();
        let [major, minor, patch] = numbers[..] else {
            bail!(invalid());
        };
        let number = |part: &str| -> Result<u64> {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                bail!(invalid());
            }
            part.parse().map_err(|_| anyhow::anyhow!(invalid()))
        };
        let mut idents = Vec::new();
        if let Some(pre) = pre {
            for part in pre.split('.') {
                if part.is_empty()
                    || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                {
                    bail!(invalid());
                }
                idents.push(if part.bytes().all(|b| b.is_ascii_digit()) {
                    Ident::Num(number(part)?)
                } else {
                    Ident::Alpha(part.to_string())
                });
            }
        }
        Ok(Self {
            major: number(major)?,
            minor: number(minor)?,
            patch: number(patch)?,
            pre: idents,
            text: text.to_string(),
        })
    }

    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("Cargo.toml 的版本号必须是 semver")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

fn cmp_ident(a: &Ident, b: &Ident) -> Ordering {
    match (a, b) {
        (Ident::Num(x), Ident::Num(y)) => x.cmp(y),
        // 纯数字标识符低于字母数字标识符。
        (Ident::Num(_), Ident::Alpha(_)) => Ordering::Less,
        (Ident::Alpha(_), Ident::Num(_)) => Ordering::Greater,
        (Ident::Alpha(x), Ident::Alpha(y)) => x.cmp(y),
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (a, b) in self.pre.iter().zip(&other.pre) {
                        let order = cmp_ident(a, b);
                        if order != Ordering::Equal {
                            return order;
                        }
                    }
                    self.pre.len().cmp(&other.pre.len())
                }
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// macOS 版本（`15.0`、`26.1.2`）：按数字逐段比较，缺的段当 0。不是数字的版本号返回 None。
pub fn macos_at_least(current: &str, minimum: &str) -> Option<bool> {
    let parts = |text: &str| -> Option<Vec<u64>> {
        text.trim().split('.').map(|p| p.parse().ok()).collect()
    };
    let (current, minimum) = (parts(current)?, parts(minimum)?);
    for i in 0..current.len().max(minimum.len()) {
        let (a, b) = (
            current.get(i).copied().unwrap_or(0),
            minimum.get(i).copied().unwrap_or(0),
        );
        if a != b {
            return Some(a > b);
        }
    }
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|err| panic!("{text}: {err:#}"))
    }

    #[test]
    fn numeric_parts_compare_as_numbers_not_strings() {
        // 0.10.0 必须比 0.9.0 新：按字符串比会把 0.10 排在 0.9 前面，用户就永远收不到更新。
        assert!(v("0.10.0") > v("0.9.0"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("0.3.1") > v("0.3.0"));
        assert_eq!(v("0.3.0").cmp(&v("v0.3.0")), Ordering::Equal);
    }

    #[test]
    fn prerelease_ordering_follows_semver() {
        // semver 2.0 第 11 条的示例顺序：正式版高于它的任何预发布版。
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
        ];
        for pair in ordered.windows(2) {
            assert!(v(pair[1]) > v(pair[0]), "{} 应高于 {}", pair[1], pair[0]);
        }
        // 正在用 0.4.0-beta.1 的人应该收到 0.4.0 正式版；正式版用户不应「升级」到同号预发布版。
        assert!(v("0.4.0") > v("0.4.0-beta.1"));
        assert!(v("0.4.0-beta.1") < v("0.4.0"));
    }

    #[test]
    fn build_metadata_is_ignored() {
        assert_eq!(v("1.2.3+abc").cmp(&v("1.2.3+def")), Ordering::Equal);
        assert_eq!(v("1.2.3+abc").to_string(), "1.2.3");
    }

    #[test]
    fn malformed_versions_are_rejected() {
        for bad in ["", "1", "1.2", "1.2.3.4", "1.x.3", "1.2.3-", "1.2.3-a..b", "latest", "1.2.-3"] {
            assert!(Version::parse(bad).is_err(), "{bad:?} 不应被接受");
        }
    }

    #[test]
    fn current_version_parses() {
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn macos_minimum_compares_numerically() {
        assert_eq!(macos_at_least("15.0", "15.0"), Some(true));
        assert_eq!(macos_at_least("26.1", "15.0"), Some(true));
        assert_eq!(macos_at_least("15", "15.0.1"), Some(false));
        assert_eq!(macos_at_least("14.7.2", "15.0"), Some(false));
        assert_eq!(macos_at_least("15.x", "15.0"), None);
    }
}
