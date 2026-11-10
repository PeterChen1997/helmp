# 插件中心

[English](plugin-center.md) · 简体中文

插件中心是经过维护者人工审核的[插件](plugins.zh-CN.md)列表。网页版在 <https://blog.peterchen97.cn/helmp/plugins/>，Helmp 里在「设置 › 插件 › 从插件中心安装」。

插件放在公开的 GitHub 仓库 [PeterChen1997/helmp-plugins](https://github.com/PeterChen1997/helmp-plugins) 里，一个插件一个文件夹（`plugins/<id>/`）。仓库里的脚本会生成 `index.json`，列出每个插件的版本、权限，以及每个文件的 SHA-256 和大小。Helmp 通过 jsDelivr 下载索引和文件。

## 安装

下面三种方式任选一种：

- 在网页上点「在 Helmp 中安装」，它会打开 `helmp://install-plugin?id=<id>`。
- 在「设置 › 插件」里点「浏览插件中心」，然后点插件旁边的「安装」。
- 在终端运行 `helmp plugins install <id>`。想看插件中心里有哪些插件，运行 `helmp plugins available`。

插件装到 `~/Library/Application Support/Helmp/plugins/<id>/`，装好后是**停用**状态。设置页会定位到这个插件，列出它能做的事：访问哪些主机、会不会运行脚本等等。你点了「允许并启用」，它才会运行。网页里的链接可以帮你安装插件，但永远没法替你启用。

`dev.helmp.flomo`、`dev.helmp.quicklinks`、`dev.helmp.obsidian` 这些 id 是 Helmp 自带的插件，Helmp 不会再装一份去覆盖它们。遇到这种情况，Helmp 会告诉你插件已经自带，在设置里启用即可。如果你自己往插件文件夹里放了同 id 的插件，Helmp 也不会覆盖它，需要你先把它移走。

## 更新

Helmp 不在后台检查更新。在设置里打开「浏览插件中心」（或运行 `helmp plugins available`）时，从插件中心安装的插件如果有新版本，会标出「有更新」，按钮变成「更新到 x.y.z」。运行 `helmp plugins install <id>` 也能更新。

更新时先下载并校验新版本，再替换整个文件夹。新版本要的权限和你之前允许的一样时，插件保持启用。权限变了（比如多了一个主机，或者开始运行脚本）时，插件会暂停并标为「权限有变化，待确认」，等你重新确认后才恢复。

## 安全机制

- **人工审核**：每个插件都要通过 Pull Request 进入插件中心，由维护者审核。审核内容包括：权限与插件用途相符；访问的主机都在预期之内；脚本没有混淆，也不下载执行远程代码；插件按 README 实测可用。CI 只检查格式和文件规则，并把脚本标出来留给人工审核，从不运行插件代码。
- **下载校验**：Helmp 只下载索引里列出的文件，写入任何东西之前，逐个核对大小和 SHA-256。只要有一个文件对不上（被篡改，或 CDN 出错），整个安装就会被拒绝，磁盘上不留下任何东西。文件路径如果会跳出插件目录（`../`、绝对路径、隐藏文件），整份索引都会被拒绝。通过 jsDelivr 下载时，文件按生成索引时的那个注册表 commit 获取，CDN 缓存过期也不会混进别的版本。
- **原子安装**：文件先写进一个隐藏的临时文件夹，按 `helmp plugins validate` 的同一套规则校验，清单里的 id 和版本必须与索引一致。全部通过后，才把文件夹改名到正式位置。更新失败时，原来的版本保持不动。
- **用户确认**：装好的插件默认停用。插件运行前要你确认它的权限，权限有变化时还要再确认一次。密钥只存在钥匙串里。
- **限制**：每个插件最多 64 个文件，单个文件不超过 512 KiB，总共不超过 2 MiB。只允许 UTF-8 文本文件和 PNG 图标，不能有符号链接或其他二进制文件。

剩下需要信任的是：维护者的审核、GitHub、jsDelivr 的 TLS，以及插件启用后以你的身份运行的脚本。只启用权限让你放心的插件。

## 设置项

| 设置 | 作用 |
|---|---|
| `config.json` 里的 `plugins_registry_url` | 注册表地址（`index.json` 所在的目录）。默认是 `https://cdn.jsdelivr.net/gh/PeterChen1997/helmp-plugins@main/` |
| 环境变量 `HELMP_REGISTRY_URL` | 优先于上面的设置，比如指向本机的测试服务 |
| 环境变量 `HELMP_REGISTRY_REPO` | `helmp plugins submit` 开 PR 的 GitHub 仓库，默认是 `PeterChen1997/helmp-plugins` |

## 发布你的插件

详见 [plugins.zh-CN.md › 发布到插件中心](plugins.zh-CN.md#发布到插件中心)。简要步骤：

1. 在插件文件夹里加上 `"license"`（比如 `"MIT"`）和 `README.md`。
2. 运行 `helmp plugins validate <文件夹>`。
3. 运行 `helmp plugins submit <文件夹>`。它会先打印将要做的事，然后停下。加上 `--yes` 再运行一次，它会用你的 `gh` 账号 fork 注册表，推送名为 `plugin/<id>-<版本>` 的分支并开一个 PR，最后打印 PR 地址。

Claude Code、Codex 这类 coding agent 可以通过 Helmp 的 MCP 服务（`helmp mcp`）替你完成这些步骤，但提交前必须先问你。PR 由维护者审核，合并后插件就会出现在插件中心。
