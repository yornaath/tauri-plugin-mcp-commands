const COMMANDS: &[&str] = &["start", "stop", "status", "connection_configs", "respond_access", "rotate_token"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
}
