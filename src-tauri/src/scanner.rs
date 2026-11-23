use crate::db;
use image::{imageops::FilterType, DynamicImage};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::PictureType;
use lofty::tag::{Accessor, ItemKey};
use rusqlite::params;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use tauri::{AppHandle, Emitter};
use walkdir::WalkDir;

const AUDIO_EXTS: &[&str] = &[
    "flac", "mp3", "m4a", "mp4", "aac", "alac", "ogg", "oga", "wav", "wave", "aif", "aiff", "aifc",
];
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp"];
const PREFERRED_COVERS: &[&str] = &["cover", "folder", "front", "album", "artwork", "albumart"];

#[derive(Serialize, Clone)]
pub struct Progress {
    pub files: usize,
}

#[derive(Serialize, Clone)]
pub struct Done {
    pub albums: usize,
    pub error: Option<String>,
}

/// Stable FNV-1a hash used for album ids (must not change between builds).
pub fn fnv(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

struct TrackInfo {
    title: String,
    artist: Option<String>,
    album_artist: String,
    album: String,
    year: Option<u32>,
    genre: Option<String>,
    track_no: Option<u32>,
    disc_no: Option<u32>,
    duration: f64,
    picture: Option<Vec<u8>>,
}

fn clean(o: Option<Cow<'_, str>>) -> Option<String> {
    o.map(|c| c.trim().to_string()).filter(|s| !s.is_empty())
}

fn ext_of(p: &Path) -> Option<String> {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase())
}

fn read_track(p: &Path) -> TrackInfo {
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled".into());
    let dir_name = p
        .parent()
        .and_then(|d| d.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Unknown Album".into());

    let tf = lofty::read_from_path(p).ok();
    let duration = tf
        .as_ref()
        .map(|t| t.properties().duration().as_secs_f64())
        .unwrap_or(0.0);
    let tag = tf.as_ref().and_then(|t| t.primary_tag().or_else(|| t.first_tag()));

    let artist = tag.and_then(|t| clean(t.artist()));
    let album_artist = tag
        .and_then(|t| t.get_string(&ItemKey::AlbumArtist))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| artist.clone())
        .unwrap_or_else(|| "Unknown Artist".into());

    let picture = tag.and_then(|t| {
        let pics = t.pictures();
        pics.iter()
            .find(|p| p.pic_type() == PictureType::CoverFront)
            .or_else(|| pics.first())
            .map(|p| p.data().to_vec())
    });

    TrackInfo {
        title: tag.and_then(|t| clean(t.title())).unwrap_or(stem),
        artist,
        album_artist,
        album: tag.and_then(|t| clean(t.album())).unwrap_or(dir_name),
        year: tag.and_then(|t| t.year()),
        genre: tag.and_then(|t| clean(t.genre())),
        track_no: tag.and_then(|t| t.track()),
        disc_no: tag.and_then(|t| t.disk()),
        duration,
        picture,
    }
}

fn folder_cover(dir: &Path) -> Option<Vec<u8>> {
    let mut images: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && ext_of(p).is_some_and(|e| IMAGE_EXTS.contains(&e.as_str())))
        .collect();
    images.sort_by_key(|p| {
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        PREFERRED_COVERS
            .iter()
            .position(|c| stem == *c || stem.starts_with(c))
            .unwrap_or(PREFERRED_COVERS.len())
    });
    images.into_iter().find_map(|p| std::fs::read(p).ok())
}

pub(super) fn save_jpeg(img: &DynamicImage, max: u32, out: &Path) -> std::io::Result<()> {
    let img = if img.width() > max || img.height() > max {
        img.resize(max, max, FilterType::Triangle)
    } else {
        img.clone()
    };
    let rgb = img.to_rgb8();
    let file = std::fs::File::create(out)?;
    let mut w = std::io::BufWriter::new(file);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, 88)
        .encode_image(&rgb)
        .map_err(std::io::Error::other)?;
    std::io::Write::flush(&mut w)
}

pub fn save_artwork(bytes: &[u8], id: &str, data_dir: &Path) -> bool {
    let Ok(img) = image::load_from_memory(bytes) else { return false };
    save_jpeg(&img, 1400, &data_dir.join("art").join(format!("{id}.jpg"))).is_ok()
        && save_jpeg(&img, 560, &data_dir.join("thumb").join(format!("{id}.jpg"))).is_ok()
}

fn sort_key(artist: &str, year: Option<u32>, title: &str) -> String {
    let a = artist.to_lowercase();
    let a = a.strip_prefix("the ").unwrap_or(&a).to_string();
    format!("{a}\u{1}{:04}\u{1}{}", year.unwrap_or(0), title.to_lowercase())
}

pub fn scan(app: &AppHandle, db_path: &Path, data_dir: &Path) -> Result<usize, String> {
    let e2s = |e: rusqlite::Error| e.to_string();
    let mut conn = db::open(db_path).map_err(e2s)?;
    std::fs::create_dir_all(data_dir.join("art")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(data_dir.join("thumb")).map_err(|e| e.to_string())?;

    let folders = db::folders(&conn).map_err(e2s)?;
    let offline: Vec<String> = folders
        .iter()
        .filter(|f| !Path::new(f).is_dir())
        .cloned()
        .collect();

    let existing: HashMap<String, i64> = {
        let mut st = conn.prepare("SELECT path, mtime FROM tracks").map_err(e2s)?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(e2s)?;
        rows.filter_map(Result::ok).collect()
    };
    let mut with_art: HashSet<String> = {
        let mut st = conn.prepare("SELECT id FROM albums WHERE has_art=1").map_err(e2s)?;
        let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(e2s)?;
        rows.filter_map(Result::ok).collect()
    };
    let mut folder_tried: HashSet<String> = HashSet::new();
    let mut seen: HashSet<String> = HashSet::new();

    let tx = conn.transaction().map_err(e2s)?;
    let mut changed = false;
    let mut files = 0usize;
    for folder in folders.iter().filter(|f| !offline.contains(f)) {
        for entry in WalkDir::new(folder).follow_links(true).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if !ext_of(path).is_some_and(|e| AUDIO_EXTS.contains(&e.as_str())) {
                continue;
            }
            let path_str = path.to_string_lossy().to_string();
            if !seen.insert(path_str.clone()) {
                continue;
            }
            files += 1;
            if files % 50 == 0 {
                let _ = app.emit("scan-progress", Progress { files });
            }

            let mtime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            if existing.get(&path_str) == Some(&mtime) {
                continue;
            }

            let info = read_track(path);
            changed = true;
            let album_id = fnv(&format!(
                "{}\u{0}{}",
                info.album_artist.to_lowercase(),
                info.album.to_lowercase()
            ));
            tx.execute(
                "INSERT INTO albums(id, artist, title, year, genre, has_art, sort_key)
                 VALUES(?1, ?2, ?3, ?4, ?5, 0, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                   year = COALESCE(albums.year, excluded.year),
                   genre = COALESCE(albums.genre, excluded.genre)",
                params![
                    album_id,
                    info.album_artist,
                    info.album,
                    info.year,
                    info.genre,
                    sort_key(&info.album_artist, info.year, &info.album)
                ],
            )
            .map_err(e2s)?;
            tx.execute(
                "INSERT OR REPLACE INTO tracks(path, album_id, title, artist, track_no, disc_no, duration, mtime)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    path_str,
                    album_id,
                    info.title,
                    info.artist,
                    info.track_no,
                    info.disc_no,
                    info.duration,
                    mtime
                ],
            )
            .map_err(e2s)?;

            if !with_art.contains(&album_id) {
                let mut ok = info
                    .picture
                    .as_deref()
                    .is_some_and(|b| save_artwork(b, &album_id, data_dir));
                if !ok && folder_tried.insert(album_id.clone()) {
                    if let Some(bytes) = path.parent().and_then(folder_cover) {
                        ok = save_artwork(&bytes, &album_id, data_dir);
                    }
                }
                if ok {
                    with_art.insert(album_id.clone());
                    tx.execute("UPDATE albums SET has_art=1 WHERE id=?1", [&album_id])
                        .map_err(e2s)?;
                }
            }
        }
    }

    // Forget files that disappeared, but never touch folders that are currently offline (NAS, external SSD).
    for path in existing.keys() {
        if seen.contains(path) || offline.iter().any(|f| path.starts_with(f.as_str())) {
            continue;
        }
        tx.execute("DELETE FROM tracks WHERE path=?1", [path]).map_err(e2s)?;
        changed = true;
    }
    tx.execute(
        "DELETE FROM albums WHERE art_source IS NULL AND id NOT IN (SELECT DISTINCT album_id FROM tracks)",
        [],
    )
    .map_err(e2s)?;
    if changed { crate::releases::rebuild(&tx).map_err(e2s)?; }
    tx.commit().map_err(e2s)?;

    let _ = app.emit("scan-progress", Progress { files });
    let albums: i64 = conn
        .query_row("SELECT COUNT(*) FROM albums WHERE art_source IS NULL", [], |r| r.get(0))
        .map_err(e2s)?;
    Ok(albums as usize)
}
