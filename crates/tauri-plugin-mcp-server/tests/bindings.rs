//! The tauri-specta bindings for the plugin's own commands.
#![cfg(feature = "tauri-specta")]

#[test]
fn exports_typescript_bindings() {
    let path = std::env::temp_dir().join(format!("mcp-server-bindings-{}.ts", std::process::id()));
    tauri_plugin_mcp_server::specta_builder()
        .export(specta_typescript::Typescript::default(), &path)
        .expect("export succeeds without bigints");
    let ts = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);

    for command in ["start", "stop", "status", "connection_configs", "respond_access", "rotate_token"] {
        assert!(ts.contains(&format!("\"plugin:mcp-server|{command}\"")), "missing {command}:\n{ts}");
    }
    for ty in ["Status", "ConnectionConfigs", "ConnectedClient", "AccessRequest"] {
        assert!(ts.contains(&format!("export type {ty} =")), "missing {ty}:\n{ts}");
    }
    assert!(!ts.contains("bigint"), "{ts}");
}
