# Contributing to Helmp

[English](#english) · [简体中文](#简体中文)

## English

Thanks for helping out. Helmp is small and moves fast, so a short issue before a large change saves everyone time.

### Development setup

- macOS 15 or later, Xcode (for the Metal shader compiler used by GPUI), and [rustup](https://rustup.rs).
- The toolchain is pinned in `rust-toolchain.toml`; rustup installs it the first time you run `cargo`. Please don't bump it in an unrelated PR.
- `gpui-kit` is pinned to an exact version and brings in GPUI. Don't add `gpui` as a separate dependency.

```bash
cargo run                         # start the app
HELMP_DATA_DIR=/tmp/helmp-dev cargo run   # use a throwaway data folder
HELMP_TRACE=1 cargo run           # log UI events to stderr
cargo run -- query wx             # headless query, no window
HELMP_SELF_TEST='wx|cb ' cargo run   # drive the real window through a few queries and exit
bash script/bundle.sh             # build target/debug/Helmp.app
```

Permissions are granted to whatever runs the binary: when you `cargo run` from a terminal, macOS asks about the terminal, not Helmp.

### Before you open a pull request

```bash
cargo test --release
cargo clippy --release --all-targets -- -D warnings
```

CI runs the same two commands on macOS. Tests that depend on your own machine (real agent sessions, real terminals) are marked `#[ignore]`; run them with `-- --ignored` when you touch that code.

### Code style

- Comments and doc comments are written in Chinese, like the rest of the code base. Explain *why*, not *what*.
- Keep changes surgical: touch only what the change needs. No drive-by refactors, renames or formatting of unrelated code.
- Fail loudly. Don't swallow errors silently; print to stderr or show a message in the panel.
- `query()` runs on every keystroke and must only read memory. Do IO in `start()` or on a background thread.
- Tests should say why the behavior matters (see the comments in existing tests), so they break when the intent breaks.
- New modules implement the `Provider` trait; see [docs/architecture.md](docs/architecture.md).
- The interface is bilingual. Write every user-visible string as `tr("中文", "English")` (or `trf!` with arguments) from `src/i18n.rs`, and update both `README.md` and `README.zh-CN.md` (and the matching `docs/` pages) when behavior changes.

### Commit messages

Follow the existing history: `<type>: <summary in Chinese>`, optionally with a scope.

```
feat: 剪贴板支持按 App 排除
fix: 首页「最近使用」旧记录不再显示空白格
feat(agents): Claude Code / Codex 会话切换器
```

Types in use: `feat`, `fix`, `design`, `docs`, `test`, `chore`. English summaries are fine if you don't write Chinese.

## 简体中文

谢谢你愿意帮忙。Helmp 还小、变化快，大改动之前先开个 issue 聊一下，能省掉很多来回。

### 开发环境

- macOS 15 及以上、Xcode（GPUI 需要它的 Metal 着色器编译器）、[rustup](https://rustup.rs)。
- 工具链版本锁在 `rust-toolchain.toml`，第一次运行 `cargo` 时 rustup 会自动安装。不相关的 PR 里请不要顺手升级它。
- `gpui-kit` 精确锁定版本，由它带出 GPUI，不要再单独声明 `gpui` 依赖。

```bash
cargo run                         # 启动
HELMP_DATA_DIR=/tmp/helmp-dev cargo run   # 用一个临时数据目录，不碰你自己的数据
HELMP_TRACE=1 cargo run           # 把界面事件打到 stderr
cargo run -- query wx             # 无头查询，不开窗口
HELMP_SELF_TEST='wx|cb ' cargo run   # 在真实窗口里走一遍这些查询后退出
bash script/bundle.sh             # 打包 target/debug/Helmp.app
```

权限是授给「运行这个程序的进程」的：在终端里 `cargo run` 时，系统问的是终端的权限，不是 Helmp 的。

### 提 PR 之前

```bash
cargo test --release
cargo clippy --release --all-targets -- -D warnings
```

CI 在 macOS 上跑的就是这两条。依赖本机环境的测试（真实的 agent 会话、真实的终端进程）标了 `#[ignore]`，改到相关代码时用 `-- --ignored` 跑一下。

### 代码风格

- 注释用中文，和现有代码保持一致。写「为什么」，不写「做了什么」。
- 外科手术式修改：只动这次需要动的地方，不顺手重构、改名或格式化无关代码。
- 大声失败：不要默默吞掉错误，打到 stderr，或者在面板里给出提示。
- `query()` 每次按键都会调用，只能读内存；IO 放到 `start()` 或后台线程里。
- 测试要写清楚「这个行为为什么重要」（参考现有测试里的注释），这样意图被破坏时它才会失败。
- 新模块实现 `Provider` trait，见 [docs/architecture.md](docs/architecture.md)。
- 界面是中英双语的：所有给用户看的文字都写成 `src/i18n.rs` 里的 `tr("中文", "English")`（带参数用 `trf!`）；行为有变化时，`README.md`、`README.zh-CN.md` 和 `docs/` 里对应的页面要一起更新。

### 提交信息

沿用现有历史的格式：`<类型>: <中文摘要>`，可以带范围。

```
feat: 剪贴板支持按 App 排除
fix: 首页「最近使用」旧记录不再显示空白格
feat(agents): Claude Code / Codex 会话切换器
```

在用的类型：`feat`、`fix`、`design`、`docs`、`test`、`chore`。
