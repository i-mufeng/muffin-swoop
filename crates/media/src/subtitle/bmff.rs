//! Bounds-checked ISO BMFF text samples (wvtt / stpp). No codec dependency.
use crate::MediaError;

#[derive(Clone, Copy)]
pub(super) struct BoxRef<'a> { pub kind: [u8; 4], pub start: usize, pub data: &'a [u8] }
pub(super) fn boxes(bytes: &[u8]) -> Result<Vec<BoxRef<'_>>, MediaError> {
    let mut result = vec![];
    let mut offset = 0usize;
    while offset < bytes.len() {
        if result.len() >= 100_000 || bytes.len() - offset < 8 { return Err(MediaError::InvalidSubtitle); }
        let size = u32_at(bytes, offset)?;
        let header = if size == 1 { 16 } else { 8 };
        let size = match size {
            0 => bytes.len() - offset,
            1 => usize::try_from(u64_at(bytes, offset + 8)?).map_err(|_| MediaError::SubtitleLimit)?,
            value => value as usize,
        };
        if size < header || size > bytes.len() - offset { return Err(MediaError::InvalidSubtitle); }
        result.push(BoxRef { kind: bytes[offset + 4..offset + 8].try_into().unwrap(), start: offset, data: &bytes[offset + header..offset + size] });
        offset += size;
    }
    Ok(result)
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, MediaError> {
    Ok(u32::from_be_bytes(bytes.get(offset..offset + 4).ok_or(MediaError::InvalidSubtitle)?.try_into().unwrap()))
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, MediaError> {
    Ok(u64::from_be_bytes(bytes.get(offset..offset + 8).ok_or(MediaError::InvalidSubtitle)?.try_into().unwrap()))
}
fn child<'a>(bytes: &'a [u8], kind: &[u8; 4]) -> Result<BoxRef<'a>, MediaError> {
    let matches = boxes(bytes)?.into_iter().filter(|b| &b.kind == kind).collect::<Vec<_>>();
    if matches.len() != 1 { return Err(MediaError::InvalidSubtitle); }
    Ok(matches[0])
}
#[derive(Clone)]
pub(super) struct Track {
    id: u32,
    pub timescale: u32,
    pub codec: [u8; 4],
    pub language: Option<String>,
    duration: u32,
    size: u32,
}
pub(super) fn init(bytes: &[u8]) -> Result<Track, MediaError> {
    let moov = child(bytes, b"moov")?;
    let mut found = vec![];
    for trak in boxes(moov.data)?.into_iter().filter(|b| b.kind == *b"trak") {
        if boxes(trak.data)?.iter().any(|b| b.kind == *b"edts") { return Err(MediaError::UnsupportedSubtitle); }
        let mdia = child(trak.data, b"mdia")?;
        let mdhd = child(mdia.data, b"mdhd")?;
        let offset = match mdhd.data.first() { Some(0) => 12, Some(1) => 20, _ => return Err(MediaError::InvalidSubtitle) };
        let timescale = u32_at(mdhd.data, offset)?;
        if timescale == 0 { return Err(MediaError::InvalidSubtitle); }
        let lang_offset = if offset == 12 { 20 } else { 32 };
        let lang = u16::from_be_bytes(mdhd.data.get(lang_offset..lang_offset + 2).ok_or(MediaError::InvalidSubtitle)?.try_into().unwrap());
        let language = [((lang >> 10) & 31) as u8, ((lang >> 5) & 31) as u8, (lang & 31) as u8];
        let language = language.iter().all(|b| (1..=26).contains(b)).then(|| language.map(|c| (c + 0x60) as char).iter().collect());
        let stsd = child(child(child(mdia.data, b"minf")?.data, b"stbl")?.data, b"stsd")?;
        if u32_at(stsd.data, 4)? != 1 { return Err(MediaError::UnsupportedSubtitle); }
        let entries = boxes(stsd.data.get(8..).ok_or(MediaError::InvalidSubtitle)?)?;
        if entries.len() != 1 { return Err(MediaError::InvalidSubtitle); }
        let entry = entries[0];
        if !matches!(&entry.kind, b"wvtt" | b"stpp") { return Err(MediaError::UnsupportedSubtitle); }
        if entry.data.len() < 8 { return Err(MediaError::InvalidSubtitle); }
        if entry.kind == *b"wvtt" && boxes(&entry.data[8..])?.iter().any(|b| b.kind == *b"sinf") { return Err(MediaError::ProtectedMedia); }
        let tkhd = child(trak.data, b"tkhd")?;
        let id = u32_at(tkhd.data, match tkhd.data.first() { Some(0) => 12, Some(1) => 20, _ => return Err(MediaError::InvalidSubtitle) })?;
        let mut track = Track { id, timescale, codec: entry.kind, language, duration: 0, size: 0 };
        if let Ok(mvex) = child(moov.data, b"mvex") {
            for trex in boxes(mvex.data)?.into_iter().filter(|b| b.kind == *b"trex") {
                if u32_at(trex.data, 4)? == id { track.duration = u32_at(trex.data, 12)?; track.size = u32_at(trex.data, 16)?; }
            }
        }
        found.push(track);
    }
    if found.len() != 1 { return Err(MediaError::UnsupportedSubtitle); }
    Ok(found.remove(0))
}
pub(super) struct Sample<'a> { pub start: f64, pub end: f64, pub bytes: &'a [u8] }
pub(super) fn samples<'a>(bytes: &'a [u8], track: &Track, max: usize) -> Result<Vec<Sample<'a>>, MediaError> {
    let top = boxes(bytes)?;
    let mut samples = vec![];
    for moof in top.iter().filter(|b| b.kind == *b"moof") {
        for traf in boxes(moof.data)?.into_iter().filter(|b| b.kind == *b"traf") {
            let children = boxes(traf.data)?;
            if children.iter().any(|b| matches!(&b.kind, b"senc" | b"saiz" | b"saio" | b"subs")) { return Err(MediaError::UnsupportedSubtitle); }
            let tfhd = child(traf.data, b"tfhd")?;
            let flags = u32_at(tfhd.data, 0)? & 0x00ff_ffff;
            if u32_at(tfhd.data, 4)? != track.id { return Err(MediaError::UnsupportedSubtitle); }
            if flags & 0x010000 != 0 { continue; }
            let mut offset = 8;
            let base = if flags & 1 != 0 { offset += 8; usize::try_from(u64_at(tfhd.data, 8)?).map_err(|_| MediaError::SubtitleLimit)? }
                else { moof.start };
            if flags & 2 != 0 { if u32_at(tfhd.data, offset)? != 1 { return Err(MediaError::UnsupportedSubtitle); } offset += 4; }
            let duration = if flags & 8 != 0 { let v = u32_at(tfhd.data, offset)?; offset += 4; v } else { track.duration };
            let size = if flags & 16 != 0 { let v = u32_at(tfhd.data, offset)?; offset += 4; v } else { track.size };
            if flags & 32 != 0 { u32_at(tfhd.data, offset)?; }
            let tfdt = child(traf.data, b"tfdt")?;
            let mut decode = match tfdt.data.first() { Some(0) => u64::from(u32_at(tfdt.data, 4)?), Some(1) => u64_at(tfdt.data, 4)?, _ => return Err(MediaError::InvalidSubtitle) };
            let mut next_data = top.iter().find(|b| b.kind == *b"mdat" && b.start > moof.start).map(|b| b.data.as_ptr() as usize - bytes.as_ptr() as usize).ok_or(MediaError::InvalidSubtitle)?;
            for trun in children.iter().filter(|b| b.kind == *b"trun") {
                let version = *trun.data.first().ok_or(MediaError::InvalidSubtitle)?;
                if version > 1 { return Err(MediaError::InvalidSubtitle); }
                let flags = u32_at(trun.data, 0)? & 0x00ff_ffff;
                let count = u32_at(trun.data, 4)? as usize;
                if samples.len().saturating_add(count) > max { return Err(MediaError::SubtitleLimit); }
                let mut offset = 8;
                if flags & 1 != 0 {
                    next_data = usize::try_from(base as i128 + i128::from(u32_at(trun.data, offset)? as i32)).map_err(|_| MediaError::InvalidSubtitle)?;
                    offset += 4;
                }
                if flags & 4 != 0 { u32_at(trun.data, offset)?; offset += 4; }
                for _ in 0..count {
                    let duration = if flags & 0x100 != 0 { let v = u32_at(trun.data, offset)?; offset += 4; v } else { duration };
                    let size = if flags & 0x200 != 0 { let v = u32_at(trun.data, offset)?; offset += 4; v } else { size } as usize;
                    if flags & 0x400 != 0 { u32_at(trun.data, offset)?; offset += 4; }
                    let composition = if flags & 0x800 != 0 { let v = u32_at(trun.data, offset)?; offset += 4; if version == 1 { i64::from(v as i32) } else { i64::from(v) } } else { 0 };
                    let end = next_data.checked_add(size).ok_or(MediaError::SubtitleLimit)?;
                    if duration == 0 || !top.iter().filter(|b| b.kind == *b"mdat").any(|b| {
                        let begin = b.data.as_ptr() as usize - bytes.as_ptr() as usize;
                        next_data >= begin && end <= begin + b.data.len()
                    }) { return Err(MediaError::InvalidSubtitle); }
                    let pts = decode as i128 + i128::from(composition);
                    if pts < 0 { return Err(MediaError::UnsupportedSubtitle); }
                    let start = pts as f64 / f64::from(track.timescale);
                    let end_time = start + f64::from(duration) / f64::from(track.timescale);
                    samples.push(Sample { start, end: end_time, bytes: bytes.get(next_data..end).ok_or(MediaError::InvalidSubtitle)? });
                    next_data = end;
                    decode = decode.checked_add(u64::from(duration)).ok_or(MediaError::SubtitleLimit)?;
                }
                if offset != trun.data.len() { return Err(MediaError::InvalidSubtitle); }
            }
        }
    }
    if samples.is_empty() { return Err(MediaError::UnsupportedSubtitle); }
    Ok(samples)
}
pub(super) fn webvtt(sample: &Sample<'_>) -> Result<Vec<super::Cue>, MediaError> {
    let mut output = vec![];
    for cue in boxes(sample.bytes)? {
        match &cue.kind {
            b"vtte" => if !cue.data.is_empty() { return Err(MediaError::InvalidSubtitle); },
            b"vttc" => {
                let mut payload = None;
                for part in boxes(cue.data)? { match &part.kind {
                    b"payl" => { if payload.replace(std::str::from_utf8(part.data).map_err(|_| MediaError::InvalidSubtitle)?.to_owned()).is_some() { return Err(MediaError::InvalidSubtitle); } },
                    b"iden" | b"sttg" => { std::str::from_utf8(part.data).map_err(|_| MediaError::InvalidSubtitle)?; },
                    _ => return Err(MediaError::UnsupportedSubtitle),
                }}
                let payload = payload.ok_or(MediaError::InvalidSubtitle)?;
                if !payload.trim().is_empty() { output.push(super::Cue::seconds(sample.start, sample.end, payload)?); }
            },
            _ => return Err(MediaError::UnsupportedSubtitle),
        }
    }
    Ok(output)
}

/// Decode the first selected fMP4 input's presentation origin. Encrypted tracks
/// and edit lists require additional timeline handling and are rejected here.
pub(super) fn media_timestamp(init: &[u8], fragment: &[u8]) -> Result<f64, MediaError> {
    let moov = child(init, b"moov")?;
    let mut earliest: Option<f64> = None;
    for trak in boxes(moov.data)?.into_iter().filter(|b| b.kind == *b"trak") {
        if boxes(trak.data)?.iter().any(|b| b.kind == *b"edts") { return Err(MediaError::UnsupportedSubtitle); }
        let tkhd = child(trak.data, b"tkhd")?;
        let id = u32_at(tkhd.data, if tkhd.data.first() == Some(&1) { 20 } else { 12 })?;
        let mdhd = child(child(trak.data, b"mdia")?.data, b"mdhd")?;
        let timescale = u32_at(mdhd.data, if mdhd.data.first() == Some(&1) { 20 } else { 12 })?;
        if timescale == 0 { return Err(MediaError::InvalidSubtitle); }
        for moof in boxes(fragment)?.into_iter().filter(|b| b.kind == *b"moof") {
            for traf in boxes(moof.data)?.into_iter().filter(|b| b.kind == *b"traf") {
                if u32_at(child(traf.data, b"tfhd")?.data, 4)? != id { continue; }
                let tfdt = child(traf.data, b"tfdt")?;
                let decode = if tfdt.data.first() == Some(&1) { u64_at(tfdt.data, 4)? } else { u64::from(u32_at(tfdt.data, 4)?) };
                let trun = child(traf.data, b"trun")?;
                if u32_at(trun.data, 4)? == 0 { continue; }
                let flags = u32_at(trun.data, 0)? & 0x00ff_ffff;
                let offset = 8 + 4 * [1,4,0x100,0x200,0x400].iter().filter(|flag| flags & **flag != 0).count();
                let composition = if flags & 0x800 != 0 { let value = u32_at(trun.data, offset)?; if trun.data.first() == Some(&1) { i64::from(value as i32) } else { i64::from(value) } } else { 0 };
                let pts = decode as f64 + composition as f64;
                if pts < 0.0 { return Err(MediaError::UnsupportedSubtitle); }
                let timestamp = pts / f64::from(timescale);
                earliest = Some(earliest.map_or(timestamp, |previous| previous.min(timestamp)));
            }
        }
    }
    earliest.ok_or(MediaError::InvalidSubtitle)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_box_bounds_never_overread() {
        for bytes in [vec![0; 7], vec![0, 0, 0, 30, b'm', b'd', b'a', b't'], vec![0,0,0,1,b'm',b'o',b'o',b'f',255,255,255,255,255,255,255,255]] { assert!(boxes(&bytes).is_err()); }
    }
}
