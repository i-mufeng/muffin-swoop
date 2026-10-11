//! TTML/IMSC text transformation. Presentation styling is intentionally not
//! rendered; images, animation and non-media clock bases fail explicitly.
use super::{Cue, SubtitleConfig, xml::{self, Child, Node}};
use crate::MediaError;

#[derive(Clone, Copy)]
struct Clock { frames: f64, subframes: f64, ticks: f64 }
fn number(raw: &str) -> Result<f64, MediaError> {
    let value: f64 = raw.parse().map_err(|_| MediaError::InvalidSubtitle)?;
    if !value.is_finite() || value < 0.0 { return Err(MediaError::InvalidSubtitle); }
    Ok(value)
}
fn time(raw: &str, clock: Clock) -> Result<f64, MediaError> {
    let value = if raw.contains(':') {
        let fields = raw.split(':').collect::<Vec<_>>();
        if !(3..=4).contains(&fields.len()) { return Err(MediaError::InvalidSubtitle); }
        let hours = number(fields[0])?;
        let minutes = number(fields[1])?;
        let seconds = number(fields[2])?;
        if hours.fract() != 0.0 || minutes.fract() != 0.0 || minutes >= 60.0 || seconds >= 60.0 { return Err(MediaError::InvalidSubtitle); }
        let frames = if fields.len() == 4 {
            if seconds.fract() != 0.0 { return Err(MediaError::InvalidSubtitle); }
            let (frames, sub) = fields[3].split_once('.').unwrap_or((fields[3], "0"));
            (number(frames)? + number(sub)? / clock.subframes) / clock.frames
        } else { 0.0 };
        hours * 3600.0 + minutes * 60.0 + seconds + frames
    } else {
        let (value, multiplier) = [ ("ms", 0.001), ("h", 3600.0), ("m", 60.0), ("s", 1.0), ("f", 1.0 / clock.frames), ("t", 1.0 / clock.ticks) ]
            .into_iter().find_map(|(unit, multiplier)| raw.strip_suffix(unit).map(|value| (value, multiplier))).ok_or(MediaError::InvalidSubtitle)?;
        number(value)? * multiplier
    };
    if !value.is_finite() || value > 3_155_760_000.0 { return Err(MediaError::SubtitleLimit); }
    Ok(value)
}
#[derive(Clone, Copy)]
struct Timing { start: f64, end: f64 }
fn timing(node: &Node, parent: Timing, base: f64, clock: Clock) -> Result<Timing, MediaError> {
    let start = base + node.attr("begin").map(|v| time(v, clock)).transpose()?.unwrap_or(0.0);
    let mut end = node.attr("end").map(|v| time(v, clock).map(|t| base + t)).transpose()?.unwrap_or(parent.end);
    if let Some(duration) = node.attr("dur") { end = end.min(start + time(duration, clock)?); }
    Ok(Timing { start: start.max(parent.start), end: end.min(parent.end) })
}
#[derive(Clone)]
struct TextRun { timing: Timing, text: String, preserve: bool }
fn runs(node: &Node, interval: Timing, clock: Clock, preserve: bool, output: &mut Vec<TextRun>) -> Result<(), MediaError> {
    let preserve = match node.attr("xml:space") { Some("preserve") => true, Some("default") => false, Some(_) => return Err(MediaError::InvalidSubtitle), None => preserve };
    let sequential = node.attr("timeContainer") == Some("seq");
    let mut previous_end = interval.start;
    for child in &node.children {
        match child {
            Child::Text(text) => output.push(TextRun { timing: interval, text: text.clone(), preserve }),
            Child::Node(child) => {
                let child_time = timing(child, interval, if sequential { previous_end } else { interval.start }, clock)?;
                previous_end = child_time.end;
                match child.local() {
                    "span" => runs(child, child_time, clock, preserve, output)?,
                    "br" => output.push(TextRun { timing: child_time, text: "\n".into(), preserve: true }),
                    "metadata" => {},
                    _ => return Err(MediaError::UnsupportedSubtitle),
                }
            }
        }
    }
    Ok(())
}
fn text_at(runs: &[TextRun], instant: f64) -> String {
    let mut output = String::new();
    let mut whitespace = false;
    for run in runs.iter().filter(|run| run.timing.start <= instant && instant < run.timing.end) {
        for character in run.text.chars() {
            if !run.preserve && character.is_whitespace() { whitespace = true; }
            else {
                if whitespace && !output.is_empty() && !output.ends_with('\n') && character != '\n' { output.push(' '); }
                whitespace = false;
                output.push(character);
            }
        }
    }
    output.trim_matches('\n').to_owned()
}
fn walk(node: &Node, interval: Timing, clock: Clock, preserve: bool, output: &mut Vec<Cue>, config: &SubtitleConfig) -> Result<(), MediaError> {
    if interval.end <= interval.start { return Ok(()); }
    if node.local() == "p" {
        if !interval.end.is_finite() { return Err(MediaError::UnsupportedSubtitle); }
        let mut text_runs = vec![];
        runs(node, interval, clock, preserve, &mut text_runs)?;
        let mut boundaries = vec![interval.start, interval.end];
        for run in &text_runs { boundaries.extend([run.timing.start.max(interval.start), run.timing.end.min(interval.end)]); }
        boundaries.sort_by(f64::total_cmp);
        boundaries.dedup();
        for pair in boundaries.windows(2).filter(|pair| pair[1] > pair[0]) {
            let text = text_at(&text_runs, (pair[0] + pair[1]) / 2.0);
            if text.trim().is_empty() { continue; }
            if output.len() >= config.max_cues { return Err(MediaError::SubtitleLimit); }
            let cue = Cue::seconds(pair[0], pair[1], text)?;
            if let Some(previous) = output.last_mut().filter(|previous| previous.end_ms == cue.start_ms && previous.text == cue.text) { previous.end_ms = cue.end_ms; }
            else { output.push(cue); }
        }
        return Ok(());
    }
    let preserve = node.attr("xml:space").map_or(preserve, |value| value == "preserve");
    let sequential = node.attr("timeContainer") == Some("seq");
    let mut previous_end = interval.start;
    for child in node.nodes().filter(|child| !matches!(child.local(), "head" | "metadata")) {
        if !matches!(child.local(), "body" | "div" | "p") { return Err(MediaError::UnsupportedSubtitle); }
        let child_time = timing(child, interval, if sequential { previous_end } else { interval.start }, clock)?;
        previous_end = child_time.end;
        walk(child, child_time, clock, preserve, output, config)?;
    }
    Ok(())
}
fn validate(node: &Node) -> Result<(), MediaError> {
    if matches!(node.local(), "image" | "audio" | "data" | "set" | "animate" | "animation" | "ruby" | "source") { return Err(MediaError::UnsupportedSubtitle); }
    for (name, value) in &node.attrs {
        let local = name.rsplit(':').next().unwrap();
        if matches!(local, "backgroundImage" | "src" | "href" | "base" | "repeatCount" | "repeatDur" | "condition" | "ruby")
            || (local == "timeBase" && value != "media")
            || (local == "dropMode" && value != "nonDrop")
            || (local == "timeContainer" && !matches!(value.as_str(), "par" | "seq"))
            || (matches!(local, "profile" | "contentProfiles") && value.contains("/image"))
            || (local == "display" && value == "none") || (local == "visibility" && value == "hidden")
        { return Err(MediaError::UnsupportedSubtitle); }
    }
    for child in node.nodes() { validate(child)?; }
    Ok(())
}
pub(super) fn parse(bytes: &[u8], config: &SubtitleConfig, extent: Option<f64>) -> Result<(Vec<Cue>, Option<String>), MediaError> {
    let root = xml::parse(std::str::from_utf8(bytes).map_err(|_| MediaError::InvalidSubtitle)?.trim_start_matches('\u{feff}'))?;
    if root.local() != "tt" { return Err(MediaError::InvalidSubtitle); }
    validate(&root)?;
    let parameter = |name: &str| root.attrs.iter().find(|(key, _)| key.rsplit(':').next() == Some(name)).map(|(_, value)| value.as_str());
    let frame_rate = parameter("frameRate").map(number).transpose()?.unwrap_or(30.0);
    let multiplier = if let Some(raw) = parameter("frameRateMultiplier") {
        let parts = raw.split_whitespace().collect::<Vec<_>>();
        if parts.len() != 2 { return Err(MediaError::InvalidSubtitle); }
        number(parts[0])? / number(parts[1])?
    } else { 1.0 };
    let subframes = parameter("subFrameRate").map(number).transpose()?.unwrap_or(1.0);
    let frames = frame_rate * multiplier;
    let ticks = parameter("tickRate").map(number).transpose()?.unwrap_or(if parameter("frameRate").is_some() { frames * subframes } else { 1.0 });
    if [frames, subframes, ticks].iter().any(|value| !value.is_finite() || *value <= 0.0 || *value > 1_000_000_000.0) { return Err(MediaError::InvalidSubtitle); }
    let clock = Clock { frames, subframes, ticks };
    let interval = timing(&root, Timing { start: 0.0, end: extent.unwrap_or(f64::INFINITY) }, 0.0, clock)?;
    let mut cues = vec![];
    walk(&root, interval, clock, false, &mut cues, config)?;
    Ok((cues, root.attr("xml:lang").map(str::to_owned)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inherited_timing_frame_tick_and_timed_spans() {
        let raw = br#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttp="http://www.w3.org/ns/ttml#parameter" xml:lang="en" ttp:frameRate="25" ttp:tickRate="100"><body begin="1s" end="5s"><div><p begin="00:00:00:05" dur="100t">A<span begin="200ms"> B</span><br/>C &amp; D</p></div></body></tt>"#;
        let (cues, language) = parse(raw, &SubtitleConfig::default(), None).unwrap();
        assert_eq!(language.as_deref(), Some("en"));
        assert_eq!((cues[0].start_ms, cues[0].end_ms, cues[0].text.as_str()), (1200, 1400, "A\nC & D"));
        assert_eq!((cues[1].start_ms, cues[1].end_ms, cues[1].text.as_str()), (1400, 2200, "A B\nC & D"));
    }
    #[test]
    fn images_entities_unbounded_and_invalid_time_fail() {
        for raw in ["<!DOCTYPE tt [<!ENTITY x SYSTEM 'file:///secret'>]><tt/>", "<tt><body><div><image src='x'/></div></body></tt>", "<tt><body><p>forever</p></body></tt>", "<tt><body><p begin='NaNs' end='3s'>bad</p></body></tt>"] {
            assert!(parse(raw.as_bytes(), &SubtitleConfig::default(), None).is_err());
        }
    }
}
