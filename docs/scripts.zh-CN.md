# 脚本命令

[English](scripts.md) · 简体中文

脚本命令就是开头带几行特殊注释的可执行文件。Helmp 读的是 [Raycast Script Commands](https://github.com/raycast/script-commands) 的格式，所以给 Raycast 写的脚本大多拿来就能用。语言不限：Shell、Python、Ruby、Swift、JavaScript、AppleScript，只要你的 Mac 能跑就行。

## 脚本放在哪

Helmp 会递归扫描下面这些目录（跳过隐藏文件和隐藏目录）：

1. `~/Library/Application Support/Helmp/scripts/`（始终包含）
2. 在「设置 › 扩展 › 脚本目录」里添加的目录
3. 环境变量 `HELMP_SCRIPT_DIRS` 里的目录，用 `:` 分隔

每 5 秒检查一次，新增、修改、删除都不用重启；只有修改时间、大小或可执行位变了的文件才会重新解析。找头注释时每个文件只读前 64 KB。

文件里至少有一个 `@raycast.` 或 `@helmp.` 字段，才会被当作脚本命令。字段有问题的（缺标题、mode 不认识、参数 JSON 写错……）会被跳过，原因打印到 stderr（从终端启动 Helmp 就能看到）。

## 最小示例

```bash
#!/bin/bash

# @raycast.schemaVersion 1
# @raycast.title 当前时间
# @raycast.mode compact

date "+%Y-%m-%d %H:%M:%S %Z"
```

存进脚本目录，加上可执行权限（`chmod +x current-time.sh`），然后在 Helmp 里输入 `当前时间` 或者拼音 `dqsj`。

## 头注释字段

写法是 `<注释符> @raycast.<字段> <值>`，注释符可以是 `#`、`//` 或 `--`，字段名不区分大小写。

每个字段也都可以写成 `@helmp.<字段>`。同一个字段两种都写了，以 `@helmp.` 为准。这样你可以针对 Helmp 微调一个和 Raycast 共用的脚本，又不影响它在 Raycast 里的表现。

| 字段 | 必填 | 说明 |
|---|---|---|
| `title` | 是 | 列表里显示的名字，也是搜索的对象（中文标题支持拼音） |
| `mode` | 是 | `silent`、`compact`、`fullOutput` 或 `inline`，见下文 |
| `schemaVersion` | 否 | 写了的话必须是 `1` |
| `packageName` | 否 | 分组名，显示在副标题里；也能被搜到，排在标题匹配之后 |
| `description` | 否 | 显示在副标题和详情里 |
| `author` | 否 | 能解析，暂不显示 |
| `argument1` … `argument3` | 否 | 参数，见下文 |
| `needsConfirmation` | 否 | `true` 或 `false`。为 `true` 时，只有查询以 ` !` 结尾才会执行 |
| `currentDirectoryPath` | 否 | 工作目录。支持 `~` 和 `~/…`，相对路径按脚本所在目录解析。默认是脚本所在目录 |
| `platform` | 否 | 写了的话必须包含 `macOS` |
| `icon` | 否 | 能解析，暂不显示；目前所有脚本都用同一个终端图标 |
| `refreshTime` | 否 | 能解析，暂未使用 |

Raycast 的其他字段（如 `authorURL`）会被忽略。

## 输出方式

| mode | 效果 |
|---|---|
| `silent` | 什么都不显示，面板保持收起。适合复制、打开、开关某样东西这类脚本 |
| `compact` | 显示 stdout 的最后一行 |
| `fullOutput` | 显示完整的 stdout 和 stderr（等宽字体） |
| `inline` | 目前等同于 `compact`（Raycast 会把它常驻在条目上并定时刷新，Helmp 还没做） |

输出显示在面板里，可以用「复制输出」（<kbd>⌘C</kbd>）。Raycast 会把 `silent` 的输出放在 HUD 里，Helmp 没有 HUD，所以 `silent` 就是真的不打扰。

## 参数

最多声明三个参数，每个是一个 JSON 对象：

```bash
# @raycast.argument1 { "type": "text", "placeholder": "城市", "percentEncoded": true }
# @raycast.argument2 { "type": "password", "placeholder": "token", "optional": true }
# @raycast.argument3 { "type": "dropdown", "placeholder": "舱位", "data": [{"title": "经济舱", "value": "e"}, {"title": "商务舱", "value": "b"}] }
```

| 键 | 说明 |
|---|---|
| `type` | `text`（默认）、`password`（掩码输入）或 `dropdown` |
| `placeholder` | 字段名与输入提示 |
| `optional` | `true` 表示可以留空 |
| `percentEncoded` | `true` 表示先做 URL 编码再传给脚本 |
| `data` | `dropdown` 的选项：`{ "title", "value" }` 数组 |

旧写法 `"secure": true` 等同于 `"type": "password"`。

带参数的脚本会打开一个表单。也可以直接跟在标题后面输入：比如标题是「查航班」，输入 `查航班 柏林` 就会把「柏林」填进第一个字段。多个参数按空白切分，最后一个参数吃掉剩下的全部文字。参数按 `$1`、`$2`、`$3` 传给脚本。

## 脚本怎么运行

- 文件有可执行权限就直接运行；没有的话读 shebang（如 `#!/usr/bin/env python3`）调用解释器。两样都没有的文件会被跳过。Helmp 不会改你的文件权限。
- 每次运行最多 **30 秒**，超时会杀掉整个进程组，连脚本派生的子进程一起清掉。
- stdout 和 stderr 各自最多保留末尾 1 MiB。
- 退出码非零算失败：面板里会显示错误和 stderr 的最后几行。
- 脚本继承 Helmp 的环境变量。从程序坞或登录时启动的 Helmp 环境变量很少，所以请用绝对路径（如 `/opt/homebrew/bin/jq`），或者在脚本开头自己设置 `PATH`。

## 示例

[`examples/scripts`](../examples/scripts) 里有三个小脚本，各演示一种常用的输出方式：

| 脚本 | mode | 演示 |
|---|---|---|
| [`current-time.sh`](../examples/scripts/current-time.sh) | `compact` | 最简单的命令 |
| [`generate-uuid.sh`](../examples/scripts/generate-uuid.sh) | `silent` | 做完事（复制一个 UUID）就安静退场 |
| [`local-ip.sh`](../examples/scripts/local-ip.sh) | `fullOutput` | 多行输出 |

试用方法：

```bash
mkdir -p ~/Library/Application\ Support/Helmp/scripts
cp examples/scripts/*.sh ~/Library/Application\ Support/Helmp/scripts/
```

也可以不复制，直接让 Helmp 读这个目录：`HELMP_SCRIPT_DIRS="$PWD/examples/scripts" cargo run`。

[Raycast 的 script-commands 仓库](https://github.com/raycast/script-commands)里还有几百个现成脚本。运行之前先看一眼它做了什么：脚本以你当前用户的全部权限运行。
