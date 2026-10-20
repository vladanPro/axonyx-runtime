//! Explicit, typed server-side Rust callbacks. This is not a plugin sandbox.

use std::collections::BTreeMap;
use std::io::{self, Write};

use crate::native_response::{
    private_error, AxNativeBinaryResponse, AxNativeResponseContext, AxNativeResponseError,
    MAX_NATIVE_BINARY_BYTES,
};
use crate::server::AxHttpResponse;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxNativeLimits {
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub depth: usize,
    pub nodes: usize,
}

impl Default for AxNativeLimits {
    fn default() -> Self {
        Self {
            input_bytes: 64 * 1024,
            output_bytes: 64 * 1024,
            depth: 32,
            nodes: 8192,
        }
    }
}

/// Public failures deliberately exclude callback, payload and serde details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AxNativeError {
    #[error("invalid native function registration")]
    InvalidRegistration,
    #[error("native function already registered")]
    DuplicateRegistration,
    #[error("unknown native function")]
    UnknownFunction,
    #[error("invalid native function input")]
    InvalidInput,
    #[error("invalid native function output")]
    InvalidOutput,
    #[error("native function rejected the operation")]
    Rejected,
    #[error("native function payload limit exceeded")]
    PayloadLimit,
}

type Callback = dyn Fn(Value, AxNativeLimits) -> Result<Value, AxNativeError> + Send + Sync;
type ResponseCallback =
    dyn Fn(&AxNativeResponseContext, Value) -> Result<AxHttpResponse, AxNativeError> + Send + Sync;
type PublicResponseCallback = dyn Fn(Value) -> Result<AxHttpResponse, AxNativeError> + Send + Sync;

/// Register during startup; then share the immutable registry through `Arc`.
#[derive(Default)]
pub struct AxNativeRegistry {
    limits: AxNativeLimits,
    functions: BTreeMap<String, Box<Callback>>,
    private_responses: BTreeMap<String, Box<ResponseCallback>>,
    public_responses: BTreeMap<String, Box<PublicResponseCallback>>,
}

impl AxNativeRegistry {
    pub fn new(limits: AxNativeLimits) -> Self {
        Self {
            limits,
            functions: BTreeMap::new(),
            private_responses: BTreeMap::new(),
            public_responses: BTreeMap::new(),
        }
    }

    pub fn register<I, O, E, F>(&mut self, name: &str, function: F) -> Result<(), AxNativeError>
    where
        I: DeserializeOwned,
        O: Serialize + DeserializeOwned,
        F: Fn(I) -> Result<O, E> + Send + Sync + 'static,
    {
        if !valid_name(name) {
            return Err(AxNativeError::InvalidRegistration);
        }
        if self.contains_name(name) {
            return Err(AxNativeError::DuplicateRegistration);
        }
        self.functions.insert(
            name.to_owned(),
            Box::new(move |value, limits| {
                let input =
                    serde_json::from_value::<I>(value).map_err(|_| AxNativeError::InvalidInput)?;
                let output = function(input).map_err(|_| AxNativeError::Rejected)?;
                let bytes = encode(&output, limits.output_bytes, AxNativeError::InvalidOutput)?;
                // Check that the wire representation is valid for the declared result type.
                // For example serde serializes a non-finite float as null.
                serde_json::from_slice::<O>(&bytes).map_err(|_| AxNativeError::InvalidOutput)?;
                let value =
                    serde_json::from_slice(&bytes).map_err(|_| AxNativeError::InvalidOutput)?;
                check_shape(&value, limits)?;
                Ok(value)
            }),
        );
        Ok(())
    }

    /// Trusted application callback, separate from the JSON result channel.
    pub fn register_private_response<I, F>(
        &mut self,
        name: &str,
        max_bytes: usize,
        function: F,
    ) -> Result<(), AxNativeError>
    where
        I: DeserializeOwned,
        F: Fn(&AxNativeResponseContext, I) -> Result<AxNativeBinaryResponse, AxNativeResponseError>
            + Send
            + Sync
            + 'static,
    {
        if !valid_name(name) || max_bytes == 0 || max_bytes > MAX_NATIVE_BINARY_BYTES {
            return Err(AxNativeError::InvalidRegistration);
        }
        if self.contains_name(name) {
            return Err(AxNativeError::DuplicateRegistration);
        }
        self.private_responses.insert(
            name.to_owned(),
            Box::new(move |context, value| {
                let input =
                    serde_json::from_value::<I>(value).map_err(|_| AxNativeError::InvalidInput)?;
                Ok(match function(context, input) {
                    Ok(response) => response.into_response(max_bytes),
                    Err(error) => private_error(error),
                })
            }),
        );
        Ok(())
    }

    pub(crate) fn private_response(
        &self,
        context: &AxNativeResponseContext,
        name: &str,
        input: Value,
    ) -> Result<AxHttpResponse, AxNativeError> {
        let callback = self
            .private_responses
            .get(name)
            .ok_or(AxNativeError::UnknownFunction)?;
        check_shape(&input, self.limits)?;
        encode(&input, self.limits.input_bytes, AxNativeError::InvalidInput)?;
        callback(context, input)
    }

    /// Explicit anonymous admission, NOT a product authorization grant. The
    /// trusted callback must check current publication/ownership before reading
    /// bytes. It receives no invented session subject and is separate from both
    /// the private response and Native.call channels.
    pub fn register_public_response<I, F>(
        &mut self,
        name: &str,
        max_bytes: usize,
        function: F,
    ) -> Result<(), AxNativeError>
    where
        I: DeserializeOwned,
        F: Fn(I) -> Result<AxNativeBinaryResponse, AxNativeResponseError> + Send + Sync + 'static,
    {
        if !valid_name(name) || max_bytes == 0 || max_bytes > MAX_NATIVE_BINARY_BYTES {
            return Err(AxNativeError::InvalidRegistration);
        }
        if self.contains_name(name) {
            return Err(AxNativeError::DuplicateRegistration);
        }
        self.public_responses.insert(
            name.to_owned(),
            Box::new(move |value| {
                let input =
                    serde_json::from_value::<I>(value).map_err(|_| AxNativeError::InvalidInput)?;
                Ok(match function(input) {
                    Ok(response) => response.into_response(max_bytes),
                    Err(error) => private_error(error),
                })
            }),
        );
        Ok(())
    }

    pub(crate) fn public_response(
        &self,
        name: &str,
        input: Value,
    ) -> Result<AxHttpResponse, AxNativeError> {
        let callback = self
            .public_responses
            .get(name)
            .ok_or(AxNativeError::UnknownFunction)?;
        check_shape(&input, self.limits)?;
        encode(&input, self.limits.input_bytes, AxNativeError::InvalidInput)?;
        callback(input)
    }

    fn contains_name(&self, name: &str) -> bool {
        self.functions.contains_key(name)
            || self.private_responses.contains_key(name)
            || self.public_responses.contains_key(name)
    }

    pub fn call(&self, name: &str, input: Value) -> Result<Value, AxNativeError> {
        let function = self
            .functions
            .get(name)
            .ok_or(AxNativeError::UnknownFunction)?;
        check_shape(&input, self.limits)?;
        encode(&input, self.limits.input_bytes, AxNativeError::InvalidInput)?;
        function(input, self.limits)
    }
}

fn valid_name(name: &str) -> bool {
    axonyx_core::ax_native::is_native_function_name(name)
}

fn check_shape(value: &Value, limits: AxNativeLimits) -> Result<(), AxNativeError> {
    let mut stack = vec![(value, 1usize)];
    let mut nodes = 0usize;
    while let Some((value, depth)) = stack.pop() {
        if depth > limits.depth || nodes >= limits.nodes {
            return Err(AxNativeError::PayloadLimit);
        }
        nodes += 1;
        let children = match value {
            Value::Array(values) => values.len(),
            Value::Object(values) => values.len(),
            _ => 0,
        };
        // Bound pending work before allocating stack entries for a large container.
        if children > limits.nodes.saturating_sub(nodes + stack.len()) {
            return Err(AxNativeError::PayloadLimit);
        }
        match value {
            Value::Array(values) => stack.extend(values.iter().map(|value| (value, depth + 1))),
            Value::Object(values) => {
                stack.extend(values.values().map(|value| (value, depth + 1)));
            }
            _ => {}
        }
    }
    Ok(())
}

struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::other("payload limit exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode<T: Serialize>(
    value: &T,
    limit: usize,
    invalid: AxNativeError,
) -> Result<Vec<u8>, AxNativeError> {
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit,
        exceeded: false,
    };
    if serde_json::to_writer(&mut writer, value).is_err() {
        return Err(if writer.exceeded {
            AxNativeError::PayloadLimit
        } else {
            invalid
        });
    }
    Ok(writer.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        text: String,
    }

    fn registry(limits: AxNativeLimits) -> AxNativeRegistry {
        let mut registry = AxNativeRegistry::new(limits);
        registry
            .register("content.prepare", |input: Input| {
                Ok::<_, ()>(Input {
                    text: input.text.trim().to_owned(),
                })
            })
            .unwrap();
        registry
    }

    #[test]
    fn typed_callback_and_immutable_sharing() {
        let registry = Arc::new(registry(AxNativeLimits::default()));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let registry = Arc::clone(&registry);
                std::thread::spawn(move || {
                    registry.call("content.prepare", json!({"text":" hello "}))
                })
            })
            .collect();
        for handle in handles {
            assert_eq!(handle.join().unwrap().unwrap(), json!({"text":"hello"}));
        }
        assert_eq!(
            AxNativeRegistry::default().call("content.prepare", Value::Null),
            Err(AxNativeError::UnknownFunction)
        );
    }

    #[test]
    fn registration_is_explicit_and_duplicates_do_not_replace() {
        let mut registry = registry(AxNativeLimits::default());
        for name in ["", "x..y", "x::y", "1x", " x", "secret/name", "caf\u{e9}"] {
            assert_eq!(
                registry.register(name, |_: Value| Ok::<_, ()>(Value::Null)),
                Err(AxNativeError::InvalidRegistration)
            );
        }
        assert_eq!(
            registry.register("content.prepare", |_: Value| Ok::<_, ()>(Value::Null)),
            Err(AxNativeError::DuplicateRegistration)
        );
        assert_eq!(
            registry
                .call("content.prepare", json!({"text":"ok"}))
                .unwrap(),
            json!({"text":"ok"})
        );
        assert_eq!(
            registry.call("secret.function", Value::Null),
            Err(AxNativeError::UnknownFunction)
        );
    }

    #[test]
    fn invalid_input_never_executes_callback() {
        let count = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&count);
        let mut registry = AxNativeRegistry::default();
        registry
            .register("test", move |input: Input| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok::<_, ()>(input)
            })
            .unwrap();
        for input in [
            json!({}),
            json!({"text":1}),
            json!({"text":"ok", "extra":"secret"}),
        ] {
            assert_eq!(
                registry.call("test", input),
                Err(AxNativeError::InvalidInput)
            );
        }
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn errors_do_not_disclose_callback_details() {
        let mut registry = AxNativeRegistry::default();
        registry
            .register("test", |_: Value| Err::<Value, _>("password=secret"))
            .unwrap();
        let error = registry.call("test", Value::Null).unwrap_err();
        assert_eq!(error, AxNativeError::Rejected);
        assert!(!format!("{error} {error:?}").contains("secret"));
    }

    #[test]
    fn input_limits_apply_before_callback() {
        for (limits, input) in [
            (
                AxNativeLimits {
                    input_bytes: 4,
                    ..Default::default()
                },
                json!({"text":"hello"}),
            ),
            (
                AxNativeLimits {
                    depth: 1,
                    ..Default::default()
                },
                json!({"text":"hello"}),
            ),
            (
                AxNativeLimits {
                    nodes: 1,
                    ..Default::default()
                },
                json!({"text":"hello"}),
            ),
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let count = Arc::clone(&calls);
            let mut registry = AxNativeRegistry::new(limits);
            registry
                .register("test", move |input: Value| {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, ()>(input)
                })
                .unwrap();
            assert_eq!(
                registry.call("test", input),
                Err(AxNativeError::PayloadLimit)
            );
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
        let limits = AxNativeLimits {
            input_bytes: 2,
            depth: 1,
            nodes: 1,
            ..Default::default()
        };
        assert_eq!(
            encode(&json!({}), limits.input_bytes, AxNativeError::InvalidInput).unwrap(),
            b"{}"
        );
        assert!(check_shape(&json!({}), limits).is_ok());
    }

    #[test]
    fn wide_and_nested_values_obey_structural_budgets() {
        let limits = AxNativeLimits {
            nodes: 4,
            depth: 3,
            ..Default::default()
        };
        assert!(check_shape(&json!([0, 1, 2]), limits).is_ok());
        assert_eq!(
            check_shape(&json!([0, 1, 2, 3]), limits),
            Err(AxNativeError::PayloadLimit)
        );
        assert!(check_shape(&json!([[0]]), limits).is_ok());
        assert_eq!(
            check_shape(&json!([[[0]]]), limits),
            Err(AxNativeError::PayloadLimit)
        );
        assert_eq!(
            check_shape(&Value::Null, AxNativeLimits { nodes: 0, ..limits }),
            Err(AxNativeError::PayloadLimit)
        );
        assert!(!valid_name(&"a".repeat(129)));
        assert!(valid_name("_package.Content_1.prepare"));
    }

    #[test]
    fn output_limits_and_result_type_are_checked() {
        for limits in [
            AxNativeLimits {
                output_bytes: 4,
                ..Default::default()
            },
            AxNativeLimits {
                depth: 1,
                ..Default::default()
            },
            AxNativeLimits {
                nodes: 1,
                ..Default::default()
            },
        ] {
            let mut registry = AxNativeRegistry::new(limits);
            registry
                .register("test", |_: Value| Ok::<_, ()>(json!({"text":"hello"})))
                .unwrap();
            assert_eq!(
                registry.call("test", Value::Null),
                Err(AxNativeError::PayloadLimit)
            );
        }
        let mut registry = AxNativeRegistry::default();
        registry
            .register("test", |_: Value| Ok::<_, ()>(f64::NAN))
            .unwrap();
        assert_eq!(
            registry.call("test", Value::Null),
            Err(AxNativeError::InvalidOutput)
        );
    }

    #[test]
    fn wire_limits_count_escaping_and_reject_non_roundtrippable_results() {
        assert_eq!(
            encode(&"\n", 3, AxNativeError::InvalidInput),
            Err(AxNativeError::PayloadLimit)
        );
        #[derive(Deserialize, Serialize)]
        struct ResultRecord {
            #[serde(skip_serializing)]
            required: String,
        }
        let mut registry = AxNativeRegistry::default();
        registry
            .register("test", |_: Value| {
                let output = ResultRecord {
                    required: "private".to_owned(),
                };
                assert!(!output.required.is_empty());
                Ok::<_, ()>(output)
            })
            .unwrap();
        assert_eq!(
            registry.call("test", Value::Null),
            Err(AxNativeError::InvalidOutput)
        );
    }
}
