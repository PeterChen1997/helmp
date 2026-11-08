# 唤 · Huan

macOS 原生启动器（Rust + GPUI），目标是 Spotlight 式应用启动、agent 会话切换、剪贴板与截图，插件机制。
调研与决策见 `~/code_hub/_research/launcher/`。

## 功能

| 模块 | 用法 | 说明 |
|---|---|---|
| 应用搜索 | `wx` / `weixin` / `微信` | 中文名、全拼、首字母；目录变化自动重建索引 |
| agent 切换器 | 空查询、`agent`、`claude`、`codex` | 列出本机 Claude Code / Codex 会话，「等你」的排最前，回车跳到会话 |
| 剪贴板历史 | `cb ` 前缀 | 文本、图片、文件，不限条数；跳过密码管理器标记的内容 |
| 截图与 OCR | `截图`、`ocr` | 区域截图、保存、识别文字（Vision，中英文）；需录屏权限 |
| 脚本插件 | 标题前缀 + 参数 | 兼容 Raycast Script Commands；目录 `~/Library/Application Support/huan/scripts/` 或 `HUAN_SCRIPT_DIRS` |

面板：`⌥Space` 唤起，↑↓ 选择，⏎ 执行，Esc 隐藏。隐藏超过 90 秒、或执行过结果后再唤起，输入框清空；90 秒内重开保留上次输入并全选（`HUAN_RESET_AFTER_SECS` 调整，0 为每次清空）。唤起时自动切到英文键盘 ABC，隐藏时切回原输入法（`HUAN_KEEP_INPUT_SOURCE=1` 关闭）。

## 运行

```bash
cargo run
```

无头查询（不开窗口，测试和排查用）：

```bash
cargo run -- query wx
HUAN_DRY_RUN=1 cargo run -- run <结果 id> agent   # agent 跳转只打印不执行
```

图形自检（真实窗口里走一遍显示、查询、执行、隐藏）与跟踪日志：

```bash
HUAN_SELF_TEST='wx|cb |截图' HUAN_SELF_TEST_EXEC=当前时间 cargo run
HUAN_TRACE=1 cargo run
```

## 打包

```bash
script/bundle.sh          # 生成 target/debug/Huan.app（ad-hoc 签名，LSUIElement）
script/bundle.sh release
```

## 工具链

`rust-toolchain.toml` 锁定 1.99.1（`gpui-pre-macos 0.3.11` 用到 `Option::map_or_default`，1.96 编译不过）。
`gpui-kit` 精确锁定 `=0.9.0`，由它带出 `gpui-pre 0.3.11`，不要再单独声明 `gpui`。
