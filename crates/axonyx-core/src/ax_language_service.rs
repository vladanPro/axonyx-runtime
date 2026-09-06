use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxLanguageImport {
    pub source: String,
    pub line: usize,
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

pub fn ax_source_imports(path: &str, source: &str) -> Vec<AxLanguageImport> {
    let sources = match classify_ax_source(path, source) {
        AxSourceKind::Backend => parse_backend_ax(source).ok().map(|document| {
            document
                .imports
                .into_iter()
                .map(|import| import.source)
                .collect::<Vec<_>>()
        }),
        AxSourceKind::Page => page_import_sources(source),
    }
    .unwrap_or_default();

    sources
        .into_iter()
        .map(|import_source| AxLanguageImport {
            line: import_source_line(source, &import_source),
            source: import_source,
        })
        .collect()
}

pub fn diagnose_ax_workspace_imports(
    root: &Path,
    path: &Path,
    source: &str,
    package_roots: &BTreeMap<String, PathBuf>,
) -> Vec<AxLanguageDiagnostic> {
    let kind = classify_ax_source(&path.to_string_lossy(), source);

    ax_source_imports(&path.to_string_lossy(), source)
        .into_iter()
        .filter_map(|import| {
            if kind == AxSourceKind::Page
                && (import.source.starts_with("./") || import.source.starts_with("../"))
            {
                return Some(AxLanguageDiagnostic::error(
                    import.line,
                    "axonyx-import",
                    format!(
                        "relative frontend import `{}` is not supported yet; use an `@/` app alias",
                        import.source
                    ),
                ));
            }
            let resolved = resolve_ax_import_path(root, path, kind, &import.source, package_roots);
            if resolved.as_ref().is_some_and(|target| target.is_file()) {
                return None;
            }

            let expected = resolved
                .map(|target| format!(" expected '{}'", target.display()))
                .unwrap_or_default();
            let (code, label) = match kind {
                AxSourceKind::Page => ("axonyx-import", "import"),
                AxSourceKind::Backend => ("axonyx-backend-import", "backend import"),
            };
            Some(AxLanguageDiagnostic::error(
                import.line,
                code,
                format!("unable to resolve {label} `{}`{expected}", import.source),
            ))
        })
        .collect()
}

pub fn resolve_ax_import_path(
    root: &Path,
    importing_path: &Path,
    kind: AxSourceKind,
    source: &str,
    package_roots: &BTreeMap<String, PathBuf>,
) -> Option<PathBuf> {
    let (base, relative, boundary) = if let Some(relative) = source.strip_prefix("@/") {
        let base = root.join("app");
        (base.clone(), relative, root.to_path_buf())
    } else if source.starts_with("./") || source.starts_with("../") {
        let base = importing_path.parent()?.to_path_buf();
        (base, source, root.to_path_buf())
    } else {
        let (namespace, relative) = split_package_import(source)?;
        let base = package_roots.get(namespace)?.to_path_buf();
        (base.clone(), relative, base)
    };

    let boundary = normalize_ax_path(&boundary);
    let candidate = resolve_ax_import_extension(base.join(relative), kind);
    let candidate = normalize_ax_path(&candidate);
    candidate.starts_with(boundary).then_some(candidate)
}

fn page_import_sources(source: &str) -> Option<Vec<String>> {
    if let Ok(Some(file)) = parse_ax_component_module_v2(source) {
        return Some(
            file.imports
                .into_iter()
                .map(|import| import.source)
                .collect(),
        );
    }

    parse_ax_auto(source).ok().map(|document| {
        document
            .imports
            .into_iter()
            .map(|import| import.source)
            .collect()
    })
}

fn import_source_line(source: &str, import_source: &str) -> usize {
    source
        .lines()
        .position(|line| line.trim_start().starts_with("import ") && line.contains(import_source))
        .map(|index| index + 1)
        .unwrap_or(1)
}

fn split_package_import(source: &str) -> Option<(&str, &str)> {
    let mut parts = source.splitn(3, '/');
    let scope = parts.next()?;
    let package = parts.next()?;
    let relative = parts.next()?;
    if !scope.starts_with('@') || package.is_empty() || relative.is_empty() {
        return None;
    }
    let namespace_len = scope.len() + package.len() + 1;
    Some((&source[..namespace_len], relative))
}

fn resolve_ax_import_extension(path: PathBuf, kind: AxSourceKind) -> PathBuf {
    if path.extension().is_some() {
        return path;
    }

    if kind == AxSourceKind::Page {
        let canonical = path.with_extension("asx");
        if canonical.is_file() {
            return canonical;
        }
    }

    let legacy = path.with_extension("ax");
    if legacy.is_file() || kind == AxSourceKind::Backend {
        return legacy;
    }

    path.with_extension("asx")
}

fn normalize_ax_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
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
    pub use super::{
        ax_source_imports, classify_ax_source, diagnose_ax_source, diagnose_ax_workspace_imports,
        resolve_ax_import_path, AxLanguageDiagnostic, AxLanguageImport, AxSourceKind,
    };
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_workspace(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be available")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("axonyx-language-{name}-{nonce}"));
        fs::create_dir_all(root.join("app/components")).expect("workspace should be created");
        root
    }

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

    #[test]
    fn resolves_backend_relative_and_frontend_app_alias_imports() {
        let root = temp_workspace("local-imports");
        let page = root.join("app/page.asx");
        let loader = root.join("app/posts/loader.ax");
        fs::create_dir_all(root.join("app/posts")).expect("route should be created");
        fs::write(
            root.join("app/components/Card.asx"),
            "component Card { render ASX { <article /> } }",
        )
        .expect("component should be written");
        fs::write(
            root.join("app/posts/domain.ax"),
            "export fn visible() -> Bool { return true }",
        )
        .expect("backend module should be written");
        let packages = BTreeMap::new();

        assert_eq!(
            resolve_ax_import_path(&root, &loader, AxSourceKind::Backend, "./domain", &packages,),
            Some(root.join("app/posts/domain.ax"))
        );
        assert_eq!(
            resolve_ax_import_path(
                &root,
                &page,
                AxSourceKind::Page,
                "@/components/Card",
                &packages,
            ),
            Some(root.join("app/components/Card.asx"))
        );

        fs::remove_dir_all(root).expect("workspace should be removed");
    }

    #[test]
    fn diagnoses_relative_frontend_imports_until_runtime_has_parent_context() {
        let root = temp_workspace("relative-frontend");
        let page = root.join("app/page.asx");
        fs::write(
            root.join("app/components/Card.asx"),
            "component Card { render ASX { <article /> } }",
        )
        .expect("component should be written");
        let source =
            "import { Card } from \"./components/Card\"\n\npage Home() { return ASX { <Card /> } }";

        let diagnostics = diagnose_ax_workspace_imports(&root, &page, source, &BTreeMap::new());

        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("use an `@/` app alias"));
        fs::remove_dir_all(root).expect("workspace should be removed");
    }

    #[test]
    fn diagnoses_missing_local_and_package_imports_at_the_import_line() {
        let root = temp_workspace("missing-imports");
        let page = root.join("app/page.asx");
        let source = "import { Card } from \"@/components/Card\"\nimport { Button } from \"@axonyx/ui/Button\"\n\npage Home() { return ASX { <Card /> } }";
        let diagnostics = diagnose_ax_workspace_imports(&root, &page, source, &BTreeMap::new());

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].line, 1);
        assert_eq!(diagnostics[0].code, "axonyx-import");
        assert_eq!(diagnostics[1].line, 2);
        assert!(diagnostics[1].message.contains("@axonyx/ui/Button"));

        fs::remove_dir_all(root).expect("workspace should be removed");
    }

    #[test]
    fn resolves_package_imports_and_rejects_workspace_escape() {
        let root = temp_workspace("package-imports");
        let package_root = root.join("packages/ui/src/ax");
        fs::create_dir_all(&package_root).expect("package root should be created");
        fs::write(
            package_root.join("Button.asx"),
            "component Button { render ASX { <button /> } }",
        )
        .expect("package component should be written");
        let packages = BTreeMap::from([("@axonyx/ui".to_string(), package_root.clone())]);
        let page = root.join("app/page.asx");

        assert_eq!(
            resolve_ax_import_path(
                &root,
                &page,
                AxSourceKind::Page,
                "@axonyx/ui/Button",
                &packages,
            ),
            Some(package_root.join("Button.asx"))
        );
        assert_eq!(
            resolve_ax_import_path(&root, &page, AxSourceKind::Page, "../../outside", &packages,),
            None
        );

        fs::remove_dir_all(root).expect("workspace should be removed");
    }
}
