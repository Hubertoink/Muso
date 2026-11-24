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
use walkdir::WalkDir;

const AUDIO_EXTS: &[&str] = &[
    "flac", "mp3", "m4a", "aac", "alac", "ogg", "oga", "wav", "wave", "aif", "aiff", "aifc",
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
    has_album_artist: bool,
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

    let artist = tag.and_then(|t| clean(t.artist())).or_else(|| {
        tf.as_ref()?.tags().iter().find_map(|t| clean(t.artist()))
    });
    let tagged_album_artist = tag
        .and_then(|t| t.get_string(&ItemKey::AlbumArtist))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| tf.as_ref()?.tags().iter().find_map(|t|
            t.get_string(&ItemKey::AlbumArtist).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)));
    let has_album_artist = tagged_album_artist.is_some();
    let album_artist = tagged_album_artist.or_else(|| artist.clone())
        .unwrap_or_else(|| "Unknown Artist".into());

    let picture = tag.and_then(|t| {
        let pics = t.pictures();
        pics.iter()
            .find(|p| p.pic_type() == PictureType::CoverFront)
            .or_else(|| pics.first())
            .map(|p| p.data().to_vec())
    });

    let mut info = TrackInfo {
        title: tag.and_then(|t| clean(t.title())).unwrap_or(stem),
        artist,
        album_artist,
        has_album_artist,
        album: tag.and_then(|t| clean(t.album())).unwrap_or(dir_name),
        year: tag.and_then(|t| t.year()),
        genre: tag.and_then(|t| clean(t.genre())),
        track_no: tag.and_then(|t| t.track()),
        disc_no: tag.and_then(|t| t.disk()),
        duration,
        picture,
    };
    if info.album_artist == "Unknown Artist" && ext_of(p).as_deref() == Some("mp3") {
        recover_mp3_tags(p, &mut info);
    }
    info
}

// Some ID3 tags contain a malformed frame alongside usable artist and album fields.
fn recover_mp3_tags(path: &Path, info: &mut TrackInfo) -> Option<()> {
    use symphonia::core::{io::MediaSourceStream, meta::StandardTagKey as Key, probe::Hint};
    let file = std::fs::File::open(path).ok()?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut probe = symphonia::default::get_probe().format(Hint::new().with_extension("mp3"),
        stream, &Default::default(), &Default::default()).ok()?;
    let metadata = probe.metadata.get()?;
    let revision = metadata.current()?;
    let text = |key| revision.tags().iter().filter(|tag| tag.std_key == Some(key))
        .map(|tag| tag.value.to_string().trim_matches(['\0', ' ']).to_owned()).find(|s| !s.is_empty());
    info.artist = text(Key::Artist).or_else(|| info.artist.take());
    let album_artist = text(Key::AlbumArtist);
    info.has_album_artist = album_artist.is_some();
    if let Some(artist) = album_artist.or_else(|| info.artist.clone()) { info.album_artist = artist; }
    if let Some(album) = text(Key::Album) { info.album = album; }
    if let Some(title) = text(Key::TrackTitle) { info.title = title; }
    let number = |key| text(key)?.split('/').next()?.trim().parse::<u32>().ok();
    info.track_no = info.track_no.or_else(|| number(Key::TrackNumber));
    info.disc_no = info.disc_no.or_else(|| number(Key::DiscNumber));
    info.genre = info.genre.take().or_else(|| text(Key::Genre));
    info.year = info.year.or_else(|| text(Key::Date)?.get(..4)?.parse().ok());
    if info.picture.is_none() { info.picture = revision.visuals().first().map(|v| v.data.to_vec()); }
    if let Some(params) = probe.format.default_track().map(|t| &t.codec_params) {
        if let (Some(frames), Some(base)) = (params.n_frames, params.time_base) {
            let time = base.calc_time(frames);
            if info.duration == 0.0 { info.duration = time.seconds as f64 + time.frac; }
        }
    }
    Some(())
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

fn repair_unknown_albums(tx: &rusqlite::Transaction<'_>, roots: &[String]) -> rusqlite::Result<()> {
    // An untagged bonus recording can sit beside the tagged tracks of one release.
    let mut folders: HashMap<PathBuf, Option<String>> = HashMap::new();
    let mut untagged = Vec::new();
    let mut query = tx.prepare("SELECT t.path,t.album_id,t.title,a.title,a.artist FROM tracks t
        JOIN albums a ON t.album_id=a.id")?;
    let mut rows = query.query([])?;
    while let Some(row) = rows.next()? {
        let path = PathBuf::from(row.get::<_, String>(0)?);
        let Some(folder) = path.parent() else { continue };
        let id: String = row.get(1)?;
        if row.get::<_, String>(4)? == "Unknown Artist" {
            if folder.file_name().and_then(|s| s.to_str()) == Some(row.get::<_, String>(3)?.as_str())
                && path.file_stem().and_then(|s| s.to_str()) == Some(row.get::<_, String>(2)?.as_str())
                && !roots.iter().any(|r| Path::new(r) == folder) {
                untagged.push(path);
            }
        } else {
            folders.entry(folder.to_owned()).and_modify(|candidate| {
                if candidate.as_ref() != Some(&id) { *candidate = None; }
            }).or_insert(Some(id));
        }
    }
    for path in untagged {
        if let Some(Some(album)) = path.parent().and_then(|p| folders.get(p)) {
            tx.execute("UPDATE tracks SET album_id=?1 WHERE path=?2", params![album,path.to_string_lossy()])?;
        }
    }

    // Transfer saved work only where all former tracks identify a single corrected album.
    let mut moves = tx.prepare("SELECT p.album_id,min(t.album_id) FROM previous_unknown p
        LEFT JOIN tracks t ON t.path=p.path GROUP BY p.album_id HAVING count(*)=count(t.album_id)
        AND count(DISTINCT t.album_id)=1 AND NOT EXISTS(SELECT 1 FROM tracks WHERE album_id=p.album_id)")?;
    let pairs = moves.query_map([], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (old, new) in pairs {
        tx.execute("UPDATE OR IGNORE album_notes SET album_id=?2 WHERE album_id=?1", [&old,&new])?;
        tx.execute("UPDATE atmospheres SET album_id=?2 WHERE album_id=?1 AND NOT EXISTS
            (SELECT 1 FROM atmospheres WHERE album_id=?2) AND NOT EXISTS
            (SELECT 1 FROM atmosphere_variants WHERE album_id=?2 AND style='cinematic')", [&old,&new])?;
        tx.execute("UPDATE atmosphere_variants SET album_id=?2 WHERE album_id=?1 AND style NOT IN
            (SELECT style FROM atmosphere_variants WHERE album_id=?2 UNION SELECT 'cinematic' FROM atmospheres WHERE album_id=?2)",
            [&old,&new])?;
        tx.execute("UPDATE OR IGNORE settings SET key='atmos_auto_style.'||?2 WHERE key='atmos_auto_style.'||?1",
            [&old,&new])?;
        tx.execute("UPDATE albums AS dest SET has_art=1,cover_file=coalesce(src.cover_file,src.id||'.jpg')
            FROM albums AS src WHERE dest.id=?2 AND src.id=?1 AND src.has_art=1 AND dest.cover_file IS NULL
            AND (src.cover_file IS NOT NULL OR dest.has_art=0)", [&old,&new])?;
    }
    Ok(())
}

pub fn scan(db_path: &Path, data_dir: &Path, progress: impl Fn(Progress)) -> Result<usize, String> {
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
    tx.execute_batch("CREATE TEMP TABLE previous_unknown AS SELECT t.path,t.album_id FROM tracks t
        JOIN albums a ON a.id=t.album_id WHERE a.artist='Unknown Artist'").map_err(e2s)?;
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
                progress(Progress { files });
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
                "INSERT OR REPLACE INTO tracks(path, album_id, title, artist, track_no, disc_no, duration, mtime,has_album_artist)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    path_str,
                    album_id,
                    info.title,
                    info.artist,
                    info.track_no,
                    info.disc_no,
                    info.duration,
                    mtime,
                    info.has_album_artist
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

    // Keep supported tracks from offline folders. Retire formats dropped from the index.
    for path in existing.keys() {
        let file = Path::new(path);
        if seen.contains(path) || (offline.iter().any(|f| file.starts_with(f))
            && ext_of(file).is_some_and(|e| AUDIO_EXTS.contains(&e.as_str()))) {
            continue;
        }
        tx.execute("DELETE FROM tracks WHERE path=?1", [path]).map_err(e2s)?;
        changed = true;
    }
    if changed { repair_unknown_albums(&tx, &folders).map_err(e2s)?; }
    tx.execute(
        "DELETE FROM albums WHERE art_source IS NULL AND id NOT IN (SELECT DISTINCT album_id FROM tracks)",
        [],
    )
    .map_err(e2s)?;
    if changed { crate::releases::rebuild(&tx).map_err(e2s)?; }
    tx.commit().map_err(e2s)?;

    progress(Progress { files });
    let albums: i64 = conn
        .query_row("SELECT COUNT(*) FROM albums WHERE art_source IS NULL", [], |r| r.get(0))
        .map_err(e2s)?;
    Ok(albums as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_retires_cached_mp4_but_keeps_audio_and_the_original_file() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("Music");
        std::fs::create_dir(&folder).unwrap();
        let video = folder.join("concert.MP4");
        let audio = folder.join("song.m4a");
        std::fs::write(&video, b"video fixture").unwrap();
        std::fs::write(&audio, b"").unwrap();
        let path = tmp.path().join("library.db");
        let conn = db::open(&path).unwrap();
        db::add_folder(&conn, folder.to_str().unwrap()).unwrap();
        conn.execute("INSERT INTO albums(id,artist,title,sort_key) VALUES('old','Artist','Album','a')", []).unwrap();
        conn.execute("INSERT INTO tracks(path,album_id,title,mtime) VALUES(?1,'old','Concert',0)", [video.to_str()]).unwrap();
        assert!(db::tracks(&conn,"old").unwrap().is_empty());
        scan(&path, tmp.path(), |_| {}).unwrap();
        let paths = conn.prepare("SELECT path FROM tracks").unwrap()
            .query_map([], |r| r.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        assert_eq!(paths, [audio.to_str().unwrap()]);
        assert_eq!(std::fs::read(video).unwrap(), b"video fixture");
    }

    #[test]
    fn bonus_recordings_join_only_one_identifiable_release_and_keep_the_article() {
        let mut db = db::open(Path::new(":memory:")).unwrap();
        let tx = db.transaction().unwrap();
        for (id, artist, title) in [
            ("tagged", "Artist", "Alone (solo)"), ("other", "Another Artist", "Other"),
            ("old", "Unknown Artist", "Alone"), ("mixed", "Unknown Artist", "Mixtape"),
        ] {
            tx.execute("INSERT INTO albums(id,artist,title,sort_key) VALUES(?1,?2,?3,?2)",
                params![id,artist,title]).unwrap();
        }
        for (path, id) in [
            ("Music/Alone/song.mp3", "tagged"), ("Music/Alone/concert.wav", "old"),
            ("Music/Mixtape/a.mp3", "tagged"), ("Music/Mixtape/b.mp3", "other"),
            ("Music/Mixtape/bonus.wav", "mixed"),
        ] {
            tx.execute("INSERT INTO tracks(path,album_id,title,mtime) VALUES(?1,?2,?3,0)",
                params![path,id,Path::new(path).file_stem().unwrap().to_str()]).unwrap();
        }
        tx.execute_batch("CREATE TEMP TABLE previous_unknown AS SELECT path,album_id FROM tracks
            WHERE album_id IN ('old','mixed'); INSERT INTO album_notes VALUES('old','Saved article')").unwrap();
        repair_unknown_albums(&tx, &["Music".into()]).unwrap();
        assert_eq!(db::note(&tx,"tagged").unwrap().as_deref(), Some("Saved article"));
        assert!(db::tracks(&tx,"old").unwrap().is_empty());
        assert_eq!(db::tracks(&tx,"mixed").unwrap().len(), 1);
    }
}
