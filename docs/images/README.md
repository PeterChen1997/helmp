# docs/images

Images used by `README.md` and `README.zh-CN.md`. / `README.md` 与 `README.zh-CN.md` 引用的图片。

| File / 文件 | Used for / 用途 |
|---|---|
| `icon-256.png` | App icon at the top of both READMEs (256 × 256, same artwork as `resources/AppIcon-1024.png`) / 两份 README 顶部的 App 图标（256 × 256，与 `resources/AppIcon-1024.png` 同一张图） |
| `screenshot-home.png` | Panel home view with an empty query: "waiting for you" sessions, Recent, quick commands / 空查询的面板首页：等你处理的会话、最近使用、快捷命令 |
| `screenshot-agents.png` | Agent sessions scope (`ag `) with the detail pane showing the last message / Agent 会话范围（`ag `），右侧详情显示最后一条消息 |
| `screenshot-clipboard.png` | Clipboard history scope (`cb `) with day groups and the type filter / 剪贴板历史范围（`cb `），带按天分组和类型筛选 |
| `screenshot-ocr.png` | Text and QR code recognized from an image file (`ocr /path/to/image.png`) / 从图片文件识别出的文字和二维码（`ocr /path/to/image.png`） |
| `screenshot-translate.png` | Translate scope with a translation and the caption line (model · languages · time) / 翻译范围，显示译文和下方的「模型 · 语言 · 耗时」小字 |

## How the screenshots were made / 截图是怎么来的

They are real renders of the Helmp panel (English UI, light appearance, 1440 px wide), filled with sample data only: a throwaway `HELMP_DATA_DIR`, `HOME`, `CLAUDE_CONFIG_DIR` and `CODEX_HOME`, a seeded `clipboard.sqlite` with clipboard capture turned off, and a local mock Ollama for Translate. / 都是 Helmp 面板的真实渲染（英文界面、浅色外观、宽 1440 px），只用示例数据：临时的 `HELMP_DATA_DIR`、`HOME`、`CLAUDE_CONFIG_DIR` 和 `CODEX_HOME`，预先写好的 `clipboard.sqlite`（剪贴板采集关闭），翻译走本地的假 Ollama。

macOS screen capture needs the Screen Recording permission even for an app's own windows, so the panel is rendered off screen by GPUI instead: build with `cargo build --release --features readme-shots`, then run the GUI self-test with `HELMP_SNAPSHOT_DIR=<dir>` and a `snap:<name>` token in `HELMP_SELF_TEST_KEYS` (`wait:<seconds>` waits for slow steps such as OCR). The self-test panel stays off screen and never takes keyboard focus. GPUI only draws the content layer, not the system frosted glass, so the PNGs were composited onto a light background and given the panel's 24 pt rounded corners. / macOS 截屏连 App 自己的窗口都要「录屏」权限，所以改由 GPUI 离屏渲染：用 `cargo build --release --features readme-shots` 编译，跑图形自检时设 `HELMP_SNAPSHOT_DIR=<目录>`，在 `HELMP_SELF_TEST_KEYS` 里放 `snap:<名字>`（慢操作如 OCR 用 `wait:<秒>` 等一下）。自检面板在屏幕外，不抢键盘焦点。GPUI 只画内容层、不含系统毛玻璃，所以 PNG 另外垫了浅色底，并按面板 24 pt 圆角裁边。

When you retake them: use sample data only (no real clipboard contents, file paths, project names, API keys or agent conversations), keep the English UI, and keep each file under about 500 KB. / 重拍时：只用示例数据（不要出现真实的剪贴板内容、文件路径、项目名、API Key 或 Agent 对话），保持英文界面，单个文件尽量控制在 500 KB 以内。
