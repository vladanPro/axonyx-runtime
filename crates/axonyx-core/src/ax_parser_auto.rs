use std::collections::BTreeMap;
use thiserror::Error;

use crate::ax_ast::prelude::*;
use crate::ax_ast_v2::prelude::*;
use crate::ax_parser::{parse_ax, parse_expr, AxParseError};
use crate::ax_parser_v2::{parse_ax_v2, AxParseV2Error};
use crate::ax_semantics_v2::{validate_ax_v2_semantics, AxSemanticV2Error};
use crate::ax_types::{format_expr, AxDataContext, AxType, AxTypeParseError};

#[derive(Debug, Error)]
pub enum AxAutoParseError {
    #[error("failed to parse indentation-first .ax file")]
    V1(#[from] AxParseError),
    #[error("failed to parse JSX-like .ax file")]
    V2(#[from] AxParseV2Error),
    #[error("failed to validate JSX-like .ax file")]
    Semantic(#[from] AxSemanticV2Error),
    #[error("failed to lower JSX-like .ax file into runtime document")]
    Convert(#[from] AxConvertV2Error),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AxConvertV2Error {
    #[error("invalid expression `{source}`")]
    InvalidExpression {
        source: String,
        #[source]
        error: AxParseError,
    },
    #[error("`<{tag}>` requires `{attr}`")]
    MissingControlAttr { tag: String, attr: String },
    #[error("`<{tag}>` requires `{attr}` to be an identifier name")]
    InvalidBindingAttr { tag: String, attr: String },
    #[error("`<{tag}>` does not allow attributes on `<{branch}>`")]
    ControlBranchAttrsNotSupported { tag: String, branch: String },
    #[error("`<{tag}>` only allows one `<{branch}>` branch")]
    DuplicateControlBranch { tag: String, branch: String },
    #[error("`<{branch}>` must be the final control branch inside `<{tag}>`")]
    ControlBranchMustBeLast { tag: String, branch: String },
    #[error("`<{branch}>` is only valid inside `<{tag}>`")]
    UnexpectedControlBranch { tag: String, branch: String },
    #[error("`<Match>` only accepts `<Case>` and an optional final `<Default>` child")]
    InvalidMatchChild,
    #[error("`<Case>` requires `is` to be a string literal")]
    InvalidMatchCaseValue,
    #[error("`<Head>` only accepts element children")]
    InvalidHeadChild,
    #[error("unsupported head tag `<{tag}>`")]
    UnsupportedHeadTag { tag: String },
    #[error("`<{tag}>` inside `<Head>` cannot have attributes")]
    HeadValueAttrsNotSupported { tag: String },
    #[error("`<{tag}>` inside `<Head>` requires exactly one text or expression child")]
    HeadValueRequiresSingleChild { tag: String },
    #[error("`<{tag}>` inside `<Head>` only supports text or expression children")]
    HeadValueInvalidChild { tag: String },
    #[error("`<{tag}>` inside `<Head>` only supports attributes for now")]
    HeadTagChildrenNotSupported { tag: String },
    #[error("`{first}` and `{second}` both map to class; use only one class attribute")]
    DuplicateClassAttr { first: String, second: String },
    #[error(
        "invalid state initializer `{expr_source}`; expected a literal value or `signal(...)`"
    )]
    InvalidStateInitializer { expr_source: String },
    #[error("component state `{name}` uses unsupported client-state type `{ty}`")]
    UnsupportedComponentStateType { name: String, ty: String },
    #[error("component parameter `{name}` has invalid type `{ty}`: {error}")]
    InvalidComponentParamType {
        name: String,
        ty: String,
        #[source]
        error: AxTypeParseError,
    },
    #[error("component parameter `{name}` has an invalid default `{expr_source}` for type `{ty}`")]
    InvalidComponentParamDefault {
        name: String,
        ty: String,
        expr_source: String,
    },
    #[error("invalid state type contract: {error}")]
    InvalidStateTypeContract {
        #[source]
        error: AxTypeParseError,
    },
    #[error("`{attr}` must bind to a declared `state` signal")]
    UnknownStateBinding { attr: String },
    #[error("`{attr}` only supports expression bindings such as `{{theme}}`")]
    InvalidStateBinding { attr: String },
    #[error("unsupported local state event `{attr}`; use on:click, on:input, or on:change")]
    UnsupportedStateEvent { attr: String },
    #[error("`{attr}` must mutate a declared state signal")]
    UnknownStateEvent { attr: String },
    #[error(
        "invalid local state event `{expr_source}`; use `state = literal`, `state += number`, `state -= number`, or `state = !state`"
    )]
    InvalidStateEvent { expr_source: String },
    #[error("unsupported reactive expression `{expr_source}`: {reason}")]
    UnsupportedReactiveExpression { expr_source: String, reason: String },
}

pub fn looks_like_ax_v2(input: &str) -> bool {
    let has_return_asx = input
        .lines()
        .map(str::trim_start)
        .any(|line| line.starts_with("return ASX"));

    input.lines().map(str::trim).any(|line| {
        !line.is_empty()
            && (line.starts_with("import ")
                || line.starts_with("use ")
                || (line.starts_with("page ") && line.ends_with('{'))
                || (line.starts_with("page ") && line.contains('(') && has_return_asx)
                || line.starts_with('<')
                || line.starts_with("</"))
    })
}

pub fn parse_ax_auto(input: &str) -> Result<AxDocument, AxAutoParseError> {
    if looks_like_ax_v2(input) {
        let file = parse_ax_v2(input)?;
        validate_ax_v2_semantics(&file)?;
        Ok(convert_ax_v2_file(&file)?)
    } else {
        Ok(parse_ax(input)?)
    }
}

pub fn convert_ax_v2_file(file: &AxFileV2) -> Result<AxDocument, AxConvertV2Error> {
    let mut head = AxHead::default();
    let mut body = Vec::new();
    let data_context = AxDataContext::from_v2_let_types(file)
        .map_err(|error| AxConvertV2Error::InvalidStateTypeContract { error })?;
    let state_bindings = collect_state_bindings(file, &data_context)?;
    let functions = file
        .functions
        .iter()
        .map(|function| convert_function_decl(function, &data_context))
        .collect::<Result<Vec<_>, _>>()?;
    let bindings = ConversionBindings {
        states: &state_bindings,
        functions: &functions,
    };

    for binding in &file.lets {
        let mut value = parse_v2_expr(&binding.value)?;
        if let Some(source_field) = &binding.source_field {
            value = value.member(source_field.clone());
        }
        body.push(AxStatement::data(binding.name.clone(), value));
    }

    for binding in &file.states {
        body.push(AxStatement::data(
            binding.name.clone(),
            parse_state_initializer(&binding.value)?.value,
        ));
    }

    for node in &file.body {
        match node {
            AxNodeV2::Element(element) if element.name == "Head" => {
                merge_head_element(&mut head, element)?
            }
            AxNodeV2::Element(element) => {
                body.push(AxStatement::component(convert_element(element, &bindings)?));
            }
            AxNodeV2::Text(text) => body.push(AxStatement::text(text.value.clone())),
            AxNodeV2::Expr(expr) => body.push(AxStatement::text(parse_v2_expr(&expr.source)?)),
        }
    }
    let components = file
        .components
        .iter()
        .map(|component| {
            convert_component_decl(component, &state_bindings, &functions, &data_context)
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(AxDocument {
        imports: file.imports.iter().map(convert_import_decl).collect(),
        functions,
        components,
        head,
        page: AxPage::with_params(
            file.page.name.clone(),
            file.page
                .params
                .iter()
                .map(|param| convert_component_param_decl(param, &data_context))
                .collect::<Result<Vec<_>, _>>()?,
            body,
        ),
    })
}

fn convert_import_decl(import_decl: &crate::ax_ast_v2::AxImportDecl) -> AxImport {
    AxImport::new(
        import_decl.bindings.iter().map(|binding| {
            crate::ax_ast::AxImportBinding::new(binding.imported.clone(), binding.local.clone())
        }),
        import_decl.source.clone(),
    )
}

fn convert_function_decl(
    function: &AxFunctionDeclV2,
    data_context: &AxDataContext,
) -> Result<AxFunctionDef, AxConvertV2Error> {
    Ok(AxFunctionDef::new(
        function.name.clone(),
        function
            .params
            .iter()
            .map(|param| convert_component_param_decl(param, data_context))
            .collect::<Result<Vec<_>, _>>()?,
        parse_v2_expr(&function.body)?,
    ))
}

fn convert_component_decl(
    component: &AxComponentDeclV2,
    state_bindings: &BTreeMap<String, StateBindingPlan>,
    functions: &[AxFunctionDef],
    data_context: &AxDataContext,
) -> Result<AxComponentDef, AxConvertV2Error> {
    let mut bindings = state_bindings.clone();
    let mut states = Vec::new();
    for (index, state) in component.states.iter().enumerate() {
        let initializer = parse_state_initializer(&state.value)?;
        let ty = state.ty.clone().unwrap_or(initializer.ty);
        if !is_supported_client_state_type(&ty) {
            return Err(AxConvertV2Error::UnsupportedComponentStateType {
                name: state.name.clone(),
                ty,
            });
        }
        if !state_initializer_matches_type(&initializer.value, &ty, data_context) {
            return Err(AxConvertV2Error::InvalidStateInitializer {
                expr_source: state.value.clone(),
            });
        }
        let signal = format!(
            "__ax_component_state__:{}:{}:{}",
            component.name,
            state.name,
            index + 1
        );
        bindings.insert(
            state.name.clone(),
            StateBindingPlan {
                signal_id: signal.clone(),
                ty: ty.clone(),
                literals: data_context.literal_union(&ty).map(<[_]>::to_vec),
                initial: state_event_literal(&initializer.value).ok_or_else(|| {
                    AxConvertV2Error::InvalidStateInitializer {
                        expr_source: state.value.clone(),
                    }
                })?,
            },
        );
        states.push(AxComponentStateDef::new(
            state.name.clone(),
            ty,
            initializer.value,
            signal,
        ));
    }

    let conversion_bindings = ConversionBindings {
        states: &bindings,
        functions,
    };
    Ok(AxComponentDef::with_states(
        component.name.clone(),
        component
            .params
            .iter()
            .map(|param| convert_component_param_decl(param, data_context))
            .collect::<Result<Vec<_>, _>>()?,
        states,
        convert_children(&component.body, &conversion_bindings)?,
    ))
}

fn convert_component_param_decl(
    param: &AxComponentParamDeclV2,
    data_context: &AxDataContext,
) -> Result<AxComponentParamDef, AxConvertV2Error> {
    let default = param.default.as_deref().map(parse_v2_expr).transpose()?;
    let Some(ty_source) = param.ty.as_deref() else {
        return Ok(match default {
            Some(default) => AxComponentParamDef::with_default(param.name.clone(), default),
            None => AxComponentParamDef::new(param.name.clone()),
        });
    };
    let ty = AxType::parse_annotation(ty_source).map_err(|error| {
        AxConvertV2Error::InvalidComponentParamType {
            name: param.name.clone(),
            ty: ty_source.to_string(),
            error,
        }
    })?;
    if let Some(default) = &default {
        if is_static_param_default(default) && !data_context.accepts_state_initializer(&ty, default)
        {
            return Err(AxConvertV2Error::InvalidComponentParamDefault {
                name: param.name.clone(),
                ty: ty_source.to_string(),
                expr_source: param.default.clone().unwrap_or_default(),
            });
        }
    }
    let literal_values = match &ty {
        AxType::Record(name) => data_context.literal_union(name).unwrap_or_default(),
        _ => &[],
    };
    Ok(match default {
        Some(default) => {
            AxComponentParamDef::typed_with_default(param.name.clone(), ty_source, default)
        }
        None => AxComponentParamDef::typed(param.name.clone(), ty_source),
    }
    .with_literal_values(literal_values.iter().cloned()))
}

fn is_static_param_default(expr: &AxExpr) -> bool {
    match expr {
        AxExpr::String(_) | AxExpr::Bool(_) | AxExpr::Number(_) | AxExpr::Float(_) => true,
        AxExpr::Identifier(name) => name == "null",
        AxExpr::List(items) => items.iter().all(is_static_param_default),
        AxExpr::Object(fields) => fields.values().all(is_static_param_default),
        _ => false,
    }
}

fn merge_head_element(head: &mut AxHead, element: &AxElementNode) -> Result<(), AxConvertV2Error> {
    for child in &element.children {
        let AxNodeV2::Element(tag) = child else {
            return Err(AxConvertV2Error::InvalidHeadChild);
        };

        match tag.name.as_str() {
            "Title" => head.title = Some(convert_head_value(tag)?),
            "Theme" => convert_theme_head_tag(head, tag)?,
            "Meta" => head.metas.push(convert_head_tag(tag)?),
            "Link" => head.links.push(convert_head_tag(tag)?),
            "Script" => head.scripts.push(convert_head_tag(tag)?),
            other => {
                return Err(AxConvertV2Error::UnsupportedHeadTag {
                    tag: other.to_string(),
                });
            }
        }
    }

    Ok(())
}

fn convert_theme_head_tag(
    head: &mut AxHead,
    element: &AxElementNode,
) -> Result<(), AxConvertV2Error> {
    if element.attrs.is_empty() {
        head.theme = Some(convert_head_value(element)?);
        return Ok(());
    }

    if !element.children.is_empty() {
        return Err(AxConvertV2Error::HeadTagChildrenNotSupported {
            tag: element.name.clone(),
        });
    }

    for attr in &element.attrs {
        match attr.name.as_str() {
            "default" => head.theme = Some(convert_attr_value(&attr.value)?),
            "storageKey" => head.theme_storage_key = Some(convert_attr_value(&attr.value)?),
            "preflight" => {
                head.theme_preflight = head_attr_is_truthy(attr)?;
            }
            _ => {}
        }
    }

    Ok(())
}

fn head_attr_is_truthy(attr: &AxAttributeNode) -> Result<bool, AxConvertV2Error> {
    Ok(match &attr.value {
        AxAttributeValue::String(value) => value != "false",
        AxAttributeValue::Expr(source) => match parse_v2_expr(source)? {
            AxExpr::Bool(value) => value,
            AxExpr::String(value) => value != "false",
            _ => true,
        },
    })
}

fn convert_head_value(element: &AxElementNode) -> Result<AxExpr, AxConvertV2Error> {
    if !element.attrs.is_empty() {
        return Err(AxConvertV2Error::HeadValueAttrsNotSupported {
            tag: element.name.clone(),
        });
    }

    if element.children.len() != 1 {
        return Err(AxConvertV2Error::HeadValueRequiresSingleChild {
            tag: element.name.clone(),
        });
    }

    match &element.children[0] {
        AxNodeV2::Text(text) => Ok(AxExpr::string(text.value.clone())),
        AxNodeV2::Expr(expr) => parse_v2_expr(&expr.source),
        AxNodeV2::Element(_) => Err(AxConvertV2Error::HeadValueInvalidChild {
            tag: element.name.clone(),
        }),
    }
}

fn convert_head_tag(element: &AxElementNode) -> Result<AxHeadTag, AxConvertV2Error> {
    if !element.children.is_empty() {
        return Err(AxConvertV2Error::HeadTagChildrenNotSupported {
            tag: element.name.clone(),
        });
    }

    let attrs = element
        .attrs
        .iter()
        .map(convert_prop)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(AxHeadTag::new(attrs))
}

#[derive(Clone, Copy)]
struct ConversionBindings<'a> {
    states: &'a BTreeMap<String, StateBindingPlan>,
    functions: &'a [AxFunctionDef],
}

fn convert_element(
    element: &AxElementNode,
    bindings: &ConversionBindings<'_>,
) -> Result<AxComponent, AxConvertV2Error> {
    if element.name == "Each" {
        return convert_each_element(element, bindings);
    }
    if element.name == "If" {
        return convert_if_element(element, bindings);
    }
    if element.name == "Match" {
        return convert_match_element(element, bindings);
    }

    let mut component = AxComponent::new(element.name.clone());
    let mut class_attr_seen: Option<&str> = None;

    let mut reactive_expression_index = 0usize;
    for attr in &element.attrs {
        match attr.name.as_str() {
            name if name.starts_with("bind:") => {
                component = apply_state_binding_attr(component, attr, bindings.states)?;
            }
            name if name.starts_with("on:") => {
                component = apply_state_event_attr(component, attr, bindings.states)?;
            }
            "class" | "className" => {
                if let Some(first) = class_attr_seen {
                    return Err(AxConvertV2Error::DuplicateClassAttr {
                        first: first.to_string(),
                        second: attr.name.clone(),
                    });
                }
                class_attr_seen = Some(attr.name.as_str());
                component = component.class(convert_attr_value(&attr.value)?);
            }
            "recipe" => component = component.recipe(convert_attr_value(&attr.value)?),
            _ => {
                let mut value = convert_attr_value(&attr.value)?;
                if is_reactive_boolean_attr(&attr.name) {
                    if let Some(plan) = compile_reactive_expression(&value, bindings)? {
                        value = plan.expression.clone();
                        component = apply_reactive_expression_metadata(
                            component,
                            reactive_expression_index,
                            &format!("boolean:{}", attr.name),
                            &plan,
                        );
                        reactive_expression_index += 1;
                    }
                }
                component = component.prop(attr.name.clone(), value);
            }
        }
    }

    if element.children.is_empty() {
        return Ok(component);
    }

    if element
        .children
        .iter()
        .all(|child| matches!(child, AxNodeV2::Element(_)))
    {
        let body = element
            .children
            .iter()
            .map(|child| convert_child(child, bindings))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(component.block(body));
    }

    if element.children.len() == 1 {
        return match &element.children[0] {
            AxNodeV2::Text(text) => Ok(component.inline(AxExpr::string(text.value.clone()))),
            AxNodeV2::Expr(expr) => {
                let value = parse_v2_expr(&expr.source)?;
                if let AxExpr::Identifier(name) = &value {
                    if let Some(binding) = bindings.states.get(name) {
                        component = apply_state_read_binding(component, binding);
                    }
                } else if let Some(plan) = compile_reactive_expression(&value, bindings)? {
                    return Ok(
                        component.block([AxStatement::component(reactive_text_component(&plan))])
                    );
                }
                Ok(component.inline(value))
            }
            AxNodeV2::Element(child) => {
                Ok(component.block([AxStatement::component(convert_element(child, bindings)?)]))
            }
        };
    }

    Ok(component.block(convert_children(&element.children, bindings)?))
}

fn convert_children(
    children: &[AxNodeV2],
    bindings: &ConversionBindings<'_>,
) -> Result<Vec<AxStatement>, AxConvertV2Error> {
    children
        .iter()
        .map(|child| convert_child(child, bindings))
        .collect()
}

fn convert_child(
    child: &AxNodeV2,
    bindings: &ConversionBindings<'_>,
) -> Result<AxStatement, AxConvertV2Error> {
    match child {
        AxNodeV2::Element(element) if element.name == "Each" => {
            convert_each_statement(element, bindings)
        }
        AxNodeV2::Element(element) if element.name == "If" => {
            convert_if_statement(element, bindings)
        }
        AxNodeV2::Element(element) if element.name == "Match" => {
            convert_match_statement(element, bindings)
        }
        AxNodeV2::Element(element)
            if matches!(element.name.as_str(), "Else" | "Empty" | "Case" | "Default") =>
        {
            Err(AxConvertV2Error::UnexpectedControlBranch {
                tag: "control-flow".to_string(),
                branch: element.name.clone(),
            })
        }
        AxNodeV2::Element(element) => {
            Ok(AxStatement::component(convert_element(element, bindings)?))
        }
        AxNodeV2::Text(text) => Ok(AxStatement::text(text.value.clone())),
        AxNodeV2::Expr(expr) => {
            let value = parse_v2_expr(&expr.source)?;
            if let Some(plan) = compile_reactive_expression(&value, bindings)? {
                Ok(AxStatement::component(reactive_text_component(&plan)))
            } else {
                Ok(AxStatement::text(value))
            }
        }
    }
}

fn convert_each_statement(
    element: &AxElementNode,
    bindings: &ConversionBindings<'_>,
) -> Result<AxStatement, AxConvertV2Error> {
    let binding = control_binding_attr(element, &["as", "item"])?;
    let source = control_expr_attr(element, &["items", "in", "of"])?;
    let (body, empty_body) = split_each_children(element, bindings)?;
    Ok(AxStatement::Each(
        AxEachBlock::new(binding, source, body).empty(empty_body),
    ))
}

fn convert_if_statement(
    element: &AxElementNode,
    bindings: &ConversionBindings<'_>,
) -> Result<AxStatement, AxConvertV2Error> {
    let condition = control_expr_attr(element, &["when", "condition"])?;
    let (body, else_body) = split_if_children(element, bindings)?;
    if let Some(plan) = state_condition_plan(&condition, bindings.states) {
        return Ok(AxStatement::component(state_if_component(
            plan, body, else_body,
        )));
    }
    Ok(AxStatement::If(
        AxIfBlock::new(condition, body).else_body(else_body),
    ))
}

fn convert_match_statement(
    element: &AxElementNode,
    bindings: &ConversionBindings<'_>,
) -> Result<AxStatement, AxConvertV2Error> {
    let value = control_expr_attr(element, &["value"])?;
    let (cases, default_body) = split_match_children(element, bindings)?;
    if let AxExpr::Identifier(name) = &value {
        if let Some(binding) = bindings.states.get(name) {
            return Ok(AxStatement::component(state_match_component(
                name,
                binding,
                cases,
                default_body,
            )));
        }
    }
    let mut match_block = AxMatchBlock::new(value, cases);
    if let Some(default_body) = default_body {
        match_block = match_block.default_body(default_body);
    }
    Ok(AxStatement::Match(match_block))
}

fn split_match_children(
    element: &AxElementNode,
    bindings: &ConversionBindings<'_>,
) -> Result<(Vec<AxMatchCase>, Option<Vec<AxStatement>>), AxConvertV2Error> {
    let mut cases = Vec::new();
    let mut default_body = None;

    for child in &element.children {
        let AxNodeV2::Element(branch) = child else {
            return Err(AxConvertV2Error::InvalidMatchChild);
        };
        match branch.name.as_str() {
            "Case" => {
                if default_body.is_some() {
                    return Err(AxConvertV2Error::ControlBranchMustBeLast {
                        tag: "Match".to_string(),
                        branch: "Default".to_string(),
                    });
                }
                let case_value = match control_expr_attr(branch, &["is"])? {
                    AxExpr::String(value) => value,
                    _ => return Err(AxConvertV2Error::InvalidMatchCaseValue),
                };
                cases.push(AxMatchCase::new(
                    case_value,
                    convert_children(&branch.children, bindings)?,
                ));
            }
            "Default" => {
                if !branch.attrs.is_empty() {
                    return Err(AxConvertV2Error::ControlBranchAttrsNotSupported {
                        tag: "Match".to_string(),
                        branch: "Default".to_string(),
                    });
                }
                if default_body.is_some() {
                    return Err(AxConvertV2Error::DuplicateControlBranch {
                        tag: "Match".to_string(),
                        branch: "Default".to_string(),
                    });
                }
                default_body = Some(convert_children(&branch.children, bindings)?);
            }
            _ => return Err(AxConvertV2Error::InvalidMatchChild),
        }
    }

    Ok((cases, default_body))
}

fn state_match_component(
    state_name: &str,
    binding: &StateBindingPlan,
    cases: Vec<AxMatchCase>,
    default_body: Option<Vec<AxStatement>>,
) -> AxComponent {
    let initial = binding.initial.as_str();
    let has_initial_case = cases.iter().any(|case| case.value == initial);
    let mut branches = cases
        .into_iter()
        .map(|case| {
            let active = case.value == initial;
            let mut branch = AxComponent::new("__AxStateMatchCase")
                .prop("case", case.value)
                .block(case.body);
            if !active {
                branch = branch.prop("hidden", true);
            }
            AxStatement::component(branch)
        })
        .collect::<Vec<_>>();

    if let Some(default_body) = default_body {
        let mut branch = AxComponent::new("__AxStateMatchDefault").block(default_body);
        if has_initial_case {
            branch = branch.prop("hidden", true);
        }
        branches.push(AxStatement::component(branch));
    }

    let mut component = AxComponent::new("__AxStateMatch")
        .prop("data-ax-state-match-name", state_name)
        .prop("data-ax-state-match-signal", binding.signal_id.clone())
        .prop("data-ax-state-match-type", binding.ty.clone())
        .prop("data-ax-state-match-initial", binding.initial.clone())
        .block(branches);
    if let Some(literals) = &binding.literals {
        component = component.prop(
            "data-ax-state-match-literals",
            serde_json::to_string(literals)
                .expect("serializing string literal union metadata cannot fail"),
        );
    }
    component
}

fn state_if_component(
    plan: StateConditionPlan,
    body: Vec<AxStatement>,
    else_body: Vec<AxStatement>,
) -> AxComponent {
    let active = evaluate_initial_state_condition(&plan);
    let mut component = AxComponent::new("__AxStateIf")
        .prop("data-ax-state-if-signal", plan.signal_id)
        .prop("data-ax-state-if-op", plan.op)
        .prop("data-ax-state-if-type", plan.ty)
        .prop("data-ax-state-if-initial", plan.initial);
    if let Some(value) = plan.value {
        component = component.prop("data-ax-state-if-value", value);
    }

    let mut then_branch = AxComponent::new("__AxStateIfThen").block(body);
    if !active {
        then_branch = then_branch.prop("hidden", true);
    }
    let mut branches = vec![AxStatement::component(then_branch)];
    if !else_body.is_empty() {
        let mut else_branch = AxComponent::new("__AxStateIfElse").block(else_body);
        if active {
            else_branch = else_branch.prop("hidden", true);
        }
        branches.push(AxStatement::component(else_branch));
    }
    component.block(branches)
}

fn convert_each_element(
    element: &AxElementNode,
    bindings: &ConversionBindings<'_>,
) -> Result<AxComponent, AxConvertV2Error> {
    Ok(AxComponent::fragment([convert_each_statement(
        element, bindings,
    )?]))
}

fn convert_if_element(
    element: &AxElementNode,
    bindings: &ConversionBindings<'_>,
) -> Result<AxComponent, AxConvertV2Error> {
    Ok(AxComponent::fragment([convert_if_statement(
        element, bindings,
    )?]))
}

fn convert_match_element(
    element: &AxElementNode,
    bindings: &ConversionBindings<'_>,
) -> Result<AxComponent, AxConvertV2Error> {
    Ok(AxComponent::fragment([convert_match_statement(
        element, bindings,
    )?]))
}

fn control_expr_attr(element: &AxElementNode, names: &[&str]) -> Result<AxExpr, AxConvertV2Error> {
    let Some(attr) = element
        .attrs
        .iter()
        .find(|attr| names.iter().any(|name| attr.name == *name))
    else {
        return Err(AxConvertV2Error::MissingControlAttr {
            tag: element.name.clone(),
            attr: names[0].to_string(),
        });
    };

    convert_attr_value(&attr.value)
}

fn control_binding_attr(
    element: &AxElementNode,
    names: &[&str],
) -> Result<String, AxConvertV2Error> {
    let Some(attr) = element
        .attrs
        .iter()
        .find(|attr| names.iter().any(|name| attr.name == *name))
    else {
        return Err(AxConvertV2Error::MissingControlAttr {
            tag: element.name.clone(),
            attr: names[0].to_string(),
        });
    };

    match &attr.value {
        AxAttributeValue::String(value) => Ok(value.clone()),
        AxAttributeValue::Expr(source) => match parse_v2_expr(source)? {
            AxExpr::Identifier(name) => Ok(name),
            _ => Err(AxConvertV2Error::InvalidBindingAttr {
                tag: element.name.clone(),
                attr: attr.name.clone(),
            }),
        },
    }
}

fn split_each_children(
    element: &AxElementNode,
    bindings: &ConversionBindings<'_>,
) -> Result<(Vec<AxStatement>, Vec<AxStatement>), AxConvertV2Error> {
    let mut body = Vec::new();
    let mut empty_body = None;

    for child in &element.children {
        match child {
            AxNodeV2::Element(branch) if branch.name == "Empty" => {
                if !branch.attrs.is_empty() {
                    return Err(AxConvertV2Error::ControlBranchAttrsNotSupported {
                        tag: element.name.clone(),
                        branch: "Empty".to_string(),
                    });
                }
                if empty_body.is_some() {
                    return Err(AxConvertV2Error::DuplicateControlBranch {
                        tag: element.name.clone(),
                        branch: "Empty".to_string(),
                    });
                }
                empty_body = Some(convert_children(&branch.children, bindings)?);
            }
            _ => body.push(convert_child(child, bindings)?),
        }
    }

    Ok((body, empty_body.unwrap_or_default()))
}

fn split_if_children(
    element: &AxElementNode,
    bindings: &ConversionBindings<'_>,
) -> Result<(Vec<AxStatement>, Vec<AxStatement>), AxConvertV2Error> {
    split_if_children_from_slice(&element.name, &element.children, bindings)
}

fn split_if_children_from_slice(
    tag: &str,
    children: &[AxNodeV2],
    bindings: &ConversionBindings<'_>,
) -> Result<(Vec<AxStatement>, Vec<AxStatement>), AxConvertV2Error> {
    let mut body = Vec::new();
    let mut index = 0;

    while index < children.len() {
        match &children[index] {
            AxNodeV2::Element(branch) if branch.name == "Else" => {
                if !branch.attrs.is_empty() {
                    return Err(AxConvertV2Error::ControlBranchAttrsNotSupported {
                        tag: tag.to_string(),
                        branch: "Else".to_string(),
                    });
                }
                if index + 1 != children.len() {
                    return Err(AxConvertV2Error::ControlBranchMustBeLast {
                        tag: tag.to_string(),
                        branch: "Else".to_string(),
                    });
                }
                return Ok((body, convert_children(&branch.children, bindings)?));
            }
            AxNodeV2::Element(branch) if branch.name == "ElseIf" => {
                let condition = control_expr_attr(branch, &["when", "condition"])?;
                let nested_body = convert_children(&branch.children, bindings)?;
                let nested_else_body =
                    convert_if_tail_with_state(tag, &children[index + 1..], bindings)?;
                return Ok((
                    body,
                    vec![AxStatement::If(
                        AxIfBlock::new(condition, nested_body).else_body(nested_else_body),
                    )],
                ));
            }
            child => {
                body.push(convert_child(child, bindings)?);
                index += 1;
            }
        }
    }

    Ok((body, Vec::new()))
}

fn convert_if_tail_with_state(
    tag: &str,
    tail: &[AxNodeV2],
    bindings: &ConversionBindings<'_>,
) -> Result<Vec<AxStatement>, AxConvertV2Error> {
    let Some(first) = tail.first() else {
        return Ok(Vec::new());
    };

    match first {
        AxNodeV2::Element(branch) if branch.name == "Else" => {
            if !branch.attrs.is_empty() {
                return Err(AxConvertV2Error::ControlBranchAttrsNotSupported {
                    tag: tag.to_string(),
                    branch: "Else".to_string(),
                });
            }
            if tail.len() > 1 {
                return Err(AxConvertV2Error::ControlBranchMustBeLast {
                    tag: tag.to_string(),
                    branch: "Else".to_string(),
                });
            }
            convert_children(&branch.children, bindings)
        }
        AxNodeV2::Element(branch) if branch.name == "ElseIf" => {
            let condition = control_expr_attr(branch, &["when", "condition"])?;
            let nested_body = convert_children(&branch.children, bindings)?;
            let nested_else_body = convert_if_tail_with_state(tag, &tail[1..], bindings)?;
            Ok(vec![AxStatement::If(
                AxIfBlock::new(condition, nested_body).else_body(nested_else_body),
            )])
        }
        AxNodeV2::Element(branch) if branch.name == "Empty" => {
            Err(AxConvertV2Error::UnexpectedControlBranch {
                tag: tag.to_string(),
                branch: branch.name.clone(),
            })
        }
        AxNodeV2::Element(_) | AxNodeV2::Text(_) | AxNodeV2::Expr(_) => {
            Err(AxConvertV2Error::ControlBranchMustBeLast {
                tag: tag.to_string(),
                branch: "ElseIf".to_string(),
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StateBindingPlan {
    signal_id: String,
    ty: String,
    literals: Option<Vec<String>>,
    initial: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReactiveExpressionPlan {
    expression: AxExpr,
    program: Vec<u8>,
    signals: Vec<String>,
    types: Vec<String>,
    initials: Vec<String>,
}

const AX_EXPRESSION_MAGIC: &[u8; 4] = b"AXE\x01";
const AX_EXPR_PUSH_DEPENDENCY: u8 = 0;
const AX_EXPR_PUSH_NULL: u8 = 1;
const AX_EXPR_PUSH_STRING: u8 = 2;
const AX_EXPR_PUSH_BOOL: u8 = 3;
const AX_EXPR_PUSH_FLOAT: u8 = 4;
const AX_EXPR_PUSH_INT: u8 = 5;
const AX_EXPR_NOT: u8 = 10;
const AX_EXPR_NEG: u8 = 11;
const AX_EXPR_ADD: u8 = 20;
const AX_EXPR_SUB: u8 = 21;
const AX_EXPR_MUL: u8 = 22;
const AX_EXPR_DIV: u8 = 23;
const AX_EXPR_REM: u8 = 24;
const AX_EXPR_EQ: u8 = 25;
const AX_EXPR_NE: u8 = 26;
const AX_EXPR_GT: u8 = 27;
const AX_EXPR_GE: u8 = 28;
const AX_EXPR_LT: u8 = 29;
const AX_EXPR_LE: u8 = 30;
const AX_EXPR_IN: u8 = 31;
const AX_EXPR_AND: u8 = 32;
const AX_EXPR_OR: u8 = 33;
const AX_EXPR_FALLBACK: u8 = 34;
const AX_EXPR_INDEX: u8 = 40;
const AX_EXPR_MEMBER: u8 = 41;
const AX_EXPR_OPTIONAL_MEMBER: u8 = 42;
const AX_EXPR_BUILD_LIST: u8 = 50;
const AX_EXPR_BUILD_OBJECT: u8 = 51;
const AX_EXPRESSION_COLLECTION_MAX_ITEMS: usize = 1024;

fn is_reactive_boolean_attr(name: &str) -> bool {
    matches!(
        name,
        "disabled"
            | "checked"
            | "selected"
            | "hidden"
            | "required"
            | "readonly"
            | "multiple"
            | "open"
    )
}

fn reactive_text_component(plan: &ReactiveExpressionPlan) -> AxComponent {
    apply_reactive_expression_metadata(
        AxComponent::new("__AxReactiveText").inline(plan.expression.clone()),
        0,
        "text",
        plan,
    )
}

fn apply_reactive_expression_metadata(
    component: AxComponent,
    index: usize,
    target: &str,
    plan: &ReactiveExpressionPlan,
) -> AxComponent {
    let signals = serde_json::to_string(&plan.signals)
        .expect("serializing reactive signal metadata cannot fail");
    let types =
        serde_json::to_string(&plan.types).expect("serializing reactive type metadata cannot fail");
    let initials = serde_json::to_string(&plan.initials)
        .expect("serializing reactive initial metadata cannot fail");
    component
        .prop("data-ax-expression-protocol", "ax-expression/1")
        .prop("data-ax-expression-count", (index + 1).to_string())
        .prop(
            format!("data-ax-expression-{index}-program"),
            encode_hex(&plan.program),
        )
        .prop(format!("data-ax-expression-{index}-signals"), signals)
        .prop(format!("data-ax-expression-{index}-types"), types)
        .prop(format!("data-ax-expression-{index}-initials"), initials)
        .prop(
            format!("data-ax-expression-{index}-target"),
            target.to_string(),
        )
}

fn compile_reactive_expression(
    expr: &AxExpr,
    bindings: &ConversionBindings<'_>,
) -> Result<Option<ReactiveExpressionPlan>, AxConvertV2Error> {
    let directly_reactive = expression_references_state(expr, bindings.states);
    let expanded = match expand_pure_reactive_expression(
        expr,
        bindings,
        &BTreeMap::new(),
        &mut Vec::new(),
        &mut Vec::new(),
    ) {
        Ok(expanded) => expanded,
        Err(reason) if directly_reactive => {
            return Err(AxConvertV2Error::UnsupportedReactiveExpression {
                expr_source: format_expr(expr),
                reason,
            });
        }
        Err(_) => return Ok(None),
    };
    if !expression_references_state(&expanded, bindings.states) {
        return Ok(None);
    }
    let mut dependencies = Vec::<String>::new();
    let mut program = AX_EXPRESSION_MAGIC.to_vec();
    compile_reactive_expression_node(&expanded, bindings, &mut dependencies, &mut program)
        .map_err(|reason| AxConvertV2Error::UnsupportedReactiveExpression {
            expr_source: format_expr(expr),
            reason,
        })?;
    let signals = dependencies
        .iter()
        .map(|name| bindings.states[name].signal_id.clone())
        .collect();
    let types = dependencies
        .iter()
        .map(|name| bindings.states[name].ty.clone())
        .collect();
    let initials = dependencies
        .iter()
        .map(|name| bindings.states[name].initial.clone())
        .collect();
    Ok(Some(ReactiveExpressionPlan {
        expression: expanded,
        program,
        signals,
        types,
        initials,
    }))
}

fn expand_pure_reactive_expression(
    expr: &AxExpr,
    bindings: &ConversionBindings<'_>,
    locals: &BTreeMap<String, AxExpr>,
    local_stack: &mut Vec<String>,
    call_stack: &mut Vec<String>,
) -> Result<AxExpr, String> {
    Ok(match expr {
        AxExpr::Identifier(name) if locals.contains_key(name) => {
            if local_stack.contains(name) {
                return Err(format!(
                    "pure function parameter `{name}` resolves recursively"
                ));
            }
            local_stack.push(name.clone());
            let value = expand_pure_reactive_expression(
                &locals[name],
                bindings,
                locals,
                local_stack,
                call_stack,
            )?;
            local_stack.pop();
            value
        }
        AxExpr::Unary { op, expr } => AxExpr::Unary {
            op: *op,
            expr: Box::new(expand_pure_reactive_expression(
                expr,
                bindings,
                locals,
                local_stack,
                call_stack,
            )?),
        },
        AxExpr::Binary { op, left, right } => AxExpr::Binary {
            op: *op,
            left: Box::new(expand_pure_reactive_expression(
                left,
                bindings,
                locals,
                local_stack,
                call_stack,
            )?),
            right: Box::new(expand_pure_reactive_expression(
                right,
                bindings,
                locals,
                local_stack,
                call_stack,
            )?),
        },
        AxExpr::Index { object, index } => AxExpr::Index {
            object: Box::new(expand_pure_reactive_expression(
                object,
                bindings,
                locals,
                local_stack,
                call_stack,
            )?),
            index: Box::new(expand_pure_reactive_expression(
                index,
                bindings,
                locals,
                local_stack,
                call_stack,
            )?),
        },
        AxExpr::Member { object, property } => AxExpr::Member {
            object: Box::new(expand_pure_reactive_expression(
                object,
                bindings,
                locals,
                local_stack,
                call_stack,
            )?),
            property: property.clone(),
        },
        AxExpr::OptionalMember { object, property } => AxExpr::OptionalMember {
            object: Box::new(expand_pure_reactive_expression(
                object,
                bindings,
                locals,
                local_stack,
                call_stack,
            )?),
            property: property.clone(),
        },
        AxExpr::Call { path, args } => {
            let [name] = path.as_slice() else {
                return Err(format!("`{}` is not a local pure function", path.join(".")));
            };
            let Some(function) = bindings
                .functions
                .iter()
                .rev()
                .find(|function| function.name == *name)
            else {
                return Err(format!("`{name}` is not a local pure function"));
            };
            if call_stack.contains(name) {
                let mut cycle = call_stack.clone();
                cycle.push(name.clone());
                return Err(format!(
                    "recursive pure function calls are not supported (`{}`)",
                    cycle.join(" -> ")
                ));
            }
            if args.len() > function.params.len() {
                return Err(format!(
                    "pure function `{name}` expects at most {} arguments, received {}",
                    function.params.len(),
                    args.len()
                ));
            }
            let expanded_args = args
                .iter()
                .map(|arg| {
                    expand_pure_reactive_expression(arg, bindings, locals, local_stack, call_stack)
                })
                .collect::<Result<Vec<_>, _>>()?;

            call_stack.push(name.clone());
            let mut function_locals = BTreeMap::new();
            for (index, param) in function.params.iter().enumerate() {
                let value = if let Some(value) = expanded_args.get(index) {
                    value.clone()
                } else if let Some(default) = &param.default {
                    expand_pure_reactive_expression(
                        default,
                        bindings,
                        &function_locals,
                        local_stack,
                        call_stack,
                    )?
                } else {
                    call_stack.pop();
                    return Err(format!(
                        "pure function `{name}` is missing required argument `{}`",
                        param.name
                    ));
                };
                function_locals.insert(param.name.clone(), value);
            }
            let result = expand_pure_reactive_expression(
                &function.body,
                bindings,
                &function_locals,
                local_stack,
                call_stack,
            );
            call_stack.pop();
            result?
        }
        AxExpr::List(items) => AxExpr::List(
            items
                .iter()
                .map(|item| {
                    expand_pure_reactive_expression(item, bindings, locals, local_stack, call_stack)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        AxExpr::Object(fields) => AxExpr::Object(
            fields
                .iter()
                .map(|(name, value)| {
                    Ok((
                        name.clone(),
                        expand_pure_reactive_expression(
                            value,
                            bindings,
                            locals,
                            local_stack,
                            call_stack,
                        )?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()?,
        ),
        value => value.clone(),
    })
}

fn expression_references_state(
    expr: &AxExpr,
    state_bindings: &BTreeMap<String, StateBindingPlan>,
) -> bool {
    match expr {
        AxExpr::Identifier(name) => state_bindings.contains_key(name),
        AxExpr::Unary { expr, .. } => expression_references_state(expr, state_bindings),
        AxExpr::Binary { left, right, .. } => {
            expression_references_state(left, state_bindings)
                || expression_references_state(right, state_bindings)
        }
        AxExpr::Index { object, index } => {
            expression_references_state(object, state_bindings)
                || expression_references_state(index, state_bindings)
        }
        AxExpr::Member { object, .. } | AxExpr::OptionalMember { object, .. } => {
            expression_references_state(object, state_bindings)
        }
        AxExpr::Call { args, .. } => args
            .iter()
            .any(|arg| expression_references_state(arg, state_bindings)),
        AxExpr::List(items) => items
            .iter()
            .any(|item| expression_references_state(item, state_bindings)),
        AxExpr::Object(fields) => fields
            .values()
            .any(|value| expression_references_state(value, state_bindings)),
        AxExpr::String(_) | AxExpr::Number(_) | AxExpr::Float(_) | AxExpr::Bool(_) => false,
    }
}

fn compile_reactive_expression_node(
    expr: &AxExpr,
    bindings: &ConversionBindings<'_>,
    dependencies: &mut Vec<String>,
    program: &mut Vec<u8>,
) -> Result<(), String> {
    match expr {
        AxExpr::String(value) => {
            program.push(AX_EXPR_PUSH_STRING);
            push_program_string(program, value)?;
        }
        AxExpr::Number(value) => {
            program.push(AX_EXPR_PUSH_INT);
            program.extend_from_slice(&value.to_le_bytes());
        }
        AxExpr::Float(value) => {
            program.push(AX_EXPR_PUSH_FLOAT);
            program.extend_from_slice(&value.get().to_le_bytes());
        }
        AxExpr::Bool(value) => {
            program.push(AX_EXPR_PUSH_BOOL);
            program.push(u8::from(*value));
        }
        AxExpr::Identifier(name) if name == "null" => program.push(AX_EXPR_PUSH_NULL),
        AxExpr::Identifier(name) => {
            if !bindings.states.contains_key(name) {
                return Err(format!("`{name}` is not reactive state"));
            }
            let index = if let Some(index) = dependencies.iter().position(|item| item == name) {
                index
            } else {
                dependencies.push(name.clone());
                dependencies.len() - 1
            };
            let index = u16::try_from(index)
                .map_err(|_| "reactive expression has too many dependencies".to_string())?;
            program.push(AX_EXPR_PUSH_DEPENDENCY);
            program.extend_from_slice(&index.to_le_bytes());
        }
        AxExpr::Unary { op, expr } => {
            compile_reactive_expression_node(expr, bindings, dependencies, program)?;
            program.push(match op {
                AxUnaryOp::Not => AX_EXPR_NOT,
                AxUnaryOp::Neg => AX_EXPR_NEG,
            });
        }
        AxExpr::Binary { op, left, right } => {
            compile_reactive_expression_node(left, bindings, dependencies, program)?;
            compile_reactive_expression_node(right, bindings, dependencies, program)?;
            program.push(match op {
                AxBinaryOp::Add => AX_EXPR_ADD,
                AxBinaryOp::Sub => AX_EXPR_SUB,
                AxBinaryOp::Mul => AX_EXPR_MUL,
                AxBinaryOp::Div => AX_EXPR_DIV,
                AxBinaryOp::Rem => AX_EXPR_REM,
                AxBinaryOp::Eq => AX_EXPR_EQ,
                AxBinaryOp::Ne => AX_EXPR_NE,
                AxBinaryOp::Gt => AX_EXPR_GT,
                AxBinaryOp::Ge => AX_EXPR_GE,
                AxBinaryOp::Lt => AX_EXPR_LT,
                AxBinaryOp::Le => AX_EXPR_LE,
                AxBinaryOp::In => AX_EXPR_IN,
                AxBinaryOp::And => AX_EXPR_AND,
                AxBinaryOp::Or => AX_EXPR_OR,
                AxBinaryOp::Fallback => AX_EXPR_FALLBACK,
            });
        }
        AxExpr::Index { object, index } => {
            compile_reactive_expression_node(object, bindings, dependencies, program)?;
            compile_reactive_expression_node(index, bindings, dependencies, program)?;
            program.push(AX_EXPR_INDEX);
        }
        AxExpr::Member { object, property } | AxExpr::OptionalMember { object, property } => {
            compile_reactive_expression_node(object, bindings, dependencies, program)?;
            program.push(if matches!(expr, AxExpr::Member { .. }) {
                AX_EXPR_MEMBER
            } else {
                AX_EXPR_OPTIONAL_MEMBER
            });
            push_program_string(program, property)?;
        }
        AxExpr::Call { path, .. } => {
            return Err(format!(
                "pure function `{}` could not be fully inlined",
                path.join(".")
            ));
        }
        AxExpr::List(items) => {
            let count = reactive_collection_count(items.len(), "list")?;
            for item in items {
                compile_reactive_expression_node(item, bindings, dependencies, program)?;
            }
            program.push(AX_EXPR_BUILD_LIST);
            program.extend_from_slice(&count.to_le_bytes());
        }
        AxExpr::Object(fields) => {
            let count = reactive_collection_count(fields.len(), "object")?;
            for value in fields.values() {
                compile_reactive_expression_node(value, bindings, dependencies, program)?;
            }
            program.push(AX_EXPR_BUILD_OBJECT);
            program.extend_from_slice(&count.to_le_bytes());
            for name in fields.keys() {
                push_program_string(program, name)?;
            }
        }
    }
    Ok(())
}

fn reactive_collection_count(count: usize, kind: &str) -> Result<u16, String> {
    if count > AX_EXPRESSION_COLLECTION_MAX_ITEMS {
        return Err(format!(
            "reactive {kind} literal exceeds the {AX_EXPRESSION_COLLECTION_MAX_ITEMS}-item limit"
        ));
    }
    u16::try_from(count).map_err(|_| format!("reactive {kind} literal has too many items"))
}

fn push_program_string(program: &mut Vec<u8>, value: &str) -> Result<(), String> {
    let length = u32::try_from(value.len())
        .map_err(|_| "reactive expression string is too large".to_string())?;
    program.extend_from_slice(&length.to_le_bytes());
    program.extend_from_slice(value.as_bytes());
    Ok(())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StateInitializerPlan {
    value: AxExpr,
    ty: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StateConditionPlan {
    signal_id: String,
    ty: String,
    initial: String,
    op: String,
    value: Option<String>,
}

fn state_condition_plan(
    condition: &AxExpr,
    state_bindings: &BTreeMap<String, StateBindingPlan>,
) -> Option<StateConditionPlan> {
    match condition {
        AxExpr::Identifier(name) => state_bindings.get(name).map(|binding| StateConditionPlan {
            signal_id: binding.signal_id.clone(),
            ty: binding.ty.clone(),
            initial: binding.initial.clone(),
            op: "truthy".to_string(),
            value: None,
        }),
        AxExpr::Unary {
            op: AxUnaryOp::Not,
            expr,
        } => {
            let AxExpr::Identifier(name) = expr.as_ref() else {
                return None;
            };
            state_bindings.get(name).map(|binding| StateConditionPlan {
                signal_id: binding.signal_id.clone(),
                ty: binding.ty.clone(),
                initial: binding.initial.clone(),
                op: "falsy".to_string(),
                value: None,
            })
        }
        AxExpr::Binary { op, left, right } => {
            let AxExpr::Identifier(name) = left.as_ref() else {
                return None;
            };
            let binding = state_bindings.get(name)?;
            let value = state_event_literal(right)?;
            let op = match op {
                AxBinaryOp::Eq => "eq",
                AxBinaryOp::Ne => "ne",
                AxBinaryOp::Gt => "gt",
                AxBinaryOp::Ge => "ge",
                AxBinaryOp::Lt => "lt",
                AxBinaryOp::Le => "le",
                _ => return None,
            };
            Some(StateConditionPlan {
                signal_id: binding.signal_id.clone(),
                ty: binding.ty.clone(),
                initial: binding.initial.clone(),
                op: op.to_string(),
                value: Some(value),
            })
        }
        _ => None,
    }
}

fn evaluate_initial_state_condition(plan: &StateConditionPlan) -> bool {
    match plan.ty.as_str() {
        "Bool" => {
            let current = plan.initial == "true";
            match plan.op.as_str() {
                "truthy" => current,
                "falsy" => !current,
                "eq" => current == plan.value.as_deref().is_some_and(|value| value == "true"),
                "ne" => current != plan.value.as_deref().is_some_and(|value| value == "true"),
                _ => false,
            }
        }
        "Number" => {
            let current = plan.initial.parse::<i64>().unwrap_or_default();
            let expected = plan
                .value
                .as_deref()
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or_default();
            match plan.op.as_str() {
                "truthy" => current != 0,
                "falsy" => current == 0,
                "eq" => current == expected,
                "ne" => current != expected,
                "gt" => current > expected,
                "ge" => current >= expected,
                "lt" => current < expected,
                "le" => current <= expected,
                _ => false,
            }
        }
        _ => match plan.op.as_str() {
            "truthy" => !plan.initial.is_empty(),
            "falsy" => plan.initial.is_empty(),
            "eq" => plan.value.as_deref() == Some(plan.initial.as_str()),
            "ne" => plan.value.as_deref() != Some(plan.initial.as_str()),
            _ => false,
        },
    }
}

fn collect_state_bindings(
    file: &AxFileV2,
    data_context: &AxDataContext,
) -> Result<BTreeMap<String, StateBindingPlan>, AxConvertV2Error> {
    let mut bindings = BTreeMap::new();
    for (index, state) in file.states.iter().enumerate() {
        let initializer = parse_state_initializer(&state.value)?;
        let ty = state.ty.clone().unwrap_or_else(|| initializer.ty.clone());
        if !is_supported_client_state_type(&ty) {
            return Err(AxConvertV2Error::UnsupportedComponentStateType {
                name: state.name.clone(),
                ty,
            });
        }
        if !state_initializer_matches_type(&initializer.value, &ty, data_context) {
            return Err(AxConvertV2Error::InvalidStateInitializer {
                expr_source: state.value.clone(),
            });
        }
        bindings.insert(
            state.name.clone(),
            StateBindingPlan {
                signal_id: format!("root:{}:{}", state.name, index + 1),
                literals: data_context.literal_union(&ty).map(<[_]>::to_vec),
                ty,
                initial: state_event_literal(&initializer.value).ok_or_else(|| {
                    AxConvertV2Error::InvalidStateInitializer {
                        expr_source: state.value.clone(),
                    }
                })?,
            },
        );
    }
    Ok(bindings)
}

fn parse_state_initializer(source: &str) -> Result<StateInitializerPlan, AxConvertV2Error> {
    let expr = parse_v2_expr(source)?;

    let value = match expr {
        AxExpr::Call { path, args } if path.as_slice() == ["signal"] && args.len() == 1 => {
            args[0].clone()
        }
        AxExpr::Call { .. } => {
            return Err(AxConvertV2Error::InvalidStateInitializer {
                expr_source: source.to_string(),
            });
        }
        other => other,
    };

    Ok(StateInitializerPlan {
        ty: infer_state_type(&value),
        value,
    })
}

fn infer_state_type(value: &AxExpr) -> String {
    match value {
        AxExpr::String(_) => "String",
        AxExpr::Bool(_) => "Bool",
        AxExpr::Number(_) => "Number",
        AxExpr::Float(_) => "Float",
        AxExpr::List(_) => "List<Unknown>",
        AxExpr::Object(_) => "Json",
        _ => "Unknown",
    }
    .to_string()
}

fn is_supported_client_state_type(source: &str) -> bool {
    AxType::parse_annotation(source)
        .as_ref()
        .is_ok_and(|ty| ty.supports_client_state())
}

fn state_initializer_matches_type(
    value: &AxExpr,
    source: &str,
    data_context: &AxDataContext,
) -> bool {
    AxType::parse_annotation(source)
        .as_ref()
        .is_ok_and(|ty| data_context.accepts_state_initializer(ty, value))
}

fn apply_state_binding_attr(
    mut component: AxComponent,
    attr: &AxAttributeNode,
    state_bindings: &BTreeMap<String, StateBindingPlan>,
) -> Result<AxComponent, AxConvertV2Error> {
    let bind_target = attr.name.trim_start_matches("bind:");
    if bind_target.is_empty() {
        return Err(AxConvertV2Error::InvalidStateBinding {
            attr: attr.name.clone(),
        });
    }
    if !matches!(bind_target, "value" | "checked" | "text") {
        return Err(AxConvertV2Error::InvalidStateBinding {
            attr: attr.name.clone(),
        });
    }

    let AxAttributeValue::Expr(source) = &attr.value else {
        return Err(AxConvertV2Error::InvalidStateBinding {
            attr: attr.name.clone(),
        });
    };
    let AxExpr::Identifier(name) = parse_v2_expr(source)? else {
        return Err(AxConvertV2Error::InvalidStateBinding {
            attr: attr.name.clone(),
        });
    };
    let Some(binding) = state_bindings.get(&name) else {
        return Err(AxConvertV2Error::UnknownStateBinding {
            attr: attr.name.clone(),
        });
    };

    component = component
        .prop("data-ax-signal", AxExpr::string(binding.signal_id.clone()))
        .prop("data-ax-bind", AxExpr::string(bind_target.to_string()))
        .prop("data-ax-bind-protocol", AxExpr::string("ax-state-event/1"))
        .prop(
            "data-ax-dom-protocol",
            AxExpr::string("ax-dom-capability/1"),
        )
        .prop("data-ax-dom-write", AxExpr::string(bind_target.to_string()))
        .prop("data-ax-state-type", AxExpr::string(binding.ty.clone()));

    if matches!(bind_target, "value" | "checked")
        && !component.props.iter().any(|prop| prop.name == bind_target)
    {
        component = component.prop(bind_target.to_string(), AxExpr::ident(name));
    }

    Ok(component)
}

fn apply_state_read_binding(component: AxComponent, binding: &StateBindingPlan) -> AxComponent {
    component
        .prop("data-ax-signal", AxExpr::string(binding.signal_id.clone()))
        .prop("data-ax-bind", AxExpr::string("text"))
        .prop(
            "data-ax-dom-protocol",
            AxExpr::string("ax-dom-capability/1"),
        )
        .prop("data-ax-dom-write", AxExpr::string("text"))
        .prop("data-ax-state-type", AxExpr::string(binding.ty.clone()))
}

fn apply_state_event_attr(
    mut component: AxComponent,
    attr: &AxAttributeNode,
    state_bindings: &BTreeMap<String, StateBindingPlan>,
) -> Result<AxComponent, AxConvertV2Error> {
    let event = attr.name.trim_start_matches("on:");
    if !matches!(event, "click" | "input" | "change") {
        return Err(AxConvertV2Error::UnsupportedStateEvent {
            attr: attr.name.clone(),
        });
    }

    let AxAttributeValue::Expr(source) = &attr.value else {
        return Err(AxConvertV2Error::InvalidStateEvent {
            expr_source: attr.name.clone(),
        });
    };
    let mutation = parse_state_event_mutation(source)?;
    let Some(binding) = state_bindings.get(&mutation.state) else {
        return Err(AxConvertV2Error::UnknownStateEvent {
            attr: attr.name.clone(),
        });
    };
    let set_value_matches = mutation.value_ty.as_deref().is_none_or(|value_ty| {
        state_literal_matches_type(value_ty, &binding.ty)
            || value_ty == "String"
                && mutation.value.as_ref().is_some_and(|value| {
                    binding
                        .literals
                        .as_ref()
                        .is_some_and(|literals| literals.contains(value))
                })
    });
    if matches!(mutation.op.as_str(), "add" | "sub")
        && !matches!(binding.ty.as_str(), "Number" | "Int" | "Float")
        || mutation.op == "set" && !set_value_matches
        || mutation.op == "toggle" && binding.ty != "Bool"
    {
        return Err(AxConvertV2Error::InvalidStateEvent {
            expr_source: source.clone(),
        });
    }

    let prefix = format!("data-ax-on-{event}");
    component = component
        .prop(
            format!("{prefix}-signal"),
            AxExpr::string(binding.signal_id.clone()),
        )
        .prop(format!("{prefix}-op"), AxExpr::string(mutation.op))
        .prop(
            format!("{prefix}-initial"),
            AxExpr::string(binding.initial.clone()),
        )
        .prop(
            format!("{prefix}-protocol"),
            AxExpr::string("ax-state-event/1"),
        )
        .prop(format!("{prefix}-type"), AxExpr::string(binding.ty.clone()));
    if let Some(value) = mutation.value {
        component = component.prop(format!("{prefix}-value"), AxExpr::string(value));
    }
    if let Some(value_source) = mutation.value_source {
        component = component.prop(
            format!("{prefix}-value-source"),
            AxExpr::string(value_source),
        );
    }

    Ok(component)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StateEventMutation {
    state: String,
    op: String,
    value: Option<String>,
    value_ty: Option<String>,
    value_source: Option<String>,
}

fn parse_state_event_mutation(source: &str) -> Result<StateEventMutation, AxConvertV2Error> {
    let source = source.trim();
    for (token, op) in [("+=", "add"), ("-=", "sub"), ("=", "set")] {
        let Some(index) = source.find(token) else {
            continue;
        };
        let state = source[..index].trim();
        let value_source = source[index + token.len()..].trim();
        if !is_state_identifier(state) || value_source.is_empty() {
            break;
        }
        if token == "=" && value_source == format!("!{state}") {
            return Ok(StateEventMutation {
                state: state.to_string(),
                op: "toggle".to_string(),
                value: None,
                value_ty: None,
                value_source: None,
            });
        }
        if matches!(value_source, "event.value" | "event.checked") {
            return Ok(StateEventMutation {
                state: state.to_string(),
                op: op.to_string(),
                value: None,
                value_ty: (value_source == "event.checked").then(|| "Bool".to_string()),
                value_source: Some(value_source.trim_start_matches("event.").to_string()),
            });
        }
        let literal = parse_v2_expr(value_source).ok().and_then(|expr| {
            state_event_literal(&expr).map(|value| (value, infer_state_type(&expr)))
        });
        if let Some((value, value_ty)) = literal {
            return Ok(StateEventMutation {
                state: state.to_string(),
                op: op.to_string(),
                value: Some(value),
                value_ty: Some(value_ty),
                value_source: None,
            });
        }
        break;
    }

    Err(AxConvertV2Error::InvalidStateEvent {
        expr_source: source.to_string(),
    })
}

fn is_state_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(first) if first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn state_event_literal(expr: &AxExpr) -> Option<String> {
    match expr {
        AxExpr::String(value) => Some(value.clone()),
        AxExpr::Number(value) => Some(value.to_string()),
        AxExpr::Float(value) => Some(value.get().to_string()),
        AxExpr::Bool(value) => Some(value.to_string()),
        AxExpr::Identifier(value) if value == "null" => Some("null".to_string()),
        AxExpr::List(items) => Some(format!(
            "[{}]",
            items
                .iter()
                .map(state_event_nested_literal)
                .collect::<Option<Vec<_>>>()?
                .join(",")
        )),
        AxExpr::Object(fields) => Some(format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(name, value)| Some(format!(
                    "{}:{}",
                    quote_state_string(name),
                    state_event_nested_literal(value)?
                )))
                .collect::<Option<Vec<_>>>()?
                .join(",")
        )),
        _ => None,
    }
}

fn state_event_nested_literal(expr: &AxExpr) -> Option<String> {
    match expr {
        AxExpr::String(value) => Some(quote_state_string(value)),
        AxExpr::Number(value) => Some(value.to_string()),
        AxExpr::Float(value) => Some(value.get().to_string()),
        AxExpr::Bool(value) => Some(value.to_string()),
        AxExpr::Identifier(value) if value == "null" => Some("null".to_string()),
        AxExpr::List(_) | AxExpr::Object(_) => state_event_literal(expr),
        _ => None,
    }
}

fn quote_state_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn state_literal_matches_type(literal: &str, declared: &str) -> bool {
    literal == declared
        || literal == "Number" && matches!(declared, "Int" | "Float")
        || literal == "List<Unknown>" && declared.starts_with("List<")
        || literal == "Unknown" && declared.starts_with("Optional<")
        || literal == "Json"
            && AxType::parse_annotation(declared).is_ok_and(|ty| {
                matches!(
                    ty,
                    AxType::Json
                        | AxType::Unknown
                        | AxType::Map(_, _)
                        | AxType::Result(_, _)
                        | AxType::Record(_)
                )
            })
}

fn convert_prop(attr: &AxAttributeNode) -> Result<AxProp, AxConvertV2Error> {
    Ok(AxProp::new(
        attr.name.clone(),
        convert_attr_value(&attr.value)?,
    ))
}

fn convert_attr_value(value: &AxAttributeValue) -> Result<AxExpr, AxConvertV2Error> {
    match value {
        AxAttributeValue::String(value) => Ok(AxExpr::string(value.clone())),
        AxAttributeValue::Expr(expr) => parse_v2_expr(expr),
    }
}

fn parse_v2_expr(source: &str) -> Result<AxExpr, AxConvertV2Error> {
    parse_expr(source, 1).map_err(|error| AxConvertV2Error::InvalidExpression {
        source: source.to_string(),
        error,
    })
}

pub mod prelude {
    pub use super::convert_ax_v2_file;
    pub use super::looks_like_ax_v2;
    pub use super::parse_ax_auto;
    pub use super::AxAutoParseError;
    pub use super::AxConvertV2Error;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_v2_document_into_existing_runtime_ast() {
        let file = parse_ax_v2(
            r#"
import { Card } from "@/ui/Card.ax"

page Home
<Container max="xl">
  <Card title={post.title}>
    <Copy>{post.excerpt}</Copy>
  </Card>
</Container>
"#,
        )
        .expect("v2 file should parse");

        let document = convert_ax_v2_file(&file).expect("v2 file should convert");

        assert_eq!(document.page.name, "Home");
        assert_eq!(document.page.body.len(), 1);

        let AxStatement::Component(container) = &document.page.body[0] else {
            panic!("container should convert into component statement");
        };

        assert_eq!(container.name, "Container");
        assert_eq!(container.props, vec![AxProp::new("max", "xl")]);
    }

    #[test]
    fn converts_page_param_defaults_into_runtime_page_params() {
        let document = parse_ax_auto(
            r#"
page Card(title = "Untitled")

<article>{title}</article>
"#,
        )
        .expect("auto parse should support page params");

        assert_eq!(document.page.name, "Card");
        assert_eq!(
            document.page.params,
            vec![AxComponentParamDef::with_default(
                "title",
                AxExpr::string("Untitled")
            )]
        );
    }

    #[test]
    fn converts_class_name_attr_into_runtime_class_style() {
        let document = parse_ax_auto(
            r#"
page Home()
{
  const heroClass = "hero-shell"

  return ASX {
    <section className={heroClass}>Hello</section>
  }
}
"#,
        )
        .expect("className should convert");

        let AxStatement::Component(section) = &document.page.body[1] else {
            panic!("section should convert into component statement");
        };

        assert_eq!(section.name, "section");
        assert_eq!(section.style.class, Some(AxExpr::ident("heroClass")));
        assert!(section.props.iter().all(|prop| prop.name != "className"));
    }

    #[test]
    fn rejects_mixed_class_and_class_name_attrs() {
        let error = parse_ax_auto(
            r#"
page Home()
{
  return ASX {
    <section class="hero" className="panel">Hello</section>
  }
}
"#,
        )
        .expect_err("class and className should not be mixed");

        let AxAutoParseError::Convert(AxConvertV2Error::DuplicateClassAttr { first, second }) =
            error
        else {
            panic!("expected duplicate class attr error, got {error:?}");
        };

        assert_eq!(first, "class");
        assert_eq!(second, "className");
    }

    #[test]
    fn converts_v2_head_block_into_document_head() {
        let document = parse_ax_auto(
            r#"
page Home
<Head>
  <Title>{"Hello Axonyx"}</Title>
  <Theme>silver</Theme>
  <Meta name="description" content="Docs without bloat" />
  <Link rel="icon" href="/favicon.svg" />
</Head>
<Copy>Body</Copy>
"#,
        )
        .expect("auto parse should support v2 head");

        assert_eq!(document.head.title, Some(AxExpr::string("Hello Axonyx")));
        assert_eq!(document.head.theme, Some(AxExpr::string("silver")));
        assert_eq!(document.head.metas.len(), 1);
        assert_eq!(document.head.links.len(), 1);
        assert_eq!(document.page.body.len(), 1);
    }

    #[test]
    fn converts_theme_preflight_attrs_into_document_head() {
        let document = parse_ax_auto(
            r#"
page Home
<Head>
  <Theme default="silver" storageKey="axonyx-site-theme" preflight />
</Head>
<Copy>Body</Copy>
"#,
        )
        .expect("auto parse should support theme preflight attrs");

        assert_eq!(document.head.theme, Some(AxExpr::string("silver")));
        assert_eq!(
            document.head.theme_storage_key,
            Some(AxExpr::string("axonyx-site-theme"))
        );
        assert!(document.head.theme_preflight);
    }

    #[test]
    fn preserves_import_bindings_from_v2_document() {
        let document = parse_ax_auto(
            r#"
import { Card as SiteCard, Copy } from "@/ui"

page Home
<SiteCard>
  <Copy>Body</Copy>
</SiteCard>
"#,
        )
        .expect("imports should parse");

        assert_eq!(document.imports.len(), 1);
        assert_eq!(document.imports[0].source, "@/ui");
        assert_eq!(document.imports[0].bindings[0].imported, "Card");
        assert_eq!(document.imports[0].bindings[0].local, "SiteCard");
    }

    #[test]
    fn converts_mixed_children_into_statement_sequence() {
        let document = parse_ax_v2(
            r#"
page Home
<p>
  Hello
  <strong>world</strong>
</p>
"#,
        )
        .expect("v2 file should parse");

        let document = convert_ax_v2_file(&document).expect("conversion should succeed");
        let AxStatement::Component(paragraph) = &document.page.body[0] else {
            panic!("paragraph should convert into component");
        };
        let AxBody::Block(body) = &paragraph.body else {
            panic!("mixed children should become block body");
        };

        assert_eq!(body.len(), 2);
        assert_eq!(body[0], AxStatement::text("Hello"));
    }

    #[test]
    fn converts_local_component_declarations_into_runtime_defs() {
        let document = parse_ax_auto(
            r#"
page Home

component FeatureCard(title) {
  <Card title={title}>
    <Slot />
  </Card>
}

<FeatureCard title="Hello">Body</FeatureCard>
"#,
        )
        .expect("local component declaration should convert");

        assert_eq!(document.components.len(), 1);
        assert_eq!(document.components[0].name, "FeatureCard");
        assert_eq!(
            document.components[0].params,
            vec![AxComponentParamDef::new("title")]
        );
        assert_eq!(document.components[0].body.len(), 1);
        assert_eq!(document.page.body.len(), 1);
    }

    #[test]
    fn converts_local_component_param_defaults_into_runtime_exprs() {
        let document = parse_ax_auto(
            r#"
page Home

component FeatureCard(title = "Hello") {
  <Card title={title}>
    <Slot />
  </Card>
}

<FeatureCard />
"#,
        )
        .expect("local component default should convert");

        assert_eq!(
            document.components[0].params,
            vec![AxComponentParamDef::with_default(
                "title",
                AxExpr::string("Hello")
            )]
        );
    }

    #[test]
    fn preserves_literal_union_component_param_contracts() {
        let document = parse_ax_auto(
            r#"
page Home() {
  type Theme = "silver" | "bronze" | "gold"

  component ThemeSwitcher(theme: Theme = "silver") {
    <Copy>{theme}</Copy>
  }

  return ASX {
    <ThemeSwitcher theme="bronze" />
  }
}
"#,
        )
        .expect("literal union component contract should convert");

        assert_eq!(
            document.components[0].params[0].ty.as_deref(),
            Some("Theme")
        );
        assert_eq!(
            document.components[0].params[0].literal_values,
            ["silver", "bronze", "gold"]
        );
    }

    #[test]
    fn rejects_invalid_literal_union_component_param_default() {
        let error = parse_ax_auto(
            r#"
page Home() {
  type Theme = "silver" | "bronze" | "gold"

  component ThemeSwitcher(theme: Theme = "purple") {
    <Copy>{theme}</Copy>
  }

  return ASX { <ThemeSwitcher /> }
}
"#,
        )
        .expect_err("invalid literal union default should fail");

        assert!(matches!(
            error,
            AxAutoParseError::Convert(AxConvertV2Error::InvalidComponentParamDefault {
                name,
                ty,
                expr_source,
            }) if name == "theme" && ty == "Theme" && expr_source == "\"purple\""
        ));
    }

    #[test]
    fn converts_top_level_let_declarations_into_data_statements() {
        let document = parse_ax_auto(
            r#"
page Home

let heroTitle = "Hello Axonyx"

<Copy>{heroTitle}</Copy>
"#,
        )
        .expect("let declaration should convert");

        assert_eq!(document.page.body.len(), 2);
        assert_eq!(
            document.page.body[0],
            AxStatement::data("heroTitle", AxExpr::string("Hello Axonyx"))
        );
    }

    #[test]
    fn converts_function_shaped_page_return_asx_into_document() {
        let document = parse_ax_auto(
            r#"
page Home() {
  data title = "Hello Axonyx"

  return ASX {
    <Container max="xl">
      <Copy>{title}</Copy>
    </Container>
  }
}
"#,
        )
        .expect("function-shaped page should convert");

        assert_eq!(document.page.name, "Home");
        assert_eq!(document.page.body.len(), 2);
        assert_eq!(
            document.page.body[0],
            AxStatement::data("title", AxExpr::string("Hello Axonyx"))
        );
    }

    #[test]
    fn converts_page_return_type_asx_shorthand_into_document() {
        let document = parse_ax_auto(
            r#"
page Home() -> ASX {
  data title = "Hello Axonyx"

  return {
    <Copy>{title}</Copy>
  }
}
"#,
        )
        .expect("ASX return type shorthand should convert");

        assert_eq!(document.page.name, "Home");
        assert_eq!(document.page.body.len(), 2);
        assert_eq!(
            document.page.body[0],
            AxStatement::data("title", AxExpr::string("Hello Axonyx"))
        );
    }

    #[test]
    fn converts_destructured_data_bindings_to_member_bindings() {
        let document = parse_ax_auto(
            r#"
page Dashboard() {
  data { posts, total: count } = loadDashboard("published")

  return ASX {
    <Copy>{count}</Copy>
  }
}
"#,
        )
        .expect("destructured data should convert");

        assert_eq!(
            document.page.body[0],
            AxStatement::data(
                "posts",
                AxExpr::call(["loadDashboard"], [AxExpr::string("published")]).member("posts"),
            )
        );
        assert_eq!(
            document.page.body[1],
            AxStatement::data(
                "count",
                AxExpr::call(["loadDashboard"], [AxExpr::string("published")]).member("total"),
            )
        );
    }

    #[test]
    fn converts_const_declarations_to_render_local_data_bindings() {
        let document = parse_ax_auto(
            r#"
page Posts() {
  data posts = loadPosts()
  const hasPosts = posts.length > 0

  return ASX {
    <If when={hasPosts}>
      <Copy>Ready</Copy>
    </If>
  }
}
"#,
        )
        .expect("const declaration should convert");

        assert_eq!(
            document.page.body[1],
            AxStatement::data(
                "hasPosts",
                AxExpr::binary(
                    AxBinaryOp::Gt,
                    AxExpr::ident("posts").member("length"),
                    AxExpr::number(0),
                ),
            )
        );
    }

    #[test]
    fn converts_state_signal_binding_into_bridge_metadata() {
        let document = parse_ax_auto(
            r#"
page Home

state theme = "silver"
state count: Number = 0

<input bind:value={theme} />
<span bind:text={theme}>{theme}</span>
<input bind:value={count} />
"#,
        )
        .expect("state binding should convert");

        assert_eq!(document.page.body.len(), 5);
        assert_eq!(
            document.page.body[0],
            AxStatement::data("theme", AxExpr::string("silver"))
        );
        assert_eq!(
            document.page.body[1],
            AxStatement::data("count", AxExpr::number(0))
        );

        let AxStatement::Component(input) = &document.page.body[2] else {
            panic!("input should convert into component");
        };
        assert!(input.props.contains(&AxProp::new(
            "data-ax-signal",
            AxExpr::string("root:theme:1")
        )));
        assert!(input
            .props
            .contains(&AxProp::new("data-ax-bind", AxExpr::string("value"))));
        assert!(input.props.contains(&AxProp::new(
            "data-ax-bind-protocol",
            AxExpr::string("ax-state-event/1")
        )));
        assert!(input.props.contains(&AxProp::new(
            "data-ax-dom-protocol",
            AxExpr::string("ax-dom-capability/1")
        )));
        assert!(input
            .props
            .contains(&AxProp::new("data-ax-dom-write", AxExpr::string("value"))));
        assert!(input
            .props
            .contains(&AxProp::new("data-ax-state-type", AxExpr::string("String"))));
        assert!(input
            .props
            .contains(&AxProp::new("value", AxExpr::ident("theme"))));

        let AxStatement::Component(span) = &document.page.body[3] else {
            panic!("span should convert into component");
        };
        assert!(span
            .props
            .contains(&AxProp::new("data-ax-bind", AxExpr::string("text"))));
        assert!(span
            .props
            .contains(&AxProp::new("data-ax-dom-write", AxExpr::string("text"))));
        assert!(!span.props.iter().any(|prop| prop.name == "text"));

        let AxStatement::Component(count_input) = &document.page.body[4] else {
            panic!("count input should convert into component");
        };
        assert!(count_input
            .props
            .contains(&AxProp::new("data-ax-state-type", AxExpr::string("Number"))));
    }

    #[test]
    fn converts_local_state_click_mutations_into_declarative_event_metadata() {
        let document = parse_ax_auto(
            r#"
page Counter() {
  state count: Number = 0
  state ratio: Float = 0.5
  state open: Bool = false

  return ASX {
    <>
      <Button on:click={count += 1}>Increase</Button>
      <button on:click={count -= 2}>Decrease</button>
      <button on:click={ratio += 0.25}>Increase ratio</button>
      <button on:click={open = !open}>Toggle</button>
      <Copy>{count}</Copy>
    </>
  }
}
"#,
        )
        .expect("local state event should convert");

        let AxStatement::Component(fragment) = &document.page.body[3] else {
            panic!("expected fragment");
        };
        let AxBody::Block(body) = &fragment.body else {
            panic!("expected fragment body");
        };
        let AxStatement::Component(increase) = &body[0] else {
            panic!("expected increase button");
        };
        assert!(increase.props.contains(&AxProp::new(
            "data-ax-on-click-signal",
            AxExpr::string("root:count:1")
        )));
        assert!(increase
            .props
            .contains(&AxProp::new("data-ax-on-click-op", AxExpr::string("add"))));
        assert!(increase
            .props
            .contains(&AxProp::new("data-ax-on-click-value", AxExpr::string("1"))));

        let AxStatement::Component(increase_ratio) = &body[2] else {
            panic!("expected ratio button");
        };
        assert!(increase_ratio.props.contains(&AxProp::new(
            "data-ax-on-click-value",
            AxExpr::string("0.25")
        )));
        assert!(increase_ratio.props.contains(&AxProp::new(
            "data-ax-on-click-type",
            AxExpr::string("Float")
        )));

        let AxStatement::Component(toggle) = &body[3] else {
            panic!("expected toggle button");
        };
        assert!(toggle.props.contains(&AxProp::new(
            "data-ax-on-click-op",
            AxExpr::string("toggle")
        )));

        let AxStatement::Component(copy) = &body[4] else {
            panic!("expected state reader");
        };
        assert!(copy.props.contains(&AxProp::new(
            "data-ax-signal",
            AxExpr::string("root:count:1")
        )));
        assert!(copy
            .props
            .contains(&AxProp::new("data-ax-bind", AxExpr::string("text"))));
    }

    #[test]
    fn rejects_local_events_that_do_not_mutate_declared_state() {
        let error = parse_ax_auto(
            r#"
page Counter() {
  state count: Number = 0
  return ASX { <Button on:click={missing += 1}>Increase</Button> }
}
"#,
        )
        .expect_err("unknown event state should fail");

        assert!(matches!(
            error,
            AxAutoParseError::Convert(AxConvertV2Error::UnknownStateEvent { .. })
        ));
    }

    #[test]
    fn converts_state_dependent_if_into_reactive_dom_boundary() {
        let document = parse_ax_auto(
            r#"
page Counter() {
  state count: Number = 0
  return ASX {
    <If when={count > 5}>
      <Badge>High value</Badge>
      <Else><Copy>Low value</Copy></Else>
    </If>
  }
}
"#,
        )
        .expect("state-dependent if should convert");

        let AxStatement::Component(fragment) = &document.page.body[1] else {
            panic!("expected return fragment");
        };
        let AxBody::Block(body) = &fragment.body else {
            panic!("expected return body");
        };
        let AxStatement::Component(state_if) = &body[0] else {
            panic!("expected reactive state if component");
        };
        assert_eq!(state_if.name, "__AxStateIf");
        assert!(state_if.props.contains(&AxProp::new(
            "data-ax-state-if-signal",
            AxExpr::string("root:count:1")
        )));
        assert!(state_if
            .props
            .contains(&AxProp::new("data-ax-state-if-op", AxExpr::string("gt"))));
        let AxBody::Block(branches) = &state_if.body else {
            panic!("expected condition branches");
        };
        let AxStatement::Component(then_branch) = &branches[0] else {
            panic!("expected then branch");
        };
        assert!(then_branch
            .props
            .contains(&AxProp::new("hidden", AxExpr::bool(true))));
    }

    #[test]
    fn converts_state_dependent_match_into_reactive_dom_boundary() {
        let document = parse_ax_auto(
            r#"
page ThemePreview() {
  type Theme = "silver" | "bronze" | "gold"
  state theme: Theme = "silver"
  return ASX {
    <Match value={theme}>
      <Case is="silver"><Copy>Silver</Copy></Case>
      <Case is="gold"><Copy>Gold</Copy></Case>
      <Default><Copy>Other</Copy></Default>
    </Match>
  }
}
"#,
        )
        .expect("state-dependent match should convert");

        let AxStatement::Component(fragment) = &document.page.body[1] else {
            panic!("expected return fragment");
        };
        let AxBody::Block(body) = &fragment.body else {
            panic!("expected return body");
        };
        let AxStatement::Component(state_match) = &body[0] else {
            panic!("expected reactive state match component");
        };
        assert_eq!(state_match.name, "__AxStateMatch");
        assert!(state_match.props.contains(&AxProp::new(
            "data-ax-state-match-signal",
            AxExpr::string("root:theme:1")
        )));
        assert!(state_match.props.contains(&AxProp::new(
            "data-ax-state-match-literals",
            AxExpr::string(r#"["silver","bronze","gold"]"#)
        )));
        let AxBody::Block(branches) = &state_match.body else {
            panic!("expected match branches");
        };
        assert_eq!(branches.len(), 3);
        let AxStatement::Component(gold) = &branches[1] else {
            panic!("expected gold branch");
        };
        assert!(gold
            .props
            .contains(&AxProp::new("hidden", AxExpr::bool(true))));
    }

    #[test]
    fn compiles_reactive_text_and_boolean_attribute_expression_metadata() {
        let document = parse_ax_auto(
            r#"
page Counter() {
  state count: Int = 2
  state limit: Int = 5
  return ASX {
    <>
      <Copy>{count * 2}</Copy>
      <Button disabled={count >= limit}>Increase</Button>
    </>
  }
}
"#,
        )
        .expect("reactive expressions should compile");

        let AxStatement::Component(fragment) = &document.page.body[2] else {
            panic!("expected return fragment");
        };
        let AxBody::Block(body) = &fragment.body else {
            panic!("expected return body");
        };
        let AxStatement::Component(copy) = &body[0] else {
            panic!("expected Copy");
        };
        let AxBody::Block(copy_body) = &copy.body else {
            panic!("expected reactive text wrapper");
        };
        let AxStatement::Component(expression) = &copy_body[0] else {
            panic!("expected reactive expression");
        };
        assert_eq!(expression.name, "__AxReactiveText");
        assert!(expression.props.contains(&AxProp::new(
            "data-ax-expression-0-signals",
            AxExpr::string(r#"["root:count:1"]"#)
        )));
        assert!(expression.props.contains(&AxProp::new(
            "data-ax-expression-0-target",
            AxExpr::string("text")
        )));

        let AxStatement::Component(button) = &body[1] else {
            panic!("expected Button");
        };
        assert!(button.props.contains(&AxProp::new(
            "data-ax-expression-0-signals",
            AxExpr::string(r#"["root:count:1","root:limit:2"]"#)
        )));
        assert!(button.props.contains(&AxProp::new(
            "data-ax-expression-0-target",
            AxExpr::string("boolean:disabled")
        )));
    }

    #[test]
    fn inlines_local_pure_functions_into_reactive_expression_metadata() {
        let document = parse_ax_auto(
            r#"
page Counter() {
  state count: Int = 2
  state limit: Int = 5
  fn double(value: Int) = value * 2
  fn twicePlus(value: Int, extra: Int = 1) = double(value) + extra
  fn reached(value: Int, maximum: Int) = value >= maximum
  return ASX {
    <>
      <Copy>{twicePlus(count)}</Copy>
      <Button disabled={reached(double(count), limit)}>Increase</Button>
    </>
  }
}
"#,
        )
        .expect("local pure functions should inline into reactive bytecode");

        let AxStatement::Component(fragment) = &document.page.body[2] else {
            panic!("expected return fragment");
        };
        let AxBody::Block(body) = &fragment.body else {
            panic!("expected return body");
        };
        let AxStatement::Component(copy) = &body[0] else {
            panic!("expected Copy");
        };
        let AxBody::Block(copy_body) = &copy.body else {
            panic!("expected reactive text wrapper");
        };
        let AxStatement::Component(expression) = &copy_body[0] else {
            panic!("expected reactive expression");
        };
        assert!(expression.props.iter().any(|prop| {
            prop.name == "data-ax-expression-0-program"
                && matches!(&prop.value, AxExpr::String(value) if value.starts_with("41584501") && value.ends_with("14"))
        }));
        assert!(expression.props.contains(&AxProp::new(
            "data-ax-expression-0-signals",
            AxExpr::string(r#"["root:count:1"]"#)
        )));

        let AxStatement::Component(button) = &body[1] else {
            panic!("expected Button");
        };
        assert!(button.props.contains(&AxProp::new(
            "data-ax-expression-0-signals",
            AxExpr::string(r#"["root:count:1","root:limit:2"]"#)
        )));
        assert!(button.props.contains(&AxProp::new(
            "data-ax-expression-0-target",
            AxExpr::string("boolean:disabled")
        )));
    }

    #[test]
    fn rejects_unknown_state_dependent_function_calls() {
        let error = parse_ax_auto(
            r#"
page Counter() {
  state count: Int = 2
  return ASX { <Copy>{format(count)}</Copy> }
}
"#,
        )
        .expect_err("reactive calls should not silently become static");

        assert!(matches!(
            &error,
            AxAutoParseError::Convert(AxConvertV2Error::UnsupportedReactiveExpression {
                expr_source,
                reason,
            }) if expr_source == "format(count)" && reason.contains("not a local pure function")
        ));
    }

    #[test]
    fn finds_reactive_state_referenced_by_a_pure_function_default() {
        let document = parse_ax_auto(
            r#"
page Counter() {
  state count: Int = 2
  fn next(value: Int = count) = value + 1
  return ASX { <Copy>{next()}</Copy> }
}
"#,
        )
        .expect("pure function defaults should participate in dependency discovery");

        let AxStatement::Component(copy) = &document.page.body[1] else {
            panic!("expected Copy");
        };
        let AxBody::Block(body) = &copy.body else {
            panic!("expected reactive text wrapper");
        };
        let AxStatement::Component(expression) = &body[0] else {
            panic!("expected reactive expression");
        };
        assert!(expression.props.contains(&AxProp::new(
            "data-ax-expression-0-signals",
            AxExpr::string(r#"["root:count:1"]"#)
        )));
    }

    #[test]
    fn compiles_reactive_list_and_object_literals() {
        let document = parse_ax_auto(
            r#"
page CollectionProbe() {
  state count: Int = 2
  state limit: Int = 3
  return ASX {
    <>
      <Copy>{count in [1, 2, 3]}</Copy>
      <button disabled={({active: count >= limit}).active}>Locked</button>
    </>
  }
}
"#,
        )
        .expect("reactive collection literals should compile");

        let AxStatement::Component(fragment) = &document.page.body[2] else {
            panic!("expected return fragment");
        };
        let AxBody::Block(body) = &fragment.body else {
            panic!("expected fragment body");
        };
        let AxStatement::Component(copy) = &body[0] else {
            panic!("expected Copy");
        };
        let AxBody::Block(copy_body) = &copy.body else {
            panic!("expected reactive text wrapper");
        };
        let AxStatement::Component(expression) = &copy_body[0] else {
            panic!("expected reactive expression");
        };
        assert!(expression.props.iter().any(|prop| {
            prop.name == "data-ax-expression-0-program"
                && matches!(&prop.value, AxExpr::String(value) if value.ends_with("3203001f"))
        }));

        let AxStatement::Component(button) = &body[1] else {
            panic!("expected button");
        };
        assert!(button.props.iter().any(|prop| {
            prop.name == "data-ax-expression-0-program"
                && matches!(&prop.value, AxExpr::String(value) if value.contains("33010006000000616374697665"))
        }));
    }

    #[test]
    fn rejects_recursive_pure_reactive_functions() {
        let error = parse_ax_auto(
            r#"
page Counter() {
  state count: Int = 2
  fn first(value: Int) = second(value)
  fn second(value: Int) = first(value)
  return ASX { <Copy>{first(count)}</Copy> }
}
"#,
        )
        .expect_err("recursive pure functions should fail during reactive compilation");

        assert!(matches!(
            &error,
            AxAutoParseError::Convert(AxConvertV2Error::UnsupportedReactiveExpression {
                reason,
                ..
            }) if reason.contains("first -> second -> first")
        ));
    }

    #[test]
    fn rejects_literal_union_state_events_outside_the_contract() {
        let error = parse_ax_auto(
            r#"
page ThemePreview() {
  type Theme = "silver" | "gold"
  state theme: Theme = "silver"
  return ASX { <Button on:click={theme = "purple"}>Purple</Button> }
}
"#,
        )
        .expect_err("literal union events should enforce the declared contract");

        assert!(matches!(
            error,
            AxAutoParseError::Convert(AxConvertV2Error::InvalidStateEvent { .. })
        ));
    }

    #[test]
    fn converts_input_and_change_event_values_into_safe_metadata() {
        let document = parse_ax_auto(
            r#"
page Filters() {
  state query: String = ""
  state enabled: Bool = false
  return ASX {
    <>
      <input on:input={query = event.value} />
      <input type="checkbox" on:change={enabled = event.checked} />
    </>
  }
}
"#,
        )
        .expect("event values should convert");

        let AxStatement::Component(fragment) = &document.page.body[2] else {
            panic!("expected fragment");
        };
        let AxBody::Block(body) = &fragment.body else {
            panic!("expected fragment body");
        };
        let AxStatement::Component(input) = &body[0] else {
            panic!("expected text input");
        };
        assert!(input.props.contains(&AxProp::new(
            "data-ax-on-input-value-source",
            AxExpr::string("value")
        )));
        assert!(input.props.contains(&AxProp::new(
            "data-ax-on-input-protocol",
            AxExpr::string("ax-state-event/1")
        )));
        let AxStatement::Component(checkbox) = &body[1] else {
            panic!("expected checkbox");
        };
        assert!(checkbox.props.contains(&AxProp::new(
            "data-ax-on-change-value-source",
            AxExpr::string("checked")
        )));
        assert!(checkbox.props.contains(&AxProp::new(
            "data-ax-on-change-protocol",
            AxExpr::string("ax-state-event/1")
        )));
    }

    #[test]
    fn converts_component_local_state_into_component_metadata_and_bindings() {
        let document = parse_ax_auto(
            r#"
page Home

component ThemePicker() {
  state theme: String = "silver"

  render ASX {
    <input bind:value={theme} />
    <span bind:text={theme}>{theme}</span>
  }
}

<ThemePicker />
"#,
        )
        .expect("component state should convert");

        assert_eq!(document.components.len(), 1);
        let component = &document.components[0];
        assert_eq!(component.states.len(), 1);
        assert_eq!(component.states[0].name, "theme");
        assert_eq!(component.states[0].ty, "String");
        assert_eq!(component.states[0].initial, AxExpr::string("silver"));
        assert_eq!(
            component.states[0].signal,
            "__ax_component_state__:ThemePicker:theme:1"
        );

        let AxStatement::Component(input) = &component.body[0] else {
            panic!("input should convert into component");
        };
        assert!(input.props.contains(&AxProp::new(
            "data-ax-signal",
            AxExpr::string("__ax_component_state__:ThemePicker:theme:1")
        )));
        assert!(input
            .props
            .contains(&AxProp::new("data-ax-bind", AxExpr::string("value"))));
    }

    #[test]
    fn converts_structured_component_state_into_typed_metadata() {
        let document = parse_ax_auto(
            r#"
page Home

component Filters() {
  state selected: List<Optional<String>> = ["published", null]

  render ASX {
    <Copy bind:text={selected}>{selected}</Copy>
  }
}

<Filters />
"#,
        )
        .expect("structured component state should convert");

        assert_eq!(
            document.components[0].states[0].ty,
            "List<Optional<String>>"
        );
        let AxStatement::Component(copy) = &document.components[0].body[0] else {
            panic!("copy should convert into component");
        };
        assert!(copy.props.contains(&AxProp::new(
            "data-ax-state-type",
            AxExpr::string("List<Optional<String>>")
        )));
    }

    #[test]
    fn rejects_secret_component_state_types() {
        let error = parse_ax_auto(
            r#"
page Home

component Demo() {
  state token: Secret<String> = "private"

  render ASX {
    <Copy>Demo</Copy>
  }
}
"#,
        )
        .expect_err("secret component state should fail");

        assert!(matches!(
            error,
            AxAutoParseError::Convert(AxConvertV2Error::UnsupportedComponentStateType { .. })
        ));
    }

    #[test]
    fn rejects_bind_to_undeclared_state() {
        let error = parse_ax_auto(
            r#"
page Home
<input bind:value={theme} />
"#,
        )
        .expect_err("undeclared state binding should fail");

        assert!(matches!(
            error,
            AxAutoParseError::Convert(AxConvertV2Error::UnknownStateBinding { .. })
        ));
    }

    #[test]
    fn rejects_unknown_state_bind_target() {
        let error = parse_ax_auto(
            r#"
page Home

state theme = "silver"
<input bind:theme={theme} />
"#,
        )
        .expect_err("unknown state bind target should fail");

        assert!(matches!(
            error,
            AxAutoParseError::Convert(AxConvertV2Error::InvalidStateBinding { .. })
        ));
    }

    #[test]
    fn converts_top_level_function_declarations_into_runtime_defs() {
        let document = parse_ax_auto(
            r#"
page Home

fn heroTitle(title = "Hello") = title

<Copy>{heroTitle()}</Copy>
"#,
        )
        .expect("function declaration should convert");

        assert_eq!(document.functions.len(), 1);
        assert_eq!(document.functions[0].name, "heroTitle");
        assert_eq!(
            document.functions[0].params,
            vec![AxComponentParamDef::with_default(
                "title",
                AxExpr::string("Hello")
            )]
        );
        assert_eq!(document.functions[0].body, AxExpr::ident("title"));
    }

    #[test]
    fn converts_fragment_shorthand_into_runtime_fragment_component() {
        let document = parse_ax_auto(
            r#"
page Home
<>
  Hello
  <strong>Axonyx</strong>
</>
"#,
        )
        .expect("fragment shorthand should parse");

        let AxStatement::Component(fragment) = &document.page.body[0] else {
            panic!("fragment should convert into component statement");
        };

        assert_eq!(fragment.name, "Fragment");
    }

    #[test]
    fn converts_each_and_if_control_elements_into_runtime_blocks() {
        let document = parse_ax_auto(
            r#"
page Home
<Each items={posts} as="post">
  <If when={post.published}>
    <Card title={post.title} />
  </If>
</Each>
"#,
        )
        .expect("control elements should parse");

        let AxStatement::Component(fragment) = &document.page.body[0] else {
            panic!("top-level each should convert into fragment component");
        };
        let AxBody::Block(body) = &fragment.body else {
            panic!("fragment should contain converted control statements");
        };
        let AxStatement::Each(each) = &body[0] else {
            panic!("expected each block");
        };
        assert_eq!(each.binding, "post");

        let AxStatement::If(if_block) = &each.body[0] else {
            panic!("expected nested if block");
        };
        assert_eq!(
            if_block.condition,
            AxExpr::ident("post").member("published")
        );
    }

    #[test]
    fn converts_else_and_empty_control_branches() {
        let document = parse_ax_auto(
            r#"
page Home
<If when={ready}>
  <Copy>Ready</Copy>
  <Else>
    <Copy>Not ready</Copy>
  </Else>
</If>
<Each items={posts} as="post">
  <Card title={post.title} />
  <Empty>
    <Copy>No posts</Copy>
  </Empty>
</Each>
"#,
        )
        .expect("control branches should parse");

        let AxStatement::Component(if_fragment) = &document.page.body[0] else {
            panic!("if should convert into fragment component");
        };
        let AxBody::Block(if_body) = &if_fragment.body else {
            panic!("if fragment should contain converted control statements");
        };
        let AxStatement::If(if_block) = &if_body[0] else {
            panic!("expected if block");
        };
        assert_eq!(if_block.else_body.len(), 1);

        let AxStatement::Component(each_fragment) = &document.page.body[1] else {
            panic!("each should convert into fragment component");
        };
        let AxBody::Block(each_body) = &each_fragment.body else {
            panic!("each fragment should contain converted control statements");
        };
        let AxStatement::Each(each_block) = &each_body[0] else {
            panic!("expected each block");
        };
        assert_eq!(each_block.empty_body.len(), 1);
    }

    #[test]
    fn converts_match_cases_and_default_into_runtime_block() {
        let document = parse_ax_auto(
            r#"
page ThemePreview() {
  const theme: String = "gold"

  return ASX {
    <Match value={theme}>
      <Case is="silver"><Copy>Silver</Copy></Case>
      <Case is="gold"><Copy>Gold</Copy></Case>
      <Default><Copy>Custom</Copy></Default>
    </Match>
  }
}
"#,
        )
        .expect("match control element should parse");

        let AxStatement::Component(fragment) = &document.page.body[1] else {
            panic!("match should convert into a fragment component");
        };
        let AxBody::Block(body) = &fragment.body else {
            panic!("match fragment should contain a control statement");
        };
        let AxStatement::Match(match_block) = &body[0] else {
            panic!("expected match block");
        };

        assert_eq!(match_block.value, AxExpr::ident("theme"));
        assert_eq!(
            match_block
                .cases
                .iter()
                .map(|case| case.value.as_str())
                .collect::<Vec<_>>(),
            vec!["silver", "gold"]
        );
        assert_eq!(match_block.default_body.as_deref().map(<[_]>::len), Some(1));
    }

    #[test]
    fn rejects_non_literal_match_case_values() {
        let error = parse_ax_auto(
            r#"
page ThemePreview
<Match value={theme}>
  <Case is={otherTheme}><Copy>Theme</Copy></Case>
</Match>
"#,
        )
        .expect_err("match cases should require literal values");

        assert!(matches!(
            error,
            AxAutoParseError::Convert(AxConvertV2Error::InvalidMatchCaseValue)
        ));
    }

    #[test]
    fn supports_if_condition_alias_and_else_if_chain() {
        let document = parse_ax_auto(
            r#"
page Home
<If condition={is_ready}>
  <Copy>Ready</Copy>
  <ElseIf when={is_loading}>
    <Copy>Loading</Copy>
  </ElseIf>
  <Else>
    <Copy>Idle</Copy>
  </Else>
</If>
"#,
        )
        .expect("if condition alias and else if should parse");

        let AxStatement::Component(fragment) = &document.page.body[0] else {
            panic!("if should convert into fragment component");
        };
        let AxBody::Block(body) = &fragment.body else {
            panic!("fragment should contain converted if statement");
        };
        let AxStatement::If(if_block) = &body[0] else {
            panic!("expected top-level if block");
        };

        assert_eq!(if_block.condition, AxExpr::ident("is_ready"));
        let AxStatement::If(else_if) = &if_block.else_body[0] else {
            panic!("expected else-if to lower into nested if");
        };
        assert_eq!(else_if.condition, AxExpr::ident("is_loading"));
        assert_eq!(else_if.else_body.len(), 1);
    }

    #[test]
    fn rejects_nodes_after_else_branch() {
        let error = parse_ax_auto(
            r#"
page Home
<If when={ready}>
  <Copy>Ready</Copy>
  <Else>
    <Copy>Not ready</Copy>
  </Else>
  <Copy>Trailing</Copy>
</If>
"#,
        )
        .expect_err("trailing nodes after else should fail");

        assert!(matches!(
            error,
            AxAutoParseError::Convert(AxConvertV2Error::ControlBranchMustBeLast { .. })
        ));
    }

    #[test]
    fn supports_each_items_and_as_authoring_shape() {
        let document = parse_ax_auto(
            r#"
page Home
<Each items={posts} as="post">
  <Card title={post.title} />
</Each>
"#,
        )
        .expect("each items/as syntax should parse");

        let AxStatement::Component(fragment) = &document.page.body[0] else {
            panic!("each should convert into fragment component");
        };
        let AxBody::Block(body) = &fragment.body else {
            panic!("fragment should contain converted control statements");
        };
        let AxStatement::Each(each) = &body[0] else {
            panic!("expected each block");
        };

        assert_eq!(each.binding, "post");
        assert_eq!(each.source, AxExpr::ident("posts"));
    }
}
