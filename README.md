# 唤 · Huan

macOS 原生启动器（Rust + GPUI），目标是 Spotlight 式应用启动、agent 会话切换、剪贴板与截图，插件机制。
调研与决策见 `~/code_hub/_research/launcher/`。

## 骨架阶段（当前）

- `⌥Space` 唤起 / 隐藏面板（`global-hotkey`，Carbon，免辅助功能权限）
- `WindowKind::PopUp` 非激活面板，`Esc` 或失去焦点即隐藏
- 菜单栏工具型 App（`ActivationPolicy::Accessory`，不进 Dock）

## 运行

```bash
cargo run
```

无屏幕权限的环境（CI、远程 shell）可跑自检，程序化走一遍 show → hide 后退出：

```bash
HUAN_SELF_TEST=1 cargo run
```

## 打包

```bash
script/bundle.sh          # 生成 target/debug/Huan.app（ad-hoc 签名，LSUIElement）
script/bundle.sh release
```

## 工具链

`rust-toolchain.toml` 锁定 1.99.1（`gpui-pre-macos 0.3.11` 用到 `Option::map_or_default`，1.96 编译不过）。
`gpui-kit` 精确锁定 `=0.9.0`，由它带出 `gpui-pre 0.3.11`，不要再单独声明 `gpui`。
