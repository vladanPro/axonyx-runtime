use thiserror::Error;

use crate::ax_ast::prelude::{AxExpr, AxFloat};
use crate::ax_backend_ast::prelude::*;
use crate::ax_query_ast::prelude::*;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AxBackendParseError {
    #[error("document is empty")]
    EmptyDocument,
    #[error("invalid import syntax at line {line}")]
    InvalidImport { line: usize },
    #[error("missing `from` in import at line {line}")]
    MissingImportFrom { line: usize },
    #[error("empty import list at line {line}")]
    EmptyImportList { line: usize },
    #[error("tabs are not supported in indentation at line {line}")]
    TabsNotSupported { line: usize },
    #[error("indentation must use multiples of two spaces at line {line}")]
    InvalidIndentation { line: usize },
    #[error("unexpected indentation at line {line}")]
    UnexpectedIndentation { line: usize },
    #[error("invalid block header at line {line}")]
    InvalidBlock { line: usize },
    #[error("invalid data binding at line {line}")]
    InvalidDataBinding { line: usize },
    #[error("invalid env declaration at line {line}")]
    InvalidEnvDeclaration { line: usize },
    #[error("invalid input section at line {line}")]
    InvalidInputSection { line: usize },
    #[error("invalid field declaration at line {line}")]
    InvalidField { line: usize },
    #[error("invalid type declaration at line {line}")]
    InvalidTypeDeclaration { line: usize },
    #[error("invalid mutation at line {line}")]
    InvalidMutation { line: usize },
    #[error("invalid assignment at line {line}")]
    InvalidAssignment { line: usize },
    #[error("invalid response header at line {line}")]
    InvalidHeader { line: usize },
    #[error("invalid response cookie at line {line}")]
    InvalidCookie { line: usize },
    #[error("invalid hook at line {line}")]
    InvalidHook { line: usize },
    #[error("invalid requirement at line {line}")]
    InvalidRequirement { line: usize },
    #[error("invalid return statement at line {line}")]
    InvalidReturn { line: usize },
    #[error("invalid send statement at line {line}")]
    InvalidSend { line: usize },
    #[error("invalid scope declaration at line {line}")]
    InvalidScope { line: usize },
    #[error("invalid scope member at line {line}")]
    InvalidScopeMember { line: usize },
    #[error("invalid scope state declaration at line {line}")]
    InvalidScopeState { line: usize },
    #[error("invalid scope render declaration at line {line}")]
    InvalidScopeRender { line: usize },
    #[error("invalid query source at line {line}")]
    InvalidQuerySource { line: usize },
    #[error("invalid query clause at line {line}")]
    InvalidQueryClause { line: usize },
    #[error("invalid query number at line {line}")]
    InvalidQueryNumber { line: usize },
    #[error("invalid expression at line {line}: {message}")]
    InvalidExpression { line: usize, message: String },
}

#[derive(Debug, Clone)]
struct BackendLine {
    line: usize,
    indent: usize,
    text: String,
}

pub fn parse_backend_ax(input: &str) -> Result<AxBackendDocument, AxBackendParseError> {
    let lines = preprocess(input)?;
    if lines.is_empty() {
        return Err(AxBackendParseError::EmptyDocument);
    }

    let mut parser = Parser {
        lines,
        pos: 0,
        synthetic_counter: 0,
    };
    parser.parse_document()
}

struct Parser {
    lines: Vec<BackendLine>,
    pos: usize,
    synthetic_counter: usize,
}

impl Parser {
    fn parse_document(&mut self) -> Result<AxBackendDocument, AxBackendParseError> {
        let mut imports = Vec::new();
        let mut types = Vec::new();
        let mut blocks = Vec::new();

        while let Some(line) = self.current() {
            if line.indent != 0 {
                return Err(AxBackendParseError::UnexpectedIndentation { line: line.line });
            }
            if !line.text.starts_with("import ") {
                break;
            }
            imports.push(self.parse_import()?);
        }

        while self.current().is_some() {
            let line = self.current().expect("checked");
            if line.indent != 0 {
                return Err(AxBackendParseError::UnexpectedIndentation { line: line.line });
            }
            if line.text.starts_with("type ") || line.text.starts_with("export type ") {
                types.push(self.parse_type_block()?);
                continue;
            }
            blocks.push(self.parse_block()?);
        }

        Ok(AxBackendDocument::with_imports_and_types(
            imports, types, blocks,
        ))
    }

    fn parse_import(&mut self) -> Result<AxBackendImport, AxBackendParseError> {
        let line = self.current().expect("import line exists").clone();
        let Some(rest) = line.text.strip_prefix("import ") else {
            return Err(AxBackendParseError::InvalidImport { line: line.line });
        };
        let Some((bindings, source)) = split_top_level_once(rest.trim(), " from ") else {
            return Err(AxBackendParseError::MissingImportFrom { line: line.line });
        };
        let bindings = bindings.trim();
        let parsed_bindings = if let Some(local) = bindings.strip_prefix("* as ") {
            let local = local.trim();
            if !is_backend_identifier(local) {
                return Err(AxBackendParseError::InvalidImport { line: line.line });
            }

            vec![AxBackendImportBinding::namespace(local)]
        } else {
            if !bindings.starts_with('{') || !bindings.ends_with('}') {
                return Err(AxBackendParseError::InvalidImport { line: line.line });
            }

            let inner = bindings
                .trim_start_matches('{')
                .trim_end_matches('}')
                .trim();
            if inner.is_empty() {
                return Err(AxBackendParseError::EmptyImportList { line: line.line });
            }

            let mut parsed_bindings = Vec::new();
            for binding in split_top_level_commas(inner) {
                let binding = binding.trim();
                if binding.is_empty() {
                    return Err(AxBackendParseError::InvalidImport { line: line.line });
                }

                let (imported, local) =
                    if let Some((imported, local)) = split_top_level_once(binding, " as ") {
                        (imported.trim(), local.trim())
                    } else {
                        (binding, binding)
                    };
                if !is_backend_identifier(imported) || !is_backend_identifier(local) {
                    return Err(AxBackendParseError::InvalidImport { line: line.line });
                }
                parsed_bindings.push(AxBackendImportBinding::new(imported, local));
            }
            parsed_bindings
        };

        let source = trim_quotes(source.trim());
        if source.is_empty() {
            return Err(AxBackendParseError::InvalidImport { line: line.line });
        }

        self.pos += 1;
        Ok(AxBackendImport::new(parsed_bindings, source))
    }

    fn parse_block(&mut self) -> Result<AxBackendBlock, AxBackendParseError> {
        let line = self.current().expect("block line exists").clone();
        let mut text = line.text.as_str();
        let exported = if let Some(rest) = text.strip_prefix("export ") {
            text = rest.trim_start();
            true
        } else {
            false
        };

        if text == "backend" {
            if exported {
                return Err(AxBackendParseError::InvalidBlock { line: line.line });
            }
            self.pos += 1;
            return Ok(AxBackendBlock::Backend(AxBackendRoot::new(
                self.parse_backend_root_body(2)?,
            )));
        }

        if let Some(rest) = text.strip_prefix("route ") {
            let mut parts = rest.splitn(2, ' ');
            let method = parts.next().unwrap_or_default().trim();
            let path = parts.next().unwrap_or_default().trim();
            let (path, braced) = split_block_brace(path);
            let (path, returns) = split_return_contract(path);
            if method.is_empty() || path.is_empty() {
                return Err(AxBackendParseError::InvalidBlock { line: line.line });
            }

            self.pos += 1;
            let (input, body) = self.parse_input_sections(2)?;
            self.consume_block_brace(braced, 0)?;
            let mut route = AxRoute::new(method, trim_quotes(path), body).input(input);
            if let Some(returns) = returns {
                route = route.returns(returns);
            }
            return Ok(AxBackendBlock::Route(route));
        }

        if let Some(name) = text.strip_prefix("loader ") {
            let (name, braced) = split_block_brace(name);
            let (name, returns) = split_return_contract(name.trim());
            let (name, signature_input) = parse_backend_callable_signature(name, line.line)?;
            if name.is_empty() {
                return Err(AxBackendParseError::InvalidBlock { line: line.line });
            }

            self.pos += 1;
            let body = self.parse_body(2)?;
            self.consume_block_brace(braced, 0)?;
            let mut loader = AxLoader::new(name, body)
                .input(signature_input)
                .exported(exported);
            if let Some(returns) = returns {
                loader = loader.returns(returns);
            }
            return Ok(AxBackendBlock::Loader(loader));
        }

        if let Some(name) = text.strip_prefix("query ") {
            let (name, braced) = split_block_brace(name);
            let (name, returns) = split_return_contract(name.trim());
            let (name, signature_input) = parse_backend_callable_signature(name, line.line)?;
            if name.is_empty() {
                return Err(AxBackendParseError::InvalidBlock { line: line.line });
            }

            self.pos += 1;
            let body = self.parse_body(2)?;
            self.consume_block_brace(braced, 0)?;
            let mut loader = AxLoader::new(name, body)
                .input(signature_input)
                .exported(exported);
            if let Some(returns) = returns {
                loader = loader.returns(returns);
            }
            return Ok(AxBackendBlock::Loader(loader));
        }

        if let Some(name) = text.strip_prefix("action ") {
            let (name, braced) = split_block_brace(name);
            let (name, returns) = split_return_contract(name.trim());
            let (name, signature_input) = parse_backend_callable_signature(name, line.line)?;
            if name.is_empty() {
                return Err(AxBackendParseError::InvalidBlock { line: line.line });
            }

            self.pos += 1;
            let (section_input, body) = self.parse_input_sections(2)?;
            self.consume_block_brace(braced, 0)?;
            let mut input = signature_input;
            input.extend(section_input);
            let mut action = AxAction::new(name)
                .input(input)
                .body(body)
                .exported(exported);
            if let Some(returns) = returns {
                action = action.returns(returns);
            }
            return Ok(AxBackendBlock::Action(action));
        }

        if let Some(name) = text.strip_prefix("fn ") {
            let (name, braced) = split_block_brace(name);
            let (name, returns) = split_return_contract(name.trim());
            let (name, signature_input) = parse_backend_callable_signature(name, line.line)?;
            if name.is_empty() {
                return Err(AxBackendParseError::InvalidBlock { line: line.line });
            }

            self.pos += 1;
            let body = self.parse_body(2)?;
            self.consume_block_brace(braced, 0)?;
            let mut function = AxBackendFunction::new(name, body)
                .input(signature_input)
                .exported(exported);
            if let Some(returns) = returns {
                function = function.returns(returns);
            }
            return Ok(AxBackendBlock::Function(function));
        }

        if let Some(name) = text.strip_prefix("job ") {
            if exported {
                return Err(AxBackendParseError::InvalidBlock { line: line.line });
            }
            let (name, braced) = split_block_brace(name);
            let name = name.trim();
            if name.is_empty() {
                return Err(AxBackendParseError::InvalidBlock { line: line.line });
            }

            self.pos += 1;
            let body = self.parse_body(2)?;
            self.consume_block_brace(braced, 0)?;
            return Ok(AxBackendBlock::Job(AxJob::new(name, body)));
        }

        if let Some(rest) = text.strip_prefix("scope ") {
            if exported {
                return Err(AxBackendParseError::InvalidScope { line: line.line });
            }
            let (rest, braced) = split_block_brace(rest);
            let (name, members) = parse_scope_header(rest, line.line)?;

            self.pos += 1;
            let body = self.parse_scope_body(2)?;
            self.consume_block_brace(braced, 0)?;
            return Ok(AxBackendBlock::Scope(AxScope::new(name, members, body)));
        }

        Err(AxBackendParseError::InvalidBlock { line: line.line })
    }

    fn parse_type_block(&mut self) -> Result<AxBackendTypeDecl, AxBackendParseError> {
        let line = self.current().expect("type line exists").clone();
        let (rest, exported) = if let Some(rest) = line.text.strip_prefix("export type ") {
            (rest, true)
        } else if let Some(rest) = line.text.strip_prefix("type ") {
            (rest, false)
        } else {
            return Err(AxBackendParseError::InvalidTypeDeclaration { line: line.line });
        };
        if let Some((name, source)) = rest.split_once('=') {
            let name = name.trim();
            let literals = parse_backend_literal_union(source.trim())
                .ok_or(AxBackendParseError::InvalidTypeDeclaration { line: line.line })?;
            if !is_backend_identifier(name) {
                return Err(AxBackendParseError::InvalidTypeDeclaration { line: line.line });
            }
            self.pos += 1;
            return Ok(AxBackendTypeDecl::literal_union(name, literals, exported));
        }
        let (name, braced) = split_block_brace(rest);
        if !braced || !is_backend_identifier(name.trim()) {
            return Err(AxBackendParseError::InvalidTypeDeclaration { line: line.line });
        }

        self.pos += 1;
        let mut fields = Vec::new();
        while let Some(current) = self.current() {
            if current.indent == 0 && current.text == "}" {
                self.pos += 1;
                return Ok(AxBackendTypeDecl::new(name.trim(), fields, exported));
            }
            if current.indent != 2 {
                return Err(AxBackendParseError::UnexpectedIndentation { line: current.line });
            }
            let Some((raw_name, raw_ty)) = current.text.split_once(':') else {
                return Err(AxBackendParseError::InvalidField { line: current.line });
            };
            let optional = raw_name.trim().ends_with('?');
            let field_name = raw_name.trim().trim_end_matches('?').trim();
            let field_ty = raw_ty.trim().trim_end_matches(',').trim();
            if !is_backend_identifier(field_name) || field_ty.is_empty() {
                return Err(AxBackendParseError::InvalidField { line: current.line });
            }
            let field_ty = if optional {
                format!("Optional<{field_ty}>")
            } else {
                field_ty.to_string()
            };
            fields.push(AxBackendTypeField::new(field_name, field_ty));
            self.pos += 1;
        }

        Err(AxBackendParseError::InvalidTypeDeclaration { line: line.line })
    }

    fn parse_input_sections(
        &mut self,
        indent: usize,
    ) -> Result<(Vec<AxField>, Vec<AxBackendStmt>), AxBackendParseError> {
        let mut input = Vec::new();
        let mut body = Vec::new();

        while let Some(line) = self.current() {
            if line.indent < indent {
                break;
            }

            if line.indent > indent {
                return Err(AxBackendParseError::UnexpectedIndentation { line: line.line });
            }

            if line.text == "input:" {
                self.pos += 1;
                input = self.parse_input_fields(indent + 2)?;
            } else if line.text == "input {" {
                self.pos += 1;
                input = self.parse_input_fields(indent + 2)?;
                self.consume_block_brace(true, indent)?;
            } else {
                body.extend(self.parse_statements(indent)?);
            }
        }

        Ok((input, body))
    }

    fn parse_input_fields(&mut self, indent: usize) -> Result<Vec<AxField>, AxBackendParseError> {
        let mut fields = Vec::new();

        while let Some(line) = self.current() {
            if line.indent < indent {
                break;
            }

            if line.indent > indent {
                return Err(AxBackendParseError::UnexpectedIndentation { line: line.line });
            }

            let Some((name, ty)) = line.text.split_once(':') else {
                return Err(AxBackendParseError::InvalidField { line: line.line });
            };

            let raw_name = name.trim();
            let optional = raw_name.ends_with('?');
            let name = raw_name.trim_end_matches('?').trim();
            let (ty, default) = match ty.split_once('=') {
                Some((ty, default)) => {
                    let default = default.trim();
                    if default.is_empty() {
                        return Err(AxBackendParseError::InvalidField { line: line.line });
                    }
                    (ty.trim(), Some(parse_expr(default, line.line)?))
                }
                None => (ty.trim(), None),
            };
            if name.is_empty() || ty.is_empty() {
                return Err(AxBackendParseError::InvalidField { line: line.line });
            }

            fields.push(match (optional, default) {
                (true, Some(default)) => AxField::optional_with_default(name, ty, default),
                (true, None) => AxField::optional(name, ty),
                (false, Some(default)) => AxField::with_default(name, ty, default),
                (false, None) => AxField::new(name, ty),
            });
            self.pos += 1;
        }

        if fields.is_empty() {
            let line = self
                .current()
                .map(|line| line.line)
                .unwrap_or(self.lines.last().map(|line| line.line).unwrap_or(1));
            return Err(AxBackendParseError::InvalidInputSection { line });
        }

        Ok(fields)
    }

    fn parse_body(&mut self, indent: usize) -> Result<Vec<AxBackendStmt>, AxBackendParseError> {
        let mut body = Vec::new();

        while let Some(line) = self.current() {
            if line.indent < indent {
                break;
            }

            if line.indent > indent {
                return Err(AxBackendParseError::UnexpectedIndentation { line: line.line });
            }

            body.extend(self.parse_statements(indent)?);
        }

        Ok(body)
    }

    fn parse_scope_body(&mut self, indent: usize) -> Result<Vec<AxScopeStmt>, AxBackendParseError> {
        let mut body = Vec::new();

        while let Some(line) = self.current() {
            if line.indent < indent {
                break;
            }

            if line.indent > indent {
                return Err(AxBackendParseError::UnexpectedIndentation { line: line.line });
            }

            body.push(self.parse_scope_statement()?);
        }

        Ok(body)
    }

    fn parse_scope_statement(&mut self) -> Result<AxScopeStmt, AxBackendParseError> {
        let line = self.current().expect("scope statement line exists").clone();
        let text = line.text.as_str();

        if let Some(rest) = text.strip_prefix("state ") {
            let Some((name, ty)) = split_top_level_once(rest, ":") else {
                return Err(AxBackendParseError::InvalidScopeState { line: line.line });
            };
            let name = name.trim();
            if !is_backend_identifier(name) {
                return Err(AxBackendParseError::InvalidScopeState { line: line.line });
            }

            let (ty, default) = match split_top_level_once(ty, "=") {
                Some((ty, default)) => {
                    let ty = ty.trim();
                    let default = default.trim();
                    if ty.is_empty() || default.is_empty() {
                        return Err(AxBackendParseError::InvalidScopeState { line: line.line });
                    }
                    (ty, Some(parse_expr(default, line.line)?))
                }
                None => {
                    let ty = ty.trim();
                    if ty.is_empty() {
                        return Err(AxBackendParseError::InvalidScopeState { line: line.line });
                    }
                    (ty, None)
                }
            };

            self.pos += 1;
            let mut state = AxScopeState::new(name, ty);
            if let Some(default) = default {
                state = state.default(default);
            }
            return Ok(AxScopeStmt::State(state));
        }

        if let Some(call) = text.strip_prefix("render ") {
            let call = call.trim();
            if call.is_empty() {
                return Err(AxBackendParseError::InvalidScopeRender { line: line.line });
            }
            let call = parse_expr(call, line.line)?;
            if !matches!(&call, AxExpr::Call { path, .. } if path.as_slice() != ["list"]) {
                return Err(AxBackendParseError::InvalidScopeRender { line: line.line });
            }

            self.pos += 1;
            return Ok(AxScopeStmt::render(call));
        }
        if text == "render" {
            return Err(AxBackendParseError::InvalidScopeRender { line: line.line });
        }

        Err(AxBackendParseError::InvalidScope { line: line.line })
    }

    fn consume_block_brace(
        &mut self,
        braced: bool,
        indent: usize,
    ) -> Result<(), AxBackendParseError> {
        if !braced {
            return Ok(());
        }

        let Some(line) = self.current() else {
            return Err(AxBackendParseError::InvalidBlock { line: 1 });
        };

        if line.indent != indent || line.text != "}" {
            return Err(AxBackendParseError::InvalidBlock { line: line.line });
        }

        self.pos += 1;
        Ok(())
    }

    fn parse_backend_root_body(
        &mut self,
        indent: usize,
    ) -> Result<Vec<AxBackendStmt>, AxBackendParseError> {
        let mut body = Vec::new();

        while let Some(line) = self.current() {
            if line.indent < indent {
                break;
            }

            if line.indent > indent {
                return Err(AxBackendParseError::UnexpectedIndentation { line: line.line });
            }

            if !(line.text.starts_with("data ") || line.text.starts_with("env ")) {
                return Err(AxBackendParseError::InvalidBlock { line: line.line });
            }

            if line.text.starts_with("env ") {
                body.push(self.parse_env()?);
            } else {
                body.push(self.parse_data_binding()?);
            }
        }

        Ok(body)
    }

    fn parse_statements(
        &mut self,
        indent: usize,
    ) -> Result<Vec<AxBackendStmt>, AxBackendParseError> {
        let line = self.current().expect("statement line exists").clone();
        let text = line.text.as_str();

        if let Some(value) = text.strip_prefix("return ") {
            return self.parse_return_statements(&value.to_string(), line, indent);
        }

        Ok(vec![self.parse_statement(indent)?])
    }

    fn parse_statement(&mut self, indent: usize) -> Result<AxBackendStmt, AxBackendParseError> {
        let line = self.current().expect("statement line exists").clone();
        let text = line.text.as_str();

        if is_data_binding_statement(text) {
            return self.parse_data_binding();
        }

        if text.starts_with("insert ") {
            return self.parse_mutation(indent, true);
        }

        if text.starts_with("update ") {
            return self.parse_mutation(indent, false);
        }

        if text.starts_with("delete ") {
            return self.parse_delete(indent);
        }

        if let Some(statement) = parse_fluent_mutation_stmt(text, line.line)? {
            self.pos += 1;
            return Ok(statement);
        }

        if let Some(statement) = parse_function_call_stmt(text, line.line)? {
            self.pos += 1;
            return Ok(statement);
        }

        if let Some(value) = text.strip_prefix("revalidate ") {
            self.pos += 1;
            return Ok(AxBackendStmt::revalidate(parse_expr(
                value.trim(),
                line.line,
            )?));
        }

        if let Some(value) = text.strip_prefix("invalidate ") {
            self.pos += 1;
            return Ok(AxBackendStmt::invalidate(parse_expr(
                value.trim(),
                line.line,
            )?));
        }

        if let Some(rest) = text.strip_prefix("patch ") {
            let Some((signal, value)) = rest.split_once('=') else {
                return Err(AxBackendParseError::InvalidAssignment { line: line.line });
            };

            let signal = signal.trim();
            let value = value.trim();
            if signal.is_empty() || value.is_empty() {
                return Err(AxBackendParseError::InvalidAssignment { line: line.line });
            }

            self.pos += 1;
            return Ok(AxBackendStmt::patch(
                parse_expr(signal, line.line)?,
                parse_expr(value, line.line)?,
            ));
        }

        if let Some(value) = text.strip_prefix("before ") {
            let value = value.trim();
            if value.is_empty() {
                return Err(AxBackendParseError::InvalidHook { line: line.line });
            }

            self.pos += 1;
            return Ok(AxBackendStmt::before(parse_expr(value, line.line)?));
        }

        if let Some(value) = text.strip_prefix("after ") {
            let value = value.trim();
            if value.is_empty() {
                return Err(AxBackendParseError::InvalidHook { line: line.line });
            }

            self.pos += 1;
            return Ok(AxBackendStmt::after(parse_expr(value, line.line)?));
        }

        if let Some(rest) = text.strip_prefix("header ") {
            let Some((name, value)) = rest.split_once('=') else {
                return Err(AxBackendParseError::InvalidHeader { line: line.line });
            };

            let name = name.trim();
            let value = value.trim();
            if name.is_empty() || value.is_empty() {
                return Err(AxBackendParseError::InvalidHeader { line: line.line });
            }

            self.pos += 1;
            return Ok(AxBackendStmt::header(
                parse_expr(name, line.line)?,
                parse_expr(value, line.line)?,
            ));
        }

        if let Some(rest) = text.strip_prefix("cookie ") {
            let Some((name, value)) = rest.split_once('=') else {
                return Err(AxBackendParseError::InvalidCookie { line: line.line });
            };

            let name = name.trim();
            let value = value.trim();
            if name.is_empty() || value.is_empty() {
                return Err(AxBackendParseError::InvalidCookie { line: line.line });
            }

            self.pos += 1;
            return Ok(AxBackendStmt::cookie(
                parse_expr(name, line.line)?,
                parse_expr(value, line.line)?,
            ));
        }

        if let Some(name) = text.strip_prefix("clearCookie ") {
            let name = name.trim();
            if name.is_empty() {
                return Err(AxBackendParseError::InvalidCookie { line: line.line });
            }

            self.pos += 1;
            return Ok(AxBackendStmt::clear_cookie(parse_expr(name, line.line)?));
        }

        if let Some(value) = text.strip_prefix("require ") {
            let (value, fallback) = match value.split_once(" else ") {
                Some((value, fallback)) => (value.trim(), Some(fallback.trim())),
                None => (value.trim(), None),
            };
            if value.is_empty() {
                return Err(AxBackendParseError::InvalidRequirement { line: line.line });
            }
            if matches!(fallback, Some("")) {
                return Err(AxBackendParseError::InvalidRequirement { line: line.line });
            }

            self.pos += 1;
            let value = parse_requirement_expr(value, line.line)?;
            return match fallback {
                Some(fallback) => Ok(AxBackendStmt::require_with_fallback(
                    value,
                    parse_requirement_fallback_expr(fallback, line.line)?,
                )),
                None => Ok(AxBackendStmt::require(value)),
            };
        }

        if text.starts_with("guard(") && text.ends_with(')') {
            self.pos += 1;
            let (value, message) = parse_guard_call(text, line.line)?;
            return Ok(AxBackendStmt::require_with_fallback(
                value,
                AxExpr::call(["error"], [message]),
            ));
        }

        if let Some(value) = text.strip_prefix("return ") {
            self.pos += 1;
            let value = value.trim();
            if value.is_empty() {
                return Err(AxBackendParseError::InvalidReturn { line: line.line });
            }
            if value == "ok" {
                return Ok(AxBackendStmt::r#return("ok"));
            }
            if value == "ok()" {
                return Ok(AxBackendStmt::r#return("ok"));
            }
            return Ok(AxBackendStmt::r#return(parse_expr(value, line.line)?));
        }

        if let Some(rest) = text.strip_prefix("send ") {
            let Some((target, payload)) = rest.split_once(" with ") else {
                return Err(AxBackendParseError::InvalidSend { line: line.line });
            };

            let target = target.trim();
            let payload = payload.trim();
            if target.is_empty() || payload.is_empty() {
                return Err(AxBackendParseError::InvalidSend { line: line.line });
            }

            self.pos += 1;
            return Ok(AxBackendStmt::send(target, parse_expr(payload, line.line)?));
        }

        Err(AxBackendParseError::InvalidBlock { line: line.line })
    }

    fn parse_return_statements(
        &mut self,
        value: &str,
        line: BackendLine,
        indent: usize,
    ) -> Result<Vec<AxBackendStmt>, AxBackendParseError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(AxBackendParseError::InvalidReturn { line: line.line });
        }
        if value == "ok" || value == "ok()" {
            self.pos += 1;
            return Ok(vec![AxBackendStmt::r#return("ok")]);
        }

        if let Some(query) = parse_fluent_query_spec(value, line.line)? {
            self.pos += 1;
            let binding = self.next_synthetic_return_binding();
            return Ok(vec![
                AxBackendStmt::data(binding.clone(), query),
                AxBackendStmt::r#return(AxExpr::ident(binding)),
            ]);
        }

        let expr = parse_expr(value, line.line)?;
        self.pos += 1;

        if let Some(next) = self.current() {
            if next.indent == indent + 2 && is_query_clause(&next.text) {
                let query = self.parse_query_spec(expr, line.line, indent + 2)?;
                let binding = self.next_synthetic_return_binding();
                return Ok(vec![
                    AxBackendStmt::data(binding.clone(), query),
                    AxBackendStmt::r#return(AxExpr::ident(binding)),
                ]);
            }
        }

        if let Ok(query) = query_spec_from_expr(expr.clone(), line.line) {
            let binding = self.next_synthetic_return_binding();
            return Ok(vec![
                AxBackendStmt::data(binding.clone(), query),
                AxBackendStmt::r#return(AxExpr::ident(binding)),
            ]);
        }

        Ok(vec![AxBackendStmt::r#return(expr)])
    }

    fn next_synthetic_return_binding(&mut self) -> String {
        self.synthetic_counter += 1;
        format!("__ax_return_{}", self.synthetic_counter)
    }

    fn parse_data_binding(&mut self) -> Result<AxBackendStmt, AxBackendParseError> {
        let line = self.current().expect("data binding line exists").clone();
        let Some((_, body)) = split_data_binding_prefix(&line.text) else {
            return Err(AxBackendParseError::InvalidDataBinding { line: line.line });
        };
        let Some((name, expr)) = body.split_once('=') else {
            return Err(AxBackendParseError::InvalidDataBinding { line: line.line });
        };

        let name = name
            .split_once(':')
            .map(|(name, _)| name)
            .unwrap_or(name)
            .trim();
        let expr = expr.trim();
        if name.is_empty() || expr.is_empty() {
            return Err(AxBackendParseError::InvalidDataBinding { line: line.line });
        }

        if let Some(query) = parse_fluent_query_spec(expr, line.line)? {
            self.pos += 1;
            return Ok(AxBackendStmt::data(name, query));
        }

        let expr = parse_expr(expr, line.line)?;
        self.pos += 1;

        if let Some(next) = self.current() {
            if next.indent == line.indent + 2 && is_query_clause(&next.text) {
                let query = self.parse_query_spec(expr, line.line, line.indent + 2)?;
                return Ok(AxBackendStmt::data(name, query));
            }
        }

        if let Ok(query) = query_spec_from_expr(expr.clone(), line.line) {
            return Ok(AxBackendStmt::data(name, query));
        }

        Ok(AxBackendStmt::data(name, expr))
    }

    fn parse_env(&mut self) -> Result<AxBackendStmt, AxBackendParseError> {
        let line = self.current().expect("env line exists").clone();
        let body = line.text["env ".len()..].trim();
        let Some((name, ty)) = body.split_once(':') else {
            return Err(AxBackendParseError::InvalidEnvDeclaration { line: line.line });
        };

        let name = name.trim();
        let ty = ty.trim();
        if name.is_empty() || ty.is_empty() {
            return Err(AxBackendParseError::InvalidEnvDeclaration { line: line.line });
        }

        let (visibility, inner_ty) = parse_env_type(ty, line.line)?;
        self.pos += 1;

        Ok(AxBackendStmt::env(name, visibility, inner_ty))
    }

    fn parse_mutation(
        &mut self,
        indent: usize,
        is_insert: bool,
    ) -> Result<AxBackendStmt, AxBackendParseError> {
        let line = self.current().expect("mutation line exists").clone();
        let prefix = if is_insert { "insert " } else { "update " };
        let collection = line.text[prefix.len()..].trim();
        if collection.is_empty() {
            return Err(AxBackendParseError::InvalidMutation { line: line.line });
        }

        self.pos += 1;
        let (fields, filters) = self.parse_mutation_body(indent + 2)?;

        if is_insert {
            let mut mutation = AxMutation::new(trim_quotes(collection), fields);
            for filter in filters {
                mutation = mutation.filter(filter);
            }
            Ok(AxBackendStmt::Insert(mutation))
        } else {
            let mut mutation = AxMutation::new(trim_quotes(collection), fields);
            for filter in filters {
                mutation = mutation.filter(filter);
            }
            Ok(AxBackendStmt::Update(mutation))
        }
    }

    fn parse_delete(&mut self, indent: usize) -> Result<AxBackendStmt, AxBackendParseError> {
        let line = self.current().expect("delete line exists").clone();
        let collection = line.text["delete ".len()..].trim();
        if collection.is_empty() {
            return Err(AxBackendParseError::InvalidMutation { line: line.line });
        }

        self.pos += 1;
        let (_fields, filters) = self.parse_mutation_body(indent + 2)?;
        let mut mutation = AxMutation::new(trim_quotes(collection), []);
        for filter in filters {
            mutation = mutation.filter(filter);
        }
        Ok(AxBackendStmt::Delete(mutation))
    }

    fn parse_mutation_body(
        &mut self,
        indent: usize,
    ) -> Result<(Vec<AxAssignment>, Vec<AxQueryFilter>), AxBackendParseError> {
        let mut fields = Vec::new();
        let mut filters = Vec::new();
        let mut parsing_filters = false;

        while let Some(line) = self.current() {
            if line.indent < indent {
                break;
            }

            if line.indent > indent {
                return Err(AxBackendParseError::UnexpectedIndentation { line: line.line });
            }

            if let Some(rest) = line.text.strip_prefix("where ") {
                parsing_filters = true;
                let Some((field, value)) = rest.split_once('=') else {
                    return Err(AxBackendParseError::InvalidQueryClause { line: line.line });
                };

                let field = field.trim();
                let value = value.trim();
                if field.is_empty() || value.is_empty() {
                    return Err(AxBackendParseError::InvalidQueryClause { line: line.line });
                }

                filters.push(AxQueryFilter::new(
                    field,
                    AxQueryFilterOp::Eq,
                    parse_expr(value, line.line)?,
                ));
                self.pos += 1;
                continue;
            }

            if parsing_filters {
                return Err(AxBackendParseError::InvalidQueryClause { line: line.line });
            }

            let Some((name, value)) = line.text.split_once(':') else {
                return Err(AxBackendParseError::InvalidAssignment { line: line.line });
            };

            let name = name.trim();
            let value = value.trim();
            if name.is_empty() || value.is_empty() {
                return Err(AxBackendParseError::InvalidAssignment { line: line.line });
            }

            fields.push(AxAssignment::new(name, parse_expr(value, line.line)?));
            self.pos += 1;
        }

        if fields.is_empty() && filters.is_empty() {
            let line = self
                .current()
                .map(|line| line.line)
                .unwrap_or(self.lines.last().map(|line| line.line).unwrap_or(1));
            return Err(AxBackendParseError::InvalidMutation { line });
        }

        Ok((fields, filters))
    }

    fn current(&self) -> Option<&BackendLine> {
        self.lines.get(self.pos)
    }

    fn parse_query_spec(
        &mut self,
        expr: AxExpr,
        line: usize,
        indent: usize,
    ) -> Result<AxQuerySpec, AxBackendParseError> {
        let mut query = query_spec_from_expr(expr, line)?;

        while let Some(clause_line) = self.current() {
            if clause_line.indent < indent {
                break;
            }

            if clause_line.indent != indent {
                return Err(AxBackendParseError::UnexpectedIndentation {
                    line: clause_line.line,
                });
            }

            let text = clause_line.text.as_str();
            let method_text = text.strip_prefix('.').unwrap_or(text);
            if is_method_query_clause(method_text) {
                if let Some((method, args)) = parse_fluent_call(method_text, clause_line.line)? {
                    query = apply_method_query_clause(query, method, args, clause_line.line)?;
                    self.pos += 1;
                    continue;
                }
            }

            if let Some(rest) = text.strip_prefix("where ") {
                let Some((field, value)) = rest.split_once('=') else {
                    return Err(AxBackendParseError::InvalidQueryClause {
                        line: clause_line.line,
                    });
                };

                let field = field.trim();
                let value = value.trim();
                if field.is_empty() || value.is_empty() {
                    return Err(AxBackendParseError::InvalidQueryClause {
                        line: clause_line.line,
                    });
                }

                query = query.filter(AxQueryFilter::new(
                    field,
                    AxQueryFilterOp::Eq,
                    parse_expr(value, clause_line.line)?,
                ));
                self.pos += 1;
                continue;
            }

            if let Some(rest) = text.strip_prefix("order ") {
                let mut parts = rest.split_whitespace();
                let field = parts.next().unwrap_or_default();
                if field.is_empty() {
                    return Err(AxBackendParseError::InvalidQueryClause {
                        line: clause_line.line,
                    });
                }

                let direction = match parts.next() {
                    Some(value) if value.eq_ignore_ascii_case("desc") => {
                        AxQueryOrderDirection::Desc
                    }
                    Some(value) if value.eq_ignore_ascii_case("asc") => AxQueryOrderDirection::Asc,
                    None => AxQueryOrderDirection::Asc,
                    Some(_) => {
                        return Err(AxBackendParseError::InvalidQueryClause {
                            line: clause_line.line,
                        })
                    }
                };

                query = query.order(AxQueryOrder::new(field, direction));
                self.pos += 1;
                continue;
            }

            if let Some(rest) = text.strip_prefix("limit ") {
                let value = rest.trim().parse::<u32>().map_err(|_| {
                    AxBackendParseError::InvalidQueryNumber {
                        line: clause_line.line,
                    }
                })?;
                query = query.limit(value);
                self.pos += 1;
                continue;
            }

            if let Some(rest) = text.strip_prefix("offset ") {
                let value = rest.trim().parse::<u32>().map_err(|_| {
                    AxBackendParseError::InvalidQueryNumber {
                        line: clause_line.line,
                    }
                })?;
                query = query.offset(value);
                self.pos += 1;
                continue;
            }

            return Err(AxBackendParseError::InvalidQueryClause {
                line: clause_line.line,
            });
        }

        Ok(query)
    }
}

fn preprocess(input: &str) -> Result<Vec<BackendLine>, AxBackendParseError> {
    let mut lines = Vec::new();

    for (index, raw) in input.lines().enumerate() {
        let line_no = index + 1;
        if raw.trim().is_empty() {
            continue;
        }

        if raw.contains('\t') {
            return Err(AxBackendParseError::TabsNotSupported { line: line_no });
        }

        let indent = raw.chars().take_while(|c| *c == ' ').count();
        if indent % 2 != 0 {
            return Err(AxBackendParseError::InvalidIndentation { line: line_no });
        }

        lines.push(BackendLine {
            line: line_no,
            indent,
            text: raw.trim().to_string(),
        });
    }

    Ok(lines)
}

fn is_query_clause(text: &str) -> bool {
    let text = text.strip_prefix('.').unwrap_or(text);
    text.starts_with("where ")
        || text.starts_with("order ")
        || text.starts_with("limit ")
        || text.starts_with("offset ")
        || text.starts_with("where(")
        || text.starts_with("whereNot(")
        || text.starts_with("whereIn(")
        || text.starts_with("whereNotIn(")
        || text.starts_with("whereNull(")
        || text.starts_with("whereNotNull(")
        || text.starts_with("order(")
        || text.starts_with("limit(")
        || text.starts_with("offset(")
}

fn query_source_from_expr(expr: AxExpr, line: usize) -> Result<AxQuerySource, AxBackendParseError> {
    match expr {
        AxExpr::Call { path, args } if path == vec!["db".to_string(), "query".to_string()] => {
            let Some(AxExpr::String(sql)) = args.first() else {
                return Err(AxBackendParseError::InvalidQuerySource { line });
            };
            Ok(AxQuerySource::RawSql {
                sql: sql.clone(),
                params: args.into_iter().skip(1).collect(),
            })
        }
        AxExpr::Call { path, args }
            if path.len() == 3 && path[0] == "db" && path[2] == "all" && args.is_empty() =>
        {
            Ok(AxQuerySource::Stream {
                collection: path[1].clone(),
            })
        }
        AxExpr::Call { path, args }
            if path == vec!["Content".to_string(), "Collection".to_string()] && args.len() == 1 =>
        {
            match &args[0] {
                AxExpr::String(collection) => Ok(AxQuerySource::ContentCollection {
                    collection: collection.clone(),
                }),
                _ => Err(AxBackendParseError::InvalidQuerySource { line }),
            }
        }
        _ => Err(AxBackendParseError::InvalidQuerySource { line }),
    }
}

fn query_spec_from_expr(expr: AxExpr, line: usize) -> Result<AxQuerySpec, AxBackendParseError> {
    match expr {
        AxExpr::Call { path, args }
            if path.len() == 3
                && path[0] == "db"
                && matches!(path[2].as_str(), "first" | "one")
                && args.is_empty() =>
        {
            Ok(AxQuerySpec::new(AxQuerySource::Stream {
                collection: path[1].clone(),
            })
            .first())
        }
        other => query_source_from_expr(other, line).map(AxQuerySpec::new),
    }
}

fn parse_fluent_query_spec(
    input: &str,
    line: usize,
) -> Result<Option<AxQuerySpec>, AxBackendParseError> {
    let segments = split_top_level_query_segments(input);
    if segments.len() < 4 {
        return Ok(None);
    }
    if segments[0] != "db" {
        return Ok(None);
    };
    let collection = segments[1];
    if collection.is_empty() {
        return Ok(None);
    }

    let mut query = AxQuerySpec::new(AxQuerySource::Stream {
        collection: collection.to_string(),
    });
    let mut terminal = false;

    for segment in segments.into_iter().skip(2) {
        let Some((method, args)) = parse_fluent_call(segment, line)? else {
            return Ok(None);
        };

        match method {
            "where" | "whereNot" | "whereIn" | "whereNotIn" => {
                let op = match method {
                    "where" => AxQueryFilterOp::Eq,
                    "whereNot" => AxQueryFilterOp::Ne,
                    "whereIn" => AxQueryFilterOp::In,
                    "whereNotIn" => AxQueryFilterOp::NotIn,
                    _ => unreachable!("method matched above"),
                };
                for (field, value) in parse_object_fields(args, line)? {
                    query = query.filter(AxQueryFilter::new(field, op, parse_expr(value, line)?));
                }
            }
            "whereNull" | "whereNotNull" => {
                let op = match method {
                    "whereNull" => AxQueryFilterOp::IsNull,
                    "whereNotNull" => AxQueryFilterOp::IsNotNull,
                    _ => unreachable!("method matched above"),
                };
                query = query.filter(AxQueryFilter::new(
                    parse_string_arg(args, line)?,
                    op,
                    AxExpr::bool(true),
                ));
            }
            "order" => {
                for (field, value) in parse_object_fields(args, line)? {
                    let direction = match parse_expr(value, line)? {
                        AxExpr::String(value) if value.eq_ignore_ascii_case("desc") => {
                            AxQueryOrderDirection::Desc
                        }
                        AxExpr::String(value) if value.eq_ignore_ascii_case("asc") => {
                            AxQueryOrderDirection::Asc
                        }
                        _ => return Err(AxBackendParseError::InvalidQueryClause { line }),
                    };
                    query = query.order(AxQueryOrder::new(field, direction));
                }
            }
            "limit" => {
                let value = parse_u32_arg(args, line)?;
                query = query.limit(value);
            }
            "offset" => {
                let value = parse_u32_arg(args, line)?;
                query = query.offset(value);
            }
            "all" if args.trim().is_empty() => {
                terminal = true;
            }
            "first" | "one" if args.trim().is_empty() => {
                query = query.first();
                terminal = true;
            }
            _ => return Err(AxBackendParseError::InvalidQuerySource { line }),
        }
    }

    if terminal {
        Ok(Some(query))
    } else {
        Ok(None)
    }
}

fn apply_method_query_clause(
    mut query: AxQuerySpec,
    method: &str,
    args: &str,
    line: usize,
) -> Result<AxQuerySpec, AxBackendParseError> {
    match method {
        "where" | "whereNot" | "whereIn" | "whereNotIn" => {
            let op = match method {
                "where" => AxQueryFilterOp::Eq,
                "whereNot" => AxQueryFilterOp::Ne,
                "whereIn" => AxQueryFilterOp::In,
                "whereNotIn" => AxQueryFilterOp::NotIn,
                _ => unreachable!("method matched above"),
            };
            for (field, value) in parse_object_fields(args, line)? {
                query = query.filter(AxQueryFilter::new(field, op, parse_expr(value, line)?));
            }
            Ok(query)
        }
        "whereNull" | "whereNotNull" => {
            let op = match method {
                "whereNull" => AxQueryFilterOp::IsNull,
                "whereNotNull" => AxQueryFilterOp::IsNotNull,
                _ => unreachable!("method matched above"),
            };
            Ok(query.filter(AxQueryFilter::new(
                parse_string_arg(args, line)?,
                op,
                AxExpr::bool(true),
            )))
        }
        "order" => {
            for (field, value) in parse_object_fields(args, line)? {
                let direction = match parse_expr(value, line)? {
                    AxExpr::String(value) if value.eq_ignore_ascii_case("desc") => {
                        AxQueryOrderDirection::Desc
                    }
                    AxExpr::String(value) if value.eq_ignore_ascii_case("asc") => {
                        AxQueryOrderDirection::Asc
                    }
                    _ => return Err(AxBackendParseError::InvalidQueryClause { line }),
                };
                query = query.order(AxQueryOrder::new(field, direction));
            }
            Ok(query)
        }
        "limit" => Ok(query.limit(parse_u32_arg(args, line)?)),
        "offset" => Ok(query.offset(parse_u32_arg(args, line)?)),
        _ => Err(AxBackendParseError::InvalidQueryClause { line }),
    }
}

fn is_method_query_clause(text: &str) -> bool {
    let Some(open_index) = find_call_open(text) else {
        return false;
    };
    matches!(
        text[..open_index].trim(),
        "where"
            | "whereNot"
            | "whereIn"
            | "whereNotIn"
            | "whereNull"
            | "whereNotNull"
            | "order"
            | "limit"
            | "offset"
    )
}

fn parse_fluent_mutation_stmt(
    input: &str,
    line: usize,
) -> Result<Option<AxBackendStmt>, AxBackendParseError> {
    let segments = split_top_level_query_segments(input);
    if segments.len() < 3 || segments[0] != "db" {
        return Ok(None);
    }

    let collection = segments[1];
    if collection.is_empty() {
        return Ok(None);
    }

    let mut filters = Vec::new();
    for segment in segments.into_iter().skip(2) {
        let Some((method, args)) = parse_fluent_call(segment, line)? else {
            return Ok(None);
        };

        match method {
            "where" | "whereNot" | "whereIn" | "whereNotIn" => {
                let op = match method {
                    "where" => AxQueryFilterOp::Eq,
                    "whereNot" => AxQueryFilterOp::Ne,
                    "whereIn" => AxQueryFilterOp::In,
                    "whereNotIn" => AxQueryFilterOp::NotIn,
                    _ => unreachable!("method matched above"),
                };
                for (field, value) in parse_object_fields(args, line)? {
                    filters.push(AxQueryFilter::new(field, op, parse_expr(value, line)?));
                }
            }
            "whereNull" | "whereNotNull" => {
                let op = match method {
                    "whereNull" => AxQueryFilterOp::IsNull,
                    "whereNotNull" => AxQueryFilterOp::IsNotNull,
                    _ => unreachable!("method matched above"),
                };
                filters.push(AxQueryFilter::new(
                    parse_string_arg(args, line)?,
                    op,
                    AxExpr::bool(true),
                ));
            }
            "insert" => {
                if !filters.is_empty() {
                    return Err(AxBackendParseError::InvalidMutation { line });
                }
                return Ok(Some(AxBackendStmt::Insert(AxMutation::new(
                    collection,
                    parse_object_assignments(args, line)?,
                ))));
            }
            "update" => {
                let mut mutation =
                    AxMutation::new(collection, parse_object_assignments(args, line)?);
                for filter in filters {
                    mutation = mutation.filter(filter);
                }
                return Ok(Some(AxBackendStmt::Update(mutation)));
            }
            "delete" if args.trim().is_empty() => {
                let mut mutation = AxMutation::new(collection, []);
                for filter in filters {
                    mutation = mutation.filter(filter);
                }
                return Ok(Some(AxBackendStmt::Delete(mutation)));
            }
            _ => return Err(AxBackendParseError::InvalidMutation { line }),
        }
    }

    Ok(None)
}

fn is_data_binding_statement(input: &str) -> bool {
    split_data_binding_prefix(input).is_some()
}

fn split_data_binding_prefix(input: &str) -> Option<(&'static str, &str)> {
    for prefix in ["data", "const", "let"] {
        if let Some(rest) = input.strip_prefix(prefix) {
            if rest.starts_with(char::is_whitespace) {
                return Some((prefix, rest.trim_start()));
            }
        }
    }

    None
}

fn parse_function_call_stmt(
    input: &str,
    line: usize,
) -> Result<Option<AxBackendStmt>, AxBackendParseError> {
    let Some(open_index) = find_call_open(input) else {
        return Ok(None);
    };
    if !input.ends_with(')') {
        return Ok(None);
    }

    let method = input[..open_index].trim();
    if method.is_empty()
        || method.contains('.')
        || method.chars().any(char::is_whitespace)
        || !is_valid_object_key(method)
    {
        return Ok(None);
    }

    let args = split_call_args(&input[open_index + 1..input.len() - 1], line)?;
    match method {
        "revalidate" if args.len() == 1 => Ok(Some(AxBackendStmt::revalidate(args[0].clone()))),
        "invalidate" if args.len() == 1 => Ok(Some(AxBackendStmt::invalidate(args[0].clone()))),
        "before" if args.len() == 1 => Ok(Some(AxBackendStmt::before(args[0].clone()))),
        "after" if args.len() == 1 => Ok(Some(AxBackendStmt::after(args[0].clone()))),
        "clearCookie" if args.len() == 1 => Ok(Some(AxBackendStmt::clear_cookie(args[0].clone()))),
        "revalidate" | "invalidate" | "before" | "after" | "clearCookie" => {
            Err(AxBackendParseError::InvalidBlock { line })
        }
        _ => Ok(None),
    }
}

fn split_top_level_query_segments(input: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    let mut in_string: Option<char> = None;

    for (index, ch) in input.char_indices() {
        match in_string {
            Some(quote) => {
                if ch == quote {
                    in_string = None;
                }
            }
            None => match ch {
                '"' | '\'' => in_string = Some(ch),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                '.' if depth == 0 => {
                    result.push(input[start..index].trim());
                    start = index + ch.len_utf8();
                }
                _ => {}
            },
        }
    }

    result.push(input[start..].trim());
    result
}

fn parse_fluent_call<'a>(
    input: &'a str,
    line: usize,
) -> Result<Option<(&'a str, &'a str)>, AxBackendParseError> {
    let Some(open_index) = find_call_open(input) else {
        return Ok(None);
    };
    if !input.ends_with(')') {
        return Err(AxBackendParseError::InvalidQuerySource { line });
    }

    let method = input[..open_index].trim();
    if method.is_empty() || method.contains('.') {
        return Err(AxBackendParseError::InvalidQuerySource { line });
    }

    Ok(Some((
        method,
        input[open_index + 1..input.len() - 1].trim(),
    )))
}

fn parse_object_fields<'a>(
    input: &'a str,
    line: usize,
) -> Result<Vec<(String, &'a str)>, AxBackendParseError> {
    let input = input.trim();
    let Some(inner) = input
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
    else {
        return Err(AxBackendParseError::InvalidQueryClause { line });
    };

    let mut fields = Vec::new();
    for part in split_top_level(inner, ',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        match split_top_level_once(part, ":") {
            Some((field, value)) => {
                let field = parse_object_key(field.trim(), line)?;
                let value = value.trim();
                if value.is_empty() {
                    return Err(AxBackendParseError::InvalidQueryClause { line });
                }
                fields.push((field, value));
            }
            None => {
                if part.contains(' ') {
                    return Err(AxBackendParseError::InvalidQueryClause { line });
                }
                if is_quoted_object_key(part) {
                    return Err(AxBackendParseError::InvalidQueryClause { line });
                }
                fields.push((parse_object_key(part, line)?, part));
            }
        }
    }

    if fields.is_empty() {
        return Err(AxBackendParseError::InvalidQueryClause { line });
    }

    Ok(fields)
}

fn parse_object_assignments(
    input: &str,
    line: usize,
) -> Result<Vec<AxAssignment>, AxBackendParseError> {
    parse_object_fields(input, line).map(|fields| {
        fields
            .into_iter()
            .map(|(field, value)| {
                parse_expr(value, line).map(|value| AxAssignment::new(field, value))
            })
            .collect::<Result<Vec<_>, _>>()
    })?
}

fn split_call_args(input: &str, line: usize) -> Result<Vec<AxExpr>, AxBackendParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(Vec::new());
    }

    split_top_level(input, ',')
        .into_iter()
        .map(|part| parse_expr(part.trim(), line))
        .collect()
}

fn parse_object_key(input: &str, line: usize) -> Result<String, AxBackendParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AxBackendParseError::InvalidQueryClause { line });
    }

    if let Some(value) = parse_quoted_object_key(input) {
        if is_valid_object_key(value) {
            return Ok(value.to_string());
        }
        return Err(AxBackendParseError::InvalidQueryClause { line });
    }

    if is_valid_object_key(input) {
        return Ok(input.to_string());
    }

    Err(AxBackendParseError::InvalidQueryClause { line })
}

fn parse_quoted_object_key(input: &str) -> Option<&str> {
    if !is_quoted_object_key(input) {
        return None;
    }

    Some(&input[1..input.len() - 1])
}

fn is_quoted_object_key(input: &str) -> bool {
    if input.len() < 2 {
        return false;
    }
    let quote = input.as_bytes()[0] as char;
    if quote != '"' && quote != '\'' {
        return false;
    }

    input.ends_with(quote)
}

fn is_valid_object_key(input: &str) -> bool {
    let mut chars = input.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn parse_u32_arg(input: &str, line: usize) -> Result<u32, AxBackendParseError> {
    let parts = split_top_level(input, ',');
    if parts.len() != 1 {
        return Err(AxBackendParseError::InvalidQueryNumber { line });
    }
    parts[0]
        .trim()
        .parse::<u32>()
        .map_err(|_| AxBackendParseError::InvalidQueryNumber { line })
}

fn parse_string_arg(input: &str, line: usize) -> Result<String, AxBackendParseError> {
    let parts = split_top_level(input, ',');
    if parts.len() != 1 {
        return Err(AxBackendParseError::InvalidQueryClause { line });
    }
    match parse_expr(parts[0].trim(), line)? {
        AxExpr::String(value) if is_valid_object_key(&value) => Ok(value),
        _ => Err(AxBackendParseError::InvalidQueryClause { line }),
    }
}

fn parse_expr(input: &str, line: usize) -> Result<AxExpr, AxBackendParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AxBackendParseError::InvalidExpression {
            line,
            message: "expression is empty".to_string(),
        });
    }

    if (input.starts_with('"') && input.ends_with('"'))
        || (input.starts_with('\'') && input.ends_with('\''))
    {
        return Ok(AxExpr::string(input[1..input.len() - 1].to_string()));
    }

    if input == "true" {
        return Ok(AxExpr::bool(true));
    }
    if input == "false" {
        return Ok(AxExpr::bool(false));
    }

    if let Ok(value) = input.parse::<i64>() {
        return Ok(AxExpr::number(value));
    }
    if input.contains('.') {
        if let Some(value) = input.parse::<f64>().ok().and_then(AxFloat::new) {
            return Ok(AxExpr::Float(value));
        }
    }

    if input.starts_with('[') && input.ends_with(']') {
        let items = &input[1..input.len() - 1];
        let args = if items.trim().is_empty() {
            Vec::new()
        } else {
            split_top_level(items, ',')
                .into_iter()
                .map(|part| parse_expr(part.trim(), line))
                .collect::<Result<Vec<_>, _>>()?
        };

        return Ok(AxExpr::call(["list"], args));
    }

    if input.ends_with(')') {
        if let Some(open_index) = find_call_open(input) {
            let path = input[..open_index].trim();
            let args = &input[open_index + 1..input.len() - 1];
            let path: Vec<String> = path
                .split('.')
                .map(str::trim)
                .filter(|segment| !segment.is_empty())
                .map(ToOwned::to_owned)
                .collect();

            if path.is_empty() {
                return Err(AxBackendParseError::InvalidExpression {
                    line,
                    message: format!("invalid call path `{input}`"),
                });
            }

            let args = if args.trim().is_empty() {
                Vec::new()
            } else {
                split_top_level(args, ',')
                    .into_iter()
                    .map(|part| parse_expr(part.trim(), line))
                    .collect::<Result<Vec<_>, _>>()?
            };

            return Ok(AxExpr::Call { path, args });
        }
    }

    if input.contains('.') {
        let mut parts = input.split('.').map(str::trim);
        let first = parts.next().unwrap_or_default();
        if first.is_empty() {
            return Err(AxBackendParseError::InvalidExpression {
                line,
                message: format!("invalid member expression `{input}`"),
            });
        }

        let mut expr = AxExpr::ident(first);
        for property in parts {
            if property.is_empty() {
                return Err(AxBackendParseError::InvalidExpression {
                    line,
                    message: format!("invalid member expression `{input}`"),
                });
            }
            expr = expr.member(property);
        }
        return Ok(expr);
    }

    Ok(AxExpr::ident(input))
}

fn parse_requirement_expr(input: &str, line: usize) -> Result<AxExpr, AxBackendParseError> {
    if let Some((value, options)) = split_top_level_once(input, " in ") {
        let value = value.trim();
        let options = options.trim();
        if value.is_empty() || options.is_empty() {
            return Err(AxBackendParseError::InvalidRequirement { line });
        }

        return Ok(AxExpr::call(
            ["contains"],
            [parse_expr(options, line)?, parse_expr(value, line)?],
        ));
    }

    parse_expr(input, line)
}

fn parse_requirement_fallback_expr(
    input: &str,
    line: usize,
) -> Result<AxExpr, AxBackendParseError> {
    let input = input.trim();
    if let Some(message) = input.strip_prefix("error ") {
        let message = message.trim();
        if message.is_empty() {
            return Err(AxBackendParseError::InvalidRequirement { line });
        }
        return Ok(AxExpr::call(["error"], [parse_expr(message, line)?]));
    }

    parse_expr(input, line)
}

fn parse_guard_call(input: &str, line: usize) -> Result<(AxExpr, AxExpr), AxBackendParseError> {
    let Some(inner) = input
        .strip_prefix("guard(")
        .and_then(|value| value.strip_suffix(')'))
    else {
        return Err(AxBackendParseError::InvalidRequirement { line });
    };
    let args = split_top_level(inner, ',');
    let [condition, message] = args.as_slice() else {
        return Err(AxBackendParseError::InvalidRequirement { line });
    };
    if condition.trim().is_empty() || message.trim().is_empty() {
        return Err(AxBackendParseError::InvalidRequirement { line });
    }

    Ok((
        parse_requirement_expr(condition.trim(), line)?,
        parse_expr(message.trim(), line)?,
    ))
}

fn parse_env_type(
    input: &str,
    line: usize,
) -> Result<(AxBackendEnvVisibility, String), AxBackendParseError> {
    let input = input.trim();
    let Some((visibility, inner)) = input.split_once('<') else {
        return Err(AxBackendParseError::InvalidEnvDeclaration { line });
    };
    let Some(inner) = inner.strip_suffix('>') else {
        return Err(AxBackendParseError::InvalidEnvDeclaration { line });
    };

    let visibility = match visibility.trim() {
        "Public" => AxBackendEnvVisibility::Public,
        "Secret" => AxBackendEnvVisibility::Secret,
        _ => return Err(AxBackendParseError::InvalidEnvDeclaration { line }),
    };
    let inner = inner.trim();
    if inner.is_empty() {
        return Err(AxBackendParseError::InvalidEnvDeclaration { line });
    }

    Ok((visibility, inner.to_string()))
}

fn find_call_open(input: &str) -> Option<usize> {
    let mut in_string: Option<char> = None;

    for (index, ch) in input.char_indices() {
        match in_string {
            Some(quote) => {
                if ch == quote {
                    in_string = None;
                }
            }
            None => match ch {
                '"' | '\'' => in_string = Some(ch),
                '(' => return Some(index),
                _ => {}
            },
        }
    }

    None
}

fn split_top_level(input: &str, delimiter: char) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    let mut in_string: Option<char> = None;

    for (index, ch) in input.char_indices() {
        match in_string {
            Some(quote) => {
                if ch == quote {
                    in_string = None;
                }
            }
            None => match ch {
                '"' | '\'' => in_string = Some(ch),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                _ if ch == delimiter && depth == 0 => {
                    result.push(input[start..index].trim());
                    start = index + ch.len_utf8();
                }
                _ => {}
            },
        }
    }

    result.push(input[start..].trim());
    result
}

fn split_top_level_commas(input: &str) -> Vec<&str> {
    split_top_level(input, ',')
}

fn is_backend_identifier(input: &str) -> bool {
    let mut chars = input.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn parse_backend_literal_union(source: &str) -> Option<Vec<String>> {
    let mut seen = std::collections::BTreeSet::new();
    let literals = source
        .split('|')
        .map(str::trim)
        .map(|part| serde_json::from_str::<String>(part).ok())
        .collect::<Option<Vec<_>>>()?;
    if literals.len() < 2 || literals.iter().any(|literal| !seen.insert(literal.clone())) {
        return None;
    }
    Some(literals)
}

fn split_top_level_signature_params(input: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0usize;
    let mut group_depth = 0usize;
    let mut angle_depth = 0usize;
    let mut in_default = false;
    let mut in_string: Option<char> = None;

    for (index, ch) in input.char_indices() {
        match in_string {
            Some(quote) => {
                if ch == quote {
                    in_string = None;
                }
            }
            None => match ch {
                '"' | '\'' => in_string = Some(ch),
                '(' | '[' | '{' => group_depth += 1,
                ')' | ']' | '}' => group_depth = group_depth.saturating_sub(1),
                '=' if group_depth == 0 && angle_depth == 0 => in_default = true,
                '<' if group_depth == 0 && !in_default => angle_depth += 1,
                '>' if group_depth == 0 && !in_default => {
                    angle_depth = angle_depth.saturating_sub(1)
                }
                ',' if group_depth == 0 && angle_depth == 0 => {
                    result.push(input[start..index].trim());
                    start = index + ch.len_utf8();
                    in_default = false;
                }
                _ => {}
            },
        }
    }

    result.push(input[start..].trim());
    result
}

fn split_top_level_once<'a>(input: &'a str, delimiter: &str) -> Option<(&'a str, &'a str)> {
    let mut depth = 0usize;
    let mut in_string: Option<char> = None;

    for (index, ch) in input.char_indices() {
        match in_string {
            Some(quote) => {
                if ch == quote {
                    in_string = None;
                }
            }
            None => match ch {
                '"' | '\'' => in_string = Some(ch),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                _ if depth == 0 && input[index..].starts_with(delimiter) => {
                    return Some((&input[..index], &input[index + delimiter.len()..]));
                }
                _ => {}
            },
        }
    }

    None
}

fn trim_quotes(input: &str) -> String {
    input
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

fn split_return_contract(input: &str) -> (&str, Option<String>) {
    match split_top_level_once(input, "->") {
        Some((head, returns)) => {
            let returns = returns.trim();
            if returns.is_empty() {
                (head.trim(), None)
            } else {
                (head.trim(), Some(returns.to_string()))
            }
        }
        None => (input.trim(), None),
    }
}

fn split_block_brace(input: &str) -> (&str, bool) {
    let input = input.trim_end();
    if let Some(header) = input.strip_suffix('{') {
        (header.trim_end(), true)
    } else {
        (input, false)
    }
}

fn parse_scope_header(
    input: &str,
    line: usize,
) -> Result<(String, Vec<String>), AxBackendParseError> {
    let input = input.trim();
    let Some(open) = input.find('<') else {
        if is_backend_identifier(input) {
            return Ok((input.to_string(), Vec::new()));
        }
        return Err(AxBackendParseError::InvalidScope { line });
    };
    if !input.ends_with('>') {
        return Err(AxBackendParseError::InvalidScope { line });
    }

    let name = input[..open].trim();
    if !is_backend_identifier(name) {
        return Err(AxBackendParseError::InvalidScope { line });
    }

    let members = input[open + 1..input.len() - 1].trim();
    if members.is_empty() {
        return Err(AxBackendParseError::InvalidScopeMember { line });
    }

    let members = split_top_level_commas(members)
        .into_iter()
        .map(|member| {
            let member = member.trim();
            if !is_backend_identifier(member) {
                return Err(AxBackendParseError::InvalidScopeMember { line });
            }
            Ok(member.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok((name.to_string(), members))
}

fn parse_backend_callable_signature(
    input: &str,
    line: usize,
) -> Result<(String, Vec<AxField>), AxBackendParseError> {
    let input = input.trim();
    let Some(open) = input.find('(') else {
        return Ok((input.to_string(), Vec::new()));
    };
    if !input.ends_with(')') {
        return Err(AxBackendParseError::InvalidBlock { line });
    }

    let name = input[..open].trim();
    let params = input[open + 1..input.len() - 1].trim();
    if name.is_empty() {
        return Err(AxBackendParseError::InvalidBlock { line });
    }
    if params.is_empty() {
        return Ok((name.to_string(), Vec::new()));
    }

    let fields = split_top_level_signature_params(params)
        .into_iter()
        .map(|param| parse_backend_signature_param(param, line))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((name.to_string(), fields))
}

fn parse_backend_signature_param(input: &str, line: usize) -> Result<AxField, AxBackendParseError> {
    let input = input.trim();
    let Some((name, ty)) = split_top_level_once(input, ":") else {
        return Err(AxBackendParseError::InvalidField { line });
    };

    let raw_name = name.trim();
    let optional = raw_name.ends_with('?');
    let name = raw_name.trim_end_matches('?').trim();
    if name.is_empty() {
        return Err(AxBackendParseError::InvalidField { line });
    }

    let (ty, default) = match split_top_level_once(ty, "=") {
        Some((ty, default)) => {
            let ty = ty.trim();
            let default = default.trim();
            if ty.is_empty() || default.is_empty() {
                return Err(AxBackendParseError::InvalidField { line });
            }
            (ty, Some(parse_expr(default, line)?))
        }
        None => {
            let ty = ty.trim();
            if ty.is_empty() {
                return Err(AxBackendParseError::InvalidField { line });
            }
            (ty, None)
        }
    };

    Ok(match (optional, default) {
        (true, Some(default)) => AxField::optional_with_default(name, ty, default),
        (true, None) => AxField::optional(name, ty),
        (false, Some(default)) => AxField::with_default(name, ty, default),
        (false, None) => AxField::new(name, ty),
    })
}

pub mod prelude {
    pub use super::parse_backend_ax;
    pub use super::AxBackendParseError;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_exported_backend_type_contracts() {
        let document = parse_backend_ax(
            r#"
export type Post {
  title: String
  slug: String
  summary?: String
  tags: List<String>
}

loader PostsList() -> Post[] {
  return []
}
"#,
        )
        .expect("backend source should parse");

        assert_eq!(document.types.len(), 1);
        let contract = &document.types[0];
        assert_eq!(contract.name, "Post");
        assert!(contract.exported);
        assert_eq!(
            contract.fields,
            vec![
                AxBackendTypeField::new("title", "String"),
                AxBackendTypeField::new("slug", "String"),
                AxBackendTypeField::new("summary", "Optional<String>"),
                AxBackendTypeField::new("tags", "List<String>"),
            ]
        );
    }

    #[test]
    fn parses_exported_literal_union_contract() {
        let document = parse_backend_ax(
            r#"export type Theme = "silver" | "bronze" | "gold"

query loadTheme() -> Theme {
  return "silver"
}
"#,
        )
        .expect("backend literal union should parse");

        assert_eq!(
            document.types,
            vec![AxBackendTypeDecl::literal_union(
                "Theme",
                ["silver", "bronze", "gold"],
                true
            )]
        );
    }

    #[test]
    fn parses_backend_imports_before_blocks() {
        let input = r#"
import { normalizeStatus, slugify as makeSlug } from "./domain.ax"

export query loadPosts(status?: String) -> Post[] {
  data posts = db.posts.all()
  return posts
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        assert_eq!(document.imports.len(), 1);
        assert_eq!(document.imports[0].source, "./domain.ax");
        assert_eq!(document.imports[0].bindings.len(), 2);
        assert_eq!(document.imports[0].bindings[0].imported, "normalizeStatus");
        assert_eq!(document.imports[0].bindings[0].local, "normalizeStatus");
        assert_eq!(document.imports[0].bindings[1].imported, "slugify");
        assert_eq!(document.imports[0].bindings[1].local, "makeSlug");
        assert_eq!(document.blocks.len(), 1);
        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected query loader block");
        };
        assert!(loader.exported);
    }

    #[test]
    fn parses_backend_namespace_imports_before_blocks() {
        let input = r#"
import * as Domain from "./domain.ax"

scope Blog <Domain> {
  render BlogPage()
}
"#;

        let document = parse_backend_ax(input).expect("namespace import should parse");

        assert_eq!(document.imports.len(), 1);
        assert_eq!(document.imports[0].source, "./domain.ax");
        assert_eq!(document.imports[0].bindings.len(), 1);
        assert!(document.imports[0].bindings[0].is_namespace());
        assert_eq!(document.imports[0].bindings[0].local, "Domain");
    }

    #[test]
    fn rejects_empty_backend_import_list() {
        let input = r#"
import { } from "./domain.ax"

query loadPosts() -> Post[] {
  return posts
}
"#;

        let error = parse_backend_ax(input).expect_err("empty import list should fail");

        assert_eq!(error, AxBackendParseError::EmptyImportList { line: 2 });
    }

    #[test]
    fn rejects_invalid_backend_import_binding() {
        let input = r#"
import { 123bad } from "./domain.ax"

query loadPosts() -> Post[] {
  return posts
}
"#;

        let error = parse_backend_ax(input).expect_err("invalid import should fail");

        assert_eq!(error, AxBackendParseError::InvalidImport { line: 2 });
    }

    #[test]
    fn parses_scope_block_as_ast_only_metadata() {
        let input = r#"
import { RenderLayout } from "./page.ax"
import { setTheme } from "./actions.ax"
import { isTheme } from "./domain.ax"

scope Layout <RenderLayout, setTheme, isTheme> {
  state theme: String = "silver"
  render RenderLayout()
}
"#;

        let document = parse_backend_ax(input).expect("scope document should parse");

        assert_eq!(document.imports.len(), 3);
        assert_eq!(document.blocks.len(), 1);
        let AxBackendBlock::Scope(scope) = &document.blocks[0] else {
            panic!("expected scope block");
        };

        assert_eq!(scope.name, "Layout");
        assert_eq!(scope.members, vec!["RenderLayout", "setTheme", "isTheme"]);
        assert_eq!(scope.body.len(), 2);

        let AxScopeStmt::State(state) = &scope.body[0] else {
            panic!("expected scope state");
        };
        assert_eq!(state.name, "theme");
        assert_eq!(state.ty, "String");
        assert_eq!(state.default, Some(AxExpr::string("silver")));

        let AxScopeStmt::Render(render) = &scope.body[1] else {
            panic!("expected scope render");
        };
        assert_eq!(render.call, AxExpr::call(["RenderLayout"], []));
    }

    #[test]
    fn parses_scope_without_members() {
        let input = r#"
scope Layout {
  render RenderLayout()
}
"#;

        let document = parse_backend_ax(input).expect("scope document should parse");

        let AxBackendBlock::Scope(scope) = &document.blocks[0] else {
            panic!("expected scope block");
        };
        assert_eq!(scope.name, "Layout");
        assert!(scope.members.is_empty());
    }

    #[test]
    fn rejects_invalid_scope_member() {
        let input = r#"
scope Layout <RenderLayout, 123bad> {
  render RenderLayout()
}
"#;

        let error = parse_backend_ax(input).expect_err("invalid scope member should fail");

        assert_eq!(error, AxBackendParseError::InvalidScopeMember { line: 2 });
    }

    #[test]
    fn rejects_invalid_scope_state() {
        let input = r#"
scope Layout {
  state theme String = "silver"
}
"#;

        let error = parse_backend_ax(input).expect_err("invalid scope state should fail");

        assert_eq!(error, AxBackendParseError::InvalidScopeState { line: 3 });
    }

    #[test]
    fn rejects_empty_scope_render() {
        let input = r#"
scope Layout {
  render
}
"#;

        let error = parse_backend_ax(input).expect_err("empty scope render should fail");

        assert_eq!(error, AxBackendParseError::InvalidScopeRender { line: 3 });
    }

    #[test]
    fn rejects_scope_render_without_call_expression() {
        let input = r#"
scope Layout {
  render RenderLayout
}
"#;

        let error = parse_backend_ax(input).expect_err("non-call scope render should fail");

        assert_eq!(error, AxBackendParseError::InvalidScopeRender { line: 3 });
    }

    #[test]
    fn rejects_scope_render_list_literal() {
        let input = r#"
scope Layout {
  render [RenderLayout]
}
"#;

        let error = parse_backend_ax(input).expect_err("list literal render should fail");

        assert_eq!(error, AxBackendParseError::InvalidScopeRender { line: 3 });
    }

    #[test]
    fn parses_loader_and_route_blocks() {
        let input = r#"
loader PostsList
  data posts = db.posts.all()
    where status = "published"
    order created_at desc
    limit 20
  return posts

route GET "/api/posts"
  data posts = db.posts.all()
  return posts
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        assert_eq!(document.blocks.len(), 2);
        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected loader block");
        };
        assert_eq!(loader.name, "PostsList");
        let AxBackendStmt::Data(posts) = &loader.body[0] else {
            panic!("expected data statement");
        };
        assert_eq!(
            posts.value,
            AxBackendValue::Query(
                AxQuerySpec::new(AxQuerySource::Stream {
                    collection: "posts".to_string(),
                })
                .filter(AxQueryFilter::new(
                    "status",
                    AxQueryFilterOp::Eq,
                    AxExpr::string("published"),
                ))
                .order(AxQueryOrder::new("created_at", AxQueryOrderDirection::Desc,))
                .limit(20)
            )
        );

        let AxBackendBlock::Route(route) = &document.blocks[1] else {
            panic!("expected route block");
        };
        assert_eq!(route.method, "GET");
        assert_eq!(route.path, "/api/posts");
    }

    #[test]
    fn parses_backend_return_contracts() {
        let input = r#"
loader PostsList -> Post[]
  return posts

route GET "/api/posts" -> Post[]
  return json(posts)

action CreatePost -> Post
  input:
    title: string

  return json(input.title)
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected loader block");
        };
        assert_eq!(loader.name, "PostsList");
        assert_eq!(loader.returns.as_deref(), Some("Post[]"));

        let AxBackendBlock::Route(route) = &document.blocks[1] else {
            panic!("expected route block");
        };
        assert_eq!(route.path, "/api/posts");
        assert_eq!(route.returns.as_deref(), Some("Post[]"));

        let AxBackendBlock::Action(action) = &document.blocks[2] else {
            panic!("expected action block");
        };
        assert_eq!(action.name, "CreatePost");
        assert_eq!(action.returns.as_deref(), Some("Post"));
    }

    #[test]
    fn parses_function_shaped_route_and_input_blocks() {
        let input = r#"
route POST "/api/posts" -> Post {
  input {
    title: String
    summary?: String = ""
    featured?: Bool = false
  }

  return json(input.title)
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Route(route) = &document.blocks[0] else {
            panic!("expected route block");
        };
        assert_eq!(route.method, "POST");
        assert_eq!(route.path, "/api/posts");
        assert_eq!(route.returns.as_deref(), Some("Post"));
        assert_eq!(route.input.len(), 3);
        assert_eq!(route.input[0].name, "title");
        assert_eq!(route.input[1].name, "summary");
        assert!(route.input[1].optional);
        assert_eq!(route.input[2].name, "featured");
        assert!(route.input[2].optional);
        assert_eq!(route.body.len(), 1);
    }

    #[test]
    fn parses_query_function_as_loader_block() {
        let input = r#"
query loadPosts() -> Post[]
  data posts = db.posts.all()
    where status = "published"
    order created_at desc
    limit 6
  return posts
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected query function to lower as loader block");
        };

        assert_eq!(loader.name, "loadPosts");
        assert_eq!(loader.returns.as_deref(), Some("Post[]"));
        assert!(loader.input.is_empty());
        let AxBackendStmt::Data(posts) = &loader.body[0] else {
            panic!("expected data statement");
        };
        assert_eq!(
            posts.value,
            AxBackendValue::Query(
                AxQuerySpec::new(AxQuerySource::Stream {
                    collection: "posts".to_string(),
                })
                .filter(AxQueryFilter::new(
                    "status",
                    AxQueryFilterOp::Eq,
                    AxExpr::string("published"),
                ))
                .order(AxQueryOrder::new("created_at", AxQueryOrderDirection::Desc,))
                .limit(6)
            )
        );
    }

    #[test]
    fn parses_braced_query_function_as_loader_block() {
        let input = r#"
query loadPosts() -> Post[] {
  data posts = db.posts.all()
    where status = "published"
    order created_at desc
    limit 6
  return posts
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected query function to lower as loader block");
        };

        assert_eq!(loader.name, "loadPosts");
        assert_eq!(loader.returns.as_deref(), Some("Post[]"));
        assert!(loader.input.is_empty());
        assert_eq!(loader.body.len(), 2);
    }

    #[test]
    fn parses_backend_type_contract_before_query_function() {
        let input = r#"
export type Post {
  title: String
  summary?: String
}

query loadPosts() -> Post[] {
  return posts
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        assert_eq!(document.blocks.len(), 1);
        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected query function to lower as loader block");
        };

        assert_eq!(loader.name, "loadPosts");
        assert_eq!(loader.returns.as_deref(), Some("Post[]"));
    }

    #[test]
    fn parses_exported_query_function_as_loader_block() {
        let input = r#"
export query loadPosts(status: String = "published") -> Post[] {
  data posts = db.posts.all()
    where status = input.status
  return posts
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected exported query function to lower as loader block");
        };

        assert_eq!(loader.name, "loadPosts");
        assert_eq!(loader.returns.as_deref(), Some("Post[]"));
        assert_eq!(loader.input.len(), 1);
        assert_eq!(loader.input[0].name, "status");
        assert_eq!(loader.input[0].ty, "String");
        assert_eq!(loader.input[0].default, Some(AxExpr::string("published")));
    }

    #[test]
    fn parses_exported_domain_function_block() {
        let input = r#"
export fn normalizeStatus(status?: String) -> String {
  return status ?? "published"
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Function(function) = &document.blocks[0] else {
            panic!("expected function block");
        };

        assert_eq!(function.name, "normalizeStatus");
        assert_eq!(function.returns.as_deref(), Some("String"));
        assert!(function.exported);
        assert_eq!(function.input.len(), 1);
        assert_eq!(function.input[0].name, "status");
        assert!(function.input[0].optional);
        assert_eq!(function.input[0].ty, "String");
        assert_eq!(function.body.len(), 1);
    }

    #[test]
    fn parses_query_function_signature_inputs() {
        let input = r#"
query loadPosts(status: String, limit: i64 = 6) -> Post[] {
  data posts = db.posts.all()
    where status = input.status
    limit 6
  return posts
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected query function to lower as loader block");
        };

        assert_eq!(loader.name, "loadPosts");
        assert_eq!(loader.input.len(), 2);
        assert_eq!(loader.input[0], AxField::new("status", "String"));
        assert_eq!(loader.input[1].name, "limit");
        assert_eq!(loader.input[1].ty, "i64");
        assert!(loader.input[1].default.is_some());
    }

    #[test]
    fn parses_db_all_binding_as_query_without_extra_clauses() {
        let input = r#"
loader PostsList
  data posts = db.posts.all()
  return posts
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected loader block");
        };
        let AxBackendStmt::Data(posts) = &loader.body[0] else {
            panic!("expected data statement");
        };

        assert_eq!(
            posts.value,
            AxBackendValue::Query(AxQuerySpec::new(AxQuerySource::Stream {
                collection: "posts".to_string(),
            }))
        );
    }

    #[test]
    fn parses_fluent_db_query_binding() {
        let input = r#"
query loadPosts() -> Post[]
  data posts = db.posts.where({ status: "published" }).order({ created_at: "desc" }).limit(6).all()
  return posts
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected query function to lower as loader block");
        };
        let AxBackendStmt::Data(posts) = &loader.body[0] else {
            panic!("expected data statement");
        };

        assert_eq!(
            posts.value,
            AxBackendValue::Query(
                AxQuerySpec::new(AxQuerySource::Stream {
                    collection: "posts".to_string(),
                })
                .filter(AxQueryFilter::new(
                    "status",
                    AxQueryFilterOp::Eq,
                    AxExpr::string("published"),
                ))
                .order(AxQueryOrder::new("created_at", AxQueryOrderDirection::Desc))
                .limit(6)
            )
        );
    }

    #[test]
    fn parses_fluent_db_first_query_binding() {
        let input = r#"
query loadPost(slug: String) -> Post?
  data post = db.posts.where({ slug: input.slug }).first()
  return post
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected query function to lower as loader block");
        };
        let AxBackendStmt::Data(post) = &loader.body[0] else {
            panic!("expected data statement");
        };

        assert_eq!(
            post.value,
            AxBackendValue::Query(
                AxQuerySpec::new(AxQuerySource::Stream {
                    collection: "posts".to_string(),
                })
                .filter(AxQueryFilter::new(
                    "slug",
                    AxQueryFilterOp::Eq,
                    AxExpr::ident("input").member("slug"),
                ))
                .first()
            )
        );
    }

    #[test]
    fn parses_const_and_let_backend_bindings_as_data_steps() {
        let input = r#"
export fn normalizeStatus(status?: String) -> String {
  let resolved = input.status ?? "published"
  return resolved
}

query loadPosts(status: String = "published") -> Post[] {
  const posts = db.posts.where({ status: input.status }).limit(6).all()
  return posts
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Function(function) = &document.blocks[0] else {
            panic!("expected function block");
        };
        let AxBackendStmt::Data(resolved) = &function.body[0] else {
            panic!("expected let to lower as data step");
        };
        assert_eq!(resolved.name, "resolved");

        let AxBackendBlock::Loader(loader) = &document.blocks[1] else {
            panic!("expected query function to lower as loader block");
        };
        let AxBackendStmt::Data(posts) = &loader.body[0] else {
            panic!("expected const to lower as data step");
        };
        assert_eq!(posts.name, "posts");
        assert_eq!(
            posts.value,
            AxBackendValue::Query(
                AxQuerySpec::new(AxQuerySource::Stream {
                    collection: "posts".to_string(),
                })
                .filter(AxQueryFilter::new(
                    "status",
                    AxQueryFilterOp::Eq,
                    AxExpr::ident("input").member("status"),
                ))
                .limit(6)
            )
        );
    }

    #[test]
    fn parses_direct_query_return_as_synthetic_data_step() {
        let input = r#"
query loadPosts(status: String = "published") -> Post[] {
  return db.posts.where({ status: input.status }).order({ created_at: "desc" }).limit(6).all()
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected query function to lower as loader block");
        };
        assert_eq!(loader.body.len(), 2);
        let AxBackendStmt::Data(posts) = &loader.body[0] else {
            panic!("expected synthetic query data step");
        };
        assert_eq!(posts.name, "__ax_return_1");
        assert_eq!(
            posts.value,
            AxBackendValue::Query(
                AxQuerySpec::new(AxQuerySource::Stream {
                    collection: "posts".to_string(),
                })
                .filter(AxQueryFilter::new(
                    "status",
                    AxQueryFilterOp::Eq,
                    AxExpr::ident("input").member("status"),
                ))
                .order(AxQueryOrder::new("created_at", AxQueryOrderDirection::Desc))
                .limit(6)
            )
        );
        assert_eq!(
            loader.body[1],
            AxBackendStmt::r#return(AxExpr::ident("__ax_return_1"))
        );
    }

    #[test]
    fn parses_direct_multiline_and_raw_query_returns() {
        let input = r#"
query loadPublishedPosts() -> Post[] {
  return db.posts.all()
    .where({ status: "published" })
    .limit(6)
}

query loadFeaturedPosts() -> Post[] {
  return db.query("select * from posts where featured = ?", true)
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(multiline) = &document.blocks[0] else {
            panic!("expected multiline query function");
        };
        let AxBackendStmt::Data(posts) = &multiline.body[0] else {
            panic!("expected synthetic query data step");
        };
        assert_eq!(posts.name, "__ax_return_1");
        assert!(matches!(posts.value, AxBackendValue::Query(_)));
        assert_eq!(
            multiline.body[1],
            AxBackendStmt::r#return(AxExpr::ident("__ax_return_1"))
        );

        let AxBackendBlock::Loader(raw) = &document.blocks[1] else {
            panic!("expected raw query function");
        };
        let AxBackendStmt::Data(raw_posts) = &raw.body[0] else {
            panic!("expected synthetic raw query data step");
        };
        assert_eq!(raw_posts.name, "__ax_return_2");
        assert_eq!(
            raw.body[1],
            AxBackendStmt::r#return(AxExpr::ident("__ax_return_2"))
        );
    }

    #[test]
    fn parses_direct_one_query_return_as_synthetic_data_step() {
        let input = r#"
query loadPost() -> Post? {
  return db.posts.one()
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected query function to lower as loader block");
        };
        let AxBackendStmt::Data(post) = &loader.body[0] else {
            panic!("expected synthetic query data step");
        };
        assert_eq!(post.name, "__ax_return_1");
        assert_eq!(
            post.value,
            AxBackendValue::Query(
                AxQuerySpec::new(AxQuerySource::Stream {
                    collection: "posts".to_string(),
                })
                .first()
            )
        );
        assert_eq!(
            loader.body[1],
            AxBackendStmt::r#return(AxExpr::ident("__ax_return_1"))
        );
    }

    #[test]
    fn parses_multiline_first_query_return_as_synthetic_data_step() {
        let input = r#"
query loadPost(slug: String) -> Post? {
  return db.posts.first()
    .where({ slug: input.slug })
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected query function to lower as loader block");
        };
        let AxBackendStmt::Data(post) = &loader.body[0] else {
            panic!("expected synthetic query data step");
        };
        assert_eq!(post.name, "__ax_return_1");
        assert_eq!(
            post.value,
            AxBackendValue::Query(
                AxQuerySpec::new(AxQuerySource::Stream {
                    collection: "posts".to_string(),
                })
                .first()
                .filter(AxQueryFilter::new(
                    "slug",
                    AxQueryFilterOp::Eq,
                    AxExpr::ident("input").member("slug"),
                ))
            )
        );
    }

    #[test]
    fn parses_fluent_db_query_with_quoted_object_keys() {
        let input = r#"
query loadPosts() -> Post[]
  data posts = db.posts.where({ "status": "published" }).order({ "created_at": "desc" }).all()
  return posts
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected query function to lower as loader block");
        };
        let AxBackendStmt::Data(posts) = &loader.body[0] else {
            panic!("expected data statement");
        };

        assert_eq!(
            posts.value,
            AxBackendValue::Query(
                AxQuerySpec::new(AxQuerySource::Stream {
                    collection: "posts".to_string(),
                })
                .filter(AxQueryFilter::new(
                    "status",
                    AxQueryFilterOp::Eq,
                    AxExpr::string("published"),
                ))
                .order(AxQueryOrder::new("created_at", AxQueryOrderDirection::Desc))
            )
        );
    }

    #[test]
    fn parses_fluent_db_query_filter_ops() {
        let input = r#"
query loadPosts() -> Post[]
  data posts = db.posts.where({ published: true }).whereNot({ archived: true }).whereIn({ status: ["published", "featured"] }).whereNotIn({ tag: blockedTags }).whereNull("deleted_at").whereNotNull("published_at").all()
  return posts
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected query function to lower as loader block");
        };
        let AxBackendStmt::Data(posts) = &loader.body[0] else {
            panic!("expected data statement");
        };

        assert_eq!(
            posts.value,
            AxBackendValue::Query(
                AxQuerySpec::new(AxQuerySource::Stream {
                    collection: "posts".to_string(),
                })
                .filter(AxQueryFilter::new(
                    "published",
                    AxQueryFilterOp::Eq,
                    AxExpr::bool(true),
                ))
                .filter(AxQueryFilter::new(
                    "archived",
                    AxQueryFilterOp::Ne,
                    AxExpr::bool(true),
                ))
                .filter(AxQueryFilter::new(
                    "status",
                    AxQueryFilterOp::In,
                    AxExpr::call(
                        ["list"],
                        [AxExpr::string("published"), AxExpr::string("featured")],
                    ),
                ))
                .filter(AxQueryFilter::new(
                    "tag",
                    AxQueryFilterOp::NotIn,
                    AxExpr::ident("blockedTags"),
                ))
                .filter(AxQueryFilter::new(
                    "deleted_at",
                    AxQueryFilterOp::IsNull,
                    AxExpr::bool(true),
                ))
                .filter(AxQueryFilter::new(
                    "published_at",
                    AxQueryFilterOp::IsNotNull,
                    AxExpr::bool(true),
                ))
            )
        );
    }

    #[test]
    fn rejects_fluent_db_query_with_invalid_quoted_object_key() {
        let input = r#"
query loadPosts() -> Post[]
  data posts = db.posts.where({ "bad-key": "published" }).all()
  return posts
"#;

        let error = parse_backend_ax(input).expect_err("invalid key should fail before lowering");

        assert_eq!(error, AxBackendParseError::InvalidQueryClause { line: 3 });
    }

    #[test]
    fn rejects_fluent_db_query_with_quoted_shorthand_key() {
        let input = r#"
query loadPosts() -> Post[]
  data posts = db.posts.where({ "status" }).all()
  return posts
"#;

        let error =
            parse_backend_ax(input).expect_err("quoted shorthand key should fail before lowering");

        assert_eq!(error, AxBackendParseError::InvalidQueryClause { line: 3 });
    }

    #[test]
    fn parses_fluent_db_query_shorthand_filter_and_offset() {
        let input = r#"
query loadPost() -> Post[]
  data posts = db.posts.where({ slug }).offset(10).all()
  return posts
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected query function to lower as loader block");
        };
        let AxBackendStmt::Data(posts) = &loader.body[0] else {
            panic!("expected data statement");
        };

        assert_eq!(
            posts.value,
            AxBackendValue::Query(
                AxQuerySpec::new(AxQuerySource::Stream {
                    collection: "posts".to_string(),
                })
                .filter(AxQueryFilter::new(
                    "slug",
                    AxQueryFilterOp::Eq,
                    AxExpr::ident("slug"),
                ))
                .offset(10)
            )
        );
    }

    #[test]
    fn parses_content_collection_binding_as_query() {
        let input = r#"
loader DocsList
  data docs = Content.Collection("docs")
    order slug asc
  return docs
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected loader block");
        };
        let AxBackendStmt::Data(docs) = &loader.body[0] else {
            panic!("expected data statement");
        };

        assert_eq!(
            docs.value,
            AxBackendValue::Query(
                AxQuerySpec::new(AxQuerySource::ContentCollection {
                    collection: "docs".to_string(),
                })
                .order(AxQueryOrder::new("slug", AxQueryOrderDirection::Asc))
            )
        );
    }

    #[test]
    fn parses_raw_sql_binding_as_query_escape_hatch() {
        let input = r#"
loader PostsList
  data posts = db.query("select * from posts where status = ?", "published")
  return posts
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Loader(loader) = &document.blocks[0] else {
            panic!("expected loader block");
        };
        let AxBackendStmt::Data(posts) = &loader.body[0] else {
            panic!("expected data statement");
        };

        assert_eq!(
            posts.value,
            AxBackendValue::Query(AxQuerySpec::new(AxQuerySource::RawSql {
                sql: "select * from posts where status = ?".to_string(),
                params: vec![AxExpr::String("published".to_string())],
            }))
        );
    }

    #[test]
    fn parses_action_with_input_and_mutations() {
        let input = r#"
action CreatePost
  input:
    title: string
    excerpt: string

  insert "posts"
    title: input.title
    excerpt: input.excerpt

  revalidate "/posts"
  return ok
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };

        assert_eq!(action.name, "CreatePost");
        assert_eq!(action.input.len(), 2);
        assert_eq!(action.body.len(), 3);
    }

    #[test]
    fn parses_invalidate_alias_as_revalidate_step() {
        let input = r#"
action CreatePost
  invalidate posts
  return ok
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };
        let AxBackendStmt::Revalidate(revalidate) = &action.body[0] else {
            panic!("expected invalidate alias to lower to revalidate");
        };

        assert_eq!(revalidate.target, AxExpr::ident("posts"));
        assert!(revalidate.literal);
    }

    #[test]
    fn parses_braced_action_block() {
        let input = r#"
action CreatePost -> Post {
  input:
    title: string

  insert "posts"
    title: input.title

  return created
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };

        assert_eq!(action.name, "CreatePost");
        assert_eq!(action.returns.as_deref(), Some("Post"));
        assert_eq!(action.input.len(), 1);
        assert_eq!(action.body.len(), 2);
    }

    #[test]
    fn parses_function_shaped_action_signature_inputs() {
        let input = r#"
action createPost(title: string, featured?: bool = false) -> Post {
  insert posts
    title: input.title
    featured: input.featured

  return created
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };

        assert_eq!(action.name, "createPost");
        assert_eq!(action.returns.as_deref(), Some("Post"));
        assert_eq!(
            action.input,
            vec![
                AxField::new("title", "string"),
                AxField::optional_with_default("featured", "bool", AxExpr::bool(false)),
            ]
        );
        assert_eq!(action.body.len(), 2);
    }

    #[test]
    fn parses_function_style_action_mutation_calls() {
        let input = r#"
action CreatePost(title: string, excerpt: string) {
  db.posts.insert({ title: input.title, excerpt: input.excerpt, status: "published" })
  revalidate("/posts")
  return ok()
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };

        assert_eq!(action.input.len(), 2);
        assert_eq!(action.body.len(), 3);
        assert_eq!(
            action.body[0],
            AxBackendStmt::insert(
                "posts",
                [
                    AxAssignment::new("title", AxExpr::ident("input").member("title")),
                    AxAssignment::new("excerpt", AxExpr::ident("input").member("excerpt")),
                    AxAssignment::new("status", AxExpr::string("published")),
                ],
            )
        );
        assert_eq!(
            action.body[1],
            AxBackendStmt::revalidate(AxExpr::string("/posts"))
        );
        assert_eq!(action.body[2], AxBackendStmt::r#return("ok"));
    }

    #[test]
    fn parses_function_style_update_and_delete_calls() {
        let input = r#"
action PublishPost(id: string) {
  db.posts.where({ id: input.id }).update({ status: "published" })
  db.audit.where({ post_id: input.id }).delete()
  invalidate(posts)
  return ok(post)
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };

        assert_eq!(action.body.len(), 4);
        let AxBackendStmt::Update(update) = &action.body[0] else {
            panic!("expected update mutation");
        };
        assert_eq!(update.collection, "posts");
        assert_eq!(
            update.fields,
            vec![AxAssignment::new("status", AxExpr::string("published"))]
        );
        assert_eq!(
            update.filters,
            vec![AxQueryFilter::new(
                "id",
                AxQueryFilterOp::Eq,
                AxExpr::ident("input").member("id")
            )]
        );

        let AxBackendStmt::Delete(delete) = &action.body[1] else {
            panic!("expected delete mutation");
        };
        assert_eq!(delete.collection, "audit");
        assert_eq!(
            delete.filters,
            vec![AxQueryFilter::new(
                "post_id",
                AxQueryFilterOp::Eq,
                AxExpr::ident("input").member("id")
            )]
        );
        assert_eq!(
            action.body[2],
            AxBackendStmt::invalidate(AxExpr::ident("posts"))
        );
        assert_eq!(
            action.body[3],
            AxBackendStmt::r#return(AxExpr::call(["ok"], [AxExpr::ident("post")]))
        );
    }

    #[test]
    fn parses_function_shaped_action_defaults_with_return_arrow_text() {
        let input = r#"
action saveLabel(label: string = "draft -> live") -> Label {
  return input
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };

        assert_eq!(action.name, "saveLabel");
        assert_eq!(action.returns.as_deref(), Some("Label"));
        assert_eq!(
            action.input,
            vec![AxField::with_default(
                "label",
                "string",
                AxExpr::string("draft -> live")
            )]
        );
    }

    #[test]
    fn parses_function_shaped_action_less_than_defaults_before_next_param() {
        let input = r#"
action saveFlag(enabled: bool = 1 < limit, title: string) {
  return input
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };

        assert_eq!(action.input.len(), 2);
        assert_eq!(action.input[0].name, "enabled");
        assert_eq!(action.input[0].ty, "bool");
        assert!(action.input[0].default.is_some());
        assert_eq!(action.input[1], AxField::new("title", "string"));
    }

    #[test]
    fn parses_function_shaped_action_generic_parameter_types() {
        let input = r#"
action saveMetadata(metadata: std::collections::HashMap<String, String>) {
  return input
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };

        assert_eq!(action.name, "saveMetadata");
        assert_eq!(
            action.input,
            vec![AxField::new(
                "metadata",
                "std::collections::HashMap<String, String>"
            )]
        );
    }

    #[test]
    fn parses_empty_function_shaped_action_signature() {
        let input = r#"
action clearTheme() {
  return ok
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };

        assert_eq!(action.name, "clearTheme");
        assert!(action.input.is_empty());
        assert_eq!(action.body.len(), 1);
    }

    #[test]
    fn parses_optional_action_input_fields() {
        let input = r#"
action CreatePost
  input:
    title: string
    summary?: string

  return input
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };

        assert_eq!(action.input[0], AxField::new("title", "string"));
        assert_eq!(action.input[1], AxField::optional("summary", "string"));
    }

    #[test]
    fn parses_action_input_default_values() {
        let input = r#"
action SetLanguage
  input:
    language?: string = "sr"
    count: i64 = 0
    ratio: Float = 0.625

  return input
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };

        assert_eq!(
            action.input[0],
            AxField::optional_with_default("language", "string", AxExpr::string("sr"))
        );
        assert_eq!(
            action.input[1],
            AxField::with_default("count", "i64", AxExpr::number(0))
        );
        assert_eq!(
            action.input[2],
            AxField::with_default("ratio", "Float", AxExpr::float(0.625))
        );
    }

    #[test]
    fn parses_route_with_typed_input_fields() {
        let input = r#"
route POST "/api/posts"
  input:
    title: string
    featured?: bool = false

  return json(input.title)
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Route(route) = &document.blocks[0] else {
            panic!("expected route block");
        };

        assert_eq!(route.input[0], AxField::new("title", "string"));
        assert_eq!(
            route.input[1],
            AxField::optional_with_default("featured", "bool", AxExpr::bool(false))
        );
        assert_eq!(route.body.len(), 1);
    }

    #[test]
    fn parses_action_patch_step() {
        let input = r#"
action SetTheme
  input:
    theme: string

  patch theme = input.theme
  return ok
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };
        let AxBackendStmt::Patch(patch) = &action.body[0] else {
            panic!("expected patch statement");
        };

        assert_eq!(patch.signal, AxExpr::ident("theme"));
        assert_eq!(patch.value, AxExpr::ident("input").member("theme"));
    }

    #[test]
    fn parses_require_in_list_with_error_fallback() {
        let input = r#"
action SetTheme
  input:
    theme: string

  require input.theme in ["silver", "bronze", "gold"] else error "Theme is not supported."
  return ok
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };
        let AxBackendStmt::Require(requirement) = &action.body[0] else {
            panic!("expected require statement");
        };

        assert_eq!(
            requirement.value,
            AxExpr::call(
                ["contains"],
                [
                    AxExpr::call(
                        ["list"],
                        [
                            AxExpr::string("silver"),
                            AxExpr::string("bronze"),
                            AxExpr::string("gold"),
                        ],
                    ),
                    AxExpr::ident("input").member("theme"),
                ],
            )
        );
        assert_eq!(
            requirement.fallback,
            Some(AxReturn::Expr(AxExpr::call(
                ["error"],
                [AxExpr::string("Theme is not supported.")]
            )))
        );
    }

    #[test]
    fn parses_guard_call_as_requirement_with_error_fallback() {
        let input = r#"
action SetTheme(theme: string) {
  guard(isSupportedTheme(input.theme), "Theme is not supported.")
  patch theme = input.theme
  return ok
}
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };
        let AxBackendStmt::Require(requirement) = &action.body[0] else {
            panic!("expected guard to lower into require statement");
        };

        assert_eq!(
            requirement.value,
            AxExpr::call(
                ["isSupportedTheme"],
                [AxExpr::ident("input").member("theme")]
            )
        );
        assert_eq!(
            requirement.fallback,
            Some(AxReturn::Expr(AxExpr::call(
                ["error"],
                [AxExpr::string("Theme is not supported.")]
            )))
        );
    }

    #[test]
    fn parses_require_in_variable_list() {
        let input = r#"
action SetTheme
  input:
    theme: string

  data themes = ["silver", "bronze", "gold"]
  require input.theme in themes else error "Theme is not supported."
  return ok
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };
        let AxBackendStmt::Require(requirement) = &action.body[1] else {
            panic!("expected require statement");
        };

        assert_eq!(
            requirement.value,
            AxExpr::call(
                ["contains"],
                [
                    AxExpr::ident("themes"),
                    AxExpr::ident("input").member("theme")
                ],
            )
        );
    }

    #[test]
    fn parses_backend_root_data_with_optional_type_annotations() {
        let input = r#"
backend
  data themes: List<String> = ["silver", "bronze", "gold"]
  data defaultTheme: String = "silver"
  env DATABASE_URL: Secret<String>
  env PUBLIC_SITE_URL: Public<String>
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Backend(root) = &document.blocks[0] else {
            panic!("expected backend root block");
        };

        assert_eq!(root.body.len(), 4);
        let AxBackendStmt::Data(themes) = &root.body[0] else {
            panic!("expected data statement");
        };
        assert_eq!(themes.name, "themes");
        let AxBackendStmt::Data(default_theme) = &root.body[1] else {
            panic!("expected data statement");
        };
        assert_eq!(default_theme.name, "defaultTheme");
        let AxBackendStmt::Env(database_url) = &root.body[2] else {
            panic!("expected env statement");
        };
        assert_eq!(database_url.name, "DATABASE_URL");
        assert_eq!(database_url.visibility, AxBackendEnvVisibility::Secret);
        assert_eq!(database_url.ty, "String");
        let AxBackendStmt::Env(site_url) = &root.body[3] else {
            panic!("expected env statement");
        };
        assert_eq!(site_url.name, "PUBLIC_SITE_URL");
        assert_eq!(site_url.visibility, AxBackendEnvVisibility::Public);
    }

    #[test]
    fn parses_route_response_metadata_steps() {
        let input = r#"
route GET "/api/session"
  require request.cookies.session
  header "Cache-Control" = "no-store"
  cookie "theme" = query.theme
  clearCookie "flash"
  return json("ok")
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Route(route) = &document.blocks[0] else {
            panic!("expected route block");
        };

        assert_eq!(
            route.body[0],
            AxBackendStmt::require(AxExpr::ident("request").member("cookies").member("session"))
        );
        assert_eq!(
            route.body[1],
            AxBackendStmt::header(AxExpr::string("Cache-Control"), AxExpr::string("no-store"))
        );
        assert_eq!(
            route.body[2],
            AxBackendStmt::cookie(
                AxExpr::string("theme"),
                AxExpr::ident("query").member("theme")
            )
        );
        assert_eq!(
            route.body[3],
            AxBackendStmt::clear_cookie(AxExpr::string("flash"))
        );
    }

    #[test]
    fn parses_route_hooks() {
        let input = r#"
route GET "/api/admin"
  before Auth.session
  before Security.headers
  after Cache.noStore
  return json("ok")
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Route(route) = &document.blocks[0] else {
            panic!("expected route block");
        };

        assert_eq!(
            route.body[0],
            AxBackendStmt::before(AxExpr::ident("Auth").member("session"))
        );
        assert_eq!(
            route.body[1],
            AxBackendStmt::before(AxExpr::ident("Security").member("headers"))
        );
        assert_eq!(
            route.body[2],
            AxBackendStmt::after(AxExpr::ident("Cache").member("noStore"))
        );
    }

    #[test]
    fn parses_require_fallback_step() {
        let input = r#"
route GET "/api/admin"
  require request.cookies.session else redirect("/login")
  return json("ok")
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Route(route) = &document.blocks[0] else {
            panic!("expected route block");
        };

        assert_eq!(
            route.body[0],
            AxBackendStmt::require_with_fallback(
                AxExpr::ident("request").member("cookies").member("session"),
                AxExpr::call(["redirect"], [AxExpr::string("/login")])
            )
        );
    }

    #[test]
    fn parses_update_mutation_with_where_clause() {
        let input = r#"
action PublishPost
  input:
    id: i64
    title: string

  update "posts"
    title: input.title
    where id = input.id

  return ok
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };

        let AxBackendStmt::Update(mutation) = &action.body[0] else {
            panic!("expected update statement");
        };

        assert_eq!(mutation.collection, "posts");
        assert_eq!(mutation.fields.len(), 1);
        assert_eq!(mutation.filters.len(), 1);
        assert_eq!(mutation.filters[0].field, "id");
    }

    #[test]
    fn parses_job_send_step() {
        let input = r#"
job PublishDailyDigest
  data posts = Query.PublishedPosts()
  send DigestEmail with posts
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Job(job) = &document.blocks[0] else {
            panic!("expected job block");
        };

        assert_eq!(job.name, "PublishDailyDigest");
        assert_eq!(job.body.len(), 2);
    }

    #[test]
    fn parses_delete_mutation_with_where_clause() {
        let input = r#"
action RemovePost
  input:
    id: i64

  delete "posts"
    where id = input.id

  return ok
"#;

        let document = parse_backend_ax(input).expect("document should parse");

        let AxBackendBlock::Action(action) = &document.blocks[0] else {
            panic!("expected action block");
        };

        let AxBackendStmt::Delete(mutation) = &action.body[0] else {
            panic!("expected delete statement");
        };

        assert_eq!(mutation.collection, "posts");
        assert!(mutation.fields.is_empty());
        assert_eq!(mutation.filters.len(), 1);
    }
}
