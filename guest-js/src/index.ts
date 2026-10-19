/**
 * Bindings for `tauri-plugin-mcp-server`.
 *
 * Permissions: `mcp-server:default` allows `start`, `stop` and `status`.
 * `connectionConfigs` (which contains the bearer token), `rotateToken` and
 * `respondAccess` each need their own permission:
 * `mcp-server:allow-connection-configs`, `mcp-server:allow-rotate-token`,
 * `mcp-server:allow-respond-access`.
 */

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

/** A client that has talked to the server during this run of the app. */
export interface ConnectedClient {
  key: string
  name: string | null
  version: string | null
  userAgent: string | null
  /** Unix milliseconds. */
  firstSeen: number
  lastSeen: number
}

/** A new client waiting for the app to allow or deny it. */
export interface AccessRequest {
  id: number
  client: ConnectedClient
}

export interface Status {
  running: boolean
  /** `http://127.0.0.1:<port>/mcp` while running. */
  url: string | null
  port: number | null
  /** Names of the exposed tools. */
  tools: string[]
  clients: ConnectedClient[]
  pendingRequests: AccessRequest[]
}

/** Ready-to-paste client configurations. */
export interface ConnectionConfigs {
  name: string
  url: string
  token: string
  /** `claude mcp add …` command line. */
  claudeCode: string
  /** Entry for Cursor's `mcp.json`. */
  cursor: Record<string, unknown>
  /** Entry for VS Code's `.vscode/mcp.json`. */
  vscode: Record<string, unknown>
  /** Table for Codex's `~/.codex/config.toml`. */
  codex: string
  /** Entry for `claude_desktop_config.json` via the stdio bridge, if configured. */
  claudeDesktop: Record<string, unknown> | null
}

export const EVENT_CLIENT_CONNECTED = 'mcp://client-connected'
export const EVENT_ACCESS_REQUESTED = 'mcp://access-requested'

/** Starts the server (a no-op if it is running). */
export function start(): Promise<Status> {
  return invoke('plugin:mcp-server|start')
}

/** Stops the server and closes its port. */
export function stop(): Promise<Status> {
  return invoke('plugin:mcp-server|stop')
}

export function status(): Promise<Status> {
  return invoke('plugin:mcp-server|status')
}

/** Client configurations, or `null` while the server is stopped. */
export function connectionConfigs(): Promise<ConnectionConfigs | null> {
  return invoke('plugin:mcp-server|connection_configs')
}

/**
 * Answers an access request. Resolves `false` if the request is no longer
 * pending (already answered, timed out or cancelled).
 */
export function respondAccess(id: number, allow: boolean): Promise<boolean> {
  return invoke('plugin:mcp-server|respond_access', { id, allow })
}

/** Replaces the bearer token; every client must be reconfigured. */
export function rotateToken(): Promise<void> {
  return invoke('plugin:mcp-server|rotate_token')
}

export function onClientConnected(handler: (client: ConnectedClient) => void): Promise<UnlistenFn> {
  return listen<ConnectedClient>(EVENT_CLIENT_CONNECTED, (e) => handler(e.payload))
}

/** Fires when `require_approval` is on and a new client connects. */
export function onAccessRequested(handler: (request: AccessRequest) => void): Promise<UnlistenFn> {
  return listen<AccessRequest>(EVENT_ACCESS_REQUESTED, (e) => handler(e.payload))
}
