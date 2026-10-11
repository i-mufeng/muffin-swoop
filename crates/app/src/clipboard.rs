//! 剪贴板监听：每秒最多读一次，只对启动后新出现的下载链接做出响应。

use std::time::{Duration, Instant};

pub struct ClipboardWatcher {
    cb: Option<arboard::Clipboard>,
    last: Option<String>,
    last_poll: Instant,
}

impl ClipboardWatcher {
    pub fn new() -> Self {
        let mut cb = match arboard::Clipboard::new() {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!("剪贴板不可用，监听已禁用: {e}");
                None
            }
        };
        // 启动时已有的内容视为"旧"，不提示
        let last = cb.as_mut().and_then(|c| c.get_text().ok());
        Self { cb, last, last_poll: Instant::now() }
    }

    /// 应用自己写入剪贴板的内容，下次轮询时不当作新链接
    pub fn write(&mut self, text: &str) -> Result<(), String> {
        if self.cb.is_none() {
            self.cb = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        self.cb.as_mut().unwrap().set_text(text).map_err(|e| e.to_string())?;
        self.last = Some(text.to_string());
        Ok(())
    }

    /// 返回剪贴板里新出现的链接（空表示没有变化或没有链接）
    pub fn poll(&mut self) -> Vec<String> {
        if self.last_poll.elapsed() < Duration::from_secs(1) {
            return Vec::new();
        }
        self.last_poll = Instant::now();
        let Some(cb) = self.cb.as_mut() else { return Vec::new() };
        // 非文本内容（图片等）会返回 Err，直接忽略
        let Ok(text) = cb.get_text() else { return Vec::new() };
        if self.last.as_deref() == Some(text.as_str()) {
            return Vec::new();
        }
        self.last = Some(text.clone());
        extract_urls(&text)
    }
}

/// 从任意文本里挑出 HTTP(S)/FTP(S) 链接：允许和中文/标点紧挨着，遇到空白、引号、括号或非 ASCII 字符即结束，
/// 再剥掉结尾的句读符号；按出现顺序去重。
pub fn extract_urls(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(rel) =
        ["http://", "https://", "ftp://", "ftps://"].iter().filter_map(|scheme| lower[from..].find(scheme)).min()
    {
        let start = from + rel;
        let rest = &text[start..];
        // 用字节比较，避免在多字节字符中间切片
        let b = rest.as_bytes();
        let scheme_len = if b.len() >= 7 && b[..7].eq_ignore_ascii_case(b"http://") {
            7
        } else if b.len() >= 8 && b[..8].eq_ignore_ascii_case(b"https://") {
            8
        } else if b.len() >= 6 && b[..6].eq_ignore_ascii_case(b"ftp://") {
            6
        } else if b.len() >= 7 && b[..7].eq_ignore_ascii_case(b"ftps://") {
            7
        } else {
            from = start + 4;
            continue;
        };
        let body = &rest[scheme_len..];
        let end = body
            .char_indices()
            .find(|(_, c)| {
                c.is_whitespace()
                    || !c.is_ascii()
                    || matches!(c, '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | '`')
            })
            .map(|(i, _)| i)
            .unwrap_or(body.len());
        let url = rest[..scheme_len + end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if url.len() > scheme_len + 3 && crate::instance::download_url(url).is_ok() && !out.iter().any(|u| u == url) {
            out.push(url.to_string());
        }
        from = start + scheme_len + end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_and_cleans_urls() {
        let text = "看这个：<https://a.com/x.zip>，还有 'http://b.org/y?z=1' 以及 https://a.com/x.zip 重复了。";
        assert_eq!(extract_urls(text), vec!["https://a.com/x.zip", "http://b.org/y?z=1"]);
        assert_eq!(extract_urls("下载 ftp://x.y/z 和 http:/坏的"), vec!["ftp://x.y/z"]);
        assert_eq!(extract_urls("FTPS://x.y/a ftp://user:secret@x.y/b"), vec!["FTPS://x.y/a"]);
    }

    #[test]
    fn handles_cjk_adjacent_and_trailing_punctuation() {
        assert_eq!(extract_urls("链接：https://a.com/x.zip，谢谢"), vec!["https://a.com/x.zip"]);
        assert_eq!(extract_urls("see https://a.com/x.zip."), vec!["https://a.com/x.zip"]);
        assert_eq!(extract_urls("HTTPS://A.com/P%E6%96%87.iso?x=1&y=2"), vec!["HTTPS://A.com/P%E6%96%87.iso?x=1&y=2"]);
        assert_eq!(extract_urls("a https://x.io/1\nhttps://x.io/2\n"), vec!["https://x.io/1", "https://x.io/2"]);
    }
}
