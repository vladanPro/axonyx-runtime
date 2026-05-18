use std::collections::BTreeMap;

use thiserror::Error;

use crate::ax_ast::prelude::*;
use crate::ax_parser_auto::prelude::parse_ax_auto;
use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxValue {
    Null,
    String(String),
    Number(i64),
    Bool(bool),
    Record(BTreeMap<String, AxValue>),
    List(Vec<AxValue>),
}

impl AxValue {
    pub fn record(fields: impl IntoIterator<Item = (impl Into<String>, AxValue)>) -> Self {
        let mut map = BTreeMap::new();
        for (key, value) in fields {
            map.insert(key.into(), value);
        }
        Self::Record(map)
    }

    pub fn list(items: impl IntoIterator<Item = AxValue>) -> Self {
        Self::List(items.into_iter().collect())
    }

    pub fn as_string(&self) -> String {
        match self {
            AxValue::Null => String::new(),
            AxValue::String(value) => value.clone(),
            AxValue::Number(value) => value.to_string(),
            AxValue::Bool(value) => value.to_string(),
            AxValue::Record(_) => "[record]".to_string(),
            AxValue::List(_) => "[list]".to_string(),
        }
    }
}

impl From<&str> for AxValue {
    fn from(value: &str) -> Self {
        AxValue::String(value.to_string())
    }
}

impl From<String> for AxValue {
    fn from(value: String) -> Self {
        AxValue::String(value)
    }
}

impl From<i64> for AxValue {
    fn from(value: i64) -> Self {
        AxValue::Number(value)
    }
}

impl From<bool> for AxValue {
    fn from(value: bool) -> Self {
        AxValue::Bool(value)
    }
}

pub trait AxDataResolver {
    fn resolve_call(&self, path: &[String], args: &[AxValue]) -> Option<AxValue>;
}

impl<F> AxDataResolver for F
where
    F: Fn(&[String], &[AxValue]) -> Option<AxValue>,
{
    fn resolve_call(&self, path: &[String], args: &[AxValue]) -> Option<AxValue> {
        self(path, args)
    }
}

pub trait AxImportResolver {
    fn resolve_import_source(&self, source: &str) -> Option<String>;
}

impl<F> AxImportResolver for F
where
    F: Fn(&str) -> Option<String>,
{
    fn resolve_import_source(&self, source: &str) -> Option<String> {
        self(source)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AxLowerError {
    #[error("unknown identifier `{name}`")]
    UnknownIdentifier { name: String },
    #[error("unknown member `{property}` on current value")]
    UnknownMember { property: String },
    #[error("unsupported call `{path}`")]
    UnsupportedCall { path: String },
    #[error("`each` requires a list source")]
    EachRequiresList,
    #[error("failed to parse imported component from `{import_path}`: {message}")]
    ImportedComponentParse {
        import_path: String,
        message: String,
    },
}

pub fn lower_document(
    document: &AxDocument,
    resolver: &impl AxDataResolver,
) -> Result<AxNode, AxLowerError> {
    lower_document_with_scope(document, BTreeMap::new(), resolver)
}

pub fn lower_document_with_scope(
    document: &AxDocument,
    initial_scope: BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
) -> Result<AxNode, AxLowerError> {
    let import_resolver = |_: &str| None;
    lower_document_with_scope_and_imports(document, initial_scope, resolver, &import_resolver)
}

pub fn lower_document_with_scope_and_imports(
    document: &AxDocument,
    initial_scope: BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
    import_resolver: &impl AxImportResolver,
) -> Result<AxNode, AxLowerError> {
    let mut scope = initial_scope;
    let children = lower_statements(
        &document.page.body,
        &document.functions,
        &document.imports,
        &document.components,
        &mut scope,
        resolver,
        import_resolver,
        None,
    )?;

    Ok(element_with_attrs(
        "main",
        vec![
            attr("data-ax-page", document.page.name.clone()),
            attr("data-ax-root", "page"),
        ],
        children,
    ))
}

struct SlotContext<'a> {
    body: &'a [AxStatement],
    functions: &'a [AxFunctionDef],
    imports: &'a [AxImport],
    components: &'a [AxComponentDef],
    parent: Option<&'a SlotContext<'a>>,
}

fn lower_statements(
    statements: &[AxStatement],
    functions: &[AxFunctionDef],
    imports: &[AxImport],
    components: &[AxComponentDef],
    scope: &mut BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
    import_resolver: &impl AxImportResolver,
    slot_context: Option<&SlotContext<'_>>,
) -> Result<Vec<AxNode>, AxLowerError> {
    let mut nodes = Vec::new();

    for statement in statements {
        match statement {
            AxStatement::Data(binding) => {
                let value = eval_expr(&binding.value, functions, scope, resolver)?;
                scope.insert(binding.name.clone(), value);
            }
            AxStatement::Each(block) => {
                let source = eval_expr(&block.source, functions, scope, resolver)?;
                let AxValue::List(items) = source else {
                    return Err(AxLowerError::EachRequiresList);
                };

                if items.is_empty() {
                    let mut nested = scope.clone();
                    nodes.extend(lower_statements(
                        &block.empty_body,
                        functions,
                        imports,
                        components,
                        &mut nested,
                        resolver,
                        import_resolver,
                        slot_context,
                    )?);
                } else {
                    for item in items {
                        let mut nested = scope.clone();
                        nested.insert(block.binding.clone(), item);
                        nodes.extend(lower_statements(
                            &block.body,
                            functions,
                            imports,
                            components,
                            &mut nested,
                            resolver,
                            import_resolver,
                            slot_context,
                        )?);
                    }
                }
            }
            AxStatement::If(block) => {
                let condition = eval_expr(&block.condition, functions, scope, resolver)?;
                let body = if is_truthy(&condition) {
                    &block.body
                } else {
                    &block.else_body
                };
                if !body.is_empty() {
                    let mut nested = scope.clone();
                    nodes.extend(lower_statements(
                        body,
                        functions,
                        imports,
                        components,
                        &mut nested,
                        resolver,
                        import_resolver,
                        slot_context,
                    )?);
                }
            }
            AxStatement::Text(expr) => {
                nodes.push(text(
                    eval_expr(expr, functions, scope, resolver)?.as_string(),
                ));
            }
            AxStatement::Component(component) => {
                nodes.extend(lower_component_nodes(
                    component,
                    functions,
                    imports,
                    components,
                    scope,
                    resolver,
                    import_resolver,
                    slot_context,
                )?);
            }
            AxStatement::Pipeline(pipeline) => {
                nodes.push(lower_pipeline(
                    pipeline,
                    functions,
                    imports,
                    components,
                    scope,
                    resolver,
                    import_resolver,
                    slot_context,
                )?);
            }
        }
    }

    Ok(nodes)
}

fn lower_component_nodes(
    component: &AxComponent,
    functions: &[AxFunctionDef],
    imports: &[AxImport],
    components: &[AxComponentDef],
    scope: &mut BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
    import_resolver: &impl AxImportResolver,
    slot_context: Option<&SlotContext<'_>>,
) -> Result<Vec<AxNode>, AxLowerError> {
    if component.name == "Slot" {
        let Some(slot_context) = slot_context else {
            return Ok(Vec::new());
        };
        let requested_slot = slot_name_expr(component, slot_context.functions, scope, resolver)?;
        let selected_body = select_slot_statements(
            slot_context.body,
            requested_slot.as_deref(),
            slot_context.functions,
            scope,
            resolver,
        )?;
        let mut nested = scope.clone();
        return lower_statements(
            &selected_body,
            slot_context.functions,
            slot_context.imports,
            slot_context.components,
            &mut nested,
            resolver,
            import_resolver,
            slot_context.parent,
        );
    }

    if component.name == "Fragment" {
        return lower_component_children(
            component,
            functions,
            imports,
            components,
            scope,
            resolver,
            import_resolver,
            slot_context,
        );
    }

    if let Some(component_def) = resolve_component(components, &component.name) {
        return lower_local_component_nodes(
            component,
            component_def,
            functions,
            imports,
            components,
            scope,
            resolver,
            import_resolver,
            slot_context,
        );
    }

    if let Some(import_decl) = resolve_import(imports, &component.name) {
        if let Some(import_source) = import_resolver.resolve_import_source(import_decl.source) {
            return lower_imported_component_nodes(
                component,
                import_decl,
                functions,
                imports,
                components,
                scope,
                resolver,
                import_resolver,
                &import_source,
                slot_context,
            );
        }
    }

    Ok(vec![lower_component_node(
        component,
        functions,
        imports,
        components,
        scope,
        resolver,
        import_resolver,
        slot_context,
    )?])
}

fn lower_component_node(
    component: &AxComponent,
    functions: &[AxFunctionDef],
    imports: &[AxImport],
    components: &[AxComponentDef],
    scope: &mut BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
    import_resolver: &impl AxImportResolver,
    slot_context: Option<&SlotContext<'_>>,
) -> Result<AxNode, AxLowerError> {
    let children = lower_component_children(
        component,
        functions,
        imports,
        components,
        scope,
        resolver,
        import_resolver,
        slot_context,
    )?;

    let mut props = eval_props(component, functions, scope, resolver)?;
    let mut attrs = style_attrs(&component.style, functions, scope, resolver)?;

    let node = match component.name.as_str() {
        name if resolve_import(imports, name).is_some() => {
            let import_decl = resolve_import(imports, name).expect("checked above");
            attrs.insert(0, attr("data-component", component.name.clone()));
            attrs.push(attr_boxed(
                "data-import-source".to_string(),
                import_decl.source.to_string(),
            ));
            attrs.push(attr_boxed(
                "data-import-name".to_string(),
                import_decl.binding.imported.clone(),
            ));
            attrs.push(attr_boxed(
                "data-import-local".to_string(),
                import_decl.binding.local.clone(),
            ));
            push_behavior_props(&mut attrs, &mut props);
            push_remaining_props(&mut attrs, props);
            element_with_attrs("div", attrs, children)
        }
        "Container" => {
            prepend_class_attr(&mut attrs, "ax-container");
            attrs.insert(
                attrs.len().min(1),
                attr(
                    "data-max",
                    prop_string(&mut props, &["max", "max_width"])
                        .unwrap_or_else(|| "xl".to_string()),
                ),
            );
            push_behavior_props(&mut attrs, &mut props);
            push_remaining_props(&mut attrs, props);
            element_with_attrs("div", attrs, children)
        }
        "Grid" => {
            prepend_class_attr(&mut attrs, "ax-grid");
            attrs.insert(
                attrs.len().min(1),
                attr(
                    "data-cols",
                    prop_string(&mut props, &["cols"]).unwrap_or_else(|| "1".to_string()),
                ),
            );
            attrs.insert(
                attrs.len().min(2),
                attr(
                    "data-gap",
                    prop_string(&mut props, &["gap"]).unwrap_or_else(|| "md".to_string()),
                ),
            );
            push_behavior_props(&mut attrs, &mut props);
            push_remaining_props(&mut attrs, props);
            element_with_attrs("div", attrs, children)
        }
        "Card" => {
            prepend_class_attr(&mut attrs, "ax-card");
            let title = prop_string(&mut props, &["title"]);
            push_behavior_props(&mut attrs, &mut props);
            push_remaining_props(&mut attrs, props);
            let mut body = Vec::new();
            if let Some(title) = title {
                body.push(element_with_attrs(
                    "h2",
                    vec![attr("class", "ax-card__title")],
                    vec![text(title)],
                ));
            }
            body.extend(children);
            element_with_attrs("article", attrs, body)
        }
        "Copy" => {
            prepend_class_attr(&mut attrs, "ax-copy");
            let tag = prop_string(&mut props, &["as", "tag"]).unwrap_or_else(|| "p".to_string());
            push_behavior_props(&mut attrs, &mut props);
            push_remaining_props(&mut attrs, props);
            element_with_attrs(leak_tag(tag), attrs, children)
        }
        "Html" => {
            prepend_class_attr(&mut attrs, "ax-html");
            let content = prop_string(&mut props, &["content", "html"]).unwrap_or_else(|| {
                children
                    .iter()
                    .map(|child| match child {
                        AxNode::Text(value) | AxNode::RawHtml(value) => value.clone(),
                        AxNode::Element { .. } => String::new(),
                    })
                    .collect::<Vec<_>>()
                    .join("")
            });
            push_behavior_props(&mut attrs, &mut props);
            push_remaining_props(&mut attrs, props);
            element_with_attrs("div", attrs, vec![raw_html(content)])
        }
        "Button" => {
            prepend_class_attr(&mut attrs, "ax-button");
            push_selected_native_props(&mut attrs, &mut props, &["type", "name", "value", "form"]);
            push_behavior_props(&mut attrs, &mut props);
            push_remaining_props(&mut attrs, props);
            element_with_attrs("button", attrs, children)
        }
        "ActionForm" => {
            prepend_class_attr(&mut attrs, "ax-form");
            let method = prop_string(&mut props, &["method"]).unwrap_or_else(|| "post".to_string());
            let action = prop_string(&mut props, &["action"]).or_else(|| {
                prop_string(&mut props, &["name", "actionName"]).and_then(|name| {
                    resolver
                        .resolve_call(&["action".to_string()], &[AxValue::String(name)])
                        .map(|value| value.as_string())
                })
            });
            let patch = prop_bool(&mut props, &["patch"]).unwrap_or(true);

            attrs.push(attr("method", method));
            if let Some(action) = action {
                attrs.push(attr("action", action));
            }
            push_behavior_props(&mut attrs, &mut props);
            push_remaining_props(&mut attrs, props);

            let mut body = Vec::new();
            if patch {
                body.push(element_with_attrs(
                    "input",
                    vec![
                        attr("type", "hidden"),
                        attr("name", "__ax_patch"),
                        attr("value", "1"),
                    ],
                    vec![],
                ));
            }
            body.extend(children);
            element_with_attrs("form", attrs, body)
        }
        tag if is_native_html_tag(tag) => {
            push_behavior_props(&mut attrs, &mut props);
            push_native_props(&mut attrs, props);
            element_with_attrs(leak_tag(tag.to_string()), attrs, children)
        }
        other => {
            attrs.insert(0, attr("data-component", other.to_string()));
            push_behavior_props(&mut attrs, &mut props);
            push_remaining_props(&mut attrs, props);
            element_with_attrs("div", attrs, children)
        }
    };

    Ok(node)
}

fn lower_component_children(
    component: &AxComponent,
    functions: &[AxFunctionDef],
    imports: &[AxImport],
    components: &[AxComponentDef],
    scope: &mut BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
    import_resolver: &impl AxImportResolver,
    slot_context: Option<&SlotContext<'_>>,
) -> Result<Vec<AxNode>, AxLowerError> {
    match &component.body {
        AxBody::Empty => Ok(Vec::new()),
        AxBody::Inline(expr) => Ok(vec![text(
            eval_expr(expr, functions, scope, resolver)?.as_string(),
        )]),
        AxBody::Block(body) => {
            let mut nested = scope.clone();
            lower_statements(
                body,
                functions,
                imports,
                components,
                &mut nested,
                resolver,
                import_resolver,
                slot_context,
            )
        }
    }
}

fn lower_pipeline(
    pipeline: &AxPipeline,
    functions: &[AxFunctionDef],
    imports: &[AxImport],
    components: &[AxComponentDef],
    scope: &mut BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
    import_resolver: &impl AxImportResolver,
    slot_context: Option<&SlotContext<'_>>,
) -> Result<AxNode, AxLowerError> {
    let source = eval_expr(&pipeline.source, functions, scope, resolver)?;
    let source_text = source.as_string();

    let mut attrs = vec![attr("data-ax-pipeline", "true")];
    if let AxValue::List(items) = &source {
        attrs.push(attr("data-items", items.len().to_string()));
    }

    let mut children = vec![text(source_text)];
    for stage in &pipeline.stages {
        match stage {
            AxPipelineStage::Each(each) => {
                children.push(element_with_attrs(
                    "div",
                    vec![
                        attr("data-stage", "each"),
                        attr("data-binding", each.binding.clone()),
                    ],
                    vec![],
                ));
            }
            AxPipelineStage::Component(component) => {
                let mut nested_scope = scope.clone();
                children.extend(lower_component_nodes(
                    component,
                    functions,
                    imports,
                    components,
                    &mut nested_scope,
                    resolver,
                    import_resolver,
                    slot_context,
                )?);
            }
        }
    }

    Ok(element_with_attrs("section", attrs, children))
}

fn eval_props(
    component: &AxComponent,
    functions: &[AxFunctionDef],
    scope: &BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
) -> Result<BTreeMap<String, AxValue>, AxLowerError> {
    let mut props = BTreeMap::new();
    for prop in &component.props {
        props.insert(
            prop.name.clone(),
            eval_expr(&prop.value, functions, scope, resolver)?,
        );
    }
    Ok(props)
}

fn style_attrs(
    style: &AxStyle,
    functions: &[AxFunctionDef],
    scope: &BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
) -> Result<Vec<Attribute>, AxLowerError> {
    let mut attrs = Vec::new();
    if let Some(recipe) = &style.recipe {
        attrs.push(attr(
            "data-recipe",
            eval_expr(recipe, functions, scope, resolver)?.as_string(),
        ));
    }
    if let Some(class) = &style.class {
        attrs.push(attr(
            "class",
            eval_expr(class, functions, scope, resolver)?.as_string(),
        ));
    }
    Ok(attrs)
}

fn prepend_class_attr(attrs: &mut Vec<Attribute>, class_name: &str) {
    if let Some(existing) = attrs.iter_mut().find(|attr| attr.name == "class") {
        if existing.value.is_empty() {
            existing.value = class_name.to_string();
        } else {
            existing.value = format!("{class_name} {}", existing.value);
        }
        return;
    }

    attrs.insert(0, attr("class", class_name));
}

fn eval_expr(
    expr: &AxExpr,
    functions: &[AxFunctionDef],
    scope: &BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
) -> Result<AxValue, AxLowerError> {
    match expr {
        AxExpr::String(value) => Ok(AxValue::String(value.clone())),
        AxExpr::Number(value) => Ok(AxValue::Number(*value)),
        AxExpr::Bool(value) => Ok(AxValue::Bool(*value)),
        AxExpr::Identifier(name) => scope
            .get(name)
            .cloned()
            .ok_or_else(|| AxLowerError::UnknownIdentifier { name: name.clone() }),
        AxExpr::Member { object, property } => {
            let value = eval_expr(object, functions, scope, resolver)?;
            match value {
                AxValue::Record(fields) => {
                    Ok(fields.get(property).cloned().unwrap_or(AxValue::Null))
                }
                _ => Err(AxLowerError::UnknownMember {
                    property: property.clone(),
                }),
            }
        }
        AxExpr::OptionalMember { object, property } => {
            let value = match eval_expr(object, functions, scope, resolver) {
                Ok(value) => value,
                Err(AxLowerError::UnknownIdentifier { .. })
                | Err(AxLowerError::UnknownMember { .. }) => return Ok(AxValue::Null),
                Err(error) => return Err(error),
            };
            match value {
                AxValue::Record(fields) => {
                    Ok(fields.get(property).cloned().unwrap_or(AxValue::Null))
                }
                AxValue::Null => Ok(AxValue::Null),
                _ => Ok(AxValue::Null),
            }
        }
        AxExpr::Call { path, args } => {
            let args = args
                .iter()
                .map(|arg| eval_expr(arg, functions, scope, resolver))
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(function) = resolve_function(functions, path) {
                return eval_function_call(function, &args, functions, scope, resolver);
            }
            resolver
                .resolve_call(path, &args)
                .ok_or_else(|| AxLowerError::UnsupportedCall {
                    path: path.join("."),
                })
        }
    }
}

fn resolve_function<'a>(
    functions: &'a [AxFunctionDef],
    path: &[String],
) -> Option<&'a AxFunctionDef> {
    let [name] = path else {
        return None;
    };

    functions
        .iter()
        .rev()
        .find(|function| function.name == *name)
}

fn eval_function_call(
    function: &AxFunctionDef,
    args: &[AxValue],
    functions: &[AxFunctionDef],
    scope: &BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
) -> Result<AxValue, AxLowerError> {
    let mut function_scope = scope.clone();

    for (index, param) in function.params.iter().enumerate() {
        let value = if let Some(value) = args.get(index) {
            value.clone()
        } else if let Some(default) = &param.default {
            eval_expr(default, functions, &function_scope, resolver)?
        } else {
            AxValue::Null
        };
        function_scope.insert(param.name.clone(), value);
    }

    eval_expr(&function.body, functions, &function_scope, resolver)
}

fn prop_string(props: &mut BTreeMap<String, AxValue>, names: &[&str]) -> Option<String> {
    for name in names {
        if let Some(value) = props.remove(*name) {
            return Some(value.as_string());
        }
    }
    None
}

fn prop_bool(props: &mut BTreeMap<String, AxValue>, names: &[&str]) -> Option<bool> {
    for name in names {
        if let Some(value) = props.remove(*name) {
            return Some(match value {
                AxValue::Bool(value) => value,
                AxValue::String(value) => matches!(value.as_str(), "true" | "1" | "yes" | "on"),
                AxValue::Number(value) => value != 0,
                AxValue::Null => false,
                AxValue::Record(fields) => !fields.is_empty(),
                AxValue::List(items) => !items.is_empty(),
            });
        }
    }
    None
}

fn is_truthy(value: &AxValue) -> bool {
    match value {
        AxValue::Null => false,
        AxValue::String(value) => !value.is_empty(),
        AxValue::Number(value) => *value != 0,
        AxValue::Bool(value) => *value,
        AxValue::Record(fields) => !fields.is_empty(),
        AxValue::List(items) => !items.is_empty(),
    }
}

struct ResolvedImport<'a> {
    binding: &'a AxImportBinding,
    source: &'a str,
}

fn resolve_import<'a>(imports: &'a [AxImport], local_name: &str) -> Option<ResolvedImport<'a>> {
    for import_decl in imports.iter().rev() {
        for binding in import_decl.bindings.iter().rev() {
            if binding.local == local_name {
                return Some(ResolvedImport {
                    binding,
                    source: &import_decl.source,
                });
            }
        }
    }

    None
}

fn resolve_component<'a>(
    components: &'a [AxComponentDef],
    local_name: &str,
) -> Option<&'a AxComponentDef> {
    components
        .iter()
        .rev()
        .find(|component| component.name == local_name)
}

fn lower_local_component_nodes(
    component: &AxComponent,
    component_def: &AxComponentDef,
    functions: &[AxFunctionDef],
    imports: &[AxImport],
    components: &[AxComponentDef],
    scope: &mut BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
    import_resolver: &impl AxImportResolver,
    slot_context: Option<&SlotContext<'_>>,
) -> Result<Vec<AxNode>, AxLowerError> {
    let props = eval_props(component, functions, scope, resolver)?;
    let mut component_scope = scope.clone();

    for param in &component_def.params {
        let value = if let Some(value) = props.get(&param.name) {
            value.clone()
        } else if let Some(default) = &param.default {
            eval_expr(default, functions, &component_scope, resolver)?
        } else {
            AxValue::Null
        };
        component_scope.insert(param.name.clone(), value);
    }

    let slot_body = component_children_to_statements(component);
    let slot_context = SlotContext {
        body: &slot_body,
        functions,
        imports,
        components,
        parent: slot_context,
    };

    lower_statements(
        &component_def.body,
        functions,
        imports,
        components,
        &mut component_scope,
        resolver,
        import_resolver,
        Some(&slot_context),
    )
}

fn lower_imported_component_nodes(
    component: &AxComponent,
    import_decl: ResolvedImport<'_>,
    functions: &[AxFunctionDef],
    imports: &[AxImport],
    components: &[AxComponentDef],
    scope: &mut BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
    import_resolver: &impl AxImportResolver,
    import_source: &str,
    slot_context: Option<&SlotContext<'_>>,
) -> Result<Vec<AxNode>, AxLowerError> {
    let document =
        parse_ax_auto(import_source).map_err(|error| AxLowerError::ImportedComponentParse {
            import_path: import_decl.source.to_string(),
            message: error.to_string(),
        })?;

    let mut imported_scope = scope.clone();
    for (name, value) in eval_props(component, functions, scope, resolver)? {
        imported_scope.insert(name, value);
    }

    if let Some(class) = &component.style.class {
        imported_scope.insert(
            "class".to_string(),
            eval_expr(class, functions, scope, resolver)?,
        );
    }
    if let Some(recipe) = &component.style.recipe {
        imported_scope.insert(
            "recipe".to_string(),
            eval_expr(recipe, functions, scope, resolver)?,
        );
    }

    let slot_body = component_children_to_statements(component);
    let slot_context = SlotContext {
        body: &slot_body,
        functions,
        imports,
        components,
        parent: slot_context,
    };

    lower_statements(
        &document.page.body,
        &document.functions,
        &document.imports,
        &document.components,
        &mut imported_scope,
        resolver,
        import_resolver,
        Some(&slot_context),
    )
}

fn component_children_to_statements(component: &AxComponent) -> Vec<AxStatement> {
    match &component.body {
        AxBody::Empty => Vec::new(),
        AxBody::Inline(expr) => vec![AxStatement::text(expr.clone())],
        AxBody::Block(body) => body.clone(),
    }
}

fn slot_name_expr(
    component: &AxComponent,
    functions: &[AxFunctionDef],
    scope: &BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
) -> Result<Option<String>, AxLowerError> {
    for prop in &component.props {
        if prop.name == "name" || prop.name == "slot" {
            return Ok(Some(
                eval_expr(&prop.value, functions, scope, resolver)?.as_string(),
            ));
        }
    }

    Ok(None)
}

fn select_slot_statements(
    statements: &[AxStatement],
    requested_slot: Option<&str>,
    functions: &[AxFunctionDef],
    scope: &BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
) -> Result<Vec<AxStatement>, AxLowerError> {
    let mut selected = Vec::new();

    for statement in statements {
        if slot_statement_matches(statement, requested_slot, functions, scope, resolver)? {
            selected.push(strip_slot_statement(statement));
        }
    }

    Ok(selected)
}

fn slot_statement_matches(
    statement: &AxStatement,
    requested_slot: Option<&str>,
    functions: &[AxFunctionDef],
    scope: &BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
) -> Result<bool, AxLowerError> {
    let statement_slot = statement_slot_name(statement, functions, scope, resolver)?;

    Ok(match requested_slot {
        Some(name) => statement_slot.as_deref() == Some(name),
        None => statement_slot.is_none(),
    })
}

fn statement_slot_name(
    statement: &AxStatement,
    functions: &[AxFunctionDef],
    scope: &BTreeMap<String, AxValue>,
    resolver: &impl AxDataResolver,
) -> Result<Option<String>, AxLowerError> {
    let AxStatement::Component(component) = statement else {
        return Ok(None);
    };

    for prop in &component.props {
        if prop.name == "slot" {
            return Ok(Some(
                eval_expr(&prop.value, functions, scope, resolver)?.as_string(),
            ));
        }
    }

    Ok(None)
}

fn strip_slot_statement(statement: &AxStatement) -> AxStatement {
    match statement {
        AxStatement::Component(component) => {
            let mut component = component.clone();
            component.props.retain(|prop| prop.name != "slot");
            AxStatement::Component(component)
        }
        other => other.clone(),
    }
}

fn push_remaining_props(attrs: &mut Vec<Attribute>, props: BTreeMap<String, AxValue>) {
    for (name, value) in props {
        attrs.push(attr_boxed(format!("data-{name}"), value.as_string()));
    }
}

fn push_behavior_props(attrs: &mut Vec<Attribute>, props: &mut BTreeMap<String, AxValue>) {
    let mappings = [
        ("behavior", "data-ax-behavior"),
        ("behavior_target", "data-ax-behavior-target"),
        ("behaviorTarget", "data-ax-behavior-target"),
        ("behavior_action", "data-ax-behavior-action"),
        ("behaviorAction", "data-ax-behavior-action"),
        ("behavior_value", "data-ax-behavior-value"),
        ("behaviorValue", "data-ax-behavior-value"),
    ];

    for (prop_name, attr_name) in mappings {
        if let Some(value) = props.remove(prop_name) {
            attrs.push(attr_boxed(attr_name.to_string(), value.as_string()));
        }
    }
}

fn push_selected_native_props(
    attrs: &mut Vec<Attribute>,
    props: &mut BTreeMap<String, AxValue>,
    names: &[&str],
) {
    for name in names {
        if let Some(value) = props.remove(*name) {
            attrs.push(attr_boxed((*name).to_string(), value.as_string()));
        }
    }
}

fn push_native_props(attrs: &mut Vec<Attribute>, props: BTreeMap<String, AxValue>) {
    for (name, value) in props {
        attrs.push(attr_boxed(name, value.as_string()));
    }
}

fn is_native_html_tag(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    if !first.is_ascii_lowercase() {
        return false;
    }

    chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

fn attr_boxed(name: String, value: String) -> Attribute {
    Attribute {
        name: Box::leak(name.into_boxed_str()),
        value,
    }
}

fn leak_tag(tag: String) -> &'static str {
    Box::leak(tag.into_boxed_str())
}

pub mod prelude {
    pub use super::lower_document;
    pub use super::lower_document_with_scope;
    pub use super::lower_document_with_scope_and_imports;
    pub use super::AxDataResolver;
    pub use super::AxImportResolver;
    pub use super::AxLowerError;
    pub use super::AxValue;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ax_parser::parse_ax;

    #[test]
    fn lowers_indentation_first_page_into_ax_node() {
        let document = parse_ax(
            r#"
page Home
  data posts = Db.Stream("posts")

  Container max: "xl"
    Grid cols: 3, gap: "md", recipe: "screen-center"
      each post in posts
        Card title: post.title
          Copy -> post.excerpt
"#,
        )
        .expect("document should parse");

        let resolver = |path: &[String], args: &[AxValue]| -> Option<AxValue> {
            if path == ["Db".to_string(), "Stream".to_string()]
                && args == [AxValue::String("posts".to_string())]
            {
                return Some(AxValue::list([
                    AxValue::record([
                        ("title", AxValue::from("Card A")),
                        ("excerpt", AxValue::from("Intro A")),
                    ]),
                    AxValue::record([
                        ("title", AxValue::from("Card B")),
                        ("excerpt", AxValue::from("Intro B")),
                    ]),
                ]));
            }

            None
        };

        let node = lower_document(&document, &resolver).expect("document should lower");

        assert_eq!(
            node,
            AxNode::Element {
                tag: "main",
                attrs: vec![
                    Attribute {
                        name: "data-ax-page",
                        value: "Home".to_string()
                    },
                    Attribute {
                        name: "data-ax-root",
                        value: "page".to_string()
                    },
                ],
                children: vec![AxNode::Element {
                    tag: "div",
                    attrs: vec![
                        Attribute {
                            name: "class",
                            value: "ax-container".to_string()
                        },
                        Attribute {
                            name: "data-max",
                            value: "xl".to_string()
                        },
                    ],
                    children: vec![AxNode::Element {
                        tag: "div",
                        attrs: vec![
                            Attribute {
                                name: "class",
                                value: "ax-grid".to_string()
                            },
                            Attribute {
                                name: "data-cols",
                                value: "3".to_string()
                            },
                            Attribute {
                                name: "data-gap",
                                value: "md".to_string()
                            },
                            Attribute {
                                name: "data-recipe",
                                value: "screen-center".to_string()
                            },
                        ],
                        children: vec![
                            AxNode::Element {
                                tag: "article",
                                attrs: vec![Attribute {
                                    name: "class",
                                    value: "ax-card".to_string()
                                }],
                                children: vec![
                                    AxNode::Element {
                                        tag: "h2",
                                        attrs: vec![Attribute {
                                            name: "class",
                                            value: "ax-card__title".to_string()
                                        }],
                                        children: vec![AxNode::Text("Card A".to_string())],
                                    },
                                    AxNode::Element {
                                        tag: "p",
                                        attrs: vec![Attribute {
                                            name: "class",
                                            value: "ax-copy".to_string()
                                        }],
                                        children: vec![AxNode::Text("Intro A".to_string())],
                                    },
                                ],
                            },
                            AxNode::Element {
                                tag: "article",
                                attrs: vec![Attribute {
                                    name: "class",
                                    value: "ax-card".to_string()
                                }],
                                children: vec![
                                    AxNode::Element {
                                        tag: "h2",
                                        attrs: vec![Attribute {
                                            name: "class",
                                            value: "ax-card__title".to_string()
                                        }],
                                        children: vec![AxNode::Text("Card B".to_string())],
                                    },
                                    AxNode::Element {
                                        tag: "p",
                                        attrs: vec![Attribute {
                                            name: "class",
                                            value: "ax-copy".to_string()
                                        }],
                                        children: vec![AxNode::Text("Intro B".to_string())],
                                    },
                                ],
                            },
                        ],
                    }],
                }],
            }
        );
    }

    #[test]
    fn lowers_native_html_tags_with_real_attributes() {
        let document = parse_ax(
            r#"
page Home
  section class: "hero-shell"
    a href: "/docs", target: "_blank" -> "Read docs"
"#,
        )
        .expect("document should parse");

        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };
        let node = lower_document(&document, &resolver).expect("document should lower");

        assert_eq!(
            node,
            AxNode::Element {
                tag: "main",
                attrs: vec![
                    Attribute {
                        name: "data-ax-page",
                        value: "Home".to_string(),
                    },
                    Attribute {
                        name: "data-ax-root",
                        value: "page".to_string(),
                    },
                ],
                children: vec![AxNode::Element {
                    tag: "section",
                    attrs: vec![Attribute {
                        name: "class",
                        value: "hero-shell".to_string(),
                    }],
                    children: vec![AxNode::Element {
                        tag: "a",
                        attrs: vec![
                            Attribute {
                                name: "href",
                                value: "/docs".to_string(),
                            },
                            Attribute {
                                name: "target",
                                value: "_blank".to_string(),
                            },
                        ],
                        children: vec![AxNode::Text("Read docs".to_string())],
                    }],
                }],
            }
        );
    }

    #[test]
    fn button_component_keeps_submit_type_as_real_attribute() {
        let document = parse_ax(
            r#"
page Home
  Button type: "submit", tone: "primary" -> "Create"
"#,
        )
        .expect("document should parse");

        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };
        let node = lower_document(&document, &resolver).expect("document should lower");

        let AxNode::Element { children, .. } = node else {
            panic!("expected page root");
        };
        let AxNode::Element { attrs, .. } = &children[0] else {
            panic!("expected button");
        };

        assert!(attrs
            .iter()
            .any(|attr| attr.name == "type" && attr.value == "submit"));
        assert!(attrs
            .iter()
            .any(|attr| attr.name == "data-tone" && attr.value == "primary"));
    }

    #[test]
    fn component_behavior_props_emit_ax_behavior_attributes() {
        let document = parse_ax_auto(
            r##"
page Home

<Button
  behavior="toggle"
  behaviorTarget="#menu"
  behaviorAction="open"
  behaviorValue="expanded"
  tone="primary"
>
  Menu
</Button>
"##,
        )
        .expect("document should parse");
        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };

        let node = lower_document(&document, &resolver).expect("document should lower");

        let AxNode::Element { children, .. } = node else {
            panic!("expected page root");
        };
        let AxNode::Element { attrs, .. } = &children[0] else {
            panic!("expected button");
        };

        assert!(attrs
            .iter()
            .any(|attr| attr.name == "data-ax-behavior" && attr.value == "toggle"));
        assert!(attrs
            .iter()
            .any(|attr| attr.name == "data-ax-behavior-target" && attr.value == "#menu"));
        assert!(attrs
            .iter()
            .any(|attr| attr.name == "data-ax-behavior-action" && attr.value == "open"));
        assert!(attrs
            .iter()
            .any(|attr| attr.name == "data-ax-behavior-value" && attr.value == "expanded"));
        assert!(attrs
            .iter()
            .any(|attr| attr.name == "data-tone" && attr.value == "primary"));
        assert!(!attrs.iter().any(|attr| attr.name == "data-behavior"));
    }

    #[test]
    fn native_behavior_props_emit_ax_behavior_attributes() {
        let document = parse_ax_auto(
            r##"
page Home

<button behavior="dialog" behavior_target="#modal" type="button">
  Open
</button>
"##,
        )
        .expect("document should parse");
        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };

        let node = lower_document(&document, &resolver).expect("document should lower");

        let AxNode::Element { children, .. } = node else {
            panic!("expected page root");
        };
        let AxNode::Element { attrs, .. } = &children[0] else {
            panic!("expected native button");
        };

        assert!(attrs
            .iter()
            .any(|attr| attr.name == "data-ax-behavior" && attr.value == "dialog"));
        assert!(attrs
            .iter()
            .any(|attr| attr.name == "data-ax-behavior-target" && attr.value == "#modal"));
        assert!(attrs
            .iter()
            .any(|attr| attr.name == "type" && attr.value == "button"));
        assert!(!attrs.iter().any(|attr| attr.name == "behavior"));
    }

    #[test]
    fn lowers_fragment_component_without_extra_wrapper() {
        let document = AxDocument::page(
            "Home",
            [AxStatement::component(AxComponent::fragment([
                AxStatement::text("Hello "),
                AxStatement::component(AxComponent::new("strong").inline("Axonyx")),
            ]))],
        );
        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };

        let node = lower_document(&document, &resolver).expect("document should lower");

        let AxNode::Element { children, .. } = node else {
            panic!("expected page root");
        };

        assert_eq!(children.len(), 2);
        assert_eq!(children[0], text("Hello "));
        assert_eq!(
            children[1],
            element_with_attrs("strong", vec![], vec![text("Axonyx")])
        );
    }

    #[test]
    fn lowers_if_block_when_condition_is_truthy() {
        let document = AxDocument::page(
            "Home",
            [AxStatement::if_block(
                AxExpr::bool(true),
                [AxStatement::component(
                    AxComponent::new("Copy").inline("Visible"),
                )],
            )],
        );
        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };

        let node = lower_document(&document, &resolver).expect("document should lower");

        let AxNode::Element { children, .. } = node else {
            panic!("expected page root");
        };

        assert_eq!(children.len(), 1);
    }

    #[test]
    fn lowers_if_block_else_when_condition_is_false() {
        let document = AxDocument::page(
            "Home",
            [AxStatement::If(
                AxIfBlock::new(
                    AxExpr::bool(false),
                    [AxStatement::component(
                        AxComponent::new("Copy").inline("Hidden"),
                    )],
                )
                .else_body([AxStatement::component(
                    AxComponent::new("Copy").inline("Visible"),
                )]),
            )],
        );
        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };

        let node = lower_document(&document, &resolver).expect("document should lower");

        let AxNode::Element { children, .. } = node else {
            panic!("expected page root");
        };

        assert_eq!(children.len(), 1);
    }

    #[test]
    fn lowers_each_empty_body_when_list_has_no_items() {
        let document = AxDocument::page(
            "Home",
            [AxStatement::Each(
                AxEachBlock::new("post", AxExpr::ident("posts"), []).empty([
                    AxStatement::component(AxComponent::new("Copy").inline("No posts")),
                ]),
            )],
        );
        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };
        let mut scope = BTreeMap::new();
        scope.insert("posts".to_string(), AxValue::list([]));

        let node =
            lower_document_with_scope(&document, scope, &resolver).expect("document should lower");

        let AxNode::Element { children, .. } = node else {
            panic!("expected page root");
        };

        assert_eq!(children.len(), 1);
    }

    #[test]
    fn optional_member_lowers_missing_field_to_empty_text() {
        let document = AxDocument::page(
            "Home",
            [AxStatement::component(AxComponent::new("Copy").inline(
                AxExpr::ident("post").optional_member("summary"),
            ))],
        );
        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };
        let mut scope = BTreeMap::new();
        scope.insert(
            "post".to_string(),
            AxValue::record([("title", AxValue::from("Hello"))]),
        );

        let node =
            lower_document_with_scope(&document, scope, &resolver).expect("document should lower");

        let AxNode::Element { children, .. } = node else {
            panic!("expected page root");
        };
        let AxNode::Element { children, .. } = &children[0] else {
            panic!("expected copy element");
        };

        assert_eq!(children, &[text("")]);
    }

    #[test]
    fn member_lowers_missing_record_field_to_empty_text() {
        let document = AxDocument::page(
            "Home",
            [AxStatement::component(
                AxComponent::new("Copy").inline(AxExpr::ident("post").member("summary")),
            )],
        );
        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };
        let mut scope = BTreeMap::new();
        scope.insert(
            "post".to_string(),
            AxValue::record([("title", AxValue::from("Hello"))]),
        );

        let node =
            lower_document_with_scope(&document, scope, &resolver).expect("document should lower");

        let AxNode::Element { children, .. } = node else {
            panic!("expected page root");
        };
        let AxNode::Element { children, .. } = &children[0] else {
            panic!("expected copy element");
        };

        assert_eq!(children, &[text("")]);
    }

    #[test]
    fn lowers_imported_component_with_resolution_metadata() {
        let document = AxDocument {
            imports: vec![AxImport::new(
                [AxImportBinding::new("Card", "SiteCard")],
                "@/ui",
            )],
            functions: Vec::new(),
            components: Vec::new(),
            head: AxHead::default(),
            page: AxPage::new(
                "Home",
                [AxStatement::component(
                    AxComponent::new("SiteCard").prop("title", "Hello"),
                )],
            ),
        };
        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };

        let node = lower_document(&document, &resolver).expect("document should lower");

        let AxNode::Element { children, .. } = node else {
            panic!("expected page root");
        };
        let AxNode::Element { attrs, .. } = &children[0] else {
            panic!("expected imported component placeholder");
        };

        assert!(attrs
            .iter()
            .any(|attr| attr.name == "data-import-source" && attr.value == "@/ui"));
        assert!(attrs
            .iter()
            .any(|attr| attr.name == "data-import-name" && attr.value == "Card"));
        assert!(attrs
            .iter()
            .any(|attr| attr.name == "data-import-local" && attr.value == "SiteCard"));
    }

    #[test]
    fn lowers_local_component_with_props_and_slot() {
        let document = parse_ax_auto(
            r#"
page Home

component FeatureCard(title) {
  <Card title={title}>
    <Slot />
  </Card>
}

<FeatureCard title="Hello">
  <Copy>World</Copy>
</FeatureCard>
"#,
        )
        .expect("document should parse");
        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };

        let node = lower_document(&document, &resolver).expect("document should lower");

        assert_eq!(
            node,
            element_with_attrs(
                "main",
                vec![attr("data-ax-page", "Home"), attr("data-ax-root", "page")],
                vec![element_with_attrs(
                    "article",
                    vec![attr("class", "ax-card")],
                    vec![
                        element_with_attrs(
                            "h2",
                            vec![attr("class", "ax-card__title")],
                            vec![text("Hello")],
                        ),
                        element_with_attrs(
                            "p",
                            vec![attr("class", "ax-copy")],
                            vec![text("World")],
                        ),
                    ],
                )],
            )
        );
    }

    #[test]
    fn lowers_top_level_let_bindings_in_v2_documents() {
        let document = parse_ax_auto(
            r#"
page Home

let heroTitle = "Hello Axonyx"

<Card title={heroTitle}>
  <Copy>{heroTitle}</Copy>
</Card>
"#,
        )
        .expect("document should parse");
        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };

        let node = lower_document(&document, &resolver).expect("document should lower");

        assert_eq!(
            node,
            element_with_attrs(
                "main",
                vec![attr("data-ax-page", "Home"), attr("data-ax-root", "page")],
                vec![element_with_attrs(
                    "article",
                    vec![attr("class", "ax-card")],
                    vec![
                        element_with_attrs(
                            "h2",
                            vec![attr("class", "ax-card__title")],
                            vec![text("Hello Axonyx")],
                        ),
                        element_with_attrs(
                            "p",
                            vec![attr("class", "ax-copy")],
                            vec![text("Hello Axonyx")],
                        ),
                    ],
                )],
            )
        );
    }

    #[test]
    fn lowers_top_level_function_calls_in_v2_documents() {
        let document = parse_ax_auto(
            r#"
page Home

fn heroTitle(title = "Hello Axonyx") = title

<Card title={heroTitle()}>
  <Copy>{heroTitle("Custom title")}</Copy>
</Card>
"#,
        )
        .expect("document should parse");
        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };

        let node = lower_document(&document, &resolver).expect("document should lower");

        assert_eq!(
            node,
            element_with_attrs(
                "main",
                vec![attr("data-ax-page", "Home"), attr("data-ax-root", "page")],
                vec![element_with_attrs(
                    "article",
                    vec![attr("class", "ax-card")],
                    vec![
                        element_with_attrs(
                            "h2",
                            vec![attr("class", "ax-card__title")],
                            vec![text("Hello Axonyx")],
                        ),
                        element_with_attrs(
                            "p",
                            vec![attr("class", "ax-copy")],
                            vec![text("Custom title")],
                        ),
                    ],
                )],
            )
        );
    }

    #[test]
    fn lowers_local_component_default_params_when_props_are_missing() {
        let document = parse_ax_auto(
            r#"
page Home

let defaultTitle = "Default title"

component FeatureCard(title = defaultTitle, tone = "lead") {
  <Card title={title}>
    <Copy tone={tone}>Body</Copy>
  </Card>
}

<FeatureCard />
"#,
        )
        .expect("document should parse");
        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };

        let node = lower_document(&document, &resolver).expect("document should lower");

        assert_eq!(
            node,
            element_with_attrs(
                "main",
                vec![attr("data-ax-page", "Home"), attr("data-ax-root", "page")],
                vec![element_with_attrs(
                    "article",
                    vec![attr("class", "ax-card")],
                    vec![
                        element_with_attrs(
                            "h2",
                            vec![attr("class", "ax-card__title")],
                            vec![text("Default title")],
                        ),
                        element_with_attrs(
                            "p",
                            vec![attr("class", "ax-copy"), attr("data-tone", "lead")],
                            vec![text("Body")],
                        ),
                    ],
                )],
            )
        );
    }

    #[test]
    fn lowers_imported_component_from_resolved_source_with_slot_and_nested_imports() {
        let document = AxDocument {
            imports: vec![AxImport::new(
                [AxImportBinding::named("SiteCard")],
                "@/components/site-card.ax",
            )],
            functions: Vec::new(),
            components: Vec::new(),
            head: AxHead::default(),
            page: AxPage::new(
                "Home",
                [AxStatement::component(
                    AxComponent::new("SiteCard")
                        .prop("title", AxExpr::ident("heading"))
                        .block([AxStatement::component(
                            AxComponent::new("Copy").inline("Body content"),
                        )]),
                )],
            ),
        };
        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };
        let import_resolver = |source: &str| -> Option<String> {
            match source {
                "@/components/site-card.ax" => Some(
                    r#"
import { Frame } from "@/components/frame.ax"

page SiteCard
<Frame>
  <Card title={title}>
    <Slot />
  </Card>
</Frame>
"#
                    .to_string(),
                ),
                "@/components/frame.ax" => Some(
                    r#"
page Frame
<section class="frame">
  <Slot />
</section>
"#
                    .to_string(),
                ),
                _ => None,
            }
        };
        let mut scope = BTreeMap::new();
        scope.insert("heading".to_string(), AxValue::from("Imported title"));

        let node =
            lower_document_with_scope_and_imports(&document, scope, &resolver, &import_resolver)
                .expect("document should lower");

        assert_eq!(
            node,
            element_with_attrs(
                "main",
                vec![attr("data-ax-page", "Home"), attr("data-ax-root", "page"),],
                vec![element_with_attrs(
                    "section",
                    vec![attr("class", "frame")],
                    vec![element_with_attrs(
                        "article",
                        vec![attr("class", "ax-card")],
                        vec![
                            element_with_attrs(
                                "h2",
                                vec![attr("class", "ax-card__title")],
                                vec![text("Imported title")],
                            ),
                            element_with_attrs(
                                "p",
                                vec![attr("class", "ax-copy")],
                                vec![text("Body content")],
                            ),
                        ],
                    )],
                )],
            )
        );
    }

    #[test]
    fn lowers_imported_component_with_named_slots() {
        let document = AxDocument {
            imports: vec![AxImport::new(
                [AxImportBinding::named("ShellCard")],
                "@/components/shell-card.ax",
            )],
            functions: Vec::new(),
            components: Vec::new(),
            head: AxHead::default(),
            page: AxPage::new(
                "Home",
                [AxStatement::component(
                    AxComponent::new("ShellCard").block([
                        AxStatement::component(
                            AxComponent::new("Copy")
                                .prop("slot", "eyebrow")
                                .inline("Framework"),
                        ),
                        AxStatement::component(AxComponent::new("Copy").inline("Default body")),
                    ]),
                )],
            ),
        };
        let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };
        let import_resolver = |source: &str| -> Option<String> {
            match source {
                "@/components/shell-card.ax" => Some(
                    r#"
page ShellCard
<Card title="Slot demo">
  <Copy tone="lead">
    <Slot name="eyebrow" />
  </Copy>
  <Slot />
</Card>
"#
                    .to_string(),
                ),
                _ => None,
            }
        };

        let node = lower_document_with_scope_and_imports(
            &document,
            BTreeMap::new(),
            &resolver,
            &import_resolver,
        )
        .expect("document should lower");

        assert_eq!(
            node,
            element_with_attrs(
                "main",
                vec![attr("data-ax-page", "Home"), attr("data-ax-root", "page")],
                vec![element_with_attrs(
                    "article",
                    vec![attr("class", "ax-card")],
                    vec![
                        element_with_attrs(
                            "h2",
                            vec![attr("class", "ax-card__title")],
                            vec![text("Slot demo")],
                        ),
                        element_with_attrs(
                            "p",
                            vec![attr("class", "ax-copy"), attr("data-tone", "lead")],
                            vec![element_with_attrs(
                                "p",
                                vec![attr("class", "ax-copy")],
                                vec![text("Framework")],
                            )],
                        ),
                        element_with_attrs(
                            "p",
                            vec![attr("class", "ax-copy")],
                            vec![text("Default body")],
                        ),
                    ],
                )],
            )
        );
    }
}
