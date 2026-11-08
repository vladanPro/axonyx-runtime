//! Server-side message catalogs. Applications choose the locale for each request.

use serde::de::{self, MapAccess, Visitor};
use serde::Deserializer as _;
use std::collections::{btree_map::Entry, BTreeMap};
use std::fmt;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AxCatalogError {
    #[error("invalid catalog locale: {0:?}")]
    InvalidLocale(String),
    #[error("duplicate catalog locale: {0:?}")]
    DuplicateLocale(String),
    #[error("fallback catalog is missing for locale {0:?}")]
    MissingFallback(String),
    #[error("invalid JSON catalog for {locale:?}: {source}")]
    InvalidJson {
        locale: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("catalog {locale:?} has an invalid message key: {key:?}")]
    InvalidKey { locale: String, key: String },
    #[error("catalog {locale:?} uses key {key:?}, which is absent from the fallback catalog")]
    UnknownKey { locale: String, key: String },
}

#[derive(Debug)]
struct LocaleCatalog {
    tag: String,
    messages: BTreeMap<String, String>,
}

/// Immutable catalogs shareable across requests, with one explicit fallback.
///
/// ```
/// use axonyx_runtime::translation::AxTranslationCatalog;
///
/// let catalogs = AxTranslationCatalog::from_json("en", [
///     ("en", r#"{"pages.save":"Save","pages.cancel":"Cancel"}"#),
///     ("sr-Latn", r#"{"pages.save":"Snimi"}"#),
/// ])?;
/// let translator = catalogs.translator("sr-Latn");
/// assert_eq!(translator.translate("pages.save"), Some("Snimi"));
/// assert_eq!(translator.translate("pages.cancel"), Some("Cancel"));
/// # Ok::<(), axonyx_runtime::translation::AxCatalogError>(())
/// ```
#[derive(Debug)]
pub struct AxTranslationCatalog {
    locales: BTreeMap<String, LocaleCatalog>,
    fallback: String,
}

impl AxTranslationCatalog {
    /// Each source is a locale tag and a flat JSON object of string messages.
    /// Every translated key must also exist in the fallback catalog.
    pub fn from_json<'a>(
        fallback_locale: &str,
        sources: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, AxCatalogError> {
        let fallback = locale_key(fallback_locale)?;
        let mut locales = BTreeMap::new();
        for (locale, json) in sources {
            let slot = match locales.entry(locale_key(locale)?) {
                Entry::Occupied(_) => {
                    return Err(AxCatalogError::DuplicateLocale(locale.into()));
                }
                Entry::Vacant(slot) => slot,
            };
            let messages = parse_messages(json).map_err(|source| AxCatalogError::InvalidJson {
                locale: locale.into(),
                source,
            })?;
            if let Some(key) = messages.keys().find(|key| !valid_message_key(key)) {
                return Err(AxCatalogError::InvalidKey {
                    locale: locale.into(),
                    key: key.clone(),
                });
            }
            slot.insert(LocaleCatalog {
                tag: locale.into(),
                messages,
            });
        }
        let fallback_catalog = locales
            .get(&fallback)
            .ok_or_else(|| AxCatalogError::MissingFallback(fallback_locale.into()))?;
        for catalog in locales.values() {
            if let Some(key) = catalog
                .messages
                .keys()
                .find(|key| !fallback_catalog.messages.contains_key(*key))
            {
                return Err(AxCatalogError::UnknownKey {
                    locale: catalog.tag.clone(),
                    key: key.clone(),
                });
            }
        }
        Ok(Self { locales, fallback })
    }

    /// A key is known only if it is defined in the fallback catalog.
    pub fn contains_key(&self, key: &str) -> bool {
        self.locales[&self.fallback].messages.contains_key(key)
    }

    /// Bind a locale without changing the shared catalog. Unsupported locales
    /// fall back directly; locale negotiation is the caller's responsibility.
    pub fn translator(&self, locale: &str) -> AxTranslator<'_> {
        let fallback = &self.locales[&self.fallback];
        let selected = self
            .locales
            .get(&locale.to_ascii_lowercase())
            .unwrap_or(fallback);
        AxTranslator { selected, fallback }
    }
}

/// A view of the shared catalogs bound to a single effective locale.
#[derive(Debug, Clone, Copy)]
pub struct AxTranslator<'a> {
    selected: &'a LocaleCatalog,
    fallback: &'a LocaleCatalog,
}

impl<'a> AxTranslator<'a> {
    pub fn locale(&self) -> &'a str {
        &self.selected.tag
    }

    /// Look up plain text, falling back per message. The renderer must escape it.
    pub fn translate(&self, key: &str) -> Option<&'a str> {
        self.selected
            .messages
            .get(key)
            .or_else(|| self.fallback.messages.get(key))
            .map(String::as_str)
    }
}

fn locale_key(locale: &str) -> Result<String, AxCatalogError> {
    // Check the supported tag shape, not the complete BCP 47 registry.
    let mut parts = locale.split('-');
    let primary = parts.next().unwrap_or_default();
    if (2..=8).contains(&primary.len())
        && primary.bytes().all(|byte| byte.is_ascii_alphabetic())
        && parts.all(|part| {
            (1..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    {
        Ok(locale.to_ascii_lowercase())
    } else {
        Err(AxCatalogError::InvalidLocale(locale.into()))
    }
}

fn valid_message_key(key: &str) -> bool {
    key.split('.').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

fn parse_messages(json: &str) -> Result<BTreeMap<String, String>, serde_json::Error> {
    struct Messages;

    impl<'de> Visitor<'de> for Messages {
        type Value = BTreeMap<String, String>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object mapping message keys to strings")
        }

        fn visit_map<M: MapAccess<'de>>(self, mut input: M) -> Result<Self::Value, M::Error> {
            let mut messages = BTreeMap::new();
            while let Some((key, message)) = input.next_entry::<String, String>()? {
                match messages.entry(key) {
                    Entry::Vacant(slot) => {
                        slot.insert(message);
                    }
                    Entry::Occupied(slot) => {
                        return Err(de::Error::custom(format!(
                            "duplicate message key {:?}",
                            slot.key()
                        )));
                    }
                }
            }
            Ok(messages)
        }
    }

    let mut parser = serde_json::Deserializer::from_str(json);
    let messages = parser.deserialize_map(Messages)?;
    parser.end()?;
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    const EN: &str = r#"{"pages.save":"Save","pages.cancel":"Cancel"}"#;
    const SR: &str = r#"{"pages.save":"Snimi"}"#;

    #[test]
    fn falls_back_for_missing_messages_and_unsupported_locales() {
        let catalog = AxTranslationCatalog::from_json("EN", [("en", EN), ("sr-Latn", SR)]).unwrap();
        let translator = catalog.translator("SR-latn");
        assert_eq!(translator.locale(), "sr-Latn");
        assert_eq!(translator.translate("pages.save"), Some("Snimi"));
        assert_eq!(translator.translate("pages.cancel"), Some("Cancel"));
        assert_eq!(translator.translate("pages.missing"), None);
        assert!(catalog.contains_key("pages.cancel"));
        assert!(!catalog.contains_key("pages.missing"));
        assert_eq!(catalog.translator("de").locale(), "en");
        assert_eq!(
            catalog.translator("de").translate("pages.save"),
            Some("Save")
        );
    }

    #[test]
    fn rejects_invalid_or_ambiguous_catalogs() {
        assert!(matches!(
            AxTranslationCatalog::from_json("en", [("sr-Latn", SR)]),
            Err(AxCatalogError::MissingFallback(_))
        ));
        assert!(matches!(
            AxTranslationCatalog::from_json("en", [("en", EN), ("../en", EN)]),
            Err(AxCatalogError::InvalidLocale(_))
        ));
        assert!(matches!(
            AxTranslationCatalog::from_json("en", [("en", EN), ("EN", EN)]),
            Err(AxCatalogError::DuplicateLocale(_))
        ));
        assert!(matches!(
            AxTranslationCatalog::from_json("en", [("en", r#"{"pages..save":"Save"}"#)]),
            Err(AxCatalogError::InvalidKey { .. })
        ));
        assert!(matches!(
            AxTranslationCatalog::from_json(
                "en",
                [("en", EN), ("sr-Latn", r#"{"pages.svae":"Snimi"}"#)]
            ),
            Err(AxCatalogError::UnknownKey { .. })
        ));
        for json in [
            r#"{"pages.save":"Save","pages.save":"Overwrite"}"#,
            r#"{"pages.save":123}"#,
            r#"{"pages.save":"Save"} {}"#,
            r#"[]"#,
        ] {
            assert!(matches!(
                AxTranslationCatalog::from_json("en", [("en", json)]),
                Err(AxCatalogError::InvalidJson { .. })
            ));
        }
    }

    #[test]
    fn concurrent_translators_do_not_share_a_mutable_locale() {
        let catalog = AxTranslationCatalog::from_json("en", [("en", EN), ("sr-Latn", SR)]).unwrap();
        let ready = Barrier::new(2);
        std::thread::scope(|scope| {
            for (locale, message) in [("en", "Save"), ("sr-Latn", "Snimi")] {
                let catalog = &catalog;
                let ready = &ready;
                scope.spawn(move || {
                    let translator = catalog.translator(locale);
                    ready.wait();
                    assert_eq!(translator.locale(), locale);
                    assert_eq!(translator.translate("pages.save"), Some(message));
                });
            }
        });
    }
}
