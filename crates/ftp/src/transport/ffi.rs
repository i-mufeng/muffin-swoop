//! The only raw libcurl boundary. Numeric ABI constants below are from the bundled curl.h;
//! curl-sys 0.4 does not yet expose these long-standing options/info values.

use std::{
    ffi::{CString, c_long},
    net::IpAddr,
};

use curl::easy::{Easy2, Handler};

use crate::{Error, FtpConfig, FtpDataMode, FtpMetadata, FtpTlsMode, Result};

const ACCEPT_TIMEOUT_MS: curl_sys::CURLoption = curl_sys::CURLOPTTYPE_LONG + 212;
const CONTENT_LENGTH_DOWNLOAD_T: curl_sys::CURLINFO = 0x600000 + 15;
const FILETIME_T: curl_sys::CURLINFO = 0x600000 + 14;

fn check(code: curl_sys::CURLcode) -> Result<()> {
    if code == curl_sys::CURLE_OK {
        Ok(())
    } else {
        Err(Error::Transfer { code, status: 0 })
    }
}

fn set_long<H: Handler>(
    easy: &Easy2<H>,
    option: curl_sys::CURLoption,
    value: c_long,
) -> Result<()> {
    // SAFETY: only CURLOPTTYPE_LONG options are passed, with the required C long value.
    check(unsafe { curl_sys::curl_easy_setopt(easy.raw(), option, value) })
}

pub(super) fn configure<H: Handler>(
    easy: &Easy2<H>,
    config: &FtpConfig,
    tls: FtpTlsMode,
) -> Result<()> {
    // Restrict protocols even if an upstream URL check were to regress.
    set_long(easy, curl_sys::CURLOPT_PROTOCOLS, (1 << 2) | (1 << 3))?;
    set_long(easy, curl_sys::CURLOPT_FTP_USE_EPSV, 1)?;
    set_long(easy, curl_sys::CURLOPT_FTP_USE_EPRT, 1)?;
    set_long(easy, curl_sys::CURLOPT_TRANSFERTEXT, 0)?;
    set_long(easy, curl_sys::CURLOPT_FTP_SKIP_PASV_IP, 1)?;
    set_long(
        easy,
        curl_sys::CURLOPT_FTP_RESPONSE_TIMEOUT,
        config.read_timeout_secs as c_long,
    )?;
    set_long(
        easy,
        ACCEPT_TIMEOUT_MS,
        (config.connect_timeout_secs * 1000) as c_long,
    )?;
    set_long(
        easy,
        curl_sys::CURLOPT_USE_SSL,
        match tls {
            FtpTlsMode::Plain => curl_sys::CURLUSESSL_NONE as c_long,
            FtpTlsMode::Explicit | FtpTlsMode::Implicit => curl_sys::CURLUSESSL_ALL as c_long,
        },
    )?;
    if let FtpDataMode::Active(active) = &config.data_mode {
        let mut address = match active.address {
            Some(IpAddr::V6(address)) => format!("[{address}]"),
            Some(IpAddr::V4(address)) => address.to_string(),
            None => "-".to_string(),
        };
        if let Some((first, last)) = active.port_range {
            address.push_str(&format!(":{first}-{last}"));
        }
        let address =
            CString::new(address).map_err(|_| Error::InvalidConfig("invalid active address"))?;
        // SAFETY: FTPPORT takes a NUL-terminated string and copies it before returning.
        check(unsafe {
            curl_sys::curl_easy_setopt(easy.raw(), curl_sys::CURLOPT_FTPPORT, address.as_ptr())
        })?;
    }
    Ok(())
}

fn get_offset<H: Handler>(
    easy: &Easy2<H>,
    info: curl_sys::CURLINFO,
) -> Result<curl_sys::curl_off_t> {
    let mut value: curl_sys::curl_off_t = -1;
    // SAFETY: both callers use CURLINFO_OFF_T info IDs and provide a valid curl_off_t output pointer.
    check(unsafe {
        curl_sys::curl_easy_getinfo(easy.raw(), info, &mut value as *mut curl_sys::curl_off_t)
    })?;
    Ok(value)
}

pub(super) fn metadata<H: Handler>(easy: &Easy2<H>) -> Result<FtpMetadata> {
    let size = get_offset(easy, CONTENT_LENGTH_DOWNLOAD_T)?;
    let modified = get_offset(easy, FILETIME_T)?;
    Ok(FtpMetadata {
        size: (size >= 0).then_some(size as u64),
        modified_unix: (modified >= 0).then_some(modified),
    })
}
