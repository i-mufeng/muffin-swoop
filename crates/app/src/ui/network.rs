use super::dialog;
use crate::app::SwoopApp;
use std::sync::mpsc;
use swoop_core::{AuthSecret, CredentialStorage, NetworkPolicy, ProxyAuthRule, ProxyAuthScheme};
use zeroize::{Zeroize, Zeroizing};

pub struct NetworkEditor {
    pub policy: NetworkPolicy,
    original: Option<ProxyAuthRule>,
    password: Zeroizing<String>,
    password_changed: bool,
    pending: Option<mpsc::Receiver<Result<(), String>>>,
}
impl NetworkEditor {
    pub fn new(policy: NetworkPolicy) -> Self {
        Self {
            original: policy.proxy_auth.clone(),
            policy,
            password: Zeroizing::new(String::new()),
            password_changed: false,
            pending: None,
        }
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    fn clear_password(&mut self) {
        self.password.zeroize();
        self.password_changed = false;
    }
    fn secret(&mut self) -> Result<Option<AuthSecret>, String> {
        let Some(rule) = &self.policy.proxy_auth else {
            self.clear_password();
            return Ok(None);
        };
        let unchanged = self.original.as_ref().is_some_and(|old| {
            old.endpoint == rule.endpoint
                && old.scheme == rule.scheme
                && old.username == rule.username
                && old.storage == rule.storage
        });
        if rule.storage == CredentialStorage::CurrentUser || (unchanged && !self.password_changed) {
            return Ok(None);
        }
        AuthSecret::new(self.password.to_string()).map(Some).map_err(|error| error.to_string())
    }
}

impl SwoopApp {
    pub fn network_logic(&mut self) {
        let Some(editor) = &mut self.network_editor else {
            return;
        };
        let Some(pending) = &editor.pending else {
            return;
        };
        let result = match pending.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(swoop_i18n::tr!("网络设置保存线程异常退出").into()),
        };
        editor.pending = None;
        match result {
            Ok(()) => {
                self.network_editor = None;
                self.network_error = None;
                self.toasts.push_info(swoop_i18n::tr!("网络设置已保存"), "");
            }
            Err(error) => self.network_error = Some(error),
        }
    }
    pub fn network_ui(&mut self, ctx: &egui::Context) {
        let Some(editor) = &mut self.network_editor else {
            return;
        };
        let busy = editor.busy();
        let previous_identity = editor.policy.proxy_auth.clone();
        let draft = &mut editor.policy;
        let p = self.palette;
        let mut save = false;
        let mut close = false;
        let modal = dialog::modal("network_policy", &p).show(ctx, |ui| {
            ui.set_width(560.0);
            close |= dialog::header(
                ui,
                &p,
                egui_phosphor::regular::WIFI_HIGH,
                swoop_i18n::tr!("网络与重试"),
                Some(swoop_i18n::tr!("新建或继续下载时生效")),
            );
            dialog::body(ui, "network_body", |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    let secs = swoop_i18n::tr!(" 秒");
                    let ms = swoop_i18n::tr!(" 毫秒");
                    dialog::group(ui, &p, Some(swoop_i18n::tr!("超时")), |ui| {
                        dialog::row(ui, &p, swoop_i18n::tr!("连接超时"), None, |ui| {
                            ui.add(egui::DragValue::new(&mut draft.connect_timeout_secs).range(1..=86400).suffix(secs));
                        });
                        dialog::row(ui, &p, swoop_i18n::tr!("读取超时"), None, |ui| {
                            ui.add(egui::DragValue::new(&mut draft.read_timeout_secs).range(1..=86400).suffix(secs));
                        });
                        dialog::row(ui, &p, swoop_i18n::tr!("请求总时长"), None, |ui| {
                            let mut enabled = draft.request_timeout_secs.is_some();
                            if super::toggle(ui, &p, &mut enabled).changed() {
                                draft.request_timeout_secs = enabled.then_some(3600);
                            }
                            if let Some(seconds) = &mut draft.request_timeout_secs {
                                ui.add_space(4.0);
                                ui.add(egui::DragValue::new(seconds).range(1..=86400).suffix(secs));
                            }
                        });
                    });
                    dialog::group(ui, &p, Some(swoop_i18n::tr!("重试")), |ui| {
                        dialog::row(ui, &p, swoop_i18n::tr!("失败重试"), None, |ui| {
                            ui.add(
                                egui::DragValue::new(&mut draft.retries).range(0..=100).suffix(swoop_i18n::tr!(" 次")),
                            );
                        });
                        dialog::row(ui, &p, swoop_i18n::tr!("首次重试间隔"), None, |ui| {
                            ui.add(egui::DragValue::new(&mut draft.retry_delay_ms).range(1..=3_600_000).suffix(ms));
                        });
                        dialog::row(ui, &p, swoop_i18n::tr!("最大重试间隔"), None, |ui| {
                            ui.add(egui::DragValue::new(&mut draft.retry_max_delay_ms).range(1..=3_600_000).suffix(ms));
                        });
                    });
                    dialog::group(ui, &p, Some(swoop_i18n::tr!("代理")), |ui| {
                        dialog::row(ui, &p, swoop_i18n::tr!("代理"), None, |ui| {
                            let mut mode = if draft.proxy.is_some() {
                                2
                            } else if draft.use_system_proxy {
                                1
                            } else {
                                0
                            };
                            let changed = super::segmented(
                                ui,
                                &p,
                                &mut mode,
                                &[
                                    (0, swoop_i18n::tr!("直连")),
                                    (1, swoop_i18n::tr!("系统")),
                                    (2, swoop_i18n::tr!("手动")),
                                ],
                            )
                            .changed();
                            if changed {
                                draft.use_system_proxy = mode == 1;
                                draft.proxy_auth = None;
                                draft.proxy = (mode == 2).then(|| swoop_core::ProxyConfig {
                                    url: "http://127.0.0.1:7890".into(),
                                    bypass: vec!["localhost".into(), "127.0.0.1".into()],
                                });
                                self.network_bypass =
                                    draft.proxy.as_ref().map(|p| p.bypass.join(", ")).unwrap_or_default();
                            }
                        });
                        let Some(proxy) = &mut draft.proxy else { return };
                        dialog::row(ui, &p, swoop_i18n::tr!("地址"), None, |ui| {
                            ui.add(
                                super::field(&mut proxy.url)
                                    .desired_width(300.0)
                                    .hint_text(swoop_i18n::tr!("http:// 或 socks5h://")),
                            );
                        });
                        dialog::row(ui, &p, swoop_i18n::tr!("不使用代理"), None, |ui| {
                            if ui
                                .add(
                                    super::field(&mut self.network_bypass)
                                        .desired_width(300.0)
                                        .hint_text(swoop_i18n::tr!("主机或 IP，以逗号分隔")),
                                )
                                .changed()
                            {
                                proxy.bypass = self
                                    .network_bypass
                                    .split(',')
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .map(str::to_string)
                                    .collect();
                            }
                        });
                        dialog::row(ui, &p, swoop_i18n::tr!("代理认证"), None, |ui| {
                            let mut enabled = draft.proxy_auth.is_some();
                            if super::toggle(ui, &p, &mut enabled).changed() {
                                draft.proxy_auth = if enabled {
                                    let scheme = if proxy.url.trim().starts_with("socks5") {
                                        ProxyAuthScheme::Socks5
                                    } else {
                                        ProxyAuthScheme::Basic
                                    };
                                    match ProxyAuthRule::new(
                                        &proxy.url,
                                        scheme,
                                        String::new(),
                                        CredentialStorage::Session,
                                    ) {
                                        Ok(rule) => Some(rule),
                                        Err(error) => {
                                            self.network_error = Some(error.to_string());
                                            None
                                        }
                                    }
                                } else {
                                    None
                                };
                            }
                        });
                        let Some(rule) = &mut draft.proxy_auth else { return };
                        if let Ok(endpoint) = url::Url::parse(proxy.url.trim()) {
                            rule.endpoint = endpoint.to_string();
                        }
                        dialog::row(ui, &p, swoop_i18n::tr!("认证方式"), None, |ui| {
                            egui::ComboBox::from_id_salt("proxy_auth_scheme")
                                .selected_text(format!("{:?}", rule.scheme))
                                .width(180.0)
                                .show_ui(ui, |ui| {
                                    for scheme in [
                                        ProxyAuthScheme::Basic,
                                        ProxyAuthScheme::Digest,
                                        ProxyAuthScheme::Ntlm,
                                        ProxyAuthScheme::Negotiate,
                                        ProxyAuthScheme::Socks5,
                                    ] {
                                        ui.selectable_value(&mut rule.scheme, scheme, format!("{scheme:?}"));
                                    }
                                });
                        });
                        dialog::row(ui, &p, swoop_i18n::tr!("凭据保存"), None, |ui| {
                            let storage_label = |storage| match storage {
                                CredentialStorage::Session => swoop_i18n::tr!("仅本次会话"),
                                CredentialStorage::System => swoop_i18n::tr!("系统凭据库"),
                                CredentialStorage::CurrentUser => swoop_i18n::tr!("当前系统身份"),
                            };
                            egui::ComboBox::from_id_salt("proxy_credential_storage")
                                .selected_text(storage_label(rule.storage))
                                .width(180.0)
                                .show_ui(ui, |ui| {
                                    for storage in [
                                        CredentialStorage::Session,
                                        CredentialStorage::System,
                                        CredentialStorage::CurrentUser,
                                    ] {
                                        ui.selectable_value(&mut rule.storage, storage, storage_label(storage));
                                    }
                                });
                        });
                        if rule.storage == CredentialStorage::CurrentUser {
                            rule.username.clear();
                        } else {
                            dialog::row(ui, &p, swoop_i18n::tr!("用户名"), None, |ui| {
                                ui.add(super::field(&mut rule.username).desired_width(220.0));
                            });
                            dialog::row(ui, &p, swoop_i18n::tr!("密码"), None, |ui| {
                                if ui
                                    .add(
                                        super::field(&mut *editor.password)
                                            .desired_width(220.0)
                                            .password(true)
                                            .hint_text(swoop_i18n::tr!("留空保留原凭据")),
                                    )
                                    .changed()
                                {
                                    editor.password_changed = true;
                                }
                            });
                        }
                    });
                });
                if let Some(error) = &self.network_error {
                    dialog::error(ui, &p, error);
                }
            });
            dialog::footer(ui, &p, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    save = super::primary_button(ui, &p, swoop_i18n::tr!("保存")).clicked();
                    close |= super::ghost_button(ui, &p, swoop_i18n::tr!("取消")).clicked();
                });
                if busy {
                    ui.spinner();
                }
            });
        });
        if previous_identity != editor.policy.proxy_auth {
            editor.clear_password();
        }
        if save && !busy {
            // Engine validates the policy after removing any stale system-vault reference.
            let result = editor.secret();
            match result {
                Ok(secret) => {
                    let (tx, rx) = mpsc::channel();
                    editor.pending = Some(rx);
                    self.network_error = None;
                    let engine = self.engine.clone();
                    let policy = editor.policy.clone();
                    let ctx = ctx.clone();
                    std::thread::spawn(move || {
                        let result = engine.save_network_policy(policy, secret).map_err(|error| error.to_string());
                        let _ = tx.send(result);
                        ctx.request_repaint();
                    });
                }
                Err(error) => self.network_error = Some(error),
            }
        } else if (close || modal.should_close()) && !busy {
            self.network_editor = None;
            self.network_error = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn editor() -> NetworkEditor {
        NetworkEditor::new(NetworkPolicy {
            proxy: Some(swoop_core::ProxyConfig { url: "http://127.0.0.1:7890".into(), bypass: vec![] }),
            proxy_auth: Some(
                ProxyAuthRule::new(
                    "http://127.0.0.1:7890",
                    ProxyAuthScheme::Basic,
                    "user".into(),
                    CredentialStorage::Session,
                )
                .unwrap(),
            ),
            ..Default::default()
        })
    }
    #[test]
    fn unchanged_proxy_identity_retains_credentials_and_changed_identity_requires_new_secret() {
        let mut editor = editor();
        assert!(editor.secret().unwrap().is_none());
        editor.policy.proxy_auth.as_mut().unwrap().username = "other-user".into();
        assert!(editor.secret().unwrap().is_some());
        editor.policy.proxy_auth = None;
        *editor.password = "draft-secret".into();
        editor.password_changed = true;
        assert!(editor.secret().unwrap().is_none());
        assert!(editor.password.is_empty());
    }
    #[test]
    fn failed_save_can_retry_the_same_password_and_clear_removes_draft() {
        let mut editor = editor();
        *editor.password = "draft-secret".into();
        editor.password_changed = true;
        assert!(editor.secret().unwrap().is_some());
        assert!(editor.secret().unwrap().is_some());
        assert_eq!(editor.password.as_str(), "draft-secret");
        editor.clear_password();
        assert!(editor.password.is_empty());
        assert!(!editor.password_changed);
    }
}
