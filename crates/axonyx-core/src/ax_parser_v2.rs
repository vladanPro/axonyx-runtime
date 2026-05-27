use thiserror::Error;

use crate::ax_ast_v2::prelude::*;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum AxParseV2Error {
    #[error("document is empty")]
    EmptyDocument,
    #[error("invalid import syntax at line {line}")]
    InvalidImport { line: usize },
    #[error("invalid use syntax at line {line}")]
    InvalidUse { line: usize },
    #[error("missing `from` in import at line {line}")]
    MissingImportFrom { line: usize },
    #[error("empty import list at line {line}")]
    EmptyImportList { line: usize },
    #[error("expected `page <Name>` at line {line}")]
    InvalidPage { line: usize },
    #[error("invalid let syntax at line {line}")]
    InvalidLet { line: usize },
    #[error("invalid state syntax at line {line}")]
    InvalidState { line: usize },
    #[error("invalid type syntax at line {line}")]
    InvalidType { line: usize },
    #[error("invalid function syntax at line {line}")]
    InvalidFunction { line: usize },
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

        let mut package_uses = Vec::new();
        let mut imports = Vec::new();
        while self.starts_with_keyword("use") || self.starts_with_keyword("import") {
            if self.starts_with_keyword("use") {
                package_uses.push(self.parse_use()?);
            } else {
                imports.push(self.parse_import()?);
            }
            self.skip_layout_whitespace();
        }

        let page = self.parse_page_decl()?;
        self.skip_layout_whitespace();

        let mut types = Vec::new();
        let mut lets = Vec::new();
        let mut states = Vec::new();
        let mut functions = Vec::new();
        let mut components = Vec::new();
        while self.starts_with_keyword("type")
            || self.starts_with_keyword("let")
            || self.is_state_decl_start()
            || self.starts_with_keyword("fn")
            || self.starts_with_keyword("component")
        {
            if self.starts_with_keyword("type") {
                types.push(self.parse_type_decl()?);
            } else if self.starts_with_keyword("let") {
                lets.push(self.parse_let_decl()?);
            } else if self.is_state_decl_start() {
                states.push(self.parse_state_decl()?);
            } else if self.starts_with_keyword("fn") {
                functions.push(self.parse_function_decl()?);
            } else {
                components.push(self.parse_component_decl()?);
            }
            self.skip_layout_whitespace();
        }

        let body = self.parse_nodes(None)?;

        Ok(AxFileV2 {
            package_uses,
            imports,
            page,
            types,
            lets,
            states,
            functions,
            components,
            body,
        })
    }

    fn parse_use(&mut self) -> Result<String, AxParseV2Error> {
        let line = self.line;
        self.expect_keyword("use")
            .map_err(|_| AxParseV2Error::InvalidUse { line })?;
        self.skip_spaces();

        let source = self
            .parse_string_literal()
            .map_err(|_| AxParseV2Error::InvalidUse { line })?;
        self.skip_spaces();
        if !matches!(self.peek_char(), None | Some('\n') | Some('\r')) {
            return Err(AxParseV2Error::InvalidUse { line });
        }
        self.consume_until_line_end();

        Ok(source)
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
        self.skip_spaces();

        let mut params = Vec::new();
        if self.peek_char() == Some('(') {
            params = self.parse_param_list(line, AxParseV2Error::InvalidPage { line })?;
        }

        self.consume_until_line_end();
        self.page_seen = true;

        Ok(AxPageDecl::with_params(name, params))
    }

    fn parse_let_decl(&mut self) -> Result<AxLetDeclV2, AxParseV2Error> {
        let line = self.line;
        self.expect_keyword("let")
            .map_err(|_| AxParseV2Error::InvalidLet { line })?;
        self.skip_spaces();

        let name = self.parse_identifier()?;
        self.skip_spaces();

        let ty = if self.peek_char() == Some(':') {
            self.bump_char();
            self.skip_spaces();
            let ty = self.read_until_top_level_equals().trim().to_string();
            if ty.is_empty() {
                return Err(AxParseV2Error::InvalidLet { line });
            }
            self.skip_spaces();
            Some(ty)
        } else {
            None
        };

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

        Ok(match ty {
            Some(ty) => AxLetDeclV2::typed(name, ty, value),
            None => AxLetDeclV2::new(name, value),
        })
    }

    fn parse_state_decl(&mut self) -> Result<AxStateDeclV2, AxParseV2Error> {
        let line = self.line;
        let scope = if self.starts_with_keyword("app")
            || self.starts_with_keyword("layout")
            || self.starts_with_keyword("page")
        {
            let scope = self
                .parse_identifier()
                .map_err(|_| AxParseV2Error::InvalidState { line })?;
            self.skip_spaces();
            Some(scope)
        } else {
            None
        };

        self.expect_keyword("state")
            .map_err(|_| AxParseV2Error::InvalidState { line })?;
        self.skip_spaces();

        let name = self.parse_identifier()?;
        self.skip_spaces();

        let ty = if self.peek_char() == Some(':') {
            self.bump_char();
            self.skip_spaces();
            let ty = self.read_until_top_level_equals().trim().to_string();
            if ty.is_empty() {
                return Err(AxParseV2Error::InvalidState { line });
            }
            self.skip_spaces();
            Some(ty)
        } else {
            None
        };

        if self.peek_char() != Some('=') {
            return Err(AxParseV2Error::InvalidState { line });
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
            return Err(AxParseV2Error::InvalidState { line });
        }

        Ok(match (scope, ty) {
            (Some(scope), Some(ty)) => AxStateDeclV2::typed_scoped(scope, name, ty, value),
            (Some(scope), None) => AxStateDeclV2::scoped(scope, name, value),
            (None, Some(ty)) => AxStateDeclV2::typed(name, ty, value),
            (None, None) => AxStateDeclV2::new(name, value),
        })
    }

    fn parse_type_decl(&mut self) -> Result<AxTypeDeclV2, AxParseV2Error> {
        let line = self.line;
        self.expect_keyword("type")
            .map_err(|_| AxParseV2Error::InvalidType { line })?;
        self.skip_spaces();

        let name = self.parse_identifier()?;
        self.skip_layout_whitespace();
        if self.peek_char() != Some('{') {
            return Err(AxParseV2Error::InvalidType { line });
        }
        self.bump_char();
        self.skip_layout_whitespace();

        let mut fields = Vec::new();
        while !self.eof() && self.peek_char() != Some('}') {
            let field_line = self.line;
            let field_name = self
                .parse_identifier()
                .map_err(|_| AxParseV2Error::InvalidType { line: field_line })?;
            self.skip_spaces();
            let optional = if self.peek_char() == Some('?') {
                self.bump_char();
                self.skip_spaces();
                true
            } else {
                false
            };
            if self.peek_char() != Some(':') {
                return Err(AxParseV2Error::InvalidType { line: field_line });
            }
            self.bump_char();
            self.skip_spaces();

            let ty = self
                .read_until_line_end()
                .trim()
                .trim_end_matches(',')
                .trim_end_matches(';')
                .trim()
                .to_string();
            if ty.is_empty() {
                return Err(AxParseV2Error::InvalidType { line: field_line });
            }
            let ty = if optional {
                format!("Optional<{ty}>")
            } else {
                ty
            };
            fields.push(AxTypeFieldDeclV2::new(field_name, ty));
            self.skip_layout_whitespace();
        }

        if self.peek_char() != Some('}') {
            return Err(AxParseV2Error::InvalidType { line });
        }
        self.bump_char();
        self.consume_until_line_end();

        Ok(AxTypeDeclV2::new(name, fields))
    }

    fn parse_function_decl(&mut self) -> Result<AxFunctionDeclV2, AxParseV2Error> {
        let line = self.line;
        self.expect_keyword("fn")
            .map_err(|_| AxParseV2Error::InvalidFunction { line })?;
        self.skip_spaces();

        let name = self.parse_identifier()?;
        self.skip_spaces();

        if self.peek_char() != Some('(') {
            return Err(AxParseV2Error::InvalidFunction { line });
        }
        let params = self.parse_param_list(line, AxParseV2Error::InvalidFunction { line })?;
        self.skip_spaces();

        if self.peek_char() != Some('=') {
            return Err(AxParseV2Error::InvalidFunction { line });
        }
        self.bump_char();
        self.skip_spaces();

        let body = self
            .read_until_line_end()
            .trim()
            .trim_end_matches(';')
            .trim()
            .to_string();
        if body.is_empty() {
            return Err(AxParseV2Error::InvalidFunction { line });
        }

        Ok(AxFunctionDeclV2::new(name, params, body))
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
            params = self.parse_param_list(line, AxParseV2Error::InvalidComponent { line })?;
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

    fn parse_param_list(
        &mut self,
        line: usize,
        error: AxParseV2Error,
    ) -> Result<Vec<AxComponentParamDeclV2>, AxParseV2Error> {
        let mut params = Vec::new();
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
                _ => return Err(error.clone()),
            }
        }

        Ok(params)
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
        let name = self.parse_attribute_name()?;
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

    fn parse_attribute_name(&mut self) -> Result<String, AxParseV2Error> {
        let mut name = self.parse_name_like(true)?;
        if name.is_empty() {
            return Ok(name);
        }

        if self.peek_char() == Some(':') {
            self.bump_char();
            let suffix = self.parse_name_like(true)?;
            if suffix.is_empty() {
                return Ok(String::new());
            }
            name.push(':');
            name.push_str(&suffix);
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

    fn is_state_decl_start(&self) -> bool {
        self.starts_with_keyword("state")
            || self.starts_with_scoped_state_keyword("app")
            || self.starts_with_scoped_state_keyword("layout")
            || self.starts_with_scoped_state_keyword("page")
    }

    fn starts_with_scoped_state_keyword(&self, keyword: &str) -> bool {
        if !self.starts_with_keyword(keyword) {
            return false;
        }

        let rest = &self.input[self.pos + keyword.len()..];
        let trimmed = rest.trim_start_matches([' ', '\t']);
        trimmed.starts_with("state")
            && trimmed["state".len()..]
                .chars()
                .next()
                .is_some_and(|ch| ch.is_whitespace())
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

    fn read_until_top_level_equals(&mut self) -> String {
        let start = self.pos;
        let mut string_quote = None;
        let mut angle_depth = 0usize;
        let mut paren_depth = 0usize;

        while let Some(ch) = self.peek_char() {
            if let Some(quote) = string_quote {
                self.bump_char();
                if ch == '\\' {
                    self.bump_char();
                } else if ch == quote {
                    string_quote = None;
                }
                continue;
            }

            match ch {
                '"' | '\'' => {
                    string_quote = Some(ch);
                    self.bump_char();
                }
                '<' => {
                    angle_depth += 1;
                    self.bump_char();
                }
                '>' => {
                    angle_depth = angle_depth.saturating_sub(1);
                    self.bump_char();
                }
                '(' => {
                    paren_depth += 1;
                    self.bump_char();
                }
                ')' => {
                    paren_depth = paren_depth.saturating_sub(1);
                    self.bump_char();
                }
                '=' if angle_depth == 0 && paren_depth == 0 => break,
                '\n' | '\r' => break,
                _ => {
                    self.bump_char();
                }
            }
        }

        self.input[start..self.pos].to_string()
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
        assert!(file.page.params.is_empty());

        let AxNodeV2::Element(card) = &file.body[1] else {
            panic!("expected card element");
        };
        assert_eq!(card.name, "Card");
        assert_eq!(card.attrs.len(), 1);
        assert!(!card.self_closing);
        assert_eq!(card.children.len(), 2);
    }

    #[test]
    fn parses_package_use_directives_before_page() {
        let input = r#"
use "@axonyx/ui"
import { Card } from "@axonyx/ui/foundry/Card.ax"

page Home

<Card title="Hello" />
"#;

        let file = parse_ax_v2(input).expect("v2 file should parse");

        assert_eq!(file.package_uses, vec!["@axonyx/ui"]);
        assert_eq!(file.imports.len(), 1);
        assert_eq!(file.page.name, "Home");
    }

    #[test]
    fn parses_page_param_defaults_for_package_components() {
        let input = r#"
page Card(title = "Untitled", tone = "surface")

<article>{title}</article>
"#;

        let file = parse_ax_v2(input).expect("page params should parse");

        assert_eq!(file.page.name, "Card");
        assert_eq!(
            file.page.params,
            vec![
                AxComponentParamDeclV2::with_default("title", "\"Untitled\""),
                AxComponentParamDeclV2::with_default("tone", "\"surface\"")
            ]
        );
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
    fn parses_top_level_state_declarations_and_bind_attrs() {
        let input = r#"
page Home

state theme = "silver"
state count: Number = 0

<input bind:value={theme} />
"#;

        let file = parse_ax_v2(input).expect("state declaration should parse");

        assert_eq!(
            file.states,
            vec![
                AxStateDeclV2::new("theme", r#""silver""#),
                AxStateDeclV2::typed("count", "Number", "0")
            ]
        );
        let AxNodeV2::Element(input) = &file.body[0] else {
            panic!("expected input element");
        };
        assert_eq!(input.attrs[0], AxAttributeNode::expr("bind:value", "theme"));
    }

    #[test]
    fn parses_scoped_state_declarations_before_page_body() {
        let input = r#"
page Home

app state language: String = "sr"
layout state sidebarOpen: Bool = false
page state filter = signal("")

<Copy>Body</Copy>
"#;

        let file = parse_ax_v2(input).expect("scoped state declarations should parse");

        assert_eq!(
            file.states,
            vec![
                AxStateDeclV2::typed_scoped("app", "language", "String", r#""sr""#),
                AxStateDeclV2::typed_scoped("layout", "sidebarOpen", "Bool", "false"),
                AxStateDeclV2::scoped("page", "filter", r#"signal("")"#),
            ]
        );
    }

    #[test]
    fn parses_typed_top_level_let_declarations() {
        let input = r#"
page Blog

let posts: List<Post> = load PostsList

<Each items={posts} as="post">
  <Card title={post.title} />
</Each>
"#;

        let file = parse_ax_v2(input).expect("typed let declaration should parse");

        assert_eq!(
            file.lets[0],
            AxLetDeclV2::typed("posts", "List<Post>", "load PostsList")
        );
    }

    #[test]
    fn parses_top_level_type_declarations() {
        let input = r#"
page Blog

type Post {
  title: String
  slug: String
  excerpt?: String
  published: Bool
}

let posts: List<Post> = load PostsList

<Each items={posts} as="post">
  <Card title={post.title} />
</Each>
"#;

        let file = parse_ax_v2(input).expect("type declaration should parse");

        assert_eq!(
            file.types,
            vec![AxTypeDeclV2::new(
                "Post",
                [
                    AxTypeFieldDeclV2::new("title", "String"),
                    AxTypeFieldDeclV2::new("slug", "String"),
                    AxTypeFieldDeclV2::new("excerpt", "Optional<String>"),
                    AxTypeFieldDeclV2::new("published", "Bool"),
                ]
            )]
        );
    }

    #[test]
    fn parses_top_level_function_declarations_before_page_body() {
        let input = r#"
page Home

fn heroTitle(title = "Hello") = title

<Copy>{heroTitle()}</Copy>
"#;

        let file = parse_ax_v2(input).expect("function declaration should parse");

        assert_eq!(file.functions.len(), 1);
        assert_eq!(file.functions[0].name, "heroTitle");
        assert_eq!(
            file.functions[0].params,
            vec![AxComponentParamDeclV2::with_default("title", "\"Hello\"")]
        );
        assert_eq!(file.functions[0].body, "title");
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
