//! UI-only locale selection. Download identifiers, paths and protocols never use translated keys.
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};
pub use swoop_i18n_macros::{tr, trf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "zh-CN")]
    Chinese,
    #[serde(rename = "en-US")]
    English,
}
impl Language {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "zh-CN" => Some(Self::Chinese),
            "en-US" => Some(Self::English),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chinese => "zh-CN",
            Self::English => "en-US",
        }
    }
}
static LANGUAGE: AtomicU8 = AtomicU8::new(0);
pub fn set_language(language: Language) {
    LANGUAGE.store(u8::from(language == Language::English), Ordering::Relaxed);
}
pub fn language() -> Language {
    if LANGUAGE.load(Ordering::Relaxed) == 1 { Language::English } else { Language::Chinese }
}
pub fn text(chinese: &'static str, english: &'static str) -> &'static str {
    choose(language(), chinese, english)
}
pub fn choose(language: Language, chinese: &'static str, english: &'static str) -> &'static str {
    match language {
        Language::Chinese => chinese,
        Language::English => english,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locale_keys_are_stable_and_unknown_values_do_not_change_defaults() {
        for language in [Language::Chinese, Language::English] {
            assert_eq!(Language::parse(language.as_str()), Some(language));
        }
        assert_eq!(Language::parse("unknown"), None);
        assert_eq!(choose(Language::Chinese, "设置", "Settings"), "设置");
        assert_eq!(choose(Language::English, "设置", "Settings"), "Settings");
    }
}
