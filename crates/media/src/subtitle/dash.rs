use super::{Cue, Fetcher, bmff, ttml, xml::{self, Node}};
use crate::{MediaError, network_url};
use std::time::Duration;
use url::Url;

#[derive(Clone)]
struct Resource { url: Url, range: Option<(u64, u64)> }
fn resolve(base: &Url, value: &str) -> Result<Url, MediaError> {
    if value.contains(['\0', '\n', '\r']) { return Err(MediaError::InvalidSubtitle); }
    network_url(base.join(value).map_err(|_| MediaError::InvalidUrl)?.as_str())
}
fn range(value: Option<&str>) -> Result<Option<(u64, u64)>, MediaError> {
    value.map(|raw| {
        let (start, end) = raw.split_once('-').ok_or(MediaError::InvalidSubtitle)?;
        let start = start.parse().map_err(|_| MediaError::InvalidSubtitle)?;
        let end = end.parse().map_err(|_| MediaError::InvalidSubtitle)?;
        if end < start { return Err(MediaError::InvalidSubtitle); }
        Ok((start, end))
    }).transpose()
}
fn uint(raw: Option<&str>, default: u64) -> Result<u64, MediaError> { raw.map_or(Ok(default), |raw| raw.parse().map_err(|_| MediaError::InvalidSubtitle)) }
fn expand(raw: &str, id: &str, bandwidth: Option<&str>, number: u64, time: u64) -> Result<String, MediaError> {
    let mut output = String::new();
    let mut rest = raw;
    while let Some(index) = rest.find('$') {
        output.push_str(&rest[..index]); rest = &rest[index + 1..];
        let end = rest.find('$').ok_or(MediaError::UnsupportedSubtitle)?;
        let token = &rest[..end]; rest = &rest[end + 1..];
        let (name, format) = token.split_once('%').unwrap_or((token, ""));
        let value = match name { "" => "$".into(), "RepresentationID" => id.into(), "Bandwidth" => bandwidth.ok_or(MediaError::InvalidSubtitle)?.into(), "Number" => number.to_string(), "Time" => time.to_string(), _ => return Err(MediaError::UnsupportedSubtitle) };
        if format.is_empty() { output.push_str(&value); }
        else {
            let width = format.strip_prefix('0').and_then(|s| s.strip_suffix('d')).and_then(|s| s.parse::<usize>().ok()).filter(|width| (1..=20).contains(width)).ok_or(MediaError::UnsupportedSubtitle)?;
            if !value.bytes().all(|b| b.is_ascii_digit()) { return Err(MediaError::UnsupportedSubtitle); }
            output.push_str(&format!("{value:0>width$}"));
        }
    }
    output.push_str(rest);
    Ok(output)
}
fn segment_times(address: &Node, extent: Option<f64>, scale: u64, max: usize) -> Result<Vec<(u64, u64)>, MediaError> {
    let mut output = vec![];
    if let Some(timeline) = address.child("SegmentTimeline") {
        let entries = timeline.nodes().collect::<Vec<_>>();
        let mut current = 0u64;
        for (index, entry) in entries.iter().enumerate() {
            if entry.local() != "S" { return Err(MediaError::UnsupportedSubtitle); }
            current = uint(entry.attr("t"), current)?;
            let duration = uint(entry.attr("d"), 0)?;
            if duration == 0 { return Err(MediaError::InvalidSubtitle); }
            let repeats: i64 = entry.attr("r").unwrap_or("0").parse().map_err(|_| MediaError::InvalidSubtitle)?;
            let count = if repeats == -1 {
                let end = entries.get(index + 1).and_then(|next| next.attr("t")).map(|v| uint(Some(v), 0)).transpose()?
                    .or_else(|| extent.map(|seconds| (seconds * scale as f64).ceil() as u64)).ok_or(MediaError::UnsupportedSubtitle)?;
                end.checked_sub(current).ok_or(MediaError::InvalidSubtitle)?.div_ceil(duration)
            } else if repeats >= 0 { repeats as u64 + 1 } else { return Err(MediaError::UnsupportedSubtitle); };
            if count > max.saturating_sub(output.len()) as u64 { return Err(MediaError::SubtitleLimit); }
            for _ in 0..count { output.push((current, duration)); current = current.checked_add(duration).ok_or(MediaError::SubtitleLimit)?; }
        }
    } else {
        let duration = uint(address.attr("duration"), 0)?;
        let extent = extent.ok_or(MediaError::UnsupportedSubtitle)?;
        if duration == 0 { return Err(MediaError::InvalidSubtitle); }
        let count = (extent * scale as f64 / duration as f64).ceil() as usize;
        if count > max { return Err(MediaError::SubtitleLimit); }
        for index in 0..count { output.push(((index as u64).checked_mul(duration).ok_or(MediaError::SubtitleLimit)?, duration)); }
    }
    Ok(output)
}
pub(super) async fn collect(fetcher: &mut Fetcher<'_>, source: &str, id: &str, duration: Option<Duration>, live: bool) -> Result<(Vec<Cue>, Option<String>), MediaError> {
    // Dynamic text needs the same selected A/V live window; never guess its
    // time origin and emit plausibly timed but shifted subtitles.
    if live { return Err(MediaError::LiveSubtitlesUnsupported); }
    let (bytes, mut base) = fetcher.get(network_url(source)?, None).await?;
    let root = xml::parse(std::str::from_utf8(&bytes).map_err(|_| MediaError::InvalidSubtitle)?)?;
    let periods = root.nodes().filter(|node| node.local() == "Period").collect::<Vec<_>>();
    if root.local() != "MPD" || periods.len() != 1 { return Err(MediaError::UnsupportedSubtitle); }
    let period = periods[0];
    let mut found = vec![];
    for adaptation in period.nodes().filter(|node| node.local() == "AdaptationSet") {
        for representation in adaptation.nodes().filter(|node| node.local() == "Representation" && node.attr("id") == Some(id)) { found.push((adaptation, representation)); }
    }
    if found.len() != 1 { return Err(MediaError::InvalidSelection); }
    let (adaptation, representation) = found[0];
    let path = [&root, period, adaptation, representation];
    let mut address = None;
    for node in path {
        if node.nodes().any(|child| child.local() == "ContentProtection") { return Err(MediaError::ProtectedMedia); }
        let bases = node.nodes().filter(|child| child.local() == "BaseURL").collect::<Vec<_>>();
        if bases.len() > 1 { return Err(MediaError::UnsupportedSubtitle); }
        if let Some(child) = bases.first() { base = resolve(&base, child.text().trim())?; }
        if let Some(child) = node.nodes().find(|child| matches!(child.local(), "SegmentTemplate" | "SegmentList" | "SegmentBase")) {
            let mut child = child.clone();
            if let Some(parent) = &address {
                let parent: &Node = parent;
                if parent.local() != child.local() { return Err(MediaError::UnsupportedSubtitle); }
                for (key, value) in &parent.attrs { if child.attr(key).is_none() { child.attrs.push((key.clone(), value.clone())); } }
                if child.children.is_empty() { child.children = parent.children.clone(); }
            }
            address = Some(child);
        }
    }
    let extent = duration.map(|d| d.as_secs_f64()).or_else(|| period.attr("duration").or_else(|| root.attr("mediaPresentationDuration")).and_then(crate::manifest::iso_duration).map(|d| d.as_secs_f64()));
    let period_start = period.attr("start").map(|raw| crate::manifest::iso_duration(raw).ok_or(MediaError::InvalidSubtitle)).transpose()?.map_or(0.0, |d| d.as_secs_f64());
    let mut initialization = None;
    let mut resources = vec![];
    let mut shift = period_start;
    if let Some(address) = &address {
        let scale = uint(address.attr("timescale"), 1)?;
        if scale == 0 { return Err(MediaError::InvalidSubtitle); }
        shift -= uint(address.attr("presentationTimeOffset"), 0)? as f64 / scale as f64;
        if let Some(init) = address.child("Initialization") {
            initialization = Some(Resource { url: resolve(&base, init.attr("sourceURL").unwrap_or(""))?, range: range(init.attr("range"))? });
        }
        match address.local() {
            "SegmentTemplate" => {
                if let Some(raw) = address.attr("initialization") { initialization = Some(Resource { url: resolve(&base, &expand(raw, id, representation.attr("bandwidth"), 0, 0)?)?, range: None }); }
                let times = segment_times(address, extent, scale, fetcher.config.max_segments)?;
                let start = uint(address.attr("startNumber"), 1)?;
                let media = address.attr("media").ok_or(MediaError::InvalidSubtitle)?;
                for (index, (time, _)) in times.into_iter().enumerate() {
                    resources.push(Resource { url: resolve(&base, &expand(media, id, representation.attr("bandwidth"), start.checked_add(index as u64).ok_or(MediaError::SubtitleLimit)?, time)?)?, range: None });
                }
            }
            "SegmentList" => {
                for segment in address.nodes().filter(|child| child.local() == "SegmentURL") { resources.push(Resource { url: resolve(&base, segment.attr("media").ok_or(MediaError::InvalidSubtitle)?)?, range: range(segment.attr("mediaRange"))? }); }
            }
            "SegmentBase" => resources.push(Resource { url: base.clone(), range: None }),
            _ => return Err(MediaError::UnsupportedSubtitle),
        }
    } else { resources.push(Resource { url: base, range: None }); }
    if resources.is_empty() || resources.len() > fetcher.config.max_segments { return Err(MediaError::SubtitleLimit); }
    let mut track = if let Some(resource) = initialization {
        let (bytes, _) = fetcher.get(resource.url, resource.range).await?;
        Some(bmff::init(&bytes)?)
    } else { None };
    let mut cues = vec![];
    let mut language = None;
    for resource in resources {
        let (bytes, _) = fetcher.get(resource.url, resource.range).await?;
        let mut converted = if std::str::from_utf8(&bytes).is_ok_and(|text| text.trim_start_matches('\u{feff}').trim_start().starts_with('<')) {
            let (cues, lang) = ttml::parse(&bytes, fetcher.config, extent)?;
            language = language.or(lang);
            cues
        } else {
            if track.is_none() { track = Some(bmff::init(&bytes)?); }
            let track = track.as_ref().unwrap();
            language = language.or_else(|| track.language.clone());
            let mut cues = vec![];
            for sample in bmff::samples(&bytes, track, fetcher.config.max_cues)? {
                if track.codec == *b"wvtt" { cues.extend(bmff::webvtt(&sample)?); }
                else {
                    let (mut parsed, lang) = ttml::parse(sample.bytes, fetcher.config, Some(sample.end))?;
                    language = language.or(lang);
                    for cue in &mut parsed { cue.start_ms = cue.start_ms.max((sample.start * 1000.0).round() as u64); cue.end_ms = cue.end_ms.min((sample.end * 1000.0).round() as u64); }
                    cues.extend(parsed.into_iter().filter(|cue| cue.end_ms > cue.start_ms));
                }
            }
            cues
        };
        for cue in &mut converted { cue.shift(shift)?; }
        cues.extend(converted);
        if cues.len() > fetcher.config.max_cues { return Err(MediaError::SubtitleLimit); }
    }
    Ok((cues, language))
}
