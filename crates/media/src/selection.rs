use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{MediaError, MediaInfo, MediaKind, MediaSource};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoSelection {
    #[default]
    Best,
    MaxHeight(u32),
    Variant(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioSelection {
    #[default]
    Auto,
    PreferredLanguage(String),
    Track(String),
    None,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubtitleSelection {
    #[default]
    None,
    Track(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaSelection {
    pub video: VideoSelection,
    pub audio: AudioSelection,
    pub subtitle: SubtitleSelection,
}

#[derive(Clone)]
pub(crate) enum Locator {
    Hls(String),
    Dash(String),
}
impl std::fmt::Debug for Locator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

#[derive(Clone, Debug)]
pub struct MediaVariant {
    /// Opaque stable key. Downloads re-inspect and reject keys no longer present.
    pub id: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bandwidth: Option<u64>,
    pub codecs: Option<String>,
    pub(crate) audio_group: Option<String>,
    pub(crate) subtitle_group: Option<String>,
    pub(crate) locator: Locator,
}

#[derive(Clone, Debug)]
pub struct AudioTrack {
    pub id: String,
    pub name: String,
    pub language: Option<String>,
    pub bandwidth: Option<u64>,
    pub codecs: Option<String>,
    pub is_default: bool,
    pub(crate) group: Option<String>,
    pub(crate) locator: Locator,
}

pub(crate) fn key(kind: &str, identity: &str) -> String {
    format!("{kind}-{:x}", Sha256::digest(identity.as_bytes()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubtitleFormat {
    WebVtt,
    MovText,
    SubRip,
    Ass,
    Unsupported,
}

#[derive(Clone)]
pub(crate) enum SubtitleLocator {
    Hls { master: String, name: String, uri: String },
    Dash(String),
}
impl std::fmt::Debug for SubtitleLocator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

#[derive(Clone, Debug)]
pub struct SubtitleTrack {
    pub id: String,
    pub name: String,
    pub language: Option<String>,
    pub codecs: Option<String>,
    pub format: SubtitleFormat,
    pub(crate) group: Option<String>,
    pub(crate) locator: SubtitleLocator,
}

pub(crate) struct Plan {
    pub inputs: Vec<String>,
    pub maps: Vec<String>,
    pub subtitle: Option<SubtitleFormat>,
    pub subtitle_language: Option<String>,
    pub subtitle_input: Option<usize>,
}

fn dash_map(kind: &str, id: &str) -> Result<String, MediaError> {
    // Keep metadata stream specifiers unambiguous. An unsupported identifier
    // fails explicitly instead of accidentally matching another representation.
    metadata_map(0, kind, "id", id)
}

fn metadata_map(input: usize, kind: &str, field: &str, value: &str) -> Result<String, MediaError> {
    if value.is_empty() || value.contains([':', '\\', '\'', '"', '?', '\r', '\n', '\0']) {
        return Err(MediaError::InvalidSelection);
    }
    Ok(format!("{input}:{kind}:m:{field}:{value}"))
}

fn selected_video<'a>(info: &'a MediaInfo, selection: &VideoSelection) -> Result<Option<&'a MediaVariant>, MediaError> {
    let candidates = info
        .variants
        .iter()
        .filter(|variant| match selection {
            VideoSelection::Best => true,
            VideoSelection::MaxHeight(height) => *height > 0 && variant.height.is_some_and(|h| h <= *height),
            VideoSelection::Variant(id) => &variant.id == id,
        })
        .collect::<Vec<_>>();
    if !matches!(selection, VideoSelection::Best) && candidates.is_empty() {
        return Err(MediaError::InvalidSelection);
    }
    if matches!(selection, VideoSelection::Variant(_)) && candidates.len() != 1 {
        return Err(MediaError::InvalidSelection);
    }
    let video = candidates.into_iter().max_by_key(|variant| {
        (u64::from(variant.width.unwrap_or(0)) * u64::from(variant.height.unwrap_or(0)), variant.bandwidth.unwrap_or(0))
    });
    if video.is_some_and(|selected| info.variants.iter().filter(|variant| variant.id == selected.id).count() != 1) {
        return Err(MediaError::InvalidSelection);
    }
    Ok(video)
}

impl MediaInfo {
    pub fn subtitle_tracks_for(&self, selection: &VideoSelection) -> Result<Vec<&SubtitleTrack>, MediaError> {
        let video = selected_video(self, selection)?;
        Ok(self
            .subtitle_tracks
            .iter()
            .filter(|track| {
                self.kind == MediaKind::Dash || video.is_none_or(|video| video.subtitle_group == track.group)
            })
            .collect())
    }
    /// HLS rendition groups must match the selected video variant.
    pub fn audio_tracks_for(&self, selection: &VideoSelection) -> Result<Vec<&AudioTrack>, MediaError> {
        let video = selected_video(self, selection)?;
        Ok(self
            .audio_tracks
            .iter()
            .filter(|track| self.kind == MediaKind::Dash || video.is_none_or(|video| video.audio_group == track.group))
            .collect())
    }
}

pub(crate) fn plan(source: &MediaSource, info: &MediaInfo, selection: &MediaSelection) -> Result<Plan, MediaError> {
    if info.variants.is_empty()
        && info.audio_tracks.is_empty()
        && selection.video == VideoSelection::Best
        && matches!(selection.audio, AudioSelection::Auto | AudioSelection::PreferredLanguage(_))
        && selection.subtitle == SubtitleSelection::None
    {
        // Plain media playlists have no advertised alternatives. Retain FFmpeg's
        // automatic best-stream selection when no exact mapping was requested.
        return Ok(Plan {
            inputs: vec![source.url.clone()],
            maps: Vec::new(),
            subtitle: None,
            subtitle_language: None,
            subtitle_input: None,
        });
    }
    let video = selected_video(info, &selection.video)?;
    let tracks = info.audio_tracks_for(&selection.video)?;
    let audio = match &selection.audio {
        AudioSelection::None => None,
        AudioSelection::Track(id) => {
            let found = tracks.iter().copied().filter(|track| &track.id == id).collect::<Vec<_>>();
            if found.len() != 1 { return Err(MediaError::InvalidSelection); }
            Some(found[0])
        }
        preference => {
            tracks.into_iter().max_by_key(|track| (
                matches!(preference, AudioSelection::PreferredLanguage(language) if track.language.as_ref().is_some_and(|value| value.eq_ignore_ascii_case(language))),
                track.is_default,
                track.bandwidth.unwrap_or(0),
            ))
        }
    };
    if audio.is_some_and(|selected| info.audio_tracks.iter().filter(|track| track.id == selected.id).count() != 1) {
        return Err(MediaError::InvalidSelection);
    }
    let mut inputs = vec![source.url.clone()];
    let mut maps = Vec::new();
    if let Some(video) = video {
        match &video.locator {
            Locator::Hls(url) => {
                inputs[0] = url.clone();
                maps.push(if video.height.is_some() || video.width.is_some() { "0:v:0" } else { "0:v:0?" }.into());
            }
            Locator::Dash(id) => maps.push(dash_map("v", id)?),
        }
    } else {
        maps.push("0:v:0?".into());
    }
    if let Some(audio) = audio {
        match &audio.locator {
            Locator::Hls(url) => {
                if inputs[0] == *url {
                    maps.push("0:a:0".into());
                } else {
                    inputs.push(url.clone());
                    maps.push("1:a:0".into());
                }
            }
            Locator::Dash(id) => maps.push(dash_map("a", id)?),
        }
    } else if selection.audio != AudioSelection::None {
        maps.push("0:a:0?".into());
    }
    let mut subtitle = None;
    let mut subtitle_language = None;
    let mut subtitle_input = None;
    if let SubtitleSelection::Track(id) = &selection.subtitle {
        let matches =
            info.subtitle_tracks_for(&selection.video)?.into_iter().filter(|track| &track.id == id).collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(MediaError::InvalidSelection);
        }
        let track = matches[0];
        if track.format == SubtitleFormat::Unsupported {
            return Err(MediaError::UnsupportedSubtitle);
        }
        match &track.locator {
            SubtitleLocator::Dash(id) => {
                if info.subtitle_tracks.iter().filter(|other| other.id == track.id).count() != 1 {
                    return Err(MediaError::InvalidSelection);
                }
                inputs.push(source.url.clone());
                let index = inputs.len() - 1;
                maps.push(metadata_map(index, "s", "id", id)?);
                subtitle_input = Some(index);
            }
            SubtitleLocator::Hls { master, name, .. } => {
                // Subtitle timing needs master rendition context. Select its
                // unique NAME metadata; do not concatenate WebVTT ourselves.
                if info
                    .subtitle_tracks
                    .iter()
                    .filter(|other| matches!(&other.locator, SubtitleLocator::Hls { name: other, .. } if other == name))
                    .count()
                    != 1
                {
                    return Err(MediaError::InvalidSelection);
                }
                // Keep a dedicated subtitle input so normal A/V probing never
                // advances the subtitle start position or loses early cues.
                inputs.push(master.clone());
                let index = inputs.len() - 1;
                maps.push(metadata_map(index, "s", "comment", name)?);
                subtitle_input = Some(index);
            }
        }
        subtitle = Some(track.format);
        subtitle_language = track.language.clone();
    }
    Ok(Plan { inputs, maps, subtitle, subtitle_language, subtitle_input })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> MediaInfo {
        MediaInfo {
            kind: MediaKind::Hls,
            is_live: false,
            is_master: true,
            duration: None,
            variant_count: 2,
            uses_aes128: false,
            variants: [720, 1080]
                .into_iter()
                .map(|height| MediaVariant {
                    id: format!("v{height}"),
                    width: Some(height * 16 / 9),
                    height: Some(height),
                    bandwidth: Some(1000),
                    codecs: None,
                    audio_group: Some("main".into()),
                    subtitle_group: None,
                    locator: Locator::Hls(format!("https://example.org/{height}.m3u8")),
                })
                .collect(),
            audio_tracks: ["main", "other"]
                .into_iter()
                .map(|group| AudioTrack {
                    id: group.into(),
                    name: group.into(),
                    language: Some("en".into()),
                    bandwidth: None,
                    codecs: None,
                    is_default: true,
                    group: Some(group.into()),
                    locator: Locator::Hls(format!("https://example.org/{group}.m3u8")),
                })
                .collect(),
            subtitle_tracks: Vec::new(),
        }
    }

    #[test]
    fn selection_is_exact_bounded_and_audio_groups_are_compatible() {
        let info = info();
        let source = MediaSource::new("https://example.org/master.m3u8");
        let best = plan(&source, &info, &MediaSelection::default()).unwrap();
        assert_eq!(best.inputs, ["https://example.org/1080.m3u8", "https://example.org/main.m3u8"]);
        let capped = plan(
            &source,
            &info,
            &MediaSelection {
                video: VideoSelection::MaxHeight(800),
                audio: AudioSelection::None,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(capped.inputs, ["https://example.org/720.m3u8"]);
        assert_eq!(capped.maps, ["0:v:0"]);
        for video in [VideoSelection::MaxHeight(100), VideoSelection::Variant("stale".into())] {
            assert!(matches!(
                plan(&source, &info, &MediaSelection { video, audio: AudioSelection::Auto, ..Default::default() }),
                Err(MediaError::InvalidSelection)
            ));
        }
        assert!(matches!(
            plan(
                &source,
                &info,
                &MediaSelection {
                    video: VideoSelection::Best,
                    audio: AudioSelection::Track("other".into()),
                    ..Default::default()
                }
            ),
            Err(MediaError::InvalidSelection)
        ));
        assert_eq!(info.audio_tracks_for(&VideoSelection::Best).unwrap().len(), 1);
    }

    #[test]
    fn ambiguous_ids_and_specifier_characters_are_rejected_and_keys_hide_urls() {
        let mut info = info();
        info.variants.push(info.variants[1].clone());
        assert!(matches!(selected_video(&info, &VideoSelection::Best), Err(MediaError::InvalidSelection)));
        assert!(dash_map("v", "unsafe:specifier").is_err());
        assert!(dash_map("a", "optional?").is_err());
        assert_eq!(dash_map("v", "video-01").unwrap(), "0:v:m:id:video-01");
        assert_ne!(key("hls-v", "https://example.org/a?token=1"), key("hls-v", "https://example.org/a?token=2"));
        assert!(!format!("{:?}", Locator::Hls("https://example.org/?secret".into())).contains("secret"));
    }

    #[test]
    fn subtitles_default_off_and_saved_selection_roundtrips() {
        let old: MediaSelection = serde_json::from_str(r#"{"video":"Best","audio":"Auto"}"#).unwrap();
        assert_eq!(old.subtitle, SubtitleSelection::None);
        let selection = MediaSelection { subtitle: SubtitleSelection::Track("opaque-id".into()), ..old };
        assert_eq!(
            serde_json::from_str::<MediaSelection>(&serde_json::to_string(&selection).unwrap()).unwrap(),
            selection
        );
    }

    #[test]
    fn subtitles_require_compatible_unique_supported_renditions() {
        let mut info = info();
        for variant in &mut info.variants {
            variant.subtitle_group = Some("subs".into());
        }
        info.subtitle_tracks.push(SubtitleTrack {
            id: "captions".into(),
            name: "Chinese".into(),
            language: Some("zho".into()),
            codecs: Some("webvtt".into()),
            format: SubtitleFormat::WebVtt,
            group: Some("subs".into()),
            locator: SubtitleLocator::Hls { master: "https://example.org/master.m3u8".into(), name: "Chinese".into(), uri: "https://example.org/sub.m3u8".into() },
        });
        let source = MediaSource::new("https://example.org/master.m3u8");
        let selection = MediaSelection { subtitle: SubtitleSelection::Track("captions".into()), ..Default::default() };
        let planned = plan(&source, &info, &selection).unwrap();
        assert_eq!(planned.maps.last().unwrap(), "2:s:m:comment:Chinese");
        assert_eq!(planned.subtitle_input, Some(2));
        assert_eq!(planned.subtitle, Some(SubtitleFormat::WebVtt));
        info.is_live = true;
        assert_eq!(plan(&source, &info, &selection).unwrap().subtitle_input, Some(2));
        info.is_live = false;
        info.subtitle_tracks[0].group = Some("other".into());
        assert!(matches!(plan(&source, &info, &selection), Err(MediaError::InvalidSelection)));
        info.subtitle_tracks[0].group = Some("subs".into());
        info.subtitle_tracks[0].format = SubtitleFormat::Unsupported;
        assert!(matches!(plan(&source, &info, &selection), Err(MediaError::UnsupportedSubtitle)));
        info.subtitle_tracks[0].format = SubtitleFormat::WebVtt;
        let mut duplicate = info.subtitle_tracks[0].clone();
        duplicate.id = "different-uri-same-name".into();
        info.subtitle_tracks.push(duplicate);
        assert!(matches!(plan(&source, &info, &selection), Err(MediaError::InvalidSelection)));
        info.subtitle_tracks.clear();
        assert!(matches!(plan(&source, &info, &selection), Err(MediaError::InvalidSelection)));
        assert!(metadata_map(2, "s", "comment", "unsafe:name").is_err());
    }
}
