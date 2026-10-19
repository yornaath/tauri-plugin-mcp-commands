# `.mcpb` template

An [MCP Bundle](https://github.com/modelcontextprotocol/mcpb) that gives Claude Desktop one-click access to your app through `mcp-stdio-bridge`.

1. Copy this folder and edit `manifest.json`: `name`, `display_name`, `description`, `author`, and the `--app-id` argument (your app's `identifier` from `tauri.conf.json`).
2. Build the bridge for the target platform and put it in `server/`:
   `cargo build --release -p mcp-stdio-bridge`, then copy `target/release/mcp-stdio-bridge` (`.exe` on Windows).
   Make one bundle per platform, or put a binary per platform in `server/` and adjust `platform_overrides`.
3. `npx @anthropic-ai/mcpb validate manifest.json`
4. `npx @anthropic-ai/mcpb pack . my-app.mcpb`

The bundle runs the bridge, which finds the running app through its discovery file. Tools show up while the app runs with its MCP server turned on.
