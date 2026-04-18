pub mod backend;

use axonyx_core::ax_ast_prelude::{
    AxBody, AxComponent, AxDocument, AxExpr, AxHead, AxHeadTag, AxPipeline, AxPipelineStage,
    AxProp, AxStatement,
};
use axonyx_core::ax_lowering::AxLowerError;
use axonyx_core::ax_lowering_prelude::{lower_document, AxValue};
use axonyx_core::ax_parser::AxParseError;
use axonyx_core::ax_parser_prelude::parse_ax;
use axonyx_core::prelude::{Attribute, AxNode};
use axonyx_core::{AxonyxIr, SourceKind, TransformKind, ViewKind};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

pub use backend::prelude as backend_prelude;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RenderPlan {
    pub source: String,
    pub layout: LayoutPlan,
    pub view: ViewPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LayoutPlan {
    pub kind: String,
    pub columns: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ViewPlan {
    pub component: String,
    pub props: serde_json::Value,
}

pub fn execute(ir: &AxonyxIr) -> RenderPlan {
    let source = match &ir.source.kind {
        SourceKind::Collection { name } => name.clone(),
    };

    let mut columns = 1;
    for transform in &ir.transforms {
        match transform.kind {
            TransformKind::Grid { columns: c } => columns = c,
        }
    }

    let component = match &ir.view.kind {
        ViewKind::Card => "Card".to_string(),
        ViewKind::Named { name } => name.clone(),
    };

    RenderPlan {
        source,
        layout: LayoutPlan {
            kind: "grid".to_string(),
            columns,
        },
        view: ViewPlan {
            component,
            props: json!({
                "runtime": "axonyx-runtime-v1",
            }),
        },
    }
}

pub fn execute_json(ir_json: &str) -> Result<RenderPlan, serde_json::Error> {
    let ir: AxonyxIr = serde_json::from_str(ir_json)?;
    Ok(execute(&ir))
}

#[derive(Debug, Error)]
pub enum PreviewError {
    #[error("failed to parse .ax file")]
    Parse(#[from] AxParseError),
    #[error("failed to lower .ax file")]
    Lower(#[from] AxLowerError),
}

pub fn preview_ax_page(ax_source: &str) -> Result<String, PreviewError> {
    preview_ax_app(None, ax_source)
}

pub fn preview_ax_app(
    layout_source: Option<&str>,
    page_source: &str,
) -> Result<String, PreviewError> {
    let layout_sources = layout_source.into_iter().collect::<Vec<_>>();
    preview_ax_route(&layout_sources, page_source)
}

pub fn preview_ax_route(
    layout_sources: &[&str],
    page_source: &str,
) -> Result<String, PreviewError> {
    let page_document = parse_ax(page_source)?;
    let mut document = page_document;

    for layout_source in layout_sources.iter().rev() {
        let layout_document = parse_ax(layout_source)?;
        document = compose_layout_with_page(layout_document, document);
    }

    let resolver = |_: &[String], _: &[AxValue]| -> Option<AxValue> { None };
    let node = lower_document(&document, &resolver)?;
    Ok(render_preview_document(&document, &node))
}

fn compose_layout_with_page(mut layout: AxDocument, page: AxDocument) -> AxDocument {
    let page_name = page.page.name;
    let page_head = page.head;
    let page_body = page.page.body;

    if !inject_slot_statements(&mut layout.page.body, &page_body) {
        layout.page.body.extend(page_body);
    }

    layout.page.name = page_name;
    layout.head.merge(page_head);
    layout
}

fn inject_slot_statements(statements: &mut Vec<AxStatement>, page_body: &[AxStatement]) -> bool {
    let mut found_slot = false;
    let mut composed = Vec::with_capacity(statements.len() + page_body.len());

    for statement in statements.drain(..) {
        match statement {
            AxStatement::Component(component) if is_slot_component(&component) => {
                composed.extend(page_body.iter().cloned());
                found_slot = true;
            }
            AxStatement::Component(mut component) => {
                if let AxBody::Block(body) = &mut component.body {
                    found_slot |= inject_slot_statements(body, page_body);
                }
                composed.push(AxStatement::Component(component));
            }
            AxStatement::Each(mut each) => {
                found_slot |= inject_slot_statements(&mut each.body, page_body);
                composed.push(AxStatement::Each(each));
            }
            AxStatement::Pipeline(mut pipeline) => {
                found_slot |= inject_slot_pipeline(&mut pipeline, page_body);
                composed.push(AxStatement::Pipeline(pipeline));
            }
            other => composed.push(other),
        }
    }

    *statements = composed;
    found_slot
}

fn inject_slot_pipeline(pipeline: &mut AxPipeline, page_body: &[AxStatement]) -> bool {
    let mut found_slot = false;

    for stage in &mut pipeline.stages {
        if let AxPipelineStage::Component(component) = stage {
            if is_slot_component(component) {
                *component = AxComponent::new("Fragment").block(page_body.iter().cloned());
                found_slot = true;
                continue;
            }

            if let AxBody::Block(body) = &mut component.body {
                found_slot |= inject_slot_statements(body, page_body);
            }
        }
    }

    found_slot
}

fn is_slot_component(component: &AxComponent) -> bool {
    component.name == "Slot"
}

fn render_preview_document(document: &AxDocument, root: &AxNode) -> String {
    let mut body = String::new();
    render_node(root, &mut body);
    let head = render_head_html(&document.head);

    format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">{}<style>{}</style></head><body>{}</body></html>",
        head,
        preview_styles(),
        body
    )
}

fn render_head_html(head: &AxHead) -> String {
    let mut html = String::new();

    match &head.title {
        Some(title) => {
            html.push_str("<title>");
            html.push_str(&escape_html(&head_expr_to_string(title)));
            html.push_str("</title>");
        }
        None => html.push_str("<title>Axonyx Preview</title>"),
    }

    for tag in &head.metas {
        html.push_str(&render_head_void_tag("meta", tag));
    }

    for tag in &head.links {
        html.push_str(&render_head_void_tag("link", tag));
    }

    for tag in &head.scripts {
        html.push_str(&render_head_script_tag(tag));
    }

    html
}

fn render_head_void_tag(tag: &str, head_tag: &AxHeadTag) -> String {
    let mut out = String::new();
    out.push('<');
    out.push_str(tag);
    push_head_attrs(head_tag, &mut out);
    out.push('>');
    out
}

fn render_head_script_tag(head_tag: &AxHeadTag) -> String {
    let mut out = String::new();
    out.push_str("<script");
    push_head_attrs(head_tag, &mut out);
    out.push_str("></script>");
    out
}

fn push_head_attrs(head_tag: &AxHeadTag, out: &mut String) {
    for attr in &head_tag.attrs {
        push_head_attr(attr, out);
    }
}

fn push_head_attr(attr: &AxProp, out: &mut String) {
    out.push(' ');
    out.push_str(&attr.name);
    out.push_str("=\"");
    out.push_str(&escape_html(&head_expr_to_string(&attr.value)));
    out.push('"');
}

fn head_expr_to_string(expr: &AxExpr) -> String {
    match expr {
        AxExpr::String(value) => value.clone(),
        AxExpr::Number(value) => value.to_string(),
        AxExpr::Bool(value) => value.to_string(),
        AxExpr::Identifier(value) => value.clone(),
        AxExpr::Member { object, property } => {
            format!("{}.{}", head_expr_to_string(object), property)
        }
        AxExpr::Call { path, args } => {
            let args = args
                .iter()
                .map(head_expr_to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({args})", path.join("."))
        }
    }
}

fn render_node(node: &AxNode, out: &mut String) {
    match node {
        AxNode::Text(text) => out.push_str(&escape_html(text)),
        AxNode::Element {
            tag,
            attrs,
            children,
        } => {
            out.push('<');
            out.push_str(tag);
            for attr in attrs {
                push_attr(attr, out);
            }
            out.push('>');
            for child in children {
                render_node(child, out);
            }
            out.push_str("</");
            out.push_str(tag);
            out.push('>');
        }
    }
}

fn push_attr(attr: &Attribute, out: &mut String) {
    out.push(' ');
    out.push_str(attr.name);
    out.push_str("=\"");
    out.push_str(&escape_html(&attr.value));
    out.push('"');
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn preview_styles() -> &'static str {
    r#"
        :root {
            color-scheme: dark;
            --ax-bg: #0b1220;
            --ax-surface: rgba(15, 23, 42, 0.78);
            --ax-surface-strong: rgba(30, 41, 59, 0.92);
            --ax-border: rgba(148, 163, 184, 0.18);
            --ax-text: #e5eefb;
            --ax-muted: #9fb0ca;
            --ax-accent: #7dd3fc;
            --ax-accent-strong: #38bdf8;
            --ax-shadow: 0 24px 80px rgba(15, 23, 42, 0.35);
        }

        * { box-sizing: border-box; }

        body {
            margin: 0;
            min-height: 100vh;
            font-family: "Segoe UI", Inter, sans-serif;
            background:
                radial-gradient(circle at top, rgba(56, 189, 248, 0.16), transparent 32rem),
                linear-gradient(180deg, #020617 0%, #0f172a 100%);
            color: var(--ax-text);
        }

        [data-ax-root="page"] {
            min-height: 100vh;
            padding: 48px 20px 72px;
        }

        [data-layout="container"] {
            width: min(100%, 1120px);
            margin: 0 auto;
        }

        [data-layout="grid"] {
            display: grid;
            gap: 20px;
        }

        [data-layout="grid"][data-cols="2"] {
            grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
        }

        [data-layout="grid"][data-cols="3"] {
            grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
        }

        [data-layout="grid"][data-cols="4"] {
            grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
        }

        [data-recipe="hello-shell"] {
            gap: 24px;
        }

        [data-recipe="app-shell"] {
            display: grid;
            gap: 18px;
        }

        [data-recipe="app-frame"] {
            gap: 20px;
        }

        [data-ui="card"] {
            padding: 24px;
            border-radius: 24px;
            border: 1px solid var(--ax-border);
            background: linear-gradient(180deg, rgba(15, 23, 42, 0.94), rgba(15, 23, 42, 0.74));
            box-shadow: var(--ax-shadow);
        }

        [data-recipe="hero-card"] {
            padding: 32px;
            background:
                linear-gradient(135deg, rgba(56, 189, 248, 0.16), rgba(14, 165, 233, 0.04)),
                linear-gradient(180deg, rgba(15, 23, 42, 0.96), rgba(15, 23, 42, 0.8));
        }

        [data-ui="card-header"] {
            margin-bottom: 14px;
            font-size: clamp(1.5rem, 3vw, 2.8rem);
            line-height: 1.05;
            font-weight: 700;
            letter-spacing: -0.04em;
        }

        [data-ui="copy"] {
            margin: 0 0 14px;
            color: var(--ax-muted);
            font-size: 1rem;
            line-height: 1.65;
        }

        [data-ui="copy"][data-tone="lead"] {
            font-size: 1.12rem;
            color: #d6e4f5;
            max-width: 60ch;
        }

        [data-ui="copy"][data-tone="eyebrow"] {
            color: var(--ax-accent);
            font-size: 0.82rem;
            font-weight: 700;
            text-transform: uppercase;
            letter-spacing: 0.14em;
        }

        [data-ui="copy"][data-tone="muted"] {
            color: var(--ax-muted);
            font-size: 0.95rem;
        }

        [data-ui="button"] {
            display: inline-flex;
            align-items: center;
            justify-content: center;
            min-height: 44px;
            padding: 0 16px;
            border: 0;
            border-radius: 999px;
            background: linear-gradient(135deg, var(--ax-accent), var(--ax-accent-strong));
            color: #082032;
            font-weight: 700;
            box-shadow: 0 12px 30px rgba(56, 189, 248, 0.22);
        }
    "#
}

#[cfg(test)]
mod tests {
    use axonyx_core::compile_pipeline;

    use super::*;

    #[test]
    fn builds_render_plan_from_ir() {
        let ir = compile_pipeline(r#"Db.Stream("posts") |> layout.Grid(3) |> Card()"#)
            .expect("pipeline should compile");
        let plan = execute(&ir);

        assert_eq!(plan.source, "posts");
        assert_eq!(plan.layout.kind, "grid");
        assert_eq!(plan.layout.columns, 3);
        assert_eq!(plan.view.component, "Card");
    }

    #[test]
    fn builds_render_plan_from_json() {
        let ir = compile_pipeline(r#"Db.Stream("users") |> layout.Grid(2) |> ProfileCard()"#)
            .expect("pipeline should compile");
        let ir_json = serde_json::to_string(&ir).expect("serialize");
        let plan = execute_json(&ir_json).expect("json execution should work");

        assert_eq!(plan.source, "users");
        assert_eq!(plan.layout.columns, 2);
        assert_eq!(plan.view.component, "ProfileCard");
    }

    #[test]
    fn previews_static_ax_page_as_html_document() {
        let html = preview_ax_page(
            r#"
page Home
  Container max: "xl", recipe: "hello-shell"
    Card title: "Hello Axonyx", recipe: "hero-card"
      Copy tone: "lead" -> "A Rust-first page preview."
      Button tone: "primary" -> "Edit app/page.ax"
"#,
        )
        .expect("preview should render");

        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("Hello Axonyx"));
        assert!(html.contains("data-recipe=\"hero-card\""));
        assert!(html.contains("Edit app/page.ax"));
    }

    #[test]
    fn previews_layout_and_page_as_one_html_document() {
        let html = preview_ax_app(
            Some(
                r#"
page RootLayout
  Container max: "xl", recipe: "app-shell"
    Copy tone: "eyebrow" -> "Axonyx Layout"
    Slot
"#,
            ),
            r#"
page Home
  Card title: "Hello Axonyx"
    Copy -> "Page content"
"#,
        )
        .expect("layout preview should render");

        assert!(html.contains("Axonyx Layout"));
        assert!(html.contains("Hello Axonyx"));
        assert!(html.contains("Page content"));
        assert!(html.contains("data-ax-page=\"Home\""));
        assert!(!html.contains("data-component=\"Slot\""));
    }

    #[test]
    fn appends_page_when_layout_has_no_slot() {
        let html = preview_ax_app(
            Some(
                r#"
page RootLayout
  Copy -> "Layout only"
"#,
            ),
            r#"
page Home
  Copy -> "Page body"
"#,
        )
        .expect("layout without slot should still render");

        assert!(html.contains("Layout only"));
        assert!(html.contains("Page body"));
    }

    #[test]
    fn previews_route_with_nested_layouts() {
        let html = preview_ax_route(
            &[
                r#"
page RootLayout
  Container max: "xl", recipe: "app-shell"
    Copy tone: "eyebrow" -> "Root Layout"
    Slot
"#,
                r#"
page DocsLayout
  Card title: "Docs Shell"
    Slot
"#,
            ],
            r#"
page DocsHome
  Copy -> "Nested page"
"#,
        )
        .expect("route preview should render");

        assert!(html.contains("Root Layout"));
        assert!(html.contains("Docs Shell"));
        assert!(html.contains("Nested page"));
        assert!(html.contains("data-ax-page=\"DocsHome\""));
    }

    #[test]
    fn previews_head_metadata_inside_html_head() {
        let html = preview_ax_page(
            r#"
page Home
  title "Axonyx Site"
  meta name: "description", content: "Fast pages with minimal JS."
  link rel: "icon", href: "/favicon.svg", type: "image/svg+xml"
  script src: "/app.js", defer: true
  Copy -> "Hello"
"#,
        )
        .expect("preview should render");

        assert!(html.contains("<title>Axonyx Site</title>"));
        assert!(html.contains("<meta name=\"description\" content=\"Fast pages with minimal JS.\">"));
        assert!(html.contains("<link rel=\"icon\" href=\"/favicon.svg\" type=\"image/svg+xml\">"));
        assert!(html.contains("<script src=\"/app.js\" defer=\"true\"></script>"));
    }

    #[test]
    fn page_title_overrides_layout_title_while_layout_meta_stays() {
        let html = preview_ax_app(
            Some(
                r#"
page RootLayout
  title "Layout Title"
  meta name: "description", content: "Layout description."
  Slot
"#,
            ),
            r#"
page Home
  title "Page Title"
  link rel: "icon", href: "/favicon.svg"
  Copy -> "Body"
"#,
        )
        .expect("layout preview should render");

        assert!(html.contains("<title>Page Title</title>"));
        assert!(html.contains("<meta name=\"description\" content=\"Layout description.\">"));
        assert!(html.contains("<link rel=\"icon\" href=\"/favicon.svg\">"));
    }
}
