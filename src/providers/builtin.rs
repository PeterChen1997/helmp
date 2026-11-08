//! 内置命令：退出 Helmp、打开设置。也是最小的 provider 示例。

use crate::matcher;
use crate::provider::{Accessory, Action, Icon, Item, Outcome, Provider, Tint};

const BADGE: &str = "命令";

struct Command {
    key: &'static str,
    title: &'static str,
    subtitle: &'static str,
    keywords: &'static [&'static str],
    icon: Icon,
    shortcut: Option<&'static str>,
    action_title: &'static str,
    run: fn() -> anyhow::Result<Outcome>,
}

const COMMANDS: [Command; 2] = [
    Command {
        key: "settings",
        title: "设置…",
        subtitle: "Helmp Settings",
        keywords: &[
            "设置",
            "偏好设置",
            "settings",
            "preferences",
            "helmp 设置",
            "helmp settings",
        ],
        icon: Icon::Symbol {
            name: "settings",
            tint: Tint::Gray,
        },
        shortcut: Some("⌘,"),
        action_title: "打开设置",
        // 动作在后台线程执行，不能直接开窗口；交给界面线程。
        run: || Ok(Outcome::OpenSettings),
    },
    Command {
        key: "quit",
        title: "退出 Helmp",
        subtitle: "Quit Helmp",
        keywords: &["退出 Helmp", "quit helmp"],
        icon: Icon::Symbol {
            name: "power",
            tint: Tint::Graphite,
        },
        shortcut: None,
        action_title: "退出",
        run: || std::process::exit(0),
    },
];

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
                    .filter_map(|keyword| matcher::score(query, keyword))
                    .max()?;
                Some(Item {
                    id: format!("builtin:{}", command.key),
                    title: command.title.into(),
                    subtitle: command.subtitle.into(),
                    badge: BADGE.into(),
                    score,
                    icon: command.icon.clone(),
                    accessories: command.shortcut.map(Accessory::keys).into_iter().collect(),
                    action: Action::new(command.run),
                    action_title: command.action_title.into(),
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
                    tint: Tint::Gray
                }
            );
        }
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
                panic!("{} 应使用内置符号", command.title);
            };
            let path = format!("icons/{name}.svg");
            assert!(gpui_kit::assets::AllAssets.load(&path).unwrap().is_some());
        }
    }
}
