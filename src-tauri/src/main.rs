#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod audio;
mod covers;
mod db;
mod scanner;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::http::{Request, Response};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

struct AppState {
    db: Mutex<rusqlite::Connection>,
    db_path: PathBuf,
    data_dir: PathBuf,
    player: audio::Player,
    scanning: Arc<AtomicBool>,
    cover_search: covers::Search,
}

type R<T> = Result<T, String>;

fn err<E: ToString>(e: E) -> String {
    e.to_string()
}

#[tauri::command]
fn get_folders(state: State<'_, AppState>) -> R<Vec<String>> {
    db::folders(&*state.db.lock().map_err(err)?).map_err(err)
}

#[tauri::command]
async fn pick_folders(app: AppHandle, state: State<'_, AppState>) -> R<Vec<String>> {
    let picked = app.dialog().file().set_title("Choose your music").blocking_pick_folders();
    let conn = state.db.lock().map_err(err)?;
    for p in picked.unwrap_or_default() {
        if let Ok(path) = p.into_path() {
            db::add_folder(&conn, &path.to_string_lossy()).map_err(err)?;
        }
    }
    db::folders(&conn).map_err(err)
}

#[tauri::command]
fn remove_folder(state: State<'_, AppState>, path: String) -> R<Vec<String>> {
    let conn = state.db.lock().map_err(err)?;
    db::remove_folder(&conn, &path).map_err(err)?;
    db::folders(&conn).map_err(err)
}

#[tauri::command]
fn scan_library(app: AppHandle, state: State<'_, AppState>) -> bool {
    if state.scanning.swap(true, Ordering::SeqCst) {
        return false;
    }
    let flag = state.scanning.clone();
    let db_path = state.db_path.clone();
    let data_dir = state.data_dir.clone();
    std::thread::spawn(move || {
        let done = match scanner::scan(&app, &db_path, &data_dir) {
            Ok(albums) => scanner::Done { albums, error: None },
            Err(e) => scanner::Done { albums: 0, error: Some(e) },
        };
        flag.store(false, Ordering::SeqCst);
        let _ = app.emit("scan-done", done);
    });
    true
}

#[tauri::command]
fn get_albums(state: State<'_, AppState>) -> R<Vec<db::Album>> {
    db::albums(&*state.db.lock().map_err(err)?).map_err(err)
}

#[tauri::command]
async fn upgrade_cover(app: AppHandle, state: State<'_, AppState>, album_id: String) -> R<Option<String>> {
    let album = db::album(&*state.db.lock().map_err(err)?, &album_id).map_err(err)?;
    let Some(album) = album else { return Ok(None) };
    let Some(bytes) = state.cover_search.find(&album).await? else { return Ok(None) };
    tauri::async_runtime::spawn_blocking(move || {
        let img = covers::decode(&bytes)?;
        let state = app.state::<AppState>();
        let mut db = state.db.lock().map_err(err)?;
        covers::install(&mut db, &state.data_dir, &album.id, &img)
    }).await.map_err(err)?
}

#[tauri::command]
fn open_openai_keys() -> R<()> {
    open::that_detached("https://platform.openai.com/api-keys").map_err(err)
}

fn environment_key() -> Option<String> {
    std::env::var("OPENAI_API_KEY").ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

#[tauri::command]
fn has_environment_key() -> bool {
    environment_key().is_some()
}

#[tauri::command]
fn get_tracks(state: State<'_, AppState>, album_id: String) -> R<Vec<db::Track>> {
    db::tracks(&*state.db.lock().map_err(err)?, &album_id).map_err(err)
}

#[tauri::command]
fn play_track(state: State<'_, AppState>, path: String, playback_id: Option<u64>) {
    state.player.send(audio::Cmd::Play(path, playback_id.unwrap_or_default()));
}

#[tauri::command]
fn pause(state: State<'_, AppState>) {
    state.player.send(audio::Cmd::Pause);
}

#[tauri::command]
fn resume(state: State<'_, AppState>) {
    state.player.send(audio::Cmd::Resume);
}

#[tauri::command]
fn stop(state: State<'_, AppState>) {
    state.player.send(audio::Cmd::Stop);
}

#[tauri::command]
fn get_setting(state: State<'_, AppState>, key: String) -> R<Option<String>> {
    db::get_setting(&*state.db.lock().map_err(err)?, &key).map_err(err)
}

#[tauri::command]
fn set_setting(state: State<'_, AppState>, key: String, value: String) -> R<()> {
    db::set_setting(&*state.db.lock().map_err(err)?, &key, value.trim()).map_err(err)
}

#[tauri::command]
fn get_atmospheres(state: State<'_, AppState>, album_id: String, style: Option<String>) -> R<Vec<String>> {
    db::atmospheres(&*state.db.lock().map_err(err)?, &album_id, style.as_deref().unwrap_or("cinematic")).map_err(err)
}

#[tauri::command]
fn open_image_folder(state: State<'_, AppState>, album_id: String) -> R<()> {
    let dir = state.data_dir.join("atmos");
    if !dir.is_dir() {
        return Err("No atmosphere images have been saved yet.".into());
    }
    #[cfg(windows)]
    {
        let prefix = format!("{album_id}_");
        let entries = std::fs::read_dir(&dir).map_err(err)?;
        if let Some(image) = entries.filter_map(Result::ok).map(|entry| entry.path()).find(|path| {
            path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".jpg"))
        }) {
            std::process::Command::new("explorer.exe").arg("/select,").arg(image).spawn().map_err(err)?;
            return Ok(());
        }
    }
    #[cfg(not(windows))]
    let _ = album_id;
    open::that_detached(dir).map_err(err)
}

#[tauri::command]
async fn generate_atmospheres(app: AppHandle, state: State<'_, AppState>, album_id: String, research: Option<bool>, style: Option<String>) -> R<Vec<String>> {
    let style = style.as_deref().unwrap_or("cinematic");
    let direction = ai::style_direction(style).ok_or("Unknown atmosphere style")?;
    let (key, album, tracks) = {
        let conn = state.db.lock().map_err(err)?;
        let key = db::get_setting(&conn, "openai_key")
            .map_err(err)?
            .filter(|k| !k.is_empty())
            .or_else(environment_key)
            .ok_or("No OpenAI API key set")?;
        let album = db::album(&conn, &album_id).map_err(err)?.ok_or("Album not found")?;
        let tracks: Vec<String> = db::tracks(&conn, &album_id)
            .map_err(err)?
            .into_iter()
            .map(|t| t.title)
            .collect();
        (key, album, tracks)
    };
    let files = ai::generate(&app, &key, &album, &tracks, &state.data_dir, research.unwrap_or(false), direction).await?;
    let old = {
        let mut conn = state.db.lock().map_err(err)?;
        let tx = conn.transaction().map_err(err)?;
        let old = db::atmospheres(&tx, &album_id, style).map_err(err)?;
        db::replace_atmospheres(&tx, &album_id, style, &files).map_err(err)?;
        tx.commit().map_err(err)?;
        old
    };
    for f in old {
        let _ = std::fs::remove_file(state.data_dir.join(f));
    }
    Ok(files)
}

/// Serves cached artwork / atmospheres from the app data dir via `muso://`.
fn serve(app: &AppHandle, req: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let not_found = || Response::builder().status(404).body(Vec::new()).unwrap();
    let rel = req.uri().path().replace("%2F", "/").replace("%2f", "/");
    let rel = rel.trim_start_matches('/');
    let safe = !rel.contains("..")
        && rel
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.'));
    let image_path = rel.split_once('/').is_some_and(|(dir, file)| {
        matches!(dir, "art" | "thumb" | "atmos") && !file.contains('/') && file.ends_with(".jpg")
    });
    if !safe || !image_path {
        return not_found();
    }
    let Some(state) = app.try_state::<AppState>() else { return not_found() };
    match std::fs::read(state.data_dir.join(rel)) {
        Ok(data) => Response::builder()
            .header("Content-Type", "image/jpeg")
            .header("Cache-Control", "max-age=31536000, immutable")
            .header("Access-Control-Allow-Origin", "*")
            .body(data)
            .unwrap(),
        Err(_) => not_found(),
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .register_uri_scheme_protocol("muso", |ctx, req| serve(ctx.app_handle(), req))
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join("muso.db");
            let conn = db::open(&db_path)?;
            app.manage(AppState {
                db: Mutex::new(conn),
                db_path,
                data_dir,
                player: audio::Player::new(app.handle().clone()),
                scanning: Arc::new(AtomicBool::new(false)),
                cover_search: covers::Search::new()?,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_folders,
            pick_folders,
            remove_folder,
            scan_library,
            get_albums,
            upgrade_cover,
            open_openai_keys,
            has_environment_key,
            get_tracks,
            play_track,
            pause,
            resume,
            stop,
            get_setting,
            set_setting,
            get_atmospheres,
            open_image_folder,
            generate_atmospheres
        ])
        .run(tauri::generate_context!())
        .expect("error while running Muso");
}
