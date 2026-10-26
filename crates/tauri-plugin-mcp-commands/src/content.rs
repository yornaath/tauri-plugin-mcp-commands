//! Tool results as MCP content blocks, for tools that return more than JSON,
//! e.g. images an agent should look at.
//!
//! A tool returning [`ToolContent`] (or `Result<ToolContent, E>`) gets no
//! output schema, and its result is sent as `content` blocks instead of
//! `structuredContent`.

use base64::Engine as _;
use serde::Serialize;
use serde_json::Value;

/// The key [`ToolContent`] serializes under. The server recognises results
/// and output schemas by it.
pub(crate) const MARKER: &str = "__mcp_tool_content";

/// A tool result made of MCP content blocks, sent in order.
///
/// ```
/// # use tauri_plugin_mcp_commands::ToolContent;
/// let png: Vec<u8> = vec![];
/// let result = ToolContent::new()
///     .text("Rendered 3 frames.")
///     .image_png(&png);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ToolContent {
    #[serde(rename = "__mcp_tool_content")]
    blocks: Vec<ContentPart>,
}

/// One content block of a [`ToolContent`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentPart {
    Text {
        text: String,
    },
    Image {
        /// Base64 encoded.
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

impl ToolContent {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.blocks.push(ContentPart::Text { text: text.into() });
        self
    }

    /// An image block from raw bytes of type `mime_type`, e.g. `image/png`.
    pub fn image(mut self, bytes: &[u8], mime_type: impl Into<String>) -> Self {
        self.blocks.push(ContentPart::Image {
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
            mime_type: mime_type.into(),
        });
        self
    }

    pub fn image_png(self, bytes: &[u8]) -> Self {
        self.image(bytes, "image/png")
    }

    pub fn blocks(&self) -> &[ContentPart] {
        &self.blocks
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for ToolContent {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ToolContent".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "object",
            "properties": { "__mcp_tool_content": { "type": "array" } },
            "required": ["__mcp_tool_content"],
        })
    }
}

/// Whether an output schema describes a [`ToolContent`], which gets none.
pub(crate) fn is_content_schema(schema: &serde_json::Map<String, Value>) -> bool {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|properties| properties.len() == 1 && properties.contains_key(MARKER))
}

/// The content blocks of a serialized [`ToolContent`], or `None` for any
/// other value.
pub(crate) fn blocks_of(value: &Value) -> Option<&Vec<Value>> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    object.get(MARKER)?.as_array()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_under_the_marker() {
        let value = serde_json::to_value(ToolContent::new().text("hi").image(&[1, 2, 3], "image/png")).unwrap();
        assert_eq!(
            value,
            json!({ MARKER: [
                { "type": "text", "text": "hi" },
                { "type": "image", "data": "AQID", "mimeType": "image/png" },
            ]})
        );
        assert_eq!(blocks_of(&value).map(Vec::len), Some(2));
    }

    #[test]
    fn other_values_are_not_content() {
        assert!(blocks_of(&json!({ "result": [] })).is_none());
        assert!(blocks_of(&json!({ MARKER: [], "extra": 1 })).is_none());
        assert!(blocks_of(&json!([1])).is_none());
    }
}
