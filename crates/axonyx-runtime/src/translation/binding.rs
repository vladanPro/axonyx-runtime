use std::collections::{BTreeMap, BTreeSet};

use axonyx_core::ax_ast::{
    AxBody, AxComponent, AxComponentParamDef, AxDocument, AxExpr, AxPipelineStage, AxStatement,
};

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct AxTranslationBindingError {
    pub pattern: String,
    message: String,
}

type Result<T> = std::result::Result<T, AxTranslationBindingError>;

fn error(pattern: impl Into<String>, message: impl Into<String>) -> AxTranslationBindingError {
    AxTranslationBindingError {
        pattern: pattern.into(),
        message: message.into(),
    }
}

pub fn bound_translator_initializer(expr: &AxExpr) -> bool {
    matches!(expr, AxExpr::Call { path, .. } if path == &["I18n", "withLocale"])
}

/// Lower one source document before layout composition. The caller assigns
/// distinct source indices to the page and layouts that share a render scope.
pub fn lower_bound_translators(document: &mut AxDocument, source_index: usize) -> Result<()> {
    let mut scope = Bindings::default();
    for statement in &document.page.body {
        let AxStatement::Data(binding) = statement else {
            continue;
        };
        if bound_translator_initializer(&binding.value) {
            let name = &binding.name;
            // A NUL cannot occur in a source identifier. It keeps the lowered
            // binding distinct from user variables and aliases in other files.
            let internal = format!("\0i18n:{source_index}:{name}");
            if matches!(name.as_str(), "t" | "I18n")
                || scope.names.insert(name.clone(), internal).is_some()
            {
                return Err(error(
                    name,
                    format!("invalid or duplicate translator name `{name}`"),
                ));
            }
        }
    }
    for binding in document.imports.iter().flat_map(|item| &item.bindings) {
        scope.reject_shadow(&binding.local)?;
    }
    scope.defaults(&mut document.page.params)?;
    for function in &mut document.functions {
        scope.reject_shadow(&function.name)?;
        scope.defaults(&mut function.params)?;
        scope.expr(&mut function.body)?;
    }
    for component in &mut document.components {
        scope.defaults(&mut component.params)?;
        for state in &mut component.states {
            scope.expr(&mut state.initial)?;
        }
        scope.statements(&mut component.body)?;
    }
    let head = &mut document.head;
    for expr in [
        &mut head.title,
        &mut head.theme,
        &mut head.theme_storage_key,
    ]
    .into_iter()
    .flatten()
    .chain(
        head.metas
            .iter_mut()
            .chain(&mut head.links)
            .chain(&mut head.scripts)
            .flat_map(|tag| tag.attrs.iter_mut().map(|attr| &mut attr.value)),
    ) {
        scope.expr(expr)?;
    }
    for statement in &mut document.page.body {
        if let AxStatement::Data(binding) = statement {
            if let AxExpr::Call { path, args } = &mut binding.value {
                if path == &["I18n", "withLocale"] {
                    let [locale] = args.as_mut_slice() else {
                        return Err(error(
                            "I18n.withLocale",
                            "`I18n.withLocale()` expects one locale string",
                        ));
                    };
                    if matches!(
                        locale,
                        AxExpr::Bool(_)
                            | AxExpr::Number(_)
                            | AxExpr::Float(_)
                            | AxExpr::List(_)
                            | AxExpr::Object(_)
                    ) {
                        return Err(error(
                            "I18n.withLocale",
                            "`I18n.withLocale()` expects a locale string",
                        ));
                    }
                    scope.expr(locale)?;
                    let internal = scope.names[&binding.name].clone();
                    let name = std::mem::replace(&mut binding.name, internal);
                    binding.value = args.pop().expect("one locale argument");
                    scope.active.insert(name);
                    continue;
                }
            }
        }
        scope.statement(statement)?;
    }
    Ok(())
}

#[derive(Default)]
struct Bindings {
    names: BTreeMap<String, String>,
    active: BTreeSet<String>,
}

impl Bindings {
    fn reject_shadow(&self, name: &str) -> Result<()> {
        if self.names.contains_key(name) {
            return Err(error(
                name,
                format!("`{name}` conflicts with a local translator binding"),
            ));
        }
        Ok(())
    }

    fn defaults(&self, params: &mut [AxComponentParamDef]) -> Result<()> {
        for param in params {
            self.reject_shadow(&param.name)?;
            if let Some(default) = &mut param.default {
                self.expr(default)?;
            }
        }
        Ok(())
    }

    fn statements(&self, statements: &mut [AxStatement]) -> Result<()> {
        for statement in statements {
            self.statement(statement)?;
        }
        Ok(())
    }

    fn statement(&self, statement: &mut AxStatement) -> Result<()> {
        match statement {
            AxStatement::Data(binding) => {
                self.reject_shadow(&binding.name)?;
                self.expr(&mut binding.value)
            }
            AxStatement::Text(expr) => self.expr(expr),
            AxStatement::Each(block) => {
                self.reject_shadow(&block.binding)?;
                self.expr(&mut block.source)?;
                if let Some(key) = &mut block.key {
                    self.expr(key)?;
                }
                self.statements(&mut block.body)?;
                self.statements(&mut block.empty_body)
            }
            AxStatement::If(block) => {
                self.expr(&mut block.condition)?;
                self.statements(&mut block.body)?;
                self.statements(&mut block.else_body)
            }
            AxStatement::Match(block) => {
                self.expr(&mut block.value)?;
                for case in &mut block.cases {
                    self.statements(&mut case.body)?;
                }
                if let Some(body) = &mut block.default_body {
                    self.statements(body)?;
                }
                Ok(())
            }
            AxStatement::Component(component) => self.component(component),
            AxStatement::Pipeline(pipeline) => {
                self.expr(&mut pipeline.source)?;
                for stage in &mut pipeline.stages {
                    match stage {
                        AxPipelineStage::Component(component) => self.component(component)?,
                        AxPipelineStage::Each(each) => self.reject_shadow(&each.binding)?,
                    }
                }
                Ok(())
            }
        }
    }

    fn component(&self, component: &mut AxComponent) -> Result<()> {
        for expr in component
            .props
            .iter_mut()
            .map(|prop| &mut prop.value)
            .chain(
                [&mut component.style.recipe, &mut component.style.class]
                    .into_iter()
                    .flatten(),
            )
        {
            self.expr(expr)?;
        }
        match &mut component.body {
            AxBody::Empty => Ok(()),
            AxBody::Inline(expr) => self.expr(expr),
            AxBody::Block(statements) => self.statements(statements),
        }
    }

    fn expr(&self, expr: &mut AxExpr) -> Result<()> {
        match expr {
            AxExpr::Call { path, args } => {
                if path == &["I18n", "withLocale"] {
                    return Err(error(
                        "I18n.withLocale",
                        "bind `I18n.withLocale()` to a local name in the page or layout body",
                    ));
                }
                if let Some((name, internal)) =
                    path.first().and_then(|name| self.names.get_key_value(name))
                {
                    if path.len() != 1 || !self.active.contains(name) {
                        return Err(error(name, format!("call local translator `{name}` directly after its declaration in the page or layout body")));
                    }
                    if !matches!(args.as_slice(), [AxExpr::String(_)]) {
                        return Err(error(
                            name,
                            format!("translator `{name}` expects one literal string key"),
                        ));
                    }
                    *path = vec!["t".into()];
                    args.push(AxExpr::Identifier(internal.clone()));
                }
                for arg in args {
                    self.expr(arg)?;
                }
            }
            AxExpr::Identifier(name) if self.names.contains_key(name) => {
                return Err(error(
                    &*name,
                    format!("translator `{name}` cannot be used as a value"),
                ));
            }
            AxExpr::List(items) => {
                for item in items {
                    self.expr(item)?;
                }
            }
            AxExpr::Object(fields) => {
                for value in fields.values_mut() {
                    self.expr(value)?;
                }
            }
            AxExpr::Binary { left, right, .. }
            | AxExpr::Index {
                object: left,
                index: right,
            } => {
                self.expr(left)?;
                self.expr(right)?;
            }
            AxExpr::Unary { expr, .. }
            | AxExpr::Member { object: expr, .. }
            | AxExpr::OptionalMember { object: expr, .. } => self.expr(expr)?,
            _ => {}
        }
        Ok(())
    }
}
