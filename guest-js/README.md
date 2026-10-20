# tauri-plugin-mcp-commands-api

JavaScript bindings for [tauri-plugin-mcp-commands](../README.md).

```ts
import { start, stop, status, connectionConfigs, onAccessRequested, respondAccess } from 'tauri-plugin-mcp-commands-api'

await start()
const { url, claudeCode } = (await connectionConfigs())!

await onAccessRequested(async ({ id, client }) => {
  await respondAccess(id, confirm(`Allow ${client.name ?? 'this client'} to use the app's tools?`))
})
```

Permissions: `mcp-commands:default` (start, stop, status), plus `mcp-commands:allow-connection-configs`, `mcp-commands:allow-rotate-token` and `mcp-commands:allow-respond-access` as needed.
