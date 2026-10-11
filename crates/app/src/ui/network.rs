use crate::app::SwoopApp;
use egui::{Frame, Margin, RichText};
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
        let modal = egui::Modal::new(egui::Id::new("network_policy"))
            .frame(Frame::window(&ctx.style_of(ctx.theme())).fill(p.surface).inner_margin(Margin::same(22)))
            .show(ctx, |ui| {
                ui.set_width(510.0);
                ui.label(RichText::new(swoop_i18n::tr!("网络与重试")).size(18.0).strong());
                ui.label(RichText::new(swoop_i18n::tr!("新建或继续下载时生效")).size(12.0).color(p.text_muted));
                super::modal_body(ui, "network_body", 190.0, |ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        egui::Grid::new("network_grid").num_columns(2).spacing([18.0, 12.0]).show(ui, |ui| {
                            ui.label(swoop_i18n::tr!("连接超时"));
                            ui.add(
                                egui::DragValue::new(&mut draft.connect_timeout_secs)
                                    .range(1..=86400)
                                    .suffix(swoop_i18n::tr!(" 秒")),
                            );
                            ui.end_row();
                            ui.label(swoop_i18n::tr!("读取超时"));
                            ui.add(
                                egui::DragValue::new(&mut draft.read_timeout_secs)
                                    .range(1..=86400)
                                    .suffix(swoop_i18n::tr!(" 秒")),
                            );
                            ui.end_row();
                            ui.label(swoop_i18n::tr!("请求总时长"));
                            ui.horizontal(|ui| {
                                let mut enabled = draft.request_timeout_secs.is_some();
                                if ui.checkbox(&mut enabled, swoop_i18n::tr!("限制")).changed() {
                                    draft.request_timeout_secs = enabled.then_some(3600);
                                }
                                if let Some(seconds) = &mut draft.request_timeout_secs {
                                    ui.add(
                                        egui::DragValue::new(seconds).range(1..=86400).suffix(swoop_i18n::tr!(" 秒")),
                                    );
                                }
                            });
                            ui.end_row();
                            ui.label(swoop_i18n::tr!("失败重试"));
                            ui.add(
                                egui::DragValue::new(&mut draft.retries).range(0..=100).suffix(swoop_i18n::tr!(" 次")),
                            );
                            ui.end_row();
                            ui.label(swoop_i18n::tr!("首次重试间隔"));
                            ui.add(
                                egui::DragValue::new(&mut draft.retry_delay_ms)
                                    .range(1..=3_600_000)
                                    .suffix(swoop_i18n::tr!(" 毫秒")),
                            );
                            ui.end_row();
                            ui.label(swoop_i18n::tr!("最大重试间隔"));
                            ui.add(
                                egui::DragValue::new(&mut draft.retry_max_delay_ms)
                                    .range(1..=3_600_000)
                                    .suffix(swoop_i18n::tr!(" 毫秒")),
                            );
                            ui.end_row();
                            ui.label(swoop_i18n::tr!("代理"));
                            ui.horizontal(|ui| {
                                let mut mode = if draft.proxy.is_some() {
                                    2
                                } else if draft.use_system_proxy {
                                    1
                                } else {
                                    0
                                };
                                let old = mode;
                                for (id, label) in [
                                    (0, swoop_i18n::tr!("直连")),
                                    (1, swoop_i18n::tr!("系统")),
                                    (2, swoop_i18n::tr!("手动")),
                                ] {
                                    ui.selectable_value(&mut mode, id, label);
                                }
                                if mode != old {
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
                            ui.end_row();
                            if let Some(proxy) = &mut draft.proxy {
                                ui.label(swoop_i18n::tr!("地址"));
                                ui.add(
                                    egui::TextEdit::singleline(&mut proxy.url)
                                        .desired_width(340.0)
                                        .hint_text(swoop_i18n::tr!("http:// 或 socks5h://")),
                                );
                                ui.end_row();
                                ui.label(swoop_i18n::tr!("不使用代理"));
                                if ui
                                    .add(
                                        egui::TextEdit::singleline(&mut self.network_bypass)
                                            .desired_width(340.0)
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
                                ui.end_row();
                                ui.label(swoop_i18n::tr!("代理认证"));
                                let mut enabled = draft.proxy_auth.is_some();
                                if ui.checkbox(&mut enabled, swoop_i18n::tr!("启用")).changed() {
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
                                ui.end_row();
                                if let Some(rule) = &mut draft.proxy_auth {
                                    if let Ok(endpoint) = url::Url::parse(proxy.url.trim()) {
                                        rule.endpoint = endpoint.to_string();
                                    }
                                    ui.label(swoop_i18n::tr!("认证方式"));
                                    egui::ComboBox::from_id_salt("proxy_auth_scheme")
                                        .selected_text(format!("{:?}", rule.scheme))
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
                                    ui.end_row();
                                    ui.label(swoop_i18n::tr!("凭据保存"));
                                    egui::ComboBox::from_id_salt("proxy_credential_storage")
                                        .selected_text(match rule.storage {
                                            CredentialStorage::Session => swoop_i18n::tr!("仅本次会话"),
                                            CredentialStorage::System => swoop_i18n::tr!("系统凭据库"),
                                            CredentialStorage::CurrentUser => swoop_i18n::tr!("当前系统身份"),
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut rule.storage,
                                                CredentialStorage::Session,
                                                swoop_i18n::tr!("仅本次会话"),
                                            );
                                            ui.selectable_value(
                                                &mut rule.storage,
                                                CredentialStorage::System,
                                                swoop_i18n::tr!("系统凭据库"),
                                            );
                                            ui.selectable_value(
                                                &mut rule.storage,
                                                CredentialStorage::CurrentUser,
                                                swoop_i18n::tr!("当前系统身份"),
                                            );
                                        });
                                    ui.end_row();
                                    if rule.storage == CredentialStorage::CurrentUser {
                                        rule.username.clear();
                                    } else {
                                        ui.label(swoop_i18n::tr!("用户名"));
                                        ui.text_edit_singleline(&mut rule.username);
                                        ui.end_row();
                                        ui.label(swoop_i18n::tr!("密码"));
                                        if ui
                                            .add(
                                                egui::TextEdit::singleline(&mut *editor.password)
                                                    .password(true)
                                                    .hint_text(swoop_i18n::tr!("留空保留原凭据")),
                                            )
                                            .changed()
                                        {
                                            editor.password_changed = true;
                                        }
                                        ui.end_row();
                                    }
                                }
                            }
                        });
                    });
                    if let Some(error) = &self.network_error {
                        ui.colored_label(p.danger, error);
                    }
                });
                ui.add_space(14.0);
                if busy {
                    ui.spinner();
                }
                ui.add_enabled_ui(!busy, |ui| {
                    save = super::primary_button(ui, &p, swoop_i18n::tr!("保存")).clicked();
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
        } else if modal.should_close() && !busy {
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
