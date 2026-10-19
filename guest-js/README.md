# tauri-plugin-mcp-server-api

JavaScript bindings for [tauri-plugin-mcp-server](../README.md).

```ts
import { start, stop, status, connectionConfigs, onAccessRequested, respondAccess } from 'tauri-plugin-mcp-server-api'

await start()
const { url, claudeCode } = (await connectionConfigs())!

await onAccessRequested(async ({ id, client }) => {
  await respondAccess(id, confirm(`Allow ${client.name ?? 'this client'} to use the app's tools?`))
})
```

Permissions: `mcp-server:default` (start, stop, status), plus `mcp-server:allow-connection-configs`, `mcp-server:allow-rotate-token` and `mcp-server:allow-respond-access` as needed.
