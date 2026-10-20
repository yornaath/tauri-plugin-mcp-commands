//! Fallback schema source for apps that don't use specta: `schemars`
//! (feature `schemars`). Used for `#[mcp_tool]` commands without
//! `#[specta::specta]`; every input and the result must implement
//! `schemars::JsonSchema`.

use schemars::{SchemaGenerator, generate::SchemaSettings};
use serde_json::{Map, Value, json};

use crate::{
    schema_util::object_or_wrapped,
    tool::{JsonObject, SchemarsSig, ToolSchemas},
};

pub(crate) fn build(f: &fn(&mut SchemaGenerator) -> SchemarsSig, tool: &str, inputs: &[&str]) -> Result<ToolSchemas, String> {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let sig = f(&mut generator);
    let args: Vec<_> = sig.inputs.into_iter().flatten().collect();
    if args.len() != inputs.len() {
        return Err(format!("schemars saw {} input parameters for `{tool}` but the macro found {}", args.len(), inputs.len()));
    }
    let mut properties = Map::new();
    let mut required = Vec::new();
    for ((schema, optional), name) in args.into_iter().zip(inputs) {
        if !optional {
            required.push(Value::String((*name).to_owned()));
        }
        properties.insert((*name).to_owned(), schema.to_value());
    }
    let defs = generator.take_definitions(true);

    let mut input = JsonObject::new();
    input.insert("type".into(), "object".into());
    input.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        input.insert("required".into(), Value::Array(required));
    }
    if !defs.is_empty() {
        input.insert("$defs".into(), Value::Object(defs.clone()));
    }

    let output = sig
        .result
        .map(|s| inline_root_ref(s.to_value(), &defs))
        .filter(|s| s != &json!({ "type": "null" }));
    let (output, wrap_output) = match output {
        None => (None, false),
        Some(schema) => {
            let (mut obj, wrap) = object_or_wrapped(schema);
            if !defs.is_empty() {
                obj.insert("$defs".into(), Value::Object(defs));
            }
            (Some(obj), wrap)
        }
    };
    Ok(ToolSchemas { input, output, wrap_output })
}

/// A root `{"$ref": "#/$defs/X"}` becomes X's body, so an object result shows
/// `"type": "object"` at the root as MCP requires.
fn inline_root_ref(schema: Value, defs: &Map<String, Value>) -> Value {
    let target = schema.as_object().filter(|o| o.len() == 1).and_then(|o| o.get("$ref")).and_then(Value::as_str);
    match target.and_then(|r| r.strip_prefix("#/$defs/")).and_then(|name| defs.get(name)) {
        Some(body) => body.clone(),
        None => schema,
    }
}
