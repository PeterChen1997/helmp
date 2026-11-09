## What / 改了什么

<!-- One or two sentences. Link the issue if there is one: Closes #123 -->
<!-- 一两句话说明。有对应 issue 请关联：Closes #123 -->

## Why / 为什么

<!-- The problem this solves. / 解决了什么问题。 -->

## How I tested it / 怎么验证的

<!-- Commands you ran, and what you checked by hand in the app. / 跑了哪些命令，在 App 里手动检查了什么。 -->

- [ ] `cargo test --release`
- [ ] `cargo clippy --release --all-targets -- -D warnings`
- [ ] Tried it in the running app (`cargo run` or `script/bundle.sh`) / 在实际运行的 App 里试过

## Checklist / 自查

- [ ] The change is limited to what this PR is about (no unrelated refactors or formatting) / 只改了本次需要改的地方（没有顺手重构或格式化）
- [ ] Docs updated if behavior, shortcuts, settings or stored data changed (README, `docs/`) / 行为、快捷键、设置或存储的数据有变化时，已更新文档
- [ ] No new network requests, or they are explained here and in `docs/privacy.md` / 没有新增网络请求；如有，已在此处和 `docs/privacy.md` 说明
