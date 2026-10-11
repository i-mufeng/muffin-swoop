//! FTP defaults contain transport settings only; login secrets live in the open dialog.
use swoop_core::{
    ActiveMode, FtpCredentials, FtpDataMode, FtpProxy, FtpProxyKind, FtpProxyPolicy, FtpTaskOptions, FtpTlsMode, Store,
};

const KEY: &str = "ftp_defaults";

pub fn is_ftp(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "ftp" | "ftps"))
}

pub struct FtpEditor {
    pub options: FtpTaskOptions,
    pub username: String,
    pub password: String,
    ca: String,
    active_address: String,
    bypass: String,
}

impl Default for FtpEditor {
    fn default() -> Self {
        Self::from_options(FtpTaskOptions::default())
    }
}

impl FtpEditor {
    pub fn load(store: &Store) -> Self {
        let options = store
            .get_setting(KEY)
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str::<FtpTaskOptions>(&text).ok())
            .filter(|options| {
                options
                    .validate(if options.tls == FtpTlsMode::Implicit {
                        "ftps://example.test/file"
                    } else {
                        "ftp://example.test/file"
                    })
                    .is_ok()
            })
            .unwrap_or_default();
        let mut editor = Self::from_options(options);
        // Defaults never supply login identity, even if an old/hand-edited key contains one.
        editor.username.clear();
        editor.options.username = None;
        editor
    }

    pub fn from_options(options: FtpTaskOptions) -> Self {
        let ca = options.config.ca_certificate.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
        let active_address = match &options.config.data_mode {
            FtpDataMode::Active(active) => active.address.map(|a| a.to_string()).unwrap_or_default(),
            _ => String::new(),
        };
        let bypass = match &options.proxy {
            FtpProxyPolicy::Custom(proxy) => proxy.bypass_hosts.join(", "),
            _ => String::new(),
        };
        Self {
            username: options.username.clone().unwrap_or_default(),
            options,
            password: String::new(),
            ca,
            active_address,
            bypass,
        }
    }

    pub fn clear_credentials(&mut self) {
        self.username.clear();
        self.password.clear();
        self.options.username = None;
    }

    pub fn prepare(&self, value: &str) -> Result<(FtpTaskOptions, Option<FtpCredentials>), String> {
        let mut options = self.options.clone();
        if url::Url::parse(value).is_ok_and(|url| url.scheme() == "ftps") {
            options.tls = FtpTlsMode::Implicit;
        }
        options.config.ca_certificate = (!self.ca.trim().is_empty()).then(|| std::path::PathBuf::from(self.ca.trim()));
        if let FtpDataMode::Active(active) = &mut options.config.data_mode {
            active.address = if self.active_address.trim().is_empty() {
                None
            } else {
                Some(
                    self.active_address
                        .trim()
                        .parse()
                        .map_err(|_| swoop_i18n::tr!("主动连接地址需要 IPv4 或 IPv6 地址"))?,
                )
            };
        }
        if let FtpProxyPolicy::Custom(proxy) = &mut options.proxy {
            proxy.bypass_hosts =
                self.bypass.split([',', '\n']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
        }
        options.username = (!self.username.is_empty()).then(|| self.username.clone());
        if options.username.is_none() && !self.password.is_empty() {
            return Err(swoop_i18n::tr!("请填写 FTP 用户名").into());
        }
        let credentials = options
            .username
            .as_ref()
            .map(|name| FtpCredentials::new(name, &self.password).map_err(|e| e.to_string()))
            .transpose()?;
        options.validate(value).map_err(|e| e.to_string())?;
        Ok((options, credentials))
    }

    pub fn validate_batch(&self, urls: &[String]) -> Result<(), String> {
        let mut origin = None;
        for value in urls.iter().filter(|u| is_ftp(u)) {
            self.prepare(value)?;
            if !self.username.is_empty() {
                let url = url::Url::parse(value).map_err(|_| swoop_i18n::tr!("FTP 链接无效"))?;
                let current = (
                    url.scheme().to_string(),
                    url.host_str().unwrap_or_default().to_string(),
                    url.port_or_known_default(),
                );
                if origin.as_ref().is_some_and(|previous| previous != &current) {
                    return Err(swoop_i18n::tr!("需要登录的 FTP 链接请按服务器和端口分别添加").into());
                }
                origin = Some(current);
            }
        }
        Ok(())
    }

    pub fn save_defaults(&self, engine: &swoop_core::Engine) -> Result<(), String> {
        let url = if self.options.tls == FtpTlsMode::Implicit {
            "ftps://example.test/file"
        } else {
            "ftp://example.test/file"
        };
        let (mut options, _) = self.prepare(url)?;
        options.username = None;
        engine.set_ftp_defaults(options).map_err(|e| e.to_string())
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, implicit: bool) {
        ui.horizontal(|ui| {
            ui.label(swoop_i18n::tr!("用户名"));
            ui.add(
                egui::TextEdit::singleline(&mut self.username)
                    .hint_text(swoop_i18n::tr!("留空为匿名"))
                    .desired_width(150.0),
            );
            ui.label(swoop_i18n::tr!("密码"));
            ui.add(egui::TextEdit::singleline(&mut self.password).password(true).desired_width(130.0));
        });
        egui::CollapsingHeader::new(swoop_i18n::tr!("FTP 连接设置")).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("TLS");
                if implicit {
                    ui.label(swoop_i18n::tr!("隐式 TLS（FTPS）"));
                } else {
                    for (mode, label) in [
                        (FtpTlsMode::Plain, swoop_i18n::tr!("无")),
                        (FtpTlsMode::Explicit, swoop_i18n::tr!("显式")),
                        (FtpTlsMode::Implicit, swoop_i18n::tr!("隐式")),
                    ] {
                        ui.selectable_value(&mut self.options.tls, mode, label);
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(swoop_i18n::tr!("数据连接"));
                let mut active = matches!(self.options.config.data_mode, FtpDataMode::Active(_));
                let previous = active;
                ui.selectable_value(&mut active, false, swoop_i18n::tr!("被动"));
                ui.selectable_value(&mut active, true, swoop_i18n::tr!("主动"));
                if previous != active {
                    self.options.config.data_mode =
                        if active { FtpDataMode::Active(ActiveMode::default()) } else { FtpDataMode::Passive };
                }
            });
            if let FtpDataMode::Active(active) = &mut self.options.config.data_mode {
                ui.horizontal(|ui| {
                    ui.label(swoop_i18n::tr!("本机地址"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.active_address)
                            .hint_text(swoop_i18n::tr!("自动"))
                            .desired_width(160.0),
                    );
                    let mut fixed = active.port_range.is_some();
                    if ui.checkbox(&mut fixed, swoop_i18n::tr!("限定端口")).changed() {
                        active.port_range = fixed.then_some((41000, 41010));
                    }
                });
                if let Some((first, last)) = &mut active.port_range {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(first).range(1..=65535));
                        ui.label("–");
                        ui.add(egui::DragValue::new(last).range(1..=65535));
                    });
                }
            }
            ui.horizontal(|ui| {
                ui.label(swoop_i18n::tr!("代理"));
                let mut choice = match self.options.proxy {
                    FtpProxyPolicy::Network => 0,
                    FtpProxyPolicy::Direct => 1,
                    FtpProxyPolicy::Custom(_) => 2,
                };
                let previous = choice;
                for (value, label) in [
                    (0, swoop_i18n::tr!("跟随网络设置")),
                    (1, swoop_i18n::tr!("直连")),
                    (2, swoop_i18n::tr!("指定代理")),
                ] {
                    ui.selectable_value(&mut choice, value, label);
                }
                if choice != previous {
                    self.options.proxy = match choice {
                        0 => FtpProxyPolicy::Network,
                        1 => FtpProxyPolicy::Direct,
                        _ => FtpProxyPolicy::Custom(FtpProxy::default()),
                    };
                }
            });
            if let FtpProxyPolicy::Custom(proxy) = &mut self.options.proxy {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("ftp_proxy_kind")
                        .selected_text(match proxy.kind {
                            FtpProxyKind::HttpTunnel => "HTTP",
                            FtpProxyKind::HttpsTunnel => "HTTPS",
                            FtpProxyKind::Socks5 => "SOCKS5",
                            FtpProxyKind::Socks5Hostname => swoop_i18n::tr!("SOCKS5 远程 DNS"),
                        })
                        .show_ui(ui, |ui| {
                            for (kind, label) in [
                                (FtpProxyKind::HttpTunnel, "HTTP"),
                                (FtpProxyKind::HttpsTunnel, "HTTPS"),
                                (FtpProxyKind::Socks5, "SOCKS5"),
                                (FtpProxyKind::Socks5Hostname, swoop_i18n::tr!("SOCKS5 远程 DNS")),
                            ] {
                                ui.selectable_value(&mut proxy.kind, kind, label);
                            }
                        });
                    ui.add(egui::TextEdit::singleline(&mut proxy.host).desired_width(130.0));
                    ui.add(egui::DragValue::new(&mut proxy.port).range(1..=65535));
                });
                ui.add(
                    egui::TextEdit::singleline(&mut self.bypass)
                        .hint_text(swoop_i18n::tr!("不走代理的主机，以逗号分隔"))
                        .desired_width(f32::INFINITY),
                );
            }
            ui.horizontal(|ui| {
                ui.label(swoop_i18n::tr!("连接超时"));
                ui.add(
                    egui::DragValue::new(&mut self.options.config.connect_timeout_secs)
                        .range(1..=86400)
                        .suffix(swoop_i18n::tr!(" 秒")),
                );
                ui.label(swoop_i18n::tr!("读取超时"));
                ui.add(
                    egui::DragValue::new(&mut self.options.config.read_timeout_secs)
                        .range(1..=86400)
                        .suffix(swoop_i18n::tr!(" 秒")),
                );
            });
            ui.add(
                egui::TextEdit::singleline(&mut self.ca)
                    .hint_text(swoop_i18n::tr!("自定义 CA 证书路径（可选）"))
                    .desired_width(f32::INFINITY),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_restart_without_login_and_private_batches_are_origin_scoped() {
        let store = std::sync::Arc::new(Store::in_memory().unwrap());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let engine = swoop_core::Engine::with_store(runtime.handle().clone(), Default::default(), store.clone());
        let mut editor = FtpEditor::default();
        editor.options.proxy = FtpProxyPolicy::Direct;
        editor.options.tls = FtpTlsMode::Explicit;
        editor.username = "user-fixture".into();
        editor.password = "secret-fixture".into();
        editor.save_defaults(&engine).unwrap();
        let saved = store.get_setting(KEY).unwrap().unwrap();
        assert!(!saved.contains("fixture"));
        let loaded = FtpEditor::load(&store);
        assert!(loaded.username.is_empty() && loaded.password.is_empty());
        assert_eq!(loaded.options.tls, FtpTlsMode::Explicit);
        assert!(editor.validate_batch(&["ftp://one.test/a".into(), "ftp://one.test/b".into()]).is_ok());
        assert!(editor.validate_batch(&["ftp://one.test/a".into(), "ftp://two.test/b".into()]).is_err());
        assert!(editor.validate_batch(&["ftp://one.test/a".into(), "ftp://one.test:22/b".into()]).is_err());
        assert_eq!(editor.prepare("ftps://one.test/a").unwrap().0.tls, FtpTlsMode::Implicit);
    }
    #[test]
    fn invalid_defaults_and_dialog_login_reset_are_safe() {
        let store = Store::in_memory().unwrap();
        store.set_setting(KEY, "{broken").unwrap();
        assert_eq!(FtpEditor::load(&store).options, FtpTaskOptions::default());
        let mut editor = FtpEditor { password: "fixture".into(), ..Default::default() };
        assert!(editor.prepare("ftp://one.test/a").is_err());
        editor.username = "user".into();
        editor.clear_credentials();
        assert!(editor.password.is_empty());
        assert!(editor.prepare("ftp://one.test/a").unwrap().1.is_none());
    }
}
