//! The system language ([`trino_core::Platform::system_language`], issue
//! #59): `TRINO_LANG` when set (tests, the editor's preview language), else
//! the OS locale (`sys-locale`: `LANG`/`LC_*` on Linux, the user locale on
//! Windows and macOS). Read once, when the platform is made.

use trino_core::LangTag;

/// The language from an override and the OS locale (pure, so tests don't
/// touch the process environment). An override that does not parse (empty,
/// `C`) falls through to the OS.
pub fn pick(override_value: Option<&str>, os_locale: Option<&str>) -> Option<LangTag> {
    override_value
        .and_then(LangTag::parse)
        .or_else(|| os_locale.and_then(LangTag::parse))
}

/// This process's system language.
pub fn detect() -> Option<LangTag> {
    let env = std::env::var("TRINO_LANG").ok();
    let os = sys_locale::get_locale();
    pick(env.as_deref(), os.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_override_wins_then_the_os_locale() {
        assert_eq!(
            pick(Some("pt-BR"), Some("en-US")).unwrap().as_str(),
            "pt-BR"
        );
        assert_eq!(pick(None, Some("en_US.UTF-8")).unwrap().as_str(), "en-US");
        assert_eq!(pick(Some(""), Some("es-ES")).unwrap().as_str(), "es-ES");
        assert_eq!(
            pick(Some("C"), Some("fr_FR@euro")).unwrap().as_str(),
            "fr-FR"
        );
        assert_eq!(pick(None, Some("C")), None);
        assert_eq!(pick(None, None), None);
    }
}
