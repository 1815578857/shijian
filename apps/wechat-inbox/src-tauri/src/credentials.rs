use sha2::{Digest, Sha256};

use crate::validation;

// No credential is persisted in SQLite, exported, or included in an error message.
// Deliberately never read the legacy unscoped "model-api-key" account: changing
// service URLs must never reuse a key saved for a different endpoint.
fn account(base_url: &str) -> Result<String, String> {
    let url = validation::normalized_model_base_url(base_url)?;
    if url.is_empty() {
        return Err("保存模型密钥前请先填写模型服务地址".into());
    }
    Ok(format!(
        "model-api-key:{:x}",
        Sha256::digest(url.as_bytes())
    ))
}

#[cfg(windows)]
fn entry(base_url: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new("dev.shijian.inbox", &account(base_url)?)
        .map_err(|_| "无法打开 Windows 凭据管理器".into())
}

#[cfg(windows)]
pub fn read(base_url: &str) -> Result<Option<String>, String> {
    if crate::native_test_mode() {
        return Ok(None);
    }
    if validation::normalized_model_base_url(base_url)?.is_empty() {
        return Ok(None);
    }
    match entry(base_url)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err("无法读取此服务地址在 Windows 凭据管理器中的模型密钥".into()),
    }
}

#[cfg(windows)]
pub fn save(base_url: &str, value: &str) -> Result<(), String> {
    if crate::native_test_mode() {
        return Err("原生测试模式不保存真实模型密钥".into());
    }
    entry(base_url)?
        .set_password(value)
        .map_err(|_| "无法将此服务地址的模型密钥保存到 Windows 凭据管理器".into())
}

#[cfg(windows)]
pub fn delete(base_url: &str) -> Result<(), String> {
    if crate::native_test_mode() {
        return Err("原生测试模式不删除真实模型密钥".into());
    }
    if validation::normalized_model_base_url(base_url)?.is_empty() {
        return Ok(());
    }
    match entry(base_url)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err("无法删除此服务地址在 Windows 凭据管理器中的模型密钥".into()),
    }
}

#[cfg(not(windows))]
pub fn read(base_url: &str) -> Result<Option<String>, String> {
    validation::normalized_model_base_url(base_url)?;
    Ok(None)
}
#[cfg(not(windows))]
pub fn save(base_url: &str, _value: &str) -> Result<(), String> {
    account(base_url)?;
    Err("此版本仅支持 Windows 凭据管理器".into())
}
#[cfg(not(windows))]
pub fn delete(base_url: &str) -> Result<(), String> {
    if validation::normalized_model_base_url(base_url)?.is_empty() {
        return Ok(());
    }
    Err("此版本仅支持 Windows 凭据管理器".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_accounts_are_bound_to_normalized_full_urls() {
        let first = account("https://api.example.com/v1").unwrap();
        assert_eq!(
            first,
            account("  https://API.EXAMPLE.COM:443/v1///  ").unwrap()
        );
        assert_eq!(
            account("https://api.example.com").unwrap(),
            account("https://api.example.com/").unwrap()
        );
        for other in [
            "https://other.example.com/v1",
            "https://api.example.com/v2",
            "https://api.example.com:8443/v1",
            "https://api.example.com/V1",
        ] {
            assert_ne!(first, account(other).unwrap(), "{other}");
        }
        assert_ne!(
            account("http://localhost:8080/v1").unwrap(),
            account("https://localhost:8080/v1").unwrap()
        );
        assert_eq!(first.len(), "model-api-key:".len() + 64);
        assert!(!first.contains("example.com"));
        assert!(account("").is_err());
        assert!(account("https://user:password@example.com").is_err());
        assert!(account("http://not-local.example.com").is_err());
    }
}
