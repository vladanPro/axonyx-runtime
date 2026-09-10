use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::ax_backend_parser::{parse_backend_ax, parse_backend_ax_with_span, AxBackendSourceSpan};
use crate::ax_parser::AxParseError;
use crate::ax_parser_auto::{
    convert_ax_v2_file, looks_like_ax_v2, parse_ax_auto, AxAutoParseError,
};
use crate::ax_parser_v2::{
    parse_ax_component_module_v2, parse_ax_v2_with_span, AxParseV2Error, AxSourceSpanV2,
};
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
    pub end_line: usize,
    pub end_column: usize,
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxLanguageImport {
    pub source: String,
    pub line: usize,
    pub bindings: Vec<AxLanguageImportBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxLanguageImportBinding {
    pub imported: String,
    pub local: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxLanguageSymbolKind {
    Page,
    Layout,
    Component,
    Function,
    Type,
    Query,
    Action,
    Scope,
    Job,
}

impl AxLanguageSymbolKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Layout => "layout",
            Self::Component => "component",
            Self::Function => "function",
            Self::Type => "type",
            Self::Query => "query",
            Self::Action => "action",
            Self::Scope => "scope",
            Self::Job => "job",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxLanguageSymbol {
    pub name: String,
    pub line: usize,
    pub column: usize,
    pub kind: AxLanguageSymbolKind,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxLanguageIdentifierOccurrence {
    pub name: String,
    pub line: usize,
    pub column: usize,
    pub end_column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxLanguageComponentContract {
    pub name: String,
    pub props: Vec<AxLanguageComponentProp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxLanguageComponentProp {
    pub name: String,
    pub ty: Option<String>,
    pub default: Option<String>,
    pub required: bool,
    pub allowed_values: Vec<String>,
}

impl AxLanguageDiagnostic {
    fn error(line: usize, code: &'static str, message: impl Into<String>) -> Self {
        let line = line.max(1);
        Self {
            line,
            column: 1,
            end_line: line,
            end_column: 2,
            code,
            message: message.into(),
        }
    }

    fn error_at(span: AxSourceSpanV2, code: &'static str, message: impl Into<String>) -> Self {
        Self::error_range(
            span.line,
            span.column,
            span.end_line,
            span.end_column,
            code,
            message,
        )
    }

    fn backend_error_at(
        span: AxBackendSourceSpan,
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self::error_range(
            span.line,
            span.column,
            span.end_line,
            span.end_column,
            code,
            message,
        )
    }

    fn error_range(
        line: usize,
        column: usize,
        end_line: usize,
        end_column: usize,
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            line: line.max(1),
            column: column.max(1),
            end_line: end_line.max(line).max(1),
            end_column: end_column.max(column + 1),
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
        AxSourceKind::Backend => parse_backend_ax_with_span(source).err().map(|failure| {
            AxLanguageDiagnostic::backend_error_at(
                failure.span,
                "axonyx-backend-parse",
                failure.error.to_string(),
            )
        }),
    };

    diagnostic.into_iter().collect()
}

pub fn ax_source_imports(path: &str, source: &str) -> Vec<AxLanguageImport> {
    let parsed = match classify_ax_source(path, source) {
        AxSourceKind::Backend => parse_backend_ax(source)
            .ok()
            .map(|document| {
                document
                    .imports
                    .into_iter()
                    .map(|import| AxLanguageImport {
                        line: import_source_line(source, &import.source),
                        source: import.source,
                        bindings: import
                            .bindings
                            .into_iter()
                            .map(|binding| AxLanguageImportBinding {
                                imported: binding.imported,
                                local: binding.local,
                            })
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        AxSourceKind::Page => page_language_imports(source).unwrap_or_default(),
    };

    if parsed.is_empty() {
        tolerant_language_imports(source)
    } else {
        parsed
    }
}

pub fn ax_source_symbols(_path: &str, source: &str) -> Vec<AxLanguageSymbol> {
    source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| parse_language_symbol(line, index + 1))
        .collect()
}

pub fn ax_source_identifier_occurrences(source: &str) -> Vec<AxLanguageIdentifierOccurrence> {
    let mut occurrences = Vec::new();
    let mut block_comment = false;

    for (line_index, line) in source.lines().enumerate() {
        let mut index = 0usize;
        let mut quote = None;
        let mut escaped = false;

        while index < line.len() {
            let rest = &line[index..];
            if block_comment {
                if rest.starts_with("*/") {
                    block_comment = false;
                    index += 2;
                } else {
                    index += rest
                        .chars()
                        .next()
                        .expect("non-empty source remainder")
                        .len_utf8();
                }
                continue;
            }

            if let Some(delimiter) = quote {
                let character = rest.chars().next().expect("non-empty source remainder");
                index += character.len_utf8();
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == delimiter {
                    quote = None;
                }
                continue;
            }

            if rest.starts_with("//") {
                break;
            }
            if rest.starts_with("/*") {
                block_comment = true;
                index += 2;
                continue;
            }

            let character = rest.chars().next().expect("non-empty source remainder");
            if matches!(character, '\'' | '"' | '`') {
                quote = Some(character);
                index += character.len_utf8();
                continue;
            }
            if !is_language_identifier_start(character) {
                index += character.len_utf8();
                continue;
            }

            let start = index;
            index += character.len_utf8();
            while index < line.len() {
                let next = line[index..]
                    .chars()
                    .next()
                    .expect("non-empty source remainder");
                if !is_language_identifier_continue(next) {
                    break;
                }
                index += next.len_utf8();
            }

            occurrences.push(AxLanguageIdentifierOccurrence {
                name: line[start..index].to_string(),
                line: line_index + 1,
                column: line[..start].encode_utf16().count() + 1,
                end_column: line[..index].encode_utf16().count() + 1,
            });
        }
    }

    occurrences
}

fn is_language_identifier_start(character: char) -> bool {
    character.is_ascii_alphabetic() || character == '_'
}

fn is_language_identifier_continue(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

pub fn ax_source_component_contracts(source: &str) -> Vec<AxLanguageComponentContract> {
    let parsed: Vec<AxLanguageComponentContract> = parse_ax_component_module_v2(source)
        .ok()
        .flatten()
        .map(|file| {
            file.components
                .into_iter()
                .map(|component| AxLanguageComponentContract {
                    name: component.name,
                    props: component
                        .params
                        .into_iter()
                        .map(|prop| language_component_prop(prop.name, prop.ty, prop.default))
                        .collect(),
                })
                .collect()
        })
        .unwrap_or_default();

    if parsed.is_empty() {
        tolerant_language_component_contracts(source)
    } else {
        parsed
    }
}

fn language_component_prop(
    name: String,
    ty: Option<String>,
    default: Option<String>,
) -> AxLanguageComponentProp {
    let allowed_values = ty
        .as_deref()
        .map(language_literal_union_values)
        .unwrap_or_default();
    AxLanguageComponentProp {
        name,
        required: default.is_none() && !ty.as_deref().is_some_and(language_type_is_optional),
        ty,
        default,
        allowed_values,
    }
}

fn tolerant_language_component_contracts(source: &str) -> Vec<AxLanguageComponentContract> {
    let mut contracts = Vec::new();
    let mut offset = 0usize;

    for line in source.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let leading = content.len() - content.trim_start().len();
        let declaration = &source[offset + leading..];
        if content
            .trim_start()
            .strip_prefix("export ")
            .unwrap_or(content.trim_start())
            .starts_with("component ")
        {
            if let Some(contract) = tolerant_language_component_contract(declaration) {
                contracts.push(contract);
            }
        }
        offset += line.len();
    }

    contracts
}

fn tolerant_language_component_contract(line: &str) -> Option<AxLanguageComponentContract> {
    let declaration = line.trim_start();
    let declaration = declaration.strip_prefix("export ").unwrap_or(declaration);
    let declaration = declaration.strip_prefix("component ")?;
    let name_end = declaration
        .char_indices()
        .find(|(_, character)| !character.is_ascii_alphanumeric() && *character != '_')
        .map(|(index, _)| index)
        .unwrap_or(declaration.len());
    let name = declaration.get(..name_end)?;
    if !valid_language_identifier(name) {
        return None;
    }

    let remainder = declaration.get(name_end..)?.trim_start();
    let Some(params_source) = remainder.strip_prefix('(') else {
        return Some(AxLanguageComponentContract {
            name: name.to_string(),
            props: Vec::new(),
        });
    };
    let (params_source, complete) = tolerant_component_param_source(params_source);
    let mut params = split_tolerant_component_params(params_source);
    if !complete && !params_source.trim_end().ends_with(',') {
        params.pop();
    }

    Some(AxLanguageComponentContract {
        name: name.to_string(),
        props: params
            .into_iter()
            .filter_map(parse_tolerant_component_prop)
            .collect(),
    })
}

fn tolerant_component_param_source(source: &str) -> (&str, bool) {
    let mut quote = None;
    let mut escaped = false;
    let mut paren_depth = 0usize;

    for (index, character) in source.char_indices() {
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == delimiter {
                quote = None;
            }
            continue;
        }

        match character {
            '\'' | '"' => quote = Some(character),
            '(' => paren_depth += 1,
            ')' if paren_depth == 0 => return (&source[..index], true),
            ')' => paren_depth -= 1,
            _ => {}
        }
    }

    (source, false)
}

fn split_tolerant_component_params(source: &str) -> Vec<&str> {
    let mut params = Vec::new();
    let mut start = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut angle_depth = 0usize;

    for (index, character) in source.char_indices() {
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == delimiter {
                quote = None;
            }
            continue;
        }

        match character {
            '\'' | '"' => quote = Some(character),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            '<' => angle_depth += 1,
            '>' => angle_depth = angle_depth.saturating_sub(1),
            ',' if paren_depth == 0
                && bracket_depth == 0
                && brace_depth == 0
                && angle_depth == 0 =>
            {
                params.push(source[start..index].trim());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    if start < source.len() {
        params.push(source[start..].trim());
    }
    params
}

fn parse_tolerant_component_prop(source: &str) -> Option<AxLanguageComponentProp> {
    let source = source.trim();
    if source.is_empty() {
        return None;
    }
    let equals = top_level_component_param_separator(source, '=');
    let declaration = equals.map(|index| &source[..index]).unwrap_or(source);
    let colon = top_level_component_param_separator(declaration, ':');
    let name = colon
        .map(|index| &declaration[..index])
        .unwrap_or(declaration)
        .trim();
    if !valid_language_identifier(name) {
        return None;
    }

    let ty = colon
        .map(|index| declaration[index + 1..].trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let default = equals
        .map(|index| source[index + 1..].trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Some(language_component_prop(name.to_string(), ty, default))
}

fn top_level_component_param_separator(source: &str, separator: char) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut angle_depth = 0usize;

    for (index, character) in source.char_indices() {
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == delimiter {
                quote = None;
            }
            continue;
        }

        match character {
            '\'' | '"' => quote = Some(character),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            '<' => angle_depth += 1,
            '>' => angle_depth = angle_depth.saturating_sub(1),
            _ if character == separator
                && paren_depth == 0
                && bracket_depth == 0
                && brace_depth == 0
                && angle_depth == 0 =>
            {
                return Some(index);
            }
            _ => {}
        }
    }
    None
}

fn language_type_is_optional(ty: &str) -> bool {
    let ty = ty.trim();
    ty.ends_with('?') || (ty.starts_with("Optional<") && ty.ends_with('>'))
}

fn language_literal_union_values(ty: &str) -> Vec<String> {
    let mut ty = ty.trim();
    if let Some(inner) = ty.strip_suffix('?') {
        ty = inner.trim();
    }
    if let Some(inner) = ty
        .strip_prefix("Optional<")
        .and_then(|inner| inner.strip_suffix('>'))
    {
        ty = inner.trim();
    }

    let values = ty
        .split('|')
        .map(str::trim)
        .map(|value| serde_json::from_str::<String>(value).ok())
        .collect::<Option<Vec<_>>>();
    values
        .filter(|values| !values.is_empty())
        .unwrap_or_default()
}

fn tolerant_language_imports(source: &str) -> Vec<AxLanguageImport> {
    source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| tolerant_language_import(line, index + 1))
        .collect()
}

fn tolerant_language_import(line: &str, line_number: usize) -> Option<AxLanguageImport> {
    let declaration = line.trim().strip_prefix("import ")?;
    let (bindings_source, source_literal) = declaration.rsplit_once(" from ")?;
    let source = source_literal
        .trim()
        .strip_suffix(';')
        .unwrap_or(source_literal.trim())
        .trim();
    let quote = source.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let source = source.strip_prefix(quote)?.strip_suffix(quote)?;
    if source.is_empty() {
        return None;
    }

    let bindings_source = bindings_source.trim();
    let bindings = if let Some(local) = bindings_source.strip_prefix("* as ") {
        let local = local.trim();
        valid_language_identifier(local).then(|| {
            vec![AxLanguageImportBinding {
                imported: "*".to_string(),
                local: local.to_string(),
            }]
        })?
    } else {
        let named = bindings_source.strip_prefix('{')?.strip_suffix('}')?;
        let bindings = named
            .split(',')
            .filter_map(|binding| {
                let binding = binding.trim();
                if binding.is_empty() {
                    return None;
                }
                let (imported, local) = binding
                    .split_once(" as ")
                    .map(|(imported, local)| (imported.trim(), local.trim()))
                    .unwrap_or((binding, binding));
                (valid_language_identifier(imported) && valid_language_identifier(local)).then(
                    || AxLanguageImportBinding {
                        imported: imported.to_string(),
                        local: local.to_string(),
                    },
                )
            })
            .collect::<Vec<_>>();
        (!bindings.is_empty()).then_some(bindings)?
    };

    Some(AxLanguageImport {
        source: source.to_string(),
        line: line_number,
        bindings,
    })
}

fn valid_language_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
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

fn page_language_imports(source: &str) -> Option<Vec<AxLanguageImport>> {
    if let Ok(Some(file)) = parse_ax_component_module_v2(source) {
        return Some(
            file.imports
                .into_iter()
                .map(|import| AxLanguageImport {
                    line: import_source_line(source, &import.source),
                    source: import.source,
                    bindings: import
                        .bindings
                        .into_iter()
                        .map(|binding| AxLanguageImportBinding {
                            imported: binding.imported,
                            local: binding.local,
                        })
                        .collect(),
                })
                .collect(),
        );
    }

    parse_ax_auto(source).ok().map(|document| {
        document
            .imports
            .into_iter()
            .map(|import| AxLanguageImport {
                line: import_source_line(source, &import.source),
                source: import.source,
                bindings: import
                    .bindings
                    .into_iter()
                    .map(|binding| AxLanguageImportBinding {
                        imported: binding.imported,
                        local: binding.local,
                    })
                    .collect(),
            })
            .collect()
    })
}

fn parse_language_symbol(line: &str, line_number: usize) -> Option<AxLanguageSymbol> {
    let trimmed = line.trim_start();
    let declaration = trimmed.strip_prefix("export ").unwrap_or(trimmed);
    let declarations = [
        ("page", AxLanguageSymbolKind::Page),
        ("layout", AxLanguageSymbolKind::Layout),
        ("component", AxLanguageSymbolKind::Component),
        ("fn", AxLanguageSymbolKind::Function),
        ("type", AxLanguageSymbolKind::Type),
        ("query", AxLanguageSymbolKind::Query),
        ("loader", AxLanguageSymbolKind::Query),
        ("action", AxLanguageSymbolKind::Action),
        ("scope", AxLanguageSymbolKind::Scope),
        ("job", AxLanguageSymbolKind::Job),
    ];

    for (keyword, kind) in declarations {
        let Some(rest) = declaration.strip_prefix(keyword) else {
            continue;
        };
        if !rest.chars().next().is_some_and(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim_start();
        let name = rest
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
            .collect::<String>();
        if name.is_empty() {
            return None;
        }
        let name_offset = line.find(&name)?;
        return Some(AxLanguageSymbol {
            name,
            line: line_number,
            column: line[..name_offset].encode_utf16().count() + 1,
            kind,
            signature: declaration_signature(declaration),
        });
    }

    None
}

fn declaration_signature(declaration: &str) -> String {
    let mut quote = None;
    let mut escaped = false;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;

    for (index, character) in declaration.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active_quote {
                quote = None;
            }
            continue;
        }

        match character {
            '\'' | '"' => quote = Some(character),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' if paren_depth == 0 && bracket_depth == 0 => {
                return declaration[..index].trim_end().to_string();
            }
            _ => {}
        }
    }

    declaration.trim_end_matches([' ', ';']).to_string()
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

    if looks_like_ax_v2(source) {
        let file = match parse_ax_v2_with_span(source) {
            Ok(file) => file,
            Err(failure) => {
                return Some(AxLanguageDiagnostic::error_at(
                    failure.span,
                    "axonyx-parse",
                    failure.error.to_string(),
                ));
            }
        };
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
    error.line()
}

pub mod prelude {
    pub use super::{
        ax_source_component_contracts, ax_source_identifier_occurrences, ax_source_imports,
        ax_source_symbols, classify_ax_source, diagnose_ax_source, diagnose_ax_workspace_imports,
        resolve_ax_import_path, AxLanguageComponentContract, AxLanguageComponentProp,
        AxLanguageDiagnostic, AxLanguageIdentifierOccurrence, AxLanguageImport,
        AxLanguageImportBinding, AxLanguageSymbol, AxLanguageSymbolKind, AxSourceKind,
    };
    pub use crate::ax_local_symbols::{
        ax_source_local_symbols, AxLanguageLocalSymbol, AxLanguageLocalSymbolKind,
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
    fn reports_precise_page_parser_span() {
        let diagnostics = diagnose_ax_source(
            "app/page.asx",
            "page Home() {\n  return ASX {\n    <Card>\n    </Grid>\n  }\n}\n",
        );

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].line, 4);
        assert_eq!(diagnostics[0].column, 7);
        assert_eq!(diagnostics[0].end_line, 4);
        assert_eq!(diagnostics[0].end_column, 11);
        assert_eq!(diagnostics[0].code, "axonyx-parse");
    }

    #[test]
    fn reports_page_parser_columns_as_utf16() {
        let source = "page Home() {\n  return ASX {\n    <Copy>🔥</Copy><Card title= />\n  }\n}";
        let diagnostics = diagnose_ax_source("app/page.asx", source);
        let error_line = source.lines().nth(2).expect("error line should exist");
        let title_offset = error_line.find("title").expect("title should exist");
        let expected_column = error_line[..title_offset].encode_utf16().count() + 1;

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].line, 3);
        assert_eq!(diagnostics[0].column, expected_column);
        assert_eq!(diagnostics[0].end_column, expected_column + "title".len());
    }

    #[test]
    fn reports_backend_parser_line() {
        let diagnostics = diagnose_ax_source(
            "app/posts/loader.ax",
            "query loadPosts() -> Post[] {\n  nope ???\n}\n",
        );

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].line, 2);
        assert_eq!(diagnostics[0].column, 3);
        assert_eq!(diagnostics[0].end_line, 2);
        assert_eq!(diagnostics[0].end_column, 7);
        assert_eq!(diagnostics[0].code, "axonyx-backend-parse");
    }

    #[test]
    fn reports_backend_parser_columns_as_utf16() {
        let diagnostics = diagnose_ax_source(
            "app/posts/loader.ax",
            "query loadPosts() -> Post[] {\n  🔥 nope\n}\n",
        );

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].line, 2);
        assert_eq!(diagnostics[0].column, 3);
        assert_eq!(diagnostics[0].end_line, 2);
        assert_eq!(diagnostics[0].end_column, 5);
        assert_eq!(diagnostics[0].code, "axonyx-backend-parse");
    }

    #[test]
    fn reports_backend_expression_operator_range() {
        let diagnostics = diagnose_ax_source(
            "app/posts/domain.ax",
            "fn normalize(status: String) -> String {\n  return status ??\n}\n",
        );

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].line, 2);
        assert_eq!(diagnostics[0].column, 17);
        assert_eq!(diagnostics[0].end_line, 2);
        assert_eq!(diagnostics[0].end_column, 19);
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
    fn exposes_named_alias_and_namespace_import_bindings() {
        let frontend = ax_source_imports(
            "app/page.asx",
            "import { Card as Panel } from \"@/components/Card\"\nimport * as Domain from \"@/domain\"\n\npage Home() { return ASX { <Panel /> } }",
        );

        assert_eq!(frontend.len(), 2);
        assert_eq!(frontend[0].bindings[0].imported, "Card");
        assert_eq!(frontend[0].bindings[0].local, "Panel");
        assert_eq!(frontend[1].bindings[0].imported, "*");
        assert_eq!(frontend[1].bindings[0].local, "Domain");

        let backend = ax_source_imports(
            "app/posts/loader.ax",
            "import { visible as isVisible } from \"./domain.ax\"\n\nquery loadPosts() -> Post[] {\n  return []\n}",
        );
        assert_eq!(backend[0].bindings[0].imported, "visible");
        assert_eq!(backend[0].bindings[0].local, "isVisible");
    }

    #[test]
    fn indexes_complete_imports_and_declarations_while_the_body_is_incomplete() {
        let source = "import { Card as Panel } from \"@/components/Card\"\n\ncomponent Preview() {\n  render ASX {\n    <Pan";

        let imports = ax_source_imports("app/components/Preview.asx", source);
        let symbols = ax_source_symbols("app/components/Preview.asx", source);

        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].source, "@/components/Card");
        assert_eq!(imports[0].bindings[0].imported, "Card");
        assert_eq!(imports[0].bindings[0].local, "Panel");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Preview");
        assert_eq!(symbols[0].kind, AxLanguageSymbolKind::Component);
    }

    #[test]
    fn indexes_frontend_and_backend_declaration_locations() {
        let frontend = ax_source_symbols(
            "app/components/Card.asx",
            "component Card(title = \"\") {\n  render ASX {\n    <article>{title}</article>\n  }\n}",
        );
        assert_eq!(frontend.len(), 1);
        assert_eq!(frontend[0].name, "Card");
        assert_eq!(frontend[0].line, 1);
        assert_eq!(frontend[0].column, 11);
        assert_eq!(frontend[0].kind, AxLanguageSymbolKind::Component);
        assert_eq!(frontend[0].kind.label(), "component");
        assert_eq!(frontend[0].signature, "component Card(title = \"\")");

        let backend = ax_source_symbols(
            "app/posts/domain.ax",
            "export type Post {\n  title: String\n}\n\nexport fn visible(post: Post) -> Bool {\n  return true\n}",
        );
        assert_eq!(backend.len(), 2);
        assert_eq!(backend[0].name, "Post");
        assert_eq!(backend[0].kind, AxLanguageSymbolKind::Type);
        assert_eq!(backend[1].name, "visible");
        assert_eq!(backend[1].line, 5);
        assert_eq!(backend[1].kind, AxLanguageSymbolKind::Function);
        assert_eq!(backend[1].signature, "fn visible(post: Post) -> Bool");
    }

    #[test]
    fn symbol_signature_preserves_nested_default_values() {
        assert_eq!(
            declaration_signature("fn build(options: Config = { mode: \"safe\" }) -> Result {"),
            "fn build(options: Config = { mode: \"safe\" }) -> Result"
        );
        assert_eq!(
            declaration_signature("type Theme = \"silver\" | \"gold\";"),
            "type Theme = \"silver\" | \"gold\""
        );
    }

    #[test]
    fn identifier_index_uses_utf16_columns_and_skips_strings_and_comments() {
        let source = concat!(
            "component Card() {\n",
            "  return ASX { <Card title=\"Card\">Čelik {Card()}</Card> }\n",
            "  // Card()\n",
            "  /* Card() */ Card()\n",
            "}\n",
        );

        let cards = ax_source_identifier_occurrences(source)
            .into_iter()
            .filter(|occurrence| occurrence.name == "Card")
            .collect::<Vec<_>>();

        assert_eq!(cards.len(), 5);
        assert_eq!((cards[0].line, cards[0].column), (1, 11));
        assert_eq!((cards[1].line, cards[1].column), (2, 17));
        assert_eq!((cards[2].line, cards[2].column), (2, 42));
        assert_eq!((cards[3].line, cards[3].column), (2, 51));
        assert_eq!((cards[4].line, cards[4].column), (4, 16));
    }

    #[test]
    fn exposes_typed_component_prop_contracts_and_literal_values() {
        let contracts = ax_source_component_contracts(
            r#"
component Button(label: String, variant: "primary" | "ghost" = "primary", size: Optional<"sm" | "md" | "lg">, disabled: Bool = false) {
  render ASX {
    <button>{label}</button>
  }
}
"#,
        );

        assert_eq!(contracts.len(), 1);
        assert_eq!(contracts[0].name, "Button");
        assert_eq!(contracts[0].props.len(), 4);
        assert!(contracts[0].props[0].required);
        assert_eq!(
            contracts[0].props[1].allowed_values,
            vec!["primary", "ghost"]
        );
        assert!(!contracts[0].props[1].required);
        assert_eq!(contracts[0].props[2].allowed_values, vec!["sm", "md", "lg"]);
        assert!(!contracts[0].props[2].required);
        assert_eq!(contracts[0].props[3].default.as_deref(), Some("false"));
    }

    #[test]
    fn retains_component_contract_when_the_surrounding_body_is_incomplete() {
        let contracts = ax_source_component_contracts(
            r#"
component Button(label: String, variant: "primary" | "ghost" = "primary") {
  render ASX {
    <button>{label}
"#,
        );

        assert_eq!(contracts.len(), 1);
        assert_eq!(contracts[0].name, "Button");
        assert_eq!(contracts[0].props.len(), 2);
        assert!(contracts[0].props[0].required);
        assert_eq!(
            contracts[0].props[1].allowed_values,
            vec!["primary", "ghost"]
        );
    }

    #[test]
    fn retains_multiline_component_contract_when_the_page_is_incomplete() {
        let contracts = ax_source_component_contracts(
            r#"
component Button(
  label: String,
  variant: "primary" | "ghost" = "primary",
) {
  render ASX { <button>{label}</button> }
}

page Home() { return ASX { <Button va
"#,
        );

        assert_eq!(contracts.len(), 1);
        assert_eq!(contracts[0].props.len(), 2);
        assert_eq!(contracts[0].props[0].name, "label");
        assert_eq!(
            contracts[0].props[1].allowed_values,
            vec!["primary", "ghost"]
        );
    }

    #[test]
    fn tolerant_component_contract_keeps_only_completed_partial_params() {
        let contracts = ax_source_component_contracts(
            r#"
component Button(label: String, variant: "primary" | "ghost" = "primary", disa
"#,
        );

        assert_eq!(contracts.len(), 1);
        assert_eq!(contracts[0].props.len(), 2);
        assert_eq!(contracts[0].props[0].name, "label");
        assert_eq!(contracts[0].props[1].name, "variant");
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
