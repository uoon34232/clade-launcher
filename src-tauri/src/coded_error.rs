//! Structured errors for the frontend.
//!
//! Rust never returns finished prose for the user: it returns a stable `code` and, where a message
//! needs facts (a profile name, a byte count), named `params`. The frontend turns code + params
//! into English or Russian text from its local translation files (`src/locales`). The English
//! `message` is for logs and developers; the UI does not show it. Params never hold secrets.

use std::fmt;

use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CodedError {
    pub code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// English detail for logs and developers. Never shown to the user.
    pub message: String,
}

impl CodedError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            params: None,
            message: message.into(),
        }
    }

    pub fn with(mut self, params: Value) -> Self {
        self.params = Some(params);
        self
    }

    /// The generic "could not read or save files" failure.
    pub fn storage(detail: impl fmt::Display) -> Self {
        Self::new("storage", format!("storage failure: {detail}"))
    }
}

impl fmt::Display for CodedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// A parameter object from key/value pairs, for call sites that read better that way.
pub fn params<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

/// Shorthand for a single string parameter.
pub fn param(key: &str, value: impl Into<String>) -> Value {
    json!({ key: value.into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_code_and_params_and_omits_missing_params() {
        let plain = serde_json::to_value(CodedError::new("not_found", "gone")).unwrap();
        assert_eq!(plain["code"], "not_found");
        assert!(plain.get("params").is_none());
        let rich = serde_json::to_value(
            CodedError::new("duplicate_name", "dup").with(param("name", "Survival")),
        )
        .unwrap();
        assert_eq!(rich["params"]["name"], "Survival");
    }
}
