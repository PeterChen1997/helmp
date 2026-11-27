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

从插件中心或插件源安装的插件会自动更新。「设置 › 插件 › 自动更新插件」开着时（默认开），Helmp 每天检查一次：启动 20 秒后一次，之后每 24 小时，和 Helmp 自己的更新检查同一个节奏。每个源只取一次索引，每个插件只和安装它的那个源比较。同一栏显示上次检查的时间和结果，还有「现在检查」按钮。

- 新版本要的权限和你之前允许的一样，而且会运行代码的插件文件没变：静默装上，保持启用。
- 权限变了（比如多了一个主机、MCP 服务器的命令行不一样了，或者开始运行脚本）：装上但暂停，标为「权限或内容有变化，待确认」，等你重新确认后才恢复。会运行代码的插件（脚本、定时任务、MCP 服务器）更新时换了任何文件也这样：你确认的是当时装的那份脚本，换了文件要等你再看一眼。只升版本号不算。
- 任何一步失败（离线、HTTP 401、文件和 SHA-256 对不上）：什么都不变，旧版本原样保留，不打扰你，1 小时后再试。
- 那一刻正在运行脚本或 MCP 服务器的插件跳过，下次再更新。

也可以手动更新：`helmp plugins update`（或 `--all`）检查全部插件，`helmp plugins update <id>` 只查一个，`helmp plugins install <id>` 照样能用。打开「浏览插件中心」时，有新版本的插件按钮会变成「更新到 x.y.z」。关掉开关后 Helmp 不再自己检查；`helmp plugins update --scheduled` 和每日检查一样，开关关着时什么都不做。

更新时先下载并校验新版本，再替换插件目录。插件存在自己数据目录里的东西（`HELMP_PLUGIN_DATA_DIR`，见[插件数据](plugins.zh-CN.md#插件数据)）不受影响。

## 卸载

在「设置 › 插件」里点开插件，点「卸载…」。插件存过数据时，Helmp 会问要一并删掉，还是留着以后重装接着用。卸载也会清掉「你已启用它」的记录，重装回来默认停用。命令行：`helmp plugins uninstall <id>` 保留数据，加 `--delete-data` 一并删除。手动放进插件文件夹的插件不走这里，请自己删掉它的文件夹。

## 安全机制

- **人工审核**：每个插件都要通过 Pull Request 进入插件中心，由维护者审核。审核内容包括：权限与插件用途相符；访问的主机都在预期之内；脚本没有混淆，也不下载执行远程代码；插件按 README 实测可用。CI 只检查格式和文件规则，并把脚本标出来留给人工审核，从不运行插件代码。
- **下载校验**：Helmp 只下载索引里列出的文件，写入任何东西之前，逐个核对大小和 SHA-256。只要有一个文件对不上（被篡改，或 CDN 出错），整个安装就会被拒绝，磁盘上不留下任何东西。文件路径如果会跳出插件目录（`../`、绝对路径、隐藏文件），整份索引都会被拒绝。通过 jsDelivr 下载时，文件按生成索引时的那个注册表 commit 获取，CDN 缓存过期也不会混进别的版本。
- **原子安装**：文件先写进一个隐藏的临时文件夹，按 `helmp plugins validate` 的同一套规则校验，清单里的 id 和版本必须与索引一致。全部通过后，才把文件夹改名到正式位置。更新失败时，原来的版本保持不动。
- **用户确认**：装好的插件默认停用。插件运行前要你确认它的权限，权限有变化时还要再确认一次。密钥只存在钥匙串里。
- **限制**：每个插件最多 64 个文件，单个文件不超过 512 KiB，总共不超过 2 MiB。只允许 UTF-8 文本文件和 PNG 图标，不能有符号链接或其他二进制文件。Helmp 下载后逐个文件再查一遍：图片按文件头核对必须真是扩展名说的格式，其余文件必须是 UTF-8 文本。

剩下需要信任的是：维护者的审核、GitHub、jsDelivr 的 TLS，以及插件启用后以你的身份运行的脚本。只启用权限让你放心的插件。

## 自定义插件源

除了公开的插件中心，Helmp 还能从**自定义插件源**浏览和安装插件：结构相同、由你自己托管的注册表，类似 Homebrew tap 或团队的私有商店。插件中心总是第一个源，自定义源排在它后面。

### 添加插件源

- **设置 › 插件 › 插件源 › 添加插件源**：填名字、地址，需要的话再填鉴权请求头名和令牌。
- **终端**：`helmp plugins source add <名字> <地址> [--header <请求头名>] [--token-stdin]`，例如 `printf 'Bearer …' | helmp plugins source add Team https://plugins.example.com/team/ --token-stdin`。`helmp plugins source list` 列出插件源（从不打印令牌）；`helmp plugins source remove <名字>` 删除插件源和它的令牌。
- **链接**：`helmp://add-plugin-source?url=<地址>&name=<名字>` 打开设置并预填表单。你点「添加」之前什么都不会加；链接里也不能带令牌，带了其他任何参数的链接都不认。

地址必须是 `https`（只有 `localhost` / `127.0.0.1` 可以用 `http`，测试用），不能带用户名、密码、查询参数或片段。

### 目录结构

插件源就是一个用 HTTPS 提供的目录，结构和插件中心完全一样：

```text
<地址>/index.json
<地址>/plugins/<id>/plugin.json
<地址>/plugins/<id>/<index.json 里列出的其他文件>
```

`index.json`（`"schema": 1`）列出每个插件的 `id`、`version`、`name`、`author`、可选的 `description`、`permissions` 和 `permissions_text`，以及 `files`：每个文件的 `path`、`sha256`、`size` 和 `executable`。对于 `plugins/<id>/…` 这样的目录，可以这样生成：

```sh
python3 - <<'PY'
import hashlib, json, os, subprocess
plugins = []
for pid in sorted(os.listdir("plugins")):
    root = os.path.join("plugins", pid)
    info = json.loads(subprocess.run(["helmp", "plugins", "validate", root, "--json"],
                                     capture_output=True, text=True, check=True).stdout)
    manifest = json.load(open(os.path.join(root, "plugin.json")))
    files = []
    for dirpath, _, names in os.walk(root):
        for name in sorted(names):
            path = os.path.join(dirpath, name)
            data = open(path, "rb").read()
            files.append({"path": os.path.relpath(path, root), "sha256": hashlib.sha256(data).hexdigest(),
                          "size": len(data), "executable": os.access(path, os.X_OK)})
    plugins.append({"id": pid, "version": info["version"], "name": manifest["name"],
                    "author": manifest["author"], "description": manifest.get("description"),
                    "permissions": info["permissions"], "permissions_text": {"en": info["permissions_text"]},
                    "files": sorted(files, key=lambda f: f["path"])})
json.dump({"schema": 1, "commit": None, "plugins": plugins}, open("index.json", "w"), indent=2)
PY
```

自定义源是你自己选择信任的，限制比插件中心宽：每个插件最多 64 个文件，单个文件 4 MiB，总共 16 MiB。除了 UTF-8 文本和 PNG，还可以带 JPEG（`.jpg` / `.jpeg`）、WebP 和 GIF 图片，用来放插图或动图。每张图片都按文件头核对，不只看扩展名，改了扩展名的二进制文件会被拒绝。不能有隐藏文件和符号链接，并且要能通过 `helmp plugins validate`。（合计 16 MiB 让每日自动更新的下载量保持在一张照片左右；`helmp plugins submit` 和插件中心仍按自己更严的规则。）

### 鉴权

插件源需要令牌时，填请求头名（默认 `Authorization`）和令牌的值，Helmp 原样发送（比如 `Bearer <令牌>`，或用 `--header X-Api-Key` 直接发密钥）。私有 GitHub 仓库可以用 `raw.githubusercontent.com` 的地址加 `Authorization: token <PAT>`，也可以放在任何支持请求头鉴权的 HTTPS 服务上。

- 令牌存在 macOS 钥匙串（服务名 `dev.helmp.plugin-sources`），从不写进 `config.json`、日志、报错文案或崩溃报告。
- 只发给这个插件源自己的主机。重定向只在同一主机内跟随；跳到别的主机会直接停下，不会把令牌带过去。
- HTTP 401 / 403、源连不上时，按源分别说明（哪个源、什么原因）。一个源失败不影响其他源。

### 浏览、安装与更新

设置里的插件列表（以及 `helmp plugins available`）合并展示所有源的插件，并标出来源。同一个 id 出现在多个源里时，插件中心优先，其次是列表里靠前的源，插件旁边会明确提示。要从指定的源安装，在设置里点那一行，或用 `helmp plugins install <id> --source <名字>`。

插件只会从安装它的那个源更新，别的源发布同 id 的插件也替换不了它。删掉那个源后，插件仍然保留，只是不会再有更新。

其余和插件中心完全一样：校验 SHA-256、原子安装、装好默认停用、权限变化要重新确认。

**只添加你信任的源。** 自定义源没有经过插件中心维护者的审核。令牌要当密码看待：主机支持的话给每个人单独发只读令牌，泄露了就换掉。

## 设置项

| 设置 | 作用 |
|---|---|
| `config.json` 里的 `plugins_registry_url` | 注册表地址（`index.json` 所在的目录）。默认是 `https://cdn.jsdelivr.net/gh/PeterChen1997/helmp-plugins@main/` |
| 环境变量 `HELMP_REGISTRY_URL` | 优先于上面的设置，比如指向本机的测试服务 |
| `config.json` 里的 `general.auto_update_plugins` | 每天检查从插件中心或插件源安装的插件并自动更新，默认 `true`。即设置里的「自动更新插件」开关 |
| `config.json` 里的 `plugin_sources` | 自定义插件源（`name`、`url`、可选的 `auth_header`）。令牌在钥匙串里，不在这里。在设置里或用 `helmp plugins source` 管理 |
| 环境变量 `HELMP_REGISTRY_REPO` | `helmp plugins submit` 开 PR 的 GitHub 仓库，默认是 `PeterChen1997/helmp-plugins` |

## 发布你的插件

详见 [plugins.zh-CN.md › 发布到插件中心](plugins.zh-CN.md#发布到插件中心)。简要步骤：

1. 在插件文件夹里加上 `"license"`（比如 `"MIT"`）和 `README.md`。
2. 运行 `helmp plugins validate <文件夹>`。
3. 运行 `helmp plugins submit <文件夹>`。它会先打印将要做的事，然后停下。加上 `--yes` 再运行一次，它会用你的 `gh` 账号 fork 注册表，推送名为 `plugin/<id>-<版本>` 的分支并开一个 PR，最后打印 PR 地址。

Claude Code、Codex 这类 coding agent 可以通过 Helmp 的 MCP 服务（`helmp mcp`）替你完成这些步骤，但提交前必须先问你。PR 由维护者审核，合并后插件就会出现在插件中心。
