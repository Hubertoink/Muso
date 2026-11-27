#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod archive;
mod audio;
mod covers;
mod crates;
mod db;
#[cfg(target_os = "linux")]
mod file_manager;
mod genres;
mod heos;
mod paintings;
mod personal_images;
mod releases;
mod scanner;
mod album_titles;
#[cfg(any(windows, target_os = "linux"))]
mod screen;
mod spectrum;
mod style_history;
mod visual_context;

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
    cover_install: Mutex<()>,
    note_generation: tokio::sync::Mutex<()>,
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
        let done = match scanner::scan(&db_path, &data_dir, |p| { let _ = app.emit("scan-progress", p); }) {
            Ok(albums) => scanner::Done { albums, error: None },
            Err(e) => scanner::Done { albums: 0, error: Some(e) },
        };
        flag.store(false, Ordering::SeqCst);
        let _ = app.emit("scan-done", done);
    });
    true
}

#[tauri::command]
fn get_albums(state: State<'_, AppState>) -> R<Vec<db::LibraryAlbum>> {
    let mut conn = state.db.lock().map_err(err)?;
    let tx = conn.transaction().map_err(err)?;
    db::albums(&tx).map_err(err)
}

#[tauri::command]
async fn upgrade_cover(app: AppHandle, state: State<'_, AppState>, album_id: String, step: covers::Step, write_mp3_covers: Option<bool>) -> R<covers::Outcome> {
    use covers::{Outcome, Step};
    let album = db::album(&*state.db.lock().map_err(err)?, &album_id).map_err(err)?;
    let album = album.ok_or("This album is no longer in your library.")?;
    let progress = |source: &str, message: String| { let _ = app.emit("cover-search-progress", serde_json::json!({"albumId": album_id, "source": source, "message": message})); };
    let search = &state.cover_search;
    let lookup = match step {
        Step::Lookup { offer_ai } => {
            let result = search.lookup(&album, false, &progress).await;
            match result {
                Ok(covers::Lookup::Result(Outcome::ReviewCovers { .. })) if !offer_ai => return Ok(Outcome::Ambiguous),
                Ok(covers::Lookup::Result(Outcome::NoMatch | Outcome::NoFrontCover | Outcome::UnknownArtist)) | Err(_)
                    if offer_ai && openai_key(&*state.db.lock().map_err(err)?)?.is_some() =>
                    return Ok(Outcome::OfferResearch { reason: result.err() }),
                other => other?,
            }
        }
        Step::Research => {
            let key = openai_key(&*state.db.lock().map_err(err)?)?.ok_or("Add an OpenAI API key in Settings first.")?;
            progress("OpenAI", "Researching the release with OpenAI…".into());
            let tracks = db::tracks(&*state.db.lock().map_err(err)?, &album.id).map_err(err)?;
            let Some(identity) = ai::research_cover_identity(&key, &album, &tracks).await? else { return Ok(Outcome::NoMatch) };
            let candidate = db::Album { title: identity.title, artist: identity.artist, ..album.clone() };
            search.lookup(&candidate, true, &progress).await?
        }
        Step::UseRelease { location } => {
            let Some(bytes) = search.download(&location, &progress).await? else { return Ok(Outcome::NoFrontCover) };
            covers::Lookup::Image(bytes, location.origin())
        }
    };
    let (bytes, source) = match lookup {
        covers::Lookup::Result(outcome) => return Ok(outcome),
        covers::Lookup::Image(bytes, source) => (bytes, source),
    };
    tauri::async_runtime::spawn_blocking(move || {
        let img = covers::decode(&bytes)?;
        let state = app.state::<AppState>();
        let _install = state.cover_install.lock().map_err(err)?;
        let mut db = state.db.lock().map_err(err)?;
        let tracks = if write_mp3_covers == Some(true) {
            db::tracks(&db, &album.id).map_err(err)?
        } else { Vec::new() };
        let Some(file) = covers::install(&mut db, &state.data_dir, &album.id, &img, &source)? else {
            return Ok(Outcome::AlreadyLargeEnough);
        };
        drop(db);
        let tags = covers::write_mp3_covers(&tracks, &state.data_dir.join("art").join(&file));
        Ok(Outcome::Updated { file, source, tags })
    }).await.map_err(err)?
}

#[tauri::command]
fn open_openai_keys() -> R<()> {
    open::that_detached("https://platform.openai.com/api-keys").map_err(err)
}

fn environment_key() -> Option<String> {
    std::env::var("OPENAI_API_KEY").ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

fn openai_key(conn: &rusqlite::Connection) -> R<Option<String>> {
    let saved = db::get_setting(conn, "openai_key").map_err(err)?;
    Ok(saved.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()).or_else(environment_key))
}

#[tauri::command]
fn has_openai_key(state: State<'_, AppState>) -> R<bool> {
    let conn = state.db.lock().map_err(err)?;
    Ok(openai_key(&conn)?.is_some())
}

#[tauri::command]
fn has_environment_key() -> bool {
    environment_key().is_some()
}

#[tauri::command]
fn get_album_note(state: State<'_, AppState>, album_id: String) -> R<Option<String>> {
    db::note(&*state.db.lock().map_err(err)?, &album_id).map_err(err)
}

#[tauri::command]
async fn delete_album_note(state: State<'_, AppState>, album_id: String) -> R<()> {
    let _guard = state.note_generation.lock().await;
    state.db.lock().map_err(err)?.execute("DELETE FROM album_notes WHERE album_id=?1", [&album_id]).map_err(err)?;
    Ok(())
}

#[tauri::command]
async fn generate_album_note(app: AppHandle, state: State<'_, AppState>, album_id: String, research: bool, replace: Option<bool>) -> R<String> {
    let (album, tracks, key) = {
        let c = state.db.lock().map_err(err)?;
        if !replace.unwrap_or(false) {
            if let Some(note) = db::note(&c,&album_id).map_err(err)? { return Ok(note); }
        }
        (db::album(&c,&album_id).map_err(err)?.ok_or("Album not found")?,
         db::tracks(&c,&album_id).map_err(err)?,
         openai_key(&c)?.ok_or("Add an OpenAI API key in Settings first.")?)
    };
    let progress = |stage: &str| {
        let _ = app.emit("article-progress", (&album_id, stage));
    };
    let purpose = if replace.unwrap_or(false) { ContextFor::Rewrite } else { ContextFor::Article };
    album_context(&app, &state, &key, &album, &tracks, research, purpose, progress).await?
        .article.ok_or("Could not save album article".into())
}

#[derive(Clone, Copy, PartialEq)]
enum ContextFor { Article, Rewrite, Images }

async fn album_context(app: &AppHandle, state: &AppState, key: &str, album: &db::Album,
    tracks: &[db::Track], research: bool, purpose: ContextFor, progress: impl Fn(&str)) -> R<ai::AlbumContext> {
    let _guard = state.note_generation.lock().await;
    let id = &album.id;
    let mut context = {
        let c = state.db.lock().map_err(err)?;
        let book = genres::Book::load(&c).map_err(err)?;
        let genre = genres::Library::load(&c).map_err(err)?.assigned(&book, id);
        let parent_genre = genre.as_ref().and_then(|a| a.genre.as_deref())
            .and_then(|name| book.genres.iter().find(|g| g.name.eq_ignore_ascii_case(name)))
            .and_then(|g| g.parent.clone());
        ai::AlbumContext { article: db::note(&c, id).map_err(err)?, genre, parent_genre, ..Default::default() }
    };
    if purpose == ContextFor::Images && !research {
        context.research = db::research(&*state.db.lock().map_err(err)?, id).map_err(err)?.unwrap_or_default();
        if context.research.trim().is_empty() {
            let _ = app.emit("atmos-article-warning", format!("{} · No album research saved yet. The paintings can only draw on titles and credits; turn on research for more specific scenes.", album.title));
        }
        return Ok(context);
    }
    if purpose == ContextFor::Article && context.article.is_some() {
        return Ok(context);
    }
    if research {
        let cached = db::research(&*state.db.lock().map_err(err)?, id).map_err(err)?;
        // Paintings need the sourced anchors that older briefs lack.
        context.research = match cached.filter(|brief| purpose != ContextFor::Rewrite
            && (purpose != ContextFor::Images || brief.contains(ai::ANCHOR_MARK))) {
            Some(brief) => brief,
            None => {
                progress("Researching");
                let brief = ai::research_album(key, album, tracks, &context).await?;
                state.db.lock().map_err(err)?.execute("INSERT INTO album_research(album_id,brief) VALUES(?1,?2)
                    ON CONFLICT(album_id) DO UPDATE SET brief=excluded.brief", (id, &brief)).map_err(err)?;
                brief
            }
        };
    }
    if context.article.is_none() || purpose == ContextFor::Rewrite {
        progress("Writing");
        match ai::album_note(key, album, tracks, &context).await {
            Ok(article) => {
                progress("Saving");
                let c = state.db.lock().map_err(err)?;
                c.execute("INSERT INTO album_notes(album_id,body) VALUES(?1,?2)
                    ON CONFLICT(album_id) DO UPDATE SET body=excluded.body WHERE ?3",
                    rusqlite::params![id, article, purpose == ContextFor::Rewrite]).map_err(err)?;
                context.article = db::note(&c,id).map_err(err)?;
                let _ = app.emit("article-saved", id);
            }
            Err(error) if purpose == ContextFor::Images => {
                let _ = app.emit("atmos-article-warning", format!("{} · Article could not be written: {error}. Continuing with the album research.", album.title));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(context)
}

#[tauri::command]
async fn transfer_library_art(app: AppHandle, importing: bool) -> R<Option<archive::Summary>> {
    tauri::async_runtime::spawn_blocking(move || {
        let dialog = app.dialog().file().add_filter("Muso archive", &["zip"]);
        let picked = if importing { dialog.blocking_pick_file() } else {
            dialog.set_file_name("Muso collection.zip").blocking_save_file()
        };
        let Some(picked) = picked else { return Ok(None) };
        let path = picked.into_path().map_err(err)?;
        let state = app.state::<AppState>();
        let mut c = state.db.lock().map_err(err)?;
        if importing { archive::import(&mut c,&state.data_dir,&path) }
        else { archive::export(&mut c,&state.data_dir,&path) }.map(Some)
    }).await.map_err(err)?
}

#[tauri::command]
fn get_tracks(state: State<'_, AppState>, album_id: String) -> R<Vec<db::Track>> {
    db::tracks(&*state.db.lock().map_err(err)?, &album_id).map_err(err)
}

#[tauri::command]
async fn play_track(app: AppHandle, state: State<'_, AppState>, path: String, duration: Option<f64>, playback_id: Option<u64>, vinyl_lead: Option<audio::Lead>, after_playback_id: Option<u64>, last_track: Option<bool>, runout_seconds: Option<u32>, change_at_end: Option<bool>) -> R<()> {
    state.player.send(audio::Cmd::Play { path, duration: duration.and_then(|s| std::time::Duration::try_from_secs_f64(s).ok()), id: playback_id.unwrap_or_default(), lead: vinyl_lead, after: after_playback_id,
        last_track: last_track.unwrap_or(false), runout_seconds: runout_seconds.unwrap_or(0), change: change_at_end.unwrap_or(false) });
    app.state::<heos::Heos>().playback(&app, true, true).await;
    Ok(())
}

#[tauri::command]
fn set_runout(state: State<'_, AppState>, playback_id: u64, seconds: u32, change_at_end: bool) {
    state.player.send(audio::Cmd::Runout(playback_id, seconds, change_at_end));
}

#[tauri::command]
async fn pause(app: AppHandle, state: State<'_, AppState>) -> R<()> {
    state.player.send(audio::Cmd::Pause);
    app.state::<heos::Heos>().playback(&app, false, false).await;
    Ok(())
}

#[tauri::command]
fn set_spectrum_enabled(window: tauri::WebviewWindow, state: State<'_, AppState>, enabled: bool) {
    state.player.set_meter(window.label(), enabled);
}

#[tauri::command]
async fn resume(app: AppHandle, state: State<'_, AppState>) -> R<()> {
    state.player.send(audio::Cmd::Resume);
    app.state::<heos::Heos>().playback(&app, true, false).await;
    Ok(())
}

#[tauri::command]
fn stop(state: State<'_, AppState>, heos: State<'_, heos::Heos>) {
    state.player.send(audio::Cmd::Stop);
    heos.ended();
}

#[tauri::command]
fn local_volume(state: State<'_, AppState>, value: Option<u8>, save: Option<bool>) -> R<u8> {
    let bus = &state.player.bus;
    if let Some(value) = value {
        if value > 100 { return Err("Invalid volume".into()); }
        if save == Some(true) {
            db::set_setting(&*state.db.lock().map_err(err)?, "local_volume", &value.to_string()).map_err(err)?;
        }
        bus.set_local_volume(value);
    }
    Ok(bus.local_volume())
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
fn painting_credit(state: State<'_, AppState>, album_id: String, style: String, file: String) -> R<Option<paintings::Credit>> {
    paintings::credit(&*state.db.lock().map_err(err)?, &album_id, &style, &file).map_err(err)
}

#[tauri::command]
fn delete_atmosphere(state: State<'_, AppState>, album_id: String, style: String, file: String) -> R<()> {
    let mut conn = state.db.lock().map_err(err)?;
    paintings::delete(&mut conn, &state.data_dir, &album_id, &style, &file)
}

#[tauri::command]
async fn open_image_folder(state: State<'_, AppState>, album_id: String) -> R<()> {
    let dir = state.data_dir.join("atmos");
    std::fs::create_dir_all(&dir).map_err(err)?;
    #[cfg(windows)]
    {
        let selected = {
            let conn = state.db.lock().map_err(err)?;
            let mut query = conn.prepare("SELECT file FROM atmosphere_variants WHERE album_id=?1
                UNION ALL SELECT file FROM atmospheres WHERE album_id=?1").map_err(err)?;
            let files = query.query_map([album_id], |r| r.get::<_, String>(0)).map_err(err)?;
            let image = files.filter_map(Result::ok).map(|f| state.data_dir.join(f))
                .find(|p| p.parent() == Some(dir.as_path()) && p.is_file());
            image
        };
        if let Some(image) = selected {
            if tauri_plugin_opener::reveal_item_in_dir(image).is_ok() {
                return Ok(());
            }
        }
    }
    #[cfg(not(windows))]
    let _ = album_id;
    #[cfg(target_os = "linux")]
    return tauri::async_runtime::spawn_blocking(move || file_manager::show_folder(&dir)).await.map_err(err)?;
    #[cfg(not(target_os = "linux"))]
    open::that_detached(dir).map_err(err)
}

#[tauri::command]
async fn generate_atmospheres(app: AppHandle, state: State<'_, AppState>, album_id: String, research: Option<bool>, style: Option<String>, count: Option<usize>, allow_repaint: Option<bool>, artist_personas: Option<bool>, artist_persona: Option<String>, only_if_empty: Option<bool>, image_settings: Option<ai::ImageSettings>) -> R<Vec<String>> {
    let count = count.unwrap_or(3);
    if !(3..=5).contains(&count) { return Err("Choose 3 to 5 images.".into()); }
    let only_if_empty = only_if_empty.unwrap_or(false);
    // A process-owned lock also protects against a second running instance before the first image is saved.
    let locks = state.data_dir.join("generation-locks");
    std::fs::create_dir_all(&locks).map_err(err)?;
    let lock = std::fs::File::options().read(true).write(true).create(true).truncate(false)
        .open(locks.join(scanner::fnv(&album_id))).map_err(err)?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) if only_if_empty => return Ok(Vec::new()),
        Err(e) => return Err(format!("Could not start atmosphere generation: {e}")),
    }
    let style = style.as_deref().unwrap_or("cinematic");
    let persona = ai::resolve_persona(artist_persona.as_deref(), artist_personas.unwrap_or(false))?;
    let direction = if style == "auto" { None } else {
        Some(ai::preset(style).ok_or("Unknown atmosphere style")?)
    };
    let (key, album, tracks, references) = {
        let conn = state.db.lock().map_err(err)?;
        if only_if_empty && db::has_atmospheres(&conn, &album_id).map_err(err)? { return Ok(Vec::new()); }
        let key = openai_key(&conn)?.ok_or("No OpenAI API key set")?;
        let album = db::album(&conn, &album_id).map_err(err)?.ok_or("Album not found")?;
        let tracks = db::tracks(&conn, &album_id).map_err(err)?;
        let references = visual_context::References::load(&conn, &album).map_err(err)?;
        (key, album, tracks, references)
    };
    let context = album_context(&app, &state, &key, &album, &tracks, research.unwrap_or(false), ContextFor::Images, |stage| {
        let message = match stage { "Researching" => "Researching the album", "Writing" => "Writing the album article", _ => "Saving the album article" };
        let _ = app.emit("atmos-progress", ai::AtmosProgress { album_id: album_id.clone(), step: 0, total: count + 1, message: message.into() });
    }).await?;
    ai::generate(&key, &album, &tracks, &state.data_dir, &context, references, direction, count, allow_repaint.unwrap_or(false), persona, image_settings.unwrap_or_default(), |step, message| {
        let _ = app.emit("atmos-progress", ai::AtmosProgress { album_id: album_id.clone(), step, total: count+1, message: message.into() });
    }, |files, name, technique, prompt| {
        let mut conn = state.db.lock().map_err(err)?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(err)?;
        let saved = paintings::append_generated(&tx, &album_id, style, files, name, technique, prompt, only_if_empty)?;
        tx.commit().map_err(err)?;
        let _ = app.emit("atmos-image-ready", serde_json::json!({
            "albumId": album_id, "style": style, "files": saved, "firstInSeries": files.len() == 1
        }));
        Ok(())
    }).await?;
    db::atmospheres(&*state.db.lock().map_err(err)?, &album_id, style).map_err(err)
}

/// Serves cached artwork / atmospheres from the app data dir via `muso://`.
fn serve(app: &AppHandle, req: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let not_found = || Response::builder().status(404).body(Vec::new()).unwrap();
    if let Some(token) = req.uri().path().strip_prefix("/personal/") {
        return personal_images::serve(app, token).unwrap_or_else(not_found);
    }
    let Some(rel) = paintings::image_path(req.uri().path()) else { return not_found() };
    let Some(state) = app.try_state::<AppState>() else { return not_found() };
    match std::fs::read(state.data_dir.join(rel.as_ref())) {
        Ok(data) => Response::builder()
            .header("Content-Type", "image/jpeg")
            .header("Cache-Control", "max-age=31536000, immutable")
            .header("Access-Control-Allow-Origin", "*")
            .body(data)
            .unwrap(),
        Err(_) => not_found(),
    }
}

#[tauri::command]
async fn set_presenter(app: AppHandle, display: Option<(i32, i32)>) -> R<()> {
    let existing = app.get_webview_window("presenter");
    let Some((x, y)) = display else {
        if let Some(window) = existing { window.close().map_err(err)?; }
        return Ok(());
    };
    let main = app.get_webview_window("main").ok_or("Main window unavailable")?;
    if !main.available_monitors().map_err(err)?.iter().any(|m| m.position().x == x && m.position().y == y) {
        return Err("Display disconnected. Choose an available display.".into());
    }
    let window = match existing {
        Some(window) => window,
        None => {
            let window = tauri::WebviewWindowBuilder::new(&app, "presenter", tauri::WebviewUrl::App("presenter.html".into()))
                .title("Muso TV").visible(false).focused(false)
                .focusable(!cfg!(target_os = "linux")).decorations(false).build().map_err(err)?;
            #[cfg(any(windows, target_os = "linux"))]
            {
                let presenter = window.clone();
                app.run_on_main_thread(move || {
                    if let Err(error) = screen::watch(presenter) { eprintln!("TV screen idle inhibition: {error}"); }
                }).map_err(err)?;
            }
            window
        }
    };
    window.set_fullscreen_on_monitor(tauri::PhysicalPosition::new(x as f64, y as f64)).map_err(err)?;
    window.show().map_err(err)?;
    main.set_focus().map_err(err)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .register_uri_scheme_protocol("muso", |ctx, req| serve(ctx.app_handle(), req))
        .setup(|app| {
            app.manage(heos::Heos::default());
            #[cfg(any(windows, target_os = "linux"))]
            if let Some(window) = app.get_webview_window("main") {
                #[cfg(target_os = "linux")]
                screen::focus_on_touch(&window)?;
                screen::watch(window)?;
            }
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join("muso.db");
            let mut conn = db::open(&db_path)?;
            if let Err(e) = paintings::rename_legacy(&mut conn, &data_dir) {
                eprintln!("Could not rename saved paintings: {e}");
            }
            let player = audio::Player::new(app.handle().clone());
            if let Some(value) = db::get_setting(&conn, "local_volume")?.and_then(|value| value.parse().ok()) {
                player.bus.set_local_volume(value);
            }
            app.manage(AppState {
                db: Mutex::new(conn),
                db_path,
                data_dir,
                player,
                scanning: Arc::new(AtomicBool::new(false)),
                cover_search: covers::Search::new()?,
                cover_install: Mutex::new(()),
                note_generation: tokio::sync::Mutex::new(()),
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                if let Some(state) = window.try_state::<AppState>() { state.player.set_meter(window.label(), false); }
                match window.label() {
                    "main" => if let Some(tv) = window.app_handle().get_webview_window("presenter") { let _ = tv.close(); },
                    "presenter" => { let _ = window.emit_to("main", "presenter-closed", ()); }
                    _ => {}
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            heos::heos_rooms,
            heos::heos_output,
            heos::heos_group,
            heos::heos_levels,
            heos::heos_set_level,
            local_volume,
            set_presenter,
            personal_images::image_inventory,
            personal_images::pick_image_folders,
            personal_images::rescan_image_folders,
            personal_images::remove_image_folder,
            personal_images::classify_next_image,
            personal_images::album_images,
            get_folders,
            pick_folders,
            remove_folder,
            scan_library,
            get_albums,
            crates::library_crates,
            genres::genre_state,
            genres::genre_inventory,
            genres::genre_save,
            genres::set_album_genre,
            genres::genre_suggestions,
            genres::genre_review,
            genres::genre_propose,
            genres::genre_classify,
            genres::genre_cancel,
            get_album_note,
            generate_album_note,
            delete_album_note,
            transfer_library_art,
            upgrade_cover,
            open_openai_keys,
            has_environment_key,
            has_openai_key,
            get_tracks,
            play_track,
            set_runout,
            set_spectrum_enabled,
            pause,
            resume,
            stop,
            get_setting,
            set_setting,
            get_atmospheres,
            painting_credit,
            delete_atmosphere,
            open_image_folder,
            generate_atmospheres
        ])
        .run(tauri::generate_context!())
        .expect("error while running Muso");
}
