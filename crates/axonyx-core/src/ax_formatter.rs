#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxFormatOptions {
    pub indent_width: usize,
}

impl Default for AxFormatOptions {
    fn default() -> Self {
        Self { indent_width: 2 }
    }
}

/// Formats Axonyx source without rewriting expressions, strings, or embedded code.
///
/// V0 deliberately owns whitespace only. Syntax-aware rewrites belong to later
/// formatter versions once every language surface has a lossless syntax tree.
pub fn format_ax_source(source: &str) -> String {
    format_ax_source_with_options(source, AxFormatOptions::default())
}

pub fn format_ax_source_with_options(source: &str, options: AxFormatOptions) -> String {
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    // Older declaration bodies use indentation as syntax, not just presentation.
    // Leave those files untouched until they have a lossless legacy formatter.
    if normalized.lines().any(is_indentation_sensitive_line) {
        return normalized;
    }
    let indent_unit = " ".repeat(options.indent_width);
    let mut formatted = Vec::new();
    let mut depth = 0usize;
    let mut previous_blank = false;
    let mut scanner = StructuralScanner::default();
    let mut raw_block_depth: Option<usize> = None;

    for raw_line in normalized.lines() {
        if let Some(current_raw_depth) = raw_block_depth {
            let line = raw_line.trim();
            let balance = scanner.scan_line(line);
            let next_raw_depth = current_raw_depth
                .saturating_add(balance.opens)
                .saturating_sub(balance.closes);
            let next_depth = depth
                .saturating_add(balance.opens)
                .saturating_sub(balance.closes);

            if next_raw_depth == 0 {
                let alignment = leading_alignment(line, scanner.pending_tag);
                let line_depth = depth.saturating_sub(alignment);
                formatted.push(format!("{}{}", indent_unit.repeat(line_depth), line));
                previous_blank = false;
                raw_block_depth = None;
            } else {
                formatted.push(raw_line.to_string());
                previous_blank = line.is_empty();
                raw_block_depth = Some(next_raw_depth);
            }
            depth = next_depth;
            continue;
        }

        let line = raw_line.trim();
        if line.is_empty() {
            if !previous_blank && !formatted.is_empty() {
                formatted.push(String::new());
                previous_blank = true;
            }
            continue;
        }

        let alignment = leading_alignment(line, scanner.pending_tag);
        let line_depth = depth.saturating_sub(alignment);
        let expanded = if !scanner.pending_tag && scanner.quote.is_none() && !scanner.block_comment
        {
            expand_compact_markup(line, &indent_unit)
        } else {
            None
        };
        for part in expanded.as_deref().unwrap_or(&[line.to_string()]) {
            let prefix = indent_unit.repeat(line_depth);
            let wrapped =
                if scanner.quote.is_none() && !scanner.pending_tag && !scanner.block_comment {
                    wrap_long_open_tag(part, &indent_unit, prefix.len())
                } else {
                    None
                };
            for part in wrapped.as_deref().unwrap_or(&[part.to_string()]) {
                formatted.push(format!("{prefix}{part}"));
            }
        }
        previous_blank = false;

        let balance = scanner.scan_line(line);
        if is_raw_block_start(line) && balance.opens > balance.closes {
            raw_block_depth = Some(balance.opens - balance.closes);
        }
        depth = depth
            .saturating_add(balance.opens)
            .saturating_sub(balance.closes);
    }

    while formatted.last().is_some_and(String::is_empty) {
        formatted.pop();
    }

    if formatted.is_empty() {
        String::new()
    } else {
        format!("{}\n", formatted.join("\n"))
    }
}

fn is_raw_block_start(line: &str) -> bool {
    ["client JS", "client WASM", "style"].iter().any(|keyword| {
        line.strip_prefix(keyword)
            .is_some_and(|rest| rest.trim_start().starts_with('{'))
    })
}

fn is_indentation_sensitive_line(line: &str) -> bool {
    let line = crate::ax_backend_parser::backend_code_before_comment(line);
    let text = line.trim_start();
    if ["where", "order", "limit", "offset"].iter().any(|keyword| {
        text.strip_prefix(keyword)
            .is_some_and(|rest| rest.starts_with(char::is_whitespace))
    }) {
        return true;
    }
    if line.len() != line.trim_start().len() {
        return false;
    }
    let header = line
        .trim_end()
        .strip_prefix("export ")
        .unwrap_or(line.trim_end());
    if header.contains('{') || header.ends_with('(') {
        return false;
    }
    [
        "backend",
        "page",
        "loader",
        "route",
        "action",
        "job",
        "query",
        "fn",
        "component",
        "scope",
    ]
    .iter()
    .any(|keyword| {
        header == *keyword
            || header
                .strip_prefix(keyword)
                .is_some_and(|rest| rest.starts_with(char::is_whitespace))
    })
}

struct InlineMarkup<'a> {
    source: &'a str,
    open: &'a str,
    close: &'a str,
    children: Vec<InlineMarkup<'a>>,
    mixed: bool,
}

fn wrap_long_open_tag(line: &str, indent: &str, prefix_width: usize) -> Option<Vec<String>> {
    let leading = line.len() - line.trim_start().len();
    let source = &line[leading..];
    if !source.starts_with('<') || source.starts_with("</") {
        return None;
    }
    let end = inline_tag_end(source)?;
    if prefix_width + leading + source[..end].chars().count() <= 100 {
        return None;
    }
    let self_closing = source[..end].ends_with("/>");
    let suffix = if self_closing { "/>" } else { ">" };
    let attributes_end = end - suffix.len();
    let name_end = source.find(char::is_whitespace)?;
    if name_end >= attributes_end {
        return None;
    }
    let mut attributes = Vec::new();
    let mut cursor = name_end;
    while cursor < attributes_end {
        let ch = source[cursor..].chars().next()?;
        if ch.is_whitespace() {
            cursor += ch.len_utf8();
            continue;
        }
        let start = cursor;
        let mut quote = None;
        let mut escaped = false;
        while cursor < attributes_end {
            let ch = source[cursor..].chars().next()?;
            if let Some(active) = quote {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == active {
                    quote = None;
                }
            } else if matches!(ch, '\'' | '"') {
                quote = Some(ch);
            } else if ch == '{' {
                cursor += inline_expression_end(&source[cursor..])?;
                continue;
            } else if ch.is_whitespace() {
                break;
            }
            cursor += ch.len_utf8();
        }
        let attribute = &source[start..cursor];
        // Leave unusual/spaced assignment syntax alone rather than splitting it.
        if !attribute.starts_with(|ch: char| ch.is_alphabetic() || ch == '_')
            || attribute.ends_with('=')
        {
            return None;
        }
        attributes.push(attribute);
    }
    if attributes.len() < 2 {
        return None;
    }
    let padding = " ".repeat(leading);
    let mut lines = vec![format!("{padding}{}", &source[..name_end])];
    lines.extend(
        attributes
            .iter()
            .map(|attribute| format!("{padding}{indent}{attribute}")),
    );
    lines.push(format!("{padding}{suffix}{}", &source[end..]));
    // Validate the opening tag independently, including incomplete outer containers.
    let original_probe = format!("{} />", source[..attributes_end].trim_end());
    let formatted_probe = format!("{}\n{}\n/>", &source[..name_end], attributes.join("\n"));
    let wrap = |tag: &str| format!("page FormatProbe() {{\nreturn ASX {{\n{tag}\n}}\n}}");
    let before = crate::ax_parser_v2::parse_ax_v2(&wrap(&original_probe)).ok()?;
    let after = crate::ax_parser_v2::parse_ax_v2(&wrap(&formatted_probe)).ok()?;
    (before == after).then_some(lines)
}

// Only expand complete element-only containers. Leaf text and mixed prose keep
// their exact source bytes; the parser equality check is the final safety gate.
fn expand_compact_markup(line: &str, indent: &str) -> Option<Vec<String>> {
    let (node, consumed) = inline_markup(line, 0)?;
    if consumed != line.len() || node.mixed || node.children.is_empty() {
        return None;
    }
    fn emit(node: &InlineMarkup<'_>, prefix: &str, indent: &str, lines: &mut Vec<String>) {
        if node.mixed || node.children.is_empty() {
            lines.push(format!("{prefix}{}", node.source));
        } else {
            lines.push(format!("{prefix}{}", node.open));
            for child in &node.children {
                emit(child, &format!("{prefix}{indent}"), indent, lines);
            }
            lines.push(format!("{prefix}{}", node.close));
        }
    }
    let mut lines = Vec::new();
    emit(&node, "", indent, &mut lines);
    let wrap = |markup: &str| format!("page FormatProbe() {{\nreturn ASX {{\n{markup}\n}}\n}}");
    let before = crate::ax_parser_v2::parse_ax_v2(&wrap(line)).ok()?;
    let after = crate::ax_parser_v2::parse_ax_v2(&wrap(&lines.join("\n"))).ok()?;
    (before == after).then_some(lines)
}

fn inline_markup(source: &str, depth: usize) -> Option<(InlineMarkup<'_>, usize)> {
    if depth > 128 || !source.starts_with('<') || source.starts_with("</") {
        return None;
    }
    let open_end = inline_tag_end(source)?;
    let open = &source[..open_end];
    let name = open[1..].split([' ', '\t', '/', '>']).next()?;
    if !name.is_empty()
        && !name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return None;
    }
    if open.ends_with("/>") {
        return Some((
            InlineMarkup {
                source: open,
                open,
                close: "",
                children: Vec::new(),
                mixed: false,
            },
            open_end,
        ));
    }
    let closing = format!("</{name}>");
    let mut cursor = open_end;
    let mut children = Vec::new();
    let mut mixed = matches!(name, "pre" | "code" | "script" | "style" | "textarea");
    loop {
        let remaining = source.get(cursor..)?;
        if remaining.starts_with(&closing) {
            let end = cursor + closing.len();
            return Some((
                InlineMarkup {
                    source: &source[..end],
                    open,
                    close: &source[cursor..end],
                    children,
                    mixed,
                },
                end,
            ));
        }
        if remaining.starts_with('<') {
            let (child, length) = inline_markup(remaining, depth + 1)?;
            children.push(child);
            cursor += length;
        } else {
            // Expressions may contain strings that look like markup.
            let length = if remaining.starts_with('{') {
                inline_expression_end(remaining)?
            } else {
                remaining.chars().next()?.len_utf8()
            };
            mixed = true;
            cursor += length;
        }
    }
}

fn inline_expression_end(source: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in source.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active {
                quote = None;
            }
        } else if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        } else if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index + 1);
            }
        }
    }
    None
}

fn inline_tag_end(source: &str) -> Option<usize> {
    let mut cursor = 1;
    let mut quote = None;
    let mut escaped = false;
    while let Some(ch) = source.get(cursor..)?.chars().next() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active {
                quote = None;
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch == '{' {
            cursor += inline_expression_end(&source[cursor..])?;
            continue;
        } else if ch == '>' {
            return Some(cursor + 1);
        }
        cursor += ch.len_utf8();
    }
    None
}

fn leading_alignment(line: &str, pending_tag: bool) -> usize {
    let mut rest = line.trim_start();
    let mut closes = 0;

    loop {
        if rest.starts_with("</") {
            closes += 1;
            rest = rest
                .find('>')
                .map(|index| &rest[index + 1..])
                .unwrap_or_default()
                .trim_start();
            continue;
        }
        if let Some(first) = rest
            .chars()
            .next()
            .filter(|ch| matches!(ch, '}' | ']' | ')'))
        {
            closes += 1;
            rest = rest[first.len_utf8()..].trim_start();
            continue;
        }
        break;
    }

    if closes == 0 && pending_tag && (rest.starts_with('>') || rest.starts_with("/>")) {
        1
    } else {
        closes
    }
}

#[derive(Debug, Default)]
struct StructuralScanner {
    pending_tag: bool,
    block_comment: bool,
    quote: Option<char>,
    escaped: bool,
}

#[derive(Debug, Default)]
struct StructuralBalance {
    opens: usize,
    closes: usize,
}

impl StructuralScanner {
    fn scan_line(&mut self, line: &str) -> StructuralBalance {
        let chars = line.char_indices().collect::<Vec<_>>();
        let mut balance = StructuralBalance::default();
        let mut index = 0;

        while index < chars.len() {
            let (byte_index, ch) = chars[index];
            let next = chars.get(index + 1).map(|(_, ch)| *ch);

            if self.block_comment {
                if ch == '*' && next == Some('/') {
                    self.block_comment = false;
                    index += 2;
                } else {
                    index += 1;
                }
                continue;
            }

            if let Some(quote) = self.quote {
                if self.escaped {
                    self.escaped = false;
                } else if ch == '\\' {
                    self.escaped = true;
                } else if ch == quote {
                    self.quote = None;
                }
                index += 1;
                continue;
            }

            if ch == '/' && next == Some('/') {
                break;
            }
            if ch == '/' && next == Some('*') {
                self.block_comment = true;
                index += 2;
                continue;
            }
            if matches!(ch, '"' | '\'' | '`') {
                self.quote = Some(ch);
                index += 1;
                continue;
            }

            if self.pending_tag {
                if ch == '{' {
                    if let Some(length) = inline_expression_end(&line[byte_index..]) {
                        while index < chars.len() && chars[index].0 < byte_index + length {
                            index += 1;
                        }
                        continue;
                    }
                }
                if ch == '/' && next == Some('>') {
                    balance.closes += 1;
                    self.pending_tag = false;
                    index += 2;
                    continue;
                }
                if ch == '>' {
                    self.pending_tag = false;
                }
                index += 1;
                continue;
            }

            match ch {
                '{' | '[' | '(' => balance.opens += 1,
                '}' | ']' | ')' => balance.closes += 1,
                '<' => {
                    let tail = &line[byte_index..];
                    // A complete inline subtree has zero structural balance.
                    // Its prose may contain quotes, URLs or tags after text;
                    // those are markup content, not code delimiters.
                    if tag_can_start_at(line, byte_index) {
                        if let Some((_, length)) = inline_markup(tail, 0) {
                            while index < chars.len() && chars[index].0 < byte_index + length {
                                index += 1;
                            }
                            continue;
                        }
                    }
                    if tail.starts_with("</") {
                        balance.closes += 1;
                    } else if tail.starts_with("<>") {
                        balance.opens += 1;
                    } else if next.is_some_and(|next| next.is_ascii_alphabetic())
                        && tag_can_start_at(line, byte_index)
                    {
                        match tag_end_kind(tail) {
                            TagEnd::SelfClosing => {}
                            TagEnd::Open => balance.opens += 1,
                            TagEnd::Pending => {
                                balance.opens += 1;
                                self.pending_tag = true;
                            }
                        }
                    }
                }
                _ => {}
            }

            index += 1;
        }

        balance
    }
}

fn tag_can_start_at(line: &str, byte_index: usize) -> bool {
    line[..byte_index]
        .chars()
        .next_back()
        .is_none_or(|previous| {
            previous.is_whitespace() || matches!(previous, '{' | '(' | '[' | '>' | ',' | ':')
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagEnd {
    SelfClosing,
    Open,
    Pending,
}

fn tag_end_kind(source: &str) -> TagEnd {
    match inline_tag_end(source) {
        Some(end) if source[..end].ends_with("/>") => TagEnd::SelfClosing,
        Some(_) => TagEnd::Open,
        None => TagEnd::Pending,
    }
}

pub mod prelude {
    pub use super::{format_ax_source, format_ax_source_with_options, AxFormatOptions};
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ax_backend_parser::parse_backend_ax;
    use crate::ax_parser_v2::parse_ax_v2;

    #[test]
    fn formats_function_shaped_asx_and_is_idempotent() {
        let source = "page Home() {\nreturn ASX {\n<Container max=\"xl\">\n<Copy>{title}</Copy>\n</Container>\n}\n}\n";
        let expected = "page Home() {\n  return ASX {\n    <Container max=\"xl\">\n      <Copy>{title}</Copy>\n    </Container>\n  }\n}\n";

        let formatted = format_ax_source(source);
        assert_eq!(formatted, expected);
        assert_eq!(format_ax_source(&formatted), expected);
    }

    #[test]
    fn leaves_indentation_based_backend_and_legacy_headers_untouched() {
        for source in [
            "backend\n  env DATABASE_URL: Secret<String>\n  data themes: List<String> = [\"silver\"]\n",
            "route GET \"/posts\"\n  data posts = db.posts.all()\n  return posts\n",
            "job PublishDigest\n  data posts = db.posts.all()\n  send Digest with posts\n",
            "page Home\n  Card title: \"Welcome\"\n    Copy -> \"Hello\"\n",
            "import { posts } from \"./queries.ax\"\n\nexport loader Posts\n  return posts\n",
            "query loadDocs() -> Doc[] {\n  data docs = Content.Collection(\"docs\")\n    where slug = input.slug\n    order title asc\n    limit 1\n    offset 0\n  return docs\n}\n",
        ] {
            assert_eq!(format_ax_source(source), source);
        }
    }

    #[test]
    fn expands_nested_table_rows_without_changing_the_ast() {
        let source = "page Posts() {\nreturn ASX {\n<TableRow><TableCell><a href=\"/post\">Post</a></TableCell><TableCell>{post.status}</TableCell></TableRow>\n}\n}\n";
        let expected = "page Posts() {\n  return ASX {\n    <TableRow>\n      <TableCell>\n        <a href=\"/post\">Post</a>\n      </TableCell>\n      <TableCell>{post.status}</TableCell>\n    </TableRow>\n  }\n}\n";
        let formatted = format_ax_source(source);
        assert_eq!(formatted, expected);
        assert_eq!(format_ax_source(&formatted), formatted);
        assert_eq!(
            parse_ax_v2(source).unwrap(),
            parse_ax_v2(&formatted).unwrap()
        );
    }

    #[test]
    fn wraps_long_attributes_and_preserves_expressions_and_ast() {
        let source = "page Home() {\nreturn ASX {\n<Card description=\"A long description for a readable Foundry component with meaningful attributes\" when={count > 0}><Copy>Ready</Copy></Card>\n}\n}\n";
        let formatted = format_ax_source(source);
        assert!(formatted.contains("    <Card\n      description="));
        assert!(formatted.contains("      when={count > 0}\n    >\n"));
        assert_eq!(format_ax_source(&formatted), formatted);
        assert_eq!(
            parse_ax_v2(source).unwrap(),
            parse_ax_v2(&formatted).unwrap()
        );
    }

    #[test]
    fn wraps_self_closing_attributes_but_keeps_short_tags_compact() {
        let long = "<Input label=\"A sufficiently long accessible input label for this formatting regression test\" value={\"a > b\"} />";
        let lines = wrap_long_open_tag(long, "  ", 4).unwrap();
        assert_eq!(lines.first().unwrap(), "<Input");
        assert_eq!(lines.last().unwrap(), "/>");
        assert_eq!(lines[2], "  value={\"a > b\"}");
        assert!(wrap_long_open_tag("<Input label=\"Name\" value={name} />", "  ", 4).is_none());
        let source = format!("page Home() {{\nreturn ASX {{\n{long}\n}}\n}}\n");
        let formatted = format_ax_source(&source);
        assert_eq!(format_ax_source(&formatted), formatted);
        assert_eq!(
            parse_ax_v2(&source).unwrap(),
            parse_ax_v2(&formatted).unwrap()
        );
    }

    #[test]
    fn self_closing_comparison_attribute_does_not_change_following_indentation() {
        let source = "page Home() {\nreturn ASX {\n<Input label=\"A long accessible label that forces automatic wrapping in the shared Axonyx formatter\" value={count > 0} />\n}\n}\n";
        let formatted = format_ax_source(source);
        assert_eq!(format_ax_source(&formatted), formatted);
        assert_eq!(
            parse_ax_v2(source).unwrap(),
            parse_ax_v2(&formatted).unwrap()
        );
        assert!(formatted.ends_with("    />\n  }\n}\n"));
    }

    #[test]
    fn mixed_inline_subtrees_do_not_shift_following_siblings() {
        let source = "page Layout() {\nreturn ASX {\n<Container>\n<header><a href=\"/\"><img src=\"/logo.svg\" />Legura</a><label>Appearance<select><option>Light</option></select></label></header>\n<main><Slot /></main>\n</Container>\n}\n}\n";
        let formatted = format_ax_source(source);
        assert!(formatted
            .contains("      <main>\n        <Slot />\n      </main>\n    </Container>\n  }\n}\n"));
        assert_eq!(format_ax_source(&formatted), formatted);
        assert_eq!(
            parse_ax_v2(source).unwrap(),
            parse_ax_v2(&formatted).unwrap()
        );
    }

    #[test]
    fn balanced_inline_prose_does_not_open_a_code_quote_or_comment() {
        for text in [
            "Don't change this",
            "See https://example.test/path",
            "Literal /* text */",
        ] {
            let source = format!("page Home() {{\nreturn ASX {{\n<Copy>{text}</Copy>\n<Card><Slot /></Card>\n}}\n}}\n");
            let formatted = format_ax_source(&source);
            assert!(formatted.contains("    <Card>\n      <Slot />\n    </Card>\n  }\n}\n"));
            assert_eq!(format_ax_source(&formatted), formatted);
            assert_eq!(
                parse_ax_v2(&source).unwrap(),
                parse_ax_v2(&formatted).unwrap()
            );
        }
    }

    #[test]
    fn attribute_wrapping_preserves_inline_prose_and_raw_code_strings() {
        let tag = "<Copy title=\"This is a long title that should be wrapped while preserving the inline prose exactly as it was\" tone=\"lead\">Hello <strong>world</strong>!</Copy>";
        let source = format!("page Home() {{\nreturn ASX {{\n{tag}\n}}\n}}\n");
        let formatted = format_ax_source(&source);
        assert!(formatted.contains(">Hello <strong>world</strong>!</Copy>"));
        assert_eq!(
            parse_ax_v2(&source).unwrap(),
            parse_ax_v2(&formatted).unwrap()
        );
        assert_eq!(format_ax_source(&formatted), formatted);
        assert!(wrap_long_open_tag(&format!("const sample = {tag:?}"), "  ", 0).is_none());
    }

    #[test]
    fn compact_markup_preserves_mixed_text_and_sensitive_content() {
        for markup in [
            "<Copy>Hello <strong>world</strong>!</Copy>",
            "<pre><span> keep  spaces </span></pre>",
            "<div>{\"<Card><Copy>fake</Copy></Card>\"}<Slot /></div>",
            "<div><span>A</span> <span>B</span></div>",
        ] {
            assert!(expand_compact_markup(markup, "  ").is_none(), "{markup}");
        }
    }

    #[test]
    fn expands_fragments_and_ignores_angle_brackets_in_attributes() {
        let markup = "<><Card title=\"a > b\"><Copy>{value}</Copy></Card><Slot /></>";
        let lines = expand_compact_markup(markup, "  ").unwrap();
        assert_eq!(
            lines,
            [
                "<>",
                "  <Card title=\"a > b\">",
                "    <Copy>{value}</Copy>",
                "  </Card>",
                "  <Slot />",
                "</>"
            ]
        );
        let expression = "<Card when={count > 0}><Copy>Ready</Copy></Card>";
        assert!(expand_compact_markup(expression, "  ").is_some());
    }

    #[test]
    fn compact_markup_leaves_partial_or_invalid_lines_unchanged() {
        for markup in [
            "<Card><Copy>Hi</Copy>",
            "<Card><Copy /></Wrong>",
            "const x = \"<Card><Copy /></Card>\"",
            "<Card><Copy /></Card> // comment",
        ] {
            assert!(expand_compact_markup(markup, "  ").is_none());
        }
    }

    #[test]
    fn formats_multiline_elements_and_self_closing_tags() {
        let source = "page Home() {\nreturn ASX {\n<ComponentPage\ntitle=\"Button\"\ndescription=\"A {safe} value\"\n>\n<Meta\nname=\"description\"\ncontent=\"Foundry\"\n/>\n<Copy>Ready</Copy>\n</ComponentPage>\n}\n}\n";
        let expected = "page Home() {\n  return ASX {\n    <ComponentPage\n      title=\"Button\"\n      description=\"A {safe} value\"\n    >\n      <Meta\n        name=\"description\"\n        content=\"Foundry\"\n      />\n      <Copy>Ready</Copy>\n    </ComponentPage>\n  }\n}\n";

        assert_eq!(format_ax_source(source), expected);
    }

    #[test]
    fn formats_multiline_component_signatures_and_remains_parseable() {
        let source = "page Home() {\ncomponent Button(\nlabel: String,\nvariant: \"primary\" | \"ghost\" = \"primary\",\ndisabled: Bool = false,\n) {\nrender ASX { <button>{label}</button> }\n}\nreturn ASX { <Button label=\"Ship\" /> }\n}\n";
        let expected = "page Home() {\n  component Button(\n    label: String,\n    variant: \"primary\" | \"ghost\" = \"primary\",\n    disabled: Bool = false,\n  ) {\n    render ASX { <button>{label}</button> }\n  }\n  return ASX { <Button label=\"Ship\" /> }\n}\n";

        let formatted = format_ax_source(source);
        assert_eq!(formatted, expected);
        assert_eq!(format_ax_source(&formatted), expected);
        parse_ax_v2(&formatted).expect("formatted multiline signature should parse");
    }

    #[test]
    fn formats_component_return_asx_and_remains_parseable() {
        let source = "page Home() {\ncomponent Greeting(title = \"Hello\") {\nreturn ASX {\n<div>{title}<Slot /></div>\n}\n}\nreturn ASX { <Greeting /> }\n}\n";
        let expected = "page Home() {\n  component Greeting(title = \"Hello\") {\n    return ASX {\n      <div>{title}<Slot /></div>\n    }\n  }\n  return ASX { <Greeting /> }\n}\n";

        let formatted = format_ax_source(source);
        assert_eq!(formatted, expected);
        assert_eq!(format_ax_source(&formatted), expected);
        parse_ax_v2(&formatted).expect("formatted component return should parse");
    }

    #[test]
    fn keeps_else_blocks_at_the_parent_depth() {
        let source =
            "fn label(value: Bool) {\nif value {\nreturn \"on\"\n} else {\nreturn \"off\"\n}\n}\n";
        let expected = "fn label(value: Bool) {\n  if value {\n    return \"on\"\n  } else {\n    return \"off\"\n  }\n}\n";

        assert_eq!(format_ax_source(source), expected);
    }

    #[test]
    fn normalizes_line_endings_and_blank_lines() {
        let source = "page Home() {\r\n\r\n\r\nreturn ASX { <Copy>Ready</Copy> }\r\n}\r\n\r\n";
        let expected = "page Home() {\n\n  return ASX { <Copy>Ready</Copy> }\n}\n";

        assert_eq!(format_ax_source(source), expected);
    }

    #[test]
    fn ignores_structural_characters_inside_strings_and_comments() {
        let source = "page Home() {\nconst sample = \"{ [ ( <Fake>\"\n// } </Fake>\nreturn ASX {\n<Copy>{sample}</Copy>\n}\n}\n";
        let expected = "page Home() {\n  const sample = \"{ [ ( <Fake>\"\n  // } </Fake>\n  return ASX {\n    <Copy>{sample}</Copy>\n  }\n}\n";

        assert_eq!(format_ax_source(source), expected);
    }

    #[test]
    fn formatted_frontend_and_backend_sources_still_parse() {
        let frontend = "page Home() {\nreturn ASX {\n<><Copy>{title}</Copy><Meta name=\"description\" content=\"Ready\" /></>\n}\n}\n";
        let backend = "export query loadPosts() -> Post[] {\ndata posts = db.posts.all().where({status: \"published\"})\nreturn posts\n}\n";

        parse_ax_v2(&format_ax_source(frontend)).expect("formatted ASX should parse");
        parse_backend_ax(&format_ax_source(backend))
            .expect("formatted backend source should parse");
    }

    #[test]
    fn preserves_backend_doc_comments_and_ignores_braces_in_legacy_comments() {
        let source = "/// Returns the endpoint.\nexport fn endpoint() -> String {\n// Keep // and } inside comments.\nreturn \"https://example.test\" // URL\n}\n";
        let formatted = format_ax_source(source);
        assert_eq!(formatted, "/// Returns the endpoint.\nexport fn endpoint() -> String {\n  // Keep // and } inside comments.\n  return \"https://example.test\" // URL\n}\n");
        assert_eq!(format_ax_source(&formatted), formatted);
        assert_eq!(
            parse_backend_ax(&formatted).unwrap(),
            parse_backend_ax(
                "export fn endpoint() -> String {\n  return \"https://example.test\"\n}\n"
            )
            .unwrap()
        );

        let legacy = "/// Legacy loader.\nloader Posts // { not a body delimiter\n  data posts = db.posts.all()\n  return posts\n";
        assert_eq!(format_ax_source(legacy), legacy);
        parse_backend_ax(legacy).unwrap();
    }

    #[test]
    fn preserves_embedded_client_and_style_block_contents() {
        let source = "component Probe() {\nclient JS {\n    const template = `first\n  second`;  \n    if (ready) {\n      mount();\n    }\n}\nstyle {\n  .probe { white-space: pre; }  \n}\nrender ASX { <div class=\"probe\">Ready</div> }\n}\n";
        let expected = "component Probe() {\n  client JS {\n    const template = `first\n  second`;  \n    if (ready) {\n      mount();\n    }\n  }\n  style {\n  .probe { white-space: pre; }  \n  }\n  render ASX { <div class=\"probe\">Ready</div> }\n}\n";

        assert_eq!(format_ax_source(source), expected);
    }

    #[test]
    fn does_not_treat_generic_types_as_markup_tags() {
        let source = "export query loadPosts(input: Map<String, Post>) -> List<Post> {\ndata posts: List<Post> = db.posts.all()\nreturn posts\n}\n";
        let expected = "export query loadPosts(input: Map<String, Post>) -> List<Post> {\n  data posts: List<Post> = db.posts.all()\n  return posts\n}\n";

        let formatted = format_ax_source(source);
        assert_eq!(formatted, expected);
        parse_backend_ax(&formatted).expect("formatted generic backend source should parse");
    }
}
