use std::collections::HashMap;

use lol_html::{RewriteStrSettings, element, rewrite_str, text};
use url::Url;

use crate::{CaptureOptions, css, srcset};

#[derive(Debug)]
pub(crate) struct Link {
    pub url: Url,
    pub page: bool,
    pub manifest: bool,
}

pub(crate) fn network_reference(value: &str, base: &Url) -> Option<Url> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let url = base.join(value).ok()?;
    matches!(url.scheme(), "http" | "https").then_some(url)
}

pub(crate) fn local_reference(value: &str, base: &Url, downloaded: &HashMap<String, String>) -> Option<String> {
    let mut url = network_reference(value, base)?;
    let fragment = url.fragment().map(str::to_owned);
    url.set_fragment(None);
    let mut value = downloaded.get(url.as_str()).cloned().unwrap_or_else(|| url.to_string());
    if let Some(fragment) = fragment {
        value.push('#');
        value.push_str(&fragment);
    }
    Some(value)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Always,
    Scripts,
    Media,
    Embedded,
    Manifests,
    Lazy,
}
impl Group {
    fn enabled(self, options: &CaptureOptions) -> bool {
        match self {
            Self::Always => true,
            Self::Scripts => options.scripts,
            Self::Media => options.media,
            Self::Embedded => options.embedded_documents,
            Self::Manifests => options.manifests,
            Self::Lazy => options.lazy_images,
        }
    }
}
const ATTRS: &[(&str, &str, bool, Group)] = &[
    ("a[href], area[href]", "href", true, Group::Always),
    (
        "link[rel~=stylesheet], link[rel~=icon], link[rel~=preload], link[rel~=modulepreload], link[rel~=apple-touch-icon], link[rel~=mask-icon]",
        "href",
        false,
        Group::Always,
    ),
    ("link[rel~=manifest]", "href", false, Group::Manifests),
    ("img[src], input[type=image][src]", "src", false, Group::Always),
    ("script[src]", "src", false, Group::Scripts),
    ("source[src], audio[src], video[src], track[src]", "src", false, Group::Media),
    ("embed[src]", "src", false, Group::Embedded),
    ("iframe[src], frame[src]", "src", true, Group::Embedded),
    ("video[poster]", "poster", false, Group::Media),
    ("object[data]", "data", false, Group::Embedded),
    ("body[background], table[background], td[background], th[background]", "background", false, Group::Always),
    ("img[data-src], source[data-src]", "data-src", false, Group::Lazy),
    ("img[data-original]", "data-original", false, Group::Lazy),
    ("image[href], use[href], feImage[href]", "href", false, Group::Always),
    ("image[xlink\\:href], use[xlink\\:href], feImage[xlink\\:href]", "xlink:href", false, Group::Always),
];

fn attr_reference(value: &str, base: &Url, svg: bool) -> Option<Url> {
    let value = html_escape::decode_html_entities(value);
    if svg && value.trim().starts_with('#') {
        return None;
    }
    network_reference(&value, base)
}
fn svg_element(tag: &str) -> bool {
    matches!(tag.to_ascii_lowercase().as_str(), "image" | "use" | "feimage")
}

fn base_url(html: &str, page: &Url) -> Result<Url, String> {
    let mut base = None;
    rewrite_str(
        html,
        RewriteStrSettings::new().append_element_content_handler(element!("base[href]", |el| {
            if base.is_none() {
                base = el
                    .get_attribute("href")
                    .and_then(|value| page.join(&html_escape::decode_html_entities(&value)).ok());
            }
            Ok(())
        })),
    )
    .map_err(|error| error.to_string())?;
    Ok(base.unwrap_or_else(|| page.clone()))
}

pub(crate) fn links(html: &str, page: &Url, options: &CaptureOptions) -> Result<Vec<Link>, String> {
    let base = base_url(html, page)?;
    let mut links = Vec::new();
    // 分别处理属性，使同一元素的 poster/src 都能被保留。
    for &(selector, attr, is_page, group) in ATTRS {
        if !group.enabled(options) {
            continue;
        }
        rewrite_str(
            html,
            RewriteStrSettings::new().append_element_content_handler(element!(selector, |el| {
                if el.tag_name().eq_ignore_ascii_case("link") {
                    let relation = el.get_attribute("rel").unwrap_or_default().to_ascii_lowercase();
                    let kind = el.get_attribute("as").unwrap_or_default().to_ascii_lowercase();
                    if (!options.scripts
                        && (relation.split_whitespace().any(|rel| rel == "modulepreload") || kind == "script"))
                        || (!options.media && matches!(kind.as_str(), "audio" | "video" | "track"))
                        || (!options.embedded_documents && kind == "document")
                    {
                        return Ok(());
                    }
                }
                if let Some(url) =
                    el.get_attribute(attr).and_then(|value| attr_reference(&value, &base, svg_element(&el.tag_name())))
                {
                    links.push(Link { url, page: is_page, manifest: group == Group::Manifests });
                }
                Ok(())
            })),
        )
        .map_err(|error| error.to_string())?;
    }
    for attr in ["srcset", "data-srcset"] {
        if attr == "data-srcset" && !options.lazy_images {
            continue;
        }
        rewrite_str(
            html,
            RewriteStrSettings::new().append_element_content_handler(element!("img, source", |el| {
                if let Some(value) = el.get_attribute(attr) {
                    let value = html_escape::decode_html_entities(&value);
                    for range in srcset::urls(&value) {
                        if let Some(url) = network_reference(&value[range], &base) {
                            links.push(Link { url, page: false, manifest: false });
                        }
                    }
                }
                Ok(())
            })),
        )
        .map_err(|error| error.to_string())?;
    }
    rewrite_str(
        html,
        RewriteStrSettings::new().append_element_content_handler(element!("[style]", |el| {
            if let Some(value) = el.get_attribute("style") {
                links.extend(css::links(&html_escape::decode_html_entities(&value), &base));
            }
            Ok(())
        })),
    )
    .map_err(|error| error.to_string())?;
    let mut style = String::new();
    rewrite_str(
        html,
        RewriteStrSettings::new().append_element_content_handler(text!("style", |chunk| {
            style.push_str(chunk.as_str());
            if chunk.last_in_text_node() {
                links.extend(css::links(&style, &base));
                style.clear();
            }
            Ok(())
        })),
    )
    .map_err(|error| error.to_string())?;
    Ok(links)
}

pub(crate) fn rewrite(
    html: &str,
    page: &Url,
    downloaded: &HashMap<String, String>,
    options: &CaptureOptions,
) -> Result<String, String> {
    let base = base_url(html, page)?;
    let mut output = rewrite_str(
        html,
        RewriteStrSettings::new().append_element_content_handler(element!("base", |el| {
            el.remove();
            Ok(())
        })),
    )
    .map_err(|error| error.to_string())?;
    if options.strip_scripts_from_mirror {
        output = rewrite_str(
            &output,
            RewriteStrSettings::new().append_element_content_handler(element!("script", |el| {
                el.remove();
                Ok(())
            })),
        )
        .map_err(|error| error.to_string())?;
    }
    for &(selector, attr, _, _) in ATTRS {
        output = rewrite_str(
            &output,
            RewriteStrSettings::new().append_element_content_handler(element!(selector, |el| {
                if let Some(value) = el.get_attribute(attr).and_then(|value| {
                    if svg_element(&el.tag_name()) && value.trim().starts_with('#') {
                        return None;
                    }
                    local_reference(&html_escape::decode_html_entities(&value), &base, downloaded)
                }) {
                    el.set_attribute(attr, &value)?;
                }
                Ok(())
            })),
        )
        .map_err(|error| error.to_string())?;
    }
    for attr in ["srcset", "data-srcset"] {
        output = rewrite_str(
            &output,
            RewriteStrSettings::new().append_element_content_handler(element!("img, source", |el| {
                if let Some(value) = el.get_attribute(attr) {
                    let value = html_escape::decode_html_entities(&value);
                    let mut rewritten = String::new();
                    let mut cursor = 0;
                    for range in srcset::urls(&value) {
                        if let Some(url) = local_reference(&value[range.clone()], &base, downloaded) {
                            rewritten.push_str(&value[cursor..range.start]);
                            rewritten.push_str(&url);
                            cursor = range.end;
                        }
                    }
                    rewritten.push_str(&value[cursor..]);
                    el.set_attribute(attr, &rewritten)?;
                }
                Ok(())
            })),
        )
        .map_err(|error| error.to_string())?;
    }
    output = rewrite_str(
        &output,
        RewriteStrSettings::new().append_element_content_handler(element!("[style]", |el| {
            if let Some(value) = el.get_attribute("style") {
                el.set_attribute(
                    "style",
                    &css::rewrite(&html_escape::decode_html_entities(&value), &base, downloaded),
                )?;
            }
            Ok(())
        })),
    )
    .map_err(|error| error.to_string())?;
    let mut style = String::new();
    output = rewrite_str(
        &output,
        RewriteStrSettings::new().append_element_content_handler(text!("style", |chunk| {
            style.push_str(chunk.as_str());
            if chunk.last_in_text_node() {
                chunk.set_str(css::rewrite(&style, &base, downloaded));
                style.clear();
            } else {
                chunk.set_str(String::new());
            }
            Ok(())
        })),
    )
    .map_err(|error| error.to_string())?;
    Ok(output)
}
