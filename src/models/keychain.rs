//! API Key 存在 macOS 钥匙串（通用密码），服务名 `config::KEYCHAIN_SERVICE`，
//! 账户名 `ModelConfig::keychain_account()`。config.json 里永远不出现 Key。

use anyhow::{Context as _, Result};
use security_framework::base::Error;
use security_framework::passwords;

use crate::config::{KEYCHAIN_SERVICE, ModelConfig};
use crate::i18n::{tr, trf};

/// `errSecItemNotFound`：条目不存在。
const NOT_FOUND: i32 = -25300;

fn is_not_found(err: &Error) -> bool {
    err.code() == NOT_FOUND
}

pub fn get_in(service: &str, account: &str) -> Result<Option<String>> {
    match passwords::get_generic_password(service, account) {
        Ok(bytes) => Ok(Some(String::from_utf8(bytes).context(tr(
            "钥匙串里的 API Key 不是 UTF-8 文本",
            "The API key in the Keychain isn't valid UTF-8 text",
        ))?)),
        Err(err) if is_not_found(&err) => Ok(None),
        // 条目存在但不让读：多半是旧签名的 Helmp 存的（钥匙串按签名授权），换签名后新程序没有权限。
        Err(err) if is_denied(&err) => Err(err).context(tr(
            "钥匙串不允许这个版本的 Helmp 读取已存的 Key（Helmp 更新后常见）。到设置 › 模型重新粘贴一次 Key 即可。",
            "The Keychain won't let this version of Helmp read the saved key (common after an update). Paste the key again in Settings › Models.",
        )),
        Err(err) => Err(err).with_context(|| {
            trf!(
                "读取钥匙串（{service} / {account}）失败",
                "Couldn't read from the Keychain ({service} / {account})"
            )
        }),
    }
}

/// errSecAuthFailed / errSecInteractionNotAllowed / userCanceled：条目在，但当前程序拿不到。
fn is_denied(err: &Error) -> bool {
    matches!(err.code(), -25293 | -25308 | -128)
}

/// 写入：先删后建。直接覆盖会沿用旧条目的访问授权，旧签名存的 Key 换签名后仍然读不出来。
pub fn set_in(service: &str, account: &str, key: &str) -> Result<()> {
    delete_in(service, account)?;
    passwords::set_generic_password(service, account, key.as_bytes()).with_context(|| {
        trf!(
            "写入钥匙串（{service} / {account}）失败",
            "Couldn't save to the Keychain ({service} / {account})"
        )
    })
}

/// 删除；条目本来就不存在不算错。
pub fn delete_in(service: &str, account: &str) -> Result<()> {
    match passwords::delete_generic_password(service, account) {
        Ok(()) => Ok(()),
        Err(err) if is_not_found(&err) => Ok(()),
        Err(err) => Err(err).with_context(|| {
            trf!(
                "删除钥匙串（{service} / {account}）失败",
                "Couldn't delete from the Keychain ({service} / {account})"
            )
        }),
    }
}

/// 某个模型的 API Key；没存过返回 None。
pub fn api_key(model: &ModelConfig) -> Result<Option<String>> {
    get_in(KEYCHAIN_SERVICE, &model.keychain_account())
}

/// 保存 API Key；空字符串等于删除。
pub fn set_api_key(model: &ModelConfig, key: &str) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        delete_in(KEYCHAIN_SERVICE, &model.keychain_account())
    } else {
        set_in(KEYCHAIN_SERVICE, &model.keychain_account(), key)
    }
}

pub fn delete_api_key(model: &ModelConfig) -> Result<()> {
    delete_in(KEYCHAIN_SERVICE, &model.keychain_account())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_reads_overwrites_and_deletes() {
        // Key 只存在钥匙串里：存不进、读不回或删不掉，用户要么每次重填，要么移除模型后 Key 还残留。
        // 用独立的测试服务名，绝不碰真实条目；结束时删除。
        let service = format!("dev.helmp.launcher.test-{}", std::process::id());
        let account = "model:keychain-test";
        assert_eq!(get_in(&service, account).unwrap(), None);
        set_in(&service, account, "sk-first").unwrap();
        assert_eq!(
            get_in(&service, account).unwrap().as_deref(),
            Some("sk-first")
        );
        set_in(&service, account, "sk-second").unwrap();
        assert_eq!(
            get_in(&service, account).unwrap().as_deref(),
            Some("sk-second")
        );
        delete_in(&service, account).unwrap();
        assert_eq!(get_in(&service, account).unwrap(), None);
        // 再删一次也不报错：移除一个从没填过 Key 的模型是正常操作。
        delete_in(&service, account).unwrap();
    }
}
