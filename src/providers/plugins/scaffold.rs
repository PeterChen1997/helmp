//! 「新建插件」：在用户插件目录里生成一个能直接用的模板插件（清单 + 两个脚本），作为照着改的起点。
//! 模板覆盖三种常见写法：打开链接、运行脚本、脚本驱动的列表。

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::i18n::trf;

const MANIFEST: &str = r#"{
  "$comment": "Helmp plugin manifest. Full reference: https://github.com/PeterChen1997/helmp/blob/HEAD/docs/plugins.md · 中文文档：docs/plugins.zh-CN.md. Check it with: helmp plugins validate <this folder>",
  "schemaVersion": 1,
  "id": "__ID__",
  "name": { "en": "My Plugin", "zh-Hans": "我的插件" },
  "description": { "en": "A starting point: a quicklink, a script and a script-driven list.", "zh-Hans": "起步模板：一个快捷链接、一个脚本、一个脚本驱动的列表。" },
  "version": "0.1.0",
  "author": "You",
  "icon": "puzzle",
  "tint": "indigo",
  "preferences": [
    {
      "$comment": "Preferences show up in Settings › Plugins. Use \"type\": \"secret\" for tokens: they are kept in the Keychain only.",
      "id": "greeting",
      "type": "text",
      "title": { "en": "Greeting", "zh-Hans": "问候语" },
      "default": "Hello"
    }
  ],
  "commands": [
    {
      "$comment": "Select the command (type its keyword) and press Tab, then type: the text becomes {{query}}. Use the |url filter inside URLs.",
      "id": "search",
      "title": { "en": "Search Wikipedia", "zh-Hans": "搜索维基百科" },
      "keywords": ["wiki"],
      "icon": "globe",
      "argument": { "placeholder": { "en": "Search terms", "zh-Hans": "搜索内容" }, "required": true },
      "action": { "type": "open", "url": "https://en.wikipedia.org/w/index.php?search={{query|url}}" }
    },
    {
      "$comment": "Scripts get their inputs as separate argv entries (never through a shell) and environment variables.",
      "id": "hello",
      "title": { "en": "Say Hello", "zh-Hans": "打个招呼" },
      "keywords": ["hello"],
      "icon": "square-terminal",
      "argument": { "placeholder": { "en": "Name", "zh-Hans": "名字" } },
      "action": {
        "type": "script",
        "path": "bin/hello.sh",
        "args": ["{{query}}"],
        "env": { "GREETING": "{{pref.greeting}}" },
        "output": "message"
      }
    },
    {
      "$comment": "A list command runs its script while you type and shows the items it prints (Alfred Script Filter JSON).",
      "id": "list",
      "title": { "en": "Example List", "zh-Hans": "示例列表" },
      "keywords": ["mylist"],
      "icon": "list",
      "list": {
        "script": { "path": "bin/list.sh", "args": ["{{query}}"] },
        "action": { "type": "copy", "text": "{{item.arg}}" }
      }
    }
  ]
}
"#;

const HELLO: &str = r#"#!/bin/sh
# Helmp 插件示例脚本：$1 是关键词后面输入的文字，GREETING 来自偏好设置。
# Example script: $1 is the text typed after the keyword; GREETING comes from the plugin's preferences.
echo "${GREETING:-Hello}, ${1:-world}!"
"#;

const LIST: &str = r#"#!/bin/sh
# 列表脚本：向 stdout 输出 Alfred Script Filter JSON。$1 是关键词后面输入的文字。
# List script: print Alfred Script Filter JSON to stdout. $1 is the text typed after the keyword.
q=$(printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g')
cat <<EOF
{"items": [
  {"uid": "upper", "title": "Upper: $(printf '%s' "$q" | tr '[:lower:]' '[:upper:]')", "arg": "$(printf '%s' "$q" | tr '[:lower:]' '[:upper:]')"},
  {"uid": "lower", "title": "Lower: $(printf '%s' "$q" | tr '[:upper:]' '[:lower:]')", "arg": "$(printf '%s' "$q" | tr '[:upper:]' '[:lower:]')"}
]}
EOF
"#;

/// 在 `root` 下建一个新插件目录（`com.example.my-plugin`，重名时加序号），返回目录路径。
pub fn create(root: &Path) -> Result<PathBuf> {
    let (id, dir) = (1..1000)
        .map(|n| {
            let id = if n == 1 {
                "com.example.my-plugin".to_string()
            } else {
                format!("com.example.my-plugin-{n}")
            };
            let dir = root.join(&id);
            (id, dir)
        })
        .find(|(_, dir)| !dir.exists())
        .context(trf!(
            "{} 里的示例插件太多了，先删掉几个",
            "Too many example plugins in {}; remove some first",
            root.display()
        ))?;
    write_starter(&dir, &id)?;
    Ok(dir)
}

/// 在 `dir`（不存在则新建）里写入起步模板，插件 id 为 `id`。`helmp mcp` 的脚手架工具也用它。
pub fn write_starter(dir: &Path, id: &str) -> Result<()> {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin)
        .with_context(|| trf!("创建 {}", "Couldn't create {}", bin.display()))?;
    let write = |path: PathBuf, text: &str, executable: bool| -> Result<()> {
        std::fs::write(&path, text)
            .with_context(|| trf!("写入 {}", "Couldn't write {}", path.display()))?;
        if executable {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).with_context(
                || {
                    trf!(
                        "设置 {} 可执行",
                        "Couldn't make {} executable",
                        path.display()
                    )
                },
            )?;
        }
        Ok(())
    };
    write(bin.join("hello.sh"), HELLO, true)?;
    write(bin.join("list.sh"), LIST, true)?;
    write(
        dir.join("plugin.json"),
        &MANIFEST.replace("__ID__", id),
        false,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::plugins::registry::{Source, load_dir};

    #[test]
    fn scaffold_is_a_valid_plugin_that_runs() {
        // 「新建插件」生成的模板本身必须通过校验、脚本能跑，否则用户第一步就卡住。
        let root = std::env::temp_dir().join(format!("helmp-scaffold-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let first = create(&root).unwrap();
        let second = create(&root).unwrap();
        assert_ne!(first, second, "重名时要换一个目录，不能覆盖用户改过的插件");
        let entry = load_dir(&first, Source::User);
        let plugin = entry
            .plugin()
            .unwrap_or_else(|| panic!("{:?}", entry.loaded.as_ref().err()));
        assert_eq!(plugin.id(), "com.example.my-plugin");
        assert_eq!(
            load_dir(&second, Source::User).id,
            "com.example.my-plugin-2"
        );

        let list = plugin
            .manifest
            .command("list")
            .and_then(|c| c.list.as_ref())
            .unwrap();
        let items = super::super::exec::run_list(plugin, "Example", list, "Ab \"c\"").unwrap();
        assert_eq!(items[0].fields["arg"], "AB \"C\"");
        assert_eq!(items[1].fields["arg"], "ab \"c\"");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
