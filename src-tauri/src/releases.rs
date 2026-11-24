use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{params, Transaction};

fn disc_folder(dir: &Path) -> Option<u32> {
    let name = dir.file_name()?.to_str()?.to_ascii_lowercase();
    for prefix in ["disc", "disk", "cd"] {
        if let Some(suffix) = name.strip_prefix(prefix) {
            return suffix.trim_start_matches([' ', '-', '_', '.']).trim().parse().ok();
        }
    }
    None
}

struct Part {
    path: String,
    member: String,
    disc: Option<i64>,
    tagged: bool,
}

// The member ID already represents the scanner's album artist (or its track-artist fallback).
// A consistent album credit wins. Without tags, require a strong majority to merge guests.
fn lead_member(parts: &[Part]) -> Option<&str> {
    let mut tagged = parts.iter().filter(|p| p.tagged).map(|p| p.member.as_str());
    if let Some(first) = tagged.next() {
        return tagged.all(|member| member == first).then_some(first);
    }
    let mut counts = HashMap::new();
    for p in parts { *counts.entry(p.member.as_str()).or_insert(0) += 1; }
    counts.into_iter().find(|(_, count)| count * 3 > parts.len() * 2).map(|(id, _)| id)
}

/// Project multi-artist releases from the existing index, leaving the artist catalogue intact.
/// Called within the same transaction as a scan or the initial library upgrade.
pub fn rebuild(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    let mut groups: HashMap<String, Vec<Part>> = HashMap::new();
    let mut query = tx.prepare("SELECT t.path, t.album_id, a.title, t.disc_no, COALESCE(t.has_album_artist,0) FROM tracks t
        JOIN albums a ON a.id=t.album_id ORDER BY (a.cover_file IS NOT NULL) DESC, a.has_art DESC,
        COALESCE(t.disc_no,1), COALESCE(t.track_no,99999), t.path")?;
    let mut rows = query.query([])?;
    while let Some(row) = rows.next()? {
        let path: String = row.get(0)?;
        let folder = Path::new(&path).parent().unwrap_or(Path::new(""));
        let disc = disc_folder(folder);
        let root = if disc.is_some() { folder.parent().unwrap_or(folder) } else { folder };
        let title: String = row.get(2)?;
        let key = format!("{}\0{}", root.display(), title.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase());
        groups.entry(key).or_default().push(Part {
            path, member: row.get(1)?, disc: row.get::<_, Option<i64>>(3)?.or(disc.map(i64::from)), tagged: row.get(4)?,
        });
    }
    let mut participating = HashSet::new();
    for parts in groups.values() {
        if parts.iter().any(|p| p.member != parts[0].member) {
            participating.extend(parts.iter().map(|p| p.member.as_str()));
        }
    }

    tx.execute("DELETE FROM release_tracks", [])?;
    let mut insert_album = tx.prepare("INSERT INTO albums(id,artist,title,year,genre,sort_key,art_source,has_art)
        SELECT ?1, COALESCE(?2,artist), title, year, genre, sort_key, ?4, 0 FROM albums WHERE id=?3
        ON CONFLICT(id) DO UPDATE SET artist=excluded.artist,title=excluded.title,year=excluded.year,
        genre=excluded.genre,sort_key=excluded.sort_key,art_source=excluded.art_source")?;
    let mut insert_track = tx.prepare("INSERT INTO release_tracks(release_id,path,disc_no) VALUES(?1,?2,?3)")?;
    for (key, parts) in &groups {
        let source = &parts[0].member;
        if !participating.contains(source.as_str()) { continue; }
        let id = format!("release-{}", crate::scanner::fnv(key));
        let lead = lead_member(parts);
        let artist = if lead.is_none() { Some("Various Artists") } else { None };
        insert_album.execute(params![id, artist, lead.unwrap_or(source), source])?;
        for p in parts { insert_track.execute(params![id, p.path, p.disc])?; }
    }
    tx.execute("DELETE FROM albums WHERE art_source IS NOT NULL AND id NOT IN
        (SELECT release_id FROM release_tracks)", [])?;
    // Older indexes only stored the resolved name. Re-read just the affected releases on scan.
    tx.execute("UPDATE tracks SET mtime=-1 WHERE has_album_artist IS NULL AND mtime<>-1
        AND path IN (SELECT path FROM release_tracks)", [])?;
    Ok(())
}
