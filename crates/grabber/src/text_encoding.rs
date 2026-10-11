use encoding_rs::{Encoding, UTF_8};
use lol_html::{RewriteStrSettings, element, rewrite_str};

pub(crate) fn header_charset(content_type: &str) -> Option<String> {
    content_type
        .parse::<mime::Mime>()
        .ok()?
        .get_param(mime::CHARSET)
        .map(|value| value.as_str().to_owned())
}

/// Explicit BOM/HTTP/meta/@charset declarations only; no locale guessing.
pub(crate) fn decode(
    bytes: &[u8],
    explicit: Option<&str>,
    html: bool,
) -> Result<(String, String), String> {
    let encoding = if let Some((encoding, _)) = Encoding::for_bom(bytes) {
        encoding
    } else {
        let declared = explicit
            .map(str::to_owned)
            .or_else(|| declaration(bytes, html));
        match declared {
            Some(label) => Encoding::for_label_no_replacement(label.as_bytes())
                .ok_or_else(|| format!("不支持声明的字符编码：{label}"))?,
            None => UTF_8,
        }
    };
    let (text, actual, errors) = encoding.decode(bytes);
    if errors {
        return Err(format!("内容不符合 {} 编码，未生成损坏镜像", actual.name()));
    }
    Ok((text.into_owned(), actual.name().to_owned()))
}

fn declaration(bytes: &[u8], html: bool) -> Option<String> {
    let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]);
    if !html {
        // CSS encoding declarations are only valid at the very beginning.
        return prefix
            .strip_prefix("@charset \"")?
            .split_once("\";")
            .map(|(label, _)| label.to_owned());
    }
    let mut charset = None;
    let _ = rewrite_str(
        &prefix,
        RewriteStrSettings::new().append_element_content_handler(element!("meta", |el| {
            if charset.is_none() {
                charset = el
                    .get_attribute("charset")
                    .map(|value| html_escape::decode_html_entities(&value).into_owned());
                if charset.is_none()
                    && el
                        .get_attribute("http-equiv")
                        .is_some_and(|value| value.eq_ignore_ascii_case("content-type"))
                {
                    charset = el.get_attribute("content").and_then(|value| {
                        header_charset(&html_escape::decode_html_entities(&value))
                    });
                }
            }
            Ok(())
        })),
    );
    charset
}

/// A BOM takes precedence over legacy HTML meta/CSS @charset declarations and
/// ensures the rewritten UTF-8 bytes are decoded consistently from file://.
pub(crate) fn utf8_mirror(text: String) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 3);
    bytes.extend_from_slice(b"\xef\xbb\xbf");
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations_bom_precedence_and_invalid_data() {
        assert_eq!(
            header_charset("text/html; charset=\"windows-1252\""),
            Some("windows-1252".into())
        );
        assert!(
            decode(b"<meta charset=windows-1252><p>caf\xe9", None, true)
                .unwrap()
                .0
                .ends_with("café")
        );
        assert!(
            decode(b"@charset \"windows-1252\"; /*caf\xe9*/", None, false)
                .unwrap()
                .0
                .ends_with("café*/")
        );
        assert_eq!(
            decode(b"\xef\xbb\xbfhello", Some("unknown"), true)
                .unwrap()
                .1,
            "UTF-8"
        );
        assert!(decode(b"bad\xff", None, true).is_err());
        assert!(decode(b"plain", Some("unknown"), true).is_err());
    }
}
