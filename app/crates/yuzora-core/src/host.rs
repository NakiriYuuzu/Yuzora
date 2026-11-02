//! Helpers for a lane crate's `host_module()`: JSON in and out of the
//! script boundary, and the module naming rule (`yuzora:<lane>`).
//!
//! A HostModule's `declarations` must name every function it registers and
//! nothing else (`HostModule::validate` refuses a mismatch at startup), so a
//! lane adds a function and its `export function …` line together.

use gpui_shell::{HostArguments, HostError, HostValue};
use serde_json::{Map, Number, Value};

/// The import specifier for a lane: `yuzora:git`, `yuzora:db`, …
pub fn module_name(lane: &str) -> String {
    format!("yuzora:{lane}")
}

pub fn to_host(v: &Value) -> HostValue {
    match v {
        Value::Null => HostValue::Null,
        Value::Bool(b) => HostValue::Bool(*b),
        Value::Number(n) => HostValue::Number(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => HostValue::Str(s.clone()),
        Value::Array(a) => HostValue::Array(a.iter().map(to_host).collect()),
        Value::Object(o) => HostValue::Object(o.iter().map(|(k, v)| (k.clone(), to_host(v))).collect()),
    }
}

pub fn from_host(v: &HostValue) -> Value {
    match v {
        HostValue::Null => Value::Null,
        HostValue::Bool(b) => Value::Bool(*b),
        // Whole numbers come back as integers, so `3` round-trips as `3`, not `3.0`.
        HostValue::Number(n) if n.fract() == 0.0 && n.abs() < 9.0e15 => Value::Number((*n as i64).into()),
        HostValue::Number(n) => Number::from_f64(*n).map_or(Value::Null, Value::Number),
        HostValue::Str(s) => Value::String(s.clone()),
        HostValue::Array(a) => Value::Array(a.iter().map(from_host).collect()),
        HostValue::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), from_host(v))).collect::<Map<_, _>>()),
    }
}

/// Argument `i` as JSON (`null` when missing).
pub fn arg(a: &HostArguments, i: usize) -> Value {
    a.get(i).map(from_host).unwrap_or(Value::Null)
}

/// Argument `i`, an object's string field.
pub fn opt_str(a: &HostArguments, i: usize, key: &str) -> Option<String> {
    a.get(i).and_then(|o| o.get(key)).and_then(HostValue::as_str).map(str::to_owned)
}

/// `{name: value}` variables for i18n from an argument (strings and numbers).
pub fn vars(a: &HostArguments, i: usize) -> Vec<(String, String)> {
    match arg(a, i) {
        Value::Object(m) => m
            .into_iter()
            .map(|(k, v)| (k, match v {
                Value::String(s) => s,
                other => other.to_string(),
            }))
            .collect(),
        _ => Vec::new(),
    }
}

pub fn error(e: impl std::fmt::Display) -> HostError {
    HostError::new(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_round_trips_through_host_values() {
        let v = json!({ "n": 3, "f": 1.5, "s": "字", "a": [true, null], "o": { "k": -2 } });
        assert_eq!(from_host(&to_host(&v)), v);
        assert_eq!(module_name("git"), "yuzora:git");
    }
}
