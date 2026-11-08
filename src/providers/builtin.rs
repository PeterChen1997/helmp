//! 内置命令：退出等。也是最小的 provider 示例。

use crate::matcher;
use crate::provider::{Action, Item, Provider};

pub struct Builtin;

impl Provider for Builtin {
    fn id(&self) -> &'static str {
        "builtin"
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let quit = ["退出 Helmp", "quit helmp"]
            .iter()
            .filter_map(|name| matcher::score(query, name))
            .max();
        match (query.trim().is_empty(), quit) {
            (false, Some(score)) => vec![Item {
                id: "builtin:quit".into(),
                title: "退出 Helmp".into(),
                subtitle: "Quit Helmp".into(),
                badge: "命令".into(),
                score,
                action: Action::new(|| std::process::exit(0)),
                ..Item::blank()
            }],
            _ => Vec::new(),
        }
    }
}
