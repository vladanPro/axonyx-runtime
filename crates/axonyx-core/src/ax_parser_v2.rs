use thiserror::Error;

use crate::ax_ast_v2::prelude::*;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AxParseV2Error {
    #[error("document is empty")]
    EmptyDocument,
    #[error("invalid import syntax at line {line}")]
    InvalidImport { line: usize },
    #[error("missing `from` in import at line {line}")]
    MissingImportFrom { line: usize },
    #[error("empty import list at line {line}")]
    EmptyImportList { line: usize },
    #[error("expected `page <Name>` at line {line}")]
    InvalidPage { line: usize },
    #[error("invalid let syntax at line {line}")]
    InvalidLet { line: usize },
    #[error("invalid component syntax at line {line}")]
    InvalidComponent { line: usize },
    #[error("duplicate page declaration at line {line}")]
    DuplicatePage { line: usize },
    #[error("missing page declaration")]
    MissingPage,
    #[error("invalid tag syntax at line {line}")]
    InvalidTag { line: usize },
    #[error("unterminated tag at line {line}")]
    UnterminatedTag { line: usize },
    #[error("unterminated string literal at line {line}")]
    UnterminatedString { line: usize },
    #[error("unterminated expression block at line {line}")]
    UnterminatedExpression { line: usize },
    #[error("unexpected closing tag `</{name}>` at line {line}")]
    UnexpectedClosingTag { line: usize, name: String },
    #[error("mismatched closing tag `</{found}>` at line {line}, expected `</{expected}>`")]
    MismatchedClosingTag {
        line: usize,
        expected: String,
        found: String,
    },
    #[error("attribute `{name}` is missing a value at line {line}")]
    MissingAttributeValue { line: usize, name: String },
}

pub fn parse_ax_v2(input: &str) -> Result<AxFileV2, AxParseV2Error> {
    let mut parser = Parser::new(input);
    parser.parse_file()
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
    line: usize,
    page_seen: bool,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            pos: 0,
            line: 1,
            page_seen: false,
        }
    }

    fn parse_file(&mut self) -> Result<AxFileV2, AxParseV2Error> {
        if self.input.trim().is_empty() {
            return Err(AxParseV2Error::EmptyDocument);
        }

        self.skip_layout_whitespace();

        let mut imports = Vec::new();
        while self.starts_with_keyword("import") {
            imports.push(self.parse_import()?);
            self.skip_layout_whitespace();
        }

        let page = self.parse_page_decl()?;
        self.skip_layout_whitespace();

        let mut lets = Vec::new();
        let mut components = Vec::new();
        while self.starts_with_keyword("let") || self.starts_with_keyword("component") {
            if self.starts_with_keyword("let") {
                lets.push(self.parse_let_decl()?);
            } else {
                components.push(self.parse_component_decl()?);
            }
            self.skip_layout_whitespace();
        }

        let body = self.parse_nodes(None)?;

        Ok(AxFileV2 {
            imports,
            page,
            lets,
            components,
            body,
        })
    }

    fn parse_import(&mut self) -> Result<AxImportDecl, AxParseV2Error> {
        let line = self.line;
        self.expect_keyword("import")
            .map_err(|_| AxParseV2Error::InvalidImport { line })?;
        self.skip_spaces();

        if self.peek_char() != Some('{') {
            return Err(AxParseV2Error::InvalidImport { line });
        }
        self.bump_char();

        let mut bindings = Vec::new();
        loop {
            self.skip_spaces();
            if self.peek_char() == Some('}') {
                self.bump_char();
                break;
            }

            let imported = self.parse_identifier()?;
            self.skip_spaces();
            let local = if self.starts_with_keyword("as") {
                self.expect_keyword("as")
                    .map_err(|_| AxParseV2Error::InvalidImport { line })?;
                self.skip_spaces();
                self.parse_identifier()?
            } else {
                imported.clone()
            };
            bindings.push(AxImportBinding::new(imported, local));
            self.skip_spaces();

            match self.peek_char() {
                Some(',') => {
                    self.bump_char();
                }
                Some('}') => {
                    self.bump_char();
                    break;
                }
                _ => return Err(AxParseV2Error::InvalidImport { line }),
            }
        }

        if bindings.is_empty() {
            return Err(AxParseV2Error::EmptyImportList { line });
        }

        self.skip_spaces();
        if !self.starts_with_keyword("from") {
            return Err(AxParseV2Error::MissingImportFrom { line });
        }
        self.expect_keyword("from")
            .map_err(|_| AxParseV2Error::MissingImportFrom { line })?;
        self.skip_spaces();

        let source = self.parse_string_literal()?;
        self.consume_until_line_end();

        Ok(AxImportDecl::new(bindings, source))
    }

    fn parse_page_decl(&mut self) -> Result<AxPageDecl, AxParseV2Error> {
        let line = self.line;
        if self.page_seen {
            return Err(AxParseV2Error::DuplicatePage { line });
        }

        if !self.starts_with_keyword("page") {
            return Err(AxParseV2Error::MissingPage);
        }

        self.expect_keyword("page")
            .map_err(|_| AxParseV2Error::InvalidPage { line })?;
        self.skip_spaces();
        let name = self.parse_identifier()?;
        self.consume_until_line_end();
        self.page_seen = true;

        Ok(AxPageDecl::new(name))
    }

    fn parse_let_decl(&mut self) -> Result<AxLetDeclV2, AxParseV2Error> {
        let line = self.line;
        self.expect_keyword("let")
            .map_err(|_| AxParseV2Error::InvalidLet { line })?;
        self.skip_spaces();

        let name = self.parse_identifier()?;
        self.skip_spaces();

        if self.peek_char() != Some('=') {
            return Err(AxParseV2Error::InvalidLet { line });
        }
        self.bump_char();
        self.skip_spaces();

        let value = self
            .read_until_line_end()
            .trim()
            .trim_end_matches(';')
            .trim()
            .to_string();
        if value.is_empty() {
            return Err(AxParseV2Error::InvalidLet { line });
        }

        Ok(AxLetDeclV2::new(name, value))
    }

    fn parse_component_decl(&mut self) -> Result<AxComponentDeclV2, AxParseV2Error> {
        let line = self.line;
        self.expect_keyword("component")
            .map_err(|_| AxParseV2Error::InvalidComponent { line })?;
        self.skip_spaces();

        let name = self.parse_identifier()?;
        self.skip_spaces();

        let mut params = Vec::new();
        if self.peek_char() == Some('(') {
            self.bump_char();
            loop {
                self.skip_spaces();
                if self.peek_char() == Some(')') {
                    self.bump_char();
                    break;
                }

                params.push(self.parse_component_param(line)?);

                match self.peek_char() {
                    Some(',') => {
                        self.bump_char();
                    }
                    Some(')') => {
                        self.bump_char();
                        break;
                    }
                    _ => return Err(AxParseV2Error::InvalidComponent { line }),
                }
            }
        }

        self.skip_layout_whitespace();
        if self.peek_char() != Some('{') {
            return Err(AxParseV2Error::InvalidComponent { line });
        }
        self.bump_char();
        self.skip_layout_whitespace();
        let body = self.parse_nodes_until_component_body_end()?;

        Ok(AxComponentDeclV2::new(name, params, body))
    }

    fn parse_component_param(
        &mut self,
        line: usize,
    ) -> Result<AxComponentParamDeclV2, AxParseV2Error> {
        let name = self.parse_identifier()?;
        self.skip_spaces();

        if self.peek_char() != Some('=') {
            return Ok(AxComponentParamDeclV2::new(name));
        }

        self.bump_char();
        self.skip_spaces();

        let default = self.read_component_param_default().trim().to_string();
        if default.is_empty() {
            return Err(AxParseV2Error::InvalidComponent { line });
        }

        Ok(AxComponentParamDeclV2::with_default(name, default))
    }

    fn parse_nodes(&mut self, closing_tag: Option<&str>) -> Result<Vec<AxNodeV2>, AxParseV2Error> {
        let mut nodes = Vec::new();

        while !self.eof() {
            if closing_tag.is_none() && self.starts_with_keyword("page") {
                return Err(AxParseV2Error::DuplicatePage { line: self.line });
            }

            if self.starts_with("</") {
                if let Some(expected) = closing_tag {
                    let found = self.parse_closing_tag()?;
                    if found != expected {
                        return Err(AxParseV2Error::MismatchedClosingTag {
                            line: self.line,
                            expected: expected.to_string(),
                            found,
                        });
                    }
                    return Ok(nodes);
                }

                let line = self.line;
                self.bump_char();
                self.bump_char();
                let name = if self.peek_char() == Some('>') {
                    "Fragment".to_string()
                } else {
                    self.parse_identifier()?
                };
                return Err(AxParseV2Error::UnexpectedClosingTag { line, name });
            }

            if self.peek_char() == Some('<') {
                nodes.push(AxNodeV2::Element(self.parse_element()?));
                continue;
            }

            if self.peek_char() == Some('{') {
                nodes.push(AxNodeV2::Expr(AxExprNode::new(
                    self.parse_expression_block()?,
                )));
                continue;
            }

            if let Some(text) = self.parse_text_node() {
                nodes.push(AxNodeV2::Text(AxTextNode::new(text)));
            }
        }

        if let Some(expected) = closing_tag {
            return Err(AxParseV2Error::MismatchedClosingTag {
                line: self.line,
                expected: expected.to_string(),
                found: "EOF".to_string(),
            });
        }

        Ok(nodes)
    }

    fn parse_nodes_until_component_body_end(&mut self) -> Result<Vec<AxNodeV2>, AxParseV2Error> {
        let mut nodes = Vec::new();

        while !self.eof() {
            if self.peek_char() == Some('}') {
                self.bump_char();
                return Ok(nodes);
            }

            if self.starts_with("</") {
                let line = self.line;
                self.bump_char();
                self.bump_char();
                let name = if self.peek_char() == Some('>') {
                    "Fragment".to_string()
                } else {
                    self.parse_identifier()?
                };
                return Err(AxParseV2Error::UnexpectedClosingTag { line, name });
            }

            if self.peek_char() == Some('<') {
                nodes.push(AxNodeV2::Element(self.parse_element()?));
                continue;
            }

            if self.peek_char() == Some('{') {
                nodes.push(AxNodeV2::Expr(AxExprNode::new(
                    self.parse_expression_block()?,
                )));
                continue;
            }

            if let Some(text) = self.parse_text_node_until_component_body_end() {
                nodes.push(AxNodeV2::Text(AxTextNode::new(text)));
            }
        }

        Err(AxParseV2Error::InvalidComponent { line: self.line })
    }

    fn parse_element(&mut self) -> Result<AxElementNode, AxParseV2Error> {
        let line = self.line;
        if self.peek_char() != Some('<') {
            return Err(AxParseV2Error::InvalidTag { line });
        }
        self.bump_char();

        if self.peek_char() == Some('/') {
            return Err(AxParseV2Error::UnexpectedClosingTag {
                line,
                name: if self.input[self.pos + 1..].starts_with('>') {
                    "Fragment".to_string()
                } else {
                    self.parse_identifier().unwrap_or_default()
                },
            });
        }

        let name = if self.peek_char() == Some('>') {
            "Fragment".to_string()
        } else {
            self.parse_tag_name()?
        };
        let mut element = AxElementNode::new(name.clone());

        loop {
            self.skip_spaces_and_newlines_in_tag();

            if self.starts_with("/>") {
                self.bump_char();
                self.bump_char();
                element.self_closing = true;
                return Ok(element);
            }

            if self.peek_char() == Some('>') {
                self.bump_char();
                element.children = self.parse_nodes(Some(&name))?;
                return Ok(element);
            }

            if self.eof() {
                return Err(AxParseV2Error::UnterminatedTag { line });
            }

            element.attrs.push(self.parse_attribute()?);
        }
    }

    fn parse_attribute(&mut self) -> Result<AxAttributeNode, AxParseV2Error> {
        let line = self.line;
        let name = self.parse_name_like(true)?;
        if name.is_empty() {
            return Err(AxParseV2Error::InvalidTag { line });
        }
        self.skip_spaces();

        if self.peek_char() != Some('=') {
            return Err(AxParseV2Error::MissingAttributeValue { line, name });
        }
        self.bump_char();
        self.skip_spaces();

        match self.peek_char() {
            Some('"') | Some('\'') => {
                let value = self.parse_string_literal()?;
                Ok(AxAttributeNode::string(name, value))
            }
            Some('{') => {
                let source = self.parse_expression_block()?;
                Ok(AxAttributeNode::expr(name, source))
            }
            _ => Err(AxParseV2Error::MissingAttributeValue { line, name }),
        }
    }

    fn parse_text_node(&mut self) -> Option<String> {
        let start = self.pos;
        while let Some(ch) = self.peek_char() {
            if ch == '<' || ch == '{' {
                break;
            }
            self.bump_char();
        }

        let raw = &self.input[start..self.pos];
        normalize_text(raw)
    }

    fn parse_text_node_until_component_body_end(&mut self) -> Option<String> {
        let start = self.pos;
        while let Some(ch) = self.peek_char() {
            if ch == '<' || ch == '{' || ch == '}' {
                break;
            }
            self.bump_char();
        }

        let raw = &self.input[start..self.pos];
        normalize_text(raw)
    }

    fn parse_expression_block(&mut self) -> Result<String, AxParseV2Error> {
        let line = self.line;
        if self.peek_char() != Some('{') {
            return Err(AxParseV2Error::UnterminatedExpression { line });
        }
        self.bump_char();

        let start = self.pos;
        let mut depth = 1usize;
        let mut in_string: Option<char> = None;

        while let Some(ch) = self.peek_char() {
            match in_string {
                Some(quote) => {
                    self.bump_char();
                    if ch == '\\' {
                        if self.peek_char().is_some() {
                            self.bump_char();
                        }
                    } else if ch == quote {
                        in_string = None;
                    }
                }
                None => match ch {
                    '"' | '\'' => {
                        in_string = Some(ch);
                        self.bump_char();
                    }
                    '{' => {
                        depth += 1;
                        self.bump_char();
                    }
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            let source = self.input[start..self.pos].trim().to_string();
                            self.bump_char();
                            return Ok(source);
                        }
                        self.bump_char();
                    }
                    _ => {
                        self.bump_char();
                    }
                },
            }
        }

        Err(AxParseV2Error::UnterminatedExpression { line })
    }

    fn parse_string_literal(&mut self) -> Result<String, AxParseV2Error> {
        let line = self.line;
        let Some(quote) = self.peek_char() else {
            return Err(AxParseV2Error::UnterminatedString { line });
        };
        if quote != '"' && quote != '\'' {
            return Err(AxParseV2Error::UnterminatedString { line });
        }
        self.bump_char();

        let mut value = String::new();
        while let Some(ch) = self.peek_char() {
            self.bump_char();
            match ch {
                '\\' => {
                    let Some(escaped) = self.peek_char() else {
                        return Err(AxParseV2Error::UnterminatedString { line });
                    };
                    self.bump_char();
                    value.push(match escaped {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        '\\' => '\\',
                        '"' => '"',
                        '\'' => '\'',
                        other => other,
                    });
                }
                c if c == quote => return Ok(value),
                other => value.push(other),
            }
        }

        Err(AxParseV2Error::UnterminatedString { line })
    }

    fn parse_closing_tag(&mut self) -> Result<String, AxParseV2Error> {
        let line = self.line;
        if !self.starts_with("</") {
            return Err(AxParseV2Error::InvalidTag { line });
        }
        self.bump_char();
        self.bump_char();
        self.skip_spaces();
        let name = if self.peek_char() == Some('>') {
            "Fragment".to_string()
        } else {
            self.parse_tag_name()?
        };
        self.skip_spaces_and_newlines_in_tag();
        if self.peek_char() != Some('>') {
            return Err(AxParseV2Error::UnterminatedTag { line });
        }
        self.bump_char();
        Ok(name)
    }

    fn parse_tag_name(&mut self) -> Result<String, AxParseV2Error> {
        let line = self.line;
        let name = self.parse_name_like(true)?;
        if name.is_empty() {
            return Err(AxParseV2Error::InvalidTag { line });
        }
        Ok(name)
    }

    fn parse_identifier(&mut self) -> Result<String, AxParseV2Error> {
        let line = self.line;
        let name = self.parse_name_like(false)?;
        if name.is_empty() {
            return Err(AxParseV2Error::InvalidTag { line });
        }
        Ok(name)
    }

    fn parse_name_like(&mut self, allow_hyphen: bool) -> Result<String, AxParseV2Error> {
        let mut value = String::new();
        let Some(first) = self.peek_char() else {
            return Ok(value);
        };

        if !(first.is_ascii_alphabetic()
            || first == '_'
            || (allow_hyphen && first.is_ascii_lowercase()))
        {
            return Ok(value);
        }

        value.push(first);
        self.bump_char();

        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_alphanumeric() || ch == '_' || (allow_hyphen && ch == '-') {
                value.push(ch);
                self.bump_char();
            } else {
                break;
            }
        }

        Ok(value)
    }

    fn starts_with_keyword(&self, keyword: &str) -> bool {
        let rest = &self.input[self.pos..];
        if !rest.starts_with(keyword) {
            return false;
        }

        match rest[keyword.len()..].chars().next() {
            None => true,
            Some(ch) => ch.is_whitespace() || ch == '{',
        }
    }

    fn expect_keyword(&mut self, keyword: &str) -> Result<(), ()> {
        if !self.starts_with_keyword(keyword) {
            return Err(());
        }

        for _ in keyword.chars() {
            self.bump_char();
        }
        Ok(())
    }

    fn consume_until_line_end(&mut self) {
        while let Some(ch) = self.peek_char() {
            self.bump_char();
            if ch == '\n' {
                break;
            }
        }
    }

    fn read_until_line_end(&mut self) -> String {
        let start = self.pos;
        while let Some(ch) = self.peek_char() {
            if ch == '\n' || ch == '\r' {
                break;
            }
            self.bump_char();
        }

        let value = self.input[start..self.pos].to_string();
        self.consume_until_line_end();
        value
    }

    fn read_component_param_default(&mut self) -> String {
        let start = self.pos;
        let mut string_quote = None;
        let mut paren_depth = 0usize;

        while let Some(ch) = self.peek_char() {
            if let Some(quote) = string_quote {
                self.bump_char();
                if ch == quote {
                    string_quote = None;
                }
                continue;
            }

            match ch {
                '"' | '\'' => {
                    string_quote = Some(ch);
                    self.bump_char();
                }
                '(' => {
                    paren_depth += 1;
                    self.bump_char();
                }
                ')' if paren_depth == 0 => break,
                ')' => {
                    paren_depth -= 1;
                    self.bump_char();
                }
                ',' if paren_depth == 0 => break,
                _ => {
                    self.bump_char();
                }
            }
        }

        self.input[start..self.pos].to_string()
    }

    fn skip_layout_whitespace(&mut self) {
        while let Some(ch) = self.peek_char() {
            if ch.is_whitespace() {
                self.bump_char();
            } else {
                break;
            }
        }
    }

    fn skip_spaces(&mut self) {
        while let Some(ch) = self.peek_char() {
            if ch == ' ' || ch == '\r' || ch == '\t' {
                self.bump_char();
            } else {
                break;
            }
        }
    }

    fn skip_spaces_and_newlines_in_tag(&mut self) {
        while let Some(ch) = self.peek_char() {
            if ch == ' ' || ch == '\r' || ch == '\t' || ch == '\n' {
                self.bump_char();
            } else {
                break;
            }
        }
    }

    fn starts_with(&self, needle: &str) -> bool {
        self.input[self.pos..].starts_with(needle)
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn bump_char(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.pos += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
        }
        Some(ch)
    }

    fn eof(&self) -> bool {
        self.pos >= self.input.len()
    }
}

fn normalize_text(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if !trimmed.contains('\n') {
        return Some(trimmed.to_string());
    }

    let lines = trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

pub mod prelude {
    pub use super::parse_ax_v2;
    pub use super::AxParseV2Error;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v2_imports_page_and_nested_elements() {
        let input = r#"
import { Card, Copy } from "@axonyx/ui"
import { SiteHeader as Header } from "@/components/SiteHeader.ax"

page Home

<Header />

<Card title="Hello">
  <Copy>World</Copy>
  <Copy>{subtitle}</Copy>
</Card>
"#;

        let file = parse_ax_v2(input).expect("v2 file should parse");

        assert_eq!(file.imports.len(), 2);
        assert_eq!(file.page.name, "Home");
        assert_eq!(file.body.len(), 2);
        assert_eq!(file.imports[1].bindings[0].imported, "SiteHeader");
        assert_eq!(file.imports[1].bindings[0].local, "Header");

        let AxNodeV2::Element(card) = &file.body[1] else {
            panic!("expected card element");
        };
        assert_eq!(card.name, "Card");
        assert_eq!(card.attrs.len(), 1);
        assert!(!card.self_closing);
        assert_eq!(card.children.len(), 2);
    }

    #[test]
    fn parses_self_closing_elements_and_expression_attributes() {
        let input = r#"
page Home

<Grid cols={3} gap="lg" />
"#;

        let file = parse_ax_v2(input).expect("v2 file should parse");

        let AxNodeV2::Element(grid) = &file.body[0] else {
            panic!("expected grid element");
        };
        assert_eq!(grid.name, "Grid");
        assert!(grid.self_closing);
        assert_eq!(
            grid.attrs,
            vec![
                AxAttributeNode::expr("cols", "3"),
                AxAttributeNode::string("gap", "lg"),
            ]
        );
    }

    #[test]
    fn parses_local_component_declarations_before_page_body() {
        let input = r#"
page Home

component FeatureCard(title, tone) {
  <Card title={title}>
    <Copy tone={tone}>
      <Slot />
    </Copy>
  </Card>
}

<FeatureCard title="Hello" tone="lead">
  World
</FeatureCard>
"#;

        let file = parse_ax_v2(input).expect("component declaration should parse");

        assert_eq!(file.components.len(), 1);
        assert_eq!(file.components[0].name, "FeatureCard");
        assert_eq!(
            file.components[0].params,
            vec![
                AxComponentParamDeclV2::new("title"),
                AxComponentParamDeclV2::new("tone")
            ]
        );
        assert_eq!(file.components[0].body.len(), 1);
        assert_eq!(file.body.len(), 1);
    }

    #[test]
    fn parses_local_component_param_defaults() {
        let input = r#"
page Home

component FeatureCard(title = "Hello", tone = defaultTone, count = 2) {
  <Card title={title}>
    <Copy tone={tone}>{count}</Copy>
  </Card>
}

<FeatureCard />
"#;

        let file = parse_ax_v2(input).expect("component defaults should parse");

        assert_eq!(
            file.components[0].params,
            vec![
                AxComponentParamDeclV2::with_default("title", "\"Hello\""),
                AxComponentParamDeclV2::with_default("tone", "defaultTone"),
                AxComponentParamDeclV2::with_default("count", "2")
            ]
        );
    }

    #[test]
    fn parses_top_level_let_declarations_before_page_body() {
        let input = r#"
page Home

let heroTitle = "Hello Axonyx";
let columns = 3

<Grid cols={columns}>
  <Copy>{heroTitle}</Copy>
</Grid>
"#;

        let file = parse_ax_v2(input).expect("let declarations should parse");

        assert_eq!(file.lets.len(), 2);
        assert_eq!(
            file.lets[0],
            AxLetDeclV2::new("heroTitle", "\"Hello Axonyx\"")
        );
        assert_eq!(file.lets[1], AxLetDeclV2::new("columns", "3"));
        assert_eq!(file.body.len(), 1);
    }

    #[test]
    fn rejects_empty_top_level_let_declarations() {
        let input = r#"
page Home

let title =

<Copy>Body</Copy>
"#;

        let error = parse_ax_v2(input).expect_err("empty let should fail");

        assert!(matches!(error, AxParseV2Error::InvalidLet { .. }));
    }

    #[test]
    fn rejects_unclosed_local_component_declarations() {
        let input = r#"
page Home

component FeatureCard(title) {
  <Card title={title} />
"#;

        let error = parse_ax_v2(input).expect_err("component declaration should be unclosed");

        assert!(matches!(error, AxParseV2Error::InvalidComponent { .. }));
    }

    #[test]
    fn trims_structural_text_whitespace_but_keeps_text_children() {
        let input = r#"
page Home

<Copy>
  Hello Axonyx
</Copy>
"#;

        let file = parse_ax_v2(input).expect("v2 file should parse");

        let AxNodeV2::Element(copy) = &file.body[0] else {
            panic!("expected copy element");
        };
        assert_eq!(
            copy.children,
            vec![AxNodeV2::Text(AxTextNode::new("Hello Axonyx"))]
        );
    }

    #[test]
    fn parses_fragment_shorthand_as_fragment_element() {
        let input = r#"
page Home

<>
  Hello
  <strong>Axonyx</strong>
</>
"#;

        let file = parse_ax_v2(input).expect("v2 file should parse");

        let AxNodeV2::Element(fragment) = &file.body[0] else {
            panic!("expected fragment element");
        };
        assert_eq!(fragment.name, "Fragment");
        assert_eq!(fragment.children.len(), 2);
    }

    #[test]
    fn rejects_mismatched_closing_tags() {
        let input = r#"
page Home

<Card>
  <Copy>Hello</Grid>
</Card>
"#;

        let error = parse_ax_v2(input).expect_err("parse should fail");
        assert!(matches!(error, AxParseV2Error::MismatchedClosingTag { .. }));
    }

    #[test]
    fn rejects_missing_import_from() {
        let input = r#"
import { Card }

page Home
"#;

        let error = parse_ax_v2(input).expect_err("parse should fail");
        assert_eq!(error, AxParseV2Error::MissingImportFrom { line: 2 });
    }

    #[test]
    fn rejects_missing_page() {
        let input = r#"
import { Card } from "@axonyx/ui"

<Card />
"#;

        let error = parse_ax_v2(input).expect_err("parse should fail");
        assert_eq!(error, AxParseV2Error::MissingPage);
    }

    #[test]
    fn rejects_duplicate_page_declarations() {
        let input = r#"
page Home

page Docs
"#;

        let error = parse_ax_v2(input).expect_err("parse should fail");
        assert!(matches!(error, AxParseV2Error::DuplicatePage { .. }));
    }
}
