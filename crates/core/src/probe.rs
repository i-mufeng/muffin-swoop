//! 下载前探测：大小、是否可续传、文件名、ETag。

use encoding_rs::{Encoding, GB18030, UTF_8};
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
    /// 去掉参数后的小写 MIME，如 `application/pdf`
    pub content_type: Option<String>,
}

/// 任务级请求头里由引擎自己管理、调用方传了也忽略的头
const MANAGED_HEADERS: [header::HeaderName; 6] = [
    header::RANGE,
    header::HOST,
    header::CONTENT_LENGTH,
    header::CONNECTION,
    header::TRANSFER_ENCODING,
    header::PROXY_AUTHORIZATION,
];

/// 把调用方给的请求头（如浏览器带来的 Cookie / Referer / User-Agent）转成 [`HeaderMap`]。
/// 名称或值不合法时报错；引擎自己管理的头（Range 等）直接丢弃。
pub fn build_headers(pairs: &[(String, String)]) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        let name = header::HeaderName::from_bytes(name.trim().as_bytes())
            .map_err(|_| Error::InvalidHeader(name.clone()))?;
        if MANAGED_HEADERS.contains(&name) {
            continue;
        }
        let value = header::HeaderValue::from_str(value.trim())
            .map_err(|_| Error::InvalidHeader(name.to_string()))?;
        map.append(name, value);
    }
    Ok(map)
}

/// `headers` 会附加到请求上，其中的 User-Agent 覆盖 client 默认值。
/// client 必须关闭自动重定向，才能由这里逐跳执行敏感头过滤；通常使用 Engine::probe_url。
pub async fn probe(client: &Client, url: &Url, headers: &HeaderMap) -> Result<ProbeInfo> {
    let mut request_headers = headers.clone();
    request_headers.insert(header::RANGE, header::HeaderValue::from_static("bytes=0-0"));
    let resp = crate::network::get_following_redirects(client, url, &request_headers).await?;
    probe_info(resp.status(), resp.headers(), resp.url().clone())
}

pub(crate) async fn probe_network(network: &crate::network::NetworkRuntime, url: &Url, headers: &HeaderMap) -> Result<ProbeInfo> {
    let mut request_headers = headers.clone();
    request_headers.insert(header::RANGE, header::HeaderValue::from_static("bytes=0-0"));
    let resp = network.get(url, &request_headers).await?;
    probe_info(resp.status(), resp.headers(), resp.url().clone())
}

fn probe_info(status: StatusCode, headers: &HeaderMap, final_url: Url) -> Result<ProbeInfo> {
    if !status.is_success() {
        return Err(Error::Http(status.as_u16()));
    }



    let (size, resumable) = if status == StatusCode::PARTIAL_CONTENT {
        (content_range_total(headers), true)
    } else {
        // 200：服务器忽略了 Range，整体返回
        (content_length(headers), accept_ranges(headers))
    };

    let file_name = file_name_from(headers, &final_url);
    let etag = header_string(headers, header::ETAG);
    let last_modified = header_string(headers, header::LAST_MODIFIED);
    let content_type = header_string(headers, header::CONTENT_TYPE)
        .and_then(|v| v.split(';').next().map(|m| m.trim().to_ascii_lowercase()))
        .filter(|m| !m.is_empty());

    Ok(ProbeInfo {
        final_url,
        file_name,
        size,
        resumable,
        etag,
        last_modified,
        content_type,
    })
}

fn header_string(h: &HeaderMap, name: header::HeaderName) -> Option<String> {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn content_length(h: &HeaderMap) -> Option<u64> {
    h.get(header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
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

/// 文件名优先级：Content-Disposition filename* > filename > URL（见 [`file_name_from_url`]）> "download"。
/// 名字没有扩展名或是 `.php` 这类脚本扩展名时，按 Content-Type 补上真实扩展名。
pub fn file_name_from(h: &HeaderMap, url: &Url) -> String {
    let name = h
        .get(header::CONTENT_DISPOSITION)
        .and_then(|v| parse_content_disposition(&decode_text(v.as_bytes())))
        .or_else(|| file_name_from_url(url))
        .map(|name| sanitize_file_name(&name))
        .unwrap_or_else(|| "download".to_string());
    let mime = h
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|m| m.trim().to_ascii_lowercase());
    match mime.as_deref().and_then(extension_for_mime) {
        Some(ext) => fix_extension(name, ext),
        None => name,
    }
}

/// 路径最后一段带正常扩展名就用它；否则（`download.php?file=a.zip`、`/get?id=1`）
/// 先看查询参数里有没有像文件名的值，再退回路径最后一段。
pub fn file_name_from_url(url: &Url) -> Option<String> {
    let segment = url
        .path_segments()
        .and_then(|mut s| s.rfind(|s| !s.is_empty()))
        .map(|last| decode_text(&percent_decode_str(last).collect::<Vec<u8>>()).trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(name) = &segment
        && extension(name).is_some_and(|ext| !is_script_extension(ext))
    {
        return segment;
    }
    file_name_from_query(url).or(segment)
}

fn file_name_from_query(url: &Url) -> Option<String> {
    // 越靠前越可信
    const KEYS: [&str; 8] = ["filename", "file_name", "fname", "fn", "file", "name", "title", "attname"];
    let mut best: Option<(usize, String)> = None;
    for pair in url.query()?.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = key.to_ascii_lowercase();
        // form-urlencoded 的 + 是空格；按字节解码后再判断 UTF-8 / GBK
        let value = decode_text(&percent_decode_str(&value.replace('+', " ")).collect::<Vec<u8>>());
        if key == "response-content-disposition" || key == "rscd" {
            // S3 / OSS / COS 等预签名链接把下载名放在这里
            if let Some(name) = parse_content_disposition(&value).filter(|n| !n.trim().is_empty()) {
                return Some(name);
            }
        } else if let Some(rank) = KEYS.iter().position(|k| *k == key) {
            let last = value.rsplit(['/', '\\']).next().unwrap_or("").trim();
            let looks_like_file = extension(last).is_some_and(|ext| !is_script_extension(ext));
            if looks_like_file && best.as_ref().is_none_or(|(r, _)| rank < *r) {
                best = Some((rank, last.to_string()));
            }
        }
    }
    best.map(|(_, name)| name)
}

/// 链接本身指向一个文件（如 `jdk.zip`），响应却是网页：多半被跳到了登录页、验证页或过期提示页。
/// 返回链接里的文件名，用于提示。`.php` 这类脚本链接和本来就是网页的链接不判断。
pub fn html_instead_of_file(requested: &Url, info: &ProbeInfo) -> Option<String> {
    if !matches!(info.content_type.as_deref(), Some("text/html" | "application/xhtml+xml")) {
        return None;
    }
    let name = file_name_from_url(requested)?;
    let ext = extension(&name)?.to_ascii_lowercase();
    let page = matches!(ext.as_str(), "html" | "htm" | "shtml" | "xhtml" | "mht" | "mhtml");
    (!page && !is_script_extension(&ext)).then_some(name)
}

/// 像扩展名的后缀：1–5 个字母数字，且点不在开头
fn extension(name: &str) -> Option<&str> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && (1..=5).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric())).then_some(ext)
}

fn is_script_extension(ext: &str) -> bool {
    SCRIPT_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
}

pub fn parse_content_disposition(value: &str) -> Option<String> {
    let mut plain: Option<String> = None;
    for (key, val) in disposition_params(value) {
        if key == "filename*" {
            // RFC 5987: charset'lang'percent-encoded
            let mut it = val.splitn(3, '\'');
            let charset = it.next().unwrap_or("");
            let _lang = it.next();
            if let Some(enc) = it.next() {
                let bytes: Vec<u8> = percent_decode_str(enc).collect();
                let decoded = match Encoding::for_label(charset.trim().as_bytes()) {
                    Some(encoding) if encoding != UTF_8 => encoding.decode_without_bom_handling(&bytes).0.into_owned(),
                    _ => decode_text(&bytes),
                };
                if !decoded.is_empty() {
                    return Some(decoded);
                }
            }
        } else if key == "filename" && !val.is_empty() {
            plain = Some(if val.starts_with("=?") {
                decode_rfc2047(&val)
            } else if val.contains('%') {
                // 不少服务端直接把 URL 编码后的名字放进 filename，浏览器会解码，这里也解
                decode_text(&percent_decode_str(&val).collect::<Vec<u8>>())
            } else {
                val
            });
        }
    }
    plain
}

/// 拆出 `key=value` 参数：分号只在引号外才是分隔符，引号内支持 `\"` 转义
fn disposition_params(value: &str) -> Vec<(String, String)> {
    let mut params = Vec::new();
    let mut chars = value.chars().peekable();
    while chars.peek().is_some() {
        let mut key = String::new();
        let mut has_value = false;
        for c in chars.by_ref() {
            match c {
                '=' => {
                    has_value = true;
                    break;
                }
                ';' => break,
                c => key.push(c),
            }
        }
        if !has_value {
            continue;
        }
        let mut val = String::new();
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.next_if_eq(&'"').is_some() {
            while let Some(c) = chars.next() {
                match c {
                    '\\' => val.extend(chars.next()),
                    '"' => break,
                    c => val.push(c),
                }
            }
            chars.by_ref().take_while(|c| *c != ';').for_each(drop);
        } else {
            val = chars.by_ref().take_while(|c| *c != ';').collect::<String>().trim().to_string();
        }
        params.push((key.trim().to_ascii_lowercase(), val));
    }
    params
}

/// RFC 2047 编码字：`=?UTF-8?B?5rWL6K+VLnppcA==?=`，部分 Java / 邮件系统出来的服务端会这么写
fn decode_rfc2047(value: &str) -> String {
    use base64::Engine as _;
    let mut out = String::new();
    for word in value.split_whitespace() {
        let decoded = word
            .strip_prefix("=?")
            .and_then(|w| w.strip_suffix("?="))
            .and_then(|w| {
                let mut it = w.splitn(3, '?');
                let (charset, kind, text) = (it.next()?, it.next()?, it.next()?);
                let bytes = match kind.to_ascii_uppercase().as_str() {
                    "B" => base64::engine::general_purpose::STANDARD.decode(text).ok()?,
                    "Q" => percent_decode_str(&text.replace('_', " ").replace('=', "%")).collect(),
                    _ => return None,
                };
                let encoding = Encoding::for_label(charset.as_bytes()).unwrap_or(UTF_8);
                Some(encoding.decode_without_bom_handling(&bytes).0.into_owned())
            });
        match decoded {
            Some(text) => out.push_str(&text),
            None => {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(word);
            }
        }
    }
    out
}

/// 服务端常把 UTF-8 或 GBK 原始字节直接写进响应头/URL：先按 UTF-8，不合法再按 GB18030。
fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => GB18030.decode_without_bom_handling(bytes).0.into_owned(),
    }
}

/// 下载链接常是 `download.php?id=…`，这类扩展名说明不了文件内容
const SCRIPT_EXTENSIONS: [&str; 14] =
    ["php", "php3", "php5", "asp", "aspx", "ashx", "jsp", "jspx", "do", "action", "cgi", "pl", "cfm", "fcgi"];

fn fix_extension(name: String, ext: &str) -> String {
    match extension(&name) {
        Some(cur) if is_script_extension(cur) => format!("{}.{ext}", &name[..name.len() - cur.len() - 1]),
        Some(_) => name,
        // 没有扩展名，或"扩展名"其实只是名字里的点（如 "v1.2 beta"）
        None => format!("{name}.{ext}"),
    }
}

fn extension_for_mime(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/jpeg" | "image/jpg" | "image/pjpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
        "image/tiff" => "tif",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        "video/quicktime" => "mov",
        "audio/mpeg" => "mp3",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        "audio/wav" | "audio/x-wav" => "wav",
        "application/pdf" => "pdf",
        "application/zip" | "application/x-zip-compressed" => "zip",
        "application/x-rar-compressed" | "application/vnd.rar" => "rar",
        "application/x-7z-compressed" => "7z",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-tar" => "tar",
        "application/x-bittorrent" => "torrent",
        "application/vnd.android.package-archive" => "apk",
        "application/x-apple-diskimage" => "dmg",
        "application/x-msdownload" | "application/x-msdos-program" => "exe",
        "application/x-msi" => "msi",
        "application/json" => "json",
        "text/plain" => "txt",
        "text/html" => "html",
        _ => return None,
    })
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
        assert_eq!(
            parse_content_disposition("attachment; filename=\"a b.tar.gz\"").unwrap(),
            "a b.tar.gz"
        );
        assert_eq!(
            parse_content_disposition("inline; FILENAME=x.bin").unwrap(),
            "x.bin"
        );
        assert!(parse_content_disposition("attachment").is_none());
    }

    #[test]
    fn decodes_raw_and_encoded_names() {
        let mut h = HeaderMap::new();
        let url = Url::parse("https://pic.netbian.com/e/extend/netbiandownload.php?token=x").unwrap();
        // 彼岸图网：UTF-8 原始字节直接写进 Content-Disposition，Content-Type 是不规范的 image/jpg
        let raw = "attachment; filename=\"【彼岸图网】伽罗 4K壁纸.jpg\"";
        h.insert(header::CONTENT_DISPOSITION, header::HeaderValue::from_bytes(raw.as_bytes()).unwrap());
        h.insert(header::CONTENT_TYPE, "image/jpg".parse().unwrap());
        assert_eq!(file_name_from(&h, &url), "【彼岸图网】伽罗 4K壁纸.jpg");

        let (gbk, _, _) = GB18030.encode("attachment; filename=\"测试.rar\"");
        h.insert(header::CONTENT_DISPOSITION, header::HeaderValue::from_bytes(&gbk).unwrap());
        assert_eq!(file_name_from(&h, &url), "测试.rar");

        h.insert(header::CONTENT_DISPOSITION, "attachment; filename=\"%E6%B5%8B%E8%AF%95.zip\"".parse().unwrap());
        assert_eq!(file_name_from(&h, &url), "测试.zip");

        assert_eq!(parse_content_disposition("attachment; filename*=GBK''%B2%E2%CA%D4.txt").unwrap(), "测试.txt");
    }

    #[test]
    fn script_extension_replaced_by_content_type() {
        let url = Url::parse("https://example.com/down.php?id=1").unwrap();
        let mut h = HeaderMap::new();
        assert_eq!(file_name_from(&h, &url), "down.php", "不知道类型就不动");
        h.insert(header::CONTENT_TYPE, "image/jpeg".parse().unwrap());
        assert_eq!(file_name_from(&h, &url), "down.jpg");
        h.insert(header::CONTENT_TYPE, "text/html; charset=utf-8".parse().unwrap());
        assert_eq!(file_name_from(&h, &url), "down.html");

        let url = Url::parse("https://example.com/files/abcdef").unwrap();
        h.insert(header::CONTENT_TYPE, "application/pdf".parse().unwrap());
        assert_eq!(file_name_from(&h, &url), "abcdef.pdf");
        let url = Url::parse("https://example.com/files/report.docx").unwrap();
        h.insert(header::CONTENT_TYPE, "application/octet-stream".parse().unwrap());
        assert_eq!(file_name_from(&h, &url), "report.docx");
        h.insert(header::CONTENT_TYPE, "application/zip".parse().unwrap());
        assert_eq!(file_name_from(&h, &url), "report.docx", "已有正常扩展名就保留");
    }

    #[test]
    fn content_disposition_edge_cases() {
        assert_eq!(parse_content_disposition("attachment; filename=\"a;b.zip\"").unwrap(), "a;b.zip");
        assert_eq!(parse_content_disposition(r#"attachment; filename="say \"hi\".txt""#).unwrap(), "say \"hi\".txt");
        assert_eq!(parse_content_disposition("attachment;filename=x.bin;size=3").unwrap(), "x.bin");
        assert_eq!(parse_content_disposition("attachment; filename=\"=?UTF-8?B?5rWL6K+VLnppcA==?=\"").unwrap(), "测试.zip");
        assert_eq!(parse_content_disposition("attachment; filename==?utf-8?Q?a_b=E6=B5=8B.txt?=").unwrap(), "a b测.txt");
    }

    #[test]
    fn url_query_file_name() {
        let name = |u: &str| file_name_from_url(&Url::parse(u).unwrap());
        assert_eq!(name("https://x.com/download.php?id=3&file=dir%2Fa%20b.zip").unwrap(), "a b.zip");
        assert_eq!(name("https://x.com/get?name=%B2%E2%CA%D4.rar").unwrap(), "测试.rar");
        assert_eq!(name("https://x.com/get?title=foo&filename=bar.7z").unwrap(), "bar.7z");
        assert_eq!(
            name("https://b.s3.amazonaws.com/k/abc?X-Amz-Signature=1&response-content-disposition=attachment%3B%20filename%3D%22r.pdf%22").unwrap(),
            "r.pdf"
        );
        assert_eq!(name("https://x.com/files/real.iso?name=other.zip").unwrap(), "real.iso", "路径有正常扩展名优先");
        assert_eq!(name("https://x.com/dl.php?token=abc").unwrap(), "dl.php", "查询里没有名字就退回路径");
        assert_eq!(name("https://x.com/dl.php?file=next.php").unwrap(), "dl.php");
    }

    #[test]
    fn detects_html_instead_of_file() {
        let info = |mime: &str| ProbeInfo {
            final_url: Url::parse("https://login.example.com/oauth2/v1/authorize").unwrap(),
            file_name: "authorize.html".into(),
            size: None,
            resumable: false,
            etag: None,
            last_modified: None,
            content_type: Some(mime.into()),
        };
        let zip = Url::parse("https://download.oracle.com/otn/java/jdk/x/jdk-8u503-windows-i586.zip").unwrap();
        assert_eq!(html_instead_of_file(&zip, &info("text/html")).unwrap(), "jdk-8u503-windows-i586.zip");
        assert!(html_instead_of_file(&zip, &info("application/zip")).is_none());
        for page in ["https://x.com/a/index.html", "https://x.com/dl.php?token=1", "https://x.com/files/abc"] {
            assert!(html_instead_of_file(&Url::parse(page).unwrap(), &info("text/html")).is_none(), "{page}");
        }
    }

    #[test]
    fn url_file_name() {
        let u = Url::parse("https://example.com/a/b/%E6%96%87%E4%BB%B6.iso?x=1").unwrap();
        assert_eq!(file_name_from_url(&u).unwrap(), "文件.iso");
        let u = Url::parse("https://example.com/%CE%C4%BC%FE.iso").unwrap();
        assert_eq!(file_name_from_url(&u).unwrap(), "文件.iso", "GBK 编码的路径");
        let u = Url::parse("https://example.com/").unwrap();
        assert!(file_name_from_url(&u).is_none());
    }

    #[test]
    fn build_headers_validates_and_drops_managed() {
        let pairs = vec![
            ("Cookie".to_string(), "a=1; b=2".to_string()),
            ("range".to_string(), "bytes=5-".to_string()),
            ("User-Agent".to_string(), " Mozilla/5.0 ".to_string()),
        ];
        let h = build_headers(&pairs).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[header::COOKIE], "a=1; b=2");
        assert_eq!(h[header::USER_AGENT], "Mozilla/5.0");
        assert!(h.get(header::RANGE).is_none());

        assert!(build_headers(&[("Bad Name".into(), "x".into())]).is_err());
        assert!(build_headers(&[("X-A".into(), "line\nbreak".into())]).is_err());
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
