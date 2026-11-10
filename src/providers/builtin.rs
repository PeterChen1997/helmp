//! 内置命令：退出 Helmp、打开设置。也是最小的 provider 示例。

use crate::i18n::tr;
use crate::matcher;
use crate::provider::{Accessory, Action, Icon, Item, Outcome, Provider, Tint};

/// 界面文字存 (中文, 英文)，查询时按当前语言取；匹配只看 `keywords`（中英都在里面），与界面语言无关。
struct Command {
    key: &'static str,
    title: (&'static str, &'static str),
    subtitle: (&'static str, &'static str),
    keywords: &'static [&'static str],
    icon: Icon,
    shortcut: Option<&'static str>,
    action_title: (&'static str, &'static str),
    run: fn() -> anyhow::Result<Outcome>,
}

/// 查询里没提 helmp、只命中「设置」类通用词时的分数上限：低于应用名的子串命中（700）。
const GENERIC_SCORE: u32 = 500;

const COMMANDS: [Command; 2] = [
    Command {
        key: "settings",
        title: ("Helmp 设置", "Helmp Settings"),
        subtitle: (
            "Helmp 自己的设置：快捷键、扩展、插件、模型",
            "Helmp's own settings: shortcuts, extensions, plugins, models",
        ),
        keywords: &[
            "helmp 设置",
            "helmp settings",
            "helmp 偏好设置",
            "helmp preferences",
            "设置",
            "偏好设置",
            "settings",
            "preferences",
        ],
        icon: Icon::Symbol {
            name: "settings",
            tint: Tint::Blue,
        },
        shortcut: Some("⌘,"),
        action_title: ("打开设置", "Open Settings"),
        // 动作在后台线程执行，不能直接开窗口；交给界面线程。
        run: || Ok(Outcome::OpenSettings),
    },
    Command {
        key: "quit",
        title: ("退出 Helmp", "Quit Helmp"),
        subtitle: ("Quit Helmp", "Helmp"),
        keywords: &["退出 Helmp", "quit helmp"],
        icon: Icon::Symbol {
            name: "power",
            tint: Tint::Graphite,
        },
        shortcut: None,
        action_title: ("退出", "Quit"),
        run: || std::process::exit(0),
    },
];

/// Helmp 自己的命令用 Helmp 的 App 图标，和系统设置等一眼区分开；不在 .app 里运行（命令行）时用符号。
fn helmp_icon() -> Option<Icon> {
    let exe = std::env::current_exe().ok()?;
    let app = exe.ancestors().find(|dir| dir.extension().is_some_and(|ext| ext == "app"))?;
    Some(Icon::File(app.to_path_buf()))
}

pub struct Builtin;

impl Provider for Builtin {
    fn id(&self) -> &'static str {
        "builtin"
    }

    fn query(&self, query: &str) -> Vec<Item> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        COMMANDS
            .iter()
            .filter_map(|command| {
                let score = command
                    .keywords
                    .iter()
                    .filter_map(|keyword| {
                        let score = matcher::score(query, keyword)?;
                        // 只打「设置 / settings」这类通用词时，用户多半要的是系统设置：
                        // Helmp 自己的设置排到它后面；带上「helmp」才排第一。
                        let mentions_helmp = query.to_lowercase().contains("helmp");
                        Some(if mentions_helmp || !keyword.contains("设置") && !keyword.contains("setting") && !keyword.contains("preferences") {
                            score
                        } else {
                            score.min(GENERIC_SCORE)
                        })
                    })
                    .max()?;
                Some(Item {
                    id: format!("builtin:{}", command.key),
                    title: tr(command.title.0, command.title.1).into(),
                    subtitle: tr(command.subtitle.0, command.subtitle.1).into(),
                    badge: tr("命令", "Commands").into(),
                    score,
                    icon: helmp_icon().unwrap_or_else(|| command.icon.clone()),
                    accessories: command.shortcut.map(Accessory::keys).into_iter().collect(),
                    action: Action::new(command.run),
                    action_title: tr(command.action_title.0, command.action_title.1).into(),
                    ..Item::blank()
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(query: &str, id: &str) -> Option<Item> {
        Builtin.query(query).into_iter().find(|item| item.id == id)
    }

    #[test]
    fn settings_command_opens_settings_window() {
        // 动作跑在后台线程，直接开窗口会崩；必须返回 OpenSettings 交给界面线程。
        for query in ["设置", "settings", "pref"] {
            let item = find(query, "builtin:settings").unwrap_or_else(|| panic!("{query:?}"));
            assert_eq!(item.action.run().unwrap(), Outcome::OpenSettings);
            assert_eq!(item.accessories, vec![Accessory::keys("⌘,")]);
            assert_eq!(
                item.icon,
                Icon::Symbol {
                    name: "settings",
                    tint: Tint::Blue
                }
            );
        }
    }

    #[test]
    fn generic_settings_query_ranks_below_system_settings() {
        // 打「设置 / settings」多半是要系统设置：Helmp 自己的设置要排在「系统设置」应用后面
        // （应用名子串命中是 700 分），带上 helmp 才排第一。
        for query in ["设置", "settings", "setting"] {
            let item = find(query, "builtin:settings").unwrap();
            assert!(item.score < 700, "{query}: {}", item.score);
        }
        assert!(find("helmp 设置", "builtin:settings").unwrap().score >= 900);
        assert!(find("helmp settings", "builtin:settings").unwrap().score >= 900);
    }

    #[test]
    fn quit_is_found_but_never_on_empty_query() {
        // 退出不能出现在首页（空查询），误按回车就把启动器关了。
        assert!(Builtin.query("").is_empty());
        let quit = find("quit", "builtin:quit").expect("quit 应找到退出");
        assert_eq!(
            quit.icon,
            Icon::Symbol {
                name: "power",
                tint: Tint::Graphite
            }
        );
        assert!(find("退出", "builtin:quit").is_some());
    }

    #[test]
    fn symbol_icons_exist_in_bundled_assets() {
        // 图标名拼错时界面只会画一个空白方块，不会报错。
        use gpui_kit::AssetSource as _;
        for command in &COMMANDS {
            let Icon::Symbol { name, .. } = command.icon else {
                panic!("{} 应使用内置符号", command.key);
            };
            let path = format!("icons/{name}.svg");
            assert!(gpui_kit::assets::AllAssets.load(&path).unwrap().is_some());
        }
    }

    #[test]
    fn english_ui_shows_english_text_but_still_matches_chinese() {
        // 切到英文后标题、分组、动作名要跟着变；但匹配只看关键词，
        // 英文界面下输入中文「设置」「退出」仍要找得到，中文界面下输入英文也一样。
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let settings = find("设置", "builtin:settings").expect("英文界面输入中文应找到设置");
            assert_eq!(settings.title, "Helmp Settings");
            assert_eq!(settings.badge, "Commands");
            assert_eq!(settings.action_title, "Open Settings");
            let quit = find("退出", "builtin:quit").expect("英文界面输入中文应找到退出");
            assert_eq!(quit.title, "Quit Helmp");
            assert_eq!(quit.action_title, "Quit");
        });
        let settings = find("settings", "builtin:settings").expect("中文界面输入英文应找到设置");
        assert_eq!(settings.title, "Helmp 设置");
        assert_eq!(settings.badge, "命令");
    }
}
