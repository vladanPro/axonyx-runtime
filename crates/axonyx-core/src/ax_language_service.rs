use crate::ax_backend_parser::{parse_backend_ax, AxBackendParseError};
use crate::ax_parser::AxParseError;
use crate::ax_parser_auto::{convert_ax_v2_file, parse_ax_auto, AxAutoParseError};
use crate::ax_parser_v2::{parse_ax_component_module_v2, AxParseV2Error};
use crate::ax_semantics_v2::validate_ax_v2_semantics;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxSourceKind {
    Page,
    Backend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxLanguageDiagnostic {
    pub line: usize,
    pub column: usize,
    pub code: &'static str,
    pub message: String,
}

impl AxLanguageDiagnostic {
    fn error(line: usize, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            line: line.max(1),
            column: 1,
            code,
            message: message.into(),
        }
    }
}

pub fn classify_ax_source(path: &str, source: &str) -> AxSourceKind {
    if path.to_ascii_lowercase().ends_with(".asx") {
        return AxSourceKind::Page;
    }

    let lines = source.lines().map(str::trim_start).collect::<Vec<_>>();
    if lines.iter().any(|line| {
        let line = line.strip_prefix("export ").unwrap_or(line);
        line.starts_with("page ") || line.starts_with("layout ") || line.starts_with("component ")
    }) {
        return AxSourceKind::Page;
    }

    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let backend_path = normalized_path.contains("/routes/api/")
        || normalized_path.contains("/jobs/")
        || [
            "/backend.ax",
            "/loader.ax",
            "/actions.ax",
            "/action.ax",
            "/domain.ax",
        ]
        .iter()
        .any(|suffix| normalized_path.ends_with(suffix));
    let backend_declaration = lines.into_iter().any(|line| {
        let line = line.strip_prefix("export ").unwrap_or(line);
        line.starts_with("route ")
            || line == "backend"
            || line.starts_with("loader ")
            || line.starts_with("query ")
            || line.starts_with("action ")
            || line.starts_with("fn ")
            || line.starts_with("scope ")
            || line.starts_with("job ")
            || line.starts_with("env ")
            || line.starts_with("type ")
    });

    if backend_path || backend_declaration {
        AxSourceKind::Backend
    } else {
        AxSourceKind::Page
    }
}

pub fn diagnose_ax_source(path: &str, source: &str) -> Vec<AxLanguageDiagnostic> {
    let diagnostic = match classify_ax_source(path, source) {
        AxSourceKind::Page => diagnose_page_source(source),
        AxSourceKind::Backend => parse_backend_ax(source).err().map(|error| {
            AxLanguageDiagnostic::error(
                line_from_backend_parse_error(&error),
                "axonyx-backend-parse",
                error.to_string(),
            )
        }),
    };

    diagnostic.into_iter().collect()
}

fn diagnose_page_source(source: &str) -> Option<AxLanguageDiagnostic> {
    let component_only = !source
        .lines()
        .any(|line| line.trim_start().starts_with("page "));
    if component_only {
        match parse_ax_component_module_v2(source) {
            Ok(Some(file)) => {
                if let Err(error) = validate_ax_v2_semantics(&file) {
                    return Some(AxLanguageDiagnostic::error(
                        1,
                        "axonyx-semantic",
                        error.to_string(),
                    ));
                }
                if let Err(error) = convert_ax_v2_file(&file) {
                    return Some(AxLanguageDiagnostic::error(
                        1,
                        "axonyx-parse",
                        error.to_string(),
                    ));
                }
                return None;
            }
            Err(error) => {
                return Some(AxLanguageDiagnostic::error(
                    line_from_ax_parse_v2_error(&error),
                    "axonyx-parse",
                    error.to_string(),
                ));
            }
            Ok(None) => {}
        }
    }

    parse_ax_auto(source).err().map(|error| {
        AxLanguageDiagnostic::error(
            line_from_auto_parse_error(&error),
            "axonyx-parse",
            auto_parse_message(&error),
        )
    })
}

fn auto_parse_message(error: &AxAutoParseError) -> String {
    match error {
        AxAutoParseError::V1(error) => error.to_string(),
        AxAutoParseError::V2(error) => error.to_string(),
        AxAutoParseError::Semantic(error) => error.to_string(),
        AxAutoParseError::Convert(error) => error.to_string(),
    }
}

fn line_from_auto_parse_error(error: &AxAutoParseError) -> usize {
    match error {
        AxAutoParseError::V1(error) => line_from_ax_parse_error(error),
        AxAutoParseError::V2(error) => line_from_ax_parse_v2_error(error),
        AxAutoParseError::Semantic(_) | AxAutoParseError::Convert(_) => 1,
    }
}

fn line_from_ax_parse_error(error: &AxParseError) -> usize {
    match error {
        AxParseError::EmptyDocument => 1,
        AxParseError::TabsNotSupported { line }
        | AxParseError::InvalidIndentation { line }
        | AxParseError::InvalidPage { line }
        | AxParseError::UnexpectedIndentation { line }
        | AxParseError::InvalidDataBinding { line }
        | AxParseError::InvalidEach { line }
        | AxParseError::InvalidPipelineStage { line }
        | AxParseError::InvalidComponent { line }
        | AxParseError::InvalidTitle { line }
        | AxParseError::InvalidTheme { line }
        | AxParseError::InvalidHeadTag { line, .. }
        | AxParseError::InvalidExpression { line, .. } => *line,
    }
}

fn line_from_ax_parse_v2_error(error: &AxParseV2Error) -> usize {
    match error {
        AxParseV2Error::EmptyDocument | AxParseV2Error::MissingPage => 1,
        AxParseV2Error::InvalidImport { line }
        | AxParseV2Error::InvalidUse { line }
        | AxParseV2Error::MissingImportFrom { line }
        | AxParseV2Error::EmptyImportList { line }
        | AxParseV2Error::InvalidPage { line }
        | AxParseV2Error::InvalidLet { line }
        | AxParseV2Error::InvalidState { line }
        | AxParseV2Error::InvalidStatePersistence { line, .. }
        | AxParseV2Error::InvalidType { line }
        | AxParseV2Error::InvalidFunction { line }
        | AxParseV2Error::InvalidComponent { line }
        | AxParseV2Error::InvalidReturnAsx { line }
        | AxParseV2Error::DuplicatePage { line }
        | AxParseV2Error::InvalidTag { line }
        | AxParseV2Error::UnterminatedTag { line }
        | AxParseV2Error::UnterminatedString { line }
        | AxParseV2Error::UnterminatedExpression { line }
        | AxParseV2Error::UnexpectedClosingTag { line, .. }
        | AxParseV2Error::MismatchedClosingTag { line, .. }
        | AxParseV2Error::MissingAttributeValue { line, .. } => *line,
    }
}

fn line_from_backend_parse_error(error: &AxBackendParseError) -> usize {
    match error {
        AxBackendParseError::EmptyDocument => 1,
        AxBackendParseError::InvalidImport { line }
        | AxBackendParseError::MissingImportFrom { line }
        | AxBackendParseError::EmptyImportList { line }
        | AxBackendParseError::TabsNotSupported { line }
        | AxBackendParseError::InvalidIndentation { line }
        | AxBackendParseError::UnexpectedIndentation { line }
        | AxBackendParseError::InvalidBlock { line }
        | AxBackendParseError::InvalidDataBinding { line }
        | AxBackendParseError::InvalidEnvDeclaration { line }
        | AxBackendParseError::InvalidInputSection { line }
        | AxBackendParseError::InvalidField { line }
        | AxBackendParseError::InvalidTypeDeclaration { line }
        | AxBackendParseError::InvalidMutation { line }
        | AxBackendParseError::InvalidTransaction { line }
        | AxBackendParseError::InvalidAssignment { line }
        | AxBackendParseError::InvalidHeader { line }
        | AxBackendParseError::InvalidCookie { line }
        | AxBackendParseError::InvalidHook { line }
        | AxBackendParseError::InvalidRequirement { line }
        | AxBackendParseError::InvalidReturn { line }
        | AxBackendParseError::InvalidSend { line }
        | AxBackendParseError::InvalidScope { line }
        | AxBackendParseError::InvalidScopeMember { line }
        | AxBackendParseError::InvalidScopeState { line }
        | AxBackendParseError::InvalidScopeRender { line }
        | AxBackendParseError::InvalidQuerySource { line }
        | AxBackendParseError::InvalidQueryClause { line }
        | AxBackendParseError::InvalidQueryNumber { line }
        | AxBackendParseError::InvalidExpression { line, .. } => *line,
    }
}

pub mod prelude {
    pub use super::{classify_ax_source, diagnose_ax_source, AxLanguageDiagnostic, AxSourceKind};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_frontend_and_backend_sources() {
        assert_eq!(
            classify_ax_source("app/page.asx", "page Home() { return ASX { <>Hi</> } }"),
            AxSourceKind::Page
        );
        assert_eq!(
            classify_ax_source(
                "app/posts/loader.ax",
                "query loadPosts() -> Post[] { return [] }"
            ),
            AxSourceKind::Backend
        );
        assert_eq!(
            classify_ax_source(
                "app/components/Card.ax",
                "component Card { render ASX { <article /> } }"
            ),
            AxSourceKind::Page
        );
    }

    #[test]
    fn reports_page_parser_line() {
        let diagnostics = diagnose_ax_source(
            "app/page.asx",
            "page Home() {\n  return ASX {\n    <Card>\n    </Grid>\n  }\n}\n",
        );

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].line, 4);
        assert_eq!(diagnostics[0].column, 1);
        assert_eq!(diagnostics[0].code, "axonyx-parse");
    }

    #[test]
    fn reports_backend_parser_line() {
        let diagnostics = diagnose_ax_source(
            "app/posts/loader.ax",
            "query loadPosts() -> Post[] {\n  nope ???\n}\n",
        );

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].line, 2);
        assert_eq!(diagnostics[0].code, "axonyx-backend-parse");
    }

    #[test]
    fn valid_sources_have_no_diagnostics() {
        assert!(diagnose_ax_source(
            "app/page.asx",
            "page Home() {\n  return ASX {\n    <Copy>Hello</Copy>\n  }\n}\n"
        )
        .is_empty());
        assert!(diagnose_ax_source(
            "app/posts/loader.ax",
            "query loadPosts() -> Post[] {\n  return []\n}\n"
        )
        .is_empty());
        assert!(diagnose_ax_source(
            "app/components/Greeting.asx",
            "component Greeting {\n  render ASX {\n    <Copy>Hello</Copy>\n  }\n}\n"
        )
        .is_empty());
    }
}
