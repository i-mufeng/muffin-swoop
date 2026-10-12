use crate::media_tasks::{MediaNetworkMode, MediaPreferences};
use swoop_core::{AuthSecret, CredentialStorage, ProxyAuthRule, ProxyAuthScheme, ProxyCredentials};
use zeroize::{Zeroize, Zeroizing};

#[derive(Default)]
pub(super) struct ProxyLoginForm {
    enabled: bool,
    scheme: ProxyAuthScheme,
    username: String,
    endpoint: String,
    password: Zeroizing<String>,
    changed: bool,
    original: Option<ProxyAuthRule>,
}
impl ProxyLoginForm {
    pub fn from_preferences(preferences: &MediaPreferences) -> Self {
        let rule = preferences.network.proxy_auth.clone();
        Self {
            enabled: rule.is_some(),
            scheme: rule.as_ref().map(|r| r.scheme).unwrap_or_default(),
            username: rule.as_ref().map(|r| r.username.clone()).unwrap_or_default(),
            endpoint: preferences.network.proxy.as_ref().map(|p| p.url.clone()).unwrap_or_default(),
            original: rule,
            ..Default::default()
        }
    }
    fn clear_password(&mut self) {
        self.password.zeroize();
        self.changed = false;
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, endpoint: &str) {
        if self.endpoint != endpoint {
            self.endpoint = endpoint.to_owned();
            self.clear_password();
        }
        ui.label(swoop_i18n::tr!("代理认证"));
        if crate::ui::checkbox(ui, &mut self.enabled, swoop_i18n::tr!("仅本次会话")).changed() {
            self.clear_password();
        }
        ui.end_row();
        if !self.enabled {
            return;
        }
        let old_identity = (self.scheme, self.username.clone());
        ui.label(swoop_i18n::tr!("认证方式"));
        let socks = endpoint.starts_with("socks5:") || endpoint.starts_with("socks5h:");
        if socks {
            self.scheme = ProxyAuthScheme::Socks5;
            ui.label("SOCKS5");
        } else {
            if self.scheme == ProxyAuthScheme::Socks5 {
                self.scheme = ProxyAuthScheme::Basic;
            }
            egui::ComboBox::from_id_salt("media_proxy_auth_scheme")
                .selected_text(format!("{:?}", self.scheme))
                .show_ui(ui, |ui| {
                    for scheme in [
                        ProxyAuthScheme::Basic,
                        ProxyAuthScheme::Digest,
                        ProxyAuthScheme::Ntlm,
                        ProxyAuthScheme::Negotiate,
                    ] {
                        ui.selectable_value(&mut self.scheme, scheme, format!("{scheme:?}"));
                    }
                });
        }
        ui.end_row();
        ui.label(swoop_i18n::tr!("账号"));
        ui.text_edit_singleline(&mut self.username);
        ui.end_row();
        if old_identity != (self.scheme, self.username.clone()) {
            self.clear_password();
        }
        ui.label(swoop_i18n::tr!("密码"));
        if ui
            .add(
                egui::TextEdit::singleline(&mut *self.password)
                    .password(true)
                    .hint_text(swoop_i18n::tr!("留空保留原凭据")),
            )
            .changed()
        {
            self.changed = true;
        }
        ui.end_row();
    }
    pub fn apply(&self, preferences: &mut MediaPreferences) -> Result<Option<ProxyCredentials>, String> {
        if preferences.network_mode != MediaNetworkMode::Manual {
            return Ok(None);
        }
        if !self.enabled {
            preferences.network.proxy_auth = None;
            return Ok(None);
        }
        let endpoint = &preferences.network.proxy.as_ref().ok_or(swoop_i18n::tr!("请填写媒体代理地址"))?.url;
        if &self.endpoint != endpoint {
            return Err(swoop_i18n::tr!("代理地址已变更，请重新输入本次代理凭据").into());
        }
        let rule = ProxyAuthRule::new(endpoint, self.scheme, self.username.clone(), CredentialStorage::Session)
            .map_err(|error| error.to_string())?;
        rule.validate().map_err(|error| error.to_string())?;
        preferences.network.proxy_auth = Some(rule.clone());
        if self.original.as_ref() == Some(&rule) && !self.changed {
            return Ok(None);
        }
        ProxyCredentials::session(rule, AuthSecret::new(self.password.to_string()).map_err(|error| error.to_string())?)
            .map(Some)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edited_proxy_never_reuses_password_and_serialized_preferences_contain_no_secret() {
        let mut preferences = MediaPreferences { network_mode: MediaNetworkMode::Manual, ..Default::default() };
        preferences.network.proxy =
            Some(swoop_media::MediaProxyConfig { url: "http://127.0.0.1:7890".into(), bypass: vec![] });
        let mut form = ProxyLoginForm::from_preferences(&preferences);
        form.enabled = true;
        form.username = "proxy-user".into();
        *form.password = "do-not-persist-proxy-secret".into();
        form.changed = true;
        let credentials = form.apply(&mut preferences).unwrap().unwrap();
        assert_eq!(credentials.rule(), preferences.network.proxy_auth.as_ref().unwrap());
        assert!(!serde_json::to_string(&preferences).unwrap().contains("do-not-persist"));
        let unchanged = ProxyLoginForm::from_preferences(&preferences);
        assert!(unchanged.apply(&mut preferences).unwrap().is_none());
        preferences.network.proxy.as_mut().unwrap().url = "http://another.test:8080".into();
        assert!(form.apply(&mut preferences).is_err());
        let context = egui::Context::default();
        let mut output = context.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                egui::Grid::new("proxy-test").show(ui, |ui| form.ui(ui, "http://another.test:8080"));
            });
        });
        output.textures_delta.clear();
        assert!(form.password.is_empty());
        assert!(!form.changed);
    }
}
