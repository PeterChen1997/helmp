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
bash script/bundle.sh             # build target/debug/Helmp.app (debug is the default)
bash script/bundle.sh release     # build target/release/Helmp.app
```

Permissions are granted to whatever runs the binary: when you `cargo run` from a terminal, macOS asks about the terminal, not Helmp.

### Before you open a pull request

```bash
cargo test --release
cargo clippy --release --all-targets -- -D warnings
```

CI runs the same two commands on macOS. Tests that depend on your own machine (real agent sessions, real terminals) are marked `#[ignore]`; run them with `-- --ignored` when you touch that code.

### End-to-end tests

Every user-visible feature has end-to-end tests in `tests/e2e_*.rs`. They run the real `helmp` binary through "query → run", so they catch what unit tests can't: wiring, ranking against other providers, and the exact action a result performs. When you add or change a feature, add or update a test there: use the file for that area, or start `tests/e2e_<area>.rs` with `mod support;`.

The harness is in [`tests/support/mod.rs`](tests/support/mod.rs):

- `Helmp::new()` builds an isolated environment for one test: a temporary data folder and `HOME`, its own `CLAUDE_CONFIG_DIR` and `CODEX_HOME`, clipboard recording off, English UI, and `HELMP_DRY_RUN=1`. Adjust it with `.config(|c| …)` (merged into `config.json`), `.env(key, value)`, `.lang("zh")`, and write fixtures with `.write(path, content)`. Everything is deleted when the value is dropped.
- `h.query(text)` returns the rows (badge, title, subtitle, id); `h.find(text, |row| …)` returns the first matching row and prints all rows if none matches; `h.run(id, query, &["--action", "…"])` runs a result and returns its output; `h.cli(&[…])` / `h.cli_err(&[…])` run any other command and expect success / failure.
- `MockServer::start(|request| (status, body))` is a local HTTP server that records every request (`server.requests()`). Point the feature at it through its URL override (for example `HELMP_FX_URL`, `HELMP_PUBLIC_IP_URL`, or a model endpoint in the config) and assert on what was sent.

```rust
mod support;
use support::{Helmp, MockServer};

#[test]
fn currency_converts_with_ecb_rates() {
    // Say why this matters, so the test fails when the intent breaks:
    // rates come from the ECB, are cached, and the result names their date.
    let server = MockServer::start(|_| (200, ECB_XML.to_string()));
    let h = Helmp::new().env("HELMP_FX_URL", format!("{}/eurofxref-daily.xml", server.url));
    let row = h.find("100 usd to cny", |r| r.id == "calculator:fx:CNY");
    assert_eq!(row.title, "700.00 CNY");
    assert_eq!(server.requests().len(), 1);
}
```

Rules:

- **Every new side effect goes through `crate::effects::dry` first.** Anything that changes the user's environment (copy, paste, open a URL or file, run a command, quit a process, move a window, change a system setting, send a request to a real host) must start with
  `if let Some(outcome) = crate::effects::dry("<kind>", &detail) { return Ok(outcome); }`.
  In dry-run mode the action then only reports `[dry-run] <kind> <detail>`, and the test asserts on that exact line, e.g. `assert_eq!(h.run("calculator:unit:mi", "10 km to mi", &[]).trim(), "[dry-run] copy 6.21371");`.
- Tests must never touch the user's clipboard, Keychain, `~/.claude`, `~/.codex`, windows, processes or the real network. Secrets come from `HELMP_TEST_SECRETS` (honored only with an isolated data folder and dry-run), HTTP goes to a `MockServer`, and offline cases use a closed local port.
- When a feature depends on the machine (time, time zone, screens, Spotlight, a color picker), add a narrow test seam: an environment variable read only by that module and documented in its module comment, like `HELMP_NOW`, `HELMP_WINDOW_FIXTURE`, `HELMP_COLOR_FIXTURE` or `HELMP_FILES_ROOTS`. Set `TZ` when output depends on the time zone.
- A test that can't be made reliable everywhere gets `#[ignore = "<why, and the command to run it>"]` instead of being flaky.

### Code style

- Comments and doc comments are written in Chinese, like the rest of the code base. Explain *why*, not *what*.
- Keep changes surgical: touch only what the change needs. No drive-by refactors, renames or formatting of unrelated code.
- Fail loudly. Don't swallow errors silently; print to stderr or show a message in the panel.
- `query()` runs on every keystroke and must only read memory. Do IO in `start()` or on a background thread.
- Tests should say why the behavior matters (see the comments in existing tests), so they break when the intent breaks.
- New modules implement the `Provider` trait; see [docs/architecture.md](docs/architecture.md).
- The interface is bilingual. Write every user-visible string as `tr("中文", "English")` (or `trf!` with arguments) from `src/i18n.rs`, and update both `README.md` and `README.zh-CN.md` (and the matching `docs/` pages) when behavior changes.

### Releases (maintainers)

`script/release.sh` builds what goes on GitHub Releases. It runs `script/bundle.sh release`, re-signs the app with a Developer ID certificate (hardened runtime, secure timestamp, `resources/Helmp.entitlements`), notarizes and staples it, then makes a signed, notarized and stapled dmg (with an Applications shortcut) and a zip of the stapled app. Output goes to `target/dist/`: `Helmp-<version>.dmg`, `Helmp-<version>.zip` and `SHA256SUMS.txt`. The version comes from `Cargo.toml`.

```bash
bash script/release.sh --dry-run   # print each command instead of building, signing or notarizing
bash script/release.sh             # the real thing
```

It needs a "Developer ID Application" certificate in your keychain (or `HELMP_SIGN_IDENTITY`) and an App Store Connect API key for `notarytool`; `HELMP_NOTARY_KEY_ID`, `HELMP_NOTARY_ISSUER` and `HELMP_NOTARY_KEY_PATH` override the defaults at the top of the script. `--dry-run` still checks that the certificate and the key file exist. The script stops with `RELEASE_FAILED` if a tool or credential is missing or notarization isn't accepted, and prints the notary log in that case. Publishing the GitHub release itself is a separate, manual step.

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
bash script/bundle.sh             # 打包 target/debug/Helmp.app（默认是 debug）
bash script/bundle.sh release     # 打包 target/release/Helmp.app
```

权限是授给「运行这个程序的进程」的：在终端里 `cargo run` 时，系统问的是终端的权限，不是 Helmp 的。

### 提 PR 之前

```bash
cargo test --release
cargo clippy --release --all-targets -- -D warnings
```

CI 在 macOS 上跑的就是这两条。依赖本机环境的测试（真实的 agent 会话、真实的终端进程）标了 `#[ignore]`，改到相关代码时用 `-- --ignored` 跑一下。

### 端到端测试

每个用户能看到的功能，在 `tests/e2e_*.rs` 里都有端到端测试。它们用真实的 `helmp` 可执行文件走「查询 → 执行」，能发现单元测试发现不了的问题：模块有没有接上、和其他 provider 一起排序时排在哪、一条结果实际执行了什么。新增或修改功能时，要在这里加上或更新用例：放进对应领域的文件，或者新建 `tests/e2e_<领域>.rs`，开头写 `mod support;`。

测试框架在 [`tests/support/mod.rs`](tests/support/mod.rs)：

- `Helmp::new()` 为一个用例建一个隔离环境：临时数据目录和 `HOME`、单独的 `CLAUDE_CONFIG_DIR` 和 `CODEX_HOME`、关闭剪贴板记录、英文界面、`HELMP_DRY_RUN=1`。用 `.config(|c| …)`（合并进 `config.json`）、`.env(key, value)`、`.lang("zh")` 调整，用 `.write(路径, 内容)` 写夹具。值被 drop 时全部删除。
- `h.query(text)` 返回结果行（badge、标题、副标题、id）；`h.find(text, |row| …)` 返回第一条符合条件的行，找不到时把全部结果打出来；`h.run(id, query, &["--action", "…"])` 执行一条结果并返回输出；`h.cli(&[…])` / `h.cli_err(&[…])` 运行其他命令，分别要求成功 / 失败。
- `MockServer::start(|request| (状态码, 正文))` 是本机的假 HTTP 服务，记录每个请求（`server.requests()`）。通过功能自己的地址覆盖项（比如 `HELMP_FX_URL`、`HELMP_PUBLIC_IP_URL`，或配置里的模型地址）把请求指向它，再断言发出去的内容。

```rust
mod support;
use support::{Helmp, MockServer};

#[test]
fn currency_converts_with_ecb_rates() {
    // 写清楚这个行为为什么重要，意图被破坏时测试才会失败：
    // 汇率来自 ECB、会缓存，结果里写明汇率日期。
    let server = MockServer::start(|_| (200, ECB_XML.to_string()));
    let h = Helmp::new().env("HELMP_FX_URL", format!("{}/eurofxref-daily.xml", server.url));
    let row = h.find("100 usd to cny", |r| r.id == "calculator:fx:CNY");
    assert_eq!(row.title, "700.00 CNY");
    assert_eq!(server.requests().len(), 1);
}
```

规则：

- **新的副作用一律先过 `crate::effects::dry`。** 凡是会改动用户环境的动作（复制、粘贴、打开网址或文件、运行命令、结束进程、移动窗口、改系统设置、向真实主机发请求），开头都要写
  `if let Some(outcome) = crate::effects::dry("<动作>", &detail) { return Ok(outcome); }`。
  这样演练模式下它只回报 `[dry-run] <动作> <内容>`，测试直接断言这一行，例如 `assert_eq!(h.run("calculator:unit:mi", "10 km to mi", &[]).trim(), "[dry-run] copy 6.21371");`。
- 测试绝不能碰用户的剪贴板、钥匙串、`~/.claude`、`~/.codex`、窗口、进程和真实网络。密钥走 `HELMP_TEST_SECRETS`（只在隔离数据目录 + 演练模式下生效），HTTP 发往 `MockServer`，离线场景用一个没人监听的本机端口。
- 功能依赖本机环境（时间、时区、屏幕、Spotlight、取色器）时，加一个范围很窄的测试接缝：只在该模块里读取、并写进模块注释的环境变量，比如 `HELMP_NOW`、`HELMP_WINDOW_FIXTURE`、`HELMP_COLOR_FIXTURE`、`HELMP_FILES_ROOTS`。输出和时区有关时设好 `TZ`。
- 没法在所有机器上稳定通过的用例，标 `#[ignore = "<原因，以及怎么手动运行>"]`，不要留一个时好时坏的测试。

### 代码风格

- 注释用中文，和现有代码保持一致。写「为什么」，不写「做了什么」。
- 外科手术式修改：只动这次需要动的地方，不顺手重构、改名或格式化无关代码。
- 大声失败：不要默默吞掉错误，打到 stderr，或者在面板里给出提示。
- `query()` 每次按键都会调用，只能读内存；IO 放到 `start()` 或后台线程里。
- 测试要写清楚「这个行为为什么重要」（参考现有测试里的注释），这样意图被破坏时它才会失败。
- 新模块实现 `Provider` trait，见 [docs/architecture.md](docs/architecture.md)。
- 界面是中英双语的：所有给用户看的文字都写成 `src/i18n.rs` 里的 `tr("中文", "English")`（带参数用 `trf!`）；行为有变化时，`README.md`、`README.zh-CN.md` 和 `docs/` 里对应的页面要一起更新。

### 发布（维护者）

GitHub Releases 上的安装包由 `script/release.sh` 生成：它先跑 `script/bundle.sh release`，再用 Developer ID 证书重新签名（hardened runtime、安全时间戳、`resources/Helmp.entitlements`），公证并 staple；然后打一个签名、公证、staple 过的 dmg（带「应用程序」快捷方式），再把 staple 后的 App 打成 zip。产物在 `target/dist/`：`Helmp-<版本>.dmg`、`Helmp-<版本>.zip` 和 `SHA256SUMS.txt`，版本号取自 `Cargo.toml`。

```bash
bash script/release.sh --dry-run   # 只打印每一步要执行的命令，不构建、不签名、不提交公证
bash script/release.sh             # 正式打包
```

需要钥匙串里有「Developer ID Application」证书（或用 `HELMP_SIGN_IDENTITY` 指定），以及给 `notarytool` 用的 App Store Connect API 密钥；`HELMP_NOTARY_KEY_ID`、`HELMP_NOTARY_ISSUER`、`HELMP_NOTARY_KEY_PATH` 可以覆盖脚本开头的默认值。`--dry-run` 也会先检查证书和密钥文件是否存在。缺工具、缺证书或密钥、公证没通过时，脚本会以 `RELEASE_FAILED` 退出，公证失败还会打印公证日志。在 GitHub 上创建 release 并上传产物是另外手动做的一步。

### 提交信息

沿用现有历史的格式：`<类型>: <中文摘要>`，可以带范围。

```
feat: 剪贴板支持按 App 排除
fix: 首页「最近使用」旧记录不再显示空白格
feat(agents): Claude Code / Codex 会话切换器
```

在用的类型：`feat`、`fix`、`design`、`docs`、`test`、`chore`。
