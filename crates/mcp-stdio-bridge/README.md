# mcp-stdio-bridge

Bridges stdio-only MCP clients (Claude Desktop, `.mcpb` bundles) to a Tauri app using [tauri-plugin-mcp-commands](../../README.md).

```text
mcp-stdio-bridge --app-id com.example.notes
mcp-stdio-bridge --discovery /path/to/mcp-commands/mcp.json
mcp-stdio-bridge --url http://127.0.0.1:43110/mcp --token-env NOTES_MCP_TOKEN
```

It reads the app's owner-only discovery file (`<data dir>/<identifier>/mcp-commands/mcp.json`) on every reconnect, follows restarts and token rotation, and can start before the app (announcing tools with `tools/list_changed` once the app's server runs). Logs go to stderr.
