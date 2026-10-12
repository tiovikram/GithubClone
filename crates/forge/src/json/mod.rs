//! Builders for GitHub-shaped JSON. Shared by REST and GraphQL.
pub mod git;
pub mod issue;
pub mod key;
pub mod pull;
pub mod repo;
pub mod user;

use chrono::{DateTime, Utc};
use serde_json::Value;

/// GitHub timestamp format: `2011-01-26T19:01:12Z`.
pub fn ts(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub fn ts_opt(t: Option<DateTime<Utc>>) -> Value {
    t.map(|t| Value::String(ts(t))).unwrap_or(Value::Null)
}

pub fn opt_str(s: &Option<String>) -> Value {
    s.as_ref().map(|s| Value::String(s.clone())).unwrap_or(Value::Null)
}
