// Demo UI. Uses `window.__TAURI__` (withGlobalTauri) so it needs no bundler;
// a real app would use the `tauri-plugin-mcp-server-api` package instead.

const { invoke } = window.__TAURI__.core
const { listen } = window.__TAURI__.event

const mcp = {
  start: () => invoke('plugin:mcp-server|start'),
  stop: () => invoke('plugin:mcp-server|stop'),
  status: () => invoke('plugin:mcp-server|status'),
  connectionConfigs: () => invoke('plugin:mcp-server|connection_configs'),
  respondAccess: (id, allow) => invoke('plugin:mcp-server|respond_access', { id, allow }),
  rotateToken: () => invoke('plugin:mcp-server|rotate_token'),
}

const $ = (id) => document.getElementById(id)
const el = (tag, props = {}, ...children) => {
  const e = Object.assign(document.createElement(tag), props)
  e.append(...children)
  return e
}

let configs = null
let tab = 'claudeCode'
const TABS = {
  claudeCode: ['Claude Code', (c) => c.claudeCode],
  cursor: ['Cursor', (c) => JSON.stringify(c.cursor, null, 2)],
  vscode: ['VS Code', (c) => JSON.stringify(c.vscode, null, 2)],
  codex: ['Codex', (c) => c.codex],
  claudeDesktop: ['Claude Desktop', (c) => (c.claudeDesktop ? JSON.stringify(c.claudeDesktop, null, 2) : 'Build the bridge first: cargo build -p mcp-stdio-bridge')],
}

function renderTabs() {
  const nav = $('tabs')
  nav.replaceChildren(
    ...Object.entries(TABS).map(([key, [label]]) =>
      el('button', { type: 'button', textContent: label, ariaSelected: String(key === tab), onclick: () => { tab = key; renderTabs() } }),
    ),
  )
  $('config').textContent = configs ? TABS[tab][1](configs) : ''
}

async function refresh() {
  const s = await mcp.status()
  $('toggle').textContent = s.running ? 'Stop' : 'Start'
  $('status').textContent = s.running ? `Running at ${s.url}` : 'Stopped. Agents can only connect while the server runs.'
  $('tools').textContent = s.tools.join(', ')

  configs = s.running ? await mcp.connectionConfigs() : null
  $('connect').hidden = !configs
  renderTabs()

  $('access').hidden = s.pendingRequests.length === 0
  $('requests').replaceChildren(
    ...s.pendingRequests.map((r) =>
      el(
        'li',
        {},
        el('span', { textContent: `${r.client.name ?? r.client.userAgent ?? 'Unknown client'} wants to use this app's tools` }),
        el(
          'span',
          {},
          el('button', { type: 'button', textContent: 'Allow', onclick: () => mcp.respondAccess(r.id, true).then(refresh) }),
          ' ',
          el('button', { type: 'button', className: 'danger', textContent: 'Deny', onclick: () => mcp.respondAccess(r.id, false).then(refresh) }),
        ),
      ),
    ),
  )

  $('clients').replaceChildren(
    ...(s.clients.length
      ? s.clients.map((c) =>
          el('li', {}, el('span', { textContent: `${c.name ?? c.userAgent ?? c.key}${c.version ? ' ' + c.version : ''}` }),
            el('span', { className: 'muted small', textContent: `last seen ${new Date(c.lastSeen).toLocaleTimeString()}` })),
        )
      : [el('li', { className: 'muted', textContent: 'None yet.' })]),
  )
}

async function refreshNotes() {
  const notes = await invoke('notes_list', {})
  $('notes').replaceChildren(
    ...(notes.length
      ? notes.map((n) =>
          el('li', {}, el('span', { textContent: `#${n.id} ${n.title}${n.tags.length ? '  · ' + n.tags.join(', ') : ''}` }),
            el('button', { type: 'button', className: 'danger', textContent: 'Delete', onclick: () => invoke('notes_delete', { id: n.id }) })),
        )
      : [el('li', { className: 'muted', textContent: 'No notes. Add one, or ask an agent to.' })]),
  )
}

function banner(text) {
  const b = $('banner')
  b.textContent = text
  b.hidden = false
  clearTimeout(banner.t)
  banner.t = setTimeout(() => (b.hidden = true), 6000)
}

$('toggle').onclick = async () => {
  const s = await mcp.status()
  await (s.running ? mcp.stop() : mcp.start()).catch((e) => banner(`MCP server: ${e}`))
  refresh()
}
$('rotate').onclick = () => mcp.rotateToken().then(refresh)
$('copy').onclick = () => navigator.clipboard.writeText($('config').textContent)
$('new-note').onsubmit = async (e) => {
  e.preventDefault()
  await invoke('notes_create', { title: $('title').value }).catch((err) => banner(err))
  $('title').value = ''
}

listen('mcp://access-requested', (e) => { banner(`${e.payload.client.name ?? 'A client'} is asking for access`); refresh() })
listen('mcp://client-connected', () => refresh())
listen('notes-changed', () => refreshNotes())
listen('timer-started', (e) => banner(`⏱ ${e.payload.label}: ${e.payload.seconds}s`))
listen('timer-finished', (e) => banner(`⏰ ${e.payload.label} is done`))

Promise.all([refresh(), refreshNotes()])
  .then(() => (window.__demoReady = true))
  .catch((e) => window.__demoErrors?.push(`initial render: ${e}`))
setInterval(refresh, 5000)
