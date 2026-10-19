//! The `schemars` schema fallback: `cargo test --features schemars --test schemars`.
#![cfg(feature = "schemars")]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Runtime, State, test::MockRuntime};
use tauri_plugin_mcp_server::{ToolDef, collect_mcp_tools, mcp_tool};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    item_id: u32,
    label: Option<String>,
}

pub struct Repo;

/// Finds items.
#[mcp_tool(read_only)]
#[tauri::command]
async fn find<R: Runtime>(_app: AppHandle<R>, _repo: State<'_, Repo>, query: String, page_size: Option<u32>) -> Result<Vec<Item>, String> {
    let _ = (query, page_size);
    Ok(vec![])
}

#[mcp_tool]
#[tauri::command]
async fn get(id: u32) -> Result<Item, String> {
    Ok(Item { item_id: id, label: None })
}

#[mcp_tool]
#[tauri::command]
fn touch(item: Item) {
    let _ = item;
}

#[test]
fn schemas() {
    let tools: Vec<ToolDef<MockRuntime>> = collect_mcp_tools![find, get, touch];

    let find = tools[0].schemas().unwrap();
    assert_eq!(find.input["required"], json!(["query"]));
    assert_eq!(find.input["properties"]["query"]["type"], "string");
    assert!(find.input["properties"]["pageSize"].is_object());
    assert!(find.wrap_output);
    let out = find.output.as_ref().unwrap();
    assert_eq!(out["properties"]["result"]["type"], "array");
    assert_eq!(out["properties"]["result"]["items"]["$ref"], "#/$defs/Item");
    assert_eq!(out["$defs"]["Item"]["properties"]["itemId"]["type"], "integer");

    let get = tools[1].schemas().unwrap();
    assert!(!get.wrap_output, "Result<Item, _> unwraps to the Item object");
    assert_eq!(get.output.as_ref().unwrap()["properties"]["itemId"]["type"], "integer");

    let touch = tools[2].schemas().unwrap();
    assert!(touch.output.is_none());
    assert_eq!(touch.input["properties"]["item"]["$ref"], "#/$defs/Item");
}
