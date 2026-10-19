# Spike findings (Phase A checkpoint)

Date: 2026-10-19. Toolchain: rustc 1.98.1 on Windows 11.

Resolved versions, all the newest on crates.io: tauri 2.12.4 (tauri-macros 2.7.3), rmcp 3.5.1, axum 0.8.9, specta =2.0.0-rc.25, specta-serde =0.0.12, tauri-specta =2.0.0-rc.25 (not used yet). `cargo tree -i specta` shows a single specta in the graph.

`cargo test --workspace` passes:
- dispatch: 7 tests
- schema: 9 tests, including validating real serde values against the schemas with `jsonschema`
- HTTP: 3 tests
- trybuild: 4 cases
- macro unit tests: 3

## Spike 1: parameter classification

**Result: works, and handles more than the brief asked for.**

**Classification is type-based, via autoref specialization** (`src/dispatch.rs`).
- The macro emits `(&&&Arg::<T>::new()).__mcp_extract(&cx, "key")` for every parameter.
- Method resolution picks the first matching impl, in this order:
  1. window types: `Window`, `Webview`, `WebviewWindow`
  2. `State<'_, T>`
  3. `AppHandle<R>`
  4. `T: DeserializeOwned`
- Because it keys on the resolved type, a type alias such as `type CounterState<'a> = State<'a, Counter>` resolves correctly. Tested.
- The brief's `inject = "…"` escape hatch isn't needed.

**Window-bound parameters fail to compile, through two layers:**
- A syntactic check on bare and `tauri::` paths gives an error pointing at the parameter type (`tests/ui/window_arg.stderr`).
- A `#[diagnostic::on_unimplemented]` trait bound catches aliased window types (`tests/ui/aliased_window.stderr`). Stable rustc can't join spans, so this error's primary span is the attribute; a secondary label points at the parameter.

**Attribute order: `#[mcp_tool]` must be the outermost attribute**, above `#[tauri::command]` and `#[specta::specta]`.
- `#[tauri::command]` consumes its own arguments (`rename_all`, `rename`, `async`) when it expands. Only an attribute above it still sees them.
- `#[mcp_tool(rename_all = …)]` and `#[mcp_tool(blocking)]` override these when the attribute can't be placed first.

**Argument names are the macro's job.** specta only applies `rename_all` when one is given; its default is not camelCase. The macro computes Tauri's IPC keys itself:
- heck `to_lower_camel_case`, or snake_case, matching `tauri-macros/src/command/wrapper.rs`
- raw identifiers are unrawed (`r#type` → `type`)

**Runtime parameter of the generated `ToolDef<R>`:**
- the command's own `R: Runtime` parameter if it has one;
- otherwise the concrete runtime if the command names `AppHandle` (Wry) or `AppHandle<X>`;
- otherwise a fresh generic `__R`, so runtime-agnostic commands also work with `MockRuntime` in tests.

**Executors**, matching Tauri's semantics:

| Command | Runs on |
|---|---|
| `async fn` | tokio |
| sync fn | main thread (`run_on_main_thread` + oneshot, 30 s timeout) |
| sync fn with `#[tauri::command(async)]` or `#[mcp_tool(blocking)]` | `spawn_blocking` |

**Return mapping** uses the same autoref scheme Tauri uses.
- `Result<T, E>` requires `E: Into<InvokeError>`, the same bound Tauri has. Errors serialize exactly as the webview would see them.
- Any other return value requires `T: Serialize`.

## Spike 2: schemas on specta rc.25

**Result: works, but not with specta-jsonschema, which was dropped.**

specta-jsonschema 0.0.3 can't produce per-tool schemas:
- It only exports a whole `Types` collection. Its per-datatype converter is a private module.
- It has no serde awareness. It defaults to external tagging and ignores renames.
- It emits the 2020-12 `prefixItems` keyword even under its default Draft 7.

**Replacement:** `src/schema.rs`, about 350 LOC, emitting JSON Schema 2020-12 (the MCP default dialect).
1. Run `fn_datatype!`, then `specta_serde::PhasesFormat`, which applies serde renames, tagging, flatten and defaults, and splits types whose shapes differ by direction.
2. `select_phase_datatype` picks the shape: Deserialize for inputs, Serialize for outputs.
3. Our walker converts the result to JSON Schema.

Details verified by snapshots (`tests/snapshots/`) and by `jsonschema` validation of real serde output (`schemas_accept_real_values`):
- **Input names and `required`:** camelCase names; `Option<T>` arguments are not required; `#[serde(default)]` fields and `Option` fields are not required on input but are required on output.
- **Enum tagging** (external, internal, unit), and unit enums collapse to `{"type":"string","enum":[…]}`.
- **specta-serde's rewrite:** after it, tags are single-variant unit enums. Any other unit variant is untagged and becomes `null`. specta's built-in `serde_json::Value` is such an enum, so "no attributes" does not mean "rewritten".
- **`Phased` types:** inputs get `X_Deserialize` in `$defs`, outputs the serialize shape.
- **References:** named types go in `$defs` and are `$ref`'d. Generic types are inlined with their arguments substituted. Recursive types terminate via `$ref`. Inline-only recursive types get a definition built from their inline body.
- **`serde_json::Value`** → `{}`, meaning any JSON.
- **Nullables** are `anyOf: [T, null]`, not `"type": [T, "null"]`. The MCP Inspector's `--strict` portability check flags type arrays: single-`type` dialects such as Gemini function declarations reject them.

**Output schema:**
- `Result<T, E>` is a named generic type in specta, with body `{ok, err}`; outputs unwrap it to `T`.
- MCP requires an object output schema. A non-object result is wrapped as `{"result": …}`, and the structured content is wrapped the same way. `ToolSchemas::wrap_output` records this.
- `()` results have no output schema.

**Without `#[specta::specta]`** the tool still works, with an untyped input schema (every key is `{}`). The `schemars` fallback is Phase B.

**Why the arg zip is safe:** specta reports injected parameters as absent (tauri's `specta` feature implements `FunctionArg` for `State`/`AppHandle`/window types as `None`), and the macro's own type-based `is_input` check agrees. The counts are compared at schema build time, and a mismatch is an error rather than a silent misalignment.

## Spike 3: main-thread dispatch

**Result on Windows: works in every mode, with no deadlocks.**

`examples/demo-app` has a `--self-test` flag that calls its own MCP server over HTTP. Debug results:

| Mode | where_sync | blocking / threadpool / async | 1 slow + 5 concurrent sync calls | webview IPC |
|---|---|---|---|---|
| `--tray-only` (no windows, `prevent_exit`) | main thread | tokio workers | 502 ms, all on main | skipped (no window) |
| `--hidden` (invisible window) | main thread | tokio workers | 502 ms, all on main | works, on main thread |
| visible window | main thread | tokio workers | 502 ms, all on main | works, on main thread |

The release build was also run (see the end of this file).

**Not verified: macOS and Linux.** macOS's main-thread model (AppKit run loop) differs most, so the self-test needs a run there:
```
cargo run -p demo-app -- --tray-only --self-test
cargo run -p demo-app -- --hidden --self-test
```

A sync command that holds the main thread longer than 30 s fails with `isError` rather than hanging the client. The timeout is not configurable yet.

## Spike 4: hand-written rmcp 3.5.1 ServerHandler

**Result: works with both protocol generations** (`tests/http.rs`).

Confirmed signatures:
- `list_tools(Option<PaginatedRequestParams>, RequestContext<RoleServer>) -> Result<ListToolsResult, ErrorData>`
- `call_tool(CallToolRequestParams, RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData>`, where `CallToolResponse::Complete(CallToolResult)` implements `From<CallToolResult>`
- `get_info() -> ServerConfig` (an alias of `InitializeResult`)
- `async fn` impls are accepted.

Transport:
- `StreamableHttpService::new(factory, Arc<LocalSessionManager>, config)` with `legacy_session_mode(false)` and `json_response(true)`. This is the same setup as rmcp's own stateless tests.
- `NeverSessionManager` also exists but is untested here.

Protocol behavior:
- **2025-11-25:** `initialize` echoes the version. Later requests need only `MCP-Protocol-Version`, with no session id. `resultType` is omitted for these peers.
- **2026-07-28:** there is no handshake. Every request carries `_meta` (`io.modelcontextprotocol/{protocolVersion,clientInfo,clientCapabilities}`) and the `MCP-Protocol-Version`/`Mcp-Method`/`Mcp-Name` headers. `resultType: "complete"` is present.
- **Errors:**
  - Tool errors, including argument errors, are `isError: true` results whose text is the error message.
  - An unknown tool is JSON-RPC error `-32602`.

Security that rmcp gives us:
- `enforce_origin_validation()` rejects any request with an `Origin` header (403). That includes `http://tauri.localhost` and `tauri://localhost`, so the app's own webview can't reach the server.
- The default `allowed_hosts` is loopback, so a foreign `Host` header gets 403 (DNS rebinding).
- `max_request_body_bytes(2 MiB)` returns 413.

**MCP Inspector 2.11.0** (`npx @modelcontextprotocol/inspector --cli … --transport http`):
- `tools/list` is correct and the `--strict` portability check reports 0 warnings, after the anyOf change above.
- `tools/call` round-trips structured content, and `Err` comes back as `isError: true`.

## Changes to the plan for Phase B

1. Docs and the example: `#[mcp_tool]` goes first, above `#[tauri::command]` and `#[specta::specta]`.
2. Replace `specta-jsonschema` with `schema.rs` in the dependency list. The supported specta range is exactly `=2.0.0-rc.25` / specta-serde `=0.0.12`.
3. The `__private::serve` spike entry point becomes the plugin's `McpServer::start()`/`stop()`. `stop()` should await the server task, so that a stopped server's port is actually closed. Add a test for this.
4. Origins: keep "reject every Origin" as the default, and make `allowed_origins(..)` opt-in.

## Open risks and to-dos

- **Ordering detection:**
  - If `#[specta::specta]` sits above `#[mcp_tool]`, the macro can't see it and silently falls back to the untyped schema. Consider a `#[mcp_tool(specta)]` override, or a lint-style note.
  - The same applies to `#[tauri::command(async)]` placed above `#[mcp_tool]`.
- **Unsupported parameter types:**
  - Tauri also injects `tauri::ipc::Channel`, `tauri::ipc::Request` and the ACL scope types through `CommandArg`. None of them is `Deserialize`, so `#[mcp_tool]` already rejects them at compile time, but only with a generic "trait bounds not satisfied" error. Give them a clear diagnostic like the window types get.
  - `&str` arguments aren't supported, since inputs require `DeserializeOwned`.
- **Unsupported return types:**
  - Non-`async` fns returning `impl Future` aren't supported.
  - `tauri::ipc::Response` raw bodies aren't supported.
- **Schema gaps:**
  - Opaque specta references map to `{}`.
  - `#[deprecated]` isn't surfaced.
  - `i64`/`u64` have no precision note.
- **Configuration:** the main-thread timeout is fixed at 30 s.
- **Platforms:** macOS and Linux verification of spike 3 is still to do.

## Release build check

`cargo build --release -p demo-app`, then `--tray-only --self-test` and `--hidden --self-test`. Every check passed, with the same thread placement as the debug build. Nothing in the plugin is gated on `debug_assertions`.

## Workspace gotcha: `common-controls-v6`

Tauri's default `common-controls-v6` feature makes Windows binaries import from comctl32 v6. That import only resolves when the binary embeds an app manifest, which `tauri-build` adds for the app but not for test binaries. With the demo app in the workspace, feature unification enabled the feature for the core crate's tests too, and they died with `0xc0000139` (STATUS_ENTRYPOINT_NOT_FOUND). The demo app now uses Tauri's defaults minus that feature. Mention this in the README's testing section: app developers who test `#[mcp_tool]` commands with `tauri::test` will hit the same thing.
