//! 下载前探测：大小、是否可续传、文件名、ETag。

use percent_encoding::percent_decode_str;
use reqwest::header::{self, HeaderMap};
use reqwest::{Client, StatusCode};
use url::Url;

use crate::error::{Error, Result};
use crate::util::sanitize_file_name;

#[derive(Clone, Debug)]
pub struct ProbeInfo {
    pub final_url: Url,
    pub file_name: String,
    pub size: Option<u64>,
    pub resumable: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

pub async fn probe(client: &Client, url: &Url) -> Result<ProbeInfo> {
    let resp = client
        .get(url.clone())
        .header(header::RANGE, "bytes=0-0")
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Http(status.as_u16()));
    }
    let headers = resp.headers();
    let final_url = resp.url().clone();

    let (size, resumable) = if status == StatusCode::PARTIAL_CONTENT {
        (content_range_total(headers), true)
    } else {
        // 200：服务器忽略了 Range，整体返回
        (content_length(headers), accept_ranges(headers))
    };

    let file_name = file_name_from(headers, &final_url);
    let etag = header_string(headers, header::ETAG);
    let last_modified = header_string(headers, header::LAST_MODIFIED);

    Ok(ProbeInfo { final_url, file_name, size, resumable, etag, last_modified })
}

fn header_string(h: &HeaderMap, name: header::HeaderName) -> Option<String> {
    h.get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
}

fn content_length(h: &HeaderMap) -> Option<u64> {
    h.get(header::CONTENT_LENGTH)?.to_str().ok()?.trim().parse().ok()
}

fn accept_ranges(h: &HeaderMap) -> bool {
    h.get(header::ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("bytes"))
        .unwrap_or(false)
}

/// `Content-Range: bytes 0-0/12345` → 12345；`*` → None
pub fn content_range_total(h: &HeaderMap) -> Option<u64> {
    let v = h.get(header::CONTENT_RANGE)?.to_str().ok()?;
    let total = v.rsplit('/').next()?.trim();
    total.parse().ok()
}

/// 文件名优先级：Content-Disposition filename* > filename > URL 最后一段 > "download"
pub fn file_name_from(h: &HeaderMap, url: &Url) -> String {
    if let Some(cd) = h.get(header::CONTENT_DISPOSITION).and_then(|v| v.to_str().ok())
        && let Some(name) = parse_content_disposition(cd)
    {
        return sanitize_file_name(&name);
    }
    if let Some(name) = file_name_from_url(url) {
        return sanitize_file_name(&name);
    }
    "download".to_string()
}

pub fn file_name_from_url(url: &Url) -> Option<String> {
    let last = url.path_segments()?.rfind(|s| !s.is_empty())?;
    let decoded = percent_decode_str(last).decode_utf8_lossy();
    let decoded = decoded.trim();
    (!decoded.is_empty()).then(|| decoded.to_string())
}

pub fn parse_content_disposition(value: &str) -> Option<String> {
    let mut plain: Option<String> = None;
    for part in value.split(';').map(str::trim) {
        let Some((key, val)) = part.split_once('=') else { continue };
        let key = key.trim().to_ascii_lowercase();
        let val = val.trim();
        if key == "filename*" {
            // RFC 5987: charset'lang'percent-encoded
            let mut it = val.splitn(3, '\'');
            let _charset = it.next();
            let _lang = it.next();
            if let Some(enc) = it.next() {
                let decoded = percent_decode_str(enc.trim_matches('"')).decode_utf8_lossy().to_string();
                if !decoded.is_empty() {
                    return Some(decoded);
                }
            }
        } else if key == "filename" {
            let v = val.trim_matches('"');
            if !v.is_empty() {
                plain = Some(v.to_string());
            }
        }
    }
    plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_filename_star_first() {
        let v = "attachment; filename=\"fallback.zip\"; filename*=UTF-8''%E6%B5%8B%E8%AF%95.zip";
        assert_eq!(parse_content_disposition(v).unwrap(), "测试.zip");
    }

    #[test]
    fn parses_plain_filename() {
        assert_eq!(parse_content_disposition("attachment; filename=\"a b.tar.gz\"").unwrap(), "a b.tar.gz");
        assert_eq!(parse_content_disposition("inline; FILENAME=x.bin").unwrap(), "x.bin");
        assert!(parse_content_disposition("attachment").is_none());
    }

    #[test]
    fn url_file_name() {
        let u = Url::parse("https://example.com/a/b/%E6%96%87%E4%BB%B6.iso?x=1").unwrap();
        assert_eq!(file_name_from_url(&u).unwrap(), "文件.iso");
        let u = Url::parse("https://example.com/").unwrap();
        assert!(file_name_from_url(&u).is_none());
    }

    #[test]
    fn content_range() {
        let mut h = HeaderMap::new();
        h.insert(header::CONTENT_RANGE, "bytes 0-0/12345".parse().unwrap());
        assert_eq!(content_range_total(&h), Some(12345));
        h.insert(header::CONTENT_RANGE, "bytes 0-0/*".parse().unwrap());
        assert_eq!(content_range_total(&h), None);
    }
}
