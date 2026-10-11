use super::{Cue, Fetcher, bmff};
use crate::{MediaError, manifest, network_url};
use std::{collections::HashMap, time::Duration};
use url::Url;

pub(crate) struct Anchor { pub url: String, pub manifest: String, pub seconds: f64, pub total_bytes: usize, pub resources: usize }
#[derive(Clone)]
struct Segment { url: Url, range: Option<(u64, u64)> }
struct Playlist { segments: Vec<(u64, Segment)>, init: Option<Segment>, ended: bool }
fn resolve(base: &Url, value: &str) -> Result<Url, MediaError> { network_url(base.join(value).map_err(|_| MediaError::InvalidUrl)?.as_str()) }
fn byte_range(raw: &str, previous: u64) -> Result<(u64, u64), MediaError> {
    let (count, offset) = raw.split_once('@').unwrap_or((raw, ""));
    let count: u64 = count.parse().map_err(|_| MediaError::InvalidSubtitle)?;
    let offset = if offset.is_empty() { previous } else { offset.parse().map_err(|_| MediaError::InvalidSubtitle)? };
    if count == 0 { return Err(MediaError::InvalidSubtitle); }
    Ok((offset, offset.checked_add(count - 1).ok_or(MediaError::SubtitleLimit)?))
}
fn playlist(text: &str, base: &Url) -> Result<Playlist, MediaError> {
    manifest::parse_hls(text, base)?;
    let mut sequence: u64 = 0;
    let mut segments = vec![];
    let mut init = None;
    let mut range = None;
    let mut previous_end = 0;
    let mut previous_url: Option<Url> = None;
    for line in text.lines().map(str::trim) {
        if let Some(raw) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") { sequence = raw.parse().map_err(|_| MediaError::InvalidSubtitle)?; }
        if line.starts_with("#EXT-X-KEY:") || line.starts_with("#EXT-X-DISCONTINUITY") || line.starts_with("#EXT-X-STREAM-INF:") { return Err(MediaError::UnsupportedSubtitle); }
        if let Some(raw) = line.strip_prefix("#EXT-X-BYTERANGE:") { range = Some((byte_range(raw, previous_end)?, !raw.contains('@'))); }
        if let Some(raw) = line.strip_prefix("#EXT-X-MAP:") {
            if init.is_some() { return Err(MediaError::UnsupportedSubtitle); }
            let attrs = manifest::attributes(raw)?;
            let value = |name| attrs.iter().find(|(key, _)| *key == name).map(|(_, value)| *value);
            init = Some(Segment { url: resolve(base, value("URI").ok_or(MediaError::InvalidSubtitle)?)?, range: value("BYTERANGE").map(|raw| byte_range(raw, 0)).transpose()? });
        }
        if !line.is_empty() && !line.starts_with('#') {
            let url = resolve(base, line)?;
            if range.is_some_and(|(_, implicit)| implicit && previous_url.as_ref() != Some(&url)) { return Err(MediaError::InvalidSubtitle); }
            let range = range.take().map(|(range, _)| range);
            previous_end = range.map_or(0, |(_, end)| end + 1);
            previous_url = Some(url.clone());
            segments.push((sequence, Segment { url, range }));
            sequence = sequence.checked_add(1).ok_or(MediaError::SubtitleLimit)?;
        }
    }
    Ok(Playlist { segments, init, ended: text.lines().any(|line| line.trim() == "#EXT-X-ENDLIST") })
}
fn ts_timestamp(bytes: &[u8]) -> Result<f64, MediaError> {
    let offset = (0..188.min(bytes.len())).find(|offset| bytes[*offset] == 0x47 && bytes.get(offset + 188).is_some_and(|byte| *byte == 0x47)).ok_or(MediaError::UnsupportedSubtitle)?;
    let mut audio = None;
    for packet in bytes[offset..].as_chunks::<188>().0 {
        if packet[0] != 0x47 || packet[1] & 0x40 == 0 || packet[3] & 0x10 == 0 { continue; }
        let start = if packet[3] & 0x20 != 0 { 5 + packet[4] as usize } else { 4 };
        let Some(data) = packet.get(start..) else { return Err(MediaError::InvalidSubtitle); };
        if data.len() < 14 || data[..3] != [0, 0, 1] || data[7] & 0x80 == 0 { continue; }
        let pts = data[9..14].to_vec();
        if pts[0] & 1 == 0 || pts[2] & 1 == 0 || pts[4] & 1 == 0 { return Err(MediaError::InvalidSubtitle); }
        let timestamp = (u64::from((pts[0] >> 1) & 7) << 30) | (u64::from(pts[1]) << 22) | (u64::from(pts[2] >> 1) << 15) | (u64::from(pts[3]) << 7) | u64::from(pts[4] >> 1);
        if (0xe0..=0xef).contains(&data[3]) { return Ok(timestamp as f64 / 90_000.0); }
        if (0xc0..=0xdf).contains(&data[3]) { audio = Some(timestamp as f64 / 90_000.0); }
    }
    audio.ok_or(MediaError::UnsupportedSubtitle)
}
pub(super) async fn anchor(fetcher: &mut Fetcher<'_>, video: &str) -> Result<Anchor, MediaError> {
    let (bytes, base) = fetcher.get(network_url(video)?, None).await?;
    let manifest = std::str::from_utf8(&bytes).map_err(|_| MediaError::InvalidSubtitle)?.to_owned();
    let list = playlist(&manifest, &base)?;
    let (_, first) = list.segments.first().ok_or(MediaError::InvalidSubtitle)?;
    let (bytes, _) = fetcher.get(first.url.clone(), first.range).await?;
    let seconds = if let Some(init) = list.init {
        let (init, _) = fetcher.get(init.url, init.range).await?;
        bmff::media_timestamp(&init, &bytes)?
    } else { ts_timestamp(&bytes)? };
    Ok(Anchor { url: video.into(), manifest, seconds, total_bytes: fetcher.total, resources: fetcher.resources })
}
fn timestamp(raw: &str) -> Result<f64, MediaError> {
    let fields = raw.split(':').collect::<Vec<_>>();
    if !(2..=3).contains(&fields.len()) { return Err(MediaError::InvalidSubtitle); }
    let mut seconds = 0.0;
    for (index, raw) in fields.iter().enumerate() {
        let value: f64 = raw.parse().map_err(|_| MediaError::InvalidSubtitle)?;
        if !value.is_finite() || value < 0.0 || (index > 0 && value >= 60.0) { return Err(MediaError::InvalidSubtitle); }
        seconds = seconds * 60.0 + value;
    }
    Ok(seconds)
}
fn webvtt(bytes: &[u8], anchor: f64) -> Result<Vec<Cue>, MediaError> {
    let text = std::str::from_utf8(bytes).map_err(|_| MediaError::InvalidSubtitle)?.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    if !text.starts_with("WEBVTT") { return Err(MediaError::InvalidSubtitle); }
    let mut shift = -anchor;
    for line in text.lines().take_while(|line| !line.is_empty()) {
        if let Some(map) = line.strip_prefix("X-TIMESTAMP-MAP=") {
            let values = map.split(',').filter_map(|field| field.split_once(':')).collect::<Vec<_>>();
            let local = values.iter().find(|(key, _)| *key == "LOCAL").map(|(_, value)| *value).ok_or(MediaError::InvalidSubtitle)?;
            let mpeg: u64 = values.iter().find(|(key, _)| *key == "MPEGTS").map(|(_, value)| *value).ok_or(MediaError::InvalidSubtitle)?.parse().map_err(|_| MediaError::InvalidSubtitle)?;
            if mpeg >= 1u64 << 33 { return Err(MediaError::InvalidSubtitle); }
            let seconds = mpeg as f64 / 90_000.0;
            let wrap = (1u64 << 33) as f64 / 90_000.0;
            shift += seconds + ((anchor - seconds) / wrap).round() * wrap - timestamp(local)?;
        }
    }
    let mut cues = vec![];
    for block in text.split("\n\n").skip(1) {
        if block.starts_with("NOTE") || block.starts_with("STYLE") || block.starts_with("REGION") { continue; }
        let lines = block.lines().collect::<Vec<_>>();
        let Some((index, times)) = lines.iter().enumerate().find(|(_, line)| line.contains(" --> ")) else { if block.trim().is_empty() { continue; } return Err(MediaError::InvalidSubtitle); };
        if index > 1 { return Err(MediaError::InvalidSubtitle); }
        let (start, end) = times.split_once(" --> ").ok_or(MediaError::InvalidSubtitle)?;
        let mut cue = Cue::seconds(timestamp(start)?, timestamp(end.split_whitespace().next().ok_or(MediaError::InvalidSubtitle)?)?, lines[index + 1..].join("\n"))?;
        cue.shift(shift)?;
        if cue.end_ms > cue.start_ms && !cue.text.trim().is_empty() { cues.push(cue); }
    }
    Ok(cues)
}
pub(super) async fn collect(fetcher: &mut Fetcher<'_>, uri: &str, anchor: f64, duration: Option<Duration>, live: bool) -> Result<(Vec<Cue>, Option<String>), MediaError> {
    let deadline = tokio::time::Instant::now() + duration.unwrap_or_default() + Duration::from_secs(3);
    let mut seen: HashMap<u64, String> = HashMap::new();
    let mut track = None;
    let mut init_url = None;
    let mut cues = vec![];
    let mut language = None;
    loop {
        let (bytes, base) = fetcher.get(network_url(uri)?, None).await?;
        let list = playlist(std::str::from_utf8(&bytes).map_err(|_| MediaError::InvalidSubtitle)?, &base)?;
        if let Some(first) = list.segments.first()
            && seen.keys().max().is_some_and(|last| first.0 > last.saturating_add(1))
        { return Err(MediaError::InvalidSubtitle); }
        if let Some(init) = list.init {
            if init_url.as_ref().is_some_and(|url| url != &init.url) { return Err(MediaError::UnsupportedSubtitle); }
            if track.is_none() {
                let (bytes, _) = fetcher.get(init.url.clone(), init.range).await?;
                let parsed = bmff::init(&bytes)?;
                if parsed.codec != *b"wvtt" { return Err(MediaError::UnsupportedSubtitle); }
                language = parsed.language.clone();
                track = Some(parsed);
                init_url = Some(init.url);
            }
        }
        for (number, resource) in list.segments {
            let identity = format!("{}:{:?}", resource.url, resource.range);
            if let Some(previous) = seen.get(&number) { if previous != &identity { return Err(MediaError::InvalidSubtitle); } continue; }
            if seen.len() >= fetcher.config.max_segments { return Err(MediaError::SubtitleLimit); }
            let (bytes, _) = fetcher.get(resource.url, resource.range).await?;
            let new_cues = if let Some(track) = &track {
                let mut output = vec![];
                for sample in bmff::samples(&bytes, track, fetcher.config.max_cues)? { output.extend(bmff::webvtt(&sample)?); }
                for cue in &mut output { cue.shift(-anchor)?; }
                output
            } else { webvtt(&bytes, anchor)? };
            cues.extend(new_cues);
            if cues.len() > fetcher.config.max_cues { return Err(MediaError::SubtitleLimit); }
            seen.insert(number, identity);
        }
        if list.ended || !live || tokio::time::Instant::now() >= deadline { break; }
        tokio::select! { biased; _ = fetcher.cancel.cancelled() => return Err(MediaError::Cancelled), _ = tokio::time::sleep(fetcher.transport.live_manifest_refresh) => {} }
    }
    Ok((cues, language))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vtt_map_and_pts_wrap_preserve_relative_time() {
        let cues = webvtt(b"WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:126000\n\n00:00:00.200 --> 00:00:01.200\nhello\n", 1.4).unwrap();
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (200, 1200));
    }
}
