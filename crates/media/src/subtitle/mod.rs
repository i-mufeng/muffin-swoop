//! Explicit, bounded text conversion. Temporary files contain subtitle text,
//! never source addresses or credentials; remote XML cannot reference files.
mod bmff;
mod dash;
mod hls;
mod ttml;
mod xml;

use crate::{MediaError, MediaSessionAuth, network::NetworkTransport, network_url, selection::SubtitleLocator, SubtitleTrack};
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path, time::Duration};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleConfig {
    pub convert_text: bool,
    pub max_resource_bytes: usize,
    pub max_total_bytes: usize,
    pub max_cues: usize,
    pub max_segments: usize,
}
impl Default for SubtitleConfig {
    fn default() -> Self { Self { convert_text: true, max_resource_bytes: 8 * 1024 * 1024, max_total_bytes: 64 * 1024 * 1024, max_cues: 100_000, max_segments: 20_000 } }
}
impl SubtitleConfig {
    pub fn validate(&self) -> Result<(), MediaError> {
        if !(1024..=64 * 1024 * 1024).contains(&self.max_resource_bytes)
            || !(1024..=512 * 1024 * 1024).contains(&self.max_total_bytes)
            || self.max_resource_bytes > self.max_total_bytes
            || !(1..=1_000_000).contains(&self.max_cues)
            || !(1..=100_000).contains(&self.max_segments)
        { return Err(MediaError::InvalidConfig); }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Cue { start_ms: u64, end_ms: u64, text: String }
impl Cue {
    fn seconds(start: f64, end: f64, text: String) -> Result<Self, MediaError> {
        if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start || end > 3_155_760_000.0 || text.contains('\0') { return Err(MediaError::InvalidSubtitle); }
        Ok(Self { start_ms: (start * 1000.0).round() as u64, end_ms: (end * 1000.0).round() as u64, text })
    }
    fn shift(&mut self, seconds: f64) -> Result<(), MediaError> {
        let start = self.start_ms as f64 + seconds * 1000.0;
        let end = self.end_ms as f64 + seconds * 1000.0;
        if !start.is_finite() || !end.is_finite() { return Err(MediaError::InvalidSubtitle); }
        self.start_ms = start.max(0.0).round() as u64;
        self.end_ms = end.max(0.0).round() as u64;
        Ok(())
    }
}
pub(crate) fn filter_dash_for_ffmpeg(text: &str) -> Result<String, MediaError> {
    fn transformed(node: &xml::Node, parent: Option<&xml::Node>) -> bool {
        let attr = |name| node.attr(name).or_else(|| parent.and_then(|parent| parent.attr(name))).unwrap_or("");
        attr("mimeType") == "application/ttml+xml" || attr("codecs").starts_with("stpp") || attr("codecs").starts_with("wvtt")
    }
    fn filter(node: &mut xml::Node) {
        let inherited = node.clone();
        node.children.retain(|child| !matches!(child, xml::Child::Node(child) if child.local() == "Representation" && transformed(child, Some(&inherited))));
        for child in &mut node.children { if let xml::Child::Node(child) = child { filter(child); } }
        node.children.retain(|child| !matches!(child, xml::Child::Node(child) if child.local() == "AdaptationSet" && child.nodes().all(|node| node.local() != "Representation")));
    }
    // Keep legacy manifests byte-for-byte when no converted format is present.
    if !text.contains("application/ttml+xml") && !text.contains("stpp") && !text.contains("wvtt") { return Ok(text.into()); }
    let mut root = xml::parse(text)?;
    filter(&mut root);
    xml::serialize(&root)
}
pub(super) struct Fetcher<'a> {
    transport: &'a NetworkTransport,
    headers: &'a HeaderMap,
    auth: &'a MediaSessionAuth,
    cancel: &'a CancellationToken,
    config: &'a SubtitleConfig,
    total: usize,
    resources: usize,
}
impl Fetcher<'_> {
    async fn get(&mut self, url: Url, range: Option<(u64, u64)>) -> Result<(Vec<u8>, Url), MediaError> {
        self.resources += 1;
        if self.resources > self.config.max_segments.saturating_add(128) { return Err(MediaError::SubtitleLimit); }
        let mut headers = self.headers.clone();
        if let Some((start, end)) = range {
            headers.insert(reqwest::header::RANGE, format!("bytes={start}-{end}").parse().map_err(|_| MediaError::InvalidSubtitle)?);
        }
        let mut response = self.transport.get(url, &headers, self.auth, self.cancel).await?;
        if !response.status().is_success() { return Err(MediaError::HttpStatus(response.status().as_u16())); }
        if let Some((start, end)) = range {
            let content = response.headers().get(reqwest::header::CONTENT_RANGE).and_then(|value| value.to_str().ok());
            let prefix = format!("bytes {start}-{end}/");
            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT || content.is_none_or(|value| !value.starts_with(&prefix)) { return Err(MediaError::InvalidSubtitle); }
        }
        let url = network_url(response.url().as_str())?;
        if response.content_length().is_some_and(|size| size > self.config.max_resource_bytes as u64) { return Err(MediaError::SubtitleLimit); }
        let mut bytes = vec![];
        loop {
            let chunk = tokio::select! { biased; _ = self.cancel.cancelled() => return Err(MediaError::Cancelled), chunk = response.chunk() => chunk.map_err(|_| MediaError::Network)? };
            let Some(chunk) = chunk else { break; };
            self.total = self.total.saturating_add(chunk.len());
            if self.total > self.config.max_total_bytes || bytes.len().saturating_add(chunk.len()) > self.config.max_resource_bytes { return Err(MediaError::SubtitleLimit); }
            bytes.extend_from_slice(&chunk);
        }
        if range.is_some_and(|(start, end)| bytes.len() as u64 != end - start + 1) { return Err(MediaError::InvalidSubtitle); }
        Ok((bytes, url))
    }
}
pub(crate) struct Prepared { pub path: tempfile::TempPath, pub language: Option<String> }
pub(crate) use hls::Anchor;
pub(crate) async fn hls_anchor(transport: &NetworkTransport, headers: &HeaderMap, auth: &MediaSessionAuth, config: &SubtitleConfig, video: &str, cancel: &CancellationToken) -> Result<Anchor, MediaError> {
    let mut fetcher = Fetcher { transport, headers, auth, cancel, config, total: 0, resources: 0 };
    hls::anchor(&mut fetcher, video).await
}
#[allow(clippy::too_many_arguments)]
pub(crate) async fn collect(
    transport: &NetworkTransport, headers: &HeaderMap, auth: &MediaSessionAuth, config: &SubtitleConfig,
    track: &SubtitleTrack, source: &str, anchor: Option<&Anchor>, duration: Option<Duration>, live: bool, parent: &Path, cancel: &CancellationToken,
) -> Result<Prepared, MediaError> {
    if !config.convert_text { return Err(MediaError::UnsupportedSubtitle); }
    let mut fetcher = Fetcher { transport, headers, auth, cancel, config, total: anchor.map_or(0, |anchor| anchor.total_bytes), resources: anchor.map_or(0, |anchor| anchor.resources) };
    let (cues, language) = match &track.locator {
        SubtitleLocator::Dash(id) => dash::collect(&mut fetcher, source, id, duration, live).await?,
        SubtitleLocator::Hls { uri, .. } => hls::collect(&mut fetcher, uri, anchor.ok_or(MediaError::InvalidSubtitle)?.seconds, duration, live).await?,
    };
    let cues: BTreeSet<_> = cues.into_iter().filter(|cue| cue.end_ms > cue.start_ms).collect();
    if cues.is_empty() { return Err(MediaError::InvalidSubtitle); }
    if cues.len() > config.max_cues { return Err(MediaError::SubtitleLimit); }
    let mut text = String::from("WEBVTT\n\n");
    let timestamp = |ms: u64| format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    for cue in cues {
        text.push_str(&format!("{} --> {}\n{}\n\n", timestamp(cue.start_ms), timestamp(cue.end_ms), cue.text));
        if text.len() > config.max_total_bytes { return Err(MediaError::SubtitleLimit); }
    }
    let path = tempfile::Builder::new().prefix(".swoop-media-").suffix(".vtt").tempfile_in(parent)?.into_temp_path();
    tokio::fs::write(&path, text).await?;
    Ok(Prepared { path, language: track.language.clone().or(language) })
}
