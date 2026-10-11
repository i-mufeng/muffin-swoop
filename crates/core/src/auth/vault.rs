//! OS credential APIs only; no subprocess command line or plaintext fallback.
use super::AuthSecret;
use crate::{Error, Result};

pub trait CredentialVault: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<AuthSecret>>;
    fn set(&self, key: &str, secret: &AuthSecret) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemCredentialVault;
const SERVICE: &str = "xin.muffin.swoop.http-auth";

#[cfg(target_os = "macos")]
impl CredentialVault for SystemCredentialVault {
    fn get(&self, key: &str) -> Result<Option<AuthSecret>> {
        use security_framework::passwords::{PasswordOptions, generic_password};
        match generic_password(PasswordOptions::new_generic_password(SERVICE, key)) {
            Ok(bytes) => {
                let bytes = zeroize::Zeroizing::new(bytes);
                let text = std::str::from_utf8(&bytes).map_err(|_| Error::Authentication("系统凭据编码无效".into()))?;
                Ok(Some(AuthSecret::new(text.to_owned())?))
            }
            Err(error) if error.code() == -25300 => Ok(None),
            Err(error) => Err(Error::Authentication(format!("钥匙串读取失败（系统错误 {}）", error.code()))),
        }
    }
    fn set(&self, key: &str, secret: &AuthSecret) -> Result<()> {
        security_framework::passwords::set_generic_password(SERVICE, key, secret.expose().as_bytes())
            .map_err(|error| Error::Authentication(format!("钥匙串保存失败（系统错误 {}）", error.code())))
    }
    fn delete(&self, key: &str) -> Result<()> {
        match security_framework::passwords::delete_generic_password(SERVICE, key) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == -25300 => Ok(()),
            Err(error) => Err(Error::Authentication(format!("钥匙串删除失败（系统错误 {}）", error.code()))),
        }
    }
}

#[cfg(windows)]
impl CredentialVault for SystemCredentialVault {
    fn get(&self, key: &str) -> Result<Option<AuthSecret>> {
        use windows_sys::Win32::Security::Credentials::*;
        let target = target(key);
        let mut credential = std::ptr::null_mut();
        // CredRead allocates the record and blob together; CredFree owns both.
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0 {
            let code = std::io::Error::last_os_error().raw_os_error().unwrap_or_default();
            return if code == 1168 { Ok(None) } else { Err(vault_error("读取", code)) };
        }
        let bytes = unsafe {
            let record = &*credential;
            let bytes = if record.CredentialBlobSize == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(record.CredentialBlob, record.CredentialBlobSize as usize).to_vec()
            };
            if record.CredentialBlobSize != 0 {
                use zeroize::Zeroize;
                std::slice::from_raw_parts_mut(record.CredentialBlob, record.CredentialBlobSize as usize).zeroize();
            }
            CredFree(credential.cast());
            zeroize::Zeroizing::new(bytes)
        };
        let text = std::str::from_utf8(&bytes).map_err(|_| Error::Authentication("系统凭据编码无效".into()))?;
        Ok(Some(AuthSecret::new(text.to_owned())?))
    }
    fn set(&self, key: &str, secret: &AuthSecret) -> Result<()> {
        use windows_sys::Win32::Security::Credentials::*;
        let mut target = target(key);
        let mut username: Vec<u16> = "Swoop\0".encode_utf16().collect();
        let mut bytes = zeroize::Zeroizing::new(secret.expose().as_bytes().to_vec());
        let record = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            CredentialBlobSize: bytes.len() as u32,
            CredentialBlob: bytes.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: username.as_mut_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        if unsafe { CredWriteW(&record, 0) } == 0 {
            return Err(vault_error("保存", std::io::Error::last_os_error().raw_os_error().unwrap_or_default()));
        }
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        use windows_sys::Win32::Security::Credentials::*;
        if unsafe { CredDeleteW(target(key).as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            let code = std::io::Error::last_os_error().raw_os_error().unwrap_or_default();
            if code != 1168 {
                return Err(vault_error("删除", code));
            }
        }
        Ok(())
    }
}
#[cfg(windows)]
fn target(key: &str) -> Vec<u16> {
    format!("{SERVICE}/{key}\0").encode_utf16().collect()
}
#[cfg(windows)]
fn vault_error(action: &str, code: i32) -> Error {
    Error::Authentication(format!("Windows 凭据{action}失败（系统错误 {code}）"))
}

#[cfg(not(any(target_os = "macos", windows)))]
impl CredentialVault for SystemCredentialVault {
    fn get(&self, _: &str) -> Result<Option<AuthSecret>> {
        Err(unsupported())
    }
    fn set(&self, _: &str, _: &AuthSecret) -> Result<()> {
        Err(unsupported())
    }
    fn delete(&self, _: &str) -> Result<()> {
        Err(unsupported())
    }
}
#[cfg(not(any(target_os = "macos", windows)))]
fn unsupported() -> Error {
    let _ = SERVICE;
    Error::Authentication("此平台未配置系统凭据库，请使用会话凭据".into())
}
