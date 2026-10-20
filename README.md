# tauri-plugin-mcp-commands

Turn a Tauri 2 app into an **MCP server**, so your users can connect their own AI agents (Claude Code, Claude Desktop, Cursor, VS Code, Codex…) to the running app. Agents call the commands you choose as MCP tools.

Unlike the existing Tauri MCP projects, which are developer debugging tools that refuse to run in release builds, this plugin is meant to **ship in release builds**:

- **Explicit.** Only the commands you list are exposed. They keep working as normal IPC commands.
- **Typed.** Tool input and output schemas come from your Rust types, via specta or schemars. You write no JSON Schema.
- **Local and authenticated.** The server binds `127.0.0.1` only, requires a bearer token and rejects browsers. It is off until your app starts it, and per-client approval is optional.
- **Any client.** HTTP clients connect directly. stdio-only clients (Claude Desktop) go through a bundled bridge or a one-click `.mcpb` bundle.

```rust
/// Lists notes, newest first. Optionally only those with `tag`.
#[mcp_tool(read_only)]
#[tauri::command]
#[specta::specta]
async fn notes_list(repo: State<'_, NotesRepo>, tag: Option<String>, limit: Option<u32>) -> Result<Vec<Note>, String> { … }
```

becomes this tool:

```json
{
  "name": "notes_list",
  "description": "Lists notes, newest first. Optionally only those with `tag`.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "tag": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
      "limit": { "anyOf": [{ "type": "integer", "minimum": 0, "maximum": 4294967295 }, { "type": "null" }] }
    }
  },
  "outputSchema": { "type": "object", "properties": { "result": { "type": "array", "items": { "$ref": "#/$defs/Note" } } }, "required": ["result"], "$defs": { "Note": { … } } },
  "annotations": { "readOnlyHint": true }
}
```

## Quick start

```toml
# src-tauri/Cargo.toml
[dependencies]
tauri-plugin-mcp-commands = "0.1"
specta = { version = "=2.0.0-rc.25", features = ["derive", "function"] }
```

```rust
use tauri_plugin_mcp_commands::{collect_mcp_tools, mcp_tool};

/// Creates a note and returns it.
#[mcp_tool(idempotent = false)]
#[tauri::command]
#[specta::specta]
async fn notes_create<R: Runtime>(app: AppHandle<R>, repo: State<'_, NotesRepo>, title: String) -> Result<Note, String> { … }

/// Permanently deletes the note with `id`.
#[mcp_tool(destructive, idempotent)]
#[tauri::command]
#[specta::specta]
async fn notes_delete(repo: State<'_, NotesRepo>, id: u32) -> Result<(), String> { … }

fn main() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_mcp_commands::Builder::new()
                .tools(collect_mcp_tools![notes_list, notes_create, notes_delete])
                .preferred_port(43110)
                .build(),
        )
        .manage(NotesRepo::default())
        .invoke_handler(tauri::generate_handler![notes_list, notes_create, notes_delete])
        .run(tauri::generate_context!())
        .unwrap();
}
```

The server is **off by default**. Start it when the user opts in, from Rust:

```rust
let server = app.state::<tauri_plugin_mcp_commands::McpServer<tauri::Wry>>();
server.start().await?;
let configs = server.connection_configs().unwrap(); // ready-to-paste client configs
```

or from the webview, with the [`tauri-plugin-mcp-commands-api`](guest-js) package:

```ts
import { start, connectionConfigs } from 'tauri-plugin-mcp-commands-api'
await start()
const { claudeCode } = (await connectionConfigs())!
```

Add the permissions you use to your capability file:

```json
"permissions": ["mcp-server:default", "mcp-server:allow-connection-configs"]
```

`mcp-server:default` allows `start`, `stop` and `status`. `allow-connection-configs` exposes the bearer token to the webview. `allow-rotate-token` and `allow-respond-access` are also separate.

[`examples/demo-app`](examples/demo-app) is a complete app with a settings panel: `cargo run -p demo-app`.

## `#[mcp_tool]`

Put it **above** `#[tauri::command]` and `#[specta::specta]`. It needs to read the command's `rename_all`, `rename` and `async` options, which `#[tauri::command]` consumes when it expands.

| Argument | Effect |
|---|---|
| `read_only`, `destructive`, `idempotent`, `open_world` | MCP tool annotations (each optionally `= false`). |
| `title = "…"` | Human-readable title. |
| `name = "…"` | Tool name. The default is the command name (or its `rename`). |
| `blocking` | Run a sync command on the blocking thread pool instead of the main thread. |
| `rename_all = "snake_case"` | Argument casing. Only needed if the attribute can't go above `#[tauri::command]`. |
| `specta` | Use specta schemas even if `#[specta::specta]` isn't visible below the attribute. |

The doc comment becomes the tool description. Parameters are sorted by type, so aliases work:

- `State<'_, T>` is injected from `app.state::<T>()`.
- `AppHandle` is injected as a clone of the app handle.
- Everything else is an MCP input. Inputs use the IPC argument names: camelCase by default (`page_size` → `pageSize`), or snake_case with `#[tauri::command(rename_all = "snake_case")]`.

Some parameters fail to compile, with an explanation, because an agent call has no calling window or IPC request:

- `Window`, `Webview`, `WebviewWindow`
- `Channel`, `Request`, `CommandScope`, `GlobalScope`

Commands run the way Tauri runs them:
- `async fn` runs on the async runtime.
- A sync fn runs on the **main thread**.
- A sync fn with `#[tauri::command(async)]` or `#[mcp_tool(blocking)]` runs on the blocking pool.

Main-thread calls work with every window hidden or closed (tray-only apps). A call waiting more than 30 s for the main thread fails.

Results map to MCP like this:
- `Ok(v)` or a plain `v` becomes `structuredContent`, plus a JSON text block.
- `Err(e)` becomes a result with `isError: true` whose text is `e`, serialized the way the webview would receive it.
- A `Result<T, E>` output schema describes `T`.
- MCP requires object outputs, so other results (numbers, lists…) are wrapped as `{ "result": … }`.

Collect tools with `collect_mcp_tools![a, module::b]`. Generic commands need no turbofish.

### Schemas

| Source | When | Requirement |
|---|---|---|
| **specta** (default feature) | the command has `#[specta::specta]` or `#[mcp_tool(specta)]` | inputs and result implement `specta::Type` |
| **schemars** (feature `schemars`) | other commands | inputs and result implement `schemars::JsonSchema` |
| untyped | other commands, without `schemars` | none: every input is `{}` |

Schemas are JSON Schema 2020-12. With specta they follow your serde attributes: renames, enum tagging, `default`, flattening, and `specta_serde::Phased` types that serialize and deserialize differently. Tool schemas therefore match the TypeScript bindings tauri-specta generates.

### Manual tools

For tools that aren't a command:

```rust
use tauri_plugin_mcp_commands::ToolDef;

let timer = ToolDef::manual(
    "start_timer",
    json!({ "type": "object", "properties": { "seconds": { "type": "integer", "minimum": 1 } }, "required": ["seconds"] }),
    |app, args| async move {
        let seconds = args["seconds"].as_u64().ok_or("`seconds` must be an integer")?;
        start_timer(&app, seconds);
        Ok(json!({ "started": true }))
    },
)
.with_description("Starts a countdown timer in the app.");

Builder::new().tools(collect_mcp_tools![…]).tool(timer)
```

## Runtime control

`app.state::<McpServer<R>>()`:

| Method | |
|---|---|
| `start().await` | Starts the server. A no-op if it's running. Writes the discovery file. |
| `stop().await` | Stops the server. On return, the port is closed and the discovery file removed. |
| `status()` | `running`, `url`, `port`, `tools`, connected `clients`, `pendingRequests`. |
| `connection_configs()` | Snippets for Claude Code, Cursor, VS Code, Codex and Claude Desktop. |
| `rotate_token()` | New bearer token. Clients must be reconfigured. |
| `respond_access(id, allow)` | Answers an access request (see below). |

Builder options:

| Option | Default | |
|---|---|---|
| `server_info(name, version)` | the app's | Reported to clients. |
| `preferred_port(p)` | random port | If `p` and the next 10 ports are taken, a random port is used. |
| `autostart(bool)` | `false` | Start when the app starts. |
| `require_approval(bool)` | `false` | Ask before serving each new client. |
| `approval_timeout(d)` | 120 s | A request with no answer by then is denied. |
| `data_dir(path)` | `<app_data_dir>/mcp-server` | Where the token and discovery file live. |
| `bridge_path(path)` | none | Used to render the Claude Desktop config. |

Events:

| Event | Payload |
|---|---|
| `mcp://client-connected` | a `ConnectedClient` |
| `mcp://access-requested` | `{ id, client }`. Answer it with `respond_access(id, allow)`. Concurrent requests from the same client share one prompt. |

### tauri-specta bindings

With the `specta` feature (default), the plugin's commands carry `#[specta::specta]` and its types implement `specta::Type`. The `tauri-specta` feature adds `specta_builder()`, which you can export instead of using the `guest-js` package:

```rust
tauri_plugin_mcp_commands::specta_builder()
    .export(specta_typescript::Typescript::default(), "../src/bindings/mcp-server.ts")?;
```

## Connecting agents

`connection_configs()` renders these for you.

**Claude Code**
```sh
claude mcp add --transport http my-app http://127.0.0.1:43110/mcp --header "Authorization: Bearer <token>"
```

**Cursor** (`~/.cursor/mcp.json`) and **VS Code** (`.vscode/mcp.json`) use a URL plus an `Authorization` header. **Codex** (`~/.codex/config.toml`):
```toml
[mcp_servers.my-app]
url = "http://127.0.0.1:43110/mcp"
http_headers = { "Authorization" = "Bearer <token>" }
```

**Claude Desktop** speaks only stdio to local servers, so use the bridge:
```json
{ "mcpServers": { "my-app": { "command": "/path/to/mcp-stdio-bridge", "args": ["--app-id", "com.example.my-app"] } } }
```

### The stdio bridge

`mcp-stdio-bridge` ([crates/mcp-stdio-bridge](crates/mcp-stdio-bridge)) is a small generic binary:

- **Finding the app.** It locates the app's discovery file from its identifier. `--discovery <path>` and `--url … --token-env VAR` also work.
- **Following the app.** It re-reads the discovery file on every reconnect, so restarts, port changes and token rotation don't break it.
- **Starting early.** It can start before the app. Until then it serves no tools, then sends `tools/list_changed` once the server runs.

Ship it with your app as a [sidecar](https://v2.tauri.app/develop/sidecar/): build it for each target, add it to `bundle.externalBin`, and pass its installed path to `Builder::bridge_path`. Tauri installs sidecars next to the app binary; the demo shows the lookup.

### One-click install: `.mcpb`

[`templates/mcpb`](templates/mcpb) holds a manifest for an [MCP Bundle](https://github.com/modelcontextprotocol/mcpb) that wraps the bridge:

```sh
cp -r templates/mcpb my-app-mcpb && cd my-app-mcpb
# edit manifest.json: name, display_name, description, author, and --app-id
cp ../target/release/mcp-stdio-bridge server/   # .exe on Windows; one bundle per platform
npx @anthropic-ai/mcpb validate manifest.json
npx @anthropic-ai/mcpb pack . my-app.mcpb
```

## Security model

- **Loopback only.** The listener binds `127.0.0.1` and nothing else; no option widens it. Remote access and OAuth are out of scope.
- **Bearer token.** 32 random bytes, created on first start and persisted, compared in constant time. A missing or wrong token gets 401.
- **No browsers.** Any request with an `Origin` header, or a cross-site `Sec-Fetch-Site`, gets 403. That includes the app's own webview. A foreign `Host` header gets 403 too, which blocks DNS rebinding.
- **Request limits.** Bodies over 2 MiB get 413.
- **Owner-only files.** The data dir, the `token` file and the `mcp.json` discovery file are owner-only: mode 0700/0600 on Unix, and a protected DACL granting only the current user on Windows. Writes are atomic.
- **Off by default**, with optional per-client approval.

Client names used for approval are self-reported, so the approval prompt is a UX safeguard, not a security boundary. The token is the boundary. Other processes running as the same OS user can read the token file, just as they could read any of the user's data.

## Testing your tools

`ToolDef::call` runs a tool directly, and the plugin works on `tauri::test`'s mock runtime. See [crates/tauri-plugin-mcp-commands/tests](crates/tauri-plugin-mcp-commands/tests).

On Windows, test binaries crash with `STATUS_ENTRYPOINT_NOT_FOUND` (`0xc0000139`) when Tauri's default `common-controls-v6` feature is enabled. That feature needs an app manifest, which test binaries don't get. Disable it, or use a separate crate for tests (see [examples/demo-app/Cargo.toml](examples/demo-app/Cargo.toml)).

## Supported versions

| | |
|---|---|
| Tauri | 2.x (tested with 2.12.4) |
| rmcp | 3.5 |
| MCP protocol | 2026-07-28 (stateless) and the handshake versions back to 2024-11-05; tested with 2026-07-28 and 2025-11-25 |
| specta | `=2.0.0-rc.25`, specta-serde `=0.0.12` (pinned while in RC) |
| schemars | 1.x |
| Rust | 1.90+ (Tauri 2.12's minimum) |

Desktop only (Windows, macOS, Linux). Main-thread dispatch has been verified on Windows; please report results on macOS and Linux (`cargo run -p demo-app -- --tray-only --self-test`).

## Not in v1

- MCP resources and prompts. The registry leaves room for them.
- Tauri events as MCP notifications.
- Remote access and OAuth.
- Mobile.
- Persisting client approvals across restarts.
- Tauri 3 (currently alpha).

## Repository layout

```
crates/tauri-plugin-mcp-commands/         registry, MCP server, HTTP transport, auth, discovery, plugin
crates/tauri-plugin-mcp-commands-macros/  #[mcp_tool], collect_mcp_tools!
crates/mcp-stdio-bridge/                stdio <-> HTTP bridge binary
guest-js/                               JS bindings
examples/demo-app/                      example app (cargo run -p demo-app)
templates/mcpb/                         .mcpb manifest template
docs/spikes.md                          design notes from the initial spikes
```

## License

MIT OR Apache-2.0.
