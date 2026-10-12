use super::dialog;
use crate::app::SwoopApp;
use crate::media_tasks::{MediaNetworkMode, MediaPreferences, MediaStatus};
use egui::{Align, Layout, RichText, TextEdit};
use std::path::Path;
use swoop_media::{
    AudioSelection, MediaAuthorization, MediaInfo, MediaSelection, MediaSessionAuth, OriginCredentials, ScopedCookie,
    SubtitleFormat, SubtitleSelection, VideoSelection,
};
use tokio_util::sync::CancellationToken;

struct Inspection {
    url: String,
    cancel: CancellationToken,
    result: std::sync::mpsc::Receiver<Result<MediaInfo, String>>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum LoginKind {
    #[default]
    Bearer,
    Basic,
    Cookies,
}

struct LoginForm {
    enabled: bool,
    bound_origin: Option<String>,
    kind: LoginKind,
    username: String,
    password: String,
    token: String,
    cookies: String,
    cookie_path: String,
    cookie_secure: bool,
}
impl Default for LoginForm {
    fn default() -> Self {
        Self {
            enabled: false,
            bound_origin: None,
            kind: LoginKind::Bearer,
            username: String::new(),
            password: String::new(),
            token: String::new(),
            cookies: String::new(),
            cookie_path: "/".into(),
            cookie_secure: true,
        }
    }
}
impl LoginForm {
    fn bind_source(&mut self, source: &str) {
        let origin = url::Url::parse(source).ok().map(|url| url.origin().ascii_serialization());
        if self.bound_origin != origin {
            *self = Self { bound_origin: origin, ..Default::default() };
        }
    }
    fn build(&self, source: &str) -> Result<MediaSessionAuth, String> {
        if !self.enabled {
            return Ok(MediaSessionAuth::default());
        }
        let url = url::Url::parse(source).map_err(|_| swoop_i18n::tr!("媒体地址无效"))?;
        if self.bound_origin.as_deref() != Some(url.origin().ascii_serialization().as_str()) {
            return Err(swoop_i18n::tr!("媒体来源已变更，请重新输入本次登录凭据").into());
        }
        let authorization = match self.kind {
            LoginKind::Bearer if self.token.trim().is_empty() => {
                return Err(swoop_i18n::tr!("请输入 Bearer Token").into());
            }
            LoginKind::Bearer => Some(MediaAuthorization::Bearer(self.token.trim().to_owned())),
            LoginKind::Basic if self.username.is_empty() => return Err(swoop_i18n::tr!("请输入账号").into()),
            LoginKind::Basic => {
                Some(MediaAuthorization::Basic { username: self.username.clone(), password: self.password.clone() })
            }
            LoginKind::Cookies => None,
        };
        let cookies = self
            .cookies
            .split(';')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                let (name, value) =
                    value.split_once('=').ok_or(swoop_i18n::tr!("Cookie 格式为 name=value；多项用分号分隔"))?;
                Ok(ScopedCookie {
                    name: name.trim().to_owned(),
                    value: value.trim().to_owned(),
                    path: self.cookie_path.clone(),
                    secure: self.cookie_secure,
                    expires_at: None,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        if authorization.is_none() && cookies.is_empty() {
            return Err(swoop_i18n::tr!("请输入本次 Cookie").into());
        }
        let origin = OriginCredentials::new(&url.origin().ascii_serialization(), authorization, cookies)
            .map_err(|error| error.to_string())?;
        MediaSessionAuth::new(vec![origin]).map_err(|error| error.to_string())
    }
    fn ui(&mut self, ui: &mut egui::Ui, source: &str) -> bool {
        self.bind_source(source);
        let mut changed = crate::ui::checkbox(ui, &mut self.enabled, swoop_i18n::tr!("本次登录")).changed();
        if !self.enabled {
            return changed;
        }
        ui.label(url::Url::parse(source).map(|url| url.origin().ascii_serialization()).unwrap_or_default());
        egui::ComboBox::from_id_salt("media_login_kind")
            .selected_text(match self.kind {
                LoginKind::Bearer => "Bearer Token",
                LoginKind::Basic => swoop_i18n::tr!("账号密码"),
                LoginKind::Cookies => swoop_i18n::tr!("仅 Cookie"),
            })
            .show_ui(ui, |ui| {
                for (kind, label) in [
                    (LoginKind::Bearer, "Bearer Token"),
                    (LoginKind::Basic, swoop_i18n::tr!("账号密码")),
                    (LoginKind::Cookies, swoop_i18n::tr!("仅 Cookie")),
                ] {
                    changed |= ui.selectable_value(&mut self.kind, kind, label).changed();
                }
            });
        match self.kind {
            LoginKind::Bearer => {
                changed |= ui
                    .add(
                        TextEdit::singleline(&mut self.token)
                            .password(true)
                            .hint_text(swoop_i18n::tr!("Token，不含 Bearer 前缀"))
                            .desired_width(450.0),
                    )
                    .changed();
            }
            LoginKind::Basic => {
                changed |=
                    ui.add(TextEdit::singleline(&mut self.username).hint_text(swoop_i18n::tr!("账号"))).changed();
                changed |= ui
                    .add(TextEdit::singleline(&mut self.password).password(true).hint_text(swoop_i18n::tr!("密码")))
                    .changed();
            }
            LoginKind::Cookies => {}
        }
        changed |= ui
            .add(
                TextEdit::singleline(&mut self.cookies)
                    .password(true)
                    .hint_text(swoop_i18n::tr!("Cookie（可选）：name=value; name2=value2"))
                    .desired_width(450.0),
            )
            .changed();
        if !self.cookies.is_empty() {
            ui.horizontal(|ui| {
                ui.label(swoop_i18n::tr!("Cookie 路径"));
                changed |= ui.add(TextEdit::singleline(&mut self.cookie_path).desired_width(160.0)).changed();
                changed |= crate::ui::checkbox(ui, &mut self.cookie_secure, swoop_i18n::tr!("仅 HTTPS")).changed();
            });
        }
        ui.weak(swoop_i18n::tr!("仅用于此站点本次媒体下载；退出后需重新输入。"));
        changed
    }
}

pub struct MediaWindow {
    pub open: bool,
    pub url: String,
    pub destination: String,
    pub headers: Vec<(String, String)>,
    pub record: bool,
    pub seconds: u64,
    pub speed_limit_bps: u64,
    rate_initialized: bool,
    proxy_login: super::media_proxy::ProxyLoginForm,
    rate_editor: Option<(u64, u64)>,
    pub error: Option<String>,
    selection: MediaSelection,
    selection_initialized: bool,
    info: Option<MediaInfo>,
    inspection: Option<Inspection>,
    preferences: Option<MediaPreferences>,
    pending: std::collections::VecDeque<(swoop_media::MediaSource, std::path::PathBuf)>,
    login: LoginForm,
    reauthenticate: Option<(u64, LoginForm)>,
}
impl Default for MediaWindow {
    fn default() -> Self {
        Self {
            open: false,
            url: String::new(),
            destination: String::new(),
            headers: Vec::new(),
            record: false,
            seconds: 3600,
            speed_limit_bps: 0,
            rate_initialized: false,
            proxy_login: Default::default(),
            rate_editor: None,
            error: None,
            selection: Default::default(),
            selection_initialized: false,
            info: None,
            inspection: None,
            preferences: None,
            pending: Default::default(),
            login: Default::default(),
            reauthenticate: None,
        }
    }
}
impl MediaWindow {
    fn clear_inspection(&mut self) {
        if let Some(inspection) = self.inspection.take() {
            inspection.cancel.cancel();
        }
        self.info = None;
        self.selection = Default::default();
        self.selection_initialized = false;
    }

    fn poll_inspection(&mut self) {
        let Some(inspection) = self.inspection.as_ref() else {
            return;
        };
        let result = match inspection.result.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(swoop_i18n::tr!("媒体检查意外结束，请重试").into()),
        };
        let inspection = self.inspection.take().unwrap();
        if inspection.url != self.url.trim() {
            return;
        }
        match result {
            Ok(info) => {
                if info.is_live {
                    self.record = true;
                }
                self.info = Some(info);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn receive(&mut self, source: swoop_media::MediaSource, directory: &Path) {
        self.open = true;
        if !self.url.trim().is_empty() {
            self.pending.push_back((source, directory.into()));
            return;
        }
        self.clear_inspection();
        self.login = LoginForm::default();
        self.login.bind_source(&source.url);
        self.rate_initialized = false;
        self.destination = directory.join(suggested_name(&source.url)).display().to_string();
        self.record = legacy_mms(&source.url);
        self.url = source.url;
        self.headers = source.headers;
        self.error = None;
    }

    fn source_edited(&mut self) {
        self.headers.clear();
        self.login = LoginForm::default();
        self.login.bind_source(&self.url);
        self.clear_inspection();
        if legacy_mms(&self.url) {
            self.record = true;
            if Path::new(&self.destination).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("mp4")) {
                self.destination = Path::new(&self.destination).with_extension("mkv").display().to_string();
            }
        }
    }

    fn next(&mut self) {
        self.clear_inspection();
        self.rate_initialized = false;
        self.url.clear();
        self.headers.clear();
        self.login = LoginForm::default();
        if let Some((source, directory)) = self.pending.pop_front() {
            self.receive(source, &directory);
        }
    }
}
impl Drop for MediaWindow {
    fn drop(&mut self) {
        if let Some(inspection) = &self.inspection {
            inspection.cancel.cancel();
        }
    }
}

fn variant_label(variant: &swoop_media::MediaVariant) -> String {
    let resolution = match (variant.width, variant.height) {
        (Some(width), Some(height)) => format!("{width}×{height}"),
        (_, Some(height)) => format!("{height}p"),
        _ => swoop_i18n::tr!("未标注分辨率").into(),
    };
    let bitrate = variant.bandwidth.map(|value| format!(" · {} kbps", value / 1000)).unwrap_or_default();
    format!("{resolution}{bitrate} · {}", variant.codecs.as_deref().unwrap_or(swoop_i18n::tr!("未标注编码")))
}

fn video_label(selection: &VideoSelection, info: Option<&MediaInfo>) -> String {
    match selection {
        VideoSelection::Best => swoop_i18n::tr!("自动最佳").into(),
        VideoSelection::MaxHeight(height) => swoop_i18n::trf!("最高 {height}p"),
        VideoSelection::Variant(id) => info
            .and_then(|info| info.variants.iter().find(|variant| &variant.id == id))
            .map(variant_label)
            .unwrap_or_else(|| swoop_i18n::tr!("指定清晰度").into()),
    }
}

fn audio_label(selection: &AudioSelection, info: Option<&MediaInfo>) -> String {
    match selection {
        AudioSelection::Auto => swoop_i18n::tr!("自动音轨").into(),
        AudioSelection::None => swoop_i18n::tr!("无音轨").into(),
        AudioSelection::PreferredLanguage(language) => swoop_i18n::trf!("优先 {language}"),
        AudioSelection::Track(id) => info
            .and_then(|info| info.audio_tracks.iter().find(|track| &track.id == id))
            .map(|track| track.name.clone())
            .unwrap_or_else(|| swoop_i18n::tr!("指定音轨").into()),
    }
}

fn subtitle_label(selection: &SubtitleSelection, info: Option<&MediaInfo>) -> String {
    match selection {
        SubtitleSelection::None => swoop_i18n::tr!("关闭").into(),
        SubtitleSelection::Track(id) => info
            .and_then(|info| info.subtitle_tracks.iter().find(|track| &track.id == id))
            .map(|track| {
                format!("{} · {}", track.name, track.language.as_deref().unwrap_or(swoop_i18n::tr!("未标注语言")))
            })
            .unwrap_or_else(|| swoop_i18n::tr!("指定字幕").into()),
    }
}

fn legacy_mms(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|url| matches!(url.scheme(), "mms" | "mmsh" | "mmst"))
}

fn rate_control(ui: &mut egui::Ui, bytes_per_second: &mut u64) {
    ui.horizontal(|ui| {
        let mut enabled = *bytes_per_second != 0;
        if crate::ui::checkbox(ui, &mut enabled, swoop_i18n::tr!("限制")).changed() {
            *bytes_per_second = if enabled { 256 * 1024 } else { 0 };
        }
        if enabled {
            let mut kib = *bytes_per_second as f64 / 1024.0;
            if ui
                .add(
                    egui::DragValue::new(&mut kib)
                        .range(1.0 / 1024.0..=1_073_741_824.0)
                        .max_decimals(3)
                        .speed(16.0)
                        .suffix(" KiB/s"),
                )
                .changed()
            {
                *bytes_per_second = (kib * 1024.0).round().max(1.0) as u64;
            }
        } else {
            ui.label(swoop_i18n::tr!("不限速"));
        }
    });
}

pub fn suggested_name(raw: &str) -> String {
    let url = url::Url::parse(raw).ok();
    let stem = url
        .as_ref()
        .and_then(|u| u.path_segments()?.next_back())
        .and_then(|last| Path::new(last).file_stem()?.to_str())
        .unwrap_or("media");
    let clean: String = stem.chars().filter(|c| c.is_ascii_alphanumeric() || "_-".contains(*c)).take(80).collect();
    let extension = if legacy_mms(raw) { "mkv" } else { "mp4" };
    format!("{}.{extension}", if clean.is_empty() { "media" } else { &clean })
}

impl SwoopApp {
    pub fn media_ui(&mut self, ctx: &egui::Context) {
        if !self.media_window.open {
            return;
        }
        self.media_window.poll_inspection();
        if !self.media_window.selection_initialized {
            self.media_window.selection = if legacy_mms(&self.media_window.url) {
                MediaSelection::default()
            } else {
                self.media_tasks.preferences.selection()
            };
            self.media_window.seconds = self.media_tasks.preferences.default_recording_secs;
            self.media_window.selection_initialized = true;
        }
        if !self.media_window.rate_initialized {
            self.media_window.speed_limit_bps = self.media_tasks.preferences.default_speed_limit_bps;
            self.media_window.rate_initialized = true;
        }
        let p = self.palette;
        let mut open = true;
        let mut start = false;
        let mut action = None;
        let mut save_preferences = false;
        let mut inspect = false;
        dialog::window(swoop_i18n::tr!("媒体下载"), &p).auto_sized().show(ctx, |ui| {
            // 固定宽度、高度随内容；主体超出窗口时在 `dialog::body` 里滚动
            ui.set_width((ctx.content_rect().width() - 40.0).min(700.0));
            if dialog::header(
                ui,
                &p,
                egui_phosphor::regular::FILM_STRIP,
                swoop_i18n::tr!("媒体下载"),
                Some("HLS / DASH / MMS"),
            ) {
                open = false;
            }
            dialog::body(ui, "media_body", |ui| {
                if !self.media_window.pending.is_empty() {
                    ui.horizontal(|ui| {
                        dialog::note(
                            ui,
                            &p,
                            &swoop_i18n::trf!("另有 {} 个媒体链接待处理", self.media_window.pending.len()),
                        );
                        if super::ghost_button(ui, &p, swoop_i18n::tr!("跳过当前")).clicked() {
                            self.media_window.next();
                        }
                    });
                    ui.add_space(8.0);
                }
                const LABEL_W: f32 = 64.0;
                dialog::group(ui, &p, None, |ui| {
                    dialog::field_row(ui, &p, swoop_i18n::tr!("链接"), LABEL_W, |ui| {
                        // 按钮先从右往左排，输入框只占剩余宽度；按钮挤出右缘会让可缩放窗口逐帧变宽。
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if self.media_window.inspection.is_some() {
                                if super::ghost_button(ui, &p, swoop_i18n::tr!("取消检查")).clicked()
                                    && let Some(inspection) = self.media_window.inspection.take()
                                {
                                    inspection.cancel.cancel();
                                }
                                ui.spinner();
                            } else {
                                ui.add_enabled_ui(!self.media_window.url.trim().is_empty(), |ui| {
                                    inspect = super::ghost_button(ui, &p, swoop_i18n::tr!("检查清晰度")).clicked();
                                });
                            }
                            if ui
                                .add(
                                    super::field(&mut self.media_window.url)
                                        .desired_width(ui.available_width())
                                        .hint_text(swoop_i18n::tr!("https://…/playlist.m3u8、manifest.mpd 或 mms://…")),
                                )
                                .changed()
                            {
                                // 改地址时不沿用浏览器上下文。
                                self.media_window.source_edited();
                            }
                        });
                    });
                    dialog::field_row(ui, &p, swoop_i18n::tr!("清晰度"), LABEL_W, |ui| {
                        let before = self.media_window.selection.video.clone();
                        egui::ComboBox::from_id_salt("media_quality")
                            .width(360.0)
                            .selected_text(video_label(&before, self.media_window.info.as_ref()))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.media_window.selection.video,
                                    VideoSelection::Best,
                                    swoop_i18n::tr!("自动最佳"),
                                );
                                for height in [2160, 1080, 720, 480] {
                                    ui.selectable_value(
                                        &mut self.media_window.selection.video,
                                        VideoSelection::MaxHeight(height),
                                        swoop_i18n::trf!("最高 {height}p"),
                                    );
                                }
                                if let Some(info) = &self.media_window.info {
                                    ui.separator();
                                    for variant in &info.variants {
                                        ui.selectable_value(
                                            &mut self.media_window.selection.video,
                                            VideoSelection::Variant(variant.id.clone()),
                                            variant_label(variant),
                                        );
                                    }
                                }
                            });
                        if before != self.media_window.selection.video {
                            self.media_window.selection.audio = AudioSelection::Auto;
                            self.media_window.selection.subtitle = SubtitleSelection::None;
                        }
                        if let Some(info) = &self.media_window.info {
                            let kind = if info.is_live { swoop_i18n::tr!("直播") } else { swoop_i18n::tr!("点播") };
                            let color = if info.is_live { p.danger } else { p.text_muted };
                            super::pill(ui, kind, p.soft(color), color);
                        }
                    });
                    dialog::field_row(ui, &p, swoop_i18n::tr!("音轨"), LABEL_W, |ui| {
                        egui::ComboBox::from_id_salt("media_audio")
                            .width(360.0)
                            .selected_text(audio_label(
                                &self.media_window.selection.audio,
                                self.media_window.info.as_ref(),
                            ))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.media_window.selection.audio,
                                    AudioSelection::Auto,
                                    swoop_i18n::tr!("自动音轨"),
                                );
                                ui.selectable_value(
                                    &mut self.media_window.selection.audio,
                                    AudioSelection::None,
                                    swoop_i18n::tr!("无音轨"),
                                );
                                if let Some(info) = &self.media_window.info {
                                    for track in
                                        info.audio_tracks_for(&self.media_window.selection.video).unwrap_or_default()
                                    {
                                        let detail = format!(
                                            "{} · {} · {}",
                                            track.name,
                                            track.language.as_deref().unwrap_or(swoop_i18n::tr!("未标注语言")),
                                            track.codecs.as_deref().unwrap_or(swoop_i18n::tr!("未标注编码"))
                                        );
                                        ui.selectable_value(
                                            &mut self.media_window.selection.audio,
                                            AudioSelection::Track(track.id.clone()),
                                            detail,
                                        );
                                    }
                                }
                            });
                    });
                    dialog::field_row(ui, &p, swoop_i18n::tr!("字幕"), LABEL_W, |ui| {
                        egui::ComboBox::from_id_salt("media_subtitle")
                            .width(360.0)
                            .selected_text(subtitle_label(
                                &self.media_window.selection.subtitle,
                                self.media_window.info.as_ref(),
                            ))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.media_window.selection.subtitle,
                                    SubtitleSelection::None,
                                    swoop_i18n::tr!("关闭"),
                                );
                                if let Some(info) = &self.media_window.info {
                                    for track in
                                        info.subtitle_tracks_for(&self.media_window.selection.video).unwrap_or_default()
                                    {
                                        let supported = track.format != SubtitleFormat::Unsupported;
                                        let label = format!(
                                            "{} · {}{}",
                                            track.name,
                                            track.language.as_deref().unwrap_or(swoop_i18n::tr!("未标注语言")),
                                            if supported { "" } else { swoop_i18n::tr!("（暂不支持）") }
                                        );
                                        ui.add_enabled_ui(supported, |ui| {
                                            ui.selectable_value(
                                                &mut self.media_window.selection.subtitle,
                                                SubtitleSelection::Track(track.id.clone()),
                                                label,
                                            )
                                            .on_hover_text(
                                                track.codecs.as_deref().unwrap_or(swoop_i18n::tr!("文本字幕")),
                                            );
                                        });
                                    }
                                }
                            });
                        if self.media_window.selection.subtitle != SubtitleSelection::None {
                            dialog::note(ui, &p, swoop_i18n::tr!("文本字幕"));
                            ui.label(super::icon_text(egui_phosphor::regular::INFO, 14.0, p.text_muted)).on_hover_text(
                                swoop_i18n::tr!("MP4 保存为 mov_text，MKV 保存为 SRT；部分原始样式不会保留。"),
                            );
                        }
                    });
                    dialog::field_row(ui, &p, swoop_i18n::tr!("保存到"), LABEL_W, |ui| {
                        ui.add(
                            super::field(&mut self.media_window.destination)
                                .desired_width(ui.available_width())
                                .hint_text(swoop_i18n::tr!("完整 .mp4 或 .mkv 文件路径")),
                        );
                    });
                });

                dialog::group(ui, &p, None, |ui| {
                    dialog::row(ui, &p, swoop_i18n::tr!("本任务限速"), None, |ui| {
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            rate_control(ui, &mut self.media_window.speed_limit_bps);
                        });
                    });
                    dialog::row(ui, &p, swoop_i18n::tr!("限定时长"), None, |ui| {
                        super::toggle(ui, &p, &mut self.media_window.record);
                        if self.media_window.record {
                            ui.add_space(4.0);
                            ui.add(
                                egui::DragValue::new(&mut self.media_window.seconds)
                                    .range(1..=86400)
                                    .suffix(swoop_i18n::tr!(" 秒")),
                            );
                        }
                    });
                });
                egui::CollapsingHeader::new(RichText::new(swoop_i18n::tr!("高级 · 本次登录")).color(p.text_muted))
                    .id_salt("media_login")
                    .show(ui, |ui| {
                        if self.media_window.login.ui(ui, &self.media_window.url) {
                            self.media_window.clear_inspection();
                        }
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    start = super::primary_button(
                        ui,
                        &p,
                        &format!("{}  {}", egui_phosphor::regular::DOWNLOAD_SIMPLE, swoop_i18n::tr!("开始下载")),
                    )
                    .clicked();
                    let settings_open = self.media_window.preferences.is_some();
                    if super::ghost_button(
                        ui,
                        &p,
                        &format!(
                            "{}  {}",
                            if settings_open { egui_phosphor::regular::CARET_UP } else { egui_phosphor::regular::GEAR },
                            swoop_i18n::tr!("媒体设置")
                        ),
                    )
                    .clicked()
                    {
                        if settings_open {
                            self.media_window.preferences = None;
                        } else {
                            self.media_window.proxy_login =
                                super::media_proxy::ProxyLoginForm::from_preferences(&self.media_tasks.preferences);
                            self.media_window.preferences = Some(self.media_tasks.preferences.clone());
                        }
                    }
                });
                ui.add_space(8.0);
                if let Some(error) = &self.media_window.error {
                    dialog::error(ui, &p, error);
                }
                if let Some(error) = &self.media_tasks.network_error {
                    dialog::error(ui, &p, error);
                }
                if let Some(preferences) = &mut self.media_window.preferences {
                    dialog::group_title(ui, &p, swoop_i18n::tr!("媒体设置"));
                    ui.add_enabled_ui(!self.media_tasks.active(), |ui| {
                        egui::Frame::new()
                            .fill(p.group)
                            .stroke(egui::Stroke::new(1.0, p.border))
                            .corner_radius(egui::CornerRadius::same(10))
                            .inner_margin(egui::Margin::same(14))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                egui::Grid::new("media_preferences").num_columns(2).spacing([18.0, 10.0]).show(
                                    ui,
                                    |ui| {
                                        ui.label("FFmpeg");
                                        let mut executable = preferences.ffmpeg.display().to_string();
                                        if ui.add(TextEdit::singleline(&mut executable).desired_width(460.0)).changed()
                                        {
                                            preferences.ffmpeg = executable.into();
                                        }
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("媒体网络"));
                                        egui::ComboBox::from_id_salt("media_network_mode")
                                            .selected_text(match preferences.network_mode {
                                                MediaNetworkMode::Direct => swoop_i18n::tr!("直连"),
                                                MediaNetworkMode::Global => swoop_i18n::tr!("跟随全局"),
                                                MediaNetworkMode::Manual => swoop_i18n::tr!("手动代理"),
                                            })
                                            .show_ui(ui, |ui| {
                                                for (mode, label) in [
                                                    (MediaNetworkMode::Direct, swoop_i18n::tr!("直连")),
                                                    (MediaNetworkMode::Global, swoop_i18n::tr!("跟随全局")),
                                                    (MediaNetworkMode::Manual, swoop_i18n::tr!("手动代理")),
                                                ] {
                                                    ui.selectable_value(&mut preferences.network_mode, mode, label);
                                                }
                                            });
                                        ui.end_row();
                                        if preferences.network_mode == MediaNetworkMode::Manual {
                                            let proxy = preferences.network.proxy.get_or_insert_with(|| {
                                                swoop_media::MediaProxyConfig { url: String::new(), bypass: Vec::new() }
                                            });
                                            ui.label(swoop_i18n::tr!("代理地址"));
                                            ui.add(
                                                TextEdit::singleline(&mut proxy.url)
                                                    .hint_text(swoop_i18n::tr!("http(s):// 或 socks5(h)://"))
                                                    .desired_width(450.0),
                                            );
                                            ui.end_row();
                                            ui.label(swoop_i18n::tr!("直连排除"));
                                            let mut bypass = proxy.bypass.join(", ");
                                            if ui
                                                .add(
                                                    TextEdit::singleline(&mut bypass)
                                                        .hint_text(swoop_i18n::tr!("主机/IP，以逗号分隔")),
                                                )
                                                .changed()
                                            {
                                                proxy.bypass = bypass
                                                    .split(',')
                                                    .map(str::trim)
                                                    .filter(|value| !value.is_empty())
                                                    .map(str::to_owned)
                                                    .collect();
                                            }
                                            ui.end_row();
                                        }
                                        if preferences.network_mode == MediaNetworkMode::Manual {
                                            let endpoint = preferences
                                                .network
                                                .proxy
                                                .as_ref()
                                                .map(|p| p.url.as_str())
                                                .unwrap_or_default();
                                            self.media_window.proxy_login.ui(ui, endpoint);
                                        }
                                        ui.label(swoop_i18n::tr!("媒体总限速"));
                                        rate_control(ui, &mut preferences.network.speed_limit_bps);
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("默认任务限速"));
                                        rate_control(ui, &mut preferences.default_speed_limit_bps);
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("网络重试"));
                                        ui.add(
                                            egui::DragValue::new(&mut preferences.network.request_retries)
                                                .range(0..=10)
                                                .suffix(swoop_i18n::tr!(" 次")),
                                        );
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("重试间隔"));
                                        ui.add(
                                            egui::DragValue::new(&mut preferences.network.retry_delay_ms)
                                                .range(0..=30000)
                                                .suffix(swoop_i18n::tr!(" 毫秒")),
                                        );
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("直播刷新间隔"));
                                        ui.add(
                                            egui::DragValue::new(&mut preferences.network.live_manifest_refresh_ms)
                                                .range(50..=30000)
                                                .suffix(swoop_i18n::tr!(" 毫秒")),
                                        );
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("MMS 传输"));
                                        egui::ComboBox::from_id_salt("media_mms_transport")
                                            .selected_text(match preferences.mms_transport {
                                                swoop_media::MmsTransport::Http => "HTTP (MMSH)",
                                                swoop_media::MmsTransport::Tcp => "TCP (MMST)",
                                            })
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(
                                                    &mut preferences.mms_transport,
                                                    swoop_media::MmsTransport::Http,
                                                    "HTTP (MMSH)",
                                                );
                                                ui.selectable_value(
                                                    &mut preferences.mms_transport,
                                                    swoop_media::MmsTransport::Tcp,
                                                    "TCP (MMST)",
                                                );
                                            });
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("默认录制时长"));
                                        ui.add(
                                            egui::DragValue::new(&mut preferences.default_recording_secs)
                                                .range(1..=86400)
                                                .suffix(swoop_i18n::tr!(" 秒")),
                                        );
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("直播收尾等待"));
                                        ui.add(
                                            egui::DragValue::new(&mut preferences.live_finalize_grace_secs)
                                                .range(0..=300)
                                                .suffix(swoop_i18n::tr!(" 秒")),
                                        );
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("恢复缓存目录"));
                                        let mut directory = preferences
                                            .network
                                            .resume_cache_dir
                                            .as_ref()
                                            .map(|p| p.display().to_string())
                                            .unwrap_or_default();
                                        if ui
                                            .add(
                                                TextEdit::singleline(&mut directory)
                                                    .hint_text(swoop_i18n::tr!("留空关闭分片缓存"))
                                                    .desired_width(450.0),
                                            )
                                            .changed()
                                        {
                                            preferences.network.resume_cache_dir = (!directory.trim().is_empty())
                                                .then(|| std::path::PathBuf::from(directory.trim()));
                                        }
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("分片内续传"));
                                        crate::ui::checkbox(
                                            ui,
                                            &mut preferences.network.resume_partial_segments,
                                            swoop_i18n::tr!("启用"),
                                        );
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("断点保存间隔"));
                                        ui.add(
                                            egui::DragValue::new(&mut preferences.network.partial_flush_bytes)
                                                .range(16384..=16777216)
                                                .suffix(" B"),
                                        );
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("缓存容量"));
                                        ui.add(
                                            egui::DragValue::new(&mut preferences.network.cache_max_mib)
                                                .range(16..=8192)
                                                .suffix(" MiB"),
                                        );
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("单片缓存上限"));
                                        ui.add(
                                            egui::DragValue::new(&mut preferences.network.cache_resource_mib)
                                                .range(1..=64)
                                                .suffix(" MiB"),
                                        );
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("同时下载"));
                                        ui.add(egui::DragValue::new(&mut preferences.concurrency).range(1..=32));
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("网络超时"));
                                        ui.add(
                                            egui::DragValue::new(&mut preferences.timeout_secs)
                                                .range(1..=3600)
                                                .suffix(swoop_i18n::tr!(" 秒")),
                                        );
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("清单大小上限"));
                                        ui.add(
                                            egui::DragValue::new(&mut preferences.max_manifest_kib)
                                                .range(1..=16 * 1024)
                                                .suffix(" KiB"),
                                        );
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("子清单上限"));
                                        ui.add(egui::DragValue::new(&mut preferences.max_playlists).range(1..=128));
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("默认清晰度"));
                                        egui::ComboBox::from_id_salt("media_default_quality")
                                            .selected_text(
                                                preferences
                                                    .default_max_height
                                                    .map(|height| swoop_i18n::trf!("最高 {height}p"))
                                                    .unwrap_or_else(|| swoop_i18n::tr!("自动最佳").into()),
                                            )
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(
                                                    &mut preferences.default_max_height,
                                                    None,
                                                    swoop_i18n::tr!("自动最佳"),
                                                );
                                                for height in [2160, 1080, 720, 480] {
                                                    ui.selectable_value(
                                                        &mut preferences.default_max_height,
                                                        Some(height),
                                                        swoop_i18n::trf!("最高 {height}p"),
                                                    );
                                                }
                                            });
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("优先音轨语言"));
                                        ui.add(
                                            TextEdit::singleline(&mut preferences.preferred_audio_language)
                                                .hint_text(swoop_i18n::tr!("例如 zh、en；留空自动")),
                                        );
                                        ui.end_row();
                                    },
                                );
                            });
                        ui.add_space(8.0);
                        save_preferences = super::primary_button(ui, &p, swoop_i18n::tr!("保存媒体设置")).clicked();
                    });
                    if self.media_tasks.active() {
                        ui.label(
                            RichText::new(swoop_i18n::tr!("停止媒体任务后可修改设置")).size(12.0).color(p.text_muted),
                        );
                    }
                }
                dialog::group_title(ui, &p, swoop_i18n::tr!("下载记录"));
                if self.media_tasks.jobs.is_empty() {
                    dialog::note(ui, &p, swoop_i18n::tr!("暂无媒体下载"));
                }
                for job in self.media_tasks.jobs.iter().rev() {
                    ui.push_id(job.id, |ui| {
                        egui::Frame::new()
                            .fill(p.group)
                            .stroke(egui::Stroke::new(1.0, p.border))
                            .corner_radius(egui::CornerRadius::same(10))
                            .inner_margin(egui::Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let name = job.destination.file_name().unwrap_or_default().to_string_lossy();
                                    super::file_tile(ui, &p, super::filekind::FileKind::Video, 36.0);
                                    ui.add_space(4.0);
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if job.status.active() {
                                            if super::icon_button(
                                                ui,
                                                &p,
                                                egui_phosphor::regular::STOP,
                                                swoop_i18n::tr!("停止"),
                                            )
                                            .clicked()
                                            {
                                                action = Some((job.id, 0));
                                            }
                                        } else {
                                            if super::icon_button(
                                                ui,
                                                &p,
                                                egui_phosphor::regular::TRASH,
                                                swoop_i18n::tr!("移除记录，保留文件"),
                                            )
                                            .clicked()
                                            {
                                                action = Some((job.id, 2));
                                            }
                                            if !matches!(
                                                job.status,
                                                MediaStatus::Completed
                                                    | MediaStatus::FileConflict
                                                    | MediaStatus::NeedsAuth
                                            ) && super::icon_button(
                                                ui,
                                                &p,
                                                egui_phosphor::regular::ARROW_CLOCKWISE,
                                                swoop_i18n::tr!("继续下载"),
                                            )
                                            .clicked()
                                            {
                                                action = Some((job.id, 1));
                                            }
                                            if job.auth_required
                                                && !matches!(
                                                    job.status,
                                                    MediaStatus::Completed | MediaStatus::FileConflict
                                                )
                                                && super::icon_button(
                                                    ui,
                                                    &p,
                                                    egui_phosphor::regular::KEY,
                                                    swoop_i18n::tr!("重新登录后下载"),
                                                )
                                                .clicked()
                                            {
                                                action = Some((job.id, 3));
                                            }
                                        }
                                        if super::icon_button(
                                            ui,
                                            &p,
                                            egui_phosphor::regular::GAUGE,
                                            swoop_i18n::tr!("任务限速"),
                                        )
                                        .clicked()
                                        {
                                            self.media_window.rate_editor = Some((job.id, job.speed_limit_bps));
                                        }
                                        ui.with_layout(Layout::top_down(Align::Min), |ui| {
                                            ui.spacing_mut().item_spacing.y = 2.0;
                                            ui.add(
                                                egui::Label::new(RichText::new(name).strong().color(p.text)).truncate(),
                                            );
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(
                                                        swoop_i18n::trf!(
                                                            "{} · {} · {} 秒",
                                                            job.status.label(),
                                                            super::format::bytes(job.bytes),
                                                            job.media_seconds
                                                        ) + " · "
                                                            + &video_label(&job.selection.video, None),
                                                    )
                                                    .size(12.0)
                                                    .color(p.text_muted),
                                                )
                                                .truncate(),
                                            );
                                        });
                                    });
                                });
                                if let MediaStatus::Failed(error) = &job.status {
                                    ui.add_space(4.0);
                                    ui.label(RichText::new(error).size(12.0).color(p.danger));
                                }
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(job.destination.display().to_string())
                                            .size(11.0)
                                            .color(p.text_muted),
                                    )
                                    .truncate(),
                                );
                            });
                        ui.add_space(6.0);
                    });
                }
            });
        });
        self.media_window.open = open;
        if !open {
            self.media_window.login = LoginForm::default();
            self.media_window.reauthenticate = None;
            self.media_window.proxy_login = Default::default();
            self.media_window.preferences = None;
            self.media_window.rate_editor = None;
        }
        if !open && let Some(inspection) = self.media_window.inspection.take() {
            inspection.cancel.cancel();
        }
        if inspect {
            let selection = self.media_window.selection.clone();
            self.media_window.clear_inspection();
            self.media_window.selection = selection;
            self.media_window.selection_initialized = true;
            let cancel = CancellationToken::new();
            let source = swoop_media::MediaSource {
                url: self.media_window.url.trim().to_owned(),
                headers: self.media_window.headers.clone(),
            };
            match self.media_window.login.build(&source.url) {
                Ok(auth) => {
                    let result = if auth.is_empty() {
                        self.media_tasks.inspect(source, cancel.clone(), ctx)
                    } else {
                        self.media_tasks.inspect_with_auth(source, auth, cancel.clone(), ctx)
                    };
                    self.media_window.inspection =
                        Some(Inspection { url: self.media_window.url.trim().to_owned(), cancel, result });
                    self.media_window.error = None;
                }
                Err(error) => self.media_window.error = Some(error),
            }
        }
        if start {
            let destination = super::add_dialog::expand_tilde(&self.media_window.destination);
            let source = swoop_media::MediaSource {
                url: self.media_window.url.trim().to_string(),
                headers: self.media_window.headers.clone(),
            };
            let result = self.media_window.login.build(&source.url).and_then(|auth| {
                self.media_tasks.add_with_auth_and_limit(
                    source,
                    destination,
                    self.media_window.record.then_some(self.media_window.seconds),
                    self.media_window.selection.clone(),
                    auth,
                    self.media_window.speed_limit_bps,
                    ctx,
                )
            });
            match result {
                Ok(()) => {
                    self.media_window.error = None;
                    self.media_window.next();
                }
                Err(error) => self.media_window.error = Some(error),
            }
        }
        if let Some((id, action)) = action {
            let result = match action {
                0 => {
                    self.media_tasks.cancel(id);
                    Ok(())
                }
                1 => self.media_tasks.restart(id, ctx),
                3 => {
                    self.media_window.reauthenticate = Some((
                        id,
                        LoginForm {
                            enabled: true,
                            bound_origin: self
                                .media_tasks
                                .jobs
                                .iter()
                                .find(|job| job.id == id)
                                .and_then(|job| url::Url::parse(&job.url).ok())
                                .map(|url| url.origin().ascii_serialization()),
                            ..Default::default()
                        },
                    ));
                    Ok(())
                }
                _ => self.media_tasks.remove(id),
            };
            if let Err(error) = result {
                self.media_window.error = Some(error);
            }
        }
        let mut login_open = true;
        let mut login_submit = false;
        if let Some((id, login)) = &mut self.media_window.reauthenticate {
            if let Some(job) = self.media_tasks.jobs.iter().find(|job| job.id == *id) {
                dialog::modal("media_relogin", &p).show(ctx, |ui| {
                    ui.set_width(460.0);
                    if dialog::header(ui, &p, egui_phosphor::regular::KEY, swoop_i18n::tr!("重新登录"), None) {
                        login_open = false;
                    }
                    dialog::body_fixed(ui, |ui| login.ui(ui, &job.url));
                    dialog::footer(ui, &p, |ui| {
                        login_submit = super::primary_button(ui, &p, swoop_i18n::tr!("登录并重新下载")).clicked();
                    });
                });
            } else {
                login_open = false;
            }
        }
        if login_submit && let Some((id, login)) = &self.media_window.reauthenticate {
            let result = self
                .media_tasks
                .jobs
                .iter()
                .find(|job| job.id == *id)
                .ok_or_else(|| swoop_i18n::tr!("媒体记录不存在").to_owned())
                .and_then(|job| login.build(&job.url))
                .and_then(|auth| self.media_tasks.reauthenticate(*id, auth, ctx));
            match result {
                Ok(()) => {
                    login_open = false;
                    self.media_window.error = None;
                }
                Err(error) => self.media_window.error = Some(error),
            }
        }
        if !login_open {
            self.media_window.reauthenticate = None;
        }
        let mut rate_open = true;
        let mut rate_save = false;
        if let Some((id, rate)) = &mut self.media_window.rate_editor {
            dialog::modal("media_task_rate", &p).show(ctx, |ui| {
                ui.set_width(420.0);
                let name = self
                    .media_tasks
                    .jobs
                    .iter()
                    .find(|job| job.id == *id)
                    .map(|job| job.destination.file_name().unwrap_or_default().to_string_lossy().into_owned());
                if dialog::header(ui, &p, egui_phosphor::regular::GAUGE, swoop_i18n::tr!("任务限速"), name.as_deref())
                {
                    rate_open = false;
                }
                if name.is_some() {
                    dialog::body_fixed(ui, |ui| {
                        rate_control(ui, rate);
                        ui.add_space(6.0);
                    });
                    dialog::footer(ui, &p, |ui| {
                        rate_save = super::primary_button(ui, &p, swoop_i18n::tr!("保存")).clicked();
                    });
                }
            });
        }
        if rate_save && let Some((id, rate)) = self.media_window.rate_editor {
            match self.media_tasks.set_speed_limit(id, rate) {
                Ok(()) => rate_open = false,
                Err(error) => self.media_window.error = Some(error),
            }
        }
        if !rate_open {
            self.media_window.rate_editor = None;
        }
        if save_preferences && let Some(mut preferences) = self.media_window.preferences.clone() {
            let saved = self.media_window.proxy_login.apply(&mut preferences).and_then(|credentials| {
                if credentials.is_some() {
                    self.media_tasks.set_preferences_with_proxy(preferences, credentials)
                } else {
                    self.media_tasks.set_preferences(preferences)
                }
            });
            match saved {
                Ok(()) => {
                    self.media_window.preferences = None;
                    self.media_window.proxy_login = Default::default();
                    self.media_window.clear_inspection();
                    self.media_window.error = None;
                }
                Err(error) => self.media_window.error = Some(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_login_is_scoped_and_browser_draft_changes_clear_credentials() {
        let mut window = MediaWindow::default();
        window.receive(swoop_media::MediaSource::new("https://first.test/vod.m3u8"), Path::new("first"));
        window.login.enabled = true;
        window.login.token = "test-session-token".into();
        window.login.cookies = "session=test-cookie".into();
        assert!(!window.login.build(&window.url).unwrap().is_empty());
        window.receive(swoop_media::MediaSource::new("https://second.test/vod.m3u8"), Path::new("second"));
        assert!(window.login.enabled);
        window.next();
        assert!(!window.login.enabled);
        assert!(window.login.token.is_empty());
        assert!(window.login.cookies.is_empty());
        window.login.bind_source(&window.url);
        window.login.enabled = true;
        window.login.kind = LoginKind::Cookies;
        window.login.cookies = "session=bad\r\nInjected: yes".into();
        assert!(window.login.build(&window.url).is_err());
        window.login.token = "old-origin-only".into();
        window.login.cookies.clear();
        window.login.kind = LoginKind::Bearer;
        assert!(window.login.build("https://third.test/vod.m3u8").is_err());
        window.url = "https://third.test/vod.m3u8".into();
        window.source_edited();
        assert!(window.login.token.is_empty());
        assert!(!window.login.enabled);
        // An empty draft can also be externally prefilled; it must not inherit
        // credentials, even if a future caller forgot to invoke source_edited.
        window.login.token = "stale-prefill-token".into();
        window.url.clear();
        window.receive(swoop_media::MediaSource::new("https://fourth.test/vod.m3u8"), Path::new("fourth"));
        assert!(window.login.token.is_empty());
        assert!(!window.login.enabled);
    }
    #[test]
    fn multiple_browser_handoffs_preserve_each_url_and_source_directory() {
        let mut window = MediaWindow::default();
        window.receive(swoop_media::MediaSource::new("https://a.test/one.m3u8"), Path::new("first"));
        window.selection.video = VideoSelection::MaxHeight(720);
        window.record = true;
        window.seconds = 120;
        window.receive(swoop_media::MediaSource::new("https://a.test/two.mpd"), Path::new("second"));
        assert_eq!(window.url, "https://a.test/one.m3u8");
        assert_eq!(window.selection.video, VideoSelection::MaxHeight(720));
        assert!(window.record);
        assert_eq!(window.seconds, 120);
        assert_eq!(window.pending.len(), 1);
        window.next();
        assert_eq!(window.url, "https://a.test/two.mpd");
        assert_eq!(std::path::PathBuf::from(&window.destination), Path::new("second").join("two.mp4"));
        window.next();
        assert!(window.url.is_empty());
        assert!(window.pending.is_empty());
    }

    #[test]
    fn changing_drafts_cancels_inspection_and_late_results_cannot_replace_the_new_draft() {
        let mut window = MediaWindow::default();
        window.receive(swoop_media::MediaSource::new("https://a.test/one.m3u8"), Path::new("first"));
        let cancel = CancellationToken::new();
        let (tx, result) = std::sync::mpsc::channel();
        window.inspection = Some(Inspection { url: window.url.clone(), cancel: cancel.clone(), result });
        window.receive(swoop_media::MediaSource::new("https://a.test/two.mpd"), Path::new("second"));
        window.next();
        assert!(cancel.is_cancelled());
        assert!(tx.send(Err("old failure".into())).is_err());
        assert_eq!(window.url, "https://a.test/two.mpd");
        assert!(window.error.is_none());
        let (tx, result) = std::sync::mpsc::channel();
        window.inspection =
            Some(Inspection { url: "https://a.test/one.m3u8".into(), cancel: CancellationToken::new(), result });
        tx.send(Err("stale".into())).unwrap();
        window.poll_inspection();
        assert!(window.error.is_none());
        assert!(window.inspection.is_none());
    }
}
