//! 内置命令：退出 Helmp、检查更新。也是最小的 provider 示例。
//!
//! Helmp 自己的设置不进搜索结果（容易和 macOS 的「系统设置」混在一起）：面板右下角提示 ⌘,，菜单栏也有入口。

use crate::i18n::tr;
use crate::matcher;
use crate::provider::{Action, Icon, Item, Outcome, Provider, Tint};

/// 界面文字存 (中文, 英文)，查询时按当前语言取；匹配只看 `keywords`（中英都在里面），与界面语言无关。
struct Command {
    key: &'static str,
    title: (&'static str, &'static str),
    subtitle: (&'static str, &'static str),
    keywords: &'static [&'static str],
    icon: Icon,
    action_title: (&'static str, &'static str),
    run: fn() -> anyhow::Result<Outcome>,
}

const COMMANDS: [Command; 2] = [
    Command {
        key: "check-updates",
        title: ("检查 Helmp 更新", "Check for Helmp Updates"),
        subtitle: (
            "打开设置 › 关于并立即检查新版本",
            "Opens Settings › About and checks for a new version",
        ),
        keywords: &[
            "检查更新",
            "helmp 更新",
            "更新 helmp",
            "check for updates",
            "update helmp",
            "helmp update",
        ],
        icon: Icon::Symbol {
            name: "refresh-cw",
            tint: Tint::Green,
        },
        action_title: ("检查更新", "Check for Updates"),
        // 动作在后台线程（或命令行）执行：只做标记，由界面的轮询任务发起检查，再翻到关于页看结果。
        run: || {
            crate::updates::request_check();
            Ok(Outcome::OpenSettingsPage(crate::settings::Page::About))
        },
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
                    .filter_map(|keyword| matcher::score(query, keyword))
                    .max()?;
                Some(Item {
                    id: format!("builtin:{}", command.key),
                    title: tr(command.title.0, command.title.1).into(),
                    subtitle: tr(command.subtitle.0, command.subtitle.1).into(),
                    badge: tr("命令", "Commands").into(),
                    score,
                    icon: helmp_icon().unwrap_or_else(|| command.icon.clone()),
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
    fn helmp_settings_stay_out_of_search_results() {
        // 打「设置 / settings」的人多半要 macOS 的系统设置，混一条 Helmp 设置容易点错；入口在面板右下角的 ⌘,。
        for query in ["设置", "settings", "setting", "pref", "helmp 设置", "helmp settings"] {
            assert!(find(query, "builtin:settings").is_none(), "{query:?}");
        }
    }

    #[test]
    fn check_updates_command_shows_about_page() {
        // 结果显示在关于页：动作必须把用户带过去，而不是在后台默默检查。
        for query in ["检查更新", "check for updates", "update helmp"] {
            let item = find(query, "builtin:check-updates").unwrap_or_else(|| panic!("{query:?}"));
            assert_eq!(
                item.action.run().unwrap(),
                Outcome::OpenSettingsPage(crate::settings::Page::About)
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
                panic!("{} 应使用内置符号", command.key);
            };
            let path = format!("icons/{name}.svg");
            assert!(gpui_kit::assets::AllAssets.load(&path).unwrap().is_some());
        }
    }

    #[test]
    fn english_ui_shows_english_text_but_still_matches_chinese() {
        // 切到英文后标题、分组、动作名要跟着变；但匹配只看关键词，
        // 英文界面下输入中文「退出」仍要找得到，中文界面下输入英文也一样。
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let quit = find("退出", "builtin:quit").expect("英文界面输入中文应找到退出");
            assert_eq!(quit.title, "Quit Helmp");
            assert_eq!(quit.badge, "Commands");
            assert_eq!(quit.action_title, "Quit");
        });
        let quit = find("quit", "builtin:quit").expect("中文界面输入英文应找到退出");
        assert_eq!(quit.title, "退出 Helmp");
        assert_eq!(quit.badge, "命令");
    }
}
