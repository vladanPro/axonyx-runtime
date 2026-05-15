use serde::{Deserialize, Serialize};

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
    pub use super::AxBindTarget;
    pub use super::AxSignalId;
    pub use super::AxSignalState;
    pub use super::AxStateBinding;
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
}
