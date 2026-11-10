//! 插件的 secret 偏好设置：只存 macOS 钥匙串（服务名 `dev.helmp.plugins`，账户 `<插件 id>/<偏好设置 id>`），
//! 从不写进 config.json、日志或错误信息。
//!
//! 测试专用覆盖：`HELMP_TEST_SECRETS='{"<插件 id>/<偏好设置 id>": "值"}'`。
//! **只有同时设置了 `HELMP_DATA_DIR`（隔离的数据目录）且 `HELMP_DRY_RUN=1` 时才生效**，这时也绝不碰钥匙串；
//! 正常运行（哪怕设置了这个变量）一律忽略它、照常读钥匙串。这样端到端测试不会读写用户真实的钥匙串，
//! 而这个变量也没法在用户的正常 Helmp 里偷换密钥。

use std::collections::BTreeMap;

use anyhow::{Result, bail};

use crate::i18n::tr;

pub const SERVICE: &str = "dev.helmp.plugins";

pub fn account(plugin: &str, pref: &str) -> String {
    format!("{plugin}/{pref}")
}

/// 测试模式：隔离的数据目录 + 演练开关同时打开。
pub fn test_mode() -> bool {
    test_mode_from(
        std::env::var_os("HELMP_DATA_DIR").is_some(),
        std::env::var("HELMP_DRY_RUN").ok().as_deref(),
    )
}

fn test_mode_from(data_dir_set: bool, dry_run: Option<&str>) -> bool {
    data_dir_set && dry_run == Some("1")
}

/// 解析测试覆盖；不在测试模式时返回 None（覆盖被忽略）。
fn overrides_from(
    data_dir_set: bool,
    dry_run: Option<&str>,
    json: Option<&str>,
) -> Option<BTreeMap<String, String>> {
    if !test_mode_from(data_dir_set, dry_run) {
        return None;
    }
    let Some(json) = json else {
        return Some(BTreeMap::new());
    };
    match serde_json::from_str(json) {
        Ok(map) => Some(map),
        Err(err) => {
            eprintln!("helmp: HELMP_TEST_SECRETS 不是 {{\"键\": \"值\"}} 形式的 JSON，忽略：{err}");
            Some(BTreeMap::new())
        }
    }
}

fn overrides() -> Option<BTreeMap<String, String>> {
    overrides_from(
        std::env::var_os("HELMP_DATA_DIR").is_some(),
        std::env::var("HELMP_DRY_RUN").ok().as_deref(),
        std::env::var("HELMP_TEST_SECRETS").ok().as_deref(),
    )
}

/// 读取；没存过返回 None。
pub fn get(plugin: &str, pref: &str) -> Result<Option<String>> {
    let account = account(plugin, pref);
    if let Some(map) = overrides() {
        return Ok(map.get(&account).cloned());
    }
    crate::models::keychain::get_in(SERVICE, &account)
}

/// 保存；空字符串等于删除。
pub fn set(plugin: &str, pref: &str, value: &str) -> Result<()> {
    if test_mode() {
        bail!(tr(
            "测试模式（HELMP_DATA_DIR + HELMP_DRY_RUN=1）下不写钥匙串",
            "Test mode (HELMP_DATA_DIR + HELMP_DRY_RUN=1) never writes to the Keychain",
        ));
    }
    let account = account(plugin, pref);
    let value = value.trim();
    if value.is_empty() {
        crate::models::keychain::delete_in(SERVICE, &account)
    } else {
        crate::models::keychain::set_in(SERVICE, &account, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: Option<&str> =
        Some(r#"{"dev.helmp.flomo/webhook": "http://127.0.0.1:1/iwh/t/s/"}"#);

    #[test]
    fn override_needs_isolated_data_dir_and_dry_run() {
        // 覆盖只能在端到端测试的隔离环境里生效：正常运行的 Helmp 若认这个变量，
        // 别的程序设个环境变量就能把用户的 webhook 换成自己的地址。
        let honored = overrides_from(true, Some("1"), JSON).unwrap();
        assert_eq!(
            honored.get("dev.helmp.flomo/webhook").map(String::as_str),
            Some("http://127.0.0.1:1/iwh/t/s/")
        );
        assert_eq!(overrides_from(false, Some("1"), JSON), None);
        assert_eq!(overrides_from(true, None, JSON), None);
        assert_eq!(overrides_from(true, Some("0"), JSON), None);
        assert_eq!(overrides_from(false, None, JSON), None);
    }

    #[test]
    fn test_mode_without_override_reads_nothing() {
        // 测试模式下没给值就当没存过，绝不回落到真实钥匙串。
        assert_eq!(overrides_from(true, Some("1"), None), Some(BTreeMap::new()));
        assert_eq!(
            overrides_from(true, Some("1"), Some("not json")),
            Some(BTreeMap::new())
        );
    }

    #[test]
    fn accounts_are_namespaced_by_plugin() {
        // 两个插件都有叫 token 的偏好设置时不能串号。
        assert_ne!(account("a.b", "token"), account("a.c", "token"));
    }
}
