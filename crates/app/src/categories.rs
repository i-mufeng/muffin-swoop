//! 可配置分类。规则按顺序匹配；已存在的下载文件不随规则变动而移动。
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use swoop_core::Store;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryRule {
    pub name: String,
    pub extensions: String,
    pub domains: String,
    pub directory: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Categories {
    pub enabled: bool,
    pub rules: Vec<CategoryRule>,
}

impl Default for Categories {
    fn default() -> Self {
        Self {
            enabled: false,
            rules: [
                (swoop_i18n::tr!("压缩包"), "zip rar 7z tar.gz tar.xz gz bz2", "Archives"),
                (swoop_i18n::tr!("程序"), "exe msi dmg pkg appimage", "Applications"),
                (swoop_i18n::tr!("文档"), "pdf doc docx xls xlsx ppt pptx txt epub", "Documents"),
                (swoop_i18n::tr!("视频"), "mp4 mkv mov avi webm m3u8 mpd", "Video"),
                (swoop_i18n::tr!("音乐"), "mp3 flac wav aac m4a ogg", "Music"),
                (swoop_i18n::tr!("图片"), "jpg jpeg png gif webp svg heic avif", "Pictures"),
            ]
            .into_iter()
            .map(|(name, extensions, directory)| CategoryRule {
                name: name.into(),
                extensions: extensions.into(),
                domains: String::new(),
                directory: directory.into(),
            })
            .collect(),
        }
    }
}

fn tokens(value: &str) -> impl Iterator<Item = &str> {
    value.split(|c: char| c.is_whitespace() || c == ',' || c == ';').filter(|s| !s.is_empty())
}

impl CategoryRule {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.name.chars().count() > 40 {
            return Err(swoop_i18n::tr!("分类名称需要 1–40 个字符").into());
        }
        if self.extensions.len() > 2048 || self.domains.len() > 2048 || self.directory.len() > 4096 {
            return Err(swoop_i18n::tr!("分类规则过长").into());
        }
        if tokens(&self.extensions).next().is_none() && tokens(&self.domains).next().is_none() {
            return Err(swoop_i18n::tr!("请填写扩展名或域名").into());
        }
        for ext in tokens(&self.extensions) {
            if ext.trim_start_matches('.').is_empty()
                || !ext.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
            {
                return Err(swoop_i18n::trf!("扩展名无效：{ext}"));
            }
        }
        for domain in tokens(&self.domains) {
            canonical_domain(domain)?;
        }
        let directory = Path::new(self.directory.trim());
        if self.directory.trim().is_empty()
            || self.directory.chars().any(char::is_control)
            || directory.components().any(|c| matches!(c, Component::ParentDir))
            || self.directory.contains(['<', '>', '|', '?', '*'])
        {
            return Err(swoop_i18n::tr!("保存目录无效，不能含 .. 或通配符").into());
        }
        Ok(())
    }

    fn matches(&self, url: &url::Url, file_name: &str) -> bool {
        let extensions: Vec<_> = tokens(&self.extensions).collect();
        let domains: Vec<_> = tokens(&self.domains).collect();
        let name = file_name.to_lowercase();
        let matches_ext = extensions.is_empty()
            || extensions
                .iter()
                .any(|ext| name.ends_with(&format!(".{}", ext.trim_start_matches('.').to_ascii_lowercase())));
        let host = url.host_str().unwrap_or("");
        let matches_domain = domains.is_empty()
            || domains.iter().any(|domain| {
                canonical_domain(domain).is_ok_and(|(pattern, subdomains)| {
                    host == pattern || (subdomains && host.ends_with(&format!(".{pattern}")))
                })
            });
        matches_ext && matches_domain
    }
}

fn canonical_domain(input: &str) -> Result<(String, bool), String> {
    let subdomains = input.starts_with("*.");
    let domain = input.strip_prefix("*.").unwrap_or(input);
    if domain.is_empty() || domain.contains(['/', '\\', ':', '?', '#', '@', '*']) {
        return Err(swoop_i18n::trf!("域名无效：{input}"));
    }
    let parsed = url::Url::parse(&format!("https://{domain}")).map_err(|_| swoop_i18n::trf!("域名无效：{input}"))?;
    Ok((parsed.host_str().ok_or(swoop_i18n::tr!("域名无效"))?.to_string(), subdomains))
}

impl Categories {
    pub fn validate(&self) -> Result<(), String> {
        if self.rules.len() > 100 {
            return Err(swoop_i18n::tr!("最多支持 100 个分类").into());
        }
        let mut names = std::collections::HashSet::new();
        for rule in &self.rules {
            rule.validate()?;
            if !names.insert(rule.name.trim().to_lowercase()) {
                return Err(swoop_i18n::tr!("分类名称不能重复").into());
            }
        }
        Ok(())
    }
    pub fn load(store: &Store) -> Self {
        store
            .get_setting("categories")
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str::<Self>(&json).ok())
            .filter(|value| value.validate().is_ok())
            .unwrap_or_default()
    }
    pub fn save(&self, store: &Store) -> Result<(), String> {
        self.validate()?;
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        store.set_setting("categories", &json).map_err(|e| e.to_string())
    }
    pub fn classify(&self, raw: &str, file_name: Option<&str>) -> Option<&CategoryRule> {
        let url = url::Url::parse(raw).ok()?;
        let decoded = percent_encoding::percent_decode_str(url.path()).decode_utf8_lossy();
        let name = file_name.unwrap_or_else(|| decoded.rsplit('/').next().unwrap_or(""));
        self.rules.iter().find(|rule| rule.matches(&url, name))
    }
    pub fn target(&self, base: &Path, url: &str, file_name: Option<&str>) -> PathBuf {
        if !self.enabled {
            return base.into();
        }
        self.classify(url, file_name).map(|rule| base.join(rule.directory.trim())).unwrap_or_else(|| base.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routes_by_type_domain_and_priority_without_prefix_host_confusion() {
        let mut rules = Categories { enabled: true, ..Default::default() };
        rules.rules.insert(
            0,
            CategoryRule {
                name: "工作".into(),
                extensions: "zip".into(),
                domains: "*.example.com".into(),
                directory: "Work".into(),
            },
        );
        assert_eq!(
            rules.target(Path::new("downloads"), "https://cdn.example.com/a.ZIP?x=.mp4", None),
            PathBuf::from("downloads/Work")
        );
        assert_eq!(
            rules.target(Path::new("downloads"), "https://badexample.com/a.zip", None),
            PathBuf::from("downloads/Archives")
        );
        assert_eq!(
            rules.target(Path::new("downloads"), "https://a.test/get", Some("real.mp4")),
            PathBuf::from("downloads/Video")
        );
        assert_eq!(
            rules.target(Path::new("downloads"), "https://a.test/a%2Ezip", None),
            PathBuf::from("downloads/Archives")
        );
        rules.enabled = false;
        assert_eq!(rules.target(Path::new("downloads"), "https://a.test/a.zip", None), PathBuf::from("downloads"));
    }
    #[test]
    fn rules_survive_reopen_and_bad_rules_fail_before_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.db");
        let store = Store::open(&path).unwrap();
        let mut rules = Categories { enabled: true, ..Default::default() };
        rules.save(&store).unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(Categories::load(&store), rules);
        rules.rules[0].directory = "../escape".into();
        assert!(rules.save(&store).is_err());
        assert_ne!(Categories::load(&store), rules);
        rules.rules[0].directory = "safe".into();
        rules.rules[0].domains = "good.test@bad.test".into();
        assert!(rules.validate().is_err());
    }
}
