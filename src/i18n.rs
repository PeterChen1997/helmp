//! 界面语言：简体中文 / English。
//!
//! 用法（中英两份文字就地写在一起）：
//! - 固定文案：`tr("设置…", "Settings…")`，按当前语言返回其一。
//! - 带参数：`trf!("{n} 个会话在等你", "{n} sessions waiting")`，返回 `String`，写法同 `format!`。
//!
//! 当前语言放在全局原子里，由 `config.general.language` 驱动：`init` 时读取并订阅配置变化，
//! 跟随系统时按 NSLocale.preferredLanguages 判断。语言真的变了 `generation()` 才加一，
//! 界面（启动器、菜单栏、设置窗口）据此刷新，无需重启。
//!
//! 只翻译界面文案，搜索逻辑不变：插件匹配用的关键词（中文名、拼音）不经过这里。
//!
//! 同一次查询内语言必须一致：结果的分组名（`Item.badge`）既是显示文字也是分组键。
//! `pinned` 在当前线程把语言固定住，`Engine` 的查询都包在里面，查询中途切换语言也不会把同一组拆成两半。

use std::cell::Cell;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use crate::config::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

/// 0 = 中文（未初始化时的默认，单测也按中文跑），1 = English。
static CURRENT: AtomicU8 = AtomicU8::new(0);
static GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// `pinned` / `with_lang` 在当前线程固定的语言，优先于全局设置。
    static PINNED: Cell<Option<Lang>> = const { Cell::new(None) };
}

/// 当前语言（当前线程固定过就用固定的）。
pub fn lang() -> Lang {
    PINNED.with(Cell::get).unwrap_or_else(global)
}

fn global() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::En,
        _ => Lang::Zh,
    }
}

pub fn is_en() -> bool {
    lang() == Lang::En
}

/// 按当前语言二选一。
pub fn tr<'a>(zh: &'a str, en: &'a str) -> &'a str {
    match lang() {
        Lang::Zh => zh,
        Lang::En => en,
    }
}

/// 带参数的文案：`trf!("已复制 {n} 项", "Copied {n} items")`，参数写法同 `format!`。
macro_rules! trf {
    ($zh:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if $crate::i18n::is_en() {
            format!($en $(, $arg)*)
        } else {
            format!($zh $(, $arg)*)
        }
    };
}
pub(crate) use trf;

/// 语言切换的代数：每切换一次加一。界面记下上次看到的值，不同就重新取文案。
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// 在 `f` 执行期间把当前线程的语言固定为此刻的语言（已固定则沿用）。
pub fn pinned<R>(f: impl FnOnce() -> R) -> R {
    with_lang(lang(), f)
}

/// 在 `f` 执行期间把当前线程的语言固定为 `lang`。单测用它切换语言，不影响并行的其他测试。
pub fn with_lang<R>(lang: Lang, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Lang>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PINNED.with(|pinned| pinned.set(self.0));
        }
    }
    let _restore = Restore(PINNED.with(|pinned| pinned.replace(Some(lang))));
    f()
}

/// 读取配置里的语言设置并订阅变化。启动时（CLI 与图形界面都要）调用一次。
pub fn init() {
    apply(crate::config::get().general.language);
    crate::config::subscribe(|config| apply(config.general.language));
}

/// 按设置更新全局语言；真的变了才加代数。
fn apply(setting: Language) {
    let next = resolve(setting, &system_languages());
    let value = match next {
        Lang::Zh => 0,
        Lang::En => 1,
    };
    if CURRENT.swap(value, Ordering::Relaxed) != value {
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

/// 设置 + 系统首选语言列表 → 界面语言。
///
/// 跟随系统时，按首选语言的顺序找第一个中文或英文：日语在前、中文在后的用户看中文；
/// 都没有时用英文。繁体中文用户也显示简体中文，比英文更好读。
fn resolve(setting: Language, preferred: &[String]) -> Lang {
    match setting {
        Language::ZhHans => Lang::Zh,
        Language::En => Lang::En,
        Language::System => preferred
            .iter()
            .find_map(|tag| {
                let tag = tag.to_ascii_lowercase();
                if tag.starts_with("zh") {
                    Some(Lang::Zh)
                } else if tag.starts_with("en") {
                    Some(Lang::En)
                } else {
                    None
                }
            })
            .unwrap_or(Lang::En),
    }
}

/// 系统首选语言，如 `["zh-Hans-CN", "en-US"]`。
fn system_languages() -> Vec<String> {
    objc2_foundation::NSLocale::preferredLanguages()
        .iter()
        .map(|tag| tag.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn manual_choice_overrides_system() {
        // 用户在设置里手动选了语言，就不能再被系统语言左右。
        let english_system = tags(&["en-US"]);
        let chinese_system = tags(&["zh-Hans-CN"]);
        assert_eq!(resolve(Language::ZhHans, &english_system), Lang::Zh);
        assert_eq!(resolve(Language::En, &chinese_system), Lang::En);
    }

    #[test]
    fn follows_the_first_supported_system_language() {
        // 跟随系统：中文系统看中文、英文系统看英文；
        // 首选日语、次选中文的用户应看中文，而不是因为第一项不认识就退回英文。
        assert_eq!(
            resolve(Language::System, &tags(&["zh-Hans-CN", "en-US"])),
            Lang::Zh
        );
        assert_eq!(
            resolve(Language::System, &tags(&["en-GB", "zh-Hans"])),
            Lang::En
        );
        assert_eq!(
            resolve(Language::System, &tags(&["ja-JP", "zh-Hant-TW"])),
            Lang::Zh
        );
        // 没有中英文时用英文。
        assert_eq!(
            resolve(Language::System, &tags(&["ja-JP", "fr-FR"])),
            Lang::En
        );
        assert_eq!(resolve(Language::System, &[]), Lang::En);
    }

    #[test]
    fn pinned_language_is_per_thread_and_restored() {
        // 单测与查询都靠线程内固定语言：离开作用域必须恢复，否则会串到后面的代码里。
        with_lang(Lang::En, || {
            assert_eq!(tr("设置…", "Settings…"), "Settings…");
            let n = 2;
            assert_eq!(
                trf!("{n} 个会话在等你", "{n} sessions waiting"),
                "2 sessions waiting"
            );
            with_lang(Lang::Zh, || assert_eq!(tr("设置…", "Settings…"), "设置…"));
            assert!(is_en());
        });
        with_lang(Lang::Zh, || {
            // pinned 沿用已固定的语言。
            pinned(|| assert_eq!(lang(), Lang::Zh));
        });
    }

    #[test]
    fn system_languages_are_readable() {
        // 跟随系统依赖这里；读不到时 resolve 会退回英文，至少不应崩溃。
        let _ = system_languages();
    }
}
