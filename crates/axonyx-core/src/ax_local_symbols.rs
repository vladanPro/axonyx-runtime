use crate::ax_language_service::AxLanguageIdentifierOccurrence;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxLanguageLocalSymbolKind {
    Parameter,
    State,
    Data,
    Constant,
    Variable,
}

impl AxLanguageLocalSymbolKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Parameter => "parameter",
            Self::State => "state",
            Self::Data => "data",
            Self::Constant => "constant",
            Self::Variable => "variable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxLanguageLocalSymbol {
    pub name: String,
    pub kind: AxLanguageLocalSymbolKind,
    pub declaration: AxLanguageIdentifierOccurrence,
    pub occurrences: Vec<AxLanguageIdentifierOccurrence>,
    pub scope_start: usize,
    pub scope_end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerKind {
    Page,
    Component,
    Function,
}

#[derive(Debug, Clone)]
struct Owner {
    kind: OwnerKind,
    start: usize,
    end: usize,
    open: Option<usize>,
    params: Option<(usize, usize)>,
    body: Option<(usize, usize)>,
}

#[derive(Debug, Clone)]
struct BackendOwner {
    kind: BackendOwnerKind,
    start: usize,
    end: usize,
    params: Option<(usize, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendOwnerKind {
    Callable,
    Route,
    Job,
    Scope,
}

#[derive(Debug, Clone)]
struct LocalDraft {
    name: String,
    kind: AxLanguageLocalSymbolKind,
    declaration_start: usize,
    declaration_end: usize,
    scope_start: usize,
    scope_end: usize,
}

#[derive(Debug, Clone, Copy)]
struct ByteRange {
    start: usize,
    end: usize,
}

/// Builds a conservative local-symbol index for Axonyx sources.
///
/// Only compiler-owned expression regions participate. ASX text, strings,
/// comments, property names, database fields, and embedded client/style blocks
/// are excluded so editor refactors fail closed instead of rewriting unrelated
/// text.
pub fn ax_source_local_symbols(source: &str) -> Vec<AxLanguageLocalSymbol> {
    let mask = code_mask(source);
    let line_starts = source_line_starts(source);
    let owners = collect_owners(&mask, &line_starts);
    if !owners.iter().any(|owner| owner.kind == OwnerKind::Page) {
        return backend_source_local_symbols(source, &mask, &line_starts);
    }

    let raw_ranges = collect_raw_ranges(&mask, &line_starts);
    let mut drafts = collect_parameter_drafts(&mask, &owners);
    let (declarations, initializer_ranges) =
        collect_local_declarations(&mask, &line_starts, &owners, &raw_ranges);
    drafts.extend(declarations);

    let mut expression_ranges = initializer_ranges;
    expression_ranges.extend(collect_parameter_default_ranges(&mask, &owners));
    expression_ranges.extend(
        owners
            .iter()
            .filter_map(|owner| {
                (owner.kind == OwnerKind::Function)
                    .then_some(owner.body)
                    .flatten()
            })
            .map(|(start, end)| ByteRange { start, end }),
    );
    expression_ranges.extend(collect_asx_expression_ranges(
        &mask,
        &line_starts,
        &owners,
        &raw_ranges,
    ));

    let identifiers = identifier_tokens(&mask);
    let mut resolved = vec![Vec::<(usize, usize)>::new(); drafts.len()];
    for &(start, end) in &identifiers {
        if let Some(index) = drafts
            .iter()
            .position(|draft| draft.declaration_start == start && draft.declaration_end == end)
        {
            resolved[index].push((start, end));
            continue;
        }
        if !expression_ranges
            .iter()
            .any(|range| range.start <= start && end <= range.end)
            || raw_ranges
                .iter()
                .any(|range| range.start <= start && end <= range.end)
            || is_member_or_object_key(&mask, start, end)
        {
            continue;
        }

        let name = &source[start..end];
        let candidate = drafts
            .iter()
            .enumerate()
            .filter(|(_, draft)| {
                draft.name == name
                    && draft.scope_start <= start
                    && end <= draft.scope_end
                    && draft.declaration_start <= start
            })
            .min_by_key(|(_, draft)| {
                (
                    draft.scope_end.saturating_sub(draft.scope_start),
                    usize::MAX - draft.declaration_start,
                )
            })
            .map(|(index, _)| index);
        if let Some(index) = candidate {
            resolved[index].push((start, end));
        }
    }

    drafts
        .into_iter()
        .enumerate()
        .map(|(index, draft)| {
            let declaration = occurrence(
                source,
                &line_starts,
                draft.declaration_start,
                draft.declaration_end,
            );
            let mut occurrences = resolved[index]
                .iter()
                .map(|&(start, end)| occurrence(source, &line_starts, start, end))
                .collect::<Vec<_>>();
            occurrences.sort_by_key(|item| (item.line, item.column));
            occurrences.dedup();
            AxLanguageLocalSymbol {
                name: draft.name,
                kind: draft.kind,
                declaration,
                occurrences,
                scope_start: draft.scope_start,
                scope_end: draft.scope_end,
            }
        })
        .collect()
}

fn backend_source_local_symbols(
    source: &str,
    mask: &[u8],
    line_starts: &[usize],
) -> Vec<AxLanguageLocalSymbol> {
    let owners = collect_backend_owners(mask, line_starts);
    if owners.is_empty() {
        return Vec::new();
    }

    let mut drafts = collect_backend_parameter_drafts(mask, &owners);
    let (declarations, initializer_ranges) =
        collect_backend_local_declarations(mask, line_starts, &owners);
    drafts.extend(declarations);

    let mut expression_ranges = initializer_ranges;
    expression_ranges.extend(collect_backend_parameter_default_ranges(mask, &owners));
    expression_ranges.extend(collect_backend_expression_ranges(
        mask,
        line_starts,
        &owners,
    ));

    resolve_local_drafts(source, mask, line_starts, drafts, &expression_ranges)
}

fn resolve_local_drafts(
    source: &str,
    mask: &[u8],
    line_starts: &[usize],
    drafts: Vec<LocalDraft>,
    expression_ranges: &[ByteRange],
) -> Vec<AxLanguageLocalSymbol> {
    let identifiers = identifier_tokens(mask);
    let mut resolved = vec![Vec::<(usize, usize)>::new(); drafts.len()];
    for &(start, end) in &identifiers {
        if let Some(index) = drafts
            .iter()
            .position(|draft| draft.declaration_start == start && draft.declaration_end == end)
        {
            resolved[index].push((start, end));
            continue;
        }
        if !expression_ranges
            .iter()
            .any(|range| range.start <= start && end <= range.end)
            || is_member_or_object_key(mask, start, end)
        {
            continue;
        }

        let name = &source[start..end];
        let candidate = drafts
            .iter()
            .enumerate()
            .filter(|(_, draft)| {
                draft.name == name
                    && draft.scope_start <= start
                    && end <= draft.scope_end
                    && draft.declaration_start <= start
            })
            .min_by_key(|(_, draft)| {
                (
                    draft.scope_end.saturating_sub(draft.scope_start),
                    usize::MAX - draft.declaration_start,
                )
            })
            .map(|(index, _)| index);
        if let Some(index) = candidate {
            resolved[index].push((start, end));
        }
    }

    drafts
        .into_iter()
        .enumerate()
        .map(|(index, draft)| {
            let declaration = occurrence(
                source,
                line_starts,
                draft.declaration_start,
                draft.declaration_end,
            );
            let mut occurrences = resolved[index]
                .iter()
                .map(|&(start, end)| occurrence(source, line_starts, start, end))
                .collect::<Vec<_>>();
            occurrences.sort_by_key(|item| (item.line, item.column));
            occurrences.dedup();
            AxLanguageLocalSymbol {
                name: draft.name,
                kind: draft.kind,
                declaration,
                occurrences,
                scope_start: draft.scope_start,
                scope_end: draft.scope_end,
            }
        })
        .collect()
}

fn collect_backend_owners(mask: &[u8], line_starts: &[usize]) -> Vec<BackendOwner> {
    let ranges = line_ranges(mask.len(), line_starts);
    let mut owners = Vec::new();
    for (line_index, &(line_start, line_end)) in ranges.iter().enumerate() {
        let mut cursor = skip_ascii_space(mask, line_start, line_end);
        if cursor != line_start {
            continue;
        }
        if keyword_at(mask, cursor, "export") {
            cursor = skip_ascii_space(mask, cursor + "export".len(), line_end);
        }

        let declaration = [
            ("loader", BackendOwnerKind::Callable),
            ("query", BackendOwnerKind::Callable),
            ("action", BackendOwnerKind::Callable),
            ("fn", BackendOwnerKind::Callable),
            ("route", BackendOwnerKind::Route),
            ("job", BackendOwnerKind::Job),
            ("scope", BackendOwnerKind::Scope),
        ]
        .into_iter()
        .find(|(keyword, _)| keyword_at(mask, cursor, keyword));
        let Some((keyword, kind)) = declaration else {
            continue;
        };
        cursor = skip_ascii_space(mask, cursor + keyword.len(), line_end);

        let params = if kind == BackendOwnerKind::Callable {
            identifier_at(mask, cursor).and_then(|(_, name_end)| {
                let open = skip_ascii_space(mask, name_end, mask.len());
                (mask.get(open) == Some(&b'('))
                    .then(|| matching_delimiter(mask, open, b'(', b')'))
                    .flatten()
                    .map(|close| (open + 1, close))
            })
        } else {
            None
        };

        let header_end = params.map_or(line_end, |(_, close)| {
            line_end_for_offset(mask.len(), line_starts, close)
        });
        let body_search_start = params.map_or(cursor, |(_, close)| close + 1);
        let open = find_byte(mask, body_search_start, header_end, b'{');
        let (start, end) = if let Some(open) = open {
            let Some(close) = matching_delimiter(mask, open, b'{', b'}') else {
                continue;
            };
            (open + 1, close)
        } else if kind == BackendOwnerKind::Scope {
            continue;
        } else {
            let end = ranges
                .iter()
                .skip(line_index + 1)
                .find_map(|&(start, end)| {
                    let content = skip_ascii_space(mask, start, end);
                    (content == start && content < end).then_some(start)
                })
                .unwrap_or(mask.len());
            (header_end, end)
        };
        owners.push(BackendOwner {
            kind,
            start,
            end,
            params,
        });
    }
    owners
}

fn collect_backend_parameter_drafts(mask: &[u8], owners: &[BackendOwner]) -> Vec<LocalDraft> {
    let mut drafts = Vec::new();
    for owner in owners {
        let Some((start, end)) = owner.params else {
            continue;
        };
        for range in split_top_level(mask, start, end, b',') {
            let cursor = skip_ascii_space(mask, range.start, range.end);
            let Some((name_start, name_end)) = identifier_at(mask, cursor) else {
                continue;
            };
            drafts.push(LocalDraft {
                name: String::from_utf8_lossy(&mask[name_start..name_end]).into_owned(),
                kind: AxLanguageLocalSymbolKind::Parameter,
                declaration_start: name_start,
                declaration_end: name_end,
                scope_start: start,
                scope_end: owner.end,
            });
        }
    }
    drafts
}

fn collect_backend_parameter_default_ranges(
    mask: &[u8],
    owners: &[BackendOwner],
) -> Vec<ByteRange> {
    owners
        .iter()
        .filter_map(|owner| owner.params)
        .flat_map(|(start, end)| split_top_level(mask, start, end, b','))
        .filter_map(|param| {
            find_top_level_equals(mask, param.start, param.end).map(|equals| ByteRange {
                start: equals + 1,
                end: param.end,
            })
        })
        .collect()
}

fn collect_backend_local_declarations(
    mask: &[u8],
    line_starts: &[usize],
    owners: &[BackendOwner],
) -> (Vec<LocalDraft>, Vec<ByteRange>) {
    let mut drafts = Vec::new();
    let mut expressions = Vec::new();
    for &(line_start, line_end) in &line_ranges(mask.len(), line_starts) {
        let cursor = skip_ascii_space(mask, line_start, line_end);
        let Some(owner) = innermost_backend_owner(owners, cursor) else {
            continue;
        };
        let (kind, keyword_len) =
            if owner.kind == BackendOwnerKind::Scope && keyword_at(mask, cursor, "state") {
                (AxLanguageLocalSymbolKind::State, 5)
            } else if owner.kind != BackendOwnerKind::Scope && keyword_at(mask, cursor, "data") {
                (AxLanguageLocalSymbolKind::Data, 4)
            } else if owner.kind != BackendOwnerKind::Scope && keyword_at(mask, cursor, "const") {
                (AxLanguageLocalSymbolKind::Constant, 5)
            } else if owner.kind != BackendOwnerKind::Scope && keyword_at(mask, cursor, "let") {
                (AxLanguageLocalSymbolKind::Variable, 3)
            } else {
                continue;
            };
        let declaration_start = skip_ascii_space(mask, cursor + keyword_len, line_end);
        let Some((name_start, name_end)) = identifier_at(mask, declaration_start) else {
            continue;
        };
        let Some(equals) = find_top_level_equals(mask, name_end, line_end) else {
            continue;
        };
        drafts.push(LocalDraft {
            name: String::from_utf8_lossy(&mask[name_start..name_end]).into_owned(),
            kind,
            declaration_start: name_start,
            declaration_end: name_end,
            scope_start: name_start,
            scope_end: owner.end,
        });
        expressions.push(ByteRange {
            start: equals + 1,
            end: line_end,
        });
    }
    (drafts, expressions)
}

fn collect_backend_expression_ranges(
    mask: &[u8],
    line_starts: &[usize],
    owners: &[BackendOwner],
) -> Vec<ByteRange> {
    let mut ranges = Vec::new();
    for &(line_start, line_end) in &line_ranges(mask.len(), line_starts) {
        let cursor = skip_ascii_space(mask, line_start, line_end);
        if innermost_backend_owner(owners, cursor).is_none()
            || ["state", "data", "const", "let"]
                .iter()
                .any(|keyword| keyword_at(mask, cursor, keyword))
        {
            continue;
        }

        let prefixed = [
            "return",
            "require",
            "revalidate",
            "invalidate",
            "before",
            "after",
            "patch",
            "header",
            "cookie",
            "clearCookie",
            "render",
        ]
        .iter()
        .find(|keyword| keyword_at(mask, cursor, keyword))
        .map(|keyword| skip_ascii_space(mask, cursor + keyword.len(), line_end));
        let start = if let Some(start) = prefixed {
            start
        } else if keyword_at(mask, cursor, "send") {
            find_mask_sequence(mask, cursor + 4, line_end, b" with ")
                .map(|index| index + " with ".len())
                .unwrap_or(line_end)
        } else if keyword_at(mask, cursor, "where")
            || find_byte(mask, cursor, line_end, b'=').is_some()
        {
            find_byte(mask, cursor, line_end, b'=')
                .map(|index| index + 1)
                .unwrap_or(line_end)
        } else if find_byte(mask, cursor, line_end, b'(').is_some() {
            cursor
        } else {
            continue;
        };
        if start < line_end {
            ranges.push(ByteRange {
                start,
                end: line_end,
            });
        }
    }
    ranges
}

fn innermost_backend_owner(owners: &[BackendOwner], offset: usize) -> Option<&BackendOwner> {
    owners
        .iter()
        .filter(|owner| owner.start <= offset && offset <= owner.end)
        .min_by_key(|owner| owner.end.saturating_sub(owner.start))
}

fn find_mask_sequence(mask: &[u8], start: usize, end: usize, target: &[u8]) -> Option<usize> {
    mask.get(start..end)?
        .windows(target.len())
        .position(|window| window == target)
        .map(|offset| start + offset)
}

fn collect_owners(mask: &[u8], line_starts: &[usize]) -> Vec<Owner> {
    let mut owners = Vec::new();
    for &(line_start, line_end) in line_ranges(mask.len(), line_starts).iter() {
        let mut cursor = skip_ascii_space(mask, line_start, line_end);
        if keyword_at(mask, cursor, "export") {
            cursor = skip_ascii_space(mask, cursor + "export".len(), line_end);
        }
        let (kind, keyword_len) = if keyword_at(mask, cursor, "page") {
            (OwnerKind::Page, 4)
        } else if keyword_at(mask, cursor, "component") {
            (OwnerKind::Component, 9)
        } else if keyword_at(mask, cursor, "fn") {
            (OwnerKind::Function, 2)
        } else {
            continue;
        };
        cursor = skip_ascii_space(mask, cursor + keyword_len, mask.len());
        let Some((_, name_end)) = identifier_at(mask, cursor) else {
            continue;
        };
        cursor = skip_ascii_space(mask, name_end, mask.len());
        let params = if mask.get(cursor) == Some(&b'(') {
            matching_delimiter(mask, cursor, b'(', b')').map(|end| (cursor + 1, end))
        } else {
            None
        };
        if let Some((_, end)) = params {
            cursor = skip_ascii_space(mask, end + 1, mask.len());
        }

        match kind {
            OwnerKind::Function => {
                let body_line_end = line_end_for_offset(mask.len(), line_starts, cursor);
                let Some(equals) = find_byte(mask, cursor, body_line_end, b'=') else {
                    continue;
                };
                owners.push(Owner {
                    kind,
                    start: equals + 1,
                    end: body_line_end,
                    open: None,
                    params,
                    body: Some((equals + 1, body_line_end)),
                });
            }
            OwnerKind::Page if params.is_none() => owners.push(Owner {
                kind,
                start: line_end,
                end: mask.len(),
                open: None,
                params,
                body: Some((line_end, mask.len())),
            }),
            OwnerKind::Page | OwnerKind::Component => {
                let Some(open) = find_byte(mask, cursor, mask.len(), b'{') else {
                    continue;
                };
                let Some(close) = matching_delimiter(mask, open, b'{', b'}') else {
                    continue;
                };
                owners.push(Owner {
                    kind,
                    start: open + 1,
                    end: close,
                    open: Some(open),
                    params,
                    body: Some((open + 1, close)),
                });
            }
        }
    }
    owners
}

fn collect_parameter_drafts(mask: &[u8], owners: &[Owner]) -> Vec<LocalDraft> {
    let mut drafts = Vec::new();
    for owner in owners {
        let Some((start, end)) = owner.params else {
            continue;
        };
        for range in split_top_level(mask, start, end, b',') {
            let cursor = skip_ascii_space(mask, range.start, range.end);
            let Some((name_start, name_end)) = identifier_at(mask, cursor) else {
                continue;
            };
            drafts.push(LocalDraft {
                name: String::from_utf8_lossy(&mask[name_start..name_end]).into_owned(),
                kind: AxLanguageLocalSymbolKind::Parameter,
                declaration_start: name_start,
                declaration_end: name_end,
                scope_start: owner.start,
                scope_end: owner.end,
            });
        }
    }
    drafts
}

fn collect_parameter_default_ranges(mask: &[u8], owners: &[Owner]) -> Vec<ByteRange> {
    owners
        .iter()
        .filter_map(|owner| owner.params)
        .flat_map(|(start, end)| split_top_level(mask, start, end, b','))
        .filter_map(|param| {
            find_top_level_equals(mask, param.start, param.end).map(|equals| ByteRange {
                start: equals + 1,
                end: param.end,
            })
        })
        .collect()
}

fn collect_local_declarations(
    mask: &[u8],
    line_starts: &[usize],
    owners: &[Owner],
    raw_ranges: &[ByteRange],
) -> (Vec<LocalDraft>, Vec<ByteRange>) {
    let mut drafts = Vec::new();
    let mut expressions = Vec::new();
    for &(line_start, line_end) in line_ranges(mask.len(), line_starts).iter() {
        if raw_ranges
            .iter()
            .any(|range| range.start <= line_start && line_start < range.end)
        {
            continue;
        }
        let mut cursor = skip_ascii_space(mask, line_start, line_end);
        if ["app", "layout", "page"]
            .iter()
            .any(|keyword| keyword_at(mask, cursor, keyword))
        {
            let (_, end) = identifier_at(mask, cursor).unwrap_or((cursor, cursor));
            let next = skip_ascii_space(mask, end, line_end);
            if keyword_at(mask, next, "state") {
                cursor = next;
            }
        }
        let (kind, keyword_len) = if keyword_at(mask, cursor, "state") {
            (AxLanguageLocalSymbolKind::State, 5)
        } else if keyword_at(mask, cursor, "data") {
            (AxLanguageLocalSymbolKind::Data, 4)
        } else if keyword_at(mask, cursor, "const") {
            (AxLanguageLocalSymbolKind::Constant, 5)
        } else if keyword_at(mask, cursor, "let") {
            (AxLanguageLocalSymbolKind::Variable, 3)
        } else {
            continue;
        };
        let Some(owner) = innermost_owner(owners, cursor, &[OwnerKind::Page, OwnerKind::Component])
        else {
            continue;
        };
        cursor = skip_ascii_space(mask, cursor + keyword_len, line_end);
        let Some(equals) = find_top_level_equals(mask, cursor, line_end) else {
            continue;
        };
        expressions.push(ByteRange {
            start: equals + 1,
            end: line_end,
        });

        if mask.get(cursor) == Some(&b'{') {
            let Some(close) = matching_delimiter(mask, cursor, b'{', b'}') else {
                continue;
            };
            for field in split_top_level(mask, cursor + 1, close, b',') {
                let field_start = skip_ascii_space(mask, field.start, field.end);
                let Some((source_start, source_end)) = identifier_at(mask, field_start) else {
                    continue;
                };
                let colon = find_byte(mask, source_end, field.end, b':');
                let (name_start, name_end) = colon
                    .and_then(|colon| {
                        identifier_at(mask, skip_ascii_space(mask, colon + 1, field.end))
                    })
                    .unwrap_or((source_start, source_end));
                drafts.push(local_draft(mask, kind, name_start, name_end, owner));
            }
        } else if let Some((name_start, name_end)) = identifier_at(mask, cursor) {
            drafts.push(local_draft(mask, kind, name_start, name_end, owner));
        }
    }
    (drafts, expressions)
}

fn local_draft(
    mask: &[u8],
    kind: AxLanguageLocalSymbolKind,
    name_start: usize,
    name_end: usize,
    owner: &Owner,
) -> LocalDraft {
    LocalDraft {
        name: String::from_utf8_lossy(&mask[name_start..name_end]).into_owned(),
        kind,
        declaration_start: name_start,
        declaration_end: name_end,
        scope_start: name_start,
        scope_end: owner.end,
    }
}

fn collect_raw_ranges(mask: &[u8], line_starts: &[usize]) -> Vec<ByteRange> {
    line_ranges(mask.len(), line_starts)
        .into_iter()
        .filter_map(|(start, end)| {
            let cursor = skip_ascii_space(mask, start, end);
            let raw = keyword_at(mask, cursor, "style")
                || (keyword_at(mask, cursor, "client")
                    && identifier_at(mask, skip_ascii_space(mask, cursor + 6, end))
                        .is_some_and(|(start, end)| matches!(&mask[start..end], b"JS" | b"WASM")));
            if !raw {
                return None;
            }
            let open = find_byte(mask, cursor, end, b'{')?;
            let close = matching_delimiter(mask, open, b'{', b'}')?;
            Some(ByteRange {
                start: open,
                end: close + 1,
            })
        })
        .collect()
}

fn collect_asx_expression_ranges(
    mask: &[u8],
    line_starts: &[usize],
    owners: &[Owner],
    raw_ranges: &[ByteRange],
) -> Vec<ByteRange> {
    let owner_opens = owners
        .iter()
        .filter_map(|owner| owner.open)
        .collect::<Vec<_>>();
    let mut ranges = Vec::new();
    for (index, byte) in mask.iter().enumerate() {
        if *byte != b'{' || owner_opens.contains(&index) {
            continue;
        }
        if raw_ranges.iter().any(|range| range.start == index) {
            continue;
        }
        let line_start = line_starts
            .get(line_index_for_offset(line_starts, index))
            .copied()
            .unwrap_or(0);
        let prefix = String::from_utf8_lossy(&mask[line_start..index]);
        let trimmed = prefix.trim();
        if trimmed.ends_with("return ASX")
            || trimmed == "return"
            || trimmed.ends_with("render ASX")
            || trimmed.starts_with("type ")
            || trimmed.starts_with("export type ")
            || trimmed.starts_with("data ")
            || trimmed.starts_with("const ")
            || trimmed.starts_with("let ")
        {
            continue;
        }
        if let Some(close) = matching_delimiter(mask, index, b'{', b'}') {
            ranges.push(ByteRange {
                start: index + 1,
                end: close,
            });
        }
    }
    ranges
}

fn innermost_owner<'a>(
    owners: &'a [Owner],
    offset: usize,
    kinds: &[OwnerKind],
) -> Option<&'a Owner> {
    owners
        .iter()
        .filter(|owner| kinds.contains(&owner.kind) && owner.start <= offset && offset <= owner.end)
        .min_by_key(|owner| owner.end.saturating_sub(owner.start))
}

fn is_member_or_object_key(mask: &[u8], start: usize, end: usize) -> bool {
    let previous = mask[..start]
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|index| mask[index]);
    if previous == Some(b'.') {
        return true;
    }
    mask[end..]
        .iter()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| *byte == b':')
}

fn identifier_tokens(mask: &[u8]) -> Vec<(usize, usize)> {
    let mut tokens = Vec::new();
    let mut cursor = 0;
    while cursor < mask.len() {
        if is_identifier_start(mask[cursor]) {
            let start = cursor;
            cursor += 1;
            while cursor < mask.len() && is_identifier_continue(mask[cursor]) {
                cursor += 1;
            }
            tokens.push((start, cursor));
        } else {
            cursor += 1;
        }
    }
    tokens
}

fn code_mask(source: &str) -> Vec<u8> {
    let bytes = source.as_bytes();
    let mut mask = bytes.to_vec();
    let mut cursor = 0;
    let mut block_comment = false;
    let mut quote = None;
    let mut escaped = false;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        let next = bytes.get(cursor + 1).copied();
        if block_comment {
            if byte == b'*' && next == Some(b'/') {
                mask[cursor] = b' ';
                mask[cursor + 1] = b' ';
                cursor += 2;
                block_comment = false;
            } else {
                if byte != b'\n' && byte != b'\r' {
                    mask[cursor] = b' ';
                }
                cursor += 1;
            }
            continue;
        }
        if let Some(delimiter) = quote {
            if byte != b'\n' && byte != b'\r' {
                mask[cursor] = b' ';
            }
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == delimiter {
                quote = None;
            }
            cursor += 1;
            continue;
        }
        if byte == b'/' && next == Some(b'/') {
            while cursor < bytes.len() && !matches!(bytes[cursor], b'\n' | b'\r') {
                mask[cursor] = b' ';
                cursor += 1;
            }
            continue;
        }
        if byte == b'/' && next == Some(b'*') {
            mask[cursor] = b' ';
            mask[cursor + 1] = b' ';
            cursor += 2;
            block_comment = true;
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            mask[cursor] = b' ';
            quote = Some(byte);
        }
        cursor += 1;
    }
    mask
}

fn matching_delimiter(mask: &[u8], open: usize, left: u8, right: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (index, byte) in mask.iter().enumerate().skip(open) {
        if *byte == left {
            depth += 1;
        } else if *byte == right {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn split_top_level(mask: &[u8], start: usize, end: usize, separator: u8) -> Vec<ByteRange> {
    let mut ranges = Vec::new();
    let mut segment = start;
    let (mut paren, mut bracket, mut brace, mut angle) = (0usize, 0usize, 0usize, 0usize);
    for (index, byte) in mask.iter().enumerate().take(end).skip(start) {
        match *byte {
            b'(' => paren += 1,
            b')' => paren = paren.saturating_sub(1),
            b'[' => bracket += 1,
            b']' => bracket = bracket.saturating_sub(1),
            b'{' => brace += 1,
            b'}' => brace = brace.saturating_sub(1),
            b'<' => angle += 1,
            b'>' => angle = angle.saturating_sub(1),
            value
                if value == separator && paren == 0 && bracket == 0 && brace == 0 && angle == 0 =>
            {
                ranges.push(ByteRange {
                    start: segment,
                    end: index,
                });
                segment = index + 1;
            }
            _ => {}
        }
    }
    ranges.push(ByteRange {
        start: segment,
        end,
    });
    ranges
}

fn find_top_level_equals(mask: &[u8], start: usize, end: usize) -> Option<usize> {
    let (mut paren, mut bracket, mut brace, mut angle) = (0usize, 0usize, 0usize, 0usize);
    for (index, byte) in mask.iter().enumerate().take(end).skip(start) {
        match *byte {
            b'(' => paren += 1,
            b')' => paren = paren.saturating_sub(1),
            b'[' => bracket += 1,
            b']' => bracket = bracket.saturating_sub(1),
            b'{' => brace += 1,
            b'}' => brace = brace.saturating_sub(1),
            b'<' => angle += 1,
            b'>' => angle = angle.saturating_sub(1),
            b'=' if paren == 0 && bracket == 0 && brace == 0 && angle == 0 => return Some(index),
            _ => {}
        }
    }
    None
}

fn identifier_at(mask: &[u8], start: usize) -> Option<(usize, usize)> {
    if !mask.get(start).copied().is_some_and(is_identifier_start) {
        return None;
    }
    let mut end = start + 1;
    while mask.get(end).copied().is_some_and(is_identifier_continue) {
        end += 1;
    }
    Some((start, end))
}

fn keyword_at(mask: &[u8], start: usize, keyword: &str) -> bool {
    let bytes = keyword.as_bytes();
    mask.get(start..start + bytes.len()) == Some(bytes)
        && mask
            .get(start.wrapping_sub(1))
            .is_none_or(|byte| !is_identifier_continue(*byte))
        && mask
            .get(start + bytes.len())
            .is_none_or(|byte| !is_identifier_continue(*byte))
}

fn skip_ascii_space(mask: &[u8], mut cursor: usize, end: usize) -> usize {
    while cursor < end && mask[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor
}

fn find_byte(mask: &[u8], start: usize, end: usize, target: u8) -> Option<usize> {
    mask.get(start..end)?
        .iter()
        .position(|byte| *byte == target)
        .map(|offset| start + offset)
}

fn source_line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        source
            .bytes()
            .enumerate()
            .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
    );
    starts
}

fn line_ranges(length: usize, starts: &[usize]) -> Vec<(usize, usize)> {
    starts
        .iter()
        .enumerate()
        .map(|(index, start)| (*start, starts.get(index + 1).copied().unwrap_or(length)))
        .collect()
}

fn line_index_for_offset(starts: &[usize], offset: usize) -> usize {
    starts
        .partition_point(|start| *start <= offset)
        .saturating_sub(1)
}

fn occurrence(
    source: &str,
    line_starts: &[usize],
    start: usize,
    end: usize,
) -> AxLanguageIdentifierOccurrence {
    let line_index = line_index_for_offset(line_starts, start);
    let line_start = line_starts[line_index];
    AxLanguageIdentifierOccurrence {
        name: source[start..end].to_string(),
        line: line_index + 1,
        column: source[line_start..start].encode_utf16().count() + 1,
        end_column: source[line_start..end].encode_utf16().count() + 1,
    }
}

fn line_end_for_offset(length: usize, starts: &[usize], offset: usize) -> usize {
    starts
        .get(line_index_for_offset(starts, offset) + 1)
        .copied()
        .unwrap_or(length)
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_page_locals_without_touching_asx_text_properties_or_strings() {
        let source = r#"page Posts(slug: String) -> ASX {
  data posts = loadPosts(slug)
  const title = posts[0].title

  return {
    <Copy title={title}>title {slug} {posts.length}</Copy>
    <Copy>{"title posts slug"}</Copy>
  }
}
"#;

        let symbols = ax_source_local_symbols(source);
        let slug = symbols.iter().find(|symbol| symbol.name == "slug").unwrap();
        let posts = symbols
            .iter()
            .find(|symbol| symbol.name == "posts")
            .unwrap();
        let title = symbols
            .iter()
            .find(|symbol| symbol.name == "title")
            .unwrap();

        assert_eq!(slug.kind, AxLanguageLocalSymbolKind::Parameter);
        assert_eq!(slug.occurrences.len(), 3);
        assert_eq!(posts.occurrences.len(), 3);
        assert_eq!(title.occurrences.len(), 2);
    }

    #[test]
    fn component_and_function_params_shadow_page_locals() {
        let source = r#"page Home(title: String) {
  const cardTitle = title
  fn label(title: String) = title
  component Card(title: String) {
    state open = false
    render ASX { <Copy>{title} {open}</Copy> }
  }
  return ASX { <Card title={title} /> }
}
"#;

        let symbols = ax_source_local_symbols(source);
        let titles = symbols
            .iter()
            .filter(|symbol| symbol.name == "title")
            .collect::<Vec<_>>();
        assert_eq!(titles.len(), 3);
        assert_eq!(titles[0].occurrences.len(), 3);
        assert_eq!(titles[1].occurrences.len(), 2);
        assert_eq!(titles[2].occurrences.len(), 2);
        assert!(symbols
            .iter()
            .any(|symbol| symbol.name == "open" && symbol.occurrences.len() == 2));
    }

    #[test]
    fn indexes_multiline_signatures_and_next_line_component_braces() {
        let source = r#"page Home(fallback: String) {
  fn label(
    title: String = fallback,
  ) = title
  component Card(
    title: String = fallback,
  )
  {
    render ASX { <Copy>{title}</Copy> }
  }
  return ASX { <Card title={fallback} /> }
}
"#;

        let symbols = ax_source_local_symbols(source);
        let fallback = symbols
            .iter()
            .find(|symbol| symbol.name == "fallback")
            .unwrap();
        let titles = symbols
            .iter()
            .filter(|symbol| symbol.name == "title")
            .collect::<Vec<_>>();

        assert_eq!(fallback.occurrences.len(), 4);
        assert_eq!(titles.len(), 2);
        assert!(titles.iter().all(|symbol| symbol.occurrences.len() == 2));
    }

    #[test]
    fn skips_client_and_style_block_identifiers() {
        let source = r#"page Home() {
  component Counter(label: String) {
    state count = 0
    client JS {
      const count = "label"
    }
    style {
      .label { color: red; }
    }
    render ASX { <button>{label} {count}</button> }
  }
  return ASX { <Counter label="Count" /> }
}
"#;
        let symbols = ax_source_local_symbols(source);
        assert_eq!(
            symbols
                .iter()
                .find(|symbol| symbol.name == "label")
                .unwrap()
                .occurrences
                .len(),
            2
        );
        assert_eq!(
            symbols
                .iter()
                .find(|symbol| symbol.name == "count")
                .unwrap()
                .occurrences
                .len(),
            2
        );
    }

    #[test]
    fn indexes_backend_function_params_and_local_bindings() {
        let source = r#"export fn normalize(status: String, fallback: String = status) -> String {
  const selected = status ?? fallback
  let result = selected
  return result
}
"#;

        let symbols = ax_source_local_symbols(source);
        let symbol = |name: &str| symbols.iter().find(|symbol| symbol.name == name).unwrap();

        assert_eq!(symbol("status").kind, AxLanguageLocalSymbolKind::Parameter);
        assert_eq!(symbol("status").occurrences.len(), 3);
        assert_eq!(symbol("fallback").occurrences.len(), 2);
        assert_eq!(symbol("selected").occurrences.len(), 2);
        assert_eq!(symbol("result").occurrences.len(), 2);
    }

    #[test]
    fn keeps_backend_local_shadowing_inside_each_callable() {
        let source = r#"fn first(value: String) -> String {
  const result = value
  return result
}

fn second(value: String) -> String {
  const result = value
  return result
}
"#;

        let symbols = ax_source_local_symbols(source);
        let values = symbols
            .iter()
            .filter(|symbol| symbol.name == "value")
            .collect::<Vec<_>>();
        let results = symbols
            .iter()
            .filter(|symbol| symbol.name == "result")
            .collect::<Vec<_>>();

        assert_eq!(values.len(), 2);
        assert_eq!(results.len(), 2);
        assert!(values.iter().all(|symbol| symbol.occurrences.len() == 2));
        assert!(results.iter().all(|symbol| symbol.occurrences.len() == 2));
    }

    #[test]
    fn ignores_object_defaults_when_finding_a_backend_callable_body() {
        let source = r#"fn describe(options: Map<String, String> = { label: "default" }) -> String {
  const label = options.label
  return label
}
"#;

        let symbols = ax_source_local_symbols(source);
        let options = symbols
            .iter()
            .find(|symbol| symbol.name == "options")
            .unwrap();
        let label = symbols
            .iter()
            .find(|symbol| symbol.name == "label")
            .unwrap();

        assert_eq!(options.occurrences.len(), 2);
        assert_eq!(label.occurrences.len(), 2);
    }

    #[test]
    fn backend_locals_do_not_capture_database_fields_members_or_strings() {
        let source = r#"action updateStatus(status: String) {
  data current = db.posts.where({ status: status }).first()
  update posts
    status = status
    where status = current.id
  return current
  // status current
  header "status" = "current"
}
"#;

        let symbols = ax_source_local_symbols(source);
        let status = symbols
            .iter()
            .find(|symbol| symbol.name == "status")
            .unwrap();
        let current = symbols
            .iter()
            .find(|symbol| symbol.name == "current")
            .unwrap();

        assert_eq!(status.occurrences.len(), 3);
        assert_eq!(current.occurrences.len(), 3);
    }

    #[test]
    fn indexes_scope_state_and_keeps_sibling_scopes_isolated() {
        let source = r#"scope App <RenderLayout> {
  state theme: String = "silver"
  state selected: String = theme
  render RenderLayout(theme, selected)
}

scope Admin <RenderLayout> {
  state theme: String = "bronze"
  render RenderLayout(theme)
}
"#;

        let symbols = ax_source_local_symbols(source);
        let themes = symbols
            .iter()
            .filter(|symbol| symbol.name == "theme")
            .collect::<Vec<_>>();
        let selected = symbols
            .iter()
            .find(|symbol| symbol.name == "selected")
            .expect("selected scope state should be indexed");

        assert_eq!(themes.len(), 2);
        assert_eq!(themes[0].kind, AxLanguageLocalSymbolKind::State);
        assert_eq!(themes[0].occurrences.len(), 3);
        assert_eq!(themes[1].kind, AxLanguageLocalSymbolKind::State);
        assert_eq!(themes[1].occurrences.len(), 2);
        assert_eq!(selected.kind, AxLanguageLocalSymbolKind::State);
        assert_eq!(selected.occurrences.len(), 2);
        assert!(themes[0].scope_end <= themes[1].scope_start);
    }
}
