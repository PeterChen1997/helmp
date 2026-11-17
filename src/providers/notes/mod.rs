//! 便签插件：快速新建吸边、边缘书签队列、悬停展开、原地编辑与 Scope 管理。
//!
//! - `s 内容`（或 `note ` / `便签 `）在启动器里快速输入并回车直接新建便签，吸到主显示器右侧队尾。
//! - Scope「便签」列出全部便签（含已归档），支持正文与拼音搜索；⌘K 菜单提供「钉回桌面」「删除」。
//! - 桌面便签窗口、队列重排、全局快捷键与菜单栏联动见 [`window`]。

mod native;
mod screen;
pub mod store;
mod window;

pub use window::{
    NotesRequest, all_hidden, dock_all, flush_on_quit, handle, menu_generation,
    restore_on_launch, self_test_begin_edit, self_test_clear, self_test_debug_state,
    self_test_select, self_test_target, toggle_visibility,
};

use std::cmp::Reverse;

use crate::i18n::{tr, trf};
use crate::matcher;
use crate::provider::{
    Accessory, Action, Block, Detail, Icon, Item, NamedAction, Outcome, Provider, Scope, Tint, Tone,
};
use store::{Edge, Note, Placement};

const PROVIDER_ID: &str = "notes";
const PREFIXES: [&str; 3] = ["s ", "note ", "便签 "];
const TINT: Tint = Tint::Orange;
const ENTRY_KEYWORDS: [&str; 5] = ["便签", "bq", "notes", "note", "sticky"];
const NEW_KEYWORDS: [&str; 4] = ["新建便签", "记便签", "new note", "create note"];

#[derive(Default)]
pub struct Notes {
    #[cfg(test)]
    snapshot_override: Option<Vec<Note>>,
}

impl Notes {
    #[cfg(test)]
    fn with_snapshot(notes: Vec<Note>) -> Self {
        Self {
            snapshot_override: Some(notes),
        }
    }

    fn current_notes(&self) -> Vec<Note> {
        #[cfg(test)]
        if let Some(notes) = &self.snapshot_override {
            return notes.clone();
        }
        window::snapshot()
    }
}

impl Provider for Notes {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn query(&self, query: &str) -> Vec<Item> {
        let text = query.trim();
        if text.is_empty() {
            return Vec::new();
        }
        let notes = self.current_notes();
        let mut items = Vec::new();
        if let Some(score) = best(text, &ENTRY_KEYWORDS) {
            let count = notes.len();
            items.push(Item {
                id: "notes:open".into(),
                title: tr("便签", "Notes").into(),
                subtitle: trf!("管理 {count} 条桌面便签与归档", "Manage {count} sticky notes"),
                badge: badge().into(),
                score,
                icon: symbol("sticky-note"),
                action: Action::enter_scope(PROVIDER_ID),
                action_title: tr("打开", "Open").into(),
                ..Item::blank()
            });
        }
        if let Some(score) = best(text, &NEW_KEYWORDS) {
            items.push(new_note_item(score));
        }
        for note in &notes {
            if let Some(score) = note_match_score(note, text) {
                items.push(note_item(note, score.min(700)));
            }
        }
        items
    }

    fn scope(&self) -> Option<Scope> {
        let count = self.current_notes().len();
        Some(Scope {
            id: PROVIDER_ID.into(),
            label: tr("便签", "Notes").into(),
            tint: TINT,
            prefixes: PREFIXES.iter().map(|p| p.to_string()).collect(),
            placeholder: trf!(
                "输入内容回车新建便签，或搜索 {count} 条便签",
                "Type to create a note, or search {count} notes"
            ),
            filters: Vec::new(),
        })
    }

    fn query_scoped(&self, query: &str, _filter: &str) -> Vec<Item> {
        let notes = self.current_notes();
        let text = query.trim();
        let mut items = Vec::new();

        if !text.is_empty() {
            items.push(quick_create_item(text.to_string(), 20_000));
        }

        let mut matched: Vec<(&Note, u32)> = notes
            .iter()
            .filter_map(|note| {
                if text.is_empty() {
                    Some((note, 0))
                } else {
                    note_match_score(note, text).map(|score| (note, score))
                }
            })
            .collect();

        matched.sort_by_key(|(note, score)| {
            (
                Reverse(*score),
                is_archived(note),
                Reverse(note.updated_at),
            )
        });

        let total = matched.len();
        for (rank, (note, _)) in matched.into_iter().enumerate() {
            let score = 10_000_u32.saturating_sub(rank as u32);
            items.push(note_item(note, score));
        }

        if text.is_empty() {
            let new_score = if total == 0 { 100 } else { 1 };
            items.push(new_note_item(new_score));
        }
        items
    }
}

fn badge() -> &'static str {
    tr("便签", "Notes")
}

fn symbol(name: &'static str) -> Icon {
    Icon::Symbol { name, tint: TINT }
}

fn best(text: &str, keywords: &[&str]) -> Option<u32> {
    keywords
        .iter()
        .filter_map(|keyword| matcher::score(text, keyword))
        .max()
}

fn is_archived(note: &Note) -> bool {
    matches!(note.placement, Placement::Archived)
}

fn note_match_score(note: &Note, query: &str) -> Option<u32> {
    let first = window::summary_title(&note.text);
    let title_score = matcher::score(query, &first);
    let body_score = note
        .text
        .to_lowercase()
        .contains(&query.to_lowercase())
        .then_some(400);
    title_score.max(body_score)
}

fn quick_create_item(text: String, score: u32) -> Item {
    let preview = window::summary_title(&text);
    let create_text = text.clone();
    Item {
        id: "notes:quick-create".into(),
        title: trf!("新建并吸边：{preview}", "New Docked Note: {preview}"),
        subtitle: tr(
            "回车新建便签并直接停靠到主显示器右侧队尾",
            "Press Return to create and dock to the right edge of the main screen",
        )
        .into(),
        badge: badge().into(),
        score,
        icon: symbol("sticky-note-plus"),
        action_title: tr("贴边新建", "Create & Dock").into(),
        action: Action::new(move || {
            Ok(Outcome::Notes(NotesRequest::Create {
                text: create_text.clone(),
                docked: true,
                enter_edit: false,
            }))
        }),
        ..Item::blank()
    }
}

/// 空白便签入口（也是全局快捷键 `Command::RunItem { id: "notes:new", query: "新建便签" }` 的目标）。
fn new_note_item(score: u32) -> Item {
    Item {
        id: "notes:new".into(),
        title: tr("新建便签", "New Note").into(),
        subtitle: tr(
            "在桌面新建一张空白便签并立即编辑",
            "Create a blank note on your desktop and start typing",
        )
        .into(),
        badge: badge().into(),
        score,
        icon: symbol("sticky-note-plus"),
        accessories: crate::hotkey::item_hint_keys("notes:new"),
        action_title: tr("新建", "New").into(),
        action: Action::new(|| {
            Ok(Outcome::Notes(NotesRequest::Create {
                text: String::new(),
                docked: true,
                enter_edit: true,
            }))
        }),
        ..Item::blank()
    }
}

fn note_item(note: &Note, score: u32) -> Item {
    let title = if note.text.trim().is_empty() {
        tr("空白便签", "Empty Note").to_string()
    } else {
        window::summary_title(&note.text)
    };
    let (status_tag, tone, subtitle) = match &note.placement {
        Placement::Docked { edge, order, .. } => {
            let side = match edge {
                Edge::Left => tr("左侧队列", "Left Dock"),
                Edge::Right => tr("右侧队列", "Right Dock"),
            };
            (
                tr("队列", "Docked"),
                Tone::Running,
                format!("{side} #{} · {}", order + 1, tr("置顶吸边", "Pinned & Docked")),
            )
        }
        Placement::Floating { .. } => (
            tr("浮窗", "Floating"),
            Tone::Done,
            tr("桌面自由浮窗", "Floating on Desktop").into(),
        ),
        Placement::Archived => (
            tr("归档", "Archived"),
            Tone::Neutral,
            tr("已从桌面关闭（可重新钉回桌面）", "Closed from desktop (can pin back)").into(),
        ),
    };

    let pin_id = note.id.clone();
    let pin_action = Action::new(move || Ok(Outcome::Notes(NotesRequest::PinToDesktop(pin_id.clone()))));

    let delete_id = note.id.clone();
    let delete_action =
        Action::new(move || Ok(Outcome::Notes(NotesRequest::Delete(delete_id.clone()))))
            .keep_open();

    let copy_action = Action::copy(note.text.clone());

    let primary_title = if is_archived(note) {
        tr("钉回桌面", "Pin to Desktop")
    } else {
        tr("显示并编辑", "Show & Edit")
    };

    let detail_body = if note.text.trim().is_empty() {
        tr("（空白便签）", "(Empty note)").to_string()
    } else {
        note.text.clone()
    };

    Item {
        id: format!("{PROVIDER_ID}:{}", note.id),
        title: title.clone(),
        subtitle: subtitle.clone(),
        badge: badge().into(),
        score,
        icon: symbol("sticky-note"),
        accessories: vec![Accessory::Tag {
            text: status_tag.into(),
            tone,
        }],
        action: pin_action.clone(),
        action_title: primary_title.into(),
        actions: vec![
            NamedAction::new(tr("钉回桌面", "Pin to Desktop"), None, pin_action),
            NamedAction::new(tr("复制内容", "Copy Text"), Some("⌘C"), copy_action),
            NamedAction::new(tr("删除", "Delete"), Some("⌘⌫"), delete_action),
        ],
        detail: Some(Detail {
            icon: symbol("sticky-note"),
            title,
            tag: Some((status_tag.into(), tone)),
            blocks: vec![Block::Text(detail_body)],
            metadata: vec![(tr("位置", "Status").into(), subtitle)],
            ..Detail::default()
        }),
        ..Item::blank()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_uses_s_prefix_and_quick_create_ranks_first() {
        // 每个单测注入独立快照，避免并行执行时互相覆盖全局 SNAPSHOT。
        // 输入 `s 内容` 进入便签 Scope 时，第一项必须是「新建并吸边」，回车直接吸到主屏右侧队尾。
        let provider = Notes::with_snapshot(vec![store::new_note(
            "旧便签内容",
            100,
            Placement::Archived,
        )]);
        let scope = provider.scope().unwrap();
        assert!(scope.prefixes.contains(&"s ".to_string()));

        let items = provider.query_scoped("周五前归档旧接口", "");
        assert!(!items.is_empty());
        assert_eq!(items[0].id, "notes:quick-create");
        assert_eq!(
            items[0].action.run().unwrap(),
            Outcome::Notes(NotesRequest::Create {
                text: "周五前归档旧接口".into(),
                docked: true,
                enter_edit: false,
            })
        );
    }

    #[test]
    fn scope_lists_all_notes_with_pin_and_delete_actions_and_searches_body() {
        // 便签 Scope 必须同时列出桌面上和已归档的便签，支持正文搜索，且 ⌘K 包含「钉回桌面」「删除」。
        let mut active = store::new_note(
            "接口联调地址\nPOST /v1/notes",
            200,
            Placement::Docked {
                edge: Edge::Right,
                display: "main".into(),
                order: 0,
            },
        );
        active.id = "id-active".into();
        let mut archived = store::new_note("上周周报草稿", 100, Placement::Archived);
        archived.id = "id-archived".into();
        let provider = Notes::with_snapshot(vec![active, archived]);

        let all = provider.query_scoped("", "");
        assert_eq!(all.len(), 3); // 2 notes + "新建便签"
        assert_eq!(all[0].id, "notes:id-active");
        assert_eq!(all[1].id, "notes:id-archived");
        let cmd_titles: Vec<&str> = all[1].actions.iter().map(|a| a.title.as_str()).collect();
        assert!(cmd_titles.contains(&"钉回桌面"));
        assert!(cmd_titles.contains(&"删除"));
        assert_eq!(
            all[1].actions[0].action.run().unwrap(),
            Outcome::Notes(NotesRequest::PinToDesktop("id-archived".into()))
        );
        assert_eq!(
            all[1].actions[2].action.run().unwrap(),
            Outcome::Notes(NotesRequest::Delete("id-archived".into()))
        );
        assert!(all[1].actions[2].action.keeps_open());

        let search = provider.query_scoped("周报", "");
        assert!(search.iter().any(|item| item.id == "notes:id-archived"));
        assert!(!search.iter().any(|item| item.id == "notes:id-active"));
    }

    #[test]
    fn symbol_icons_exist_in_bundled_assets() {
        use gpui_kit::AssetSource as _;
        for name in ["sticky-note", "sticky-note-plus", "x"] {
            let path = format!("icons/{name}.svg");
            assert!(
                gpui_kit::assets::AllAssets.load(&path).unwrap().is_some(),
                "缺少图标 {path}"
            );
        }
    }

    #[test]
    fn s_prefix_resolves_to_notes_without_conflict() {
        // 确保 `s ` 前缀在完整注册表下直接进入便签 Scope，没有被任何其他 provider 抢占。
        let engine = crate::engine::Engine::new(crate::providers::all());
        let (scope, rest) = engine
            .resolve_prefix("s 周五前归档旧接口")
            .expect("`s ` 前缀应命中 notes");
        assert_eq!(scope.id, "notes");
        assert_eq!(rest, "周五前归档旧接口");
    }

    #[test]
    fn independent_snapshots_do_not_interfere_across_providers() {
        // 确认每个测试注入的独立快照互相隔离，不会因为共享全局状态而串台。
        let mut a = store::new_note("便签甲", 10, Placement::Archived);
        a.id = "only-a".into();
        let mut b = store::new_note("便签乙", 20, Placement::Archived);
        b.id = "only-b".into();
        let pa = Notes::with_snapshot(vec![a]);
        let pb = Notes::with_snapshot(vec![b]);

        let ra = pa.query_scoped("", "");
        let rb = pb.query_scoped("", "");
        assert!(ra.iter().any(|i| i.id == "notes:only-a"));
        assert!(!ra.iter().any(|i| i.id == "notes:only-b"));
        assert!(rb.iter().any(|i| i.id == "notes:only-b"));
        assert!(!rb.iter().any(|i| i.id == "notes:only-a"));
    }
}
