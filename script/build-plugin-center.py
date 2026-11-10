#!/usr/bin/env python3
"""从插件注册表（helmp-plugins 的 index.json）生成落地页的「插件中心」。

产物：
  site/plugins/index.html        插件中心页面（完全静态）
  site/plugins/icons/<id>.png    插件图标（仅本地注册表模式；URL 模式直接引用 CDN）
  site/index.html                替换 <!-- plugin-center:start --> … <!-- plugin-center:end --> 之间的预览区块

用法（只用标准库，建议 `python3 -I` 运行）：
  python3 -I script/build-plugin-center.py [--registry <目录或 URL>] [--out <site 目录>]

--registry 默认：~/code_hub/helmp-plugins 存在就用它，否则用 jsDelivr 上的 main 分支。
校验不通过就非零退出，且不写任何文件（不会留下半截页面）。
同样的输入两次生成的结果逐字节相同：不写生成时间，插件按 id 排序。
"""

import argparse
import html
import json
import os
import re
import shutil
import sys
import tempfile
import urllib.parse
import urllib.request

DEFAULT_LOCAL = os.path.expanduser("~/code_hub/helmp-plugins")
DEFAULT_URL = "https://cdn.jsdelivr.net/gh/PeterChen1997/helmp-plugins@main/"
REGISTRY_REPO = "https://github.com/PeterChen1997/helmp-plugins"
DOCS_EN = "https://github.com/PeterChen1997/helmp/blob/HEAD/docs/plugin-center.md"
DOCS_ZH = "https://github.com/PeterChen1997/helmp/blob/HEAD/docs/plugin-center.zh-CN.md"

# Helmp 自带插件的 id 前缀。注册表里的三个种子插件和 App 内置插件是同一份（同 id），
# 用户装了 Helmp 就已经有了；卡片上标出来，免得用户以为还要再装一次。
# 只看前缀即可：dev.helmp.* 命名空间只有维护者能用，第三方插件不会落在这个前缀下。
BUNDLED_PREFIX = "dev.helmp."

START_MARK = "<!-- plugin-center:start -->"
END_MARK = "<!-- plugin-center:end -->"

ID_RE = re.compile(r"^[a-z0-9.-]+$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
COMMIT_RE = re.compile(r"^[0-9a-f]{7,40}$")
ICON_RE = re.compile(r"^[A-Za-z0-9_-][A-Za-z0-9._-]*\.png$")
PNG_MAGIC = b"\x89PNG\r\n\x1a\n"
MAX_INDEX_BYTES = 8 * 1024 * 1024

PLUGIN_KEYS = {
    "id", "version", "name", "description", "author", "homepage", "license", "icon",
    "keywords", "commands", "permissions", "permissions_text", "review", "source", "files",
}


class RegistryError(Exception):
    """index.json 内容不合格。"""


def fail(msg):
    print("build-plugin-center: 中止：" + msg, file=sys.stderr)
    sys.exit(1)


# ---------- 校验 ----------

def need(cond, where, msg):
    if not cond:
        raise RegistryError(f"{where}: {msg}")


def is_str(v):
    return isinstance(v, str)


def is_int(v):
    # bool 是 int 的子类，要单独排除
    return isinstance(v, int) and not isinstance(v, bool)


def check_text(v, where):
    need(is_str(v) and v.strip() != "", where, "必须是非空字符串")


def check_localized(v, where):
    need(isinstance(v, dict), where, "必须是 {en, zh-Hans} 对象")
    for lang in ("en", "zh-Hans"):
        check_text(v.get(lang), f"{where}.{lang}")


def check_str_list(v, where):
    need(isinstance(v, list), where, "必须是数组")
    for i, item in enumerate(v):
        check_text(item, f"{where}[{i}]")


def check_https(v, where):
    check_text(v, where)
    parsed = urllib.parse.urlsplit(v)
    need(parsed.scheme == "https" and parsed.netloc != "", where, f"必须是 https 地址：{v!r}")


def validate_index(data):
    need(isinstance(data, dict), "index.json", "顶层必须是对象")
    need(data.get("schema") == 1 and is_int(data.get("schema")), "schema", f"只支持 1，实际是 {data.get('schema')!r}")
    commit = data.get("commit")
    need(commit is None or (is_str(commit) and COMMIT_RE.match(commit)), "commit", f"必须是 git sha 或 null：{commit!r}")
    need(is_str(data.get("generated_at")), "generated_at", "必须是字符串")
    plugins = data.get("plugins")
    need(isinstance(plugins, list), "plugins", "必须是数组")

    seen = set()
    for n, p in enumerate(plugins):
        where = f"plugins[{n}]"
        need(isinstance(p, dict), where, "必须是对象")
        pid = p.get("id")
        need(is_str(pid) and ID_RE.match(pid) and not pid.startswith(".") and ".." not in pid,
             f"{where}.id", f"只允许 [a-z0-9.-]：{pid!r}")
        where = f"plugins[{pid}]"
        need(pid not in seen, where, "id 重复")
        seen.add(pid)

        unknown = sorted(set(p) - PLUGIN_KEYS)
        if unknown:
            # 新增字段不影响页面渲染，不中止，但要大声说出来
            print(f"build-plugin-center: 警告：{where} 有未识别字段 {unknown}，已忽略", file=sys.stderr)

        check_text(p.get("version"), f"{where}.version")
        check_localized(p.get("name"), f"{where}.name")
        if p.get("description") is not None:
            check_localized(p.get("description"), f"{where}.description")
        check_text(p.get("author"), f"{where}.author")
        if p.get("homepage") is not None:
            check_https(p.get("homepage"), f"{where}.homepage")
        if p.get("license") is not None:
            check_text(p.get("license"), f"{where}.license")
        icon = p.get("icon")
        need(icon is None or (is_str(icon) and ICON_RE.match(icon)), f"{where}.icon",
             f"必须是插件目录下的 .png 文件名或 null：{icon!r}")
        check_str_list(p.get("keywords"), f"{where}.keywords")
        need(is_int(p.get("commands")) and p["commands"] >= 0, f"{where}.commands", "必须是非负整数")
        check_str_list(p.get("permissions"), f"{where}.permissions")
        pt = p.get("permissions_text")
        need(isinstance(pt, dict), f"{where}.permissions_text", "必须是 {en, zh-Hans} 对象")
        for lang in ("en", "zh-Hans"):
            check_str_list(pt.get(lang), f"{where}.permissions_text.{lang}")
        # 两种语言逐条对应（页面上按行配对显示）
        need(len(pt["en"]) == len(pt["zh-Hans"]), f"{where}.permissions_text", "en 和 zh-Hans 条数必须一致")
        need(isinstance(p.get("review"), list), f"{where}.review", "必须是数组")
        check_https(p.get("source"), f"{where}.source")
        files = p.get("files")
        need(isinstance(files, list), f"{where}.files", "必须是数组")
        for i, f in enumerate(files):
            fw = f"{where}.files[{i}]"
            need(isinstance(f, dict), fw, "必须是对象")
            check_text(f.get("path"), f"{fw}.path")
            need(is_str(f.get("sha256")) and SHA256_RE.match(f["sha256"]), f"{fw}.sha256", "必须是 64 位小写十六进制")
            need(is_int(f.get("size")) and f["size"] >= 0, f"{fw}.size", "必须是非负整数")
            need(isinstance(f.get("executable"), bool), f"{fw}.executable", "必须是布尔值")
        if icon is not None:
            need(any(f.get("path") == icon for f in files), f"{where}.icon", f"{icon} 不在 files 列表里")
    return data


# ---------- 读取注册表 ----------

def load_registry(registry):
    """返回 (index 数据, 图标来源)。图标来源：本地模式是 {id: 本地路径}，URL 模式是 {id: CDN 地址}。"""
    is_url = registry.startswith("https://") or registry.startswith("http://")
    if is_url:
        if not registry.startswith("https://"):
            fail("注册表 URL 必须是 https")
        base = registry if registry.endswith("/") else registry + "/"
        try:
            with urllib.request.urlopen(base + "index.json", timeout=30) as resp:
                raw = resp.read(MAX_INDEX_BYTES + 1)
        except OSError as e:
            fail(f"下载 {base}index.json 失败：{e}")
        if len(raw) > MAX_INDEX_BYTES:
            fail("index.json 过大")
    else:
        base = os.path.abspath(registry)
        path = os.path.join(base, "index.json")
        try:
            with open(path, "rb") as fh:
                raw = fh.read(MAX_INDEX_BYTES + 1)
        except OSError as e:
            fail(f"读取 {path} 失败：{e}")
        if len(raw) > MAX_INDEX_BYTES:
            fail("index.json 过大")

    try:
        data = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as e:
        fail(f"index.json 不是合法的 UTF-8 JSON：{e}")
    try:
        validate_index(data)
    except RegistryError as e:
        fail(f"index.json 校验失败：{e}")

    icons = {}
    for p in data["plugins"]:
        if p["icon"] is None:
            continue
        if is_url:
            icons[p["id"]] = f"{base}plugins/{p['id']}/{p['icon']}"
        else:
            src = os.path.join(base, "plugins", p["id"], p["icon"])
            try:
                with open(src, "rb") as fh:
                    head = fh.read(8)
            except OSError as e:
                fail(f"读取图标失败：{e}")
            if head != PNG_MAGIC:
                fail(f"{src} 不是 PNG 文件")
            icons[p["id"]] = src
    return data, icons, is_url


# ---------- 渲染 ----------

def esc(s):
    return html.escape(s, quote=True)


def bi(en, zh):
    """双语文字。两种语言相同就只写一份（没有 JS 时不会重复显示）。"""
    if en == zh:
        return esc(en)
    return f'<span lang="en">{esc(en)}</span><span lang="zh-CN">{esc(zh)}</span>'


def first_sentence(text, zh):
    """首页预览只放一句话。"""
    if zh:
        m = re.match(r"^(.+?[。！？])", text)
    else:
        m = re.match(r"^(.+?[.!?])(?=\s|$)", text)
    return m.group(1) if m else text


def icon_src(pid, icons, is_url, prefix):
    """prefix：页面相对 site/plugins/ 的路径（插件中心页是空串，首页是 plugins/）。"""
    if pid not in icons:
        return None
    if is_url:
        return icons[pid]
    return f"{prefix}icons/{pid}.png"


def icon_html(p, icons, is_url, prefix, cls, size):
    src = icon_src(p["id"], icons, is_url, prefix)
    if src is None:
        # 没有图标：用名称首字母做一个渐变色块
        letter = esc(p["name"]["en"].strip()[:1].upper())
        return f'<span class="{cls} pc-icon-fallback" aria-hidden="true">{letter}</span>'
    return f'<img class="{cls}" src="{esc(src)}" width="{size}" height="{size}" alt="" loading="lazy">'


HEAD_SCRIPT = """  <script>
    // 首帧之前定好语言，避免先闪一下英文再切中文。读写 localStorage 都可能抛错（隐私模式等）。
    (function () {
      var lang = null;
      try { lang = localStorage.getItem("helmp-lang"); } catch (e) {}
      if (lang !== "en" && lang !== "zh") {
        lang = "en";
        var list = navigator.languages || [navigator.language || "en"];
        for (var i = 0; i < list.length; i++) {
          var l = String(list[i]).toLowerCase();
          if (l.indexOf("zh") === 0) { lang = "zh"; break; }
          if (l.indexOf("en") === 0) { break; }
        }
      }
      document.documentElement.setAttribute("data-lang", lang);
      document.documentElement.lang = lang === "zh" ? "zh-CN" : "en";
    })();
  </script>"""


def render_card(p, icons, is_url):
    pid = p["id"]
    name = bi(p["name"]["en"], p["name"]["zh-Hans"])
    meta = [f"v{esc(p['version'])}",
            f'<span lang="en">by {esc(p["author"])}</span><span lang="zh-CN">作者 {esc(p["author"])}</span>']
    if p["license"] is not None:
        meta.append(esc(p["license"]))
    meta_html = ' <span aria-hidden="true">·</span> '.join(meta)

    out = [f'        <article class="card pc-card" id="plugin-{esc(pid)}">']
    out.append('          <div class="pc-card-head">')
    out.append("            " + icon_html(p, icons, is_url, "", "pc-icon", 56))
    out.append('            <div class="pc-card-title">')
    out.append(f"              <h3>{name}</h3>")
    out.append(f'              <p class="pc-meta">{meta_html}</p>')
    out.append("            </div>")
    out.append("          </div>")
    if pid.startswith(BUNDLED_PREFIX):
        out.append('          <p class="pc-badge">' + bi("Bundled with Helmp", "Helmp 已自带") + "</p>")
    if p["description"] is not None:
        out.append(f'          <p class="pc-desc">{bi(p["description"]["en"], p["description"]["zh-Hans"])}</p>')
    out.append('          <div class="pc-perms">')
    out.append('            <h4>' + bi("Permissions", "权限") + "</h4>")
    en_list = p["permissions_text"]["en"]
    zh_list = p["permissions_text"]["zh-Hans"]
    if not en_list:
        out.append('            <p class="pc-perm-none">' + bi("No extra permissions", "无额外权限") + "</p>")
    else:
        out.append("            <ul>")
        for en, zh in zip(en_list, zh_list):
            out.append(f"              <li>{bi(en, zh)}</li>")
        out.append("            </ul>")
    out.append("          </div>")
    install = "helmp://install-plugin?id=" + urllib.parse.quote(pid, safe="")
    out.append('          <div class="pc-actions">')
    out.append(f'            <a class="btn btn-primary pc-btn" href="{esc(install)}">'
               + bi("Install in Helmp", "在 Helmp 中安装") + "</a>")
    out.append(f'            <a class="btn btn-glass pc-btn" href="{esc(p["source"])}">'
               + bi("View source", "查看源码") + "</a>")
    out.append("          </div>")
    out.append("        </article>")
    return "\n".join(out)


def render_page(data, plugins, icons, is_url):
    count = len(plugins)
    cards = "\n".join(render_card(p, icons, is_url) for p in plugins)
    if not plugins:
        cards = '        <p class="pc-empty">' + bi("No plugins yet.", "还没有插件。") + "</p>"

    commit = data["commit"]
    if commit:
        short = commit[:7]
        source_line = (
            f'      <p class="pc-source-line">'
            f'<span lang="en">Plugin data from the registry at commit</span>'
            f'<span lang="zh-CN">插件数据来自注册表提交</span> '
            f'<a href="{esc(REGISTRY_REPO)}/commit/{esc(commit)}"><code>{esc(short)}</code></a></p>\n'
        )
    else:
        source_line = ""

    return f"""<!doctype html>
<html lang="en" data-lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
  <title>Plugin Center · Helmp</title>
  <meta name="description" content="Reviewed plugins for Helmp, the native macOS launcher. Every file is checked against its SHA-256 on install, and plugins stay off until you approve their permissions.">
  <meta name="color-scheme" content="light dark">
  <meta name="theme-color" content="#f4f6fb" media="(prefers-color-scheme: light)">
  <meta name="theme-color" content="#0b0d16" media="(prefers-color-scheme: dark)">
  <meta property="og:type" content="website">
  <meta property="og:title" content="Helmp Plugin Center">
  <meta property="og:description" content="Reviewed plugins for Helmp, the native macOS launcher.">
  <meta property="og:url" content="https://blog.peterchen97.cn/helmp/plugins/">
  <meta property="og:image" content="https://blog.peterchen97.cn/helmp/assets/icon-512.png">
  <link rel="icon" type="image/png" href="../assets/icon-64.png">
  <link rel="apple-touch-icon" href="../assets/icon-512.png">
  <link rel="stylesheet" href="../assets/style.css">
  <!-- 本文件由 script/build-plugin-center.py 生成，不要手改。 -->
{HEAD_SCRIPT}
</head>
<body data-title-en="Plugin Center · Helmp" data-title-zh="插件中心 · Helmp">
  <div class="backdrop" aria-hidden="true">
    <div class="blob blob-a"></div>
    <div class="blob blob-b"></div>
    <div class="blob blob-c"></div>
  </div>

  <header class="topbar">
    <div class="topbar-inner">
      <a class="brand" href="../">
        <img src="../assets/icon-64.png" width="28" height="28" alt="">
        <span>Helmp</span>
      </a>
      <nav class="nav" aria-label="Sections">
        <a href="../"><span lang="en">Home</span><span lang="zh-CN">首页</span></a>
        <a href="./" aria-current="page"><span lang="en">Plugin Center</span><span lang="zh-CN">插件中心</span></a>
        <a href="#submit"><span lang="en">Submit</span><span lang="zh-CN">提交插件</span></a>
      </nav>
      <div class="lang-switch" role="group" aria-label="Language / 语言">
        <button type="button" data-set-lang="en" aria-pressed="true">EN</button>
        <button type="button" data-set-lang="zh" aria-pressed="false">中文</button>
      </div>
    </div>
  </header>

  <main>
    <section class="hero pc-hero">
      <h1><span lang="en">Plugin Center</span><span lang="zh-CN">插件中心</span></h1>
      <p class="lede">
        <span lang="en">Every plugin here is reviewed by hand before it is listed. When you install one, Helmp checks each file against its SHA-256. New plugins stay off until you turn them on, and Helmp lists their permissions for you to approve first.</span>
        <span lang="zh-CN">这里的每个插件都由维护者人工审核后才上架。在 Helmp 里安装时，会逐个校验文件的 SHA-256；装好后默认关闭，启用前先列出权限让你确认。</span>
      </p>
      <ul class="pc-trust">
        <li class="glass"><span lang="en">Reviewed by hand</span><span lang="zh-CN">人工审核</span></li>
        <li class="glass"><span lang="en">SHA-256 checked</span><span lang="zh-CN">SHA-256 校验</span></li>
        <li class="glass"><span lang="en">Off until you approve</span><span lang="zh-CN">确认后才启用</span></li>
      </ul>
    </section>

    <section id="catalog" class="section pc-section">
      <h2><span lang="en">All plugins</span><span lang="zh-CN">全部插件</span> <span class="pc-count">{count}</span></h2>
      <div class="pc-grid">
{cards}
      </div>
    </section>

    <section id="submit" class="section">
      <h2><span lang="en">Submit your plugin</span><span lang="zh-CN">提交你的插件</span></h2>
      <p class="section-lede">
        <span lang="en">The easiest way is to let a coding agent do it with you. Everything goes through a pull request that the maintainer reviews by hand.</span>
        <span lang="zh-CN">最省事的办法是让 coding agent 陪你一起做。所有插件都通过 PR 提交，由维护者人工审核。</span>
      </p>
      <div class="pc-submit">
        <div class="glass install">
          <h3 class="pc-submit-title"><span lang="en">With Claude Code or Codex</span><span lang="zh-CN">用 Claude Code 或 Codex</span></h3>
          <ol class="steps">
            <li>
              <span lang="en">Connect Helmp's MCP server (<code>helmp mcp</code>) to Claude Code or Codex.</span>
              <span lang="zh-CN">在 Claude Code 或 Codex 里接入 Helmp 的 MCP 服务（<code>helmp mcp</code>）。</span>
            </li>
            <li>
              <span lang="en">Ask it to write the plugin. It checks the result with <code>helmp plugins validate</code> first.</span>
              <span lang="zh-CN">让它帮你写插件。它会先用 <code>helmp plugins validate</code> 校验。</span>
            </li>
            <li>
              <span lang="en">It always asks you before submitting. Once you agree, it opens a pull request to <code>PeterChen1997/helmp-plugins</code> with your own GitHub account through the <code>gh</code> CLI.</span>
              <span lang="zh-CN">提交前它一定先问你。你确认后，它用你自己的 GitHub 账号（<code>gh</code> CLI）向 <code>PeterChen1997/helmp-plugins</code> 开 PR。</span>
            </li>
            <li>
              <span lang="en">The maintainer reviews it by hand, then it shows up here.</span>
              <span lang="zh-CN">维护者人工审核通过后，插件就会出现在这里。</span>
            </li>
          </ol>
        </div>
        <div class="glass install">
          <h3 class="pc-submit-title"><span lang="en">By hand</span><span lang="zh-CN">手动提交</span></h3>
          <p class="pc-submit-text">
            <span lang="en">Run this in a terminal. It prints what it is about to do; add <code>--yes</code> to actually do it.</span>
            <span lang="zh-CN">在终端里运行下面的命令。它会先打印将要做什么，加上 <code>--yes</code> 才真正执行。</span>
          </p>
          <div class="code">
            <button type="button" class="copy" data-copy-target="submit-cmd">
              <span lang="en">Copy</span><span lang="zh-CN">复制</span>
            </button>
<pre id="submit-cmd"><code>helmp plugins submit &lt;plugin-dir&gt;</code></pre>
          </div>
          <ul class="pc-links">
            <li><a href="{esc(REGISTRY_REPO)}/blob/main/CONTRIBUTING.md"><span lang="en">Contributing guide</span><span lang="zh-CN">贡献指南</span></a></li>
            <li><a href="{esc(DOCS_EN)}" lang="en">Plugin Center docs</a><a href="{esc(DOCS_ZH)}" lang="zh-CN">插件中心文档</a></li>
            <li><a href="{esc(REGISTRY_REPO)}"><span lang="en">Registry on GitHub</span><span lang="zh-CN">GitHub 上的注册表</span></a></li>
          </ul>
        </div>
      </div>
    </section>
  </main>

  <footer class="footer">
    <div class="footer-inner">
      <div class="footer-brand">
        <img src="../assets/icon-64.png" width="22" height="22" alt="">
        <span>Helmp</span>
      </div>
{source_line}      <p>
        <a href="https://github.com/PeterChen1997/helmp">GitHub</a>
        <span aria-hidden="true">·</span>
        <a href="https://github.com/PeterChen1997/helmp/blob/HEAD/LICENSE"><span lang="en">MIT License</span><span lang="zh-CN">MIT 协议</span></a>
        <span aria-hidden="true">·</span>
        <span>© 2026 Peter Chen</span>
      </p>
    </div>
  </footer>

  <script src="../assets/main.js"></script>
</body>
</html>
"""


def render_home_block(plugins, icons, is_url):
    """首页 #plugins 区块末尾的精简预览（不含标记行本身，缩进 6 格）。"""
    out = ['      <div class="pc-preview">']
    out.append('        <h3 class="pc-preview-title"><span lang="en">Plugin Center</span><span lang="zh-CN">插件中心</span></h3>')
    out.append('        <p class="pc-preview-lede">')
    out.append('          <span lang="en">Reviewed plugins you can install from Helmp in one click.</span>')
    out.append('          <span lang="zh-CN">经过人工审核、在 Helmp 里一键安装的插件。</span>')
    out.append("        </p>")
    out.append('        <div class="pc-mini-grid">')
    for p in plugins:
        out.append(f'          <a class="glass pc-mini" href="plugins/#plugin-{esc(p["id"])}">')
        out.append("            " + icon_html(p, icons, is_url, "plugins/", "pc-mini-icon", 40))
        out.append('            <span class="pc-mini-text">')
        out.append(f'              <span class="pc-mini-name">{bi(p["name"]["en"], p["name"]["zh-Hans"])}</span>')
        if p["description"] is not None:
            en = first_sentence(p["description"]["en"], False)
            zh = first_sentence(p["description"]["zh-Hans"], True)
            out.append(f'              <span class="pc-mini-desc">{bi(en, zh)}</span>')
        out.append("            </span>")
        out.append("          </a>")
    out.append("        </div>")
    out.append('        <div class="cta">')
    out.append('          <a class="btn btn-glass" href="plugins/"><span lang="en">Browse the Plugin Center</span><span lang="zh-CN">浏览插件中心</span></a>')
    out.append("        </div>")
    out.append("      </div>")
    return "\n".join(out)


def replace_block(home, block):
    if home.count(START_MARK) != 1 or home.count(END_MARK) != 1:
        fail(f"首页里 {START_MARK} 和 {END_MARK} 必须各出现一次")
    start = home.index(START_MARK)
    end = home.index(END_MARK)
    if end < start:
        fail("首页里插件中心的结束标记在开始标记前面")
    # 开始标记行保留原样，结束标记沿用原来的行首缩进
    line_start = home.rfind("\n", 0, end) + 1
    indent = home[line_start:end]
    if indent.strip() != "":
        fail("首页里插件中心的结束标记必须独占一行")
    return home[: start + len(START_MARK)] + "\n" + block + "\n" + indent + home[end:]


# ---------- 写文件（先全部渲染好，再原子替换） ----------

def write_atomic(path, content):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=".tmp-", dir=os.path.dirname(path))
    try:
        with os.fdopen(fd, "wb") as fh:
            fh.write(content)
        os.chmod(tmp, 0o644)
        os.replace(tmp, path)
    except BaseException:
        if os.path.exists(tmp):
            os.unlink(tmp)
        raise


def main():
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    ap = argparse.ArgumentParser(description="从插件注册表生成落地页的插件中心")
    ap.add_argument("--registry", default=None, help="注册表本地目录或 https URL")
    ap.add_argument("--out", default=os.path.join(repo, "site"), help="site 目录")
    args = ap.parse_args()

    registry = args.registry
    if registry is None:
        registry = DEFAULT_LOCAL if os.path.isdir(DEFAULT_LOCAL) else DEFAULT_URL
    out = os.path.abspath(args.out)
    home_path = os.path.join(out, "index.html")
    if not os.path.isfile(home_path):
        fail(f"找不到 {home_path}")

    data, icons, is_url = load_registry(registry)
    plugins = sorted(data["plugins"], key=lambda p: p["id"])

    page = render_page(data, plugins, icons, is_url).encode("utf-8")
    with open(home_path, "r", encoding="utf-8", newline="") as fh:
        home = fh.read()
    new_home = replace_block(home, render_home_block(plugins, icons, is_url)).encode("utf-8")

    # 渲染全部成功后才动磁盘
    icons_dir = os.path.join(out, "plugins", "icons")
    if is_url:
        # URL 模式直接引用 CDN，清掉上次本地模式留下的图标，免得部署出过期文件
        if os.path.isdir(icons_dir):
            shutil.rmtree(icons_dir)
    else:
        os.makedirs(icons_dir, exist_ok=True)
        wanted = {f"{pid}.png" for pid in icons}
        for name in sorted(os.listdir(icons_dir)):
            if name not in wanted and not name.startswith("."):
                os.unlink(os.path.join(icons_dir, name))
        for pid, src in sorted(icons.items()):
            with open(src, "rb") as fh:
                write_atomic(os.path.join(icons_dir, f"{pid}.png"), fh.read())

    write_atomic(os.path.join(out, "plugins", "index.html"), page)
    if new_home != home.encode("utf-8"):
        write_atomic(home_path, new_home)

    mode = "URL" if is_url else "本地"
    print(f"build-plugin-center: 已从{mode}注册表 {registry} 生成 {len(plugins)} 个插件 → {out}/plugins/")


if __name__ == "__main__":
    main()
