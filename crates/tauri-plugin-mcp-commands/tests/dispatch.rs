//! Calls `#[mcp_tool]` handlers directly against tauri's mock runtime.

// These commands are deliberately untyped; with `schemars` on they would need `JsonSchema`.
#![cfg(not(feature = "schemars"))]

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager, Runtime, State, test::MockRuntime};
use tauri_plugin_mcp_commands::{ToolArgs, ToolDef, ToolError, collect_mcp_tools, mcp_tool};

#[derive(Default)]
struct Counter(Mutex<u32>);

/// A type alias the syntax can't see through.
type CounterState<'a> = State<'a, Counter>;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Note {
    title: String,
    body: Option<String>,
}

/// Adds `amount` to the counter.
///
/// Returns the new value.
#[mcp_tool(idempotent = false)]
#[tauri::command]
async fn counter_add(counter: State<'_, Counter>, amount: u32) -> Result<u32, String> {
    let mut c = counter.0.lock().unwrap();
    *c += amount;
    Ok(*c)
}

#[mcp_tool(read_only)]
#[tauri::command]
fn counter_get(counter: CounterState<'_>) -> u32 {
    *counter.0.lock().unwrap()
}

#[mcp_tool]
#[tauri::command(async)]
fn echo_note(page_size: u32, r#type: String, note: Note) -> (u32, String, Note) {
    (page_size, r#type, note)
}

#[mcp_tool(name = "snake_echo")]
#[tauri::command(rename_all = "snake_case")]
async fn snake(page_size: u32, maybe: Option<String>) -> Value {
    json!({ "page_size": page_size, "maybe": maybe })
}

#[mcp_tool(destructive)]
#[tauri::command]
async fn fails(reason: String) -> Result<(), String> {
    Err(reason)
}

#[mcp_tool]
#[tauri::command]
async fn app_name<R: Runtime>(app: AppHandle<R>) -> String {
    app.package_info().name.clone()
}

#[mcp_tool]
#[tauri::command]
fn missing_state(_s: State<'_, String>) {}

mod nested {
    use super::*;

    #[mcp_tool]
    #[tauri::command]
    pub async fn ping() -> &'static str {
        "pong"
    }
}

fn tools() -> Vec<ToolDef<MockRuntime>> {
    collect_mcp_tools![counter_add, counter_get, echo_note, snake, fails, app_name, missing_state, nested::ping]
}

fn app() -> AppHandle<MockRuntime> {
    let app = tauri::test::mock_app();
    app.manage(Counter::default());
    app.handle().clone()
}

async fn call(app: &AppHandle<MockRuntime>, name: &str, args: Value) -> Result<Value, ToolError> {
    let tools = tools();
    let tool = tools.iter().find(|t| t.name() == name).expect("tool exists");
    let Value::Object(args) = args else { panic!("args must be an object") };
    tool.call(app.clone(), ToolArgs::from(args)).await
}

#[tokio::test]
async fn metadata() {
    let tools = tools();
    let by_name = |n: &str| tools.iter().find(|t| t.name() == n).unwrap();

    let add = by_name("counter_add");
    assert_eq!(add.description(), Some("Adds `amount` to the counter.\n\nReturns the new value."));
    assert_eq!(add.inputs(), ["amount"]);
    assert_eq!(add.annotations().idempotent, Some(false));

    assert_eq!(by_name("counter_get").inputs(), [] as [&str; 0]);
    assert_eq!(by_name("counter_get").annotations().read_only, Some(true));
    assert_eq!(by_name("echo_note").inputs(), ["pageSize", "type", "note"]);
    assert_eq!(by_name("snake_echo").inputs(), ["page_size", "maybe"]);
    assert_eq!(by_name("fails").annotations().destructive, Some(true));
    assert_eq!(by_name("app_name").inputs(), [] as [&str; 0]);
    assert_eq!(by_name("ping").description(), None);
}

#[tokio::test]
async fn state_and_inputs() {
    let app = app();
    assert_eq!(call(&app, "counter_add", json!({ "amount": 2 })).await, Ok(json!(2)));
    assert_eq!(call(&app, "counter_add", json!({ "amount": 3 })).await, Ok(json!(5)));
    // Sync command, main thread (inline on the mock runtime), aliased `State`.
    assert_eq!(call(&app, "counter_get", json!({})).await, Ok(json!(5)));
}

#[tokio::test]
async fn camel_case_and_raw_idents_on_blocking_pool() {
    let app = app();
    let out = call(&app, "echo_note", json!({ "pageSize": 10, "type": "x", "note": { "title": "t" } })).await;
    assert_eq!(out, Ok(json!([10, "x", { "title": "t", "body": null }])));
}

#[tokio::test]
async fn snake_case_and_optional_args() {
    let app = app();
    assert_eq!(
        call(&app, "snake_echo", json!({ "page_size": 1 })).await,
        Ok(json!({ "page_size": 1, "maybe": null }))
    );
}

#[tokio::test]
async fn argument_errors() {
    let app = app();
    assert_eq!(
        call(&app, "counter_add", json!({})).await,
        Err(ToolError::InvalidArgs("missing required argument `amount`".into()))
    );
    let Err(ToolError::InvalidArgs(msg)) = call(&app, "counter_add", json!({ "amount": "two" })).await else {
        panic!("expected InvalidArgs");
    };
    assert!(msg.starts_with("invalid argument `amount`: invalid type: string \"two\""), "{msg}");
}

#[tokio::test]
async fn command_errors_and_injection() {
    let app = app();
    let err = call(&app, "fails", json!({ "reason": "nope" })).await.unwrap_err();
    assert_eq!(err, ToolError::Command(json!("nope")));
    assert_eq!(err.to_string(), "nope");

    assert!(call(&app, "app_name", json!({})).await.unwrap().is_string());
    assert_eq!(call(&app, "ping", json!({})).await, Ok(json!("pong")));

    let Err(ToolError::Internal(msg)) = call(&app, "missing_state", json!({})).await else {
        panic!("expected Internal");
    };
    assert!(msg.contains("state not managed for parameter `s`"), "{msg}");
}

#[test]
fn still_tauri_commands() {
    // The original commands must still register with Tauri unchanged.
    let _ = tauri::test::mock_builder().invoke_handler(tauri::generate_handler![
        counter_add,
        counter_get,
        echo_note,
        snake,
        fails,
        app_name,
        missing_state,
        nested::ping
    ]);
}
