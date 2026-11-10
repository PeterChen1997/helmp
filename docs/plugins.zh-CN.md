# 插件

[English](plugins.md)

Helmp 的插件是一个文件夹，里面有一份 `plugin.json` 清单。大多数集成其实就是「填个表单 → 发一个 HTTP 请求」或者「关键词 → 打开一个网址」，清单把这些写成声明，不用写代码。需要逻辑时，命令可以运行插件目录里的脚本；列表命令可以把脚本输出的条目显示成结果列表。

Raycast 格式的[脚本命令](scripts.zh-CN.md)照常可用。一次性的小脚本用脚本命令就够了；需要偏好设置、密钥、表单、多个命令，或者想整个文件夹分享给别人时，写成插件。

- [插件放在哪](#插件放在哪)
- [教程：从零写一个 flomo 插件](#教程从零写一个-flomo-插件)
- [清单字段参考](#清单字段参考)
- [模板语法](#模板语法)
- [权限](#权限)
- [列表命令（脚本输出协议）](#列表命令)
- [测试插件](#测试插件)
- [深链接](#深链接)

## 插件放在哪

| 类型 | 位置 | 默认 |
|---|---|---|
| 自带 | 编译进 Helmp（flomo、Obsidian、快捷搜索），只读；源文件在 [`resources/plugins/`](../resources/plugins)，也是现成的示例 | 启用，可以停用 |
| 你的 | `~/Library/Application Support/Helmp/plugins/<文件夹>/plugin.json` | 停用，你确认后才启用 |

在「设置 › 插件」里管理（也可以在 Helmp 里输入「插件管理」）。这一页列出内置功能、自带插件和你的插件，显示版本、作者、权限和出错原因，顶部有「打开插件文件夹」「重新加载」「新建插件」（生成一个能直接用的模板并在访达中显示）和「开发者文档」。

Helmp 每 3 秒检查一次插件文件夹，文件有变化就重新加载。校验不通过的插件会留在列表里，标成「出错」并写明原因，不会被悄悄跳过。

启用你自己的插件前，Helmp 先列出它能做什么（[权限](#权限)）。之后如果你改了清单、让它需要更多权限（多了一个主机名、开始运行脚本……），插件会自动变成「权限有变化，待确认」，等你重新确认才生效。

## 教程：从零写一个 flomo 插件

[flomo](https://flomoapp.com) 给会员提供一个专属的记录 API：向 `https://flomoapp.com/iwh/<token>/<secret>/` 发 `POST`、正文为 JSON `{"content": "…"}`，就会新建一条 MEMO，正文里的 `#标签` 就是标签。整条地址本身就是密钥。Helmp 已经自带了这个插件；自己写一遍只要五分钟，清单的大部分写法都会用到。

**1. 建文件夹。** 在「设置 › 插件」点「打开插件文件夹」，新建文件夹 `com.example.flomo`，在里面放一个 `plugin.json`：

```json
{
  "schemaVersion": 1,
  "id": "com.example.flomo",
  "name": "我的 flomo",
  "version": "0.1.0",
  "author": "你",
  "icon": "feather",
  "tint": "green",
  "commands": []
}
```

`id` 用反向域名（`com.你的名字.插件名`），全小写。`icon` 写任意 [Lucide](https://lucide.dev/icons) 图标名，或者插件目录里的 `.png`。

**2. 声明密钥。** webhook 地址是凭据，声明成 `secret` 类型的偏好设置。secret 只存在 macOS 钥匙串（服务名 `dev.helmp.plugins`，账户 `<插件 id>/<偏好设置 id>`），不进 `config.json`，出现在提示信息里时会被打码成 `••••`。因为整条地址来自偏好设置，Helmp 事先不知道它指向哪个主机，所以要写明允许的主机名：

```json
"preferences": [
  {
    "id": "webhook",
    "type": "secret",
    "title": { "en": "Webhook URL", "zh-Hans": "记录 API 地址" },
    "placeholder": "https://flomoapp.com/iwh/xxxx/yyyy/",
    "required": true,
    "allowedHosts": ["flomoapp.com"]
  }
]
```

**3. 写命令。** 一个只有多行文本框的表单，加一个 `http` 动作：

```json
"commands": [
  {
    "id": "memo",
    "title": { "en": "Save to flomo", "zh-Hans": "记到 flomo" },
    "keywords": ["flomo", "浮墨"],
    "form": [
      { "id": "content", "type": "textarea", "label": { "en": "Memo", "zh-Hans": "内容" }, "required": true, "default": "{{query}}" }
    ],
    "action": {
      "type": "http",
      "method": "POST",
      "url": "{{pref.webhook}}",
      "json": { "content": "{{form.content}}" },
      "success": { "jsonPath": "code", "equals": 0, "optional": true },
      "message": { "en": "Saved to flomo", "zh-Hans": "已记录" },
      "errorPath": "message"
    }
  }
]
```

- `default: "{{query}}"`：输入 `flomo 读完了一本好书 #读书`，表单里就已经填好了 `读完了一本好书 #读书`。
- `json` 是真正的 JSON，里面每个字符串都是模板；引号、换行由 Helmp 转义，不用操心。会自动加上 `Content-Type: application/json`。
- `success`：状态码是 2xx，并且响应里如果有 `code` 字段，它要等于 `0`，才算成功。其他情况一律作为错误显示（带上服务端的 `message`），绝不会显示成「已记录」。

**4. 检查。**

```bash
helmp plugins validate ~/Library/Application\ Support/Helmp/plugins/com.example.flomo
# OK com.example.flomo 0.1.0 (1 个命令)
# 权限：
#   - 访问网络：flomoapp.com（地址来自偏好设置 webhook）
```

**5. 启用。** 「设置 › 插件 › 我的插件」：打开开关，看一遍权限，点「允许并启用」。点这一行展开，粘贴你的 webhook 地址。然后在 Helmp 里输入 `flomo` 就能用了。

## 清单字段参考

`docs/plugin-schema.json` 是给编辑器补全用的 JSON Schema（在清单里写 `"$schema": "https://raw.githubusercontent.com/PeterChen1997/helmp/HEAD/docs/plugin-schema.json"` 或本地路径）。以 `helmp plugins validate` 的结果为准。不认识的字段一律报错，拼错的字段名能被发现。顶层、偏好设置、命令和表单字段里可以写 `"$comment"` 做注释。

文案类字段（`name`、`title`……）可以是普通字符串，也可以是 `{"en": "…", "zh-Hans": "…"}`；必须有 `en`，缺中文时用它。

### 顶层

| 字段 | 必填 | 说明 |
|---|---|---|
| `schemaVersion` | 是 | `1` |
| `id` | 是 | 反向域名，小写字母、数字、`-`，全局唯一；不能和自带插件重名 |
| `name`、`description` | `name` 必填 | 文案 |
| `version` | 是 | `x.y.z` |
| `author` | 是 | |
| `homepage` | | 设置页里显示成「打开主页」按钮 |
| `license` | | 开源协议的 SPDX 标识，如 `MIT`。发布到[插件中心](plugin-center.zh-CN.md)时必填 |
| `icon`、`tint` | | Lucide 图标名或插件目录里的 `.png`；底色：`blue` `indigo` `purple` `orange` `teal` `green` `red` `gray` `graphite` |
| `permissions` | | 只能声明 `["clipboard.read"]`（用 `{{clipboard}}` 时必须有）；其他权限都从清单推导 |
| `preferences` | | 见下 |
| `commands` | 是* | 至少一个（*只带 `mcpServers` 的插件可以没有） |
| `mcpServers` | | 插件带的 MCP 服务器；插件启用时它们的工具变成命令。见 [MCP 服务器](mcp-client.zh-CN.md#插件可以带服务器) |

### 偏好设置

在「设置 › 插件」里展开插件时显示。

| 字段 | 说明 |
|---|---|
| `id` | 字母、数字、`_`、`-`；用 `{{pref.<id>}}` 引用 |
| `type` | `text`、`secret`（只存钥匙串，掩码输入）、`select`、`checkbox`（`"true"` / `"false"`） |
| `title`、`description`、`placeholder` | 文案 |
| `required` | 没填时命令不运行，提示去设置里填 |
| `default` | secret 不能有默认值；select 的默认值必须是某个选项的 value |
| `options` | 仅 select：`[{"title": …, "value": "…"}]` |
| `allowedHosts` | http 地址以 `{{pref.<id>}}` 开头时，这个地址允许指向的主机名 |

### 命令

| 字段 | 说明 |
|---|---|
| `id` | 插件内唯一；结果 id 是 `plugins:<插件 id>/<命令 id>` |
| `title`、`subtitle` | 文案。标题在任何界面语言下都能搜到，支持拼音和首字母（`ggfy` 能找到「谷歌翻译」） |
| `keywords` | 额外的搜索词。第一个会作为提示里的「关键词」 |
| `icon`、`tint` | 覆盖插件的图标 |
| `argument` | `{ "placeholder": 文案, "required": 布尔 }`。输入关键词（或标题）加空格再输入文字，文字作为 `{{query}}` |
| `form` | 面板表单视图里的字段（`text`、`textarea`、`select`），每个有 `id`、`label`、`placeholder`、`required`、`default`（模板，只能用 `{{query}}`、日期和非密钥的偏好设置）、`options` |
| `action` | 回车时做什么。`action` 和 `list` 只能写一个 |
| `list` | 脚本驱动的列表，见[列表命令](#列表命令) |

带 `argument`、`form` 或 `list` 的命令接受输入：`flomo 你好` 会把 `你好` 作为 `{{query}}`。

### 动作

| `type` | 字段 | 说明 |
|---|---|---|
| `open` | `url` | 用 macOS 的 `open` 打开链接（任意 scheme：`https:`、`obsidian:`、`things:`……） |
| `copy` | `text` | 复制到剪贴板 |
| `paste` | `text` | 复制后向前台 App 发 ⌘V（需要辅助功能权限，没有时只复制） |
| `http` | `method`（`GET` `POST` `PUT` `PATCH` `DELETE`）、`url`、`headers`、`json` 或 `body`、`timeoutMs`（≤ 30000，默认 10000）、`success`、`message`、`errorPath` | 见下 |
| `script` | `path`、`args`、`env`、`stdin`、`timeoutMs`（≤ 120000，默认 30000）、`output`（默认 `message`，可选 `silent`、`copy`、`paste`） | 运行插件目录里的脚本 |

**http。** 地址必须以 `https://` 或 `http://` 加固定的主机名开头，或者以声明了 `allowedHosts` 的 `{{pref.<id>}}` 开头。主机名部分不能是模板：启用插件之前就要能确定它会访问哪里。运行时 Helmp 还会按渲染后的地址再查一次主机名、不跟随重定向，请求头的值里有换行就拒绝。`success.status` 列出算作成功的状态码（默认任意 2xx）；`success.jsonPath` + `success.equals` 再检查响应里的某个字段（`optional: true` 表示字段存在时才检查）。`message` 是成功提示，可以用 `{{response.<路径>}}`；`errorPath` 指定失败时从 JSON 响应里取哪一段作为原因。失败一律作为错误显示，带上状态码和服务端给的原因。

**script。** `path` 相对插件目录，不能跳出插件目录（会先解析符号链接再检查）。`args` 每一项就是一个 argv 参数，由模板渲染；全程不经过 shell，所以输入里的 `; rm -rf` 只是普通文字。`env` 增加环境变量（另外总会有 `HELMP_PLUGIN_DIR` 和 `HELMP_PLUGIN_ID`）；`stdin` 写进脚本的标准输入。工作目录是插件目录。脚本可执行就直接执行，否则按 `#!` 行找解释器。非零退出码作为错误显示，附 stderr 末尾几行。

## 模板语法

标为模板的字符串里可以写 `{{ … }}`：

| 变量 | 值 |
|---|---|
| `{{query}}` | 关键词后面输入的文字 |
| `{{form.<id>}}` | 表单字段 |
| `{{pref.<id>}}` | 偏好设置（secret 只在用到时才读钥匙串） |
| `{{item.title}}` `{{item.subtitle}}` `{{item.arg}}` `{{item.uid}}` `{{item.url}}` | 选中的列表项（只在 `list.action` 里可用） |
| `{{response.<路径>}}` | JSON 响应里的字段（只在 `http.message` 里可用），如 `data.id`、`items.0.title` |
| `{{clipboard}}` | 剪贴板文字。必须声明 `"permissions": ["clipboard.read"]`；只在用到时读取；表单初值和列表脚本里不可用 |
| `{{date}}` `{{time}}` `{{date:%Y/%m/%d %H%M}}` | 本地日期 `2026-11-10`、时间 `14:05`，或任意 `strftime` 格式 |

过滤器从左到右依次应用：`|url`（百分号编码，放进网址的内容都要加）、`|json`（按 JSON 字符串转义，不带引号；只有手写文本 `body` 时才需要）、`|trim`、`|upper`、`|lower`。例如 `https://www.npmjs.com/search?q={{query|trim|url}}`。

模板不支持条件和表达式：需要逻辑就用脚本。写错的变量名、过滤器名会在校验时报错，而不是渲染成空串。

## 权限

权限从清单推导出来，在设置里显示，启用插件时需要你确认：

| 权限 | 来源 |
|---|---|
| 访问网络：`主机名` | `http` 地址的主机名，或地址所来自的偏好设置的 `allowedHosts` |
| 在浏览器中打开网页 / 打开 `obsidian://` 链接（会启动对应的 App） | `open` 地址的 scheme |
| 打开任意链接或文件 | 整段都是模板的 `open` 地址，或没写 `action` 的列表（打开 `item.arg`） |
| 读取剪贴板 | `"permissions": ["clipboard.read"]` |
| 写入剪贴板 | `copy` / `paste` 动作，`script` 的 `output` 为 `copy` / `paste` |
| 向前台应用发送 ⌘V | `paste` |
| 运行插件目录里的脚本 | `script` 动作和 `list` 命令。脚本以你的用户身份运行、不受沙盒限制：只启用你信任的插件 |
| 运行本地程序：`npx` | `mcpServers` 里的每个 `command`（同样以你的身份运行、不受沙盒限制） |
| 访问网络：`主机` | 也包括 `mcpServers` 里的每个 `url` |

自带插件默认启用。你的插件默认停用，启用时 Helmp 记下你确认过的权限；权限变了会再问一次。

## 列表命令

列表命令在你输入时（关键词加空格之后）运行脚本，把脚本输出的条目显示成结果。输出格式就是 [Alfred 的 Script Filter JSON](https://www.alfredapp.com/help/workflows/inputs/script-filter/json/)，很多现成脚本可以直接用：

```json
{"items": [
  {"uid": "helmp", "title": "helmp", "subtitle": "~/code/helmp", "arg": "/Users/me/code/helmp",
   "icon": {"path": "icons/repo.png"}, "text": {"copy": "helmp"}},
  {"title": "没有更多结果", "valid": false}
]}
```

| 字段 | 用途 |
|---|---|
| `title` | 必填 |
| `subtitle`、`arg`、`uid`、`quicklookurl` | 显示，或作为 `{{item.subtitle}}`、`{{item.arg}}`、`{{item.uid}}`、`{{item.url}}` 使用。`arg` 可以是数组（按换行拼接） |
| `icon.path` | png / jpg（相对插件目录或绝对路径）；`"type": "fileicon"` 时用这个文件在访达里的图标 |
| `valid: false` | 只是提示行，回车什么也不做 |
| `text.copy` | ⌘K「复制」复制的内容（默认是 `arg`） |

```json
{
  "id": "recent",
  "title": { "en": "Recent Projects", "zh-Hans": "最近项目" },
  "keywords": ["proj"],
  "list": {
    "script": { "path": "bin/recent.sh", "args": ["{{query}}"], "timeoutMs": 1000 },
    "action": { "type": "open", "url": "{{item.arg}}" },
    "minQueryLength": 0,
    "cacheSeconds": 5
  }
}
```

列表脚本能用 `{{query}}` 和偏好设置（不能用表单和剪贴板），必须在 `timeoutMs`（默认 1000，最多 3000）内结束；同一输入的结果缓存 `cacheSeconds` 秒（至少 1 秒）。最多显示 200 项。脚本失败、或者输出的不是上面这种 JSON 时，列表里只显示一行错误和原因。没写 `action` 时，选中一项会把 `item.arg` 当作网址或路径打开。

## 测试插件

```bash
# 校验：字段、模板变量、主机名、脚本路径。一次列出全部问题，有问题时退出码为 1。
helmp plugins validate path/to/plugin
# 同上，输出 JSON（ok、id、version、commands、permissions、permissions_text、errors），给脚本和 agent 读。
helmp plugins validate path/to/plugin --json

# 列出插件及状态（enabled / disabled / needs-consent / error）。
helmp plugins

# 无副作用地运行：打开、复制、粘贴、发往真实主机的请求都只打印出来。
HELMP_DRY_RUN=1 helmp query "npm react"
HELMP_DRY_RUN=1 helmp run plugins:dev.helmp.quicklinks/npm "npm react"
HELMP_DRY_RUN=1 helmp run plugins:com.example.flomo/memo "flomo 你好" --form content="你好 #测试"
```

演练模式（`HELMP_DRY_RUN=1`）下 `{{clipboard}}` 变成 `[clipboard]`，不读你的剪贴板；发往真实主机的 http 请求只打印 `[dry-run] http POST <地址> <正文>`（密钥打码），不会发出去。脚本照常运行，和脚本命令一致。

自动化测试时，用隔离的数据目录运行：`HELMP_DATA_DIR=<临时目录> HELMP_DRY_RUN=1`。只有两者同时设置时：

- 密钥从 `HELMP_TEST_SECRETS='{"<插件 id>/<偏好设置 id>": "值"}'` 读取，绝不碰钥匙串。不在这种模式下时这个变量被忽略。
- 允许并真正发出发往 `127.0.0.1` / `localhost` 的请求，所以锁定在 `flomoapp.com` 的插件也能指向本机的假服务。

Helmp 自己的端到端测试（[`tests/e2e_plugins.rs`](../tests/e2e_plugins.rs)）就是这样跑的。

## 发布到插件中心

[插件中心](plugin-center.zh-CN.md)里是经过审核、谁都能一键安装的插件。发布你的插件：

1. 在 `plugin.json` 里写上 `"license"`（SPDX 标识，比如 `"MIT"`），再加一个 `README.md`，说明插件做什么、有哪些命令、怎么配置。256×256 的 `icon.png` 可选。
2. 文件夹保持干净：只放 UTF-8 文本文件和 PNG 图片，不要符号链接和隐藏文件；最多 64 个文件，单个文件不超过 512 KiB，总共不超过 2 MiB。id 用你自己的域名（不要用 `dev.helmp.*`）。
3. 运行 `helmp plugins validate <文件夹>`。
4. 运行 `helmp plugins submit <文件夹>`。它按注册表 CI 的同一套规则检查，打印权限、文件清单和将要做的事，然后停下，不做任何改动。确认后运行 `helmp plugins submit <文件夹> --yes`。这一步需要已登录的 [GitHub CLI](https://cli.github.com/)（`gh auth login`）。它会用你的账号 fork [PeterChen1997/helmp-plugins](https://github.com/PeterChen1997/helmp-plugins)，推送名为 `plugin/<id>-<版本>` 的分支（文件放在 `plugins/<id>/`），然后开一个 PR。PR 描述里列出权限、每个文件的 SHA-256 和审核清单。最后打印 PR 地址。
5. 维护者审核 PR。合并后注册表会重新生成 `index.json`，插件就出现在插件中心里。

发布更新：提升 `version` 后再提交一次。装过这个插件的用户会看到「有更新」；权限有变化时，他们要重新确认。

Claude Code、Codex 这类 coding agent 可以通过 Helmp 的 MCP 服务（`helmp mcp`）替你写插件、校验并提交，但运行提交这一步之前，必须先把计划给你看，等你同意。

## 深链接

`helmp://plugin/<插件 id>/<命令 id>?q=<文字>` 调用一个命令，相当于输入它的关键词、空格和 `<文字>`。只打开链接的命令会直接执行；带表单、发 http 请求、跑脚本或读写剪贴板的命令只在面板里显示出来，等你回车，所以网页里的链接没法替你提交任何内容。见 [deeplinks.md](deeplinks.md)。

```bash
open "helmp://plugin/dev.helmp.quicklinks/npm?q=react"
open "helmp://plugin/dev.helmp.flomo/memo?q=%23%E7%81%B5%E6%84%9F"
```
