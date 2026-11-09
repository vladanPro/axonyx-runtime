//! Server-side message catalogs. Applications choose the locale for each request.

use serde::de::{self, MapAccess, Visitor};
use serde::Deserializer as _;
use std::collections::{btree_map::Entry, BTreeMap};
use std::fmt;
use thiserror::Error;

mod binding;
pub use binding::{
    bound_translator_initializer, lower_bound_translators, AxTranslationBindingError,
};

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
        AxTranslator {
            catalog: self,
            selected,
            fallback,
        }
    }
}

/// A view of the shared catalogs bound to a single effective locale.
#[derive(Debug, Clone, Copy)]
pub struct AxTranslator<'a> {
    catalog: &'a AxTranslationCatalog,
    selected: &'a LocaleCatalog,
    fallback: &'a LocaleCatalog,
}

impl<'a> AxTranslator<'a> {
    pub(crate) fn for_locale(self, locale: &str) -> Self {
        self.catalog.translator(locale)
    }

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
    use crate::{
        compose_compiled_page_document, render_preview_route, AxCompiledPageRender,
        AxPreviewRouteRender, AxPreviewStore,
    };
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

    #[test]
    fn preview_and_compiled_render_translate_and_escape_with_the_same_locale() {
        let source = r#"
page Home() {
  const adminLanguage = "sr-Latn"
  return ASX {
    <>
      <button title={t("pages.save")}>{t("pages.save")} {t("pages.cancel")}</button>
      <span lang={adminLanguage}>{t("pages.save", adminLanguage) + " / " + t("pages.cancel", adminLanguage)}</span>
      <span lang="en">{t("pages.save", "unsupported") + " / " + t("pages.save", "en")}</span>
    </>
  }
}
"#;
        let document =
            serde_json::to_string(&compose_compiled_page_document(&[], source).unwrap()).unwrap();
        let catalog = AxTranslationCatalog::from_json(
            "en",
            [("en", EN), ("sr-Latn", r#"{"pages.save":"<Save>"}"#)],
        )
        .unwrap();
        let params = BTreeMap::new();
        let store = AxPreviewStore::default();
        let imports = |_: &str| None;
        let loader_values = BTreeMap::new();
        let inputs = || AxCompiledPageRender {
            document_json: &document,
            import_sources: &[],
            request_target: "/",
            route_params: &params,
            loader_values: &loader_values,
            form_result: None,
        };

        for (language, expected) in [("en", "Save"), ("sr-Latn", "&lt;Save&gt;")] {
            let translator = Some(catalog.translator(language));
            let preview = render_preview_route(
                AxPreviewRouteRender {
                    layout_sources: &[],
                    loader_sources: &[],
                    action_sources: &[],
                    page_source: source,
                    request_target: "/",
                    route_params: &params,
                    runtime: None,
                    request: None,
                    form: None,
                },
                &store,
                &imports,
                translator,
            )
            .unwrap();
            let compiled = inputs().render_document(translator).unwrap();
            assert_eq!(preview, compiled);
            assert!(compiled.contains(&format!(r#"<html lang="{language}""#)));
            let fragment = inputs().render_fragment(translator).unwrap();
            assert!(fragment.contains(&format!(r#"title="{expected}""#)));
            assert!(fragment.contains(expected));
            assert!(fragment.contains("Cancel"));
            assert!(
                fragment.contains(r#"<span lang="sr-Latn">&lt;Save&gt; / Cancel</span>"#),
                "{fragment}"
            );
            assert!(
                fragment.contains(r#"<span lang="en">Save / Save</span>"#),
                "{fragment}"
            );
            assert!(!fragment.contains("<Save>"));
            assert!(!fragment.contains("<html"));
        }
        assert!(inputs().render_document(None).is_err());
    }

    #[test]
    fn bound_translators_keep_page_and_layout_locales_separate() {
        let layout = r#"
page Layout() {
  const translate = I18n.withLocale("en")
  return ASX {
    <>
      <Slot />
      <p id="layout">{translate("pages.save")}</p>
    </>
  }
}
"#;
        let page = r#"
page Home() {
  const translate = I18n.withLocale(query.language)
  return ASX { <p id="page">{translate("pages.save")}</p> }
}
"#;
        let catalog = AxTranslationCatalog::from_json(
            "en",
            [("en", EN), ("sr-Latn", r#"{"pages.save":"Sacuvaj"}"#)],
        )
        .unwrap();
        let layouts = [layout];
        let translator = Some(catalog.translator("en"));
        let document =
            serde_json::to_string(&crate::compose_localized_page_document(&layouts, page).unwrap())
                .unwrap();
        let params = BTreeMap::new();
        let target = "/?language=sr-Latn";
        let preview = render_preview_route(
            AxPreviewRouteRender {
                layout_sources: &layouts,
                loader_sources: &[],
                action_sources: &[],
                page_source: page,
                request_target: target,
                route_params: &params,
                runtime: None,
                request: None,
                form: None,
            },
            &AxPreviewStore::default(),
            &|_: &str| None,
            translator,
        )
        .unwrap();
        let loader_values = BTreeMap::new();
        let compiled = || AxCompiledPageRender {
            document_json: &document,
            import_sources: &[],
            request_target: target,
            route_params: &params,
            loader_values: &loader_values,
            form_result: None,
        };
        assert_eq!(preview, compiled().render_document(translator).unwrap());
        assert!(preview.contains(r#"<html lang="en""#));
        let fragment = compiled().render_fragment(translator).unwrap();
        assert!(
            fragment.contains(r#"<p id="page">Sacuvaj</p>"#),
            "{fragment}"
        );
        assert!(
            fragment.contains(r#"<p id="layout">Save</p>"#),
            "{fragment}"
        );
    }
}
