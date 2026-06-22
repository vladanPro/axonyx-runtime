use std::collections::BTreeMap;
use thiserror::Error;

use crate::ax_ast::prelude::*;
use crate::ax_ast_v2::prelude::*;
use crate::ax_parser::{parse_ax, parse_expr, AxParseError};
use crate::ax_parser_v2::{parse_ax_v2, AxParseV2Error};
use crate::ax_semantics_v2::{validate_ax_v2_semantics, AxSemanticV2Error};

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
    #[error("`{attr}` must bind to a declared `state` signal")]
    UnknownStateBinding { attr: String },
    #[error("`{attr}` only supports expression bindings such as `{{theme}}`")]
    InvalidStateBinding { attr: String },
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
    let state_bindings = collect_state_bindings(file)?;

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
                body.push(AxStatement::component(convert_element(
                    element,
                    &state_bindings,
                )?));
            }
            AxNodeV2::Text(text) => body.push(AxStatement::text(text.value.clone())),
            AxNodeV2::Expr(expr) => body.push(AxStatement::text(parse_v2_expr(&expr.source)?)),
        }
    }

    Ok(AxDocument {
        imports: file.imports.iter().map(convert_import_decl).collect(),
        functions: file
            .functions
            .iter()
            .map(convert_function_decl)
            .collect::<Result<Vec<_>, _>>()?,
        components: file
            .components
            .iter()
            .map(|component| convert_component_decl(component, &state_bindings))
            .collect::<Result<Vec<_>, _>>()?,
        head,
        page: AxPage::with_params(
            file.page.name.clone(),
            file.page
                .params
                .iter()
                .map(convert_component_param_decl)
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

fn convert_function_decl(function: &AxFunctionDeclV2) -> Result<AxFunctionDef, AxConvertV2Error> {
    Ok(AxFunctionDef::new(
        function.name.clone(),
        function
            .params
            .iter()
            .map(convert_component_param_decl)
            .collect::<Result<Vec<_>, _>>()?,
        parse_v2_expr(&function.body)?,
    ))
}

fn convert_component_decl(
    component: &AxComponentDeclV2,
    state_bindings: &BTreeMap<String, StateBindingPlan>,
) -> Result<AxComponentDef, AxConvertV2Error> {
    Ok(AxComponentDef::new(
        component.name.clone(),
        component
            .params
            .iter()
            .map(convert_component_param_decl)
            .collect::<Result<Vec<_>, _>>()?,
        convert_children(&component.body, state_bindings)?,
    ))
}

fn convert_component_param_decl(
    param: &AxComponentParamDeclV2,
) -> Result<AxComponentParamDef, AxConvertV2Error> {
    Ok(match &param.default {
        Some(default) => {
            AxComponentParamDef::with_default(param.name.clone(), parse_v2_expr(default)?)
        }
        None => AxComponentParamDef::new(param.name.clone()),
    })
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

fn convert_element(
    element: &AxElementNode,
    state_bindings: &BTreeMap<String, StateBindingPlan>,
) -> Result<AxComponent, AxConvertV2Error> {
    if element.name == "Each" {
        return convert_each_element(element, state_bindings);
    }
    if element.name == "If" {
        return convert_if_element(element, state_bindings);
    }

    let mut component = AxComponent::new(element.name.clone());
    let mut class_attr_seen: Option<&str> = None;

    for attr in &element.attrs {
        match attr.name.as_str() {
            name if name.starts_with("bind:") => {
                component = apply_state_binding_attr(component, attr, state_bindings)?;
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
            _ => component = component.prop(attr.name.clone(), convert_attr_value(&attr.value)?),
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
            .map(|child| convert_child(child, state_bindings))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(component.block(body));
    }

    if element.children.len() == 1 {
        return match &element.children[0] {
            AxNodeV2::Text(text) => Ok(component.inline(AxExpr::string(text.value.clone()))),
            AxNodeV2::Expr(expr) => Ok(component.inline(parse_v2_expr(&expr.source)?)),
            AxNodeV2::Element(child) => Ok(component.block([AxStatement::component(
                convert_element(child, state_bindings)?,
            )])),
        };
    }

    Ok(component.block(convert_children(&element.children, state_bindings)?))
}

fn convert_children(
    children: &[AxNodeV2],
    state_bindings: &BTreeMap<String, StateBindingPlan>,
) -> Result<Vec<AxStatement>, AxConvertV2Error> {
    children
        .iter()
        .map(|child| convert_child(child, state_bindings))
        .collect()
}

fn convert_child(
    child: &AxNodeV2,
    state_bindings: &BTreeMap<String, StateBindingPlan>,
) -> Result<AxStatement, AxConvertV2Error> {
    match child {
        AxNodeV2::Element(element) if element.name == "Each" => {
            convert_each_statement(element, state_bindings)
        }
        AxNodeV2::Element(element) if element.name == "If" => {
            convert_if_statement(element, state_bindings)
        }
        AxNodeV2::Element(element) if element.name == "Else" || element.name == "Empty" => {
            Err(AxConvertV2Error::UnexpectedControlBranch {
                tag: "control-flow".to_string(),
                branch: element.name.clone(),
            })
        }
        AxNodeV2::Element(element) => Ok(AxStatement::component(convert_element(
            element,
            state_bindings,
        )?)),
        AxNodeV2::Text(text) => Ok(AxStatement::text(text.value.clone())),
        AxNodeV2::Expr(expr) => Ok(AxStatement::text(parse_v2_expr(&expr.source)?)),
    }
}

fn convert_each_statement(
    element: &AxElementNode,
    state_bindings: &BTreeMap<String, StateBindingPlan>,
) -> Result<AxStatement, AxConvertV2Error> {
    let binding = control_binding_attr(element, &["as", "item"])?;
    let source = control_expr_attr(element, &["items", "in", "of"])?;
    let (body, empty_body) = split_each_children(element, state_bindings)?;
    Ok(AxStatement::Each(
        AxEachBlock::new(binding, source, body).empty(empty_body),
    ))
}

fn convert_if_statement(
    element: &AxElementNode,
    state_bindings: &BTreeMap<String, StateBindingPlan>,
) -> Result<AxStatement, AxConvertV2Error> {
    let condition = control_expr_attr(element, &["when", "condition"])?;
    let (body, else_body) = split_if_children(element, state_bindings)?;
    Ok(AxStatement::If(
        AxIfBlock::new(condition, body).else_body(else_body),
    ))
}

fn convert_each_element(
    element: &AxElementNode,
    state_bindings: &BTreeMap<String, StateBindingPlan>,
) -> Result<AxComponent, AxConvertV2Error> {
    Ok(AxComponent::fragment([convert_each_statement(
        element,
        state_bindings,
    )?]))
}

fn convert_if_element(
    element: &AxElementNode,
    state_bindings: &BTreeMap<String, StateBindingPlan>,
) -> Result<AxComponent, AxConvertV2Error> {
    Ok(AxComponent::fragment([convert_if_statement(
        element,
        state_bindings,
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
    state_bindings: &BTreeMap<String, StateBindingPlan>,
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
                empty_body = Some(convert_children(&branch.children, state_bindings)?);
            }
            _ => body.push(convert_child(child, state_bindings)?),
        }
    }

    Ok((body, empty_body.unwrap_or_default()))
}

fn split_if_children(
    element: &AxElementNode,
    state_bindings: &BTreeMap<String, StateBindingPlan>,
) -> Result<(Vec<AxStatement>, Vec<AxStatement>), AxConvertV2Error> {
    split_if_children_from_slice(&element.name, &element.children, state_bindings)
}

fn split_if_children_from_slice(
    tag: &str,
    children: &[AxNodeV2],
    state_bindings: &BTreeMap<String, StateBindingPlan>,
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
                return Ok((body, convert_children(&branch.children, state_bindings)?));
            }
            AxNodeV2::Element(branch) if branch.name == "ElseIf" => {
                let condition = control_expr_attr(branch, &["when", "condition"])?;
                let nested_body = convert_children(&branch.children, state_bindings)?;
                let nested_else_body =
                    convert_if_tail_with_state(tag, &children[index + 1..], state_bindings)?;
                return Ok((
                    body,
                    vec![AxStatement::If(
                        AxIfBlock::new(condition, nested_body).else_body(nested_else_body),
                    )],
                ));
            }
            child => {
                body.push(convert_child(child, state_bindings)?);
                index += 1;
            }
        }
    }

    Ok((body, Vec::new()))
}

fn convert_if_tail_with_state(
    tag: &str,
    tail: &[AxNodeV2],
    state_bindings: &BTreeMap<String, StateBindingPlan>,
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
            convert_children(&branch.children, state_bindings)
        }
        AxNodeV2::Element(branch) if branch.name == "ElseIf" => {
            let condition = control_expr_attr(branch, &["when", "condition"])?;
            let nested_body = convert_children(&branch.children, state_bindings)?;
            let nested_else_body = convert_if_tail_with_state(tag, &tail[1..], state_bindings)?;
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StateInitializerPlan {
    value: AxExpr,
    ty: String,
}

fn collect_state_bindings(
    file: &AxFileV2,
) -> Result<BTreeMap<String, StateBindingPlan>, AxConvertV2Error> {
    let mut bindings = BTreeMap::new();
    for (index, state) in file.states.iter().enumerate() {
        let initializer = parse_state_initializer(&state.value)?;
        let ty = state.ty.clone().unwrap_or_else(|| initializer.ty.clone());
        bindings.insert(
            state.name.clone(),
            StateBindingPlan {
                signal_id: format!("root:{}:{}", state.name, index + 1),
                ty,
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
        ty: infer_state_type(&value).to_string(),
        value,
    })
}

fn infer_state_type(value: &AxExpr) -> &'static str {
    match value {
        AxExpr::String(_) => "String",
        AxExpr::Bool(_) => "Bool",
        AxExpr::Number(_) => "Number",
        AxExpr::List(_) => "List",
        _ => "Unknown",
    }
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
        .prop("data-ax-state-type", AxExpr::string(binding.ty.clone()));

    if matches!(bind_target, "value" | "checked")
        && !component.props.iter().any(|prop| prop.name == bind_target)
    {
        component = component.prop(bind_target.to_string(), AxExpr::ident(name));
    }

    Ok(component)
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
        assert!(!span.props.iter().any(|prop| prop.name == "text"));

        let AxStatement::Component(count_input) = &document.page.body[4] else {
            panic!("count input should convert into component");
        };
        assert!(count_input
            .props
            .contains(&AxProp::new("data-ax-state-type", AxExpr::string("Number"))));
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
