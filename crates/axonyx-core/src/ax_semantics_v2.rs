use std::collections::BTreeSet;

use thiserror::Error;

use crate::ax_ast_v2::prelude::*;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AxSemanticV2Error {
    #[error("import local name `{name}` is reserved by Axonyx built-ins; import it with an alias from `{import_source}`")]
    ReservedImportName { name: String, import_source: String },
    #[error("component name `{name}` is reserved by Axonyx built-ins")]
    ReservedComponentName { name: String },
    #[error("duplicate component declaration `{name}`")]
    DuplicateComponentName { name: String },
    #[error("component name `{name}` conflicts with an imported local name")]
    ComponentNameConflictsWithImport { name: String },
    #[error("`<{tag}>` is only valid inside `<Head>`")]
    HeadTagOutsideHead { tag: String },
    #[error("`<Head>` is only valid at the top level of a page")]
    HeadOutsideTopLevel,
    #[error("component `{component}` must load WASM from a file; inline `client WASM` is not supported in v1")]
    InlineComponentWasm { component: String },
}

pub fn validate_ax_v2_semantics(file: &AxFileV2) -> Result<(), AxSemanticV2Error> {
    validate_import_names(file)?;
    validate_component_names(file)?;

    for component in &file.components {
        for client in &component.clients {
            if matches!(client.target, AxComponentClientTargetV2::Wasm)
                && matches!(client.source, AxComponentClientSourceV2::Inline(_))
            {
                return Err(AxSemanticV2Error::InlineComponentWasm {
                    component: component.name.clone(),
                });
            }
        }
        for node in &component.body {
            validate_node(node, NodeContext::Body)?;
        }
    }

    for node in &file.body {
        validate_node(node, NodeContext::TopLevel)?;
    }

    Ok(())
}

fn validate_import_names(file: &AxFileV2) -> Result<(), AxSemanticV2Error> {
    for import in &file.imports {
        for binding in &import.bindings {
            if is_reserved_import_name(&binding.local) {
                return Err(AxSemanticV2Error::ReservedImportName {
                    name: binding.local.clone(),
                    import_source: import.source.clone(),
                });
            }
        }
    }

    Ok(())
}

fn validate_component_names(file: &AxFileV2) -> Result<(), AxSemanticV2Error> {
    let mut import_names = BTreeSet::new();
    for import in &file.imports {
        for binding in &import.bindings {
            import_names.insert(binding.local.as_str());
        }
    }

    let mut component_names = BTreeSet::new();
    for component in &file.components {
        if is_reserved_import_name(&component.name) {
            return Err(AxSemanticV2Error::ReservedComponentName {
                name: component.name.clone(),
            });
        }

        if import_names.contains(component.name.as_str()) {
            return Err(AxSemanticV2Error::ComponentNameConflictsWithImport {
                name: component.name.clone(),
            });
        }

        if !component_names.insert(component.name.as_str()) {
            return Err(AxSemanticV2Error::DuplicateComponentName {
                name: component.name.clone(),
            });
        }
    }

    Ok(())
}

fn validate_node(node: &AxNodeV2, context: NodeContext) -> Result<(), AxSemanticV2Error> {
    let AxNodeV2::Element(element) = node else {
        return Ok(());
    };

    validate_element(element, context)
}

fn validate_element(
    element: &AxElementNode,
    context: NodeContext,
) -> Result<(), AxSemanticV2Error> {
    if element.name == "Head" {
        if context != NodeContext::TopLevel {
            return Err(AxSemanticV2Error::HeadOutsideTopLevel);
        }

        for child in &element.children {
            validate_node(child, NodeContext::Head)?;
        }
        return Ok(());
    }

    if is_head_only_tag(&element.name) && context != NodeContext::Head {
        return Err(AxSemanticV2Error::HeadTagOutsideHead {
            tag: element.name.clone(),
        });
    }

    let child_context = if context == NodeContext::Head {
        NodeContext::Head
    } else {
        NodeContext::Body
    };

    for child in &element.children {
        validate_node(child, child_context)?;
    }

    Ok(())
}

fn is_reserved_import_name(name: &str) -> bool {
    matches!(
        name,
        "Head"
            | "Title"
            | "Theme"
            | "Meta"
            | "Link"
            | "Script"
            | "Each"
            | "If"
            | "ElseIf"
            | "Else"
            | "Empty"
            | "Slot"
    )
}

fn is_head_only_tag(name: &str) -> bool {
    matches!(name, "Title" | "Theme" | "Meta" | "Link" | "Script")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeContext {
    TopLevel,
    Body,
    Head,
}

pub mod prelude {
    pub use super::validate_ax_v2_semantics;
    pub use super::AxSemanticV2Error;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ax_parser_v2::parse_ax_v2;

    #[test]
    fn rejects_imports_that_shadow_reserved_tags() {
        let file = parse_ax_v2(
            r#"
import { Link } from "@axonyx/ui/foundry/Link.ax"

page Home
<Link href="/">Home</Link>
"#,
        )
        .expect("source should parse");

        let error = validate_ax_v2_semantics(&file).expect_err("reserved import should fail");

        assert_eq!(
            error,
            AxSemanticV2Error::ReservedImportName {
                name: "Link".to_string(),
                import_source: "@axonyx/ui/foundry/Link.ax".to_string()
            }
        );
    }

    #[test]
    fn allows_reserved_imports_when_aliased_to_safe_names() {
        let file = parse_ax_v2(
            r#"
import { Link as TextLink } from "@axonyx/ui/foundry/Link.ax"

page Home
<TextLink href="/">Home</TextLink>
"#,
        )
        .expect("source should parse");

        validate_ax_v2_semantics(&file).expect("aliased import should be valid");
    }

    #[test]
    fn rejects_head_tags_outside_head() {
        let file = parse_ax_v2(
            r#"
page Home
<Link rel="stylesheet" href="/app.css" />
"#,
        )
        .expect("source should parse");

        let error = validate_ax_v2_semantics(&file).expect_err("head tag should fail in body");

        assert_eq!(
            error,
            AxSemanticV2Error::HeadTagOutsideHead {
                tag: "Link".to_string()
            }
        );
    }

    #[test]
    fn rejects_local_components_that_shadow_reserved_tags() {
        let file = parse_ax_v2(
            r#"
page Home

component Slot() {
  <Copy>Body</Copy>
}
"#,
        )
        .expect("source should parse");

        let error = validate_ax_v2_semantics(&file).expect_err("component name should fail");

        assert_eq!(
            error,
            AxSemanticV2Error::ReservedComponentName {
                name: "Slot".to_string()
            }
        );
    }

    #[test]
    fn rejects_duplicate_local_components() {
        let file = parse_ax_v2(
            r#"
page Home

component Feature() {
  <Copy>One</Copy>
}

component Feature() {
  <Copy>Two</Copy>
}
"#,
        )
        .expect("source should parse");

        let error = validate_ax_v2_semantics(&file).expect_err("duplicate name should fail");

        assert_eq!(
            error,
            AxSemanticV2Error::DuplicateComponentName {
                name: "Feature".to_string()
            }
        );
    }

    #[test]
    fn rejects_inline_component_wasm_clients() {
        let file = parse_ax_v2(
            r#"
page Home

component Demo() {
  client WASM {
    export function boot() {}
  }

  render ASX {
    <Copy>Demo</Copy>
  }
}
"#,
        )
        .expect("source should parse");

        let error = validate_ax_v2_semantics(&file).expect_err("inline wasm should fail");

        assert_eq!(
            error,
            AxSemanticV2Error::InlineComponentWasm {
                component: "Demo".to_string()
            }
        );
    }

    #[test]
    fn allows_component_wasm_loaded_from_file() {
        let file = parse_ax_v2(
            r#"
page Home

component Demo() {
  client WASM from "./demo.wasm"

  render ASX {
    <Copy>Demo</Copy>
  }
}
"#,
        )
        .expect("source should parse");

        validate_ax_v2_semantics(&file).expect("file wasm client should be valid");
    }
}
