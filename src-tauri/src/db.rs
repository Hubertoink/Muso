use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use std::time::Duration;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let c = Connection::open(path)?;
    c.busy_timeout(Duration::from_secs(15))?;
    c.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
    c.execute_batch(
        "PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS folders(path TEXT PRIMARY KEY);
         CREATE TABLE IF NOT EXISTS albums(
            id TEXT PRIMARY KEY,
            artist TEXT NOT NULL,
            title TEXT NOT NULL,
            year INTEGER,
            genre TEXT,
            has_art INTEGER NOT NULL DEFAULT 0,
            sort_key TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS tracks(
            path TEXT PRIMARY KEY,
            album_id TEXT NOT NULL,
            title TEXT NOT NULL,
            artist TEXT,
            track_no INTEGER,
            disc_no INTEGER,
            duration REAL,
            mtime INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS tracks_album ON tracks(album_id);
         CREATE TABLE IF NOT EXISTS atmospheres(
            album_id TEXT NOT NULL,
            idx INTEGER NOT NULL,
            file TEXT NOT NULL,
            PRIMARY KEY(album_id, idx));
         CREATE TABLE IF NOT EXISTS atmosphere_variants(
            album_id TEXT NOT NULL,
            style TEXT NOT NULL,
            idx INTEGER NOT NULL,
            file TEXT NOT NULL,
            PRIMARY KEY(album_id, style, idx));
         CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    // Libraries created before online cover search have only the original art cache.
    let has_cover_file: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('albums') WHERE name='cover_file')",
        [], |r| r.get(0),
    )?;
    if !has_cover_file {
        c.execute("ALTER TABLE albums ADD COLUMN cover_file TEXT", [])?;
    }
    Ok(c)
}

#[derive(Serialize, Clone)]
pub struct Album {
    pub id: String,
    pub artist: String,
    pub title: String,
    pub year: Option<i64>,
    pub genre: Option<String>,
    pub art: bool,
    pub cover_file: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub path: String,
    pub title: String,
    pub artist: Option<String>,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub duration: Option<f64>,
}

pub fn folders(c: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut st = c.prepare("SELECT path FROM folders ORDER BY path")?;
    let rows = st.query_map([], |r| r.get(0))?;
    rows.collect()
}

pub fn add_folder(c: &Connection, path: &str) -> rusqlite::Result<()> {
    c.execute("INSERT OR IGNORE INTO folders(path) VALUES(?1)", [path])?;
    Ok(())
}

pub fn remove_folder(c: &Connection, path: &str) -> rusqlite::Result<()> {
    c.execute("DELETE FROM folders WHERE path=?1", [path])?;
    Ok(())
}

pub fn albums(c: &Connection) -> rusqlite::Result<Vec<Album>> {
    let mut st = c.prepare(
        "SELECT id, artist, title, year, genre, has_art, cover_file FROM albums ORDER BY sort_key",
    )?;
    let rows = st.query_map([], |r| {
        Ok(Album {
            id: r.get(0)?,
            artist: r.get(1)?,
            title: r.get(2)?,
            year: r.get(3)?,
            genre: r.get(4)?,
            art: r.get::<_, i64>(5)? != 0,
            cover_file: r.get(6)?,
        })
    })?;
    rows.collect()
}

pub fn album(c: &Connection, id: &str) -> rusqlite::Result<Option<Album>> {
    c.query_row(
        "SELECT id, artist, title, year, genre, has_art, cover_file FROM albums WHERE id=?1",
        [id],
        |r| {
            Ok(Album {
                id: r.get(0)?,
                artist: r.get(1)?,
                title: r.get(2)?,
                year: r.get(3)?,
                genre: r.get(4)?,
                art: r.get::<_, i64>(5)? != 0,
                cover_file: r.get(6)?,
            })
        },
    )
    .optional()
}

pub fn tracks(c: &Connection, album_id: &str) -> rusqlite::Result<Vec<Track>> {
    let mut st = c.prepare(
        "SELECT path, title, artist, track_no, disc_no, duration FROM tracks
         WHERE album_id=?1
         ORDER BY COALESCE(disc_no, 1), COALESCE(track_no, 99999), path",
    )?;
    let rows = st.query_map([album_id], |r| {
        Ok(Track {
            path: r.get(0)?,
            title: r.get(1)?,
            artist: r.get(2)?,
            track_no: r.get(3)?,
            disc_no: r.get(4)?,
            duration: r.get(5)?,
        })
    })?;
    rows.collect()
}

pub fn get_setting(c: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    c.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0))
        .optional()
}

pub fn set_setting(c: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO settings(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn atmospheres(c: &Connection, album_id: &str, style: &str) -> rusqlite::Result<Vec<String>> {
    let mut st = c.prepare("SELECT file, idx FROM atmospheres WHERE album_id=?1 AND ?2='cinematic'
        UNION ALL SELECT file, idx FROM atmosphere_variants WHERE album_id=?1 AND style=?2 ORDER BY idx")?;
    let rows = st.query_map([album_id, style], |r| r.get(0))?;
    rows.collect()
}

pub fn replace_atmospheres(c: &Connection, album_id: &str, style: &str, files: &[String]) -> rusqlite::Result<()> {
    if style == "cinematic" {
        c.execute("DELETE FROM atmospheres WHERE album_id=?1", [album_id])?;
    }
    c.execute("DELETE FROM atmosphere_variants WHERE album_id=?1 AND style=?2", [album_id, style])?;
    for (i, f) in files.iter().enumerate() {
        c.execute(
            "INSERT INTO atmosphere_variants(album_id, style, idx, file) VALUES(?1, ?2, ?3, ?4)",
            params![album_id, style, i as i64, f],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_one_style_preserves_legacy_images_and_other_styles() {
        let c = open(Path::new(":memory:")).unwrap();
        c.execute("INSERT INTO atmospheres VALUES ('album', 0, 'original.jpg')", []).unwrap();
        for (style, file) in [("dreamscape", "first.jpg"), ("ambient", "ambient.jpg"), ("dreamscape", "new.jpg")] {
            replace_atmospheres(&c, "album", style, &[file.into()]).unwrap();
        }
        assert_eq!(atmospheres(&c, "album", "cinematic").unwrap(), ["original.jpg"]);
        assert_eq!(atmospheres(&c, "album", "ambient").unwrap(), ["ambient.jpg"]);
        assert_eq!(atmospheres(&c, "album", "dreamscape").unwrap(), ["new.jpg"]);
    }
}
