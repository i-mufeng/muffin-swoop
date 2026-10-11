use swoop_i18n::{Language, language, set_language, tr, trf};

// This integration binary owns its locale, so it cannot change concurrent application tests.
#[test]
fn locale_switch_preserves_format_arguments_and_stable_language_codes() {
    set_language(Language::Chinese);
    assert_eq!(tr!("设置"), "设置");
    assert_eq!(trf!("{} 个链接", 3), "3 个链接");
    let seconds = 7;
    assert_eq!(trf!("{seconds} 秒后{}", "退出"), "7 秒后退出");

    set_language(Language::English);
    assert_eq!(language(), Language::English);
    assert_eq!(tr!("设置"), "Settings");
    assert_eq!(trf!("{} 个链接", 3), "3 links");
    assert_eq!(trf!("{seconds} 秒后{}", "Quit"), "Quit in 7 seconds");
    assert_eq!(Language::English.as_str(), "en-US");
    set_language(Language::Chinese);
}
