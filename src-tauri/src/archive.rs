use crate::{db, err, genres, paintings, R};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

#[derive(Default, Serialize)]
pub struct Summary {
    albums: usize,
    notes: usize,
    images: usize,
    genres: usize,
    skipped: usize,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    albums: Vec<Entry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    genres: Option<GenreBook>,
}

#[derive(Serialize, Deserialize)]
struct GenreBook {
    genres: Vec<genres::Genre>,
    map: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
struct GenreChoice {
    genre: String,
    source: String,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    id: String,
    artist: String,
    title: String,
    members: Vec<String>,
    note: Option<String>,
    research: Option<String>,
    cover: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cover_source: Option<crate::covers::Origin>,
    paintings: BTreeMap<String, Vec<usize>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    prompts: BTreeMap<usize, String>,
    auto_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    genre: Option<GenreChoice>,
}

fn media_name(index: usize) -> String {
    format!("images/{index}.jpg")
}

fn add_image(
    zip: &mut ZipWriter<&mut File>,
    dir: &Path,
    rel: &str,
    summary: &mut Summary,
) -> R<usize> {
    let (folder, name) = rel.split_once('/').ok_or("Invalid cached image path")?;
    if !matches!(folder, "art" | "atmos")
        || !name.ends_with(".jpg")
        || name.contains(['/', '\\', ':'])
    {
        return Err("Invalid cached image path".into());
    }
    let mut image =
        File::open(dir.join(rel)).map_err(|e| format!("Could not export {rel}: {e}"))?;
    let index = summary.images;
    zip.start_file(
        media_name(index),
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )
    .map_err(err)?;
    std::io::copy(&mut image, zip).map_err(err)?;
    summary.images += 1;
    Ok(index)
}

pub fn export(c: &mut Connection, dir: &Path, dest: &Path) -> R<Summary> {
    if !dest
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("zip"))
    {
        return Err("Choose a .zip filename.".into());
    }
    let mut temp =
        NamedTempFile::new_in(dest.parent().ok_or("Missing export folder")?).map_err(err)?;
    let mut zip = ZipWriter::new(temp.as_file_mut());
    let tx = c.transaction().map_err(err)?;
    let mut summary = Summary::default();
    let mut manifest = Manifest {
        version: 1,
        albums: Vec::new(),
        genres: None,
    };
    let book = genres::Book::load(&tx).map_err(err)?;
    if !book.genres.is_empty() || !book.map.is_empty() {
        manifest.genres = Some(GenreBook {
            genres: book.genres.clone(),
            map: book.map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        });
    }
    let mut choices = tx.prepare("SELECT genre, source FROM album_genre WHERE album_id=?1").map_err(err)?;
    let mut saved_styles = tx.prepare("SELECT style FROM atmosphere_variants WHERE album_id=?1
        UNION SELECT 'cinematic' FROM atmospheres WHERE album_id=?1").map_err(err)?;
    for record in db::albums(&tx).map_err(err)? {
        let a = record.album;
        let note = db::note(&tx, &a.id).map_err(err)?;
        let research = db::research(&tx, &a.id).map_err(err)?;
        let mut paintings = BTreeMap::new();
        let mut prompts = BTreeMap::new();
        let styles = saved_styles.query_map([&a.id], |row| row.get::<_, String>(0)).map_err(err)?;
        for style in styles {
            let style = style.map_err(err)?;
            let files = db::atmospheres(&tx, &a.id, &style).map_err(err)?;
            if files.is_empty() {
                continue;
            }
            let mut indices = Vec::new();
            for file in files {
                let index = add_image(&mut zip, dir, &file, &mut summary)?;
                if let Some(prompt) = tx.query_row("SELECT prompt FROM painting_prompts WHERE file=?1", [&file],
                    |r| r.get::<_,String>(0)).optional().map_err(err)? {
                    prompts.insert(index, prompt);
                }
                indices.push(index);
            }
            paintings.insert(style.to_owned(), indices);
        }
        // Release sleeves can inherit their artwork from an artist entry already in this archive.
        let own_art: bool = tx
            .query_row("SELECT has_art=1 FROM albums WHERE id=?1", [&a.id], |r| {
                r.get(0)
            })
            .map_err(err)?;
        let (cover, cover_source) = if own_art {
            let file = a.cover_file.unwrap_or_else(|| format!("{}.jpg", a.id));
            let source = tx.query_row("SELECT provider,album_url FROM cover_sources WHERE file=?1", [&file],
                |r| Ok(crate::covers::Origin { provider: r.get(0)?, album_url: r.get(1)? })).optional().map_err(err)?;
            (Some(add_image(
                &mut zip,
                dir,
                &format!("art/{file}"),
                &mut summary,
            )?), source)
        } else {
            (None, None)
        };
        let genre = choices
            .query_row([&a.id], |r| Ok(GenreChoice { genre: r.get(0)?, source: r.get(1)? }))
            .optional()
            .map_err(err)?;
        if note.is_none() && research.is_none() && cover.is_none() && paintings.is_empty() && genre.is_none() {
            continue;
        }
        summary.notes += usize::from(note.is_some());
        summary.genres += usize::from(genre.is_some());
        let auto_style =
            db::get_setting(&tx, &format!("atmos_auto_style.{}", a.id)).map_err(err)?;
        manifest.albums.push(Entry {
            id: a.id,
            artist: a.artist,
            title: a.title,
            members: record.members,
            note,
            research,
            cover,
            cover_source,
            paintings,
            prompts,
            auto_style,
            genre,
        });
    }
    summary.albums = manifest.albums.len();
    zip.start_file(
        "muso.json",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )
    .map_err(err)?;
    serde_json::to_writer(&mut zip, &manifest).map_err(err)?;
    zip.finish().map_err(err)?.sync_all().map_err(err)?;
    temp.persist(dest).map_err(err)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transfers_articles_and_images_between_different_music_roots() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("source");
        let destination = folder.path().join("destination");
        let catalog = |root: &str| {
            let mut c = db::open(Path::new(":memory:")).unwrap();
            for (id, n) in [("artist1", 1), ("artist2", 2)] {
                c.execute(
                    "INSERT INTO albums(id,artist,title,sort_key) VALUES(?1,?1,'Sampler',?1)",
                    [id],
                )
                .unwrap();
                c.execute(
                    "INSERT INTO tracks(path,album_id,title,mtime) VALUES(?1,?2,'Song',0)",
                    rusqlite::params![format!("{root}/{n}.mp3"), id],
                )
                .unwrap();
            }
            let tx = c.transaction().unwrap();
            crate::releases::rebuild(&tx).unwrap();
            tx.commit().unwrap();
            let id = db::albums(&c)
                .unwrap()
                .into_iter()
                .find(|a| !a.members.is_empty())
                .unwrap()
                .album
                .id;
            (c, id)
        };
        let (mut c, id) = catalog("Music/A");
        fs::create_dir_all(source.join("atmos")).unwrap();
        fs::create_dir_all(source.join("art")).unwrap();
        let jpg = image::DynamicImage::new_rgb8(60, 40);
        jpg.save(source.join("atmos/painting.jpg")).unwrap();
        jpg.save(source.join("art/artist1.jpg")).unwrap();
        crate::covers::Origin { provider: "TheAudioDB".into(), album_url: "https://www.theaudiodb.com/album/123".into() }
            .save(&c, "artist1.jpg").unwrap();
        c.execute("UPDATE albums SET has_art=1 WHERE id='artist1'", [])
            .unwrap();
        db::replace_atmospheres(&c, &id, "auto", &["atmos/painting.jpg".into()]).unwrap();
        db::replace_atmospheres(&c, &id, "cel", &["atmos/painting.jpg".into()]).unwrap();
        db::set_setting(&c, &format!("atmos_auto_style.{id}"), "Ink").unwrap();
        db::set_setting(&c, "openai_key", "secret-test-marker").unwrap();
        c.execute("INSERT INTO painting_prompts VALUES('atmos/painting.jpg','An open clearing')",[]).unwrap();
        c.execute("INSERT INTO album_notes VALUES(?1,'An article')", [&id])
            .unwrap();
        let path = folder.path().join("collection.zip");
        c.execute("INSERT INTO album_research VALUES(?1,'Label notes')", [&id]).unwrap();
        let electronic = genres::Genre { name: "Electronic".into(), parent: None };
        genres::save_book(&mut c, &[electronic], &BTreeMap::from([("Dance & DJ".into(), "Electronic".into())]), &BTreeMap::new()).unwrap();
        genres::set_choice(&c, &id, Some("Modern Classical")).unwrap();
        assert_eq!(export(&mut c, &source, &path).unwrap().images, 3);
        assert!(!fs::read(&path)
            .unwrap()
            .windows(18)
            .any(|b| b == b"secret-test-marker"));
        let (mut other, new_id) = catalog("Other/Drive");
        assert_ne!(id, new_id);
        let s = import(&mut other, &destination, &path).unwrap();
        assert_eq!((s.notes, s.images, s.skipped, s.genres), (1, 3, 0, 1));
        let imported_source: (String, String) = other.query_row("SELECT provider,album_url FROM cover_sources
            JOIN albums ON albums.cover_file=cover_sources.file WHERE albums.id='artist1'", [],
            |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(imported_source, ("TheAudioDB".into(), "https://www.theaudiodb.com/album/123".into()));
        let book = genres::Book::load(&other).unwrap();
        assert_eq!(book.map.get("dance and dj").map(String::as_str), Some("Electronic"));
        let chosen: (String, String) = other.query_row("SELECT genre, source FROM album_genre WHERE album_id=?1",
            [&new_id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(chosen, ("Modern Classical".into(), "user".into()));
        assert_eq!(db::research(&other, &new_id).unwrap().as_deref(), Some("Label notes"));
        assert!(db::atmospheres(&other, &new_id, "cel").unwrap()[0].contains("Midnight Cel"));
        assert_eq!(
            db::note(&other, &new_id).unwrap().as_deref(),
            Some("An article")
        );
        let file = db::atmospheres(&other, &new_id, "auto")
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(other.query_row("SELECT prompt FROM painting_prompts WHERE file=?1", [&file],
            |r| r.get::<_,String>(0)).unwrap(), "An open clearing");
        assert_eq!(
            fs::read(destination.join(file)).unwrap(),
            fs::read(source.join("atmos/painting.jpg")).unwrap()
        );
        assert_eq!(
            db::get_setting(&other, &format!("atmos_auto_style.{new_id}"))
                .unwrap()
                .as_deref(),
            Some("Ink")
        );
        let s = import(&mut other, &destination, &path).unwrap();
        assert_eq!((s.notes, s.images), (0, 0));
    }
}

fn read_entry(zip: &mut ZipArchive<File>, name: &str, limit: u64) -> R<Vec<u8>> {
    let entry = zip.by_name(name).map_err(err)?;
    if entry.size() > limit {
        return Err(format!("{name} exceeds the import size limit"));
    }
    let mut bytes = Vec::new();
    entry.take(limit + 1).read_to_end(&mut bytes).map_err(err)?;
    if bytes.len() as u64 > limit {
        return Err(format!("{name} exceeds the import size limit"));
    }
    Ok(bytes)
}

#[derive(Default)]
struct NewFiles(Vec<PathBuf>);

impl NewFiles {
    fn write(&mut self, dir: &Path, name: &str, data: &[u8]) -> R<()> {
        fs::create_dir_all(dir).map_err(err)?;
        let path = dir.join(name);
        let mut out = File::create_new(&path).map_err(err)?;
        self.0.push(path);
        out.write_all(data).map_err(err)
    }
}
impl Drop for NewFiles {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

fn jpeg(zip: &mut ZipArchive<File>, index: usize) -> R<Vec<u8>> {
    let bytes = read_entry(zip, &media_name(index), 64 * 1024 * 1024)?;
    if image::guess_format(&bytes).ok() != Some(image::ImageFormat::Jpeg) {
        return Err("Invalid JPEG in archive".into());
    }
    Ok(bytes)
}

pub fn import(c: &mut Connection, dir: &Path, src: &Path) -> R<Summary> {
    let mut zip = ZipArchive::new(File::open(src).map_err(err)?).map_err(err)?;
    let manifest: Manifest =
        serde_json::from_slice(&read_entry(&mut zip, "muso.json", 128 * 1024 * 1024)?)
            .map_err(err)?;
    if manifest.version != 1 {
        return Err("Unsupported Muso archive version".into());
    }
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(err)?;
    // Local genre choices win; the archive only fills gaps.
    if let Some(book) = manifest.genres {
        for g in genres::validate(&book.genres)? {
            tx.execute(
                "INSERT OR IGNORE INTO genres(name, parent, pos) VALUES(?1, ?2, (SELECT COALESCE(MAX(pos), -1) + 1 FROM genres))",
                rusqlite::params![g.name, g.parent],
            )
            .map_err(err)?;
        }
        tx.execute("UPDATE genres SET parent=NULL WHERE parent IS NOT NULL
            AND parent NOT IN (SELECT name FROM genres WHERE parent IS NULL)", []).map_err(err)?;
        for (tag, genre) in book.map {
            tx.execute("INSERT OR IGNORE INTO genre_map(tag, genre) SELECT ?1, name FROM genres WHERE name=?2",
                [genres::normalize(&tag), genre]).map_err(err)?;
        }
    }
    let library = db::albums(&tx).map_err(err)?;
    let local: HashMap<_, _> = library.iter().map(|a| (a.album.id.as_str(), a)).collect();
    let mut summary = Summary::default();
    let mut new_files = NewFiles::default();
    let mut serial = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(err)?
        .as_nanos();
    for e in manifest.albums {
        let record = local.get(e.id.as_str()).copied().or_else(|| {
            if e.members.is_empty() {
                return None;
            }
            let mut matches = library.iter().filter(|a| {
                a.members == e.members
                    && a.album.title.eq_ignore_ascii_case(&e.title)
                    && (a.album.artist.eq_ignore_ascii_case(&e.artist) || e.artist.eq_ignore_ascii_case("Various Artists"))
            });
            let first = matches.next()?;
            matches.next().is_none().then_some(first)
        });
        let Some(record) = record else {
            summary.skipped += 1;
            continue;
        };
        let a = &record.album;
        let id = &a.id;
        if let Some(brief) = e.research.filter(|s| !s.trim().is_empty()) {
            tx.execute("INSERT OR IGNORE INTO album_research(album_id,brief) VALUES(?1,?2)", (id, brief)).map_err(err)?;
        }
        if let Some(note) = e.note.filter(|n| !n.trim().is_empty()) {
            summary.notes += tx
                .execute(
                    "INSERT OR IGNORE INTO album_notes(album_id,body) VALUES(?1,?2)",
                    [id, &note],
                )
                .map_err(err)?;
        }
        for (style, indices) in e.paintings {
            if indices.is_empty() {
                continue;
            }
            if paintings::style_label(&style).is_none() && style != "auto" {
                return Err("Unknown painting style".into());
            }
            if !db::atmospheres(&tx, id, &style).map_err(err)?.is_empty() {
                continue;
            }
            let label = paintings::style_label(&style)
                .or(e.auto_style.as_deref())
                .unwrap_or("AI choice");
            let mut files = Vec::new();
            for (n, index) in indices.into_iter().enumerate() {
                let bytes = jpeg(&mut zip, index)?;
                crate::covers::decode(&bytes)?;
                serial += 1;
                let name = paintings::filename(&a.title, label, n + 1, serial);
                new_files.write(&dir.join("atmos"), &name, &bytes)?;
                let file = format!("atmos/{name}");
                if let Some(prompt) = e.prompts.get(&index) {
                    tx.execute("INSERT INTO painting_prompts(file,prompt) VALUES(?1,?2)", (&file, prompt)).map_err(err)?;
                }
                files.push(file);
            }
            db::replace_atmospheres(&tx, id, &style, &files).map_err(err)?;
            if style == "auto" {
                db::set_setting(&tx, &format!("atmos_auto_style.{id}"), label).map_err(err)?;
            }
            summary.images += files.len();
        }
        if let Some(index) = e.cover {
            let bytes = jpeg(&mut zip, index)?;
            let img = crate::covers::decode(&bytes)?;
            let old = a.cover_file.clone().unwrap_or_else(|| format!("{id}.jpg"));
            let existing = image::image_dimensions(dir.join("art").join(old)).ok();
            let better = existing.is_none_or(|(w, h)| {
                img.width() >= w && img.height() >= h && (img.width() > w || img.height() > h)
            });
            if better {
                serial += 1;
                let name = format!("{id}_import{serial:x}.jpg");
                let mut thumb = Vec::new();
                img.thumbnail(560, 560)
                    .write_to(
                        &mut std::io::Cursor::new(&mut thumb),
                        image::ImageFormat::Jpeg,
                    )
                    .map_err(err)?;
                new_files.write(&dir.join("art"), &name, &bytes)?;
                new_files.write(&dir.join("thumb"), &name, &thumb)?;
                tx.execute(
                    "UPDATE albums SET has_art=1,cover_file=?2 WHERE id=?1",
                    [id, &name],
                )
                .map_err(err)?;
                if let Some(source) = e.cover_source { source.save(&tx, &name).map_err(err)?; }
                summary.images += 1;
            }
        }
        if let Some(choice) = e.genre.filter(|g| !g.genre.trim().is_empty()) {
            let local: Option<String> = tx
                .query_row("SELECT source FROM album_genre WHERE album_id=?1", [id], |r| r.get(0))
                .optional()
                .map_err(err)?;
            let source = if choice.source == "user" { "user" } else { "ai" };
            if local.is_none() || (local.as_deref() == Some("ai") && source == "user") {
                let name = genres::append_genre(&tx, &choice.genre, None).map_err(err)?;
                tx.execute(
                    "INSERT INTO album_genre(album_id, genre, source) VALUES(?1, ?2, ?3)
                     ON CONFLICT(album_id) DO UPDATE SET genre=excluded.genre, source=excluded.source",
                    [id.as_str(), name.as_str(), source],
                )
                .map_err(err)?;
                summary.genres += 1;
            }
        }
        summary.albums += 1;
    }
    tx.commit().map_err(err)?;
    new_files.0.clear();
    Ok(summary)
}
