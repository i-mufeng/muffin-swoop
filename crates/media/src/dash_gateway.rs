//! Bounded DASH XML rewriting. Segment addressing is resolved separately for
//! every Representation, so no untrusted URL suffix can select another origin.
use crate::{
    MediaError,
    gateway::{Catalog, Kind},
    network_url,
};
use quick_xml::{
    Reader, Writer,
    events::{BytesEnd, BytesStart, BytesText, Event},
};
use url::Url;

#[derive(Clone)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
    text: String,
}
impl Node {
    fn local(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }
}
fn parse(text: &str) -> Result<Node, MediaError> {
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Node> = Vec::new();
    let mut root = None;
    let mut count = 0;
    loop {
        let event = reader.read_event().map_err(|_| MediaError::InvalidManifest)?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                count += 1;
                if stack.len() > 32 || count > 100_000 {
                    return Err(MediaError::ManifestLimit);
                }
                let name =
                    std::str::from_utf8(element.name().as_ref()).map_err(|_| MediaError::InvalidManifest)?.to_owned();
                let mut attrs = Vec::new();
                for attr in element.attributes() {
                    let attr = attr.map_err(|_| MediaError::InvalidManifest)?;
                    let key =
                        std::str::from_utf8(attr.key.as_ref()).map_err(|_| MediaError::InvalidManifest)?.to_owned();
                    let value = attr
                        .decoded_and_normalized_value(quick_xml::XmlVersion::default(), reader.decoder())
                        .map_err(|_| MediaError::InvalidManifest)?
                        .into_owned();
                    if matches!(key.as_str(), "xml:base" | "xlink:href") || key.rsplit(':').next() == Some("href") {
                        return Err(MediaError::UnsupportedGatewayMedia);
                    }
                    attrs.push((key, value));
                }
                let node = Node { name, attrs, children: Vec::new(), text: String::new() };
                if matches!(node.local(), "Location" | "PatchLocation" | "UTCTiming" | "ContentSteering") {
                    return Err(MediaError::UnsupportedGatewayMedia);
                }
                if node.local() == "ContentProtection" {
                    return Err(MediaError::ProtectedMedia);
                }
                stack.push(node);
                if empty {
                    complete(&mut stack, &mut root)?;
                }
            }
            Event::End(_) => complete(&mut stack, &mut root)?,
            Event::Text(value) => {
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(
                        &value
                            .xml_content(quick_xml::XmlVersion::default())
                            .map_err(|_| MediaError::InvalidManifest)?,
                    );
                }
            }
            Event::CData(value) => {
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(&value.decode().map_err(|_| MediaError::InvalidManifest)?);
                }
            }
            Event::GeneralRef(reference) => {
                let value = if let Some(c) = reference.resolve_char_ref().map_err(|_| MediaError::InvalidManifest)? {
                    c.to_string()
                } else {
                    match reference.decode().map_err(|_| MediaError::InvalidManifest)?.as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "apos" => "'",
                        "quot" => "\"",
                        _ => return Err(MediaError::InvalidManifest),
                    }
                    .into()
                };
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(&value);
                }
            }
            Event::DocType(_) => return Err(MediaError::InvalidManifest),
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(MediaError::InvalidManifest);
    }
    let root = root.ok_or(MediaError::InvalidManifest)?;
    if root.local() != "MPD" {
        return Err(MediaError::InvalidManifest);
    }
    Ok(root)
}
fn complete(stack: &mut Vec<Node>, root: &mut Option<Node>) -> Result<(), MediaError> {
    let node = stack.pop().ok_or(MediaError::InvalidManifest)?;
    if let Some(parent) = stack.last_mut() {
        parent.children.push(node);
    } else if root.replace(node).is_some() {
        return Err(MediaError::InvalidManifest);
    }
    Ok(())
}

#[derive(Clone)]
pub(crate) struct Template {
    parts: Vec<String>,
}
impl Template {
    /// Only numeric substitutions remain after Representation expansion. Each
    /// substitution has its own path component; literals never become routing.
    pub fn from_url(url: &Url) -> Result<Option<(Self, String)>, MediaError> {
        let raw = url.as_str();
        if !raw.contains('$') {
            return Ok(None);
        }
        let mut parts = Vec::new();
        let mut route = String::new();
        let mut remaining = raw;
        while let Some(index) = remaining.find('$') {
            parts.push(remaining[..index].to_owned());
            remaining = &remaining[index + 1..];
            let end = remaining.find('$').ok_or(MediaError::UnsupportedGatewayMedia)?;
            let field = &remaining[..end];
            let name = field.split('%').next().unwrap_or("");
            if !matches!(name, "Number" | "Time" | "Bandwidth") {
                return Err(MediaError::UnsupportedGatewayMedia);
            }
            if let Some((_, format)) = field.split_once('%') {
                let width = format
                    .strip_prefix('0')
                    .and_then(|s| s.strip_suffix('d'))
                    .and_then(|s| s.parse::<u8>().ok())
                    .filter(|w| *w > 0 && *w <= 20)
                    .ok_or(MediaError::UnsupportedGatewayMedia)?;
                let _ = width;
            }
            route.push('/');
            route.push('$');
            route.push_str(field);
            route.push('$');
            remaining = &remaining[end + 1..];
        }
        parts.push(remaining.to_owned());
        Ok(Some((Self { parts }, route)))
    }
    pub fn resolve(&self, suffix: &str) -> Result<Url, MediaError> {
        let values = suffix.split('/').collect::<Vec<_>>();
        if values.len() + 1 != self.parts.len() {
            return Err(MediaError::InvalidUrl);
        }
        let mut url = self.parts[0].clone();
        for (index, value) in values.iter().enumerate() {
            if value.is_empty()
                || value.len() > 20
                || !value.bytes().all(|b| b.is_ascii_digit())
                || value.parse::<u64>().is_err()
            {
                return Err(MediaError::InvalidUrl);
            }
            url.push_str(value);
            url.push_str(&self.parts[index + 1]);
        }
        network_url(&url)
    }
}
fn addressing(name: &str) -> bool {
    matches!(name, "SegmentTemplate" | "SegmentList" | "SegmentBase")
}
fn rewrite_address(
    node: &mut Node,
    base: &Url,
    id: &str,
    bandwidth: Option<&str>,
    catalog: &mut Catalog,
) -> Result<(), MediaError> {
    for (key, value) in &mut node.attrs {
        if matches!(key.as_str(), "media" | "initialization" | "sourceURL" | "index") {
            let raw = value.replace("$RepresentationID$", id);
            let raw = if let Some(bw) = bandwidth { raw.replace("$Bandwidth$", bw) } else { raw };
            if raw.contains(['\r', '\n', '\0']) {
                return Err(MediaError::InvalidUrl);
            }
            let url = base.join(&raw).map_err(|_| MediaError::InvalidUrl)?;
            *value = catalog.register_dash(url)?;
        }
    }
    for child in &mut node.children {
        rewrite_address(child, base, id, bandwidth, catalog)?;
    }
    Ok(())
}

// FFmpeg 9.0.1 rounds SegmentTimeline positions to whole seconds when
// refreshing. Fractional audio segment boundaries then replay the previous
// fragment. Feed the same, unmodified media through an explicit ordered list.
// Keep the observed window for this recording so FFmpeg never renumbers an
// already consumed fragment when the origin removes it from the live window.
#[derive(Default)]
pub(crate) struct LiveTimeline {
    entries: Vec<(u64, u64, String)>, // source number, source time, opaque URL
}
fn number(raw: Option<&str>, default: u64) -> Result<u64, MediaError> {
    raw.map_or(Ok(default), |value| value.parse().map_err(|_| MediaError::InvalidManifest))
}
fn expand(template: &str, number: u64, time: u64) -> Result<String, MediaError> {
    let mut remaining = template;
    let mut result = String::new();
    while let Some(index) = remaining.find('$') {
        result.push_str(&remaining[..index]);
        remaining = &remaining[index + 1..];
        let end = remaining.find('$').ok_or(MediaError::UnsupportedGatewayMedia)?;
        let field = &remaining[..end];
        let (name, width) = if let Some((name, format)) = field.split_once('%') {
            let width = format
                .strip_prefix('0')
                .and_then(|s| s.strip_suffix('d'))
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|w| *w > 0 && *w <= 20)
                .ok_or(MediaError::UnsupportedGatewayMedia)?;
            (name, width)
        } else {
            (field, 0)
        };
        let value = match name {
            "Number" => number,
            "Time" => time,
            _ => return Err(MediaError::UnsupportedGatewayMedia),
        };
        result.push_str(&format!("{value:0width$}"));
        remaining = &remaining[end + 1..];
    }
    result.push_str(remaining);
    Ok(result)
}
fn flatten_live_timeline(node: &mut Node, identity: String, catalog: &mut Catalog) -> Result<(), MediaError> {
    if node.local() != "SegmentTemplate" {
        return Ok(());
    }
    let Some(timeline) = node.children.iter().find(|child| child.local() == "SegmentTimeline") else {
        return Ok(());
    };
    let template = node.attr("media").ok_or(MediaError::InvalidManifest)?;
    let first_number = number(node.attr("startNumber"), 1)?;
    let mut sequence = first_number;
    let mut time = 0;
    let mut entries = Vec::new();
    for (index, segment) in timeline.children.iter().enumerate() {
        if segment.local() != "S" {
            return Err(MediaError::UnsupportedGatewayMedia);
        }
        time = number(segment.attr("t"), time)?;
        let duration = number(segment.attr("d"), 0)?;
        if duration == 0 {
            return Err(MediaError::InvalidManifest);
        }
        let repeats = if segment.attr("r") == Some("-1") {
            let end = timeline
                .children
                .get(index + 1)
                .and_then(|next| next.attr("t"))
                .ok_or(MediaError::UnsupportedGatewayMedia)?;
            let span = number(Some(end), 0)?
                .checked_sub(time)
                .filter(|span| *span > 0 && *span % duration == 0)
                .ok_or(MediaError::InvalidManifest)?;
            span / duration - 1
        } else {
            number(segment.attr("r"), 0)?
        };
        let count = repeats.checked_add(1).ok_or(MediaError::ManifestLimit)?;
        if count > catalog.max_timeline_segments as u64
            || entries.len().saturating_add(count as usize) > catalog.max_timeline_segments
        {
            return Err(MediaError::ManifestLimit);
        }
        for _ in 0..count {
            entries.push((sequence, time, expand(template, sequence, time)?));
            time = time.checked_add(duration).ok_or(MediaError::ManifestLimit)?;
            sequence = sequence.checked_add(1).ok_or(MediaError::ManifestLimit)?;
        }
    }
    if entries.is_empty() {
        return Err(MediaError::InvalidManifest);
    }
    let history = catalog.dash_timelines.entry(identity).or_default();
    for entry in entries {
        if let Some(existing) = history.entries.iter_mut().find(|existing| existing.0 == entry.0) {
            if existing.1 != entry.1 {
                return Err(MediaError::InvalidManifest);
            }
            // Signed resource URLs may rotate while sequence/time stay stable.
            existing.2 = entry.2;
        } else if history.entries.last().is_none_or(|last| entry.0 == last.0 + 1) {
            if catalog.timeline_segments >= catalog.max_timeline_segments {
                return Err(MediaError::ManifestLimit);
            }
            catalog.timeline_segments += 1;
            history.entries.push(entry);
        } else {
            // A moving window must not silently jump over missed segments.
            return Err(MediaError::InvalidManifest);
        }
    }
    let initialization = node.attr("initialization").map(str::to_owned);
    node.name = "SegmentList".into();
    node.attrs.retain(|(key, _)| matches!(key.as_str(), "timescale" | "presentationTimeOffset"));
    // The retained window starts at local index zero; media URLs retain their
    // original Number/Time. No timestamps in the actual fragments are changed.
    node.attrs.push(("startNumber".into(), "0".into()));
    node.children.retain(|child| child.local() != "SegmentTimeline");
    if let Some(url) = initialization {
        if url.contains('$') {
            return Err(MediaError::UnsupportedGatewayMedia);
        }
        node.children.push(Node {
            name: "Initialization".into(),
            attrs: vec![("sourceURL".into(), url)],
            children: vec![],
            text: String::new(),
        });
    }
    for (_, _, url) in &history.entries {
        node.children.push(Node {
            name: "SegmentURL".into(),
            attrs: vec![("media".into(), url.clone())],
            children: vec![],
            text: String::new(),
        });
    }
    Ok(())
}

fn rewrite_node(
    node: &mut Node,
    parent: &Url,
    inherited: Option<Node>,
    live: bool,
    catalog: &mut Catalog,
) -> Result<(), MediaError> {
    let bases = node.children.iter().filter(|n| n.local() == "BaseURL").collect::<Vec<_>>();
    if bases.len() > 1 {
        return Err(MediaError::UnsupportedGatewayMedia);
    }
    let base = if let Some(base) = bases.first() {
        if base.text.contains(['\r', '\n', '\0']) {
            return Err(MediaError::InvalidUrl);
        }
        network_url(parent.join(base.text.trim()).map_err(|_| MediaError::InvalidUrl)?.as_str())?
    } else {
        parent.clone()
    };
    let explicit = node.children.iter().find(|n| addressing(n.local())).cloned();
    // Inheritance merges attributes and SegmentTimeline according to DASH.
    let address = match (explicit, inherited) {
        (Some(mut child), Some(parent)) if child.local() == parent.local() => {
            for (key, value) in parent.attrs {
                if !child.attrs.iter().any(|(k, _)| *k == key) {
                    child.attrs.push((key, value));
                }
            }
            for value in parent.children {
                if !child.children.iter().any(|c| c.local() == value.local()) {
                    child.children.push(value);
                }
            }
            Some(child)
        }
        (Some(child), _) => Some(child),
        (None, parent) => parent,
    };
    node.children.retain(|n| n.local() != "BaseURL" && !addressing(n.local()));
    if node.local() == "Representation" {
        let id = node.attr("id").unwrap_or("").to_owned();
        let bandwidth = node.attr("bandwidth").map(str::to_owned);
        if !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) || id.len() > 128 {
            return Err(MediaError::UnsupportedGatewayMedia);
        }
        if bandwidth.as_ref().is_some_and(|value| {
            value.is_empty()
                || value.len() > 20
                || !value.bytes().all(|b| b.is_ascii_digit())
                || value.parse::<u64>().is_err()
        }) {
            return Err(MediaError::UnsupportedGatewayMedia);
        }
        if let Some(mut address) = address {
            let identity = format!("{base}#{id}");
            rewrite_address(&mut address, &base, &id, bandwidth.as_deref(), catalog)?;
            if live {
                flatten_live_timeline(&mut address, identity, catalog)?;
            }
            if address.local() == "SegmentBase" {
                node.children.push(Node {
                    name: "BaseURL".into(),
                    attrs: vec![],
                    children: vec![],
                    text: catalog.register(base, Kind::Media, 0)?,
                });
            }
            node.children.push(address);
        } else {
            node.children.push(Node {
                name: "BaseURL".into(),
                attrs: vec![],
                children: vec![],
                text: catalog.register(base, Kind::Media, 0)?,
            });
        }
        return Ok(());
    }
    for child in &mut node.children {
        rewrite_node(child, &base, address.clone(), live, catalog)?;
    }
    Ok(())
}
fn write(node: &Node, writer: &mut Writer<Vec<u8>>) -> Result<(), MediaError> {
    let mut start = BytesStart::new(&node.name);
    for (key, value) in &node.attrs {
        start.push_attribute((key.as_str(), value.as_str()));
    }
    writer.write_event(Event::Start(start))?;
    if !node.text.trim().is_empty() {
        writer.write_event(Event::Text(BytesText::new(&node.text)))?;
    }
    for child in &node.children {
        write(child, writer)?;
    }
    writer.write_event(Event::End(BytesEnd::new(&node.name)))?;
    Ok(())
}
pub(crate) fn rewrite(text: &str, base: &Url, catalog: &mut Catalog) -> Result<(Vec<u8>, bool), MediaError> {
    let mut root = parse(text)?;
    let live = root.attr("type") == Some("dynamic");
    rewrite_node(&mut root, base, None, live, catalog)?;
    let mut writer = Writer::new(Vec::new());
    write(&root, &mut writer)?;
    Ok((writer.into_inner(), live))
}
