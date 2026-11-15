# Helmp 仓库约定（给 coding agent）

开发环境、测试写法、代码风格、提交格式见 `CONTRIBUTING.md`，这里只记读代码看不出的规矩。

## 分支与 CI
- 远端主分支是 `main`（GitHub 仓库名 `helmp`，本地目录叫 `helep`）。个人仓库，默认直接在分支上提交，不开 PR。
- GitHub CI 已停用（2026-11-14 `gh workflow disable CI`，`.github/workflows/ci.yml` 文件还留着）。不要重新启用、不要新加 CI workflow；
  提交前在本地跑 `cargo test --release` 和 `cargo clippy --release --all-targets -- -D warnings`。

## 必须真机验证的
- 中英文输入法切换反复坏过：动到面板激活 / 焦点 / 输入框时，先 `bash script/bundle.sh release`，再跑 `bash script/ime-check.sh`（真实按键，需辅助功能权限）。
- 剪贴板自动粘贴：同上，跑 `bash script/paste-check.sh`。
- 截图、钉图这类窗口交互只做离屏渲染不算验证完，汇报里要写明「待真机验证」。

## 发版
- 打包、签名、公证都在本地：`bash script/release.sh`（产物在 `target/dist/`）。
- 发布由维护者本人决定：`script/publish-release.sh --publish` → `script/update-landing.sh <版本>`（部署 `site/` 并回读）。没明说「发布」时只打包，不上传。

## 其他
- `design/` 是设计探索产物；`design/archive/` 里是已过时的草稿（如旧名「唤」时期的方向简报），不代表当前设计。
- 项目进度记在 hub 状态文件 `~/code_hub/_hub/projects/helep.md`，不在仓库里。
