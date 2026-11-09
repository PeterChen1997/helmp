//! 插件（Provider）接口：所有功能模块都通过它接入启动器。
//!
//! 统一视图语法（见 design/canvas 的「统一的面板结构」）：插件只返回数据，界面由框架渲染。
//! - **列表**：`Item` 的图标、标题、副标题、右侧附件（标签 / 文字 / 按键）。
//! - **详情**：选中项带 `detail` 时，面板变成「列表 + 详情」。
//! - **表单**：选中项带 `form` 时，面板变成「表单 + 详情」，回车把表单值交给 `Action`。
//! - **操作**：主操作 `action` 对应回车，`actions` 进 ⌘K 菜单；详情区里不放按钮。
//! - **范围**：插件可声明 `Scope`（如「剪贴板」），输入前缀或命令进入后只查这一个插件。
//!
//! 约定：
//! - `start` 在后台线程调用一次，负责建索引、起监听线程；可以阻塞到首次索引完成。
//! - `query` / `query_scoped` / `suggestions` 每次输入变化都会调用，必须只读内存、毫秒级返回。
//! - 每个 `Item.id` 在同一 provider 内稳定唯一，格式 `<provider>:<domain id>`。

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use crate::i18n::{tr, trf};

/// 执行一个结果后的去向。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 完成，面板保持隐藏。
    Done,
    /// 完成，并重新打开面板展示一段文字（脚本输出、错误说明）。
    Message(String),
    /// 完成，并在面板里展示一份详情（翻译结果、OCR 结果）。表单视图下面板不关，直接显示在右侧。
    Show(Detail),
    /// 进入某个插件的范围（如从首页的「剪贴板历史」进入剪贴板）。
    EnterScope(&'static str),
    /// 进入范围并带上初始输入（如把识别出的文字带进翻译）。
    EnterScopeWith(&'static str, String),
    /// 打开设置窗口（动作在后台线程执行，不能直接开窗口，交给界面线程做）。
    OpenSettings,
}

/// 执行时框架传给动作的输入：表单视图下是各字段的值。
#[derive(Debug, Clone, Default)]
pub struct ActionInput {
    pub form: BTreeMap<String, String>,
}

type RunFn = dyn Fn(&ActionInput) -> Result<Outcome> + Send + Sync;

/// 结果被选中时要做的事。启动器先隐藏面板（表单视图除外），再在后台线程执行它。
#[derive(Clone)]
pub struct Action {
    run: Arc<RunFn>,
    /// 执行时不隐藏面板，完成后原地刷新列表（如删除一条剪贴板记录）。
    keep_open: bool,
}

impl Action {
    /// 不需要输入的动作。
    pub fn new(f: impl Fn() -> Result<Outcome> + Send + Sync + 'static) -> Self {
        Self {
            run: Arc::new(move |_| f()),
            keep_open: false,
        }
    }

    /// 需要表单输入的动作。
    pub fn with_input(f: impl Fn(&ActionInput) -> Result<Outcome> + Send + Sync + 'static) -> Self {
        Self {
            run: Arc::new(f),
            keep_open: false,
        }
    }

    /// 用 macOS `open` 打开文件、.app 或 URL（含 `claude://`、`codex://` 深链）。
    pub fn open(target: impl Into<String>) -> Self {
        let target = target.into();
        Self::new(move || {
            let status = std::process::Command::new("open").arg(&target).status()?;
            anyhow::ensure!(
                status.success(),
                trf!("open {target} 失败：{status}", "Couldn't open {target}: {status}")
            );
            Ok(Outcome::Done)
        })
    }

    /// 进入某个插件范围。
    pub fn enter_scope(scope: &'static str) -> Self {
        Self::new(move || Ok(Outcome::EnterScope(scope)))
    }

    /// 标记为「执行时不隐藏面板、完成后原地刷新」。
    pub fn keep_open(mut self) -> Self {
        self.keep_open = true;
        self
    }

    pub fn keeps_open(&self) -> bool {
        self.keep_open
    }

    /// 把文字写进系统剪贴板。用 pbcopy 并强制 UTF-8：Helmp 可能从 LANG 为空的环境启动，
    /// 那样 pbcopy 会按 C locale 把中文写成问号。
    pub fn copy(text: impl Into<String>) -> Self {
        let text = text.into();
        Self::new(move || {
            use std::io::Write as _;
            let mut child = std::process::Command::new("pbcopy")
                .env("LANG", "en_US.UTF-8")
                .env("LC_ALL", "en_US.UTF-8")
                .stdin(std::process::Stdio::piped())
                .spawn()?;
            child
                .stdin
                .take()
                .ok_or_else(|| anyhow::anyhow!(tr("pbcopy 没有 stdin", "pbcopy has no stdin")))?
                .write_all(text.as_bytes())?;
            let status = child.wait()?;
            anyhow::ensure!(
                status.success(),
                trf!("写入剪贴板失败：{status}", "Couldn't copy to the clipboard: {status}")
            );
            Ok(Outcome::Done)
        })
    }

    /// 在访达中选中这个文件或目录。
    pub fn reveal(path: impl Into<String>) -> Self {
        let path = path.into();
        Self::new(move || {
            let status = std::process::Command::new("open")
                .arg("-R")
                .arg(&path)
                .status()?;
            anyhow::ensure!(
                status.success(),
                trf!("在访达中显示 {path} 失败：{status}", "Couldn't show {path} in Finder: {status}")
            );
            Ok(Outcome::Done)
        })
    }

    pub fn run(&self) -> Result<Outcome> {
        (self.run)(&ActionInput::default())
    }

    pub fn run_with(&self, input: &ActionInput) -> Result<Outcome> {
        (self.run)(input)
    }
}

/// 动作没有可比较的内容：同一个闭包（同一次构造的克隆）才算相等。
impl PartialEq for Action {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.run, &other.run) && self.keep_open == other.keep_open
    }
}

impl Eq for Action {}

impl fmt::Debug for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Action(..)")
    }
}

/// ⌘K 菜单里的次级操作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedAction {
    pub title: String,
    /// 显示用的快捷键，如 `⌘↵`、`⌘⇧C`。框架按它绑定按键。
    pub shortcut: Option<String>,
    pub action: Action,
}

impl NamedAction {
    pub fn new(title: impl Into<String>, shortcut: Option<&str>, action: Action) -> Self {
        Self {
            title: title.into(),
            shortcut: shortcut.map(str::to_string),
            action,
        }
    }
}

/// 图标底块颜色，对应设计稿里的系统色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Tint {
    Blue,
    Indigo,
    Purple,
    Orange,
    Teal,
    Green,
    Red,
    Gray,
    Graphite,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Icon {
    #[default]
    None,
    /// 本地图片（png / jpg）。
    Image(PathBuf),
    /// `.app` 包或任意文件的系统图标，由框架解析并缓存成 PNG。
    File(PathBuf),
    /// 内置符号：Lucide 图标名（kebab-case，如 `scan-text`），白色画在彩色底块上。
    Symbol { name: &'static str, tint: Tint },
}

/// 状态色调，对应设计稿：等你（橙）、运行中（蓝）、已完成（绿）、空闲 / 中性（灰）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Waiting,
    Running,
    Done,
    // 插件 API 的一部分，界面已支持；内置插件暂时没用到。
    #[allow(dead_code)]
    Accent,
}

/// 列表行右侧的附件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Accessory {
    // 插件 API 的一部分，界面已支持；内置插件暂时没用到。
    #[allow(dead_code)]
    Text(String),
    Tag {
        text: String,
        tone: Tone,
    },
    /// 快捷键提示，每个元素一个键帽，如 `["⌘", "⇧", "V"]`。
    Keys(Vec<String>),
}

impl Accessory {
    /// 从显示用的快捷键构造键帽：`Accessory::keys("⌘⇧V")` → ⌘ ⇧ V 三个键帽。
    pub fn keys(shortcut: &str) -> Self {
        Self::Keys(keycaps(shortcut))
    }
}

/// 把 `⌘⇧V`、`⌥Space` 拆成一个个键帽。
pub fn keycaps(shortcut: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for ch in shortcut.chars() {
        if "⌘⇧⌥⌃↵⌫⇥".contains(ch) {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
            out.push(ch.to_string());
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// 详情区的内容块，自上而下排列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Text(String),
    /// 等宽文本（命令、代码、脚本输出）。
    Code(String),
    /// 带小标题的引用框（如「最后一条消息」）。
    Quote {
        label: String,
        text: String,
    },
    Image(PathBuf),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detail {
    pub icon: Icon,
    pub title: String,
    pub tag: Option<(String, Tone)>,
    pub blocks: Vec<Block>,
    /// 底部的键值表，如 项目 / 分支 / 来源。
    pub metadata: Vec<(String, String)>,
    /// 结果的操作（执行后展示的详情才用）：第一个是回车的主操作（如「复制译文」），其余进 ⌘K。
    pub actions: Vec<NamedAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    Text {
        placeholder: String,
        multiline: bool,
    },
    /// 掩码输入（密码类参数）。
    Secret { placeholder: String },
    /// (显示名, 值)
    Select { options: Vec<(String, String)> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub key: String,
    pub label: String,
    pub kind: FieldKind,
    /// 初始值（如从查询里解析出的参数、选中的文字）。
    pub value: String,
    pub required: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Form {
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone)]
pub struct Item {
    /// `<provider>:<domain id>`，同一 provider 内稳定唯一。
    pub id: String,
    pub title: String,
    pub subtitle: String,
    /// 分组名，列表里作为小节标题，如「应用」「Agent 会话」「剪贴板」。
    pub badge: String,
    /// 越大越靠前。匹配分见 `matcher::score`，provider 可以再加权。
    pub score: u32,
    /// 主操作，回车执行。
    pub action: Action,
    /// 主操作在状态栏里的名字，如「打开」「跳转到会话」「粘贴到微信」。
    pub action_title: String,
    pub icon: Icon,
    pub accessories: Vec<Accessory>,
    /// ⌘K 菜单里的次级操作。
    pub actions: Vec<NamedAction>,
    pub detail: Option<Detail>,
    pub form: Option<Form>,
}

impl Item {
    /// 新字段的默认值，供 `Item { id, title, .., ..Item::blank() }` 使用。
    pub fn blank() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            subtitle: String::new(),
            badge: String::new(),
            score: 0,
            action: Action::new(|| Ok(Outcome::Done)),
            action_title: tr("打开", "Open").into(),
            icon: Icon::None,
            accessories: Vec::new(),
            actions: Vec::new(),
            detail: None,
            form: None,
        }
    }
}

/// 插件的专属范围：进入后搜索栏左侧显示彩色标签，只查询这个插件。
#[derive(Debug, Clone)]
pub struct Scope {
    pub id: &'static str,
    pub label: String,
    pub tint: Tint,
    /// 输入这些前缀（含结尾空格）时自动进入，如 `"cb "`。
    pub prefixes: Vec<String>,
    pub placeholder: String,
    /// 搜索栏右侧的分段筛选，如 全部 / Claude / Codex。第一个是默认。
    pub filters: Vec<String>,
}

pub trait Provider: Send + Sync {
    /// 短标识，如 `apps`、`agents`。也是 `Item.id` 的前缀。
    fn id(&self) -> &'static str;

    /// 建索引 / 起监听。后台线程调用一次，可阻塞。
    fn start(&self) -> Result<()> {
        Ok(())
    }

    /// 全局搜索：返回与 `query` 匹配的结果。空查询时一般返回空，首页内容走 `suggestions`。
    fn query(&self, query: &str) -> Vec<Item>;

    /// 声明自己的范围。没有就不能被「进入」。
    fn scope(&self) -> Option<Scope> {
        None
    }

    /// 范围内的查询：`query` 已去掉前缀，`filter` 是选中的分段筛选（默认第一个）。
    fn query_scoped(&self, query: &str, _filter: &str) -> Vec<Item> {
        self.query(query)
    }

    /// 首页（空查询）要展示的项，如「等你处理」的会话、「快捷命令」。
    fn suggestions(&self) -> Vec<Item> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::keycaps;

    #[test]
    fn splits_shortcuts_into_keycaps() {
        // 列表和状态栏按键帽显示快捷键，拆错会显示成一整块看不懂的字符串。
        assert_eq!(keycaps("⌘⇧V"), ["⌘", "⇧", "V"]);
        assert_eq!(keycaps("⌥Space"), ["⌥", "Space"]);
    }
}
