//! Web app manifest URLs are selected by their schema, never arbitrary JSON strings.
use crate::html::{Link, local_reference, network_reference};
use serde_json::Value;
use std::collections::HashMap;
use url::Url;

fn visit(value: &mut Value, visitor: &mut impl FnMut(&mut String, bool)) {
    for key in ["icons", "screenshots"] {
        if let Some(images) = value.get_mut(key).and_then(Value::as_array_mut) {
            for image in images {
                if let Some(Value::String(src)) = image.get_mut("src") {
                    visitor(src, false);
                }
            }
        }
    }
    if let Some(Value::String(url)) = value.get_mut("start_url") {
        visitor(url, true);
    }
    if let Some(shortcuts) = value.get_mut("shortcuts").and_then(Value::as_array_mut) {
        for shortcut in shortcuts {
            if let Some(Value::String(url)) = shortcut.get_mut("url") {
                visitor(url, true);
            }
            // Shortcut icons share the top-level icon schema. No arbitrary recursion.
            if let Some(icons) = shortcut.get_mut("icons").and_then(Value::as_array_mut) {
                for icon in icons {
                    if let Some(Value::String(src)) = icon.get_mut("src") {
                        visitor(src, false);
                    }
                }
            }
        }
    }
}
pub(crate) fn links(text: &str, base: &Url) -> Result<Vec<Link>, String> {
    let mut value: Value = serde_json::from_str(text).map_err(|error| format!("manifest JSON 无效：{error}"))?;
    if !value.is_object() {
        return Err("manifest 必须是 JSON 对象".into());
    }
    let mut links = vec![];
    visit(&mut value, &mut |raw, page| {
        if let Some(url) = network_reference(raw, base) {
            links.push(Link { url, page, manifest: false });
        }
    });
    Ok(links)
}
pub(crate) fn rewrite(text: &str, base: &Url, mapping: &HashMap<String, String>) -> Result<String, String> {
    let mut value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    visit(&mut value, &mut |raw, _| {
        if let Some(url) = local_reference(raw, base, mapping) {
            *raw = url;
        }
    });
    // Scope/id are identities, not resources to crawl. Keep their original online meaning.
    for key in ["scope", "id"] {
        if let Some(Value::String(raw)) = value.get_mut(key)
            && let Some(url) = network_reference(raw, base)
        {
            *raw = url.to_string();
        }
    }
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}
