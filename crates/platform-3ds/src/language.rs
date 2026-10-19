//! The system language ([`trino_core::Platform::system_language`], issue
//! #59): the shim's `CFGU_GetSystemLanguage` code → a language tag (pure,
//! so host tests check the mapping without a console). The 3DS stores a
//! language, not a region: Portuguese is `pt`, English `en`, and the
//! game's `Localizer::pick` matches it to `pt-BR` or `en-US` by its
//! primary subtag.

use trino_core::LangTag;

/// libctru's `CFG_Language` order (`CFG_LANGUAGE_JP` = 0 … `CFG_LANGUAGE_TW` = 11).
const TAGS: [&str; 12] = [
    "ja", "en", "fr", "de", "it", "es", "zh-CN", "ko", "nl", "pt", "ru", "zh-TW",
];

/// The tag of `trino_system_language`'s code (`None`: negative or unknown).
pub fn tag(code: i32) -> Option<LangTag> {
    usize::try_from(code)
        .ok()
        .and_then(|i| TAGS.get(i))
        .and_then(|t| LangTag::parse(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cfg_language_maps_to_its_tag() {
        for (code, want) in TAGS.iter().enumerate() {
            assert_eq!(tag(code as i32).unwrap().as_str(), *want);
        }
        assert_eq!(tag(1).unwrap().as_str(), "en");
        assert_eq!(tag(9).unwrap().as_str(), "pt");
        assert_eq!(tag(-1), None, "the shim could not read it");
        assert_eq!(tag(12), None);
    }
}
