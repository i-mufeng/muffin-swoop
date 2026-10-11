use std::{
    collections::{HashSet, VecDeque},
    time::Duration,
};

use quick_xml::{Reader, events::Event};
use reqwest::header::HeaderMap;
use url::Url;

use crate::selection::{AudioTrack, Locator, MediaVariant, SubtitleFormat, SubtitleLocator, SubtitleTrack, key};
use crate::{MediaConfig, MediaError, network_url};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Hls,
    Dash,
    Mms,
}

#[derive(Clone, Debug)]
pub struct MediaInfo {
    pub kind: MediaKind,
    pub is_live: bool,
    pub is_master: bool,
    pub duration: Option<Duration>,
    pub variant_count: usize,
    pub uses_aes128: bool,
    pub variants: Vec<MediaVariant>,
    pub audio_tracks: Vec<AudioTrack>,
    pub subtitle_tracks: Vec<SubtitleTrack>,
}

pub(crate) async fn inspect(
    transport: &crate::network::NetworkTransport,
    config: &MediaConfig,
    url: Url,
    headers: HeaderMap,
    auth: &crate::MediaSessionAuth,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<MediaInfo, MediaError> {
    let mut pending = VecDeque::from([(url, 0usize)]);
    let mut seen = HashSet::new();
    let mut root: Option<MediaInfo> = None;
    let mut playable = false;
    while let Some((url, depth)) = pending.pop_front() {
        if !seen.insert(url.clone()) {
            continue;
        }
        if seen.len() > config.max_playlists || depth > 4 {
            return Err(MediaError::ManifestLimit);
        }
        let mut response = transport.get(url, &headers, auth, cancel).await?;
        if !response.status().is_success() {
            return Err(MediaError::HttpStatus(response.status().as_u16()));
        }
        let final_url = network_url(response.url().as_str())?;
        if response.content_length().is_some_and(|size| size > config.max_manifest_bytes as u64) {
            return Err(MediaError::ManifestLimit);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| MediaError::Network)? {
            if bytes.len().saturating_add(chunk.len()) > config.max_manifest_bytes {
                return Err(MediaError::ManifestLimit);
            }
            bytes.extend_from_slice(&chunk);
        }
        let text =
            std::str::from_utf8(&bytes).map_err(|_| MediaError::InvalidManifest)?.trim_start_matches('\u{feff}').trim();
        let (info, children) = if text.starts_with("#EXTM3U") {
            parse_hls(text, &final_url)?
        } else {
            (parse_dash(text, &final_url)?, Vec::new())
        };
        playable |= info.kind == MediaKind::Dash || !info.is_master;
        if let Some(root) = root.as_mut() {
            if info.kind != MediaKind::Hls {
                return Err(MediaError::InvalidManifest);
            }
            root.is_live |= info.is_live;
            root.uses_aes128 |= info.uses_aes128;
            root.duration = root.duration.max(info.duration);
        } else {
            root = Some(info);
        }
        if pending.len().saturating_add(children.len()).saturating_add(seen.len()) > config.max_playlists {
            return Err(MediaError::ManifestLimit);
        }
        pending.extend(children.into_iter().map(|url| (url, depth + 1)));
    }
    if !playable {
        return Err(MediaError::InvalidManifest);
    }
    root.ok_or(MediaError::InvalidManifest)
}

fn resolve(base: &Url, value: &str) -> Result<Url, MediaError> {
    // Url::join normalizes control characters; reject them before normalization.
    if value.contains(['\r', '\n', '\0']) {
        return Err(MediaError::InvalidUrl);
    }
    let joined = base.join(value).map_err(|_| MediaError::InvalidUrl)?;
    network_url(joined.as_str())
}

pub(crate) fn attributes(raw: &str) -> Result<Vec<(&str, &str)>, MediaError> {
    let mut result = Vec::new();
    let mut quoted = false;
    let mut begin = 0;
    for (index, character) in raw.char_indices().chain(std::iter::once((raw.len(), ','))) {
        if character == '"' {
            quoted = !quoted;
        }
        if character == ',' && !quoted {
            let (key, value) = raw[begin..index].trim().split_once('=').ok_or(MediaError::InvalidManifest)?;
            if result.iter().any(|(existing, _)| *existing == key) {
                return Err(MediaError::InvalidManifest);
            }
            let value = if value.starts_with('"') {
                value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).ok_or(MediaError::InvalidManifest)?
            } else {
                value
            };
            result.push((key, value));
            begin = index + 1;
        }
    }
    if quoted {
        return Err(MediaError::InvalidManifest);
    }
    Ok(result)
}

pub(crate) fn parse_hls(text: &str, base: &Url) -> Result<(MediaInfo, Vec<Url>), MediaError> {
    let mut info = MediaInfo {
        kind: MediaKind::Hls,
        is_live: false,
        is_master: false,
        duration: None,
        variant_count: 0,
        uses_aes128: false,
        variants: Vec::new(),
        audio_tracks: Vec::new(),
        subtitle_tracks: Vec::new(),
    };
    let mut children = Vec::new();
    let mut stream_next = false;
    let mut variant_next = None;
    let mut segments = 0;
    let mut seconds = 0.0;
    let mut ended = false;
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("#EXTM3U") {
        return Err(MediaError::InvalidManifest);
    }
    for line in lines.map(str::trim).filter(|line| !line.is_empty()) {
        if line == "#EXT-X-ENDLIST" {
            ended = true;
        }
        if let Some(duration) = line.strip_prefix("#EXTINF:") {
            let duration =
                duration.split(',').next().unwrap_or("").parse::<f64>().map_err(|_| MediaError::InvalidManifest)?;
            if !duration.is_finite() || duration < 0.0 {
                return Err(MediaError::InvalidManifest);
            }
            seconds += duration;
            if seconds > 3_155_760_000.0 {
                return Err(MediaError::ManifestLimit);
            }
        }
        if let Some((tag, raw)) = line.split_once(':').filter(|(tag, _)| tag.starts_with("#EXT-X-")) {
            // Only these tags have attribute lists containing resource URIs.
            if matches!(
                tag,
                "#EXT-X-KEY"
                    | "#EXT-X-SESSION-KEY"
                    | "#EXT-X-MAP"
                    | "#EXT-X-MEDIA"
                    | "#EXT-X-I-FRAME-STREAM-INF"
                    | "#EXT-X-PART"
                    | "#EXT-X-PRELOAD-HINT"
                    | "#EXT-X-RENDITION-REPORT"
                    | "#EXT-X-STREAM-INF"
            ) {
                let attrs = attributes(raw)?;
                let get = |key| attrs.iter().find(|(name, _)| *name == key).map(|(_, value)| *value);
                if matches!(tag, "#EXT-X-KEY" | "#EXT-X-SESSION-KEY") {
                    match get("METHOD") {
                        Some("NONE") => {}
                        Some("AES-128") if get("KEYFORMAT").is_none_or(|format| format == "identity") => {
                            if get("URI").is_none() {
                                return Err(MediaError::InvalidManifest);
                            }
                            info.uses_aes128 = true;
                        }
                        _ => return Err(MediaError::ProtectedMedia),
                    }
                }
                if let Some(uri) = get("URI") {
                    let uri = resolve(base, uri)?;
                    if tag == "#EXT-X-MEDIA" && get("TYPE") == Some("AUDIO") {
                        let group = get("GROUP-ID").map(str::to_owned);
                        let name = get("NAME").unwrap_or("Audio").to_owned();
                        info.audio_tracks.push(AudioTrack {
                            id: key("hls-a", &format!("{}|{}|{}", uri, group.as_deref().unwrap_or(""), name)),
                            name,
                            language: get("LANGUAGE").map(str::to_owned),
                            bandwidth: None,
                            codecs: None,
                            is_default: get("DEFAULT") == Some("YES"),
                            group,
                            locator: Locator::Hls(uri.to_string()),
                        });
                    }
                    if tag == "#EXT-X-MEDIA" && get("TYPE") == Some("SUBTITLES") {
                        let group = get("GROUP-ID").map(str::to_owned);
                        let name = get("NAME").ok_or(MediaError::InvalidManifest)?.to_owned();
                        info.subtitle_tracks.push(SubtitleTrack {
                            id: key("hls-s", &format!("{}|{}|{}", uri, group.as_deref().unwrap_or(""), name)),
                            name: name.clone(),
                            language: get("LANGUAGE").map(str::to_owned),
                            codecs: Some("webvtt".into()),
                            format: SubtitleFormat::WebVtt,
                            group,
                            locator: SubtitleLocator::Hls { master: base.to_string(), name, uri: uri.to_string() },
                        });
                    }
                    if matches!(tag, "#EXT-X-MEDIA" | "#EXT-X-I-FRAME-STREAM-INF" | "#EXT-X-RENDITION-REPORT") {
                        children.push(uri);
                    }
                }
                if tag == "#EXT-X-STREAM-INF" {
                    let resolution = get("RESOLUTION")
                        .and_then(|raw| raw.split_once('x'))
                        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
                    variant_next = Some(MediaVariant {
                        id: String::new(),
                        width: resolution.map(|(w, _)| w),
                        height: resolution.map(|(_, h)| h),
                        bandwidth: get("AVERAGE-BANDWIDTH")
                            .or_else(|| get("BANDWIDTH"))
                            .and_then(|raw| raw.parse().ok()),
                        codecs: get("CODECS").map(str::to_owned),
                        audio_group: get("AUDIO").map(str::to_owned),
                        subtitle_group: get("SUBTITLES").filter(|value| *value != "NONE").map(str::to_owned),
                        locator: Locator::Hls(String::new()),
                    });
                    stream_next = true;
                    info.variant_count += 1;
                }
            }
        }
        if !line.starts_with('#') {
            let uri = resolve(base, line)?;
            if stream_next {
                if let Some(mut variant) = variant_next.take() {
                    variant.id = key("hls-v", &format!("{}|{}", uri, variant.audio_group.as_deref().unwrap_or("")));
                    variant.locator = Locator::Hls(uri.to_string());
                    info.variants.push(variant);
                }
                children.push(uri);
                stream_next = false;
            } else {
                segments += 1;
            }
        }
    }
    if stream_next || (segments == 0 && children.is_empty()) {
        return Err(MediaError::InvalidManifest);
    }
    info.is_master = !children.is_empty() && segments == 0;
    info.is_live = segments > 0 && !ended;
    if seconds > 0.0 {
        info.duration = Some(Duration::from_secs_f64(seconds));
    }
    Ok((info, children))
}

fn parse_dash(text: &str, base: &Url) -> Result<MediaInfo, MediaError> {
    let mut reader = Reader::from_str(text);
    let mut info = MediaInfo {
        kind: MediaKind::Dash,
        is_live: false,
        is_master: false,
        duration: None,
        variant_count: 0,
        uses_aes128: false,
        variants: Vec::new(),
        audio_tracks: Vec::new(),
        subtitle_tracks: Vec::new(),
    };
    let mut root_seen = false;
    let mut depth = 0usize;
    let mut base_text: Option<String> = None;
    let mut adaptation = std::collections::HashMap::<String, String>::new();
    loop {
        let event = reader.read_event().map_err(|_| MediaError::InvalidManifest)?;
        let empty = matches!(&event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let local_name = element.local_name();
                let name = local_name.as_ref();
                if depth == 0 {
                    if root_seen || name != b"MPD" {
                        return Err(MediaError::InvalidManifest);
                    }
                    root_seen = true;
                }
                if name == b"ContentProtection" {
                    return Err(MediaError::ProtectedMedia);
                }
                if name == b"Representation" {
                    info.variant_count += 1;
                }
                if name == b"BaseURL" {
                    base_text = Some(String::new());
                }
                let mut attrs = std::collections::HashMap::new();
                for attr in element.attributes() {
                    let attr = attr.map_err(|_| MediaError::InvalidManifest)?;
                    let value = attr
                        .decoded_and_normalized_value(quick_xml::XmlVersion::default(), reader.decoder())
                        .map_err(|_| MediaError::InvalidManifest)?;
                    match attr.key.local_name().as_ref() {
                        b"type" if name == b"MPD" => info.is_live = value == "dynamic",
                        b"mediaPresentationDuration" if name == b"MPD" => info.duration = iso_duration(&value),
                        b"media" | b"initialization" | b"sourceURL" | b"href" | b"base" => {
                            resolve(base, &value)?;
                        }
                        _ => {}
                    }
                    attrs.insert(
                        String::from_utf8_lossy(attr.key.local_name().as_ref()).into_owned(),
                        value.into_owned(),
                    );
                }
                if name == b"AdaptationSet" {
                    adaptation = attrs.clone();
                }
                if name == b"Representation" {
                    let value = |name: &str| attrs.get(name).or_else(|| adaptation.get(name));
                    let mime = value("mimeType").map(String::as_str).unwrap_or("");
                    let content = value("contentType").map(String::as_str).unwrap_or("");
                    if let Some(id) = attrs.get("id") {
                        if content == "video" || mime.starts_with("video/") || value("height").is_some() {
                            info.variants.push(MediaVariant {
                                id: key("dash-v", id),
                                width: value("width").and_then(|raw| raw.parse().ok()),
                                height: value("height").and_then(|raw| raw.parse().ok()),
                                bandwidth: value("bandwidth").and_then(|raw| raw.parse().ok()),
                                codecs: value("codecs").cloned(),
                                audio_group: None,
                                subtitle_group: None,
                                locator: Locator::Dash(id.clone()),
                            });
                        } else if content == "text"
                            || content == "subtitle"
                            || mime.starts_with("text/") || mime == "application/ttml+xml"
                            || value("codecs")
                                .is_some_and(|codec| codec.starts_with("stpp") || codec.starts_with("wvtt"))
                        {
                            let codecs = value("codecs").cloned();
                            let format = if mime == "text/vtt" {
                                SubtitleFormat::WebVtt
                            } else if matches!(mime, "application/x-subrip" | "text/srt") {
                                SubtitleFormat::SubRip
                            } else if matches!(mime, "text/x-ssa" | "text/x-ass") {
                                SubtitleFormat::Ass
                            } else if codecs.as_deref().is_some_and(|codec| codec == "tx3g" || codec == "mov_text") {
                                SubtitleFormat::MovText
                            } else {
                                SubtitleFormat::Unsupported
                            };
                            info.subtitle_tracks.push(SubtitleTrack {
                                id: key("dash-s", id),
                                name: format!("Subtitle {id}"),
                                language: value("lang").cloned(),
                                codecs,
                                format,
                                group: None,
                                locator: SubtitleLocator::Dash(id.clone()),
                            });
                        } else if content == "audio"
                            || mime.starts_with("audio/")
                            || value("audioSamplingRate").is_some()
                        {
                            info.audio_tracks.push(AudioTrack {
                                id: key("dash-a", id),
                                name: format!("Audio {id}"),
                                language: value("lang").cloned(),
                                bandwidth: value("bandwidth").and_then(|raw| raw.parse().ok()),
                                codecs: value("codecs").cloned(),
                                is_default: false,
                                group: None,
                                locator: Locator::Dash(id.clone()),
                            });
                        }
                    }
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(element) => {
                depth = depth.checked_sub(1).ok_or(MediaError::InvalidManifest)?;
                if element.local_name().as_ref() == b"AdaptationSet" {
                    adaptation.clear();
                }
                if let (true, Some(value)) = (element.local_name().as_ref() == b"BaseURL", base_text.as_ref()) {
                    resolve(base, value.trim())?;
                    base_text = None;
                }
            }
            Event::Text(value) => {
                if let Some(text) = base_text.as_mut() {
                    let decoded =
                        value.xml_content(quick_xml::XmlVersion::default()).map_err(|_| MediaError::InvalidManifest)?;
                    text.push_str(&quick_xml::escape::unescape(&decoded).map_err(|_| MediaError::InvalidManifest)?);
                }
            }
            Event::CData(value) => {
                if let Some(text) = base_text.as_mut() {
                    text.push_str(&value.decode().map_err(|_| MediaError::InvalidManifest)?);
                }
            }
            Event::GeneralRef(reference) => {
                let decoded =
                    if let Some(character) = reference.resolve_char_ref().map_err(|_| MediaError::InvalidManifest)? {
                        character.to_string()
                    } else {
                        let name = reference.decode().map_err(|_| MediaError::InvalidManifest)?;
                        match name.as_ref() {
                            "amp" => "&",
                            "lt" => "<",
                            "gt" => ">",
                            "apos" => "'",
                            "quot" => "\"",
                            _ => return Err(MediaError::InvalidManifest),
                        }
                        .to_owned()
                    };
                if let Some(text) = base_text.as_mut() {
                    text.push_str(&decoded);
                }
            }
            Event::DocType(_) => return Err(MediaError::InvalidManifest),
            Event::Eof => break,
            _ => {}
        }
    }
    if !root_seen || depth != 0 || info.variant_count == 0 {
        return Err(MediaError::InvalidManifest);
    }
    Ok(info)
}

pub(crate) fn iso_duration(value: &str) -> Option<Duration> {
    let value = value.strip_prefix("PT")?;
    let mut begin = 0;
    let mut seconds = 0.0;
    for (index, character) in value.char_indices() {
        let multiplier = match character {
            'H' => 3600.0,
            'M' => 60.0,
            'S' => 1.0,
            _ => continue,
        };
        seconds += value[begin..index].parse::<f64>().ok()? * multiplier;
        begin = index + 1;
    }
    if begin != value.len() || !seconds.is_finite() || !(0.0..=3_155_760_000.0).contains(&seconds) {
        return None;
    }
    Some(Duration::from_secs_f64(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn base() -> Url {
        Url::parse("https://example.org/media/master.m3u8?token=private").unwrap()
    }

    #[test]
    fn hls_aes128_is_supported_but_drm_and_local_files_are_rejected() {
        let plain = "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\nsegment.ts\n#EXT-X-ENDLIST\n";
        let (info, _) = parse_hls(plain, &base()).unwrap();
        assert!(!info.is_live);
        assert_eq!(info.duration, Some(Duration::from_secs(2)));
        let aes = plain.replace("#EXTINF", "#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n#EXTINF");
        assert!(parse_hls(&aes, &base()).unwrap().0.uses_aes128);
        assert!(matches!(parse_hls(&aes.replace("AES-128", "SAMPLE-AES"), &base()), Err(MediaError::ProtectedMedia)));
        assert!(matches!(
            parse_hls(&aes.replace("key.bin", "file:///private/key.bin"), &base()),
            Err(MediaError::InvalidUrl)
        ));
        assert!(parse_hls(&plain.replace("segment.ts", "file:///private/segment.ts"), &base()).is_err());
    }

    #[test]
    fn master_tracks_variants_and_quoted_attribute_commas() {
        let (info, children) =
            parse_hls("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100,CODECS=\"avc1,mp4a\"\nv1.m3u8\n", &base()).unwrap();
        assert!(info.is_master);
        assert_eq!(info.variant_count, 1);
        assert_eq!(children[0].as_str(), "https://example.org/media/v1.m3u8");
    }

    #[test]
    fn dash_rejects_protection_and_local_base_urls() {
        let xml = "<MPD type=\"static\" mediaPresentationDuration=\"PT3.5S\"><Period><AdaptationSet><Representation id=\"1\"><BaseURL>segment.mp4</BaseURL></Representation></AdaptationSet></Period></MPD>";
        assert_eq!(parse_dash(xml, &base()).unwrap().duration, Some(Duration::from_millis(3500)));
        assert!(matches!(
            parse_dash(&xml.replace("<BaseURL>", "<ContentProtection/><BaseURL>"), &base()),
            Err(MediaError::ProtectedMedia)
        ));
        assert!(parse_dash(&xml.replace("segment.mp4", "file:///private/local.mp4"), &base()).is_err());
        assert!(parse_dash(&xml.replace("segment.mp4", "segment.mp4?x=1&amp;y=2"), &base()).is_ok());
        assert!(parse_dash(&xml.replace("segment.mp4", "&#102;ile:///private/local.mp4"), &base()).is_err());
        assert!(parse_dash("<MPD><Representation></MPD>", &base()).is_err());
    }

    #[test]
    fn subtitle_metadata_and_unverified_formats_are_explicit() {
        let (info, children) = parse_hls(concat!(
            "#EXTM3U\n",
            "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"中文\",LANGUAGE=\"zh-Hans\",URI=\"zh.m3u8?key=secret\"\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=123,RESOLUTION=320x180,SUBTITLES=\"subs\"\nvideo.m3u8\n"
        ), &base()).unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(info.subtitle_tracks[0].language.as_deref(), Some("zh-Hans"));
        assert_eq!(info.subtitle_tracks[0].format, SubtitleFormat::WebVtt);
        assert_eq!(info.subtitle_tracks_for(&crate::VideoSelection::Best).unwrap().len(), 1);
        assert!(!format!("{:?}", info.subtitle_tracks).contains("secret"));
        let xml = concat!(
            "<MPD><Period>",
            "<AdaptationSet contentType=\"text\" mimeType=\"text/vtt\" lang=\"eng\"><Representation id=\"vtt\"><BaseURL>en.vtt</BaseURL></Representation></AdaptationSet>",
            "<AdaptationSet contentType=\"text\" mimeType=\"application/mp4\" lang=\"zho\"><Representation id=\"text\" codecs=\"tx3g\"/></AdaptationSet>",
            "<AdaptationSet contentType=\"text\" mimeType=\"application/mp4\"><Representation id=\"ttml\" codecs=\"stpp\"/><Representation id=\"boxed-vtt\" codecs=\"wvtt\"/></AdaptationSet>",
            "</Period></MPD>"
        );
        let info = parse_dash(xml, &base()).unwrap();
        assert_eq!(
            info.subtitle_tracks.iter().map(|track| track.format).collect::<Vec<_>>(),
            [SubtitleFormat::WebVtt, SubtitleFormat::MovText, SubtitleFormat::Unsupported, SubtitleFormat::Unsupported]
        );
        assert_eq!(info.subtitle_tracks[1].language.as_deref(), Some("zho"));
        assert!(matches!(
            parse_dash(&xml.replace("<BaseURL>", "<ContentProtection/><BaseURL>"), &base()),
            Err(MediaError::ProtectedMedia)
        ));
    }
}
