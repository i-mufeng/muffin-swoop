//! Opt-in external tests: cargo test -p swoop-media --test real_media -- --ignored
//! Requires real FFmpeg and FFprobe; failures never become silent skips.
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use swoop_media::{
    AudioSelection, MediaConfig, MediaDownloader, MediaError, MediaEvent, MediaKind, MediaRequest, MediaSelection,
    MediaSource, SubtitleSelection, VideoSelection,
};
use tokio_util::sync::CancellationToken;

type Requests = Arc<Mutex<Vec<(String, Vec<String>)>>>;
type Replacement = Arc<Mutex<Option<(String, String)>>>;

struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    delay: Arc<AtomicUsize>,
    segments: Arc<AtomicUsize>,
    requests: Requests,
    replacement: Replacement,
    authorization: Arc<Mutex<Option<String>>>,
}
impl Server {
    fn start(root: PathBuf) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let delay = Arc::new(AtomicUsize::new(0));
        let segments = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let replacement = Arc::new(Mutex::new(None));
        let authorization = Arc::new(Mutex::new(None));
        let server = Self {
            url,
            stop: stop.clone(),
            delay: delay.clone(),
            segments: segments.clone(),
            requests: requests.clone(),
            replacement: replacement.clone(),
            authorization: authorization.clone(),
        };
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (root, delay, segments, requests, replacement, authorization) = (
                            root.clone(),
                            delay.clone(),
                            segments.clone(),
                            requests.clone(),
                            replacement.clone(),
                            authorization.clone(),
                        );
                        std::thread::spawn(move || {
                            handle(stream, &root, &delay, &segments, &requests, &replacement, &authorization)
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        server
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn handle(
    mut stream: TcpStream,
    root: &Path,
    delay: &AtomicUsize,
    segments: &AtomicUsize,
    requests: &Requests,
    replacement: &Replacement,
    authorization: &Mutex<Option<String>>,
) {
    // BSD/macOS accept() inherits the listener's O_NONBLOCK. BufReader is a
    // blocking HTTP reader: otherwise a client that writes after connect (or
    // splits headers across packets) gets WouldBlock and an early close/reset.
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut first = String::new();
    if reader.read_line(&mut first).unwrap_or(0) == 0 {
        return;
    }
    let target = first.split_whitespace().nth(1).unwrap_or("/");
    // Also acts as an explicit HTTP proxy in gateway tests: media.invalid has
    // no direct origin, so successful media can only have used this listener.
    let path = url::Url::parse(target)
        .map(|url| url.path().to_owned())
        .unwrap_or_else(|_| target.split('?').next().unwrap().to_owned());
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        headers.push(line.trim().to_owned());
    }
    let lower: Vec<_> = headers.iter().map(|line| line.to_ascii_lowercase()).collect();
    let count = {
        let mut requests = requests.lock().unwrap();
        requests.push((path.clone(), headers));
        requests.iter().filter(|(p, _)| p == &path).count()
    };
    if !lower.iter().any(|line| line == "user-agent: swoopmediatest/1.0") {
        let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    if authorization.lock().unwrap().as_ref().is_some_and(|expected| !headers_authorized(&lower, expected)) {
        let _ = stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    if path.contains("..") {
        return;
    }
    let override_body = replacement
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(p, _)| p == &path && count > 1)
        .map(|(_, body)| body.as_bytes().to_vec());
    let body = override_body.or_else(|| fs::read(root.join(path.trim_start_matches('/'))).ok());
    let Some(body) = body else {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    };
    let etag = format!("\"{:x}\"", Sha256::digest(&body));
    if lower.iter().any(|line| line == &format!("if-none-match: {etag}")) {
        let _ = stream
            .write_all(format!("HTTP/1.1 304 Not Modified\r\nETag: {etag}\r\nConnection: close\r\n\r\n").as_bytes());
        return;
    }
    if path.ends_with(".ts") || path.ends_with(".m4s") {
        segments.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(delay.load(Ordering::SeqCst) as u64));
    }
    let range =
        lower.iter().find_map(|line| line.strip_prefix("range: bytes=")).and_then(|raw| raw.split_once('-')).and_then(
            |(start, end)| {
                let start = start.parse::<usize>().ok()?;
                let end = if end.is_empty() {
                    body.len().checked_sub(1)?
                } else {
                    end.parse::<usize>().ok()?.min(body.len().checked_sub(1)?)
                };
                (start <= end && start < body.len()).then_some((start, end))
            },
        );
    let (status, start, end) = match range {
        Some((start, end)) => ("206 Partial Content", start, end + 1),
        None => ("200 OK", 0, body.len()),
    };
    let content_type = if path.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else if path.ends_with(".mpd") {
        "application/dash+xml"
    } else {
        "application/octet-stream"
    };
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\nETag: {etag}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n",
        end - start
    );
    if range.is_some() {
        response.push_str(&format!("Content-Range: bytes {start}-{}/{total}\r\n", end - 1, total = body.len()));
    }
    response.push_str("\r\n");
    let _ = stream.write_all(response.as_bytes()).and_then(|_| stream.write_all(&body[start..end]));
}

fn headers_authorized(headers: &[String], expected: &str) -> bool {
    headers.iter().any(|line| line == &format!("authorization: {}", expected.to_ascii_lowercase()))
}

fn tool(name: &str) -> String {
    std::env::var(format!("{}_BINARY", name.to_ascii_uppercase())).unwrap_or_else(|_| name.into())
}

#[test]
fn fixture_accepts_delayed_and_split_request_headers() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("probe.bin"), b"fixture is alive").unwrap();
    let server = Server::start(dir.path().into());
    let mut socket = TcpStream::connect(server.url.trim_start_matches("http://")).unwrap();
    socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    // On macOS/BSD accepted sockets inherit O_NONBLOCK from the listener.
    // A client is allowed to complete connect() before writing any HTTP bytes.
    std::thread::sleep(Duration::from_millis(60));
    socket.write_all(b"GET /probe.bin HTTP/1.1\r\n").expect("server must keep an idle accepted socket open");
    std::thread::sleep(Duration::from_millis(60));
    socket
        .write_all(b"Host: 127.0.0.1\r\nUser-Agent: SwoopMediaTest/1.0\r\n\r\n")
        .expect("server must wait for complete request headers");
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.ends_with("fixture is alive"));
}
fn ffmpeg(args: &[&str], root: &Path) {
    let output = Command::new(std::env::var("FFMPEG_FIXTURE_BINARY").unwrap_or_else(|_| "ffmpeg".into()))
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .current_dir(root)
        .output()
        .expect("real FFmpeg must be installed for this opt-in test");
    assert!(output.status.success(), "fixture FFmpeg failed: {}", String::from_utf8_lossy(&output.stderr));
}

struct Fixture {
    dir: tempfile::TempDir,
    server: Server,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=12",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=1000:sample_rate=44100",
                "-t",
                "3",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
                "-g",
                "12",
                "-c:a",
                "aac",
                "source.mp4",
            ],
            dir.path(),
        );
        ffmpeg(
            &[
                "-i",
                "source.mp4",
                "-c",
                "copy",
                "-hls_time",
                "1",
                "-hls_list_size",
                "0",
                "-hls_segment_filename",
                "clear-%03d.ts",
                "clear.m3u8",
            ],
            dir.path(),
        );
        ffmpeg(&["-i", "source.mp4", "-c", "copy", "-f", "dash", "-seg_duration", "1", "stream.mpd"], dir.path());
        fs::write(dir.path().join("key.bin"), [7u8; 16]).unwrap();
        fs::write(dir.path().join("key-info.txt"), format!("key.bin\n{}\n", dir.path().join("key.bin").display()))
            .unwrap();
        ffmpeg(
            &[
                "-i",
                "source.mp4",
                "-c",
                "copy",
                "-hls_time",
                "1",
                "-hls_list_size",
                "0",
                "-hls_key_info_file",
                "key-info.txt",
                "-hls_segment_filename",
                "encrypted-%03d.ts",
                "encrypted.m3u8",
            ],
            dir.path(),
        );
        fs::write(dir.path().join("master.m3u8"), "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=400000\nclear.m3u8\n").unwrap();
        let server = Server::start(dir.path().to_owned());
        Self { dir, server }
    }
    fn request(&self, manifest: &str, output: &str) -> MediaRequest {
        let mut source = MediaSource::new(format!("{}/{manifest}?token=do-not-log", self.server.url));
        source.headers = vec![
            ("User-Agent".into(), "SwoopMediaTest/1.0".into()),
            ("Referer".into(), "https://example.org/private/page?token=secret".into()),
        ];
        MediaRequest {
            source,
            destination: self.dir.path().join(output),
            duration_limit: None,
            selection: Default::default(),
            overwrite: false,
        }
    }
    fn downloader(&self) -> MediaDownloader {
        MediaDownloader::new(MediaConfig {
            ffmpeg_path: tool("ffmpeg").into(),
            max_concurrent: 1,
            ..Default::default()
        })
        .unwrap()
    }
    fn add_quality_tracks(&self) {
        ffmpeg(
            &[
                "-i",
                "source.mp4",
                "-an",
                "-vf",
                "scale=320:180",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-g",
                "12",
                "high.mp4",
            ],
            self.dir.path(),
        );
        ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=500:sample_rate=48000",
                "-t",
                "3",
                "-ac",
                "2",
                "-c:a",
                "aac",
                "stereo.m4a",
            ],
            self.dir.path(),
        );
        for (input, selector, output, segments) in [
            ("source.mp4", "0:v", "low.m3u8", "low-%03d.m4s"),
            ("high.mp4", "0:v", "high.m3u8", "high-%03d.m4s"),
            ("source.mp4", "0:a", "english.m3u8", "en-%03d.m4s"),
            ("stereo.m4a", "0:a", "chinese.m3u8", "zh-%03d.m4s"),
        ] {
            ffmpeg(
                &[
                    "-i",
                    input,
                    "-map",
                    selector,
                    "-c",
                    "copy",
                    "-hls_segment_type",
                    "fmp4",
                    "-hls_fmp4_init_filename",
                    &format!("{output}-init.mp4"),
                    "-hls_time",
                    "1",
                    "-hls_list_size",
                    "0",
                    "-hls_segment_filename",
                    segments,
                    output,
                ],
                self.dir.path(),
            );
        }
        fs::write(self.dir.path().join("qualities.m3u8"), concat!(
            "#EXTM3U\n",
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"English\",LANGUAGE=\"en\",DEFAULT=YES,AUTOSELECT=YES,URI=\"english.m3u8\"\n",
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"Chinese\",LANGUAGE=\"zh\",DEFAULT=NO,AUTOSELECT=YES,URI=\"chinese.m3u8\"\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=200000,RESOLUTION=160x90,CODECS=\"avc1.42c00b,mp4a.40.2\",AUDIO=\"audio\"\nlow.m3u8\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=500000,RESOLUTION=320x180,CODECS=\"avc1.42c00d,mp4a.40.2\",AUDIO=\"audio\"\nhigh.m3u8\n"
        )).unwrap();
        ffmpeg(
            &[
                "-i",
                "source.mp4",
                "-i",
                "high.mp4",
                "-i",
                "stereo.m4a",
                "-map",
                "0:v",
                "-map",
                "1:v",
                "-map",
                "0:a",
                "-map",
                "2:a",
                "-c",
                "copy",
                "-metadata:s:a:0",
                "language=en",
                "-metadata:s:a:1",
                "language=zh",
                "-f",
                "dash",
                "-seg_duration",
                "1",
                "-adaptation_sets",
                "id=0,streams=0,1 id=1,streams=2 id=2,streams=3",
                "qualities.mpd",
            ],
            self.dir.path(),
        );
    }
    fn assert_clean(&self) {
        let leftovers: Vec<_> = fs::read_dir(self.dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".swoop-media-"))
            .collect();
        assert!(leftovers.is_empty(), "temporary files remain: {leftovers:?}");
    }
    fn add_subtitles(&self) {
        self.add_quality_tracks();
        for (language, first, second) in
            [("eng", "English first", "English second"), ("zho", "中文字幕第一句", "中文字幕第二句")]
        {
            fs::write(
                self.dir.path().join(format!("{language}.vtt")),
                format!(
                    "WEBVTT\n\n00:00:00.200 --> 00:00:01.200\n{first}\n\n00:00:01.500 --> 00:00:02.800\n{second}\n"
                ),
            )
            .unwrap();
            for (index, start, end, text) in
                [(0, "00:00:00.200", "00:00:01.200", first), (1, "00:00:01.500", "00:00:02.800", second)]
            {
                fs::write(
                    self.dir.path().join(format!("{language}-{index}.vtt")),
                    format!("WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:0\n\n{start} --> {end}\n{text}\n"),
                )
                .unwrap();
            }
            fs::write(self.dir.path().join(format!("{language}-sub.m3u8")), format!(
                "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-VERSION:3\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1.5,\n{language}-0.vtt\n#EXTINF:1.5,\n{language}-1.vtt\n#EXT-X-ENDLIST\n"
            )).unwrap();
            ffmpeg(
                &["-i", &format!("{language}.vtt"), "-c:s", "mov_text", &format!("{language}-captions.mp4")],
                self.dir.path(),
            );
        }
        let master = fs::read_to_string(self.dir.path().join("qualities.m3u8")).unwrap();
        let master = master.replace("#EXTM3U\n", concat!(
            "#EXTM3U\n",
            "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"English captions\",LANGUAGE=\"eng\",DEFAULT=NO,AUTOSELECT=YES,URI=\"eng-sub.m3u8\"\n",
            "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"Chinese captions\",LANGUAGE=\"zho\",DEFAULT=NO,AUTOSELECT=YES,URI=\"zho-sub.m3u8\"\n"
        )).replace("AUDIO=\"audio\"\n", "AUDIO=\"audio\",SUBTITLES=\"subs\"\n");
        fs::write(self.dir.path().join("subtitles.m3u8"), master).unwrap();
        for language in ["eng", "zho"] {
            let playlist = fs::read_to_string(self.dir.path().join(format!("{language}-sub.m3u8"))).unwrap();
            fs::write(self.dir.path().join(format!("{language}-ts-sub.m3u8")), playlist.replace(".vtt", "-ts.vtt"))
                .unwrap();
            for index in 0..2 {
                let vtt = fs::read_to_string(self.dir.path().join(format!("{language}-{index}.vtt"))).unwrap();
                // FFmpeg's default MPEG-TS muxer starts the timeline at 1.4s.
                fs::write(
                    self.dir.path().join(format!("{language}-{index}-ts.vtt")),
                    vtt.replace("MPEGTS:0", "MPEGTS:126000"),
                )
                .unwrap();
            }
        }
        fs::write(self.dir.path().join("subtitles-ts.m3u8"), concat!(
            "#EXTM3U\n",
            "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"English captions\",LANGUAGE=\"eng\",URI=\"eng-ts-sub.m3u8\"\n",
            "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"Chinese captions\",LANGUAGE=\"zho\",URI=\"zho-ts-sub.m3u8\"\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=400000,RESOLUTION=160x90,SUBTITLES=\"subs\"\nclear.m3u8\n"
        )).unwrap();
        let mpd = fs::read_to_string(self.dir.path().join("qualities.mpd")).unwrap();
        let mpd = mpd.replace("</Period>", concat!(
            "<AdaptationSet id=\"sub-en\" contentType=\"text\" mimeType=\"text/vtt\" lang=\"eng\"><Representation id=\"caption-en\" bandwidth=\"256\"><BaseURL>eng.vtt</BaseURL></Representation></AdaptationSet>",
            "<AdaptationSet id=\"sub-zh\" contentType=\"text\" mimeType=\"text/vtt\" lang=\"zho\"><Representation id=\"caption-zh\" bandwidth=\"256\"><BaseURL>zho.vtt</BaseURL></Representation></AdaptationSet>",
            "</Period>"
        ));
        fs::write(self.dir.path().join("subtitles.mpd"), &mpd).unwrap();
        let mov_text = mpd
            .replace("mimeType=\"text/vtt\"", "mimeType=\"application/mp4\" codecs=\"tx3g\"")
            .replace("eng.vtt", "eng-captions.mp4")
            .replace("zho.vtt", "zho-captions.mp4");
        fs::write(self.dir.path().join("mov-text.mpd"), mov_text).unwrap();
    }
}

fn probe_subtitles(path: &Path, codec: Option<&str>) {
    let output = Command::new(tool("ffprobe"))
        .args(["-v", "error", "-select_streams", "s", "-show_streams", "-show_packets", "-of", "json"])
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success(), "FFprobe failed");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let streams = json["streams"].as_array().unwrap();
    let Some(codec) = codec else {
        assert!(streams.is_empty(), "subtitles must default off: {json}");
        return;
    };
    assert_eq!(streams.len(), 1, "{json}");
    assert_eq!(streams[0]["codec_name"], codec, "{json}");
    assert_eq!(streams[0]["tags"]["language"], "zho", "{json}");
    let packets = json["packets"].as_array().unwrap();
    for (start, duration) in [(0.2, 1.0), (1.5, 1.3)] {
        assert!(
            packets.iter().any(|packet| {
                let actual_start = packet["pts_time"].as_str().and_then(|s| s.parse::<f64>().ok());
                let actual_duration = packet["duration_time"].as_str().and_then(|s| s.parse::<f64>().ok());
                actual_start.is_some_and(|n| (n - start).abs() < 0.03)
                    && actual_duration.is_some_and(|n| (n - duration).abs() < 0.03)
            }),
            "subtitle time shifted or truncated: {json}"
        );
    }
    let output = Command::new(tool("ffmpeg"))
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:s:0", "-f", "srt", "pipe:1"])
        .output()
        .unwrap();
    assert!(output.status.success(), "subtitle extraction failed");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("中文字幕第一句") && text.contains("中文字幕第二句"), "{text}");
    assert!(!text.contains("English"), "wrong language selected: {text}");
    assert_eq!(text.matches(" --> ").count(), 2, "repeated subtitle cues: {text}");
}

#[tokio::test]
#[ignore = "requires real HLS/DASH FFmpeg with WebVTT/mov_text/SubRip subtitle codecs"]
async fn selected_hls_and_dash_subtitles_match_language_timing_and_container() {
    let fixture = Fixture::new();
    fixture.add_subtitles();
    let downloader = fixture.downloader();
    for manifest in ["subtitles.m3u8", "subtitles-ts.m3u8", "subtitles.mpd", "mov-text.mpd"] {
        let info = downloader
            .inspect(&fixture.request(manifest, "unused.mp4").source, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(info.subtitle_tracks.len(), 2, "{info:?}");
        let chinese = info.subtitle_tracks.iter().find(|track| track.language.as_deref() == Some("zho")).unwrap();
        for (extension, codec) in [("mp4", "mov_text"), ("mkv", "subrip")] {
            let mut request = fixture.request(manifest, &format!("captions-{manifest}.{extension}"));
            request.selection.audio = AudioSelection::PreferredLanguage("en".into());
            request.selection.subtitle = SubtitleSelection::Track(chinese.id.clone());
            let output = request.destination.clone();
            let result = downloader.download(request, CancellationToken::new(), |_| {}).await;
            assert!(result.is_ok(), "{manifest}/{extension}: {result:?}");
            probe_selected(&output, if manifest == "subtitles-ts.m3u8" { 90 } else { 180 }, Some((1, "44100")));
            probe_subtitles(&output, Some(codec));
        }
        let request = fixture.request(manifest, &format!("no-captions-{manifest}.mp4"));
        let output = request.destination.clone();
        let result = downloader.download(request, CancellationToken::new(), |_| {}).await;
        assert!(result.is_ok(), "default off {manifest}: {result:?}");
        probe_subtitles(&output, None);
    }
    fixture.assert_clean();
}

#[tokio::test]
#[ignore = "requires real subtitle-capable FFmpeg; validates failure publication boundaries"]
async fn unavailable_empty_and_missing_subtitles_never_complete() {
    let fixture = Fixture::new();
    fixture.add_subtitles();
    let downloader = fixture.downloader();
    for manifest in ["subtitles.m3u8", "subtitles.mpd"] {
        let mut request = fixture.request(manifest, &format!("stale-subtitle-{manifest}.mp4"));
        request.selection.subtitle = SubtitleSelection::Track("removed-subtitle".into());
        let destination = request.destination.clone();
        assert!(matches!(
            downloader.download(request, CancellationToken::new(), |_| {}).await,
            Err(MediaError::InvalidSelection)
        ));
        assert!(!destination.exists());
    }
    // Keep the first cue valid: a missing second segment must not publish a
    // movie that silently contains only the opening subtitle.
    let playlist = fs::read_to_string(fixture.dir.path().join("zho-sub.m3u8")).unwrap();
    fs::write(fixture.dir.path().join("zho-sub.m3u8"), playlist.replace("zho-1.vtt", "missing-caption.vtt")).unwrap();
    let mut request = fixture.request("subtitles.m3u8", "missing-captions.mp4");
    let info = downloader.inspect(&request.source, &CancellationToken::new()).await.unwrap();
    let chinese = info.subtitle_tracks.iter().find(|track| track.language.as_deref() == Some("zho")).unwrap();
    request.selection.subtitle = SubtitleSelection::Track(chinese.id.clone());
    let destination = request.destination.clone();
    let mut completed = false;
    let result = downloader
        .download(request, CancellationToken::new(), |event| completed |= matches!(event, MediaEvent::Completed { .. }))
        .await;
    assert!(matches!(result, Err(MediaError::FfmpegFailed(_)) | Err(MediaError::FfmpegDiagnostic)), "{result:?}");
    assert!(!completed && !destination.exists());
    fs::write(fixture.dir.path().join("zho-sub.m3u8"), playlist).unwrap();
    for file in ["zho-0.vtt", "zho-1.vtt", "zho.vtt"] {
        fs::write(fixture.dir.path().join(file), "WEBVTT\n\n").unwrap();
    }
    for manifest in ["subtitles.m3u8", "subtitles.mpd"] {
        let mut request = fixture.request(manifest, &format!("empty-captions-{manifest}.mp4"));
        let info = downloader.inspect(&request.source, &CancellationToken::new()).await.unwrap();
        let chinese = info.subtitle_tracks.iter().find(|track| track.language.as_deref() == Some("zho")).unwrap();
        request.selection.subtitle = SubtitleSelection::Track(chinese.id.clone());
        let destination = request.destination.clone();
        let mut completed = false;
        let result = downloader
            .download(request, CancellationToken::new(), |event| {
                completed |= matches!(event, MediaEvent::Completed { .. })
            })
            .await;
        assert!(
            matches!(result, Err(MediaError::FfmpegFailed(_)) | Err(MediaError::FfmpegDiagnostic)),
            "{manifest}: {result:?}"
        );
        assert!(!completed && !destination.exists());
    }
    fixture.assert_clean();
}

fn probe_selected(path: &Path, expected_height: u64, audio: Option<(u64, &str)>) {
    let output = Command::new(tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,width,height,channels,sample_rate:format=duration",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success(), "FFprobe failed");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let duration = json["format"]["duration"].as_str().unwrap().parse::<f64>().unwrap();
    assert!((2.5..3.5).contains(&duration), "{json}");
    let streams = json["streams"].as_array().unwrap();
    let video = streams.iter().filter(|stream| stream["codec_type"] == "video").collect::<Vec<_>>();
    assert_eq!(video.len(), 1, "{json}");
    assert_eq!(video[0]["height"], expected_height, "{json}");
    let actual_audio = streams.iter().filter(|stream| stream["codec_type"] == "audio").collect::<Vec<_>>();
    if let Some((channels, rate)) = audio {
        assert_eq!(actual_audio.len(), 1, "{json}");
        assert_eq!(actual_audio[0]["channels"], channels, "{json}");
        assert_eq!(actual_audio[0]["sample_rate"], rate, "{json}");
    } else {
        assert!(actual_audio.is_empty(), "{json}");
    }
}

#[tokio::test]
#[ignore = "requires real FFmpeg and FFprobe; validates selected output streams"]
async fn selected_hls_and_dash_quality_and_audio_match_real_output() {
    let fixture = Fixture::new();
    fixture.add_quality_tracks();
    let downloader = fixture.downloader();
    for manifest in ["qualities.m3u8", "qualities.mpd"] {
        let source = fixture.request(manifest, "unused.mp4").source;
        let info = downloader.inspect(&source, &CancellationToken::new()).await.unwrap();
        assert_eq!(info.variants.len(), 2, "{info:?}");
        assert_eq!(info.audio_tracks.len(), 2, "{info:?}");
        assert!(info.variants.iter().all(|v| v.codecs.is_some() && v.bandwidth.is_some()));
        let low = info.variants.iter().find(|variant| variant.height == Some(90)).unwrap();
        let chinese = info.audio_tracks.iter().find(|track| track.language.as_deref() == Some("zh")).unwrap();
        for (index, selection, height, audio) in [
            (
                0,
                MediaSelection {
                    video: VideoSelection::Variant(low.id.clone()),
                    audio: AudioSelection::Track(chinese.id.clone()),
                    ..Default::default()
                },
                90,
                Some((2, "48000")),
            ),
            (
                1,
                MediaSelection {
                    video: VideoSelection::Best,
                    audio: AudioSelection::PreferredLanguage("en".into()),
                    ..Default::default()
                },
                180,
                Some((1, "44100")),
            ),
            (
                2,
                MediaSelection {
                    video: VideoSelection::MaxHeight(100),
                    audio: AudioSelection::None,
                    ..Default::default()
                },
                90,
                None,
            ),
        ] {
            let mut request = fixture.request(manifest, &format!("selected-{manifest}-{index}.mp4"));
            request.selection = selection;
            let output = request.destination.clone();
            let result = downloader.download(request, CancellationToken::new(), |_| {}).await;
            assert!(result.is_ok(), "{manifest} choice {index}: {result:?}");
            probe_selected(&output, height, audio);
        }
        let mut stale = fixture.request(manifest, &format!("stale-{manifest}.mp4"));
        stale.selection.video = VideoSelection::Variant("removed-choice".into());
        let output = stale.destination.clone();
        assert!(matches!(
            downloader.download(stale, CancellationToken::new(), |_| {}).await,
            Err(MediaError::InvalidSelection)
        ));
        assert!(!output.exists());
    }
    fixture.assert_clean();
}

fn probe(path: &Path) {
    let output = Command::new(tool("ffprobe"))
        .args(["-v", "error", "-show_entries", "format=duration:stream=codec_type", "-of", "json"])
        .arg(path)
        .output()
        .expect("real FFprobe must be installed");
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let duration = json["format"]["duration"].as_str().unwrap().parse::<f64>().unwrap();
    assert!((2.5..3.5).contains(&duration), "duration {duration}");
    let streams = json["streams"].as_array().unwrap();
    assert!(streams.iter().any(|stream| stream["codec_type"] == "video"));
    assert!(streams.iter().any(|stream| stream["codec_type"] == "audio"));
}

#[tokio::test]
#[ignore = "requires real FFmpeg/FFprobe; validates guarded replacement and original-file preservation"]
async fn explicit_overwrite_preserves_original_on_cancel_error_and_user_changes() {
    let fixture = Fixture::new();
    let downloader = fixture.downloader();
    let mut request = fixture.request("clear.m3u8", "overwrite.mp4");
    let destination = request.destination.clone();
    fs::write(&destination, b"original file").unwrap();
    assert!(matches!(
        downloader.download(request.clone(), CancellationToken::new(), |_| {}).await,
        Err(MediaError::DestinationExists)
    ));
    request.overwrite = true;
    let mut started = false;
    let mut completed = false;
    downloader
        .download(request.clone(), CancellationToken::new(), |event| {
            if matches!(event, MediaEvent::Started { .. }) {
                started = true;
                assert_eq!(fs::read(&destination).unwrap(), b"original file");
            }
            completed |= matches!(event, MediaEvent::Completed { .. });
        })
        .await
        .unwrap();
    assert!(started && completed);
    probe(&destination);

    fs::write(&destination, b"original file").unwrap();
    let mut completed = false;
    let result = downloader
        .download(request.clone(), CancellationToken::new(), |event| {
            if matches!(event, MediaEvent::Started { .. }) {
                fs::write(&destination, b"user changed during download").unwrap();
            }
            completed |= matches!(event, MediaEvent::Completed { .. });
        })
        .await;
    assert!(matches!(result, Err(MediaError::DestinationChanged)), "{result:?}");
    assert!(!completed);
    assert_eq!(fs::read(&destination).unwrap(), b"user changed during download");

    fs::write(&destination, b"original file").unwrap();
    let good = fs::read_to_string(fixture.dir.path().join("clear.m3u8")).unwrap();
    fs::write(fixture.dir.path().join("broken.m3u8"), good.replace("clear-001.ts", "missing-001.ts")).unwrap();
    let mut broken = fixture.request("broken.m3u8", "overwrite.mp4");
    broken.overwrite = true;
    let result = downloader.download(broken, CancellationToken::new(), |_| {}).await;
    assert!(matches!(result, Err(MediaError::FfmpegDiagnostic) | Err(MediaError::FfmpegFailed(_))), "{result:?}");
    assert_eq!(fs::read(&destination).unwrap(), b"original file");

    fixture.server.delay.store(400, Ordering::SeqCst);
    fixture.server.segments.store(0, Ordering::SeqCst);
    let cancel = CancellationToken::new();
    let worker = tokio::spawn({
        let cancel = cancel.clone();
        async move { downloader.download(request, cancel, |_| {}).await }
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.server.segments.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "FFmpeg did not request a real segment");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    cancel.cancel();
    assert!(matches!(worker.await.unwrap(), Err(MediaError::Cancelled)));
    assert_eq!(fs::read(&destination).unwrap(), b"original file");
    fixture.assert_clean();
}

#[tokio::test]
#[ignore = "requires real FFmpeg and FFprobe; run explicitly for media acceptance"]
async fn downloads_real_hls_master_aes128_and_dash_with_progress() {
    let fixture = Fixture::new();
    let downloader = fixture.downloader();
    for (manifest, output, kind, aes) in [
        ("master.m3u8", "中文 HLS.mp4", MediaKind::Hls, false),
        ("encrypted.m3u8", "AES.mp4", MediaKind::Hls, true),
        ("stream.mpd", "DASH.mkv", MediaKind::Dash, false),
    ] {
        let request = fixture.request(manifest, output);
        let mut progress_count = 0;
        let mut completed = false;
        let result = downloader
            .download(request, CancellationToken::new(), |event| match event {
                MediaEvent::Progress(_) => progress_count += 1,
                MediaEvent::Completed { .. } => completed = true,
                _ => {}
            })
            .await
            .unwrap_or_else(|error| panic!("{manifest}: {error}"));
        assert_eq!(result.info.kind, kind);
        assert_eq!(result.info.uses_aes128, aes);
        assert!(result.bytes > 0 && completed && progress_count > 0);
        probe(&result.destination);
    }
    for (_, headers) in fixture.server.requests.lock().unwrap().iter() {
        assert!(headers.iter().any(|header| header.eq_ignore_ascii_case("referer: https://example.org/")));
        assert!(!headers.iter().any(|header| header.contains("secret")));
    }
    fixture.assert_clean();
}

#[tokio::test]
#[ignore = "requires real FFmpeg; tests cancellation while a media response is delayed"]
async fn cancellation_reaps_ffmpeg_cleans_files_and_limits_concurrency() {
    let fixture = Fixture::new();
    fixture.server.delay.store(2000, Ordering::SeqCst);
    let downloader = fixture.downloader();
    let cancel_first = CancellationToken::new();
    let first = tokio::spawn({
        let downloader = downloader.clone();
        let request = fixture.request("clear.m3u8", "cancel-first.mp4");
        let cancel = cancel_first.clone();
        async move { downloader.download(request, cancel, |_| {}).await }
    });
    let start = Instant::now();
    while fixture.server.segments.load(Ordering::SeqCst) == 0 {
        assert!(start.elapsed() < Duration::from_secs(8), "FFmpeg did not request a media segment");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let cancel_second = CancellationToken::new();
    let second_started = Arc::new(AtomicBool::new(false));
    let (waiting, ready) = tokio::sync::oneshot::channel();
    let second = tokio::spawn({
        let downloader = downloader.clone();
        let request = fixture.request("clear.m3u8", "cancel-second.mp4");
        let cancel = cancel_second.clone();
        let started = second_started.clone();
        async move {
            let mut waiting = Some(waiting);
            downloader
                .download(request, cancel, move |event| match event {
                    MediaEvent::Waiting => {
                        if let Some(waiting) = waiting.take() {
                            let _ = waiting.send(());
                        }
                    }
                    MediaEvent::Started { .. } => {
                        started.store(true, Ordering::SeqCst);
                    }
                    _ => {}
                })
                .await
        }
    });
    ready.await.unwrap();
    cancel_second.cancel();
    assert!(matches!(second.await.unwrap(), Err(MediaError::Cancelled)));
    assert!(!second_started.load(Ordering::SeqCst));
    cancel_first.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), first).await.unwrap().unwrap(),
        Err(MediaError::Cancelled)
    ));
    assert!(!fixture.dir.path().join("cancel-first.mp4").exists());
    assert!(!fixture.dir.path().join("cancel-second.mp4").exists());
    fixture.assert_clean();
}

#[tokio::test]
#[ignore = "requires real FFmpeg; tests final publication and protocol defense"]
async fn no_clobber_and_changed_remote_manifest_cannot_read_local_file() {
    let fixture = Fixture::new();
    let downloader = fixture.downloader();
    let request = fixture.request("clear.m3u8", "late-conflict.mp4");
    let destination = request.destination.clone();
    let result = downloader
        .download(request, CancellationToken::new(), |event| {
            if matches!(event, MediaEvent::Started { .. }) {
                fs::write(&destination, b"existing user file").unwrap();
            }
        })
        .await;
    assert!(matches!(result, Err(MediaError::DestinationExists)));
    assert_eq!(fs::read(&destination).unwrap(), b"existing user file");
    fixture.assert_clean();

    let attack = "attack.m3u8";
    fs::copy(fixture.dir.path().join("clear.m3u8"), fixture.dir.path().join(attack)).unwrap();
    *fixture.server.replacement.lock().unwrap() = Some((
        format!("/{attack}"),
        format!(
            "#EXTM3U\n#EXT-X-TARGETDURATION:3\n#EXTINF:3,\nfile://{}\n#EXT-X-ENDLIST\n",
            fixture.dir.path().join("clear-000.ts").display()
        ),
    ));
    let request = fixture.request(attack, "blocked-local.mp4");
    let destination = request.destination.clone();
    let error = downloader.download(request, CancellationToken::new(), |_| {}).await.unwrap_err();
    assert!(matches!(error, MediaError::FfmpegFailed(_)), "{error}");
    assert!(!format!("{error:?}").contains("do-not-log"));
    assert!(!destination.exists());
    fixture.assert_clean();

    // FFmpeg may skip missing HLS segments and still return exit status 0.
    // The library must never publish such an incomplete download as completed.
    fs::remove_file(fixture.dir.path().join("clear-001.ts")).unwrap();
    let request = fixture.request("clear.m3u8", "missing-segment.mp4");
    let destination = request.destination.clone();
    let mut completed = false;
    let error = downloader
        .download(request, CancellationToken::new(), |event| {
            completed |= matches!(event, MediaEvent::Completed { .. });
        })
        .await
        .unwrap_err();
    assert!(matches!(error, MediaError::FfmpegDiagnostic | MediaError::FfmpegFailed(_)));
    assert!(!completed && !destination.exists());
    fixture.assert_clean();
}

#[tokio::test]
async fn manifest_budgets_nested_protection_and_cycles_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(dir.path().to_owned());
    let source = |name: &str| MediaSource {
        url: format!("{}/{name}", server.url),
        headers: vec![("User-Agent".into(), "SwoopMediaTest/1.0".into())],
    };
    let downloader = MediaDownloader::new(MediaConfig { max_manifest_bytes: 1024, ..Default::default() }).unwrap();
    let cancel = CancellationToken::new();
    fs::write(dir.path().join("oversized.m3u8"), format!("#EXTM3U\n#{}\n", "a".repeat(1024))).unwrap();
    assert!(matches!(downloader.inspect(&source("oversized.m3u8"), &cancel).await, Err(MediaError::ManifestLimit)));
    fs::write(dir.path().join("master.m3u8"), "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100\nchild.m3u8\n").unwrap();
    fs::write(
        dir.path().join("child.m3u8"),
        "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"key.bin\"\n#EXTINF:3,\nsegment.ts\n#EXT-X-ENDLIST\n",
    )
    .unwrap();
    assert!(matches!(downloader.inspect(&source("master.m3u8"), &cancel).await, Err(MediaError::ProtectedMedia)));
    fs::write(dir.path().join("child.m3u8"), "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100\nmaster.m3u8\n").unwrap();
    assert!(matches!(downloader.inspect(&source("master.m3u8"), &cancel).await, Err(MediaError::InvalidManifest)));
    let constrained = MediaDownloader::new(MediaConfig { max_playlists: 1, ..Default::default() }).unwrap();
    assert!(matches!(constrained.inspect(&source("master.m3u8"), &cancel).await, Err(MediaError::ManifestLimit)));
    let error = downloader.inspect(&source("missing.m3u8"), &cancel).await.unwrap_err();
    assert!(matches!(error, MediaError::HttpStatus(404)));
    assert!(!format!("{error:?}").contains(&server.url));
}

#[tokio::test]
#[ignore = "requires real FFmpeg/FFprobe; validates scoped-auth HLS gateway media/key/map/range/subtitles"]
async fn authenticated_gateway_preserves_hls_quality_subtitles_aes_and_byte_ranges() {
    use swoop_media::{MediaAuthorization, MediaNetworkConfig, MediaSessionAuth, OriginCredentials};
    let fixture = Fixture::new();
    fixture.add_subtitles();
    *fixture.server.authorization.lock().unwrap() = Some("Bearer session-test-only".into());
    ffmpeg(
        &[
            "-i",
            "source.mp4",
            "-c",
            "copy",
            "-hls_time",
            "1",
            "-hls_list_size",
            "0",
            "-hls_flags",
            "single_file",
            "-hls_segment_filename",
            "single.ts",
            "single.m3u8",
        ],
        fixture.dir.path(),
    );
    let downloader = MediaDownloader::new_with_network(
        MediaConfig { ffmpeg_path: tool("ffmpeg").into(), max_concurrent: 1, ..Default::default() },
        MediaNetworkConfig::default(),
    )
    .unwrap();
    let auth = MediaSessionAuth::new(vec![
        OriginCredentials::new(
            &fixture.server.url,
            Some(MediaAuthorization::Bearer("session-test-only".into())),
            vec![],
        )
        .unwrap(),
    ])
    .unwrap();
    for manifest in ["master.m3u8", "encrypted.m3u8", "single.m3u8"] {
        let request = fixture.request(manifest, &format!("gateway-{manifest}.mp4"));
        let destination = request.destination.clone();
        let result = downloader.download_with_auth(request, auth.clone(), CancellationToken::new(), |_| {}).await;
        assert!(result.is_ok(), "{manifest}: {result:?}");
        probe(&destination);
    }
    for (extension, codec) in [("mp4", "mov_text"), ("mkv", "subrip")] {
        let mut request = fixture.request("subtitles.m3u8", &format!("gateway-captions.{extension}"));
        let info = downloader.inspect_with_auth(&request.source, &auth, &CancellationToken::new()).await.unwrap();
        request.selection.video = VideoSelection::MaxHeight(90);
        request.selection.audio = AudioSelection::PreferredLanguage("en".into());
        request.selection.subtitle = SubtitleSelection::Track(
            info.subtitle_tracks.iter().find(|track| track.language.as_deref() == Some("zho")).unwrap().id.clone(),
        );
        let destination = request.destination.clone();
        let result = downloader.download_with_auth(request, auth.clone(), CancellationToken::new(), |_| {}).await;
        assert!(result.is_ok(), "subtitle {extension}: {result:?}");
        probe_selected(&destination, 90, Some((1, "44100")));
        probe_subtitles(&destination, Some(codec));
    }
    let requests = fixture.server.requests.lock().unwrap();
    assert!(requests.iter().all(|(_, headers)| {
        headers.iter().any(|line| line.eq_ignore_ascii_case("Authorization: Bearer session-test-only"))
    }));
    assert!(requests.iter().any(|(path, _)| path == "/key.bin"));
    assert!(requests.iter().any(|(path, headers)| {
        path == "/single.ts" && headers.iter().any(|line| line.to_ascii_lowercase().starts_with("range: bytes="))
    }));
    assert!(requests.iter().any(|(path, _)| path.ends_with(".mp4")));
    fixture.assert_clean();
}

#[tokio::test]
#[ignore = "requires real FFmpeg; verifies gateway failures and cancellation cannot publish output"]
async fn authenticated_gateway_cancellation_and_missing_segments_do_not_complete() {
    use swoop_media::{MediaAuthorization, MediaNetworkConfig, MediaSessionAuth, OriginCredentials};
    let fixture = Fixture::new();
    *fixture.server.authorization.lock().unwrap() = Some("Bearer session-test-only".into());
    let auth = MediaSessionAuth::new(vec![
        OriginCredentials::new(
            &fixture.server.url,
            Some(MediaAuthorization::Bearer("session-test-only".into())),
            vec![],
        )
        .unwrap(),
    ])
    .unwrap();
    let downloader = MediaDownloader::new_with_network(
        MediaConfig { ffmpeg_path: tool("ffmpeg").into(), ..Default::default() },
        MediaNetworkConfig::default(),
    )
    .unwrap();
    let clear = fs::read_to_string(fixture.dir.path().join("clear.m3u8")).unwrap();
    fs::write(fixture.dir.path().join("broken-gateway.m3u8"), clear.replace("clear-001.ts", "absent.ts")).unwrap();
    let request = fixture.request("broken-gateway.m3u8", "gateway-broken.mp4");
    let destination = request.destination.clone();
    let completed = AtomicBool::new(false);
    assert!(
        downloader
            .download_with_auth(request, auth.clone(), CancellationToken::new(), |event| {
                if matches!(event, MediaEvent::Completed { .. }) {
                    completed.store(true, Ordering::Relaxed);
                }
            })
            .await
            .is_err()
    );
    assert!(!destination.exists());
    assert!(!completed.load(Ordering::Relaxed));
    fixture.server.delay.store(400, Ordering::SeqCst);
    fixture.server.segments.store(0, Ordering::SeqCst);
    let request = fixture.request("clear.m3u8", "gateway-cancel.mp4");
    let destination = request.destination.clone();
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let worker = tokio::spawn(async move { downloader.download_with_auth(request, auth, worker_cancel, |_| {}).await });
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.server.segments.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "gateway must request real media bytes");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    cancel.cancel();
    assert!(matches!(worker.await.unwrap(), Err(MediaError::Cancelled)));
    assert!(!destination.exists());
    fixture.assert_clean();
}

#[tokio::test]
#[ignore = "requires real FFmpeg/FFprobe; proves authenticated manifests/keys/segments all use explicit proxy"]
async fn authenticated_gateway_manual_proxy_routes_every_upstream_resource() {
    use swoop_media::{MediaAuthorization, MediaNetworkConfig, MediaProxyConfig, MediaSessionAuth, OriginCredentials};
    let fixture = Fixture::new();
    *fixture.server.authorization.lock().unwrap() = Some("Bearer proxy-session-test".into());
    let downloader = MediaDownloader::new_with_network(
        MediaConfig { ffmpeg_path: tool("ffmpeg").into(), ..Default::default() },
        MediaNetworkConfig {
            proxy: Some(MediaProxyConfig { url: fixture.server.url.clone(), bypass: vec![] }),
            ..Default::default()
        },
    )
    .unwrap();
    let auth = MediaSessionAuth::new(vec![
        OriginCredentials::new(
            "http://media.invalid",
            Some(MediaAuthorization::Bearer("proxy-session-test".into())),
            vec![],
        )
        .unwrap(),
    ])
    .unwrap();
    let mut request = fixture.request("encrypted.m3u8", "proxy-encrypted.mp4");
    request.source.url = "http://media.invalid/encrypted.m3u8".into();
    let destination = request.destination.clone();
    downloader.download_with_auth(request, auth, CancellationToken::new(), |_| {}).await.unwrap();
    probe(&destination);
    let requests = fixture.server.requests.lock().unwrap();
    assert!(requests.iter().any(|(path, _)| path == "/key.bin"));
    assert!(requests.iter().any(|(path, _)| path.ends_with(".ts")));
    assert!(
        requests
            .iter()
            .all(|(_, headers)| { headers.iter().any(|line| line.eq_ignore_ascii_case("Host: media.invalid")) })
    );
    assert!(requests.iter().all(|(_, headers)| {
        headers.iter().any(|line| line.eq_ignore_ascii_case("Authorization: Bearer proxy-session-test"))
    }));
    fixture.assert_clean();
}

#[tokio::test]
#[ignore = "requires real FFmpeg/FFprobe; authenticated DASH templates, BaseURL, subtitles and proxy"]
async fn authenticated_dash_gateway_preserves_quality_and_subtitles() {
    use swoop_media::{MediaAuthorization, MediaNetworkConfig, MediaProxyConfig, MediaSessionAuth, OriginCredentials};
    let fixture = Fixture::new();
    fixture.add_subtitles();
    *fixture.server.authorization.lock().unwrap() = Some("Bearer dash-session".into());
    let downloader = MediaDownloader::new_with_network(
        MediaConfig { ffmpeg_path: tool("ffmpeg").into(), ..Default::default() },
        MediaNetworkConfig {
            proxy: Some(MediaProxyConfig { url: fixture.server.url.clone(), bypass: vec![] }),
            ..Default::default()
        },
    )
    .unwrap();
    let auth = MediaSessionAuth::new(vec![
        OriginCredentials::new("http://media.invalid", Some(MediaAuthorization::Bearer("dash-session".into())), vec![])
            .unwrap(),
    ])
    .unwrap();
    for manifest in ["qualities.mpd", "subtitles.mpd", "mov-text.mpd"] {
        let mut request = fixture.request(manifest, &format!("controlled-{manifest}.mp4"));
        request.source.url = format!("http://media.invalid/{manifest}?secret=dash-query");
        if manifest != "qualities.mpd" {
            let info = downloader.inspect_with_auth(&request.source, &auth, &CancellationToken::new()).await.unwrap();
            request.selection.video = VideoSelection::MaxHeight(90);
            request.selection.audio = AudioSelection::PreferredLanguage("en".into());
            request.selection.subtitle = SubtitleSelection::Track(
                info.subtitle_tracks.iter().find(|track| track.language.as_deref() == Some("zho")).unwrap().id.clone(),
            );
        }
        let destination = request.destination.clone();
        let result = downloader.download_with_auth(request, auth.clone(), CancellationToken::new(), |_| {}).await;
        assert!(result.is_ok(), "{manifest}: {result:?}");
        if manifest == "qualities.mpd" {
            probe(&destination);
        } else {
            probe_selected(&destination, 90, Some((1, "44100")));
            probe_subtitles(&destination, Some("mov_text"));
        }
    }
    let requests = fixture.server.requests.lock().unwrap();
    assert!(requests.iter().all(|(_, headers)| {
        headers.iter().any(|line| line.eq_ignore_ascii_case("Authorization: Bearer dash-session"))
    }));
    assert!(requests.iter().any(|(path, _)| path.ends_with(".m4s")));
    fixture.assert_clean();
}

#[tokio::test]
#[ignore = "requires real FFmpeg/FFprobe; authenticated live HLS reload and bounded recording"]
async fn authenticated_live_hls_refreshes_manifests_and_records_bounded_output() {
    use swoop_media::{MediaAuthorization, MediaNetworkConfig, MediaSessionAuth, OriginCredentials};
    let fixture = Fixture::new();
    let full = fs::read_to_string(fixture.dir.path().join("clear.m3u8")).unwrap();
    // Start with only one second. The next live reload reveals the remaining
    // real segments; a frozen first snapshot cannot generate a 2.5s recording.
    let initial = full.split("#EXTINF:").next().unwrap().to_owned() + "#EXTINF:1.000000,\nclear-000.ts\n";
    fs::write(fixture.dir.path().join("live.m3u8"), initial).unwrap();
    let path = fixture.dir.path().join("live.m3u8");
    let live = full.replace("#EXT-X-ENDLIST\n", "");
    let updater = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1300)).await;
        fs::write(path, live).unwrap();
    });
    *fixture.server.authorization.lock().unwrap() = Some("Bearer live-session".into());
    let auth = MediaSessionAuth::new(vec![
        OriginCredentials::new(&fixture.server.url, Some(MediaAuthorization::Bearer("live-session".into())), vec![])
            .unwrap(),
    ])
    .unwrap();
    let downloader = MediaDownloader::new_with_network(
        MediaConfig { ffmpeg_path: tool("ffmpeg").into(), ..Default::default() },
        MediaNetworkConfig::default(),
    )
    .unwrap();
    let mut request = fixture.request("live.m3u8", "live-recorded.mp4");
    request.duration_limit = Some(Duration::from_millis(2500));
    let destination = request.destination.clone();
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        downloader.download_with_auth(request, auth, CancellationToken::new(), |_| {}),
    )
    .await
    .expect("live recording must finish at configured duration");
    updater.await.unwrap();
    assert!(result.is_ok(), "{result:?}");
    let output = Command::new(tool("ffprobe"))
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1"])
        .arg(&destination)
        .output()
        .unwrap();
    assert!(output.status.success());
    let seconds = String::from_utf8(output.stdout).unwrap().trim().parse::<f64>().unwrap();
    assert!((2.4..2.8).contains(&seconds), "recording duration {seconds}");
    let requests = fixture.server.requests.lock().unwrap();
    assert!(
        requests.iter().filter(|(path, _)| path == "/live.m3u8").count() >= 4,
        "live playlist must refresh after initial inspect/prepare"
    );
    assert!(requests.iter().all(|(_, headers)| {
        headers.iter().any(|line| line.eq_ignore_ascii_case("Authorization: Bearer live-session"))
    }));
    fixture.assert_clean();
}

#[tokio::test]
#[ignore = "requires real FFmpeg/FFprobe; live WebVTT and standalone DASH SRT/ASS text subtitles"]
async fn live_and_additional_text_subtitles_preserve_cues() {
    use swoop_media::{MediaNetworkConfig, SubtitleFormat};
    let fixture = Fixture::new();
    fixture.add_subtitles();
    for language in ["eng", "zho"] {
        for ext in ["srt", "ass"] {
            ffmpeg(&["-i", &format!("{language}.vtt"), &format!("{language}.{ext}")], fixture.dir.path());
        }
    }
    let mpd = fs::read_to_string(fixture.dir.path().join("subtitles.mpd")).unwrap();
    for (ext, mime) in [("srt", "application/x-subrip"), ("ass", "text/x-ass")] {
        fs::write(
            fixture.dir.path().join(format!("subtitles-{ext}.mpd")),
            mpd.replace("text/vtt", mime).replace(".vtt", &format!(".{ext}")),
        )
        .unwrap();
    }
    // All rendition playlists are live and share the same media sequence and
    // X-TIMESTAMP-MAP timeline. No ENDLIST shortcut is used for subtitles.
    for file in ["clear.m3u8", "eng-ts-sub.m3u8", "zho-ts-sub.m3u8"] {
        let path = fixture.dir.path().join(file);
        let text = fs::read_to_string(&path).unwrap().replace("#EXT-X-ENDLIST\n", "");
        fs::write(path, text).unwrap();
    }
    let downloader = MediaDownloader::new_with_network(
        MediaConfig { ffmpeg_path: tool("ffmpeg").into(), ..Default::default() },
        MediaNetworkConfig::default(),
    )
    .unwrap();
    for (manifest, format, live) in [
        ("subtitles-ts.m3u8", SubtitleFormat::WebVtt, true),
        ("subtitles-srt.mpd", SubtitleFormat::SubRip, false),
        ("subtitles-ass.mpd", SubtitleFormat::Ass, false),
    ] {
        let mut request = fixture.request(manifest, &format!("text-{manifest}.mkv"));
        let info = downloader.inspect(&request.source, &CancellationToken::new()).await.unwrap();
        assert_eq!(info.is_live, live);
        let subtitle = info.subtitle_tracks.iter().find(|s| s.language.as_deref() == Some("zho")).unwrap();
        assert_eq!(subtitle.format, format);
        request.selection.subtitle = SubtitleSelection::Track(subtitle.id.clone());
        request.selection.video = VideoSelection::MaxHeight(90);
        request.selection.audio = AudioSelection::PreferredLanguage("en".into());
        request.duration_limit = live.then_some(Duration::from_secs(3));
        let destination = request.destination.clone();
        let result = tokio::time::timeout(
            Duration::from_secs(20),
            downloader.download(request, CancellationToken::new(), |_| {}),
        )
        .await
        .expect("bounded live captions");
        assert!(result.is_ok(), "{manifest}: {result:?}");
        probe_subtitles(&destination, Some("subrip"));
    }
    fixture.assert_clean();
}

#[tokio::test]
#[ignore = "requires real FFmpeg with MMSH/ASF and FFprobe; authentic two-phase MMSH framing"]
async fn mmsh_legacy_stream_records_to_mkv_and_preserves_existing_file_on_failure() {
    let fixture = Fixture::new();
    ffmpeg(
        &["-stream_loop", "3", "-i", "source.mp4", "-c:v", "wmv2", "-c:a", "wmav2", "legacy.asf"],
        fixture.dir.path(),
    );
    let asf = fs::read(fixture.dir.path().join("legacy.asf")).unwrap();
    let header_len = u64::from_le_bytes(asf[16..24].try_into().unwrap()) as usize;
    let data_len = u64::from_le_bytes(asf[header_len + 16..header_len + 24].try_into().unwrap()) as usize;
    // File Properties object starts at byte 30 in FFmpeg's generated ASF.
    let packet_len = u32::from_le_bytes(asf[30 + 96..30 + 100].try_into().unwrap()) as usize;
    assert!(packet_len > 0 && packet_len < 65520);
    let mut framed = Vec::new();
    let mut chunk = |kind: u16, sequence: u32, payload: &[u8]| {
        framed.extend(kind.to_le_bytes());
        framed.extend(((payload.len() + 8) as u16).to_le_bytes());
        framed.extend(sequence.to_le_bytes());
        framed.extend(0u16.to_le_bytes());
        framed.extend((payload.len() as u16).to_le_bytes());
        framed.extend(payload);
    };
    chunk(0x4824, 0, &asf[..header_len + 50]);
    for (index, packet) in asf[header_len + 50..header_len + data_len].chunks_exact(packet_len).enumerate() {
        chunk(0x4424, index as u32, packet);
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let requests = Arc::new(AtomicUsize::new(0));
    let worker_requests = requests.clone();
    let worker = std::thread::spawn(move || {
        while !worker_stop.load(Ordering::Relaxed) {
            let (mut socket, _) = match listener.accept() {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(_) => break,
            };
            socket.set_nonblocking(false).unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            let mut request = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                request.push_str(&line);
            }
            if request.is_empty() {
                continue;
            }
            assert!(request.contains("NSPlayer"));
            assert!(request.contains("Pragma:"));
            worker_requests.fetch_add(1, Ordering::Relaxed);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-mms-framed\r\nPragma: features=\"broadcast\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                framed.len()
            );
            let _ = socket.write_all(response.as_bytes()).and_then(|_| socket.write_all(&framed));
        }
    });
    let downloader = fixture.downloader();
    let destination = fixture.dir.path().join("legacy.mkv");
    let request = MediaRequest {
        source: MediaSource::new(format!("mms://{address}/stream")),
        destination: destination.clone(),
        duration_limit: Some(Duration::from_secs(2)),
        selection: Default::default(),
        overwrite: false,
    };
    let result =
        tokio::time::timeout(Duration::from_secs(15), downloader.download(request, CancellationToken::new(), |_| {}))
            .await
            .expect("MMS recording time bounded");
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
    assert!(result.is_ok(), "MMSH: {result:?}");
    assert!(requests.load(Ordering::Relaxed) >= 2, "MMSH describe and play exchanges");
    let output = Command::new(tool("ffprobe"))
        .args(["-v", "error", "-show_streams", "-show_format", "-of", "json"])
        .arg(&destination)
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(json["streams"].as_array().unwrap().iter().any(|s| s["codec_name"] == "wmv2"));
    assert!(json["streams"].as_array().unwrap().iter().any(|s| s["codec_name"] == "wmav2"));
    let seconds = json["format"]["duration"].as_str().unwrap().parse::<f64>().unwrap();
    assert!((1.9..2.3).contains(&seconds), "{seconds}");
    let original = fs::read(&destination).unwrap();
    let request = MediaRequest {
        source: MediaSource::new(format!("mmsh://{address}/missing")),
        destination: destination.clone(),
        duration_limit: Some(Duration::from_secs(2)),
        selection: Default::default(),
        overwrite: true,
    };
    assert!(downloader.download(request, CancellationToken::new(), |_| {}).await.is_err());
    assert_eq!(fs::read(destination).unwrap(), original);
    fixture.assert_clean();
}

#[tokio::test]
#[ignore = "requires real FFmpeg/FFprobe; persistent completed-segment cache with origin revalidation"]
async fn validated_segment_cache_recovers_across_downloader_restart_without_trusting_corrupt_bytes() {
    use swoop_media::MediaNetworkConfig;
    let fixture = Fixture::new();
    let directory = tempfile::tempdir().unwrap();
    let network = MediaNetworkConfig { resume_cache_dir: Some(directory.path().into()), ..Default::default() };
    let build = || {
        MediaDownloader::new_with_network(
            MediaConfig { ffmpeg_path: tool("ffmpeg").into(), ..Default::default() },
            network.clone(),
        )
        .unwrap()
    };
    let downloader = build();
    let request = fixture.request("clear.m3u8", "cache-first.mp4");
    downloader.download(request, CancellationToken::new(), |_| {}).await.unwrap();
    drop(downloader);
    let segments = fixture.server.segments.load(Ordering::Relaxed);
    assert!(segments > 0);
    let downloader = build();
    let request = fixture.request("clear.m3u8", "cache-restart.mp4");
    let destination = request.destination.clone();
    downloader.download(request, CancellationToken::new(), |_| {}).await.unwrap();
    probe(&destination);
    assert_eq!(
        fixture.server.segments.load(Ordering::Relaxed),
        segments,
        "validated hits must not transfer segment bodies"
    );
    assert!(
        fixture
            .server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(_, headers)| headers.iter().any(|h| h.to_ascii_lowercase().starts_with("if-none-match:")))
    );
    let file = fs::read_dir(directory.path().join("swoop-media-cache-v1"))
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| e.path().extension().is_some_and(|ext| ext == "bin"))
        .unwrap()
        .path();
    fs::write(file, b"corrupt cached bytes").unwrap();
    let request = fixture.request("clear.m3u8", "cache-repaired.mp4");
    let destination = request.destination.clone();
    build().download(request, CancellationToken::new(), |_| {}).await.unwrap();
    probe(&destination);
    assert!(
        fixture.server.segments.load(Ordering::Relaxed) > segments,
        "corrupt completed resource must refetch from origin"
    );
    fixture.assert_clean();
}

#[tokio::test]
#[ignore = "requires real live FFmpeg DASH publisher and authenticated FFmpeg recorder"]
async fn authenticated_dynamic_dash_records_while_publisher_updates_manifest() {
    use swoop_media::{MediaAuthorization, MediaNetworkConfig, MediaSessionAuth, OriginCredentials};
    let fixture = Fixture::new();
    let generator = std::env::var("FFMPEG_FIXTURE_BINARY").unwrap_or_else(|_| "ffmpeg".into());
    let mut publisher = Command::new(generator)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-re",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=160x90:rate=12",
            "-re",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=44100",
            "-t",
            "20",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "12",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-f",
            "dash",
            "-init_seg_name",
            "live-init-$RepresentationID$.m4s",
            "-media_seg_name",
            "live-chunk-$RepresentationID$-$Number%05d$.m4s",
            "-seg_duration",
            "1",
            "-use_timeline",
            "1",
            "-window_size",
            "3",
            "-streaming",
            "0",
            "live.mpd",
        ])
        .current_dir(fixture.dir.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2400)).await;
    assert!(publisher.try_wait().unwrap().is_none(), "live publisher must still be running");
    *fixture.server.authorization.lock().unwrap() = Some("Bearer live-dash".into());
    let auth = MediaSessionAuth::new(vec![
        OriginCredentials::new(&fixture.server.url, Some(MediaAuthorization::Bearer("live-dash".into())), vec![])
            .unwrap(),
    ])
    .unwrap();
    let downloader = MediaDownloader::new_with_network(
        MediaConfig { ffmpeg_path: tool("ffmpeg").into(), ..Default::default() },
        MediaNetworkConfig::default(),
    )
    .unwrap();
    let mut request = fixture.request("live.mpd", "dash-recorded.mp4");
    request.duration_limit = Some(Duration::from_millis(4500));
    let destination = request.destination.clone();
    let info = downloader.inspect_with_auth(&request.source, &auth, &CancellationToken::new()).await.unwrap();
    assert!(info.is_live);
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        downloader.download_with_auth(request, auth, CancellationToken::new(), |_| {}),
    )
    .await;
    publisher.kill().unwrap();
    publisher.wait().unwrap();
    let result = result.expect("dynamic DASH recording must finish");
    assert!(
        result.is_ok(),
        "{result:?}; requests: {:?}",
        fixture.server.requests.lock().unwrap().iter().map(|(path, _)| path.clone()).collect::<Vec<_>>()
    );
    let output = Command::new(tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type,duration:packet=stream_index,dts_time,pts_time",
            "-of",
            "json",
        ])
        .arg(&destination)
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let seconds = json["format"]["duration"].as_str().unwrap().parse::<f64>().unwrap();
    assert!(
        (4.3..4.8).contains(&seconds),
        "{seconds}: {json}; requests: {:?}",
        fixture.server.requests.lock().unwrap().iter().map(|(path, _)| path.clone()).collect::<Vec<_>>()
    );
    let mut last_dts = std::collections::HashMap::new();
    for packet in json["packets"].as_array().unwrap() {
        let stream = packet["stream_index"].as_u64().unwrap();
        let dts = packet["dts_time"].as_str().unwrap().parse::<f64>().unwrap();
        if let Some(previous) = last_dts.insert(stream, dts) {
            assert!(dts > previous, "duplicate or reversed DTS: {previous}, {dts}");
        }
    }
    assert_eq!(last_dts.len(), 2, "both video and audio must be present");
    assert!(last_dts.values().all(|time| *time > 4.3 && *time < 4.8));
    let requests = fixture.server.requests.lock().unwrap();
    assert!(
        requests.iter().filter(|(path, _)| path == "/live.mpd").count() > 4,
        "live timeline must be refreshed after the initial four inspections/loads"
    );
    assert!(
        requests.iter().any(|(path, _)| path.contains("live-chunk-0-00005")),
        "recording must extend beyond the initially published two-second window"
    );
    assert!(
        requests.iter().all(|(_, headers)| headers
            .iter()
            .any(|line| line.eq_ignore_ascii_case("Authorization: Bearer live-dash")))
    );
    fixture.assert_clean();
}
