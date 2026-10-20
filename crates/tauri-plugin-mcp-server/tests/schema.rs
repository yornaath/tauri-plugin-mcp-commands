//! Tool schemas built from specta metadata.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::json;
use specta::Type;
use tauri::{State, test::MockRuntime};
use tauri_plugin_mcp_server::{ToolDef, collect_mcp_tools, mcp_tool};

#[derive(Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    /// Stable identifier.
    note_id: u32,
    title: String,
    tags: Vec<String>,
    body: Option<String>,
    #[serde(default)]
    pinned: bool,
    meta: Meta,
}

#[derive(Serialize, Deserialize, Type)]
pub struct Meta {
    created_at: i64,
}

#[derive(Serialize, Deserialize, Type)]
pub enum Shape {
    Circle { radius: f64 },
    Square(f64),
    Empty,
}

#[derive(Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Created { id: u32 },
    Deleted { id: u32, hard: bool },
}

/// How urgent a note is.
#[derive(Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    High,
}

#[derive(Serialize, Deserialize, Type)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

/// Serializes as a list, but also accepts a single string.
#[derive(Serialize, Deserialize, Type)]
pub struct Filters {
    #[specta(type = specta_serde::Phased<Vec<String>, OneOrMany>)]
    tags: Vec<String>,
}

#[derive(Serialize, Deserialize, Type)]
pub struct Page<T> {
    items: Vec<T>,
    total: u32,
}

#[derive(Serialize, Deserialize, Type)]
pub struct Tree {
    name: String,
    children: Vec<Tree>,
}

pub struct Repo;

#[mcp_tool]
#[tauri::command]
#[specta::specta]
fn primitives(
    flag: bool,
    name: String,
    small: u8,
    big: i64,
    ratio: f64,
    page_size: u32,
    letter: char,
) -> u32 {
    let _ = (flag, name, small, big, ratio, letter);
    page_size
}

#[mcp_tool]
#[tauri::command]
#[specta::specta]
fn optional(limit: Option<u32>) -> Option<String> {
    limit.map(|l| l.to_string())
}

/// Lists notes.
#[mcp_tool(read_only)]
#[tauri::command]
#[specta::specta]
async fn notes_list(
    _repo: State<'_, Repo>,
    priority: Priority,
    shape: Shape,
) -> Result<Vec<Note>, String> {
    let _ = (priority, shape);
    Ok(vec![])
}

#[mcp_tool]
#[tauri::command]
#[specta::specta]
async fn note_get(id: u32) -> Result<Note, String> {
    let _ = id;
    Err("not found".into())
}

#[mcp_tool]
#[tauri::command]
#[specta::specta]
async fn record(event: Event) {
    let _ = event;
}

#[mcp_tool]
#[tauri::command]
#[specta::specta]
async fn filter(filters: Filters) -> Filters {
    filters
}

#[mcp_tool]
#[tauri::command]
#[specta::specta]
async fn page(counts: HashMap<String, u32>) -> Page<Tree> {
    let _ = counts;
    Page {
        items: vec![],
        total: 0,
    }
}

#[mcp_tool]
#[tauri::command]
#[specta::specta]
async fn untyped(anything: serde_json::Value) {
    let _ = anything;
}

fn tools() -> Vec<ToolDef<MockRuntime>> {
    collect_mcp_tools![
        primitives, optional, notes_list, note_get, record, filter, page, untyped
    ]
}

fn schemas(name: &str) -> serde_json::Value {
    let tools = tools();
    let tool = tools.iter().find(|t| t.name() == name).unwrap();
    let s = tool.schemas().unwrap_or_else(|e| panic!("{name}: {e}"));
    json!({ "input": s.input, "output": s.output, "wrapOutput": s.wrap_output })
}

macro_rules! snapshot {
    ($($name:ident),*) => {
        mod snapshots {
            $(
                #[test]
                fn $name() {
                    insta::assert_json_snapshot!(super::schemas(stringify!($name)));
                }
            )*
        }
    };
}

snapshot!(
    primitives, optional, notes_list, note_get, record, filter, page, untyped
);

#[mcp_tool]
#[tauri::command]
#[specta::specta]
async fn note_create(note: Note, event: Event, shape: Shape, filters: Filters) -> Note {
    let _ = (event, shape, filters);
    note
}

/// Values serde actually produces and accepts must validate against the schemas.
#[test]
fn schemas_accept_real_values() {
    let tools: Vec<ToolDef<MockRuntime>> = collect_mcp_tools![note_create, page];
    let validator = |schema: &tauri_plugin_mcp_server::JsonObject| {
        jsonschema::draft202012::new(&serde_json::Value::Object(schema.clone()))
            .expect("valid 2020-12 schema")
    };

    let create = validator(&tools[0].schemas().unwrap().input);
    // Deserialize side: `body` (Option) and `pinned` (serde default) may be omitted,
    // and the Phased `tags` accepts a single string.
    let input = json!({
        "note": { "noteId": 1, "title": "t", "tags": [], "meta": { "created_at": 0 } },
        "event": { "kind": "deleted", "id": 3, "hard": true },
        "shape": "Empty",
        "filters": { "tags": "one" },
    });
    assert!(
        create.is_valid(&input),
        "{:?}",
        create
            .iter_errors(&input)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );
    for bad in [
        json!({ "note": { "title": "t" }, "event": { "kind": "deleted", "id": 3, "hard": true }, "shape": "Empty", "filters": { "tags": [] } }),
        json!({ "note": input["note"], "event": { "kind": "nope", "id": 3 }, "shape": "Empty", "filters": { "tags": [] } }),
        json!({ "note": input["note"], "event": input["event"], "shape": "Triangle", "filters": { "tags": [] } }),
    ] {
        assert!(!create.is_valid(&bad), "accepted {bad}");
    }

    // Serialize side: real serde output.
    let note = Note {
        note_id: 1,
        title: "t".into(),
        tags: vec!["a".into()],
        body: None,
        pinned: true,
        meta: Meta { created_at: 5 },
    };
    let out = validator(tools[0].schemas().unwrap().output.as_ref().unwrap());
    let value = serde_json::to_value(&note).unwrap();
    assert!(out.is_valid(&value), "{value}");

    let page_out = validator(tools[1].schemas().unwrap().output.as_ref().unwrap());
    let tree = Page {
        items: vec![Tree {
            name: "root".into(),
            children: vec![Tree {
                name: "leaf".into(),
                children: vec![],
            }],
        }],
        total: 1,
    };
    let value = serde_json::to_value(&tree).unwrap();
    assert!(page_out.is_valid(&value), "{value}");
    for v in [
        Shape::Circle { radius: 1.0 },
        Shape::Square(2.0),
        Shape::Empty,
    ] {
        let shape_schema = validator(&tools[0].schemas().unwrap().input);
        let input = json!({ "note": input["note"], "event": input["event"], "shape": serde_json::to_value(&v).unwrap(), "filters": { "tags": [] } });
        assert!(shape_schema.is_valid(&input), "{input}");
    }
}

#[mcp_tool]
#[tauri::command]
#[specta::specta]
async fn plant(tree: Tree) {
    let _ = tree;
}

/// Struct parameters are inlined so clients that don't resolve `$ref` see their shape.
#[test]
fn struct_inputs_are_inlined() {
    let tools: Vec<ToolDef<MockRuntime>> = collect_mcp_tools![note_create, plant];
    let input = &tools[0].schemas().unwrap().input;
    assert_eq!(input["properties"]["note"]["type"], "object");
    assert_eq!(
        input["properties"]["note"]["properties"]["meta"]["type"],
        "object"
    );
    assert!(input.get("$defs").is_none());
    // Unions whose variants share a type repeat it at the top.
    assert_eq!(input["properties"]["event"]["type"], "object");
    assert!(input["properties"]["shape"].get("type").is_none());

    // Recursive types still terminate through `$defs`.
    let input = &tools[1].schemas().unwrap().input;
    assert_eq!(input["properties"]["tree"]["type"], "object");
    assert_eq!(
        input["properties"]["tree"]["properties"]["children"]["items"]["$ref"],
        "#/$defs/Tree"
    );
    assert_eq!(input["$defs"]["Tree"]["type"], "object");
}
