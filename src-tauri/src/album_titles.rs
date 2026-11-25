use crate::{crates, releases, scanner};
use rusqlite::{params, Transaction};

// These references live outside the track index and survive a release rebuild.
fn relink(tx: &Transaction<'_>, old: &str, new: &str) -> rusqlite::Result<()> {
    scanner::move_saved_work(tx, old, new)?;
    tx.execute("INSERT INTO album_genre SELECT ?2,genre,source FROM album_genre WHERE album_id=?1
        ON CONFLICT(album_id) DO UPDATE SET genre=excluded.genre,source=excluded.source
        WHERE excluded.source='user' AND album_genre.source<>'user'", [old, new])?;
    tx.execute("DELETE FROM album_genre WHERE album_id=?1", [old])?;
    tx.execute("UPDATE OR IGNORE genre_suggestions SET album_id=?2 WHERE album_id=?1", [old, new])?;
    tx.execute("UPDATE OR IGNORE album_image_profiles SET album=?2 WHERE album=?1", [old, new])?;
    tx.execute("UPDATE OR IGNORE album_visual_profiles SET album=?2 WHERE album=?1", [old, new])?;
    tx.execute("INSERT INTO album_arrivals SELECT ?2,first_seen FROM album_arrivals WHERE id=?1
        ON CONFLICT(id) DO UPDATE SET first_seen=min(first_seen,excluded.first_seen)", [old, new])?;
    crates::relink_album(tx, old, new)
}

pub(super) fn repair(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    let mut query = tx.prepare("SELECT id,artist,title FROM albums
        WHERE art_source IS NULL AND instr(title,char(133))>0 ORDER BY id")?;
    let affected = query.query_map([], |r| Ok((
        r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?
    )))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if affected.is_empty() { return Ok(()) }

    // Keep a representative track and any custom artwork for compilation entries
    // whose generated ID changes or whose tracks collapse into a single album.
    tx.execute_batch("CREATE TEMP TABLE prior_title_releases AS
        SELECT a.id,min(r.path) AS path,a.cover_file,a.has_art FROM albums a
        JOIN release_tracks r ON r.release_id=a.id GROUP BY a.id;")?;
    for (old, artist, raw) in affected {
        let title = scanner::album_title(&raw);
        let new = scanner::album_id(&artist, &title);
        tx.execute("INSERT INTO albums(id,artist,title,year,genre,sort_key)
            SELECT ?2,artist,?3,year,genre,replace(sort_key,char(133),'…') FROM albums WHERE id=?1
            ON CONFLICT(id) DO UPDATE SET year=coalesce(albums.year,excluded.year),
            genre=coalesce(albums.genre,excluded.genre)", params![old, new, title])?;
        relink(tx, &old, &new)?;
        tx.execute("UPDATE tracks SET album_id=?2 WHERE album_id=?1", [&old, &new])?;
        tx.execute("DELETE FROM albums WHERE id=?1", [&old])?;
    }
    releases::rebuild(tx)?;
    let mut obsolete = tx.prepare("SELECT p.id,coalesce(r.release_id,t.album_id),p.cover_file,p.has_art
        FROM prior_title_releases p JOIN tracks t ON t.path=p.path
        LEFT JOIN release_tracks r ON r.path=p.path WHERE NOT EXISTS(SELECT 1 FROM albums WHERE id=p.id)")?;
    let rows = obsolete.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?,
        r.get::<_, Option<String>>(2)?, r.get::<_, bool>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    drop(obsolete);
    for (old, new, cover, has_art) in rows {
        if has_art {
            tx.execute("UPDATE albums SET has_art=1,cover_file=coalesce(?3,?1||'.jpg')
                WHERE id=?2 AND cover_file IS NULL AND (has_art=0 OR ?3 IS NOT NULL)", params![old,new,cover])?;
        }
        relink(tx, &old, &new)?;
    }
    tx.execute_batch("DROP TABLE prior_title_releases")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn reopening_an_old_index_merges_only_matching_artists_and_keeps_saved_work() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        let db = db::open(&path).unwrap();
        let old = scanner::album_id("Artist", "Then\u{85}Now");
        let new = scanner::album_id("Artist", "Then…Now");
        let other = scanner::album_id("Guest", "Then…Now");
        db.execute("DELETE FROM settings WHERE key='scanner.album_ellipsis_v1'", []).unwrap();
        for (id, artist, title, track) in [
            (&old, "Artist", "Then\u{85}Now", "Album/one.mp3"),
            (&new, "Artist", "Then…Now", "Album/two.mp3"),
            (&other, "Guest", "Then…Now", "Other/one.mp3"),
        ] {
            db.execute("INSERT INTO albums(id,artist,title,sort_key) VALUES(?1,?2,?3,?3)",
                params![id, artist, title]).unwrap();
            db.execute("INSERT INTO tracks(path,album_id,title,mtime) VALUES(?1,?2,'Track',42)",
                params![track,id]).unwrap();
        }
        db.execute("UPDATE albums SET has_art=1 WHERE id=?1", [&old]).unwrap();
        db.execute("INSERT INTO album_notes VALUES(?1,'Saved')", [&old]).unwrap();
        db.execute("INSERT INTO album_arrivals VALUES(?1,10),(?2,20)", [&old,&new]).unwrap();
        db.execute("INSERT INTO album_genre VALUES(?1,'Ambient','user'),(?2,'Rock','ai')", [&old,&new]).unwrap();
        let entry = |id: &str| serde_json::json!({"id":id,"artist":"Artist","title":"Cached"});
        let crates = serde_json::json!({"active":1,"lists":[{"id":1,"name":"Night",
            "albums":[entry(&old),entry(&other),entry(&new)]}]});
        db::set_setting(&db,"library.crates",&crates.to_string()).unwrap();
        drop(db);
        for _ in 0..2 {
            let db = db::open(&path).unwrap();
            assert_eq!(db::albums(&db).unwrap().len(),2);
            assert_eq!(db::tracks(&db,&new).unwrap().len(),2);
            assert_eq!(db::tracks(&db,&other).unwrap().len(),1);
            let album = db::album(&db,&new).unwrap().unwrap();
            assert_eq!(album.title,"Then…Now");
            assert_eq!(album.cover_file.unwrap(),format!("{old}.jpg"));
            assert_eq!(db::note(&db,&new).unwrap().as_deref(),Some("Saved"));
            assert_eq!(db.query_row("SELECT first_seen FROM album_arrivals WHERE id=?1",[&new],
                |r| r.get::<_, i64>(0)).unwrap(),10);
            assert_eq!(db.query_row("SELECT genre FROM album_genre WHERE album_id=?1",[&new],
                |r| r.get::<_, String>(0)).unwrap(),"Ambient");
            let crates: serde_json::Value = serde_json::from_str(&db::get_setting(&db,"library.crates").unwrap().unwrap()).unwrap();
            let entries = crates["lists"][0]["albums"].as_array().unwrap();
            assert_eq!(entries.len(),2);
            assert_eq!(entries[0]["id"],new);
            assert_eq!(entries[0]["title"],"Then…Now");
            assert_eq!(entries[1]["id"],other);
        }
    }
}
