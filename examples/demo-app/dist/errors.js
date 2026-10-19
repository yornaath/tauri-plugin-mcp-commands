// Records page errors for the demo's --self-test. A classic script that runs
// before app.js, so it also catches app.js failing to load or parse.
window.__demoErrors = []
window.addEventListener('error', (e) => window.__demoErrors.push(`${e.message} (${e.filename}:${e.lineno})`), true)
window.addEventListener('unhandledrejection', (e) => window.__demoErrors.push(`unhandled rejection: ${e.reason}`))
