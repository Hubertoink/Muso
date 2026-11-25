use crate::ai::VisualProfile;
use crate::{ai, db, err, AppState, R};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
use tauri::{http::Response, AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

pub fn schema(c: &mut Connection) -> rusqlite::Result<()> {
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS image_folders(path TEXT PRIMARY KEY);
        CREATE TABLE IF NOT EXISTS personal_images(
            id INTEGER PRIMARY KEY AUTOINCREMENT, source TEXT NOT NULL UNIQUE, stamp TEXT NOT NULL,
            rev INTEGER NOT NULL DEFAULT 1, energy INTEGER, abstraction INTEGER, unreadable INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS image_members(folder TEXT NOT NULL, image INTEGER NOT NULL, PRIMARY KEY(folder,image));
        CREATE TABLE IF NOT EXISTS album_image_profiles(album TEXT PRIMARY KEY, evidence TEXT NOT NULL, energy INTEGER, abstraction INTEGER);
        CREATE TABLE IF NOT EXISTS album_visual_profiles(album TEXT PRIMARY KEY, evidence TEXT NOT NULL,
            energy INTEGER, abstraction INTEGER, mood TEXT, density INTEGER);")?;
    if !tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('personal_images') WHERE name='mood')",
        [],
        |row| row.get::<_, bool>(0),
    )? {
        tx.execute_batch(
            "ALTER TABLE personal_images ADD COLUMN mood TEXT;
                          ALTER TABLE personal_images ADD COLUMN density INTEGER;",
        )?;
    }
    tx.commit()
}

#[derive(Serialize)]
pub struct Inventory {
    folders: Vec<String>,
    total: usize,
    pending: usize,
    unreadable: usize,
}

fn roots(c: &Connection) -> rusqlite::Result<Vec<String>> {
    c.prepare("SELECT path FROM image_folders ORDER BY path")?
        .query_map([], |r| r.get(0))?
        .collect()
}
fn inventory(c: &Connection) -> R<Inventory> {
    let folders = roots(c).map_err(err)?;
    let (total, pending, unreadable) = c
        .query_row(
            "SELECT count(*),count(*) FILTER(WHERE (energy IS NULL OR abstraction IS NULL OR mood IS NULL OR density IS NULL) AND unreadable=0),
        count(*) FILTER(WHERE unreadable=1) FROM personal_images",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(err)?;
    Ok(Inventory {
        folders,
        total,
        pending,
        unreadable,
    })
}
#[tauri::command]
pub fn image_inventory(state: State<'_, AppState>) -> R<Inventory> {
    inventory(&*state.db.lock().map_err(err)?)
}
fn supported(path: &Path) -> bool {
    path.extension().and_then(|v| v.to_str()).is_some_and(|v| {
        ["jpg", "jpeg", "png", "webp"]
            .iter()
            .any(|s| v.eq_ignore_ascii_case(s))
    })
}
fn prune(c: &Connection) -> rusqlite::Result<()> {
    c.execute("DELETE FROM personal_images WHERE NOT EXISTS(SELECT 1 FROM image_members WHERE image=personal_images.id)",[])?;
    Ok(())
}

// Finish the filesystem walk before taking the write transaction. Keep old
// memberships when an external drive or part of the tree cannot be read.
fn scan(c: &mut Connection, folder: &str) -> R<()> {
    let mut complete = true;
    let mut files = Vec::new();
    for result in walkdir::WalkDir::new(folder).follow_links(false) {
        let entry = match result {
            Ok(v) => v,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        if !entry.file_type().is_file() || !supported(entry.path()) {
            continue;
        }
        let (Ok(meta), Ok(source)) = (entry.metadata(), entry.path().canonicalize()) else {
            complete = false;
            continue;
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|v| v.duration_since(UNIX_EPOCH).ok())
            .map(|v| v.as_nanos())
            .unwrap_or(0);
        files.push((
            source.to_string_lossy().into_owned(),
            format!("{modified}-{}", meta.len()),
        ));
    }
    let tx = c
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(err)?;
    if !tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM image_folders WHERE path=?1)",
            [folder],
            |r| r.get::<_, bool>(0),
        )
        .map_err(err)?
    {
        return Ok(());
    }
    if complete {
        tx.execute("DELETE FROM image_members WHERE folder=?1", [folder])
            .map_err(err)?;
    }
    for (source, stamp) in files {
        tx.execute("INSERT INTO personal_images(source,stamp) VALUES(?1,?2) ON CONFLICT(source) DO UPDATE SET
            stamp=excluded.stamp,rev=rev+1,energy=NULL,abstraction=NULL,mood=NULL,density=NULL,unreadable=0 WHERE stamp<>excluded.stamp",params![source,stamp]).map_err(err)?;
        tx.execute("INSERT OR IGNORE INTO image_members(folder,image) SELECT ?1,id FROM personal_images WHERE source=?2",params![folder,source]).map_err(err)?;
    }
    prune(&tx).map_err(err)?;
    tx.commit().map_err(err)?;
    if !complete {
        return Err(format!(
            "Some files in {folder} could not be read. Previous entries were kept."
        ));
    }
    Ok(())
}

async fn index(db_path: PathBuf, additions: Vec<String>, all: bool) -> R<Inventory> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut c = Connection::open(db_path).map_err(err)?;
        c.busy_timeout(std::time::Duration::from_secs(15))
            .map_err(err)?;
        for folder in &additions {
            c.execute("INSERT OR IGNORE INTO image_folders VALUES(?1)", [folder])
                .map_err(err)?;
        }
        let folders = if all {
            roots(&c).map_err(err)?
        } else {
            additions
        };
        let mut errors = Vec::new();
        for folder in folders {
            if let Err(e) = scan(&mut c, &folder) {
                errors.push(e);
            }
        }
        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        inventory(&c)
    })
    .await
    .map_err(err)?
}
#[tauri::command]
pub async fn pick_image_folders(app: AppHandle, state: State<'_, AppState>) -> R<Inventory> {
    let picked = app
        .dialog()
        .file()
        .set_title("Choose image folders")
        .blocking_pick_folders()
        .unwrap_or_default();
    let folders = picked
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    index(state.db_path.clone(), folders, false).await
}
#[tauri::command]
pub async fn rescan_image_folders(state: State<'_, AppState>) -> R<Inventory> {
    index(state.db_path.clone(), vec![], true).await
}
#[tauri::command]
pub fn remove_image_folder(state: State<'_, AppState>, folder: String) -> R<Inventory> {
    let mut c = state.db.lock().map_err(err)?;
    let tx = c
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(err)?;
    tx.execute("DELETE FROM image_folders WHERE path=?1", [&folder])
        .map_err(err)?;
    tx.execute("DELETE FROM image_members WHERE folder=?1", [&folder])
        .map_err(err)?;
    prune(&tx).map_err(err)?;
    tx.commit().map_err(err)?;
    inventory(&c)
}

fn checked_source(source: &str) -> Option<PathBuf> {
    let path = Path::new(source);
    let canonical = path.canonicalize().ok()?;
    (canonical == path && supported(path) && path.is_file()).then_some(canonical)
}
pub fn serve(app: &AppHandle, token: &str) -> Option<Response<Vec<u8>>> {
    let (id, rev) = token.split_once('-')?;
    let (id, rev) = (id.parse::<i64>().ok()?, rev.parse::<i64>().ok()?);
    let state = app.try_state::<AppState>()?;
    let source: String = state
        .db
        .lock()
        .ok()?
        .query_row(
            "SELECT source FROM personal_images WHERE id=?1 AND rev=?2",
            params![id, rev],
            |r| r.get(0),
        )
        .ok()?;
    let bytes = std::fs::read(checked_source(&source)?).ok()?;
    let mime = match image::guess_format(&bytes).ok()? {
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::WebP => "image/webp",
        _ => return None,
    };
    Response::builder()
        .header("Content-Type", mime)
        .header("X-Content-Type-Options", "nosniff")
        .header("Access-Control-Allow-Origin", "*")
        .header("Cache-Control", "no-cache")
        .body(bytes)
        .ok()
}

#[tauri::command]
pub async fn classify_next_image(state: State<'_, AppState>) -> R<Inventory> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = LOCK.lock().await;
    let (key, id, rev, source) = {
        let c = state.db.lock().map_err(err)?;
        let next=c.query_row("SELECT id,rev,source FROM personal_images WHERE (energy IS NULL OR abstraction IS NULL OR mood IS NULL OR density IS NULL) AND unreadable=0 ORDER BY id LIMIT 1",[],
            |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?))).optional().map_err(err)?;
        let Some((id, rev, source)) = next else {
            return inventory(&c);
        };
        (
            crate::openai_key(&c)?.ok_or("Add an OpenAI API key in Settings.")?,
            id,
            rev,
            source,
        )
    };
    let part = tauri::async_runtime::spawn_blocking(move || -> R<_> {
        use image::ImageDecoder;
        let path = checked_source(&source).ok_or("Image unavailable")?;
        let mut decoder = image::ImageReader::open(path)
            .map_err(err)?
            .with_guessed_format()
            .map_err(err)?
            .into_decoder()
            .map_err(err)?;
        let orientation = decoder.orientation().map_err(err)?;
        let mut img = image::DynamicImage::from_decoder(decoder)
            .map_err(err)?
            .thumbnail(768, 768);
        img.apply_orientation(orientation);
        crate::visual_context::image_part(&img)
    })
    .await
    .map_err(err)?;
    match part {
        Ok(part) => {
            let profile = ai::image_profile(&key, json!([part]), true).await?;
            state
                .db
                .lock()
                .map_err(err)?
                .execute(
                    "UPDATE personal_images SET energy=coalesce(energy,?1),abstraction=coalesce(abstraction,?2),mood=?3,density=?4 WHERE id=?5 AND rev=?6",
                    params![profile.energy, profile.abstraction, profile.mood, profile.density, id, rev],
                )
                .map_err(err)?;
        }
        Err(_) => {
            state
                .db
                .lock()
                .map_err(err)?
                .execute(
                    "UPDATE personal_images SET unreadable=1 WHERE id=?1 AND rev=?2",
                    params![id, rev],
                )
                .map_err(err)?;
        }
    }
    inventory(&*state.db.lock().map_err(err)?)
}

// Broad genres only suggest a few dimensions. Leave the rest open to keep
// those images varied until an album-specific profile is available.
fn genre_hint(name: &str) -> Option<VisualProfile> {
    let (energy, abstraction, mood, density) = match crate::genres::normalize(name).as_str() {
        "dark ambient" => (Some(0), None, Some("sombre"), None),
        "ambient" | "drone" | "space ambient" => (Some(0), None, None, Some(0)),
        "minimal music" | "minimalism" => (None, None, None, Some(0)),
        "cool jazz" | "modal jazz" => (None, None, Some("reflective"), Some(0)),
        "jazz" | "soul and r and b" | "r and b" | "rock" | "pop" => (Some(1), None, None, None),
        "blues" | "folk" | "country" | "singer songwriter" => (None, Some(0), None, None),
        "hip hop" | "rap" | "indie" => (None, Some(1), None, None),
        "trance" | "drum and bass" | "jungle" | "punk" | "metal" => (Some(2), None, None, None),
        "house" | "techno" | "dubstep" | "electro" | "garage" => (Some(1), Some(1), None, None),
        "trip hop" | "downtempo" => (Some(0), None, None, None),
        "idm" | "electronic" | "electronica" | "chip music" => (None, Some(1), None, None),
        "classical" | "klassik" | "chamber music" => (None, None, None, Some(1)),
        "comedy" | "stand up" => (None, None, Some("playful"), None),
        "audiobook" | "podcast" => (None, Some(0), None, None),
        _ => return None,
    };
    Some(VisualProfile {
        energy,
        abstraction,
        mood: mood.map(str::to_owned),
        density,
    })
}

fn offline_hint(
    assigned: Option<&crate::genres::Assigned>,
    tag: Option<&str>,
    book: &crate::genres::Book,
) -> VisualProfile {
    let genre = assigned.and_then(|a| a.genre.as_deref());
    let parent = genre
        .and_then(|name| book.genres.iter().find(|g| g.name == name))
        .and_then(|g| g.parent.as_deref());
    let names = match assigned.map(|a| a.origin) {
        Some("user" | "ai") => [genre, parent, None],
        _ => [tag, genre, parent],
    };
    names
        .into_iter()
        .flatten()
        .find_map(genre_hint)
        .unwrap_or_default()
}

async fn album_axes(state: &AppState, album_id: &str, allow_analysis: bool) -> R<VisualProfile> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = LOCK.lock().await;
    let (key, evidence) = {
        let c = state.db.lock().map_err(err)?;
        if !c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM personal_images WHERE energy IS NOT NULL)",
                [],
                |r| r.get::<_, bool>(0),
            )
            .map_err(err)?
        {
            return Ok(VisualProfile::default());
        }
        let album = db::album(&c, album_id)
            .map_err(err)?
            .ok_or("Album unavailable")?;
        let tracks: Vec<_> = db::tracks(&c, album_id)
            .map_err(err)?
            .into_iter()
            .map(|t| t.title)
            .collect();
        let book = crate::genres::Book::load(&c).map_err(err)?;
        let genre = crate::genres::Library::load(&c)
            .map_err(err)?
            .assigned(&book, album_id);
        let evidence=json!({"artist":album.artist,"album":album.title,"tag":album.genre,"genre":genre,"year":album.year,"tracks":tracks,
            "research":db::research(&c,album_id).map_err(err)?,"article":db::note(&c,album_id).map_err(err)?}).to_string();
        if let Some(profile) = c.query_row(
            "SELECT energy,abstraction,mood,density FROM album_visual_profiles WHERE album=?1 AND evidence=?2",
            params![album_id,evidence], |r| Ok(VisualProfile {
                energy:r.get(0)?, abstraction:r.get(1)?, mood:r.get(2)?, density:r.get(3)?,
            })).optional().map_err(err)? {
            return Ok(profile);
        }
        if !allow_analysis {
            return c.query_row("SELECT energy,abstraction FROM album_image_profiles WHERE album=?1 AND evidence=?2",
                params![album_id,evidence], |r| Ok(VisualProfile { energy:r.get(0)?, abstraction:r.get(1)?, ..Default::default() }))
                .optional().map_err(err).map(|cached| cached.unwrap_or_else(|| offline_hint(genre.as_ref(), album.genre.as_deref(), &book)));
        }
        (
            crate::openai_key(&c)?.ok_or("Add an OpenAI API key to match albums.")?,
            evidence,
        )
    };
    let axes = ai::image_profile(&key, json!(evidence), false).await?;
    state
        .db
        .lock()
        .map_err(err)?
        .execute(
            "INSERT OR REPLACE INTO album_visual_profiles(album,evidence,energy,abstraction,mood,density) VALUES(?1,?2,?3,?4,?5,?6)",
            params![album_id, evidence, axes.energy, axes.abstraction, axes.mood, axes.density],
        )
        .map_err(err)?;
    Ok(axes)
}

fn pick(db: &std::sync::Mutex<Connection>, profile: VisualProfile) -> R<Vec<String>> {
    let candidates = {
        let c = db.lock().map_err(err)?;
        let mut q = c
            .prepare(
                "SELECT id,rev,source,
            (CASE WHEN ?1 IS NULL THEN 0 ELSE coalesce(abs(energy-?1),3) END
             + CASE WHEN ?2 IS NULL THEN 0 ELSE coalesce(abs(abstraction-?2),3) END
             + CASE WHEN ?3 IS NULL OR mood=?3 THEN 0
                    WHEN mood IN ('reflective','serene','sombre') AND ?3 IN ('reflective','serene','sombre') THEN 2
                    WHEN mood IN ('playful','joyful') AND ?3 IN ('playful','joyful') THEN 2
                    WHEN mood IS NULL THEN 9 ELSE 8 END
             + CASE WHEN ?4 IS NULL THEN 0 ELSE 2*coalesce(abs(density-?4),3) END) AS distance
            FROM personal_images WHERE unreadable=0 ORDER BY distance,random()",
            )
            .map_err(err)?;
        let rows = q
            .query_map(
                params![
                    profile.energy,
                    profile.abstraction,
                    profile.mood,
                    profile.density
                ],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                },
            )
            .map_err(err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)?
    };
    let mut best = None;
    let mut picks = Vec::new();
    for (id, rev, source, distance) in candidates {
        if best.is_some_and(|d| d != distance) {
            break;
        }
        if checked_source(&source).is_some() {
            best = Some(distance);
            picks.push(format!("personal/{id}-{rev}"));
            if picks.len() == 5 {
                break;
            }
        }
    }
    Ok(picks)
}

#[derive(Serialize)]
pub struct Selection {
    files: Vec<String>,
    warning: Option<String>,
}
#[tauri::command]
pub async fn album_images(
    state: State<'_, AppState>,
    album_id: String,
    matching: bool,
    allow_analysis: Option<bool>,
) -> R<Selection> {
    let axes = if matching {
        album_axes(&state, &album_id, allow_analysis.unwrap_or(false)).await
    } else {
        Ok(VisualProfile::default())
    };
    let warning = axes.as_ref().err().cloned();
    let files = pick(&state.db, axes.unwrap_or_default())?;
    Ok(Selection { files, warning })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offline_selection_uses_genre_cues_and_respects_an_explicit_assignment() {
        use crate::genres::{Assigned, Book};
        let db = Connection::open_in_memory().unwrap();
        crate::genres::schema(&db).unwrap();
        db.execute("INSERT INTO genres VALUES('My jazz shelf','Jazz',0)", [])
            .unwrap();
        let book = Book::load(&db).unwrap();
        let jazz = offline_hint(None, Some("Jazz"), &book);
        assert_eq!(jazz.energy, Some(1));
        assert!(jazz.mood.is_none());
        let assigned = Assigned {
            genre: Some("My jazz shelf".into()),
            origin: "user",
            tag: None,
        };
        let overridden = offline_hint(Some(&assigned), Some("Dark Ambient"), &book);
        assert_eq!(overridden.energy, Some(1));
        assert!(overridden.mood.is_none());
        let open = offline_hint(None, Some("Unmapped tag"), &book);
        assert!(
            open.energy.is_none()
                && open.abstraction.is_none()
                && open.mood.is_none()
                && open.density.is_none()
        );
    }

    #[test]
    fn overlapping_folders_refresh_and_closest_available_taxonomy() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let abstract_file = nested.join("abstract.png");
        let quiet_file = dir.path().join("quiet.jpg");
        for file in [&abstract_file, &quiet_file] {
            image::RgbImage::new(16, 12).save(file).unwrap();
        }
        std::fs::write(dir.path().join("other.txt"), "not an image").unwrap();
        let mut c = Connection::open_in_memory().unwrap();
        schema(&mut c).unwrap();
        for root in [dir.path(), &nested] {
            let folder = root.to_str().unwrap();
            c.execute("INSERT INTO image_folders VALUES(?1)", [folder])
                .unwrap();
            scan(&mut c, folder).unwrap();
        }
        assert_eq!(inventory(&c).unwrap().pending, 2);
        c.execute("UPDATE personal_images SET energy=0, abstraction=0", [])
            .unwrap();
        c.execute(
            "UPDATE personal_images SET energy=2, abstraction=2 WHERE source LIKE '%abstract.png'",
            [],
        )
        .unwrap();
        let db = std::sync::Mutex::new(c);
        let axes = |energy, abstraction| VisualProfile {
            energy: Some(energy),
            abstraction: Some(abstraction),
            ..Default::default()
        };
        let exact = pick(&db, axes(2, 2)).unwrap();
        assert_eq!(exact.len(), 1);
        assert_eq!(pick(&db, axes(1, 2)).unwrap(), exact);
        assert_eq!(pick(&db, VisualProfile::default()).unwrap().len(), 2);
        {
            let c = db.lock().unwrap();
            c.execute("UPDATE personal_images SET mood='playful',density=2", [])
                .unwrap();
            c.execute(
                "UPDATE personal_images SET mood='reflective',density=0 WHERE energy=0",
                [],
            )
            .unwrap();
        }
        let reflective = || VisualProfile {
            mood: Some("reflective".into()),
            density: Some(0),
            ..axes(2, 2)
        };
        assert_ne!(pick(&db, reflective()).unwrap(), exact);
        db.lock()
            .unwrap()
            .execute(
                "UPDATE personal_images SET mood='reflective',energy=1,abstraction=1",
                [],
            )
            .unwrap();
        assert_eq!(
            pick(
                &db,
                VisualProfile {
                    density: Some(2),
                    ..reflective()
                }
            )
            .unwrap(),
            exact
        );
        std::fs::remove_file(abstract_file).unwrap();
        assert_ne!(pick(&db, axes(2, 2)).unwrap(), exact);
        let mut c = db.into_inner().unwrap();
        for root in [dir.path(), &nested] {
            scan(&mut c, root.to_str().unwrap()).unwrap();
        }
        assert_eq!(inventory(&c).unwrap().total, 1);
        image::RgbImage::new(128, 128).save(&quiet_file).unwrap();
        scan(&mut c, dir.path().to_str().unwrap()).unwrap();
        assert_eq!(inventory(&c).unwrap().pending, 1);
        assert!(quiet_file.is_file());
    }
}
