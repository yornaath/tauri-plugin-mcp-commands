//! A tiny notes store, exposed both as IPC commands and as MCP tools.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Emitter, Runtime, State};
use tauri_plugin_mcp_server::mcp_tool;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: u32,
    pub title: String,
    pub body: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Default)]
pub struct NotesRepo {
    notes: Mutex<Vec<Note>>,
    next_id: Mutex<u32>,
}

pub const EVENT_NOTES_CHANGED: &str = "notes-changed";

/// Lists notes, newest first. Optionally only those with `tag`.
#[mcp_tool(read_only)]
#[tauri::command]
#[specta::specta]
pub async fn notes_list(repo: State<'_, NotesRepo>, tag: Option<String>, limit: Option<u32>) -> Result<Vec<Note>, String> {
    let notes = repo.notes.lock().unwrap();
    Ok(notes
        .iter()
        .rev()
        .filter(|n| tag.as_ref().is_none_or(|t| n.tags.contains(t)))
        .take(limit.unwrap_or(u32::MAX) as usize)
        .cloned()
        .collect())
}

/// Creates a note and returns it.
#[mcp_tool(idempotent = false)]
#[tauri::command]
#[specta::specta]
pub async fn notes_create<R: Runtime>(
    app: AppHandle<R>,
    repo: State<'_, NotesRepo>,
    title: String,
    body: Option<String>,
    tags: Option<Vec<String>>,
) -> Result<Note, String> {
    if title.trim().is_empty() {
        return Err("title must not be empty".into());
    }
    let note = {
        let mut id = repo.next_id.lock().unwrap();
        *id += 1;
        let note = Note { id: *id, title, body, tags: tags.unwrap_or_default() };
        repo.notes.lock().unwrap().push(note.clone());
        note
    };
    let _ = app.emit(EVENT_NOTES_CHANGED, ());
    Ok(note)
}

/// Permanently deletes the note with `id`.
#[mcp_tool(destructive, idempotent)]
#[tauri::command]
#[specta::specta]
pub async fn notes_delete<R: Runtime>(app: AppHandle<R>, repo: State<'_, NotesRepo>, id: u32) -> Result<(), String> {
    let mut notes = repo.notes.lock().unwrap();
    let before = notes.len();
    notes.retain(|n| n.id != id);
    if notes.len() == before {
        return Err(format!("no note with id {id}"));
    }
    drop(notes);
    let _ = app.emit(EVENT_NOTES_CHANGED, ());
    Ok(())
}

/// Counts the words in `text`. A sync command, so it runs on the main thread.
#[mcp_tool(read_only)]
#[tauri::command]
#[specta::specta]
pub fn word_count(text: String) -> usize {
    text.split_whitespace().count()
}
