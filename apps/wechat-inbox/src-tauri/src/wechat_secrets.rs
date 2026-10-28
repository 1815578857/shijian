use base64::{engine::general_purpose::STANDARD, Engine};

use crate::wechat_client::Session;

#[cfg(windows)]
fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new("dev.shijian.inbox", "wechat-session-v1")
        .map_err(|_| "无法打开微信专属凭据条目".into())
}

#[cfg(windows)]
pub fn read_session() -> Result<Option<Session>, String> {
    match entry()?.get_password() {
        Ok(value) => {
            let session: Session =
                serde_json::from_str(&value).map_err(|_| "微信凭据损坏，请断开后重新扫码")?;
            session.validate()?;
            Ok(Some(session))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err("无法读取 Windows 凭据管理器中的微信会话".into()),
    }
}

#[cfg(windows)]
pub fn save_session(session: &Session) -> Result<(), String> {
    session.validate()?;
    let json = serde_json::to_string(session).map_err(|_| "无法编码微信会话")?;
    entry()?
        .set_password(&json)
        .map_err(|_| "无法将微信会话保存到 Windows 凭据管理器".into())
}

#[cfg(windows)]
pub fn delete_session() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err("无法删除本应用的微信会话凭据".into()),
    }
}

#[cfg(not(windows))]
pub fn read_session() -> Result<Option<Session>, String> {
    Ok(None)
}
#[cfg(not(windows))]
pub fn save_session(_: &Session) -> Result<(), String> {
    Err("微信会话目前仅支持 Windows 凭据管理器".into())
}
#[cfg(not(windows))]
pub fn delete_session() -> Result<(), String> {
    Ok(())
}

pub fn protect_context(value: &str) -> Result<String, String> {
    if value.is_empty() {
        return Ok(String::new());
    }
    let bytes = protect(value.as_bytes(), true)?;
    Ok(STANDARD.encode(bytes))
}

pub fn unprotect_context(value: &str) -> Result<String, String> {
    if value.is_empty() {
        return Ok(String::new());
    }
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| "微信回复上下文编码损坏")?;
    String::from_utf8(protect(&bytes, false)?).map_err(|_| "微信回复上下文损坏".into())
}

#[cfg(windows)]
fn protect(value: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(value.len()).map_err(|_| "微信回复上下文过长")?,
        pbData: value.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    // DPAPI binds queued reply contexts to the current Windows user, without any UI.
    unsafe {
        let success = if encrypt {
            CryptProtectData(
                &input,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if success == 0 {
            return Err("Windows 无法加解密微信回复上下文".into());
        }
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData.cast());
        Ok(result)
    }
}

#[cfg(not(windows))]
fn protect(_: &[u8], _: bool) -> Result<Vec<u8>, String> {
    Err("微信回复队列需要 Windows DPAPI 加密支持".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(windows)]
    fn reply_context_is_encrypted_and_roundtrips() {
        let cipher = protect_context("sensitive-context-token").unwrap();
        assert!(!cipher.contains("sensitive"));
        assert_eq!(
            unprotect_context(&cipher).unwrap(),
            "sensitive-context-token"
        );
        assert!(unprotect_context("invalid").is_err());
    }
}
