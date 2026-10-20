use serde_json::{Value, json};

use crate::tool::JsonObject;

/// MCP output schemas must be objects: wrap anything else as `{ result }`.
#[cfg_attr(not(any(feature = "specta", feature = "schemars")), allow(dead_code))]
pub(crate) fn object_or_wrapped(schema: Value) -> (JsonObject, bool) {
    match schema {
        Value::Object(o) if o.get("type") == Some(&json!("object")) => (o, false),
        other => {
            let Value::Object(o) = json!({
                "type": "object",
                "properties": { "result": other },
                "required": ["result"],
            }) else {
                unreachable!()
            };
            (o, true)
        }
    }
}
