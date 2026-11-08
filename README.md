# Helmp

macOS 原生启动器（Rust + GPUI），目标是 Spotlight 式应用启动、agent 会话切换、剪贴板与截图，插件机制。
调研与决策见 `~/code_hub/_research/launcher/`。

## 功能

入口有两个：`⌥Space` 唤起面板，或点菜单栏的 Helmp 图标。没有 Dock 图标。

| 模块 | 进入方式 | 说明 |
|---|---|---|
| 应用搜索 | 直接输入，如 `wx` / `weixin` / `微信` | 中文名、全拼、首字母；显示匹配方式；⌘K 在访达中显示、复制路径、退出应用 |
| Agent 会话 | `ag ` 前缀、⌃⌥A、首页「等你处理」 | Claude Code / Codex 会话，按等你处理 / 进行中 / 最近完成分组，回车跳到会话 |
| 剪贴板历史 | `cb ` 前缀、⌘⇧V | 文本、图片、文件，按天分组；跳过密码类内容；回车粘贴到前台 App |
| 截图识别 | `截图`、⌘⇧2 | 区域截图、OCR（中英文），⌘T 把识别结果带进翻译；需要录屏权限 |
| 翻译 | `fy ` 前缀、⌥⇧T | 用你自己的模型（Anthropic / OpenAI 兼容 / Ollama），Key 存钥匙串；⌘↵ 替换选中文字 |
| 脚本命令 | 脚本标题前缀 | 兼容 Raycast Script Commands；有参数时显示表单 |
| 设置 | ⌘,、菜单栏「设置…」 | 通用、扩展、模型、Agent、剪贴板、截图、外观、关于 |

面板：↑↓ 选择，↵ 主操作，⌘K 更多操作，⇥ 切换筛选，esc 逐层返回。
隐藏超过 90 秒、或执行过结果后再唤起，输入框清空；90 秒内重开保留上次输入并全选。
唤起时自动切到英文键盘 ABC，隐藏时切回原输入法。以上都可在「设置 › 通用」里改。

首次启动会出现权限引导：点「前往授权」后，把浮窗里的 Helmp 图标拖进系统设置的列表即可。

## 运行

```bash
cargo run
```

无头查询（不开窗口，测试和排查用）：

```bash
cargo run -- query wx
HELMP_DRY_RUN=1 cargo run -- run <结果 id> agent   # agent 跳转只打印不执行
```

图形自检（真实窗口里走一遍显示、查询、执行、隐藏）与跟踪日志：

```bash
HELMP_SELF_TEST='wx|cb |截图' HELMP_SELF_TEST_EXEC=当前时间 cargo run
HELMP_TRACE=1 cargo run
```

## 打包

```bash
script/bundle.sh          # 生成 target/debug/Helmp.app（ad-hoc 签名，LSUIElement）
script/bundle.sh release
```

## 工具链

`rust-toolchain.toml` 锁定 1.99.1（`gpui-pre-macos 0.3.11` 用到 `Option::map_or_default`，1.96 编译不过）。
`gpui-kit` 精确锁定 `=0.9.0`，由它带出 `gpui-pre 0.3.11`，不要再单独声明 `gpui`。
