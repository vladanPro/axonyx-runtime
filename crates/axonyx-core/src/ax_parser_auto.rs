use thiserror::Error;

use crate::ax_ast::prelude::*;
use crate::ax_ast_v2::prelude::*;
use crate::ax_parser::{parse_ax, parse_expr, AxParseError};
use crate::ax_parser_v2::{parse_ax_v2, AxParseV2Error};

#[derive(Debug, Error)]
pub enum AxAutoParseError {
    #[error("failed to parse indentation-first .ax file")]
    V1(#[from] AxParseError),
    #[error("failed to parse JSX-like .ax file")]
    V2(#[from] AxParseV2Error),
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
}

pub fn looks_like_ax_v2(input: &str) -> bool {
    input.lines().map(str::trim).any(|line| {
        !line.is_empty()
            && (line.starts_with("import ") || line.starts_with('<') || line.starts_with("</"))
    })
}

pub fn parse_ax_auto(input: &str) -> Result<AxDocument, AxAutoParseError> {
    if looks_like_ax_v2(input) {
        let file = parse_ax_v2(input)?;
        Ok(convert_ax_v2_file(&file)?)
    } else {
        Ok(parse_ax(input)?)
    }
}

pub fn convert_ax_v2_file(file: &AxFileV2) -> Result<AxDocument, AxConvertV2Error> {
    let mut head = AxHead::default();
    let mut body = Vec::new();

    for node in &file.body {
        match node {
            AxNodeV2::Element(element) if element.name == "Head" => {
                merge_head_element(&mut head, element)?
            }
            AxNodeV2::Element(element) => {
                body.push(AxStatement::component(convert_element(element)?));
            }
            AxNodeV2::Text(text) => body.push(AxStatement::text(text.value.clone())),
            AxNodeV2::Expr(expr) => body.push(AxStatement::text(parse_v2_expr(&expr.source)?)),
        }
    }

    Ok(AxDocument {
        imports: file.imports.iter().map(convert_import_decl).collect(),
        head,
        page: AxPage::new(file.page.name.clone(), body),
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

fn merge_head_element(head: &mut AxHead, element: &AxElementNode) -> Result<(), AxConvertV2Error> {
    for child in &element.children {
        let AxNodeV2::Element(tag) = child else {
            return Err(AxConvertV2Error::InvalidHeadChild);
        };

        match tag.name.as_str() {
            "Title" => head.title = Some(convert_head_value(tag)?),
            "Theme" => head.theme = Some(convert_head_value(tag)?),
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

fn convert_element(element: &AxElementNode) -> Result<AxComponent, AxConvertV2Error> {
    if element.name == "Each" {
        return convert_each_element(element);
    }
    if element.name == "If" {
        return convert_if_element(element);
    }

    let mut component = AxComponent::new(element.name.clone());

    for attr in &element.attrs {
        let value = convert_attr_value(&attr.value)?;
        match attr.name.as_str() {
            "class" => component = component.class(value),
            "recipe" => component = component.recipe(value),
            _ => component = component.prop(attr.name.clone(), value),
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
            .map(convert_child)
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(component.block(body));
    }

    if element.children.len() == 1 {
        return match &element.children[0] {
            AxNodeV2::Text(text) => Ok(component.inline(AxExpr::string(text.value.clone()))),
            AxNodeV2::Expr(expr) => Ok(component.inline(parse_v2_expr(&expr.source)?)),
            AxNodeV2::Element(child) => {
                Ok(component.block([AxStatement::component(convert_element(child)?)]))
            }
        };
    }

    Ok(component.block(convert_children(&element.children)?))
}

fn convert_children(children: &[AxNodeV2]) -> Result<Vec<AxStatement>, AxConvertV2Error> {
    children.iter().map(convert_child).collect()
}

fn convert_child(child: &AxNodeV2) -> Result<AxStatement, AxConvertV2Error> {
    match child {
        AxNodeV2::Element(element) if element.name == "Each" => convert_each_statement(element),
        AxNodeV2::Element(element) if element.name == "If" => convert_if_statement(element),
        AxNodeV2::Element(element) if element.name == "Else" || element.name == "Empty" => {
            Err(AxConvertV2Error::UnexpectedControlBranch {
                tag: "control-flow".to_string(),
                branch: element.name.clone(),
            })
        }
        AxNodeV2::Element(element) => Ok(AxStatement::component(convert_element(element)?)),
        AxNodeV2::Text(text) => Ok(AxStatement::text(text.value.clone())),
        AxNodeV2::Expr(expr) => Ok(AxStatement::text(parse_v2_expr(&expr.source)?)),
    }
}

fn convert_each_statement(element: &AxElementNode) -> Result<AxStatement, AxConvertV2Error> {
    let binding = control_binding_attr(element, &["as", "item"])?;
    let source = control_expr_attr(element, &["items", "in", "of"])?;
    let (body, empty_body) = split_each_children(element)?;
    Ok(AxStatement::Each(
        AxEachBlock::new(binding, source, body).empty(empty_body),
    ))
}

fn convert_if_statement(element: &AxElementNode) -> Result<AxStatement, AxConvertV2Error> {
    let condition = control_expr_attr(element, &["when"])?;
    let (body, else_body) = split_if_children(element)?;
    Ok(AxStatement::If(
        AxIfBlock::new(condition, body).else_body(else_body),
    ))
}

fn convert_each_element(element: &AxElementNode) -> Result<AxComponent, AxConvertV2Error> {
    Ok(AxComponent::fragment([convert_each_statement(element)?]))
}

fn convert_if_element(element: &AxElementNode) -> Result<AxComponent, AxConvertV2Error> {
    Ok(AxComponent::fragment([convert_if_statement(element)?]))
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
                empty_body = Some(convert_children(&branch.children)?);
            }
            _ => body.push(convert_child(child)?),
        }
    }

    Ok((body, empty_body.unwrap_or_default()))
}

fn split_if_children(
    element: &AxElementNode,
) -> Result<(Vec<AxStatement>, Vec<AxStatement>), AxConvertV2Error> {
    let mut body = Vec::new();
    let mut else_body = None;

    for child in &element.children {
        match child {
            AxNodeV2::Element(branch) if branch.name == "Else" => {
                if !branch.attrs.is_empty() {
                    return Err(AxConvertV2Error::ControlBranchAttrsNotSupported {
                        tag: element.name.clone(),
                        branch: "Else".to_string(),
                    });
                }
                if else_body.is_some() {
                    return Err(AxConvertV2Error::DuplicateControlBranch {
                        tag: element.name.clone(),
                        branch: "Else".to_string(),
                    });
                }
                else_body = Some(convert_children(&branch.children)?);
            }
            _ => body.push(convert_child(child)?),
        }
    }

    Ok((body, else_body.unwrap_or_default()))
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
