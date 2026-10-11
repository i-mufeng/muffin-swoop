//! CSS Syntax tokenizer: comments/ordinary strings cannot accidentally become requests.
use std::collections::HashMap;
use std::ops::Range;

use cssparser::{ParseError, Parser, ParserInput, Token, serialize_string};
use url::Url;

use crate::html::{Link, local_reference, network_reference};

struct Reference {
    range: Range<usize>,
    value: String,
    function: bool,
}

fn references(css: &str) -> Vec<Reference> {
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    let mut refs = Vec::new();
    collect(&mut parser, &mut refs, 0, false);
    refs
}

fn collect(parser: &mut Parser<'_, '_>, refs: &mut Vec<Reference>, depth: usize, image_set: bool) {
    // Bound Rust recursion even on adversarial nested functions. The tokenizer skips
    // unvisited nested blocks without recursively calling this function.
    if depth >= 64 {
        return;
    }
    let mut import = false;
    loop {
        let start = parser.position().byte_index();
        let Ok(token) = parser.next_including_whitespace_and_comments().cloned() else {
            break;
        };
        let mut found = None;
        match token {
            Token::WhiteSpace(_) | Token::Comment(_) => continue,
            Token::AtKeyword(name) if name.eq_ignore_ascii_case("import") => {
                import = true;
                continue;
            }
            Token::QuotedString(value) if import || image_set => found = Some((value.to_string(), false)),
            Token::UnquotedUrl(value) => found = Some((value.to_string(), true)),
            Token::Function(name) if name.eq_ignore_ascii_case("url") => {
                let result: Result<_, ParseError<'_, ()>> = parser.parse_nested_block(|nested| {
                    let value = nested.expect_string_cloned()?;
                    nested.expect_exhausted()?;
                    Ok(value.to_string())
                });
                if let Ok(value) = result {
                    found = Some((value, true));
                }
            }
            Token::Function(name) => {
                let image_set =
                    name.eq_ignore_ascii_case("image-set") || name.eq_ignore_ascii_case("-webkit-image-set");
                let _: Result<_, ParseError<'_, ()>> = parser.parse_nested_block(|nested| {
                    collect(nested, refs, depth + 1, image_set);
                    Ok(())
                });
            }
            Token::ParenthesisBlock | Token::CurlyBracketBlock | Token::SquareBracketBlock => {
                let _: Result<_, ParseError<'_, ()>> = parser.parse_nested_block(|nested| {
                    collect(nested, refs, depth + 1, false);
                    Ok(())
                });
            }
            _ => {}
        }
        if let Some((value, function)) = found {
            refs.push(Reference { range: start..parser.position().byte_index(), value, function });
        }
        import = false;
    }
}

pub(crate) fn links(css: &str, base: &Url) -> Vec<Link> {
    references(css)
        .into_iter()
        .filter(|reference| !reference.value.trim().starts_with('#'))
        .filter_map(|reference| network_reference(&reference.value, base))
        .map(|url| Link { url, page: false, manifest: false })
        .collect()
}

pub(crate) fn rewrite(css: &str, base: &Url, downloaded: &HashMap<String, String>) -> String {
    let mut output = String::with_capacity(css.len());
    let mut cursor = 0;
    for reference in references(css) {
        if reference.value.trim().starts_with('#') {
            continue;
        }
        let Some(value) = local_reference(&reference.value, base, downloaded) else {
            continue;
        };
        output.push_str(&css[cursor..reference.range.start]);
        if reference.function {
            output.push_str("url(");
        }
        // CSS escaping, rather than HTML/string escaping, also handles quotes and
        // parentheses in URL values. Encoding '<' protects embedded <style> text.
        let mut quoted = String::new();
        serialize_string(&value, &mut quoted).expect("writing to String cannot fail");
        output.push_str(&quoted.replace('<', "\\3c "));
        if reference.function {
            output.push(')');
        }
        cursor = reference.range.end;
    }
    output.push_str(&css[cursor..]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_escapes_comments_imports_and_nested_functions() {
        let base = Url::parse("https://example.org/css/main.css").unwrap();
        let source = r##"/* url(fake.png) */ @IMPORT /*gap*/ "other.css" screen;
            x { content: 'url(no.png)'; background: image-set(URL("a)b.png?q=1#frag") 1x, url(b\ c.png) 2x);
            mask: url(#local); a:url(data:image/svg+xml,a,b); b:url(blob:opaque); broken:url("x" bad); }"##;
        let links = links(source, &base);
        assert_eq!(
            links.iter().map(|l| l.url.as_str()).collect::<Vec<_>>(),
            [
                "https://example.org/css/other.css",
                "https://example.org/css/a)b.png?q=1#frag",
                "https://example.org/css/b%20c.png",
            ]
        );
        let mapping = HashMap::from([("https://example.org/css/a)b.png?q=1".into(), "image.png".into())]);
        let out = rewrite(source, &base, &mapping);
        assert!(out.contains("url(\"image.png#frag\")"));
        assert!(out.contains("\"https://example.org/css/other.css\" screen"));
        assert!(out.contains("url(data:image/svg+xml,a,b)"));
        assert!(out.contains("url(blob:opaque)"));
        assert!(out.contains("url(#local)"));
        assert!(out.contains("'url(no.png)'"));
    }

    #[test]
    fn excessively_nested_css_is_bounded() {
        let css = format!("{}url(a.png){}", "f(".repeat(1000), ")".repeat(1000));
        assert!(links(&css, &Url::parse("https://example.org/").unwrap()).is_empty());
    }
}
