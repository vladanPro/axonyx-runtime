use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxFileV2 {
    pub package_uses: Vec<String>,
    pub imports: Vec<AxImportDecl>,
    pub page: AxPageDecl,
    pub types: Vec<AxTypeDeclV2>,
    pub lets: Vec<AxLetDeclV2>,
    pub states: Vec<AxStateDeclV2>,
    pub functions: Vec<AxFunctionDeclV2>,
    pub components: Vec<AxComponentDeclV2>,
    pub body: Vec<AxNodeV2>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxImportDecl {
    pub bindings: Vec<AxImportBinding>,
    pub source: String,
}

impl AxImportDecl {
    pub fn new(
        bindings: impl IntoIterator<Item = AxImportBinding>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            bindings: bindings.into_iter().collect(),
            source: source.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxImportBinding {
    pub imported: String,
    pub local: String,
}

impl AxImportBinding {
    pub fn new(imported: impl Into<String>, local: impl Into<String>) -> Self {
        Self {
            imported: imported.into(),
            local: local.into(),
        }
    }

    pub fn namespace(local: impl Into<String>) -> Self {
        Self {
            imported: "*".to_string(),
            local: local.into(),
        }
    }

    pub fn named(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            imported: name.clone(),
            local: name,
        }
    }

    pub fn is_namespace(&self) -> bool {
        self.imported == "*"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxPageDecl {
    pub name: String,
    pub params: Vec<AxComponentParamDeclV2>,
}

impl AxPageDecl {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            params: Vec::new(),
        }
    }

    pub fn with_params(
        name: impl Into<String>,
        params: impl IntoIterator<Item = AxComponentParamDeclV2>,
    ) -> Self {
        Self {
            name: name.into(),
            params: params.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxTypeDeclV2 {
    pub name: String,
    pub fields: Vec<AxTypeFieldDeclV2>,
}

impl AxTypeDeclV2 {
    pub fn new(
        name: impl Into<String>,
        fields: impl IntoIterator<Item = AxTypeFieldDeclV2>,
    ) -> Self {
        Self {
            name: name.into(),
            fields: fields.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxTypeFieldDeclV2 {
    pub name: String,
    pub ty: String,
}

impl AxTypeFieldDeclV2 {
    pub fn new(name: impl Into<String>, ty: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ty: ty.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxLetDeclV2 {
    pub name: String,
    pub ty: Option<String>,
    pub value: String,
}

impl AxLetDeclV2 {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ty: None,
            value: value.into(),
        }
    }

    pub fn typed(name: impl Into<String>, ty: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ty: Some(ty.into()),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxStateDeclV2 {
    pub scope: Option<String>,
    pub name: String,
    pub ty: Option<String>,
    pub value: String,
}

impl AxStateDeclV2 {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            scope: None,
            name: name.into(),
            ty: None,
            value: value.into(),
        }
    }

    pub fn typed(name: impl Into<String>, ty: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            scope: None,
            name: name.into(),
            ty: Some(ty.into()),
            value: value.into(),
        }
    }

    pub fn scoped(
        scope: impl Into<String>,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self {
            scope: Some(scope.into()),
            name: name.into(),
            ty: None,
            value: value.into(),
        }
    }

    pub fn typed_scoped(
        scope: impl Into<String>,
        name: impl Into<String>,
        ty: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self {
            scope: Some(scope.into()),
            name: name.into(),
            ty: Some(ty.into()),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxFunctionDeclV2 {
    pub name: String,
    pub params: Vec<AxComponentParamDeclV2>,
    pub body: String,
}

impl AxFunctionDeclV2 {
    pub fn new(
        name: impl Into<String>,
        params: impl IntoIterator<Item = AxComponentParamDeclV2>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            params: params.into_iter().collect(),
            body: body.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxComponentDeclV2 {
    pub name: String,
    pub params: Vec<AxComponentParamDeclV2>,
    pub body: Vec<AxNodeV2>,
}

impl AxComponentDeclV2 {
    pub fn new(
        name: impl Into<String>,
        params: impl IntoIterator<Item = AxComponentParamDeclV2>,
        body: impl IntoIterator<Item = AxNodeV2>,
    ) -> Self {
        Self {
            name: name.into(),
            params: params.into_iter().collect(),
            body: body.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxComponentParamDeclV2 {
    pub name: String,
    pub default: Option<String>,
}

impl AxComponentParamDeclV2 {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            default: None,
        }
    }

    pub fn with_default(name: impl Into<String>, default: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            default: Some(default.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxNodeV2 {
    Element(AxElementNode),
    Text(AxTextNode),
    Expr(AxExprNode),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxElementNode {
    pub name: String,
    pub attrs: Vec<AxAttributeNode>,
    pub children: Vec<AxNodeV2>,
    pub self_closing: bool,
}

impl AxElementNode {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            attrs: Vec::new(),
            children: Vec::new(),
            self_closing: false,
        }
    }

    pub fn attr(mut self, attr: AxAttributeNode) -> Self {
        self.attrs.push(attr);
        self
    }

    pub fn child(mut self, child: AxNodeV2) -> Self {
        self.children.push(child);
        self
    }

    pub fn self_closing(mut self, value: bool) -> Self {
        self.self_closing = value;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxAttributeNode {
    pub name: String,
    pub value: AxAttributeValue,
}

impl AxAttributeNode {
    pub fn string(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: AxAttributeValue::String(value.into()),
        }
    }

    pub fn expr(name: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: AxAttributeValue::Expr(source.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxAttributeValue {
    String(String),
    Expr(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxTextNode {
    pub value: String,
}

impl AxTextNode {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxExprNode {
    pub source: String,
}

impl AxExprNode {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
        }
    }
}

pub mod prelude {
    pub use super::AxAttributeNode;
    pub use super::AxAttributeValue;
    pub use super::AxComponentDeclV2;
    pub use super::AxComponentParamDeclV2;
    pub use super::AxElementNode;
    pub use super::AxExprNode;
    pub use super::AxFileV2;
    pub use super::AxFunctionDeclV2;
    pub use super::AxImportBinding;
    pub use super::AxImportDecl;
    pub use super::AxLetDeclV2;
    pub use super::AxNodeV2;
    pub use super::AxPageDecl;
    pub use super::AxStateDeclV2;
    pub use super::AxTextNode;
    pub use super::AxTypeDeclV2;
    pub use super::AxTypeFieldDeclV2;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_v2_ast_for_imports_and_elements() {
        let file = AxFileV2 {
            package_uses: Vec::new(),
            imports: vec![AxImportDecl::new(
                [
                    AxImportBinding::named("Card"),
                    AxImportBinding::named("Copy"),
                ],
                "@axonyx/ui",
            )],
            page: AxPageDecl::new("Home"),
            types: Vec::new(),
            lets: Vec::new(),
            states: Vec::new(),
            functions: Vec::new(),
            components: Vec::new(),
            body: vec![AxNodeV2::Element(
                AxElementNode::new("Card")
                    .attr(AxAttributeNode::string("title", "Hello"))
                    .child(AxNodeV2::Text(AxTextNode::new("World"))),
            )],
        };

        assert_eq!(file.page.name, "Home");
        assert_eq!(
            file.imports[0].bindings,
            vec![
                AxImportBinding::named("Card"),
                AxImportBinding::named("Copy")
            ]
        );
    }
}
