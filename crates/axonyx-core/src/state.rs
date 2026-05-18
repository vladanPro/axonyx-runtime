use crate::ax_ast::AxExpr;
use crate::ax_ast_v2::AxFileV2;
use crate::ax_parser::parse_expr;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AxSignalId {
    pub scope: String,
    pub name: String,
    pub index: u32,
}

impl AxSignalId {
    pub fn new(scope: impl Into<String>, name: impl Into<String>, index: u32) -> Self {
        Self {
            scope: scope.into(),
            name: name.into(),
            index,
        }
    }

    pub fn route(name: impl Into<String>, index: u32) -> Self {
        Self::new("route", name, index)
    }

    pub fn root(name: impl Into<String>, index: u32) -> Self {
        Self::new("root", name, index)
    }

    pub fn stable_key(&self) -> String {
        format!("{}:{}:{}", self.scope, self.name, self.index)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum AxStateValue {
    Null,
    String(String),
    Bool(bool),
    Number(f64),
}

impl AxStateValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "Null",
            Self::String(_) => "String",
            Self::Bool(_) => "Bool",
            Self::Number(_) => "Number",
        }
    }
}

impl From<&str> for AxStateValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<String> for AxStateValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<bool> for AxStateValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i32> for AxStateValue {
    fn from(value: i32) -> Self {
        Self::Number(value as f64)
    }
}

impl From<u32> for AxStateValue {
    fn from(value: u32) -> Self {
        Self::Number(value as f64)
    }
}

impl From<f64> for AxStateValue {
    fn from(value: f64) -> Self {
        Self::Number(value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxSignalState {
    pub id: AxSignalId,
    pub ty: String,
    pub value: AxStateValue,
}

impl AxSignalState {
    pub fn new(id: AxSignalId, value: impl Into<AxStateValue>) -> Self {
        let value = value.into();
        Self {
            ty: value.type_name().to_string(),
            id,
            value,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AxBindTarget {
    Value,
    Checked,
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxStateBinding {
    pub signal: AxSignalId,
    pub target: AxBindTarget,
}

impl AxStateBinding {
    pub fn value(signal: AxSignalId) -> Self {
        Self {
            signal,
            target: AxBindTarget::Value,
        }
    }

    pub fn checked(signal: AxSignalId) -> Self {
        Self {
            signal,
            target: AxBindTarget::Checked,
        }
    }

    pub fn text(signal: AxSignalId) -> Self {
        Self {
            signal,
            target: AxBindTarget::Text,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AxStateSnapshot {
    pub signals: Vec<AxSignalState>,
}

impl AxStateSnapshot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_signal(mut self, signal: AxSignalState) -> Self {
        self.signals.push(signal);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.signals.is_empty()
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AxStateManifestError {
    #[error("invalid state initializer for `{name}`: {message}")]
    InvalidInitializer { name: String, message: String },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AxStateManifest {
    pub signals: Vec<AxStateManifestSignal>,
}

impl AxStateManifest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_signal(mut self, signal: AxStateManifestSignal) -> Self {
        self.signals.push(signal);
        self
    }

    pub fn signal_by_name(&self, name: &str) -> Option<&AxStateManifestSignal> {
        self.signals.iter().find(|signal| signal.name == name)
    }

    pub fn signal_by_key(&self, key: &str) -> Option<&AxStateManifestSignal> {
        self.signals.iter().find(|signal| signal.key == key)
    }

    pub fn is_empty(&self) -> bool {
        self.signals.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxStateManifestSignal {
    pub id: AxSignalId,
    pub key: String,
    pub name: String,
    pub scope: String,
    pub ty: String,
    pub initial: AxStateValue,
}

impl AxStateManifestSignal {
    pub fn new(id: AxSignalId, ty: impl Into<String>, initial: impl Into<AxStateValue>) -> Self {
        let key = id.stable_key();
        Self {
            name: id.name.clone(),
            scope: id.scope.clone(),
            id,
            key,
            ty: ty.into(),
            initial: initial.into(),
        }
    }
}

pub fn build_state_manifest(file: &AxFileV2) -> Result<AxStateManifest, AxStateManifestError> {
    let mut manifest = AxStateManifest::new();

    for (index, state) in file.states.iter().enumerate() {
        let initial = parse_state_manifest_value(&state.name, &state.value)?;
        let ty = state
            .ty
            .clone()
            .unwrap_or_else(|| initial.type_name().to_string());
        manifest = manifest.with_signal(AxStateManifestSignal::new(
            AxSignalId::root(&state.name, index as u32 + 1),
            ty,
            initial,
        ));
    }

    Ok(manifest)
}

fn parse_state_manifest_value(
    name: &str,
    source: &str,
) -> Result<AxStateValue, AxStateManifestError> {
    let expr = parse_expr(source, 1).map_err(|error| AxStateManifestError::InvalidInitializer {
        name: name.to_string(),
        message: error.to_string(),
    })?;

    let expr = match expr {
        AxExpr::Call { path, args } if path.as_slice() == ["signal"] && args.len() == 1 => {
            args[0].clone()
        }
        AxExpr::Call { .. } => {
            return Err(AxStateManifestError::InvalidInitializer {
                name: name.to_string(),
                message: "expected a literal value or signal(literal)".to_string(),
            });
        }
        other => other,
    };

    expr_to_state_value(name, &expr)
}

fn expr_to_state_value(name: &str, expr: &AxExpr) -> Result<AxStateValue, AxStateManifestError> {
    match expr {
        AxExpr::String(value) => Ok(AxStateValue::String(value.clone())),
        AxExpr::Bool(value) => Ok(AxStateValue::Bool(*value)),
        AxExpr::Number(value) => Ok(AxStateValue::Number(*value as f64)),
        _ => Err(AxStateManifestError::InvalidInitializer {
            name: name.to_string(),
            message: "state manifest v1 supports only String, Bool, Number, or signal(literal)"
                .to_string(),
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxStatePatch {
    pub signal: AxSignalId,
    pub value: AxStateValue,
    pub source: Option<String>,
}

impl AxStatePatch {
    pub fn new(signal: AxSignalId, value: impl Into<AxStateValue>) -> Self {
        Self {
            signal,
            value: value.into(),
            source: None,
        }
    }

    pub fn from_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

pub mod prelude {
    pub use super::build_state_manifest;
    pub use super::AxBindTarget;
    pub use super::AxSignalId;
    pub use super::AxSignalState;
    pub use super::AxStateBinding;
    pub use super::AxStateManifest;
    pub use super::AxStateManifestError;
    pub use super::AxStateManifestSignal;
    pub use super::AxStatePatch;
    pub use super::AxStateSnapshot;
    pub use super::AxStateValue;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_id_has_stable_key() {
        let id = AxSignalId::root("theme", 1);

        assert_eq!(id.stable_key(), "root:theme:1");
    }

    #[test]
    fn signal_state_keeps_type_name() {
        let state = AxSignalState::new(AxSignalId::route("open", 2), true);

        assert_eq!(state.ty, "Bool");
        assert_eq!(state.value, AxStateValue::Bool(true));
    }

    #[test]
    fn patch_can_record_source() {
        let patch = AxStatePatch::new(AxSignalId::root("theme", 1), "gold")
            .from_source("select[data-ax-signal]");

        assert_eq!(patch.signal.stable_key(), "root:theme:1");
        assert_eq!(patch.value, AxStateValue::String("gold".to_string()));
        assert_eq!(patch.source, Some("select[data-ax-signal]".to_string()));
    }

    #[test]
    fn builds_state_manifest_from_v2_state_declarations() {
        let file = crate::ax_parser_v2::parse_ax_v2(
            r#"
page Home

state theme: String = "silver"
state count: Number = 0
state enabled = signal(true)

<input bind:value={theme} />
"#,
        )
        .expect("v2 file should parse");

        let manifest = build_state_manifest(&file).expect("manifest should build");

        assert_eq!(manifest.signals.len(), 3);
        assert_eq!(manifest.signals[0].name, "theme");
        assert_eq!(manifest.signals[0].key, "root:theme:1");
        assert_eq!(manifest.signals[0].scope, "root");
        assert_eq!(manifest.signals[0].ty, "String");
        assert_eq!(
            manifest.signals[0].initial,
            AxStateValue::String("silver".to_string())
        );
        assert_eq!(manifest.signals[1].key, "root:count:2");
        assert_eq!(manifest.signals[1].ty, "Number");
        assert_eq!(manifest.signals[1].initial, AxStateValue::Number(0.0));
        assert_eq!(manifest.signals[2].key, "root:enabled:3");
        assert_eq!(manifest.signals[2].ty, "Bool");
        assert_eq!(manifest.signals[2].initial, AxStateValue::Bool(true));
        assert_eq!(
            manifest
                .signal_by_name("theme")
                .expect("theme signal should exist")
                .key,
            "root:theme:1"
        );
    }

    #[test]
    fn state_manifest_rejects_non_literal_initializers() {
        let file = crate::ax_parser_v2::parse_ax_v2(
            r#"
page Home

state theme = Runtime.Env.public.THEME

<Copy>{theme}</Copy>
"#,
        )
        .expect("v2 file should parse");

        let error = build_state_manifest(&file).expect_err("manifest should reject initializer");

        assert_eq!(
            error,
            AxStateManifestError::InvalidInitializer {
                name: "theme".to_string(),
                message: "state manifest v1 supports only String, Bool, Number, or signal(literal)"
                    .to_string(),
            }
        );
    }
}
