use crate::domain::import_state::{LibraryAlbumPage, LibraryImagePage};
use crate::services::library_service;
use crate::state::AppState;
use tauri::State;
use uuid::Uuid;

#[tauri::command]
pub async fn get_library_albums(
    state: State<'_, AppState>,
    offset: u32,
    limit: u32,
) -> Result<LibraryAlbumPage, String> {
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = library_service::list_library_albums(&client, offset, limit)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn get_library_images(
    state: State<'_, AppState>,
    album_id: String,
    offset: u32,
    limit: u32,
) -> Result<LibraryImagePage, String> {
    let album_id = Uuid::parse_str(&album_id).map_err(|e| format!("invalid UUID: {e}"))?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = library_service::list_library_images(&client, album_id, offset, limit)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}
