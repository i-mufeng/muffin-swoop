use std::sync::mpsc;

use super::dialog;
use swoop_core::{
    AuthCapabilities, AuthSecret, CredentialStorage, Engine, HttpAuthScheme, SiteAuthRule, auth_capabilities,
};
use zeroize::{Zeroize, Zeroizing};

use crate::app::SwoopApp;

type JobResult = Result<Vec<SiteAuthRule>, String>;

pub struct SiteAuthEditor {
    rules: Vec<SiteAuthRule>,
    original: Option<SiteAuthRule>,
    origin: String,
    scheme: HttpAuthScheme,
    username: String,
    password: Zeroizing<String>,
    password_changed: bool,
    storage: CredentialStorage,
    allow_insecure: bool,
    error: Option<String>,
    pending: Option<mpsc::Receiver<JobResult>>,
    capabilities: AuthCapabilities,
}
impl SiteAuthEditor {
    pub fn new(engine: &Engine) -> Self {
        Self {
            rules: engine.site_auth_rules(),
            original: None,
            origin: String::new(),
            scheme: HttpAuthScheme::Digest,
            username: String::new(),
            password: Zeroizing::new(String::new()),
            password_changed: false,
            storage: CredentialStorage::Session,
            allow_insecure: false,
            error: None,
            pending: None,
            capabilities: auth_capabilities(),
        }
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    fn select(&mut self, rule: Option<SiteAuthRule>) {
        self.password.zeroize();
        self.password_changed = false;
        self.error = None;
        if let Some(rule) = &rule {
            self.origin.clone_from(&rule.origin);
            self.scheme = rule.scheme;
            self.username.clone_from(&rule.username);
            self.storage = rule.storage;
            self.allow_insecure = rule.allow_insecure_http;
        } else {
            self.origin.clear();
            self.username.clear();
            self.scheme = HttpAuthScheme::Digest;
            self.storage = CredentialStorage::Session;
            self.allow_insecure = false;
        }
        self.original = rule;
    }
    fn credentials_unchanged(&self) -> bool {
        self.original.as_ref().is_some_and(|rule| {
            rule.origin == self.origin.trim()
                && rule.username == self.username
                && rule.scheme == self.scheme
                && rule.storage == self.storage
        })
    }
    fn clear_password(&mut self) {
        self.password.zeroize();
        self.password_changed = false;
    }
    fn draft(&mut self) -> Result<(SiteAuthRule, Option<AuthSecret>), String> {
        let mut rule = SiteAuthRule::new(self.origin.trim(), self.scheme, self.username.clone(), self.storage)
            .map_err(|error| error.to_string())?;
        rule.allow_insecure_http = self.allow_insecure;
        rule.validate().map_err(|error| error.to_string())?;
        let secret = if self.storage == CredentialStorage::CurrentUser
            || (!self.password_changed && self.credentials_unchanged())
        {
            None
        } else {
            Some(AuthSecret::new(std::mem::take(&mut *self.password)).map_err(|error| error.to_string())?)
        };
        Ok((rule, secret))
    }
    fn start(&mut self, engine: Engine, ctx: egui::Context, action: Action) {
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.error = None;
        std::thread::spawn(move || {
            let result = match action {
                Action::Save(rule, secret) => engine.set_site_auth(rule, secret),
                Action::Remove(origin) => engine.remove_site_auth(&origin),
            }
            .map(|()| engine.site_auth_rules())
            .map_err(|error| error.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
}
enum Action {
    Save(SiteAuthRule, Option<AuthSecret>),
    Remove(String),
}

impl SwoopApp {
    pub fn site_auth_logic(&mut self, _ctx: &egui::Context) {
        let Some(editor) = &mut self.site_auth_editor else {
            return;
        };
        let Some(pending) = &editor.pending else {
            return;
        };
        let result = match pending.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(swoop_i18n::tr!("站点认证保存线程异常退出").into()),
        };
        editor.pending = None;
        match result {
            Ok(rules) => {
                let selected = rules.iter().find(|rule| rule.origin == editor.origin.trim()).cloned();
                editor.rules = rules;
                editor.select(selected);
                self.toasts.push_info(swoop_i18n::tr!("站点认证已保存"), "");
            }
            Err(error) => editor.error = Some(error),
        }
    }

    pub fn site_auth_ui(&mut self, ctx: &egui::Context) {
        let Some(editor) = &mut self.site_auth_editor else {
            return;
        };
        let p = self.palette;
        let busy = editor.pending.is_some();
        let mut save = false;
        let mut remove = false;
        let mut close = false;
        let modal = dialog::modal("site_auth_settings", &p).show(ctx, |ui| {
            ui.set_width(580.0);
            close |= dialog::header(ui, &p, egui_phosphor::regular::KEY, swoop_i18n::tr!("站点认证"), None);
            dialog::body(ui, "site_auth_body", |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("site_auth_select")
                            .selected_text(
                                editor
                                    .original
                                    .as_ref()
                                    .map(|r| r.origin.as_str())
                                    .unwrap_or(swoop_i18n::tr!("添加站点")),
                            )
                            .width(ui.available_width() - 100.0)
                            .show_ui(ui, |ui| {
                                let rules = editor.rules.clone();
                                for rule in rules {
                                    if ui
                                        .selectable_label(editor.original.as_ref() == Some(&rule), &rule.origin)
                                        .clicked()
                                    {
                                        editor.select(Some(rule));
                                    }
                                }
                            });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if super::ghost_button(ui, &p, &swoop_i18n::trf!("{} 新增", egui_phosphor::regular::PLUS))
                                .clicked()
                            {
                                editor.select(None);
                            }
                        });
                    });
                    ui.add_space(12.0);
                    let identity = (editor.origin.clone(), editor.username.clone(), editor.scheme, editor.storage);
                    dialog::group(ui, &p, None, |ui| {
                        dialog::row(ui, &p, swoop_i18n::tr!("站点"), None, |ui| {
                            ui.add(
                                super::field(&mut editor.origin).desired_width(320.0).hint_text("https://example.com"),
                            );
                        });
                        dialog::row(ui, &p, swoop_i18n::tr!("认证方式"), None, |ui| {
                            super::segmented_with(
                                ui,
                                &p,
                                &mut editor.scheme,
                                &[
                                    (HttpAuthScheme::Basic, "Basic", true),
                                    (HttpAuthScheme::Digest, "Digest", true),
                                    (HttpAuthScheme::Ntlm, "NTLM", editor.capabilities.ntlm),
                                    (HttpAuthScheme::Negotiate, "Negotiate", editor.capabilities.negotiate),
                                ],
                            );
                        });
                        dialog::row(ui, &p, swoop_i18n::tr!("保存方式"), None, |ui| {
                            let current = editor.scheme == HttpAuthScheme::Negotiate
                                || (editor.scheme == HttpAuthScheme::Ntlm && editor.capabilities.sspi);
                            if !current && editor.storage == CredentialStorage::CurrentUser {
                                editor.storage = CredentialStorage::Session;
                            }
                            let changed = super::segmented_with(
                                ui,
                                &p,
                                &mut editor.storage,
                                &[
                                    (CredentialStorage::Session, swoop_i18n::tr!("本次会话"), true),
                                    (
                                        CredentialStorage::System,
                                        swoop_i18n::tr!("系统凭据库"),
                                        editor.capabilities.system_vault,
                                    ),
                                    (CredentialStorage::CurrentUser, swoop_i18n::tr!("当前系统身份"), current),
                                ],
                            )
                            .changed();
                            if changed && editor.storage == CredentialStorage::CurrentUser {
                                editor.username.clear();
                            }
                        });
                        if editor.storage != CredentialStorage::CurrentUser {
                            dialog::row(ui, &p, swoop_i18n::tr!("用户名"), None, |ui| {
                                ui.add(
                                    super::field(&mut editor.username)
                                        .desired_width(320.0)
                                        .hint_text(swoop_i18n::tr!("用户名或 域\\用户名")),
                                );
                            });
                            if identity
                                != (editor.origin.clone(), editor.username.clone(), editor.scheme, editor.storage)
                            {
                                editor.clear_password();
                            }
                            dialog::row(ui, &p, swoop_i18n::tr!("密码"), None, |ui| {
                                let hint = if editor.credentials_unchanged() {
                                    swoop_i18n::tr!("留空保留已有密码")
                                } else {
                                    swoop_i18n::tr!("输入密码")
                                };
                                if ui
                                    .add(
                                        super::field(&mut *editor.password)
                                            .password(true)
                                            .desired_width(320.0)
                                            .hint_text(hint),
                                    )
                                    .changed()
                                {
                                    editor.password_changed = true;
                                }
                            });
                        } else {
                            editor.clear_password();
                        }
                        if editor.origin.trim().to_ascii_lowercase().starts_with("http://") {
                            dialog::row(ui, &p, swoop_i18n::tr!("允许此站点使用非加密 HTTP"), None, |ui| {
                                super::toggle(ui, &p, &mut editor.allow_insecure);
                            });
                        }
                    });
                });
                if let Some(error) = &editor.error {
                    dialog::error(ui, &p, error);
                }
            });
            dialog::footer(ui, &p, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    save = super::primary_button(ui, &p, swoop_i18n::tr!("保存")).clicked();
                    close |= super::ghost_button(ui, &p, swoop_i18n::tr!("关闭")).clicked();
                });
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    if busy {
                        ui.spinner();
                        dialog::note(ui, &p, swoop_i18n::tr!("正在保存…"));
                    } else if editor.original.is_some() {
                        remove =
                            super::danger_button(ui, &p, &swoop_i18n::trf!("{} 移除", egui_phosphor::regular::TRASH))
                                .clicked();
                    }
                });
            });
        });
        if save {
            match editor.draft() {
                Ok((rule, secret)) => {
                    editor.start(self.engine.as_ref().clone(), ctx.clone(), Action::Save(rule, secret))
                }
                Err(error) => editor.error = Some(error),
            }
        } else if remove {
            if let Some(rule) = &editor.original {
                editor.start(self.engine.as_ref().clone(), ctx.clone(), Action::Remove(rule.origin.clone()));
            }
        } else if (close || modal.should_close()) && !busy {
            self.site_auth_editor = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn editor() -> SiteAuthEditor {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        SiteAuthEditor::new(&Engine::new(runtime.handle().clone(), Default::default()))
    }
    #[test]
    fn selecting_another_origin_clears_draft_password() {
        let mut editor = editor();
        *editor.password = "private-marker".into();
        editor.password_changed = true;
        let rule =
            SiteAuthRule::new("https://second.example", HttpAuthScheme::Basic, "u".into(), CredentialStorage::Session)
                .unwrap();
        editor.select(Some(rule));
        assert!(editor.password.is_empty());
        assert!(!editor.password_changed);
    }
    #[test]
    fn unchanged_rule_retains_password_but_new_identity_never_does() {
        let mut editor = editor();
        editor.select(Some(
            SiteAuthRule::new("https://first.example", HttpAuthScheme::Digest, "u".into(), CredentialStorage::Session)
                .unwrap(),
        ));
        assert!(editor.draft().unwrap().1.is_none());
        editor.username = "another".into();
        assert!(editor.draft().unwrap().1.is_some());
    }
}
