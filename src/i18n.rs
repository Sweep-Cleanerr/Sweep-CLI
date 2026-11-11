//! Minimal internationalisation.
//!
//! The original project ships 8 languages with gettext-style lookup.
//! This rewrite keeps only the English strings plus the language list,
//! so `--lang` and `config.languages` have a home. Full translation
//! packs are intentionally not carried over.

/// Languages the rewrite knows about.
pub const SUPPORTED_LANGUAGES: &[&str] = &["en", "tr", "de", "fr", "es", "it", "pt", "ru"];

/// Normalise a user-supplied language code (`tr_TR.UTF-8` → `tr`).
pub fn normalize_lang(code: &str) -> String {
    let base = code.split(['.', '@', '_']).next().unwrap_or("en");
    let base = base.trim().to_ascii_lowercase();
    if base.is_empty() {
        "en".to_string()
    } else {
        base
    }
}

/// Always English in this rewrite; translations are not bundled.
pub fn t(_lang: &str, key: &str) -> String {
    key.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_locale_forms() {
        assert_eq!(normalize_lang("tr_TR.UTF-8"), "tr");
        assert_eq!(normalize_lang("DE_de"), "de");
        assert_eq!(normalize_lang(""), "en");
        assert!(SUPPORTED_LANGUAGES.contains(&"en"));
    }
}
