#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Locale {
    Chinese,
    English,
}

impl Locale {
    pub fn from_preference(value: &str) -> Self {
        if value == "zh-CN" {
            Self::Chinese
        } else {
            Self::English
        }
    }
    fn from_system_language(value: Option<&str>) -> Self {
        if value
            .and_then(|s| s.split(['-', '_']).next())
            .is_some_and(|s| s.eq_ignore_ascii_case("zh"))
        {
            Self::Chinese
        } else {
            Self::English
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Self::Chinese => "zh-CN",
            Self::English => "en",
        }
    }
    pub fn text(self, zh: &'static str, en: &'static str) -> &'static str {
        match self {
            Self::Chinese => zh,
            Self::English => en,
        }
    }
}

pub(crate) fn system_locale() -> Locale {
    #[cfg(target_os = "macos")]
    {
        let first = objc2_foundation::NSLocale::preferredLanguages()
            .firstObject()
            .map(|language| language.to_string());
        Locale::from_system_language(first.as_deref())
    }
    #[cfg(not(target_os = "macos"))]
    Locale::from_system_language(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_language_mapping_is_bounded_and_does_not_use_region_as_language() {
        for language in ["zh", "zh-Hans-CN", "zh-Hant-TW", "ZH_hant_HK"] {
            assert_eq!(Locale::from_system_language(Some(language)).code(), "zh-CN");
        }
        for language in [None, Some("en-US"), Some("fr-CN"), Some("zhuang"), Some("")] {
            assert_eq!(Locale::from_system_language(language).code(), "en");
        }
        assert_eq!(Locale::from_preference("en").text("开始", "Start"), "Start");
        assert_eq!(
            Locale::from_preference("zh-CN").text("开始", "Start"),
            "开始"
        );
    }
}
