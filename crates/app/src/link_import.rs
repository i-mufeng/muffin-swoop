//! 有大小上限的链接文件导入；选中文件后只预填，不自动开始下载。
use std::io::Read;
use std::path::Path;

pub const MAX_IMPORT_BYTES: usize = 2 * 1024 * 1024;

pub fn read_links(path: &Path, limit: usize) -> Result<String, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take((MAX_IMPORT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    decode_links(&bytes, limit)
}

/// Multiple dropped link files are validated as one draft, never partially imported.
pub fn read_link_files(paths: &[std::path::PathBuf], limit: usize) -> Result<String, String> {
    if paths.is_empty() || paths.len() > 1000 {
        return Err(swoop_i18n::tr!("一次最多导入 1000 个链接文件").into());
    }
    let mut text = String::new();
    let mut total = 0u64;
    for path in paths {
        let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(swoop_i18n::tr!("只接受普通链接文件").into());
        }
        let extension = path.extension().and_then(|extension| extension.to_str()).unwrap_or("").to_ascii_lowercase();
        if !matches!(extension.as_str(), "txt" | "url") {
            return Err(swoop_i18n::tr!("请拖入 .txt 或 .url 链接文件，或直接拖入浏览器链接").into());
        }
        total = total.saturating_add(metadata.len());
        if total > MAX_IMPORT_BYTES as u64 {
            return Err(swoop_i18n::tr!("链接文件合计不能超过 2 MB").into());
        }
        text.push_str(&read_links(path, limit)?);
        text.push('\n');
        if text.len() > MAX_IMPORT_BYTES {
            return Err(swoop_i18n::tr!("链接文件合计不能超过 2 MB").into());
        }
    }
    decode_links(text.as_bytes(), limit)
}

pub fn decode_links(bytes: &[u8], limit: usize) -> Result<String, String> {
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(swoop_i18n::tr!("链接文件不能超过 2 MB").into());
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| swoop_i18n::tr!("请使用 UTF-8 编码的链接文本"))?
        .trim_start_matches('\u{feff}');
    let input = if text.lines().any(|line| line.trim().eq_ignore_ascii_case("[InternetShortcut]")) {
        text.lines()
            .filter_map(|line| line.trim().split_once('='))
            .find(|(key, _)| key.eq_ignore_ascii_case("URL"))
            .map(|(_, url)| url)
            .ok_or(swoop_i18n::tr!("快捷方式中没有 URL"))?
    } else {
        text
    };
    Ok(crate::batch::parse(input, limit)?.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multiple_dropped_files_combine_atomically_with_total_limit() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("first.txt");
        let b = root.path().join("second.url");
        std::fs::write(&a, "https://example.test/one").unwrap();
        std::fs::write(&b, "[InternetShortcut]\nURL=https://example.test/two").unwrap();
        assert_eq!(
            read_link_files(&[a.clone(), b.clone()], 10).unwrap(),
            "https://example.test/one\nhttps://example.test/two"
        );
        assert!(read_link_files(&[a.clone(), b.clone()], 1).is_err());
        std::fs::write(&b, "invalid").unwrap();
        assert!(read_link_files(&[a.clone(), b.clone()], 10).is_err());
        let other = root.path().join("payload.exe");
        std::fs::write(&other, "https://example.test/three").unwrap();
        assert!(read_link_files(&[other], 10).is_err());
        std::fs::write(&b, vec![b'a'; MAX_IMPORT_BYTES]).unwrap();
        assert!(read_link_files(&[a, b], 10).is_err());
    }
    #[test]
    fn imports_bom_plain_ranges_and_windows_shortcut_without_executing_it() {
        assert_eq!(
            decode_links(b"\xef\xbb\xbfhttps://a.test/{01..02}.zip\nhttps://a.test/01.zip", 10).unwrap(),
            "https://a.test/01.zip\nhttps://a.test/02.zip"
        );
        assert_eq!(
            decode_links(b"[InternetShortcut]\r\nURL=https://a.test/f.zip\r\nIconFile=ignored.exe", 10).unwrap(),
            "https://a.test/f.zip"
        );
    }
    #[test]
    fn rejects_large_or_invalid_files_atomically() {
        assert!(decode_links(&vec![b'a'; MAX_IMPORT_BYTES + 1], 10).is_err());
        assert!(decode_links(b"https://a.test/ok\nnot a url", 10).is_err());
        assert!(decode_links(b"[InternetShortcut]\nURL=file:///private/file", 10).is_err());
        assert!(decode_links(b"\xff", 10).is_err());
        assert!(decode_links(b"https://a.test/{1..100}", 5).is_err());
    }
}
