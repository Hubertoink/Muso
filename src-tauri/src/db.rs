use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use std::time::Duration;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let mut c = Connection::open(path)?;
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
         CREATE TABLE IF NOT EXISTS cover_sources(file TEXT PRIMARY KEY, provider TEXT NOT NULL, album_url TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS album_notes(album_id TEXT PRIMARY KEY, body TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS painting_prompts(file TEXT PRIMARY KEY, prompt TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS album_research(album_id TEXT PRIMARY KEY, brief TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    crate::genres::schema(&c)?;
    crate::personal_images::schema(&mut c)?;
    // Libraries created before online cover search have only the original art cache.
    let has_cover_file: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('albums') WHERE name='cover_file')",
        [], |r| r.get(0),
    )?;
    if !has_cover_file {
        c.execute("ALTER TABLE albums ADD COLUMN cover_file TEXT", [])?;
    }
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    crate::style_history::migrate(&tx)?;
    if get_setting(&tx, "scanner.artist_repair_v1")?.is_none() {
        tx.execute("UPDATE tracks SET mtime=-1 WHERE album_id IN
            (SELECT id FROM albums WHERE artist='Unknown Artist')", [])?;
        set_setting(&tx, "scanner.artist_repair_v1", "1")?;
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS release_tracks(
        path TEXT PRIMARY KEY, release_id TEXT NOT NULL, disc_no INTEGER);
        CREATE INDEX IF NOT EXISTS tracks_release ON release_tracks(release_id);")?;
    let has_source: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('albums')
        WHERE name='art_source')", [], |r| r.get(0))?;
    if !has_source {
        tx.execute("ALTER TABLE albums ADD COLUMN art_source TEXT", [])?;
    }
    let has_credit: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('tracks')
        WHERE name='has_album_artist')", [], |r| r.get(0))?;
    if !has_credit {
        tx.execute("ALTER TABLE tracks ADD COLUMN has_album_artist INTEGER", [])?;
    }
    let has_arrivals: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master
        WHERE type='table' AND name='album_arrivals')", [], |r| r.get(0))?;
    if !has_arrivals {
        tx.execute("CREATE TABLE album_arrivals(id TEXT PRIMARY KEY, first_seen INTEGER NOT NULL)", [])?;
    }
    if !has_source || !has_credit || !has_arrivals {
        crate::releases::rebuild(&tx)?;
    }
    if !has_arrivals {
        tx.execute("UPDATE album_arrivals SET first_seen=0", [])?;
    }
    tx.commit()?;
    c.execute_batch("CREATE TEMP VIEW resolved_albums AS SELECT a.id,a.artist,a.title,a.year,a.genre,
        (a.has_art=1 OR COALESCE(s.has_art,0)=1) AS has_art,
        COALESCE(a.cover_file,s.cover_file,CASE WHEN s.has_art=1 THEN s.id || '.jpg' END) AS cover_file,
        a.sort_key FROM albums a LEFT JOIN albums s ON a.art_source=s.id;")?;
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

#[derive(Serialize)]
pub struct LibraryAlbum {
    pub first_seen: i64,
    #[serde(flatten)]
    pub album: Album,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
}

fn read_album(r: &rusqlite::Row<'_>) -> rusqlite::Result<Album> {
    Ok(Album {
        id: r.get(0)?, artist: r.get(1)?, title: r.get(2)?, year: r.get(3)?,
        genre: r.get(4)?, art: r.get::<_, bool>(5)?, cover_file: r.get(6)?,
    })
}

pub fn albums(c: &Connection) -> rusqlite::Result<Vec<LibraryAlbum>> {
    let mut members = std::collections::HashMap::<String, Vec<String>>::new();
    let mut links = c.prepare("SELECT DISTINCT r.release_id,t.album_id FROM release_tracks r
        JOIN tracks t ON t.path=r.path ORDER BY r.release_id,t.album_id")?;
    let mut rows = links.query([])?;
    while let Some(r) = rows.next()? { members.entry(r.get(0)?).or_default().push(r.get(1)?); }
    let mut st = c.prepare(
        "SELECT id, artist, title, year, genre, has_art, cover_file, COALESCE(first_seen,0)
         FROM resolved_albums LEFT JOIN album_arrivals USING(id) ORDER BY sort_key,id",
    )?;
    let rows = st.query_map([], |r| {
        let album = read_album(r)?;
        Ok(LibraryAlbum { members: members.remove(&album.id).unwrap_or_default(), album, first_seen: r.get(7)? })
    })?;
    rows.collect()
}

pub fn album(c: &Connection, id: &str) -> rusqlite::Result<Option<Album>> {
    c.query_row(
        "SELECT id, artist, title, year, genre, has_art, cover_file FROM resolved_albums WHERE id=?1",
        [id], read_album,
    )
    .optional()
}

pub fn tracks(c: &Connection, album_id: &str) -> rusqlite::Result<Vec<Track>> {
    let sql = if album_id.starts_with("release-") {
        "SELECT t.path,t.title,t.artist,t.track_no,r.disc_no,t.duration FROM release_tracks r
         JOIN tracks t ON t.path=r.path WHERE r.release_id=?1 AND lower(substr(t.path,-4))<>'.mp4'
         ORDER BY COALESCE(r.disc_no,1), COALESCE(t.track_no,99999), t.path"
    } else {
        "SELECT path, title, artist, track_no, disc_no, duration FROM tracks
         WHERE album_id=?1 AND lower(substr(path,-4))<>'.mp4'
         ORDER BY COALESCE(disc_no, 1), COALESCE(track_no, 99999), path"
    };
    let mut st = c.prepare(sql)?;
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

pub fn research(c: &Connection, id: &str) -> rusqlite::Result<Option<String>> {
    c.query_row("SELECT brief FROM album_research WHERE album_id=?1", [id], |r| r.get(0)).optional()
}

pub fn note(c: &Connection, id: &str) -> rusqlite::Result<Option<String>> {
    c.query_row("SELECT body FROM album_notes WHERE album_id=?1", [id], |r| r.get(0)).optional()
}

pub fn set_setting(c: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO settings(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn has_atmospheres(c: &Connection, album_id: &str) -> rusqlite::Result<bool> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM atmospheres WHERE album_id=?1)
        OR EXISTS(SELECT 1 FROM atmosphere_variants WHERE album_id=?1)", [album_id], |r| r.get(0))
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
    fn existing_releases_stay_old_when_a_guest_track_is_added() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("collection.db");
        let c = open(&path).unwrap();
        c.execute_batch("DROP TABLE album_arrivals;
            INSERT INTO albums(id,artist,title,sort_key) VALUES('a','A','Score','a');
            INSERT INTO tracks(path,album_id,title,mtime) VALUES('Score/one.mp3','a','One',0);").unwrap();
        drop(c);
        let mut c = open(&path).unwrap();
        assert_eq!(albums(&c).unwrap()[0].first_seen, 0);
        let tx = c.transaction().unwrap();
        tx.execute_batch("INSERT INTO albums(id,artist,title,sort_key) VALUES('b','B','Score','b'),('c','C','New','c');
            INSERT INTO tracks(path,album_id,title,mtime) VALUES('Score/guest.mp3','b','Guest',0),('New/new.mp3','c','New',0);").unwrap();
        crate::releases::rebuild(&tx).unwrap();
        let records = albums(&tx).unwrap();
        assert_eq!(records.iter().find(|a| a.members.len()==2).unwrap().first_seen, 0);
        assert!(records.iter().find(|a| a.album.id=="c").unwrap().first_seen > 0);
        tx.execute("UPDATE album_arrivals SET first_seen=123 WHERE id='c'", []).unwrap();
        crate::releases::rebuild(&tx).unwrap();
        assert_eq!(albums(&tx).unwrap().iter().find(|a| a.album.id=="c").unwrap().first_seen, 123);
    }

    #[test]
    fn compilation_projection_keeps_artist_tracks_and_separate_releases() {
        let mut c = open(Path::new(":memory:")).unwrap();
        for id in ["a", "b", "c"] {
            c.execute("INSERT INTO albums(id,artist,title,has_art,sort_key)
                VALUES(?1,?1,'Jungle Jazz Vol 2',1,?1)", [id]).unwrap();
        }
        for (path, artist, track) in [("Jazz/CD2/one.mp3","a",1), ("Jazz/CD1/two.mp3","b",2),
            ("Jazz/CD1/one.mp3","a",1), ("Elsewhere/one.mp3","a",1), ("Other/two.mp3","c",2)] {
            c.execute("INSERT INTO tracks(path,album_id,title,artist,track_no,mtime)
                VALUES(?1,?2,?1,?2,?3,1)", params![path,artist,track]).unwrap();
        }
        for _ in 0..2 {
            let tx = c.transaction().unwrap();
            crate::releases::rebuild(&tx).unwrap();
            tx.commit().unwrap();
        }
        let library = albums(&c).unwrap();
        let comp = library.iter().find(|a| a.members.len() == 2).unwrap();
        assert_eq!(comp.album.artist, "Various Artists");
        assert_eq!(comp.members, ["a","b"]);
        assert!(comp.album.art && comp.album.cover_file.is_some());
        assert_eq!(album(&c, &comp.album.id).unwrap().unwrap().title, "Jungle Jazz Vol 2");
        let parts = tracks(&c, &comp.album.id).unwrap();
        assert_eq!(parts.iter().map(|t| t.path.as_str()).collect::<Vec<_>>(),
            ["Jazz/CD1/one.mp3","Jazz/CD1/two.mp3","Jazz/CD2/one.mp3"]);
        assert_eq!(tracks(&c,"a").unwrap().len(), 3);
        let solo = library.iter().find(|a| a.members == ["a"]).unwrap();
        assert_eq!(tracks(&c, &solo.album.id).unwrap()[0].path, "Elsewhere/one.mp3");
        assert!(library.iter().any(|a| a.album.id == "c" && a.members.is_empty()));
        c.execute("DELETE FROM tracks WHERE album_id='b'", []).unwrap();
        let tx = c.transaction().unwrap();
        crate::releases::rebuild(&tx).unwrap();
        tx.commit().unwrap();
        assert!(album(&c, &comp.album.id).unwrap().is_none());
        assert_eq!(tracks(&c,"a").unwrap().len(), 3);
    }

    #[test]
    fn guest_tracks_follow_the_album_credit_and_keep_their_performers() {
        let mut db = open(Path::new(":memory:")).unwrap();
        for id in ["Composer", "Guest", "DJ"] {
            db.execute("INSERT INTO albums(id,artist,title,sort_key) VALUES(?1,?1,'Score',?1)", [id]).unwrap();
        }
        for (num, member, performer, tagged) in [
            (1, "DJ", "DJ", false), (2, "Composer", "Composer", true),
            (3, "Composer", "Composer", true), (4, "Guest", "Guest", false),
            (5, "Composer", "DJ", true),
        ] {
            db.execute("INSERT INTO tracks(path,album_id,title,artist,track_no,mtime,has_album_artist)
                VALUES(?1,?2,'Piece',?3,?4,0,?5)", params![format!("Score/{num}.mp3"),member,performer,num,tagged]).unwrap();
        }
        let tx = db.transaction().unwrap();
        crate::releases::rebuild(&tx).unwrap();
        let release = albums(&tx).unwrap().into_iter().find(|a| a.members.len()==3).unwrap().album;
        assert_eq!(release.artist, "Composer");
        let tracks = tracks(&tx, &release.id).unwrap();
        assert_eq!(tracks.iter().filter_map(|t| t.artist.as_deref()).collect::<Vec<_>>(), ["DJ","Composer","Composer","Guest","DJ"]);
        tx.execute("UPDATE tracks SET has_album_artist=1 WHERE album_id='Guest'", []).unwrap();
        crate::releases::rebuild(&tx).unwrap();
        assert_eq!(album(&tx, &release.id).unwrap().unwrap().artist, "Various Artists");
        tx.execute("UPDATE tracks SET has_album_artist=0", []).unwrap();
        crate::releases::rebuild(&tx).unwrap();
        assert_eq!(album(&tx, &release.id).unwrap().unwrap().artist, "Various Artists");
        tx.execute("DELETE FROM tracks WHERE album_id='Guest'", []).unwrap();
        crate::releases::rebuild(&tx).unwrap();
        assert_eq!(album(&tx, &release.id).unwrap().unwrap().artist, "Composer");
    }

    #[test]
    fn automatic_painting_checks_legacy_and_styled_sets_for_this_album() {
        let c = open(Path::new(":memory:")).unwrap();
        c.execute("INSERT INTO atmospheres VALUES('old',0,'first.jpg')", []).unwrap();
        replace_atmospheres(&c, "styled", "cel", &["cel.jpg".into()]).unwrap();
        assert!(has_atmospheres(&c, "old").unwrap());
        assert!(has_atmospheres(&c, "styled").unwrap());
        assert!(!has_atmospheres(&c, "empty").unwrap());
    }

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
