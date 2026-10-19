//! The system language ([`trino_core::Platform::system_language`], issue
//! #59): the page's `?lang=` (tests, a link to a translation), else the
//! browser's `navigator.language`. Read once, when the platform is made.

use trino_core::LangTag;

/// The language from `?lang=` and `navigator.language` (pure, host-tested).
pub fn pick(query: Option<&str>, navigator: Option<&str>) -> Option<LangTag> {
    query
        .and_then(LangTag::parse)
        .or_else(|| navigator.and_then(LangTag::parse))
}

/// This page's language.
#[cfg(target_arch = "wasm32")]
pub fn detect() -> Option<LangTag> {
    let query = crate::runtime::Query::from_location().get("lang");
    let navigator = web_sys::window().and_then(|w| w.navigator().language());
    pick(query.as_deref(), navigator.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_wins_then_the_browser() {
        assert_eq!(
            pick(Some("es-ES"), Some("en-US")).unwrap().as_str(),
            "es-ES"
        );
        assert_eq!(pick(None, Some("pt-BR")).unwrap().as_str(), "pt-BR");
        assert_eq!(pick(Some(""), Some("ja")).unwrap().as_str(), "ja");
        assert_eq!(pick(None, None), None);
    }
}
