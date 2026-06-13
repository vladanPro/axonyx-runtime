use thiserror::Error;

use crate::ax_ast::prelude::*;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AxParseError {
    #[error("document is empty")]
    EmptyDocument,
    #[error("tabs are not supported in indentation at line {line}")]
    TabsNotSupported { line: usize },
    #[error("indentation must use multiples of two spaces at line {line}")]
    InvalidIndentation { line: usize },
    #[error("expected `page <Name>` at line {line}")]
    InvalidPage { line: usize },
    #[error("unexpected indentation at line {line}")]
    UnexpectedIndentation { line: usize },
    #[error("invalid data binding at line {line}")]
    InvalidDataBinding { line: usize },
    #[error("invalid each statement at line {line}")]
    InvalidEach { line: usize },
    #[error("invalid pipeline stage at line {line}")]
    InvalidPipelineStage { line: usize },
    #[error("invalid component syntax at line {line}")]
    InvalidComponent { line: usize },
    #[error("invalid title syntax at line {line}")]
    InvalidTitle { line: usize },
    #[error("invalid theme syntax at line {line}")]
    InvalidTheme { line: usize },
    #[error("invalid {kind} syntax at line {line}")]
    InvalidHeadTag { line: usize, kind: String },
    #[error("invalid expression at line {line}: {message}")]
    InvalidExpression { line: usize, message: String },
}

#[derive(Debug, Clone)]
struct AxLine {
    line: usize,
    indent: usize,
    text: String,
}

pub fn parse_ax(input: &str) -> Result<AxDocument, AxParseError> {
    let lines = preprocess(input)?;
    if lines.is_empty() {
        return Err(AxParseError::EmptyDocument);
    }

    let mut parser = Parser { lines, pos: 0 };
    parser.parse_document()
}

struct Parser {
    lines: Vec<AxLine>,
    pos: usize,
}

impl Parser {
    fn parse_document(&mut self) -> Result<AxDocument, AxParseError> {
        let page_line = self.current().ok_or(AxParseError::EmptyDocument)?.clone();
        if page_line.indent != 0 || !page_line.text.starts_with("page ") {
            return Err(AxParseError::InvalidPage {
                line: page_line.line,
            });
        }

        let name = page_line.text["page ".len()..].trim();
        if name.is_empty() {
            return Err(AxParseError::InvalidPage {
                line: page_line.line,
            });
        }

        self.pos += 1;

        let mut head = AxHead::default();
        let mut body = Vec::new();

        while let Some(line) = self.current() {
            if line.indent < 2 {
                break;
            }

            if line.indent > 2 {
                return Err(AxParseError::UnexpectedIndentation { line: line.line });
            }

            if line.text.starts_with("title ") {
                head.title = Some(self.parse_title()?);
                continue;
            }

            if line.text.starts_with("theme ") {
                head.theme = Some(self.parse_theme()?);
                continue;
            }

            if line.text.starts_with("meta ") {
                head.metas.push(self.parse_head_tag("meta")?);
                continue;
            }

            if line.text.starts_with("link ") {
                head.links.push(self.parse_head_tag("link")?);
                continue;
            }

            if line.text.starts_with("script ") {
                head.scripts.push(self.parse_head_tag("script")?);
                continue;
            }

            body.push(self.parse_statement(2)?);
        }

        Ok(AxDocument {
            imports: Vec::new(),
            functions: Vec::new(),
            components: Vec::new(),
            head,
            page: AxPage::new(name, body),
        })
    }

    fn parse_block(&mut self, indent: usize) -> Result<Vec<AxStatement>, AxParseError> {
        let mut statements = Vec::new();

        while let Some(line) = self.current() {
            if line.indent < indent {
                break;
            }

            if line.indent > indent {
                return Err(AxParseError::UnexpectedIndentation { line: line.line });
            }

            statements.push(self.parse_statement(indent)?);
        }

        Ok(statements)
    }

    fn parse_statement(&mut self, indent: usize) -> Result<AxStatement, AxParseError> {
        let line = self.current().expect("checked by parse_block").clone();

        if let Some(next) = self.peek(1) {
            if next.indent == indent + 2 && next.text.starts_with("|> ") {
                return self.parse_pipeline(indent);
            }
        }

        if line.text.starts_with("data ") {
            self.parse_data()
        } else if line.text.starts_with("each ") {
            self.parse_each(indent)
        } else {
            self.parse_component(indent)
        }
    }

    fn parse_data(&mut self) -> Result<AxStatement, AxParseError> {
        let line = self.current().expect("line exists").clone();
        let body = line.text["data ".len()..].trim();
        let Some((name, expr)) = body.split_once('=') else {
            return Err(AxParseError::InvalidDataBinding { line: line.line });
        };

        let name = name.trim();
        if name.is_empty() {
            return Err(AxParseError::InvalidDataBinding { line: line.line });
        }

        let value = parse_expr(expr.trim(), line.line)?;
        self.pos += 1;

        Ok(AxStatement::data(name, value))
    }

    fn parse_each(&mut self, indent: usize) -> Result<AxStatement, AxParseError> {
        let line = self.current().expect("line exists").clone();
        let body = line.text["each ".len()..].trim();
        let Some((binding, source)) = body.split_once(" in ") else {
            return Err(AxParseError::InvalidEach { line: line.line });
        };

        let binding = binding.trim();
        if binding.is_empty() {
            return Err(AxParseError::InvalidEach { line: line.line });
        }

        let source = parse_expr(source.trim(), line.line)?;
        self.pos += 1;
        let body = self.parse_block(indent + 2)?;

        Ok(AxStatement::each(binding, source, body))
    }

    fn parse_component(&mut self, indent: usize) -> Result<AxStatement, AxParseError> {
        let line = self.current().expect("line exists").clone();
        let component = parse_component_line(&line.text, line.line)?;
        self.pos += 1;

        let component = match component.body {
            AxBody::Empty => {
                if let Some(next) = self.current() {
                    if next.indent == indent + 2 && !next.text.starts_with("|> ") {
                        let body = self.parse_block(indent + 2)?;
                        component.block(body)
                    } else {
                        component
                    }
                } else {
                    component
                }
            }
            _ => component,
        };

        Ok(AxStatement::component(component))
    }

    fn parse_pipeline(&mut self, indent: usize) -> Result<AxStatement, AxParseError> {
        let line = self.current().expect("line exists").clone();
        let source = parse_expr(&line.text, line.line)?;
        self.pos += 1;

        let mut pipeline = AxPipeline::new(source);

        while let Some(stage_line) = self.current() {
            if stage_line.indent < indent + 2 {
                break;
            }

            if stage_line.indent != indent + 2 || !stage_line.text.starts_with("|> ") {
                return Err(AxParseError::InvalidPipelineStage {
                    line: stage_line.line,
                });
            }

            let stage_text = stage_line.text["|> ".len()..].trim();
            if let Some(binding) = stage_text.strip_prefix("Each ") {
                let binding = binding.trim();
                if binding.is_empty() {
                    return Err(AxParseError::InvalidPipelineStage {
                        line: stage_line.line,
                    });
                }
                pipeline = pipeline.stage(AxPipelineStage::Each(AxEachStage::new(binding)));
            } else {
                pipeline = pipeline.stage(AxPipelineStage::Component(parse_component_line(
                    stage_text,
                    stage_line.line,
                )?));
            }

            self.pos += 1;
        }

        Ok(AxStatement::pipeline(pipeline))
    }

    fn current(&self) -> Option<&AxLine> {
        self.lines.get(self.pos)
    }

    fn peek(&self, offset: usize) -> Option<&AxLine> {
        self.lines.get(self.pos + offset)
    }

    fn parse_title(&mut self) -> Result<AxExpr, AxParseError> {
        let line = self.current().expect("line exists").clone();
        let expr = line.text["title ".len()..].trim();
        if expr.is_empty() {
            return Err(AxParseError::InvalidTitle { line: line.line });
        }

        self.pos += 1;
        parse_expr(expr, line.line)
    }

    fn parse_theme(&mut self) -> Result<AxExpr, AxParseError> {
        let line = self.current().expect("line exists").clone();
        let expr = line.text["theme ".len()..].trim();
        if expr.is_empty() {
            return Err(AxParseError::InvalidTheme { line: line.line });
        }

        self.pos += 1;
        parse_expr(expr, line.line).map_err(|_| AxParseError::InvalidTheme { line: line.line })
    }

    fn parse_head_tag(&mut self, kind: &str) -> Result<AxHeadTag, AxParseError> {
        let line = self.current().expect("line exists").clone();
        let body = line.text[kind.len()..].trim();
        let props =
            parse_named_props(body, line.line).map_err(|_| AxParseError::InvalidHeadTag {
                line: line.line,
                kind: kind.to_string(),
            })?;

        if props.is_empty() {
            return Err(AxParseError::InvalidHeadTag {
                line: line.line,
                kind: kind.to_string(),
            });
        }

        self.pos += 1;
        Ok(AxHeadTag::new(
            props
                .into_iter()
                .map(|(name, value)| AxProp::new(name, value)),
        ))
    }
}

fn preprocess(input: &str) -> Result<Vec<AxLine>, AxParseError> {
    let mut lines = Vec::new();

    for (index, raw) in input.lines().enumerate() {
        let line_no = index + 1;
        if raw.trim().is_empty() {
            continue;
        }

        if raw.contains('\t') {
            return Err(AxParseError::TabsNotSupported { line: line_no });
        }

        let indent = raw.chars().take_while(|c| *c == ' ').count();
        if indent % 2 != 0 {
            return Err(AxParseError::InvalidIndentation { line: line_no });
        }

        lines.push(AxLine {
            line: line_no,
            indent,
            text: raw.trim().to_string(),
        });
    }

    Ok(lines)
}

fn parse_component_line(input: &str, line: usize) -> Result<AxComponent, AxParseError> {
    let (head, inline) = split_inline_arrow(input);
    let (name, rest) = split_first_token(head).ok_or(AxParseError::InvalidComponent { line })?;

    if name.is_empty() || !is_component_name(name) {
        return Err(AxParseError::InvalidComponent { line });
    }

    let mut component = AxComponent::new(name);

    if !rest.trim().is_empty() {
        for (prop_name, value) in parse_named_props(rest.trim(), line)? {
            match prop_name.as_str() {
                "recipe" => component = component.recipe(value),
                "class" => component = component.class(value),
                _ => component = component.prop(prop_name, value),
            }
        }
    }

    if let Some(inline_expr) = inline {
        component = component.inline(parse_expr(inline_expr.trim(), line)?);
    }

    Ok(component)
}

fn parse_named_props(input: &str, line: usize) -> Result<Vec<(String, AxExpr)>, AxParseError> {
    if input.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut props = Vec::new();
    for part in split_top_level(input, ',') {
        let Some((name, value)) = part.split_once(':') else {
            return Err(AxParseError::InvalidComponent { line });
        };

        let name = name.trim();
        if name.is_empty() {
            return Err(AxParseError::InvalidComponent { line });
        }

        props.push((name.to_string(), parse_expr(value.trim(), line)?));
    }

    Ok(props)
}

fn is_component_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    if !first.is_ascii_alphabetic() {
        return false;
    }

    if first.is_ascii_uppercase() {
        return chars.all(|ch| ch.is_ascii_alphanumeric());
    }

    chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

fn split_inline_arrow(input: &str) -> (&str, Option<&str>) {
    if let Some(index) = find_top_level(input, "->") {
        let head = input[..index].trim_end();
        let tail = input[index + 2..].trim_start();
        (head, Some(tail))
    } else {
        (input, None)
    }
}

fn split_first_token(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(index) = trimmed.find(char::is_whitespace) {
        Some((&trimmed[..index], trimmed[index..].trim_start()))
    } else {
        Some((trimmed, ""))
    }
}

pub(crate) fn parse_expr(input: &str, line: usize) -> Result<AxExpr, AxParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AxParseError::InvalidExpression {
            line,
            message: "expression is empty".to_string(),
        });
    }

    if let Some(loader_name) = input.strip_prefix("load ") {
        let loader_name = loader_name.trim();
        if loader_name.is_empty() {
            return Err(AxParseError::InvalidExpression {
                line,
                message: "load target is empty".to_string(),
            });
        }

        let loader_name = loader_name.trim_matches('"').trim_matches('\'').to_string();

        return Ok(AxExpr::Call {
            path: vec!["load".to_string()],
            args: vec![AxExpr::string(loader_name)],
        });
    }

    if let Some(action_name) = input.strip_prefix("action ") {
        let action_name = action_name.trim();
        if action_name.is_empty() {
            return Err(AxParseError::InvalidExpression {
                line,
                message: "action target is empty".to_string(),
            });
        }

        let action_name = action_name.trim_matches('"').trim_matches('\'').to_string();

        return Ok(AxExpr::Call {
            path: vec!["action".to_string()],
            args: vec![AxExpr::string(action_name)],
        });
    }

    parse_operator_expr(input, line)
}

fn parse_operator_expr(input: &str, line: usize) -> Result<AxExpr, AxParseError> {
    parse_binary_expr(input, line, 0)
}

fn parse_binary_expr(input: &str, line: usize, min_precedence: u8) -> Result<AxExpr, AxParseError> {
    let Some((index, op, precedence)) = find_lowest_precedence_operator(input, min_precedence)
    else {
        return parse_unary_expr(input, line);
    };

    let left_source = input[..index].trim();
    let right_source = input[index + binary_op_len(op)..].trim();
    if left_source.is_empty() || right_source.is_empty() {
        return Err(AxParseError::InvalidExpression {
            line,
            message: format!("missing operand in `{input}`"),
        });
    }

    Ok(AxExpr::binary(
        op,
        parse_binary_expr(left_source, line, min_precedence)?,
        parse_binary_expr(right_source, line, precedence + 1)?,
    ))
}

fn parse_unary_expr(input: &str, line: usize) -> Result<AxExpr, AxParseError> {
    let input = input.trim();
    if let Some(rest) = input.strip_prefix('!') {
        let rest = rest.trim();
        if rest.is_empty() || rest.starts_with('=') {
            return parse_primary_expr(input, line);
        }
        return Ok(AxExpr::unary(AxUnaryOp::Not, parse_unary_expr(rest, line)?));
    }

    if let Some(rest) = input.strip_prefix('-') {
        let rest = rest.trim();
        if rest.is_empty() {
            return parse_primary_expr(input, line);
        }
        if let Ok(value) = input.parse::<i64>() {
            return Ok(AxExpr::number(value));
        }
        return Ok(AxExpr::unary(AxUnaryOp::Neg, parse_unary_expr(rest, line)?));
    }

    parse_primary_expr(input, line)
}

fn parse_primary_expr(input: &str, line: usize) -> Result<AxExpr, AxParseError> {
    let original = input.trim();
    let input = trim_wrapping_parens(original);
    if input != original {
        return parse_operator_expr(input, line);
    }

    if let Some(value) = parse_quoted_string(input, line)? {
        return Ok(AxExpr::string(value));
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

    if input.starts_with('[') || input.ends_with(']') {
        return parse_list_expr(input, line);
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
                return Err(AxParseError::InvalidExpression {
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

    if input.contains('.') || input.contains("?.") {
        return parse_member_expr(input, line);
    }

    Ok(AxExpr::ident(input))
}

fn parse_list_expr(input: &str, line: usize) -> Result<AxExpr, AxParseError> {
    if !input.starts_with('[') || !input.ends_with(']') {
        return Err(AxParseError::InvalidExpression {
            line,
            message: format!("invalid list literal `{input}`"),
        });
    }

    let inner = input[1..input.len() - 1].trim();
    if inner.is_empty() {
        return Ok(AxExpr::list([]));
    }

    let items = split_top_level(inner, ',')
        .into_iter()
        .map(|part| {
            if part.is_empty() {
                Err(AxParseError::InvalidExpression {
                    line,
                    message: format!("empty item in list literal `{input}`"),
                })
            } else {
                parse_expr(part, line)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(AxExpr::list(items))
}

fn find_lowest_precedence_operator(
    input: &str,
    min_precedence: u8,
) -> Option<(usize, AxBinaryOp, u8)> {
    let mut depth = 0usize;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    let mut result = None;

    for (index, ch) in input.char_indices() {
        match in_string {
            Some(quote) => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == quote {
                    in_string = None;
                }
                continue;
            }
            None => match ch {
                '"' | '\'' => {
                    in_string = Some(ch);
                    continue;
                }
                '(' | '[' | '{' => {
                    depth += 1;
                    continue;
                }
                ')' | ']' | '}' => {
                    depth = depth.saturating_sub(1);
                    continue;
                }
                _ if depth > 0 => continue,
                _ => {}
            },
        }

        let Some(candidate) = binary_op_at(input, index) else {
            continue;
        };
        if !operator_has_left_operand(input, index) {
            continue;
        }
        let precedence = binary_precedence(candidate);
        if precedence < min_precedence {
            continue;
        }

        if result
            .map(|(_, _, current_precedence)| precedence <= current_precedence)
            .unwrap_or(true)
        {
            result = Some((index, candidate, precedence));
        }
    }

    result
}

fn binary_op_at(input: &str, index: usize) -> Option<AxBinaryOp> {
    let rest = &input[index..];
    if rest.starts_with("in") && is_word_operator_boundary(input, index, "in") {
        return Some(AxBinaryOp::In);
    }

    for (token, op) in [
        ("??", AxBinaryOp::Fallback),
        ("||", AxBinaryOp::Or),
        ("&&", AxBinaryOp::And),
        ("==", AxBinaryOp::Eq),
        ("!=", AxBinaryOp::Ne),
        (">=", AxBinaryOp::Ge),
        ("<=", AxBinaryOp::Le),
        (">", AxBinaryOp::Gt),
        ("<", AxBinaryOp::Lt),
        ("+", AxBinaryOp::Add),
        ("-", AxBinaryOp::Sub),
        ("*", AxBinaryOp::Mul),
        ("/", AxBinaryOp::Div),
        ("%", AxBinaryOp::Rem),
    ] {
        if rest.starts_with(token) {
            return Some(op);
        }
    }
    None
}

fn binary_precedence(op: AxBinaryOp) -> u8 {
    match op {
        AxBinaryOp::Fallback => 1,
        AxBinaryOp::Or => 2,
        AxBinaryOp::And => 3,
        AxBinaryOp::Eq | AxBinaryOp::Ne => 4,
        AxBinaryOp::Gt | AxBinaryOp::Ge | AxBinaryOp::Lt | AxBinaryOp::Le | AxBinaryOp::In => 5,
        AxBinaryOp::Add | AxBinaryOp::Sub => 6,
        AxBinaryOp::Mul | AxBinaryOp::Div | AxBinaryOp::Rem => 7,
    }
}

fn binary_op_len(op: AxBinaryOp) -> usize {
    match op {
        AxBinaryOp::Gt
        | AxBinaryOp::Lt
        | AxBinaryOp::Add
        | AxBinaryOp::Sub
        | AxBinaryOp::Mul
        | AxBinaryOp::Div
        | AxBinaryOp::Rem => 1,
        AxBinaryOp::Fallback
        | AxBinaryOp::Or
        | AxBinaryOp::And
        | AxBinaryOp::Eq
        | AxBinaryOp::Ne
        | AxBinaryOp::Ge
        | AxBinaryOp::Le
        | AxBinaryOp::In => 2,
    }
}

fn is_word_operator_boundary(input: &str, index: usize, token: &str) -> bool {
    let before = input[..index].chars().next_back();
    let after_index = index + token.len();
    let after = input[after_index..].chars().next();

    before.is_none_or(|ch| !is_identifier_char(ch))
        && after.is_none_or(|ch| !is_identifier_char(ch))
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn operator_has_left_operand(input: &str, index: usize) -> bool {
    input[..index]
        .chars()
        .rev()
        .find(|ch| !ch.is_whitespace())
        .is_some_and(|ch| !matches!(ch, '(' | '[' | '{' | ',' | '+' | '-' | '*' | '/' | '%'))
}

fn trim_wrapping_parens(input: &str) -> &str {
    let mut current = input.trim();
    loop {
        if !current.starts_with('(') || !current.ends_with(')') {
            return current;
        }
        if !outer_parens_wrap(current) {
            return current;
        }
        let inner = &current[1..current.len() - 1];
        current = inner.trim();
    }
}

fn outer_parens_wrap(input: &str) -> bool {
    let mut depth = 0usize;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    let last_index = input.len() - 1;

    for (index, ch) in input.char_indices() {
        match in_string {
            Some(quote) => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == quote {
                    in_string = None;
                }
                continue;
            }
            None => match ch {
                '"' | '\'' => {
                    in_string = Some(ch);
                    continue;
                }
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && index != last_index {
                        return false;
                    }
                }
                _ => {}
            },
        }
    }

    depth == 0
}

fn parse_member_expr(input: &str, line: usize) -> Result<AxExpr, AxParseError> {
    let mut cursor = input.trim();
    let first_end = cursor
        .find(['.', '?'])
        .ok_or_else(|| AxParseError::InvalidExpression {
            line,
            message: format!("invalid member expression `{input}`"),
        })?;
    let first = cursor[..first_end].trim();
    if first.is_empty() {
        return Err(AxParseError::InvalidExpression {
            line,
            message: format!("invalid member expression `{input}`"),
        });
    }

    let mut expr = AxExpr::ident(first);
    cursor = &cursor[first_end..];

    while !cursor.is_empty() {
        let optional = if let Some(rest) = cursor.strip_prefix("?.") {
            cursor = rest;
            true
        } else if let Some(rest) = cursor.strip_prefix('.') {
            cursor = rest;
            false
        } else {
            return Err(AxParseError::InvalidExpression {
                line,
                message: format!("invalid member expression `{input}`"),
            });
        };

        let next_separator = cursor.find(['.', '?']).unwrap_or(cursor.len());
        let property = cursor[..next_separator].trim();
        if property.is_empty() {
            return Err(AxParseError::InvalidExpression {
                line,
                message: format!("invalid member expression `{input}`"),
            });
        }

        expr = if optional {
            expr.optional_member(property)
        } else {
            expr.member(property)
        };
        cursor = &cursor[next_separator..];
    }

    Ok(expr)
}

fn parse_quoted_string(input: &str, line: usize) -> Result<Option<String>, AxParseError> {
    let mut chars = input.chars();
    let Some(quote) = chars.next() else {
        return Ok(None);
    };
    if quote != '"' && quote != '\'' {
        return Ok(None);
    }
    if !input.ends_with(quote) || input.len() < 2 {
        return Err(AxParseError::InvalidExpression {
            line,
            message: "unterminated string literal".to_string(),
        });
    }

    let mut value = String::new();
    let mut escaped = false;
    for ch in input[1..input.len() - quote.len_utf8()].chars() {
        if escaped {
            value.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '\\' => '\\',
                '"' => '"',
                '\'' => '\'',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            value.push(ch);
        }
    }

    if escaped {
        return Err(AxParseError::InvalidExpression {
            line,
            message: "unterminated string escape".to_string(),
        });
    }

    Ok(Some(value))
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

fn find_top_level(input: &str, needle: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string: Option<char> = None;
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    let needle_chars: Vec<char> = needle.chars().collect();

    let mut i = 0usize;
    while i < chars.len() {
        let (byte_index, ch) = chars[i];
        match in_string {
            Some(quote) => {
                if ch == quote {
                    in_string = None;
                }
            }
            None => match ch {
                '"' | '\'' => in_string = Some(ch),
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ => {
                    if depth == 0 && matches_needle(&chars, i, &needle_chars) {
                        return Some(byte_index);
                    }
                }
            },
        }
        i += 1;
    }

    None
}

fn matches_needle(chars: &[(usize, char)], start: usize, needle: &[char]) -> bool {
    if start + needle.len() > chars.len() {
        return false;
    }

    chars[start..start + needle.len()]
        .iter()
        .map(|(_, ch)| *ch)
        .eq(needle.iter().copied())
}

pub mod prelude {
    pub use super::parse_ax;
    pub use super::AxParseError;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_indentation_first_page() {
        let input = r#"
page Home
  data posts = db.posts.all()

  Container max: "xl"
    Grid cols: 3, gap: "md"
      each post in posts
        Card title: post.title
          Copy -> post.excerpt
"#;

        let document = parse_ax(input).expect("document should parse");

        assert_eq!(document.page.name, "Home");
        assert_eq!(document.page.body.len(), 2);
    }

    #[test]
    fn parses_component_style_layers() {
        let input = r#"
page Home
  Button tone: "primary", size: "lg", recipe: "hero-cta", class: "w-full" -> "Launch"
"#;

        let document = parse_ax(input).expect("document should parse");

        let AxStatement::Component(button) = &document.page.body[0] else {
            panic!("expected button component");
        };

        assert_eq!(button.props.len(), 2);
        assert_eq!(button.style.recipe, Some(AxExpr::string("hero-cta")));
        assert_eq!(button.style.class, Some(AxExpr::string("w-full")));
    }

    #[test]
    fn parses_native_html_tag_component() {
        let input = r#"
page Home
  section class: "hero-shell"
    a href: "/docs", target: "_blank" -> "Read docs"
"#;

        let document = parse_ax(input).expect("document should parse");

        let AxStatement::Component(section) = &document.page.body[0] else {
            panic!("expected section component");
        };

        assert_eq!(section.name, "section");
        assert_eq!(section.style.class, Some(AxExpr::string("hero-shell")));

        let AxBody::Block(body) = &section.body else {
            panic!("expected block body");
        };

        let AxStatement::Component(anchor) = &body[0] else {
            panic!("expected anchor component");
        };

        assert_eq!(anchor.name, "a");
        assert_eq!(anchor.props.len(), 2);
    }

    #[test]
    fn parses_document_head_metadata() {
        let input = r#"
page Home
  title "Hello Axonyx"
  theme "silver"
  meta name: "description", content: "A Rust-first site."
  link rel: "icon", href: "/favicon.svg", type: "image/svg+xml"
  script src: "/app.js", defer: true
  section class: "hero-shell"
    Copy -> "Body"
"#;

        let document = parse_ax(input).expect("document should parse");

        assert_eq!(document.head.title, Some(AxExpr::string("Hello Axonyx")));
        assert_eq!(document.head.theme, Some(AxExpr::string("silver")));
        assert_eq!(document.head.metas.len(), 1);
        assert_eq!(document.head.links.len(), 1);
        assert_eq!(document.head.scripts.len(), 1);
        assert_eq!(document.page.body.len(), 1);
    }

    #[test]
    fn parses_pipeline_sketch() {
        let input = r#"
page Home
  db.users.all()
    |> Grid cols: 2
    |> Each user
    |> ProfileCard
"#;

        let document = parse_ax(input).expect("document should parse");

        let AxStatement::Pipeline(pipeline) = &document.page.body[0] else {
            panic!("expected pipeline");
        };

        assert_eq!(pipeline.stages.len(), 3);
    }

    #[test]
    fn parses_loader_call_sugar() {
        let input = r#"
page Posts
  data posts = load PostsList
"#;

        let document = parse_ax(input).expect("document should parse");

        let AxStatement::Data(binding) = &document.page.body[0] else {
            panic!("expected data binding");
        };

        assert_eq!(
            binding.value,
            AxExpr::Call {
                path: vec!["load".to_string()],
                args: vec![AxExpr::string("PostsList")],
            }
        );
    }

    #[test]
    fn parses_optional_member_expression() {
        let input = r#"
page Home
  Copy -> post?.summary
"#;

        let document = parse_ax(input).expect("document should parse");
        let AxStatement::Component(copy) = &document.page.body[0] else {
            panic!("expected copy component");
        };
        let AxBody::Inline(expr) = &copy.body else {
            panic!("expected inline expression");
        };

        assert_eq!(expr, &AxExpr::ident("post").optional_member("summary"));
    }

    #[test]
    fn parses_binary_operator_precedence() {
        let expr = parse_expr("count + 1 * 2", 1).expect("expression should parse");

        assert_eq!(
            expr,
            AxExpr::binary(
                AxBinaryOp::Add,
                AxExpr::ident("count"),
                AxExpr::binary(AxBinaryOp::Mul, AxExpr::number(1), AxExpr::number(2)),
            )
        );
    }

    #[test]
    fn parses_optional_fallback_expression() {
        let expr =
            parse_expr(r#"post?.summary ?? "No summary""#, 1).expect("expression should parse");

        assert_eq!(
            expr,
            AxExpr::binary(
                AxBinaryOp::Fallback,
                AxExpr::ident("post").optional_member("summary"),
                AxExpr::string("No summary"),
            )
        );
    }

    #[test]
    fn parses_logical_and_unary_expression() {
        let expr =
            parse_expr(r#"status == "published" && !hidden"#, 1).expect("expression should parse");

        assert_eq!(
            expr,
            AxExpr::binary(
                AxBinaryOp::And,
                AxExpr::binary(
                    AxBinaryOp::Eq,
                    AxExpr::ident("status"),
                    AxExpr::string("published"),
                ),
                AxExpr::unary(AxUnaryOp::Not, AxExpr::ident("hidden")),
            )
        );
    }

    #[test]
    fn parses_in_operator_without_confusing_identifier_prefixes() {
        let expr = parse_expr("input.theme in themes", 1).expect("expression should parse");

        assert_eq!(
            expr,
            AxExpr::binary(
                AxBinaryOp::In,
                AxExpr::ident("input").member("theme"),
                AxExpr::ident("themes"),
            )
        );
    }

    #[test]
    fn parses_list_literal_expression() {
        let expr =
            parse_expr(r#"["silver", "gold", input.theme]"#, 1).expect("expression should parse");

        assert_eq!(
            expr,
            AxExpr::list([
                AxExpr::string("silver"),
                AxExpr::string("gold"),
                AxExpr::ident("input").member("theme"),
            ])
        );
    }

    #[test]
    fn parses_in_operator_with_inline_list_literal() {
        let expr =
            parse_expr(r#"input.theme in ["silver", "gold"]"#, 1).expect("expression should parse");

        assert_eq!(
            expr,
            AxExpr::binary(
                AxBinaryOp::In,
                AxExpr::ident("input").member("theme"),
                AxExpr::list([AxExpr::string("silver"), AxExpr::string("gold")]),
            )
        );
    }

    #[test]
    fn parses_escaped_string_literals() {
        let input = r#"
page Home
  pre class: "docs-code" -> "line one\nline two"
"#;

        let document = parse_ax(input).expect("document should parse");
        let AxStatement::Component(pre) = &document.page.body[0] else {
            panic!("expected pre component");
        };
        let AxBody::Inline(expr) = &pre.body else {
            panic!("expected inline string");
        };

        assert_eq!(expr, &AxExpr::string("line one\nline two"));
    }

    #[test]
    fn parses_action_call_sugar() {
        let input = r#"
page Posts
  form method: "post", action: action CreatePost
"#;

        let document = parse_ax(input).expect("document should parse");

        let AxStatement::Component(form) = &document.page.body[0] else {
            panic!("expected form component");
        };

        assert_eq!(
            form.props[1].value,
            AxExpr::Call {
                path: vec!["action".to_string()],
                args: vec![AxExpr::string("CreatePost")],
            }
        );
    }
}
