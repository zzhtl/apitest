use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Language {
    Chinese,
    English,
}

/// Pick the localised literal for `language`.
///
/// A free function rather than a method on the app so widget helpers that only
/// receive a `Language` can localise without borrowing the whole app state.
pub(crate) fn tr<'a>(language: Language, chinese: &'a str, english: &'a str) -> &'a str {
    match language {
        Language::Chinese => chinese,
        Language::English => english,
    }
}
