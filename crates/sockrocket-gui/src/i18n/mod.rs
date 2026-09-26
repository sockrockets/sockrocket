//! Compile-time UI string catalogs: English, Chinese (Simplified), Vietnamese.

mod en;
mod vi;
mod zh;

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Locale {
    En = 0,
    Vi = 1,
    Zh = 2,
}

static ACTIVE: AtomicU8 = AtomicU8::new(Locale::En as u8);

impl Locale {
    pub fn available() -> &'static [Locale] {
        &[Locale::En, Locale::Vi, Locale::Zh]
    }

    pub fn id(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Vi => "vi",
            Locale::Zh => "zh",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Locale::En => "English",
            Locale::Vi => "Tiếng Việt",
            Locale::Zh => "中文",
        }
    }

    /// Cycle EN → VI → ZH → EN (used by the status-bar language control).
    pub fn next(self) -> Locale {
        match self {
            Locale::En => Locale::Vi,
            Locale::Vi => Locale::Zh,
            Locale::Zh => Locale::En,
        }
    }

    fn from_u8(v: u8) -> Locale {
        match v {
            1 => Locale::Vi,
            2 => Locale::Zh,
            _ => Locale::En,
        }
    }
}

/// Map a persisted locale id to a known [`Locale`]. Unknown values → English.
pub fn parse_locale(raw: &str) -> Locale {
    match raw.trim().to_ascii_lowercase().as_str() {
        "en" | "en-us" | "en_gb" => Locale::En,
        "zh" | "zh-cn" | "zh_cn" | "zh-hans" | "cn" => Locale::Zh,
        "vi" | "vi-vn" | "vi_vn" => Locale::Vi,
        _ => Locale::En,
    }
}

pub fn set_locale(locale: Locale) {
    ACTIVE.store(locale as u8, Ordering::Relaxed);
}

pub fn current_locale() -> Locale {
    Locale::from_u8(ACTIVE.load(Ordering::Relaxed))
}

/// Look up a UI string. Falls back to English, then to the key itself.
pub fn t(key: &str) -> &'static str {
    if let Some(s) = lookup(current_locale(), key) {
        return s;
    }
    if let Some(s) = lookup(Locale::En, key) {
        return s;
    }
    eprintln!("missing i18n key: {key}");
    Box::leak(key.to_owned().into_boxed_str())
}

fn lookup(locale: Locale, key: &str) -> Option<&'static str> {
    match locale {
        Locale::En => en::lookup(key),
        Locale::Zh => zh::lookup(key),
        Locale::Vi => vi::lookup(key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_key_returns_english() {
        set_locale(Locale::En);
        assert_eq!(t("nav.settings"), "Settings");
    }

    #[test]
    fn chinese_and_vietnamese_resolve() {
        set_locale(Locale::Zh);
        assert_eq!(t("nav.settings"), "设置");
        set_locale(Locale::Vi);
        assert_eq!(t("nav.settings"), "Cài đặt");
        set_locale(Locale::En);
    }

    #[test]
    fn unknown_key_falls_back_to_key_string() {
        set_locale(Locale::En);
        assert_eq!(t("does.not.exist"), "does.not.exist");
    }

    #[test]
    fn parse_locale_ids() {
        assert_eq!(parse_locale("zz"), Locale::En);
        assert_eq!(parse_locale("en"), Locale::En);
        assert_eq!(parse_locale("zh-CN"), Locale::Zh);
        assert_eq!(parse_locale("vi"), Locale::Vi);
    }

    #[test]
    fn next_cycles_three_locales() {
        assert_eq!(Locale::En.next(), Locale::Vi);
        assert_eq!(Locale::Vi.next(), Locale::Zh);
        assert_eq!(Locale::Zh.next(), Locale::En);
    }
}
