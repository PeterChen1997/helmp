# 自动更新

[English](updates.md) · 简体中文

Helmp 读取一份很小的 JSON 清单来发现新版本，只有你点了「下载并安装」才会安装。思路同 Sparkle：版本清单、签名校验、替换前征得你同意。

## 使用

- **自动检查。** 打开「设置 › 通用 › 自动检查更新」后，Helmp 启动 20 秒后检查一次，之后每天一次。检查失败（比如离线）1 小时后再试，不弹任何东西。关掉开关，Helmp 就不会自己检查。
- **手动检查。** 菜单栏「检查更新…」、面板命令「检查 Helmp 更新」、设置 › 通用里的按钮，都会打开「设置 › 关于」并立即检查，结果或失败原因显示在那里。
- **发现新版本时，** 菜单栏这一项变成「更新到 x.y.z…」，「设置 › 关于」显示更新说明和「下载并安装」按钮。点之前不下载任何东西。
- **安装过程。** 下载 zip，按清单核对大小和 SHA-256，解压到当前 App 旁边，然后确认：新 App 是 Helmp 的下一个版本（bundle id `dev.helmp.launcher`、版本号与清单一致、不会更旧），签名有效（`codesign --verify --deep --strict`），与正在运行的 Helmp 出自同一个 Developer ID 团队，并且 Gatekeeper 放行（`spctl --assess`）。都通过后 Helmp 才退出；一个小助手等它退出后替换 App（新版本就位前旧版本一直保留，出错就放回去），再重新打开 Helmp。
- **不能原地替换时**（不是从 `/Applications` 或 `~/Applications` 里的 Helmp.app 运行、或那个目录不可写），按钮变成「下载 dmg」：Helmp 下载并校验 dmg，存到「下载」文件夹并打开，由你把 Helmp 拖进「应用程序」。没有 Developer ID 签名的 Helmp（自己编译的）不会原地安装，因为无法确认更新出自同一个开发者，请手动下载新版本。
- **会发送什么。** 只是对清单（安装时还有安装包）的普通 `GET` 请求，User-Agent 为 `Helmp/<版本> (macOS updater)`。没有标识符，没有使用数据。

## 更新源

按顺序尝试，用第一个返回有效清单的：

1. `https://blog.peterchen97.cn/helmp/releases/latest.json`（七牛 CDN，国内可直连）
2. `https://github.com/PeterChen1997/helmp/releases/latest/download/latest.json`（附在最新 GitHub 发布上的 `latest.json`；直接下载，不受匿名 API 的频率限制）

前面的失败、后面的成功时，失败原因打到 stderr；全部失败时逐个列出每个地址的原因。也认得 GitHub Releases API 的回复（`api.github.com/repos/…/releases/latest`）：会去取附在那个发布上的 `latest.json`。

用 `HELMP_UPDATE_FEED`（优先）或 `config.json` 里的 `updates.feed_url` 替换整个列表，多个地址用逗号分隔。

## 清单格式

```json
{
  "version": "0.4.0",
  "published_at": "2026-11-20T08:00:00Z",
  "notes": {"en": "…", "zh-Hans": "…"},
  "minimum_macos": "15.0",
  "assets": [
    {"name": "Helmp-0.4.0.zip", "kind": "zip", "size": 12345678, "sha256": "<64 位十六进制>",
     "urls": ["https://blog.peterchen97.cn/helmp/releases/0.4.0/Helmp-0.4.0.zip",
              "https://github.com/PeterChen1997/helmp/releases/download/v0.4.0/Helmp-0.4.0.zip"]},
    {"name": "Helmp-0.4.0.dmg", "kind": "dmg", "…": "…"}
  ]
}
```

- `version` 按 semver 比较：数字段按数值比，预发布版排在正式版之前，构建元数据不参与比较。
- 所有问题一次报出：安装包名字不安全、`sha256` 不是 64 位十六进制、大小不合理（超过 512 MB）、地址不是 http(s)、没有 zip。
- 下载地址按顺序尝试。下载下来但 `size`、`sha256` 对不上的，直接拒绝，不换下一个地址：清单和文件对不上。
- 不认识的 `kind` 保留但不会被选中，以后加新格式时旧版本照样能读。
- `minimum_macos` 比这台 Mac 的系统新时，「设置 › 关于」会说明，不给安装按钮。

## 命令行

```sh
helmp update check            # 打印 current / latest / status（update-available | up-to-date）/ feed
helmp update install          # 只打印计划并以错误退出：什么都不装
helmp update install --yes    # 下载、校验并安装
```

`install` 不带 `--yes` 的约定与 `helmp plugins submit` 相同：替你操作的 agent 必须先问你。Helmp 图形界面在运行时，`install` 拒绝从命令行替换它，请用菜单栏。

`HELMP_DRY_RUN=1` 时只访问本机地址（`127.0.0.1`、`localhost`、`[::1]`），只下载和校验，绝不解压或替换。`HELMP_UPDATE_TARGET`（要替换的 App）只在这个模式下生效。`tests/e2e_updates.rs` 的端到端测试就是这样对着本地假更新源跑的。

## 发布新版本

1. 在 `CHANGELOG.md` 写好这个版本的一节（先英文，再 `### 简体中文`），两部分就是更新说明。
2. `script/release.sh`：签名、公证并 staple，得到 `target/dist/Helmp-<版本>.zip` 和 `.dmg`。
3. `script/publish-release.sh --dry-run`：写出 `target/dist/latest.json` 和 `<版本>.json`，列出将要上传的内容。
4. `script/publish-release.sh --publish`：
   - 用 `qshell` 上传到七牛：先传 `helmp/releases/<版本>/` 下的安装包，再传 `<版本>.json` 快照，最后传 `helmp/releases/latest.json`，客户端不会读到指向还不存在的文件的清单。所有 key 必须以 `helmp/` 开头（这个 bucket 也是博客根目录）；
   - 刷新 CDN 上的 `latest.json`；
   - GitHub 上没有 `v<版本>` 发布就先创建，再附上 zip、dmg，最后附上 `latest.json`。

不带参数时只生成清单。
