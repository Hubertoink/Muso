//! A shared folder contains immutable JPEG/JSON pairs and permanent deletion markers.
//! Each computer keeps its own album matches and rebuildable playback index in SQLite.
use crate::{db, err, paintings, R};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;
use uuid::Uuid;

const MARKER: &str = ".muso-atmospheres.json";
const FOLDER: &str = "shared_atmos.folder";
const STORE_ID: &str = "shared_atmos.id";

pub fn schema(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS shared_paintings(
        file TEXT PRIMARY KEY, album_id TEXT NOT NULL, style TEXT NOT NULL, series TEXT NOT NULL,
        stamp INTEGER NOT NULL, idx INTEGER NOT NULL, metadata TEXT NOT NULL, modified TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS shared_album ON shared_paintings(album_id,style,stamp,series,idx);
        CREATE TABLE IF NOT EXISTS shared_album_matches(store TEXT, source TEXT, album_id TEXT, PRIMARY KEY(store,source));
        CREATE TEMP VIEW active_paintings AS
        SELECT album_id,style,file,0 AS stamp,'' AS series,idx FROM atmosphere_variants
        WHERE NOT EXISTS(SELECT 1 FROM settings WHERE key='shared_atmos.folder')
        UNION ALL SELECT album_id,'cinematic',file,0,'',idx FROM atmospheres
        WHERE NOT EXISTS(SELECT 1 FROM settings WHERE key='shared_atmos.folder')
        UNION ALL SELECT album_id,style,file,stamp,series,idx FROM shared_paintings
        WHERE EXISTS(SELECT 1 FROM settings WHERE key='shared_atmos.folder');")
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AlbumKey {
    pub id: String,
    pub artist: String,
    pub title: String,
    pub members: Vec<String>,
}
impl From<&db::LibraryAlbum> for AlbumKey {
    fn from(a: &db::LibraryAlbum) -> Self {
        Self {
            id: a.album.id.clone(),
            artist: a.album.artist.clone(),
            title: a.album.title.clone(),
            members: a.members.clone(),
        }
    }
}

pub struct AlbumCatalog<'a> {
    by_id: HashMap<&'a str, &'a db::LibraryAlbum>,
    by_members: HashMap<&'a [String], Vec<&'a db::LibraryAlbum>>,
}
impl<'a> AlbumCatalog<'a> {
    pub fn new(library: &'a [db::LibraryAlbum]) -> Self {
        let mut by_members = HashMap::<_, Vec<_>>::new();
        for a in library.iter().filter(|a| !a.members.is_empty()) {
            by_members.entry(a.members.as_slice()).or_default().push(a);
        }
        Self { by_id: library.iter().map(|a| (a.album.id.as_str(), a)).collect(), by_members }
    }
    pub fn candidates(&self, key: &AlbumKey) -> Vec<&'a db::LibraryAlbum> {
        if let Some(exact) = self.by_id.get(key.id.as_str()) {
            return vec![*exact];
        }
        self.by_members
            .get(key.members.as_slice())
            .into_iter()
            .flatten()
            .copied()
            .filter(|a| {
                a.album.title.eq_ignore_ascii_case(&key.title)
                    && (a.album.artist.eq_ignore_ascii_case(&key.artist)
                        || key.artist.eq_ignore_ascii_case("Various Artists"))
            })
            .collect()
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Series {
    pub id: String,
    pub created: i64,
    pub style: String,
    pub label: String,
    pub technique: Option<String>,
}
impl Series {
    pub fn new(style: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            created: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64,
            style: style.into(),
            label: String::new(),
            technique: None,
        }
    }
}
#[derive(Serialize, Deserialize)]
pub struct Painting {
    version: u32,
    id: String,
    album: AlbumKey,
    pub series: Series,
    position: usize,
    pub prompt: Option<String>,
    bytes: u64,
    sha256: String,
}
fn digest(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}
fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}
fn rel(id: &str) -> String {
    format!("atmos/muso-{id}.jpg")
}
fn owned_id(file: &str, suffix: &str) -> Option<String> {
    file.strip_prefix("muso-")?.strip_suffix(suffix).filter(|s| valid_id(s)).map(str::to_owned)
}
fn checksum(path: &Path) -> R<String> {
    let mut hash = Sha256::new();
    std::io::copy(&mut File::open(path).map_err(err)?, &mut hash).map_err(err)?;
    Ok(format!("{:x}", hash.finalize()))
}
fn json_file<T: serde::de::DeserializeOwned>(path: &Path) -> R<T> {
    let meta = fs::symlink_metadata(path).map_err(err)?;
    if !meta.is_file() || meta.len() > 1024 * 1024 {
        return Err("Invalid Muso metadata file.".into());
    }
    serde_json::from_reader(File::open(path).map_err(err)?.take(1024 * 1024)).map_err(err)
}
fn persist(temp: NamedTempFile, path: &Path) -> R<bool> {
    temp.as_file().sync_all().map_err(err)?;
    match temp.persist_noclobber(path) {
        Ok(_) => Ok(true),
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(err(e)),
    }
}
fn write_once(path: &Path, bytes: &[u8]) -> R<bool> {
    let mut temp = NamedTempFile::new_in(path.parent().ok_or("Missing storage folder")?).map_err(err)?;
    temp.write_all(bytes).map_err(err)?;
    persist(temp, path)
}

#[derive(Serialize, Deserialize)]
struct Header {
    version: u32,
    id: Uuid,
}
#[derive(Clone, PartialEq)]
pub struct Store {
    pub folder: PathBuf,
    id: String,
}
impl Store {
    pub fn active(c: &Connection) -> rusqlite::Result<Option<Self>> {
        Ok(db::get_setting(c, FOLDER)?
            .map(PathBuf::from)
            .zip(db::get_setting(c, STORE_ID)?)
            .map(|(folder, id)| Self { folder, id }))
    }
    fn open(folder: PathBuf, create: bool) -> R<Self> {
        let marker = folder.join(MARKER);
        if create && !marker.try_exists().map_err(err)? {
            for entry in fs::read_dir(&folder).map_err(err)? {
                if entry.map_err(err)?.file_name().to_string_lossy().starts_with("muso-") {
                    return Err(
                        "This folder contains Muso files. Wait for its folder metadata to finish syncing.".into(),
                    );
                }
            }
            write_once(&marker, &serde_json::to_vec(&Header { version: 1, id: Uuid::new_v4() }).map_err(err)?)?;
        }
        let header: Header = json_file(&marker).map_err(|_| {
            "Shared Atmosphere folder unavailable or still syncing. Choose a folder created by Muso.".to_string()
        })?;
        if header.version != 1 {
            return Err("This Atmosphere folder requires a newer version of Muso.".into());
        }
        Ok(Self { folder, id: header.id.to_string() })
    }
    pub fn check(&self) -> R<()> {
        if Self::open(self.folder.clone(), false)?.id != self.id {
            return Err("A different Atmosphere folder is mounted here.".into());
        }
        Ok(())
    }
    pub fn path(&self, file: &str) -> R<PathBuf> {
        let name = file.strip_prefix("atmos/").ok_or("Invalid shared image")?;
        owned_id(name, ".jpg").ok_or("Invalid shared image name")?;
        let path = self.folder.join(name);
        if !regular(&path)? {
            return Err("Shared atmosphere image unavailable.".into());
        }
        Ok(path)
    }
    fn side(&self, id: &str, ext: &str) -> PathBuf {
        self.folder.join(format!("muso-{id}.{ext}"))
    }

    pub fn index_published(&self, c: &Connection, p: &Painting, album: &str) -> R<()> {
        if self.side(&p.id, "deleted").try_exists().map_err(err)? {
            return Ok(());
        }
        let stamp = format!("{}/{}", modified(&self.side(&p.id, "jpg"))?, modified(&self.side(&p.id, "json"))?);
        index(c, &self.read_painting(&p.id)?, album, &stamp)
    }

    // Publish the verified image first. A remote client may still receive the files in either order.
    pub fn publish(
        &self,
        source: &Path,
        album: &AlbumKey,
        series: &Series,
        position: usize,
        prompt: Option<String>,
        identity: &str,
    ) -> R<(Painting, Vec<PathBuf>)> {
        self.check()?;
        let sha256 = checksum(source)?;
        let id = digest(&serde_json::to_vec(&(&album.id, &series.style, identity, &sha256)).map_err(err)?);
        let meta = Painting {
            version: 1,
            id,
            album: album.clone(),
            series: series.clone(),
            position,
            prompt,
            bytes: source.metadata().map_err(err)?.len(),
            sha256,
        };
        let created = self.copy_pair(source, &meta)?;
        Ok((meta, created))
    }
    fn copy_pair(&self, source: &Path, meta: &Painting) -> R<Vec<PathBuf>> {
        let mut created = crate::archive::NewFiles::default();
        if self.side(&meta.id, "deleted").try_exists().map_err(err)? {
            return Ok(Vec::new());
        }
        let image = self.side(&meta.id, "jpg");
        if !image.try_exists().map_err(err)? {
            let mut temp = NamedTempFile::new_in(&self.folder).map_err(err)?;
            std::io::copy(&mut File::open(source).map_err(err)?, &mut temp).map_err(err)?;
            if persist(temp, &image)? {
                created.0.push(image.clone());
            }
        }
        if !regular(&image)? || checksum(&image)? != meta.sha256 {
            return Err("Shared image verification failed.".into());
        }
        let side = self.side(&meta.id, "json");
        if write_once(&side, &serde_json::to_vec(meta).map_err(err)?)? {
            created.0.push(side);
        }
        let saved = self.read_painting(&meta.id)?;
        if saved.sha256 != meta.sha256 || (meta.prompt.is_some() && saved.prompt != meta.prompt) {
            return Err("Shared metadata verification failed.".into());
        }
        Ok(std::mem::take(&mut created.0))
    }
    fn read_painting(&self, id: &str) -> R<Painting> {
        let p: Painting = json_file(&self.side(id, "json"))?;
        if p.version != 1
            || p.id != id
            || !valid_id(&p.sha256)
            || p.bytes == 0
            || (p.series.style != "auto" && paintings::style_label(&p.series.style).is_none())
        {
            return Err(format!("Invalid metadata in muso-{id}.json"));
        }
        Ok(p)
    }
    pub fn delete(&self, c: &Connection, album: &str, style: &str, file: &str) -> R<()> {
        self.check()?;
        let found: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM shared_paintings WHERE file=?1 AND album_id=?2 AND style=?3)",
                (file, album, style),
                |r| r.get(0),
            )
            .map_err(err)?;
        if !found {
            return Err("This atmosphere is no longer in the selected album.".into());
        }
        let path = self.path(file)?;
        write_once(&path.with_extension("deleted"), b"1\n")?;
        c.execute("DELETE FROM shared_paintings WHERE file=?1", [file]).map_err(err)?;
        for p in [path.with_extension("json"), path] {
            if regular(&p)? {
                fs::remove_file(p).map_err(err)?;
            }
        }
        Ok(())
    }
}
fn regular(path: &Path) -> R<bool> {
    match fs::symlink_metadata(path) {
        Ok(m) => Ok(m.is_file()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(err(e)),
    }
}
fn modified(path: &Path) -> R<String> {
    let m = fs::symlink_metadata(path).map_err(err)?;
    if !m.is_file() {
        return Err("Expected a regular Atmosphere file.".into());
    }
    Ok(format!(
        "{}-{}",
        m.len(),
        m.modified().map_err(err)?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
    ))
}

pub fn lookup(c: &Connection, file: &str) -> rusqlite::Result<Option<Painting>> {
    c.query_row("SELECT metadata FROM shared_paintings WHERE file=?1", [file], |row| {
        serde_json::from_str(&row.get::<_, String>(0)?)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))
    })
    .optional()
}
fn index(c: &Connection, p: &Painting, album: &str, modified: &str) -> R<()> {
    c.execute(
        "INSERT OR REPLACE INTO shared_paintings VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            rel(&p.id),
            album,
            p.series.style,
            p.series.id,
            p.series.created,
            p.position,
            serde_json::to_string(p).map_err(err)?,
            modified
        ],
    )
    .map_err(err)?;
    Ok(())
}
pub fn auto_label(c: &Connection, album: &str) -> rusqlite::Result<Option<String>> {
    if Store::active(c)?.is_none() {
        return db::get_setting(c, &format!("atmos_auto_style.{album}"));
    }
    c.query_row(
        "SELECT json_extract(metadata,'$.series.label') FROM shared_paintings WHERE album_id=?1 AND style='auto'
        ORDER BY stamp DESC,series DESC,idx DESC LIMIT 1",
        [album],
        |r| r.get(0),
    )
    .optional()
}
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub folder: Option<PathBuf>,
    pub error: Option<String>,
    pub pending: usize,
    pub unmatched: usize,
    pub conflicts: BTreeMap<String, Conflict>,
    pub changed: BTreeSet<String>,
}
#[derive(Serialize)]
pub struct Conflict {
    title: String,
    choices: Vec<(String, String)>,
}

impl Status {
    pub fn pause_automatic(&self, album: &str) -> bool {
        self.pending > 0
            || self.error.is_some()
            || self.conflicts.values().any(|conflict| conflict.choices.iter().any(|(id, _)| id == album))
    }
}

fn inventory(store: &Store) -> R<(BTreeSet<String>, BTreeSet<String>, BTreeSet<String>)> {
    store.check()?;
    let (mut metas, mut images, mut deleted) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for entry in fs::read_dir(&store.folder).map_err(err)? {
        let entry = entry.map_err(err)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(id) = owned_id(&name, ".json") {
            metas.insert(id);
        } else if let Some(id) = owned_id(&name, ".jpg") {
            images.insert(id);
        } else if let Some(id) = owned_id(&name, ".deleted") {
            deleted.insert(id);
        }
    }
    Ok((metas, images, deleted))
}

fn reconcile(c: &Connection, store: &Store) -> R<Status> {
    let mut status = Status { folder: Some(store.folder.clone()), ..Default::default() };
    let (metas, images, deleted) = inventory(store)?;
    let library = db::albums(c).map_err(err)?;
    let catalog = AlbumCatalog::new(&library);
    let mut existing: HashMap<String, (String, String, String)> = c
        .prepare("SELECT file,album_id,modified,metadata FROM shared_paintings")
        .map_err(err)?
        .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))
        .map_err(err)?
        .collect::<rusqlite::Result<_>>()
        .map_err(err)?;
    status.pending = images.symmetric_difference(&metas).filter(|id| !deleted.contains(*id)).count();
    for id in metas.intersection(&images).filter(|id| !deleted.contains(*id)) {
        let file = rel(id);
        let prior = existing.remove(&file);
        let result = (|| -> R<()> {
            let path = store.side(id, "jpg");
            let stamp = format!("{}/{}", modified(&path)?, modified(&store.side(id, "json"))?);
            let painting: Painting = match prior.as_ref().filter(|(_, s, _)| s == &stamp) {
                Some((_, _, json)) => serde_json::from_str(json).map_err(err)?,
                None => {
                    let p = store.read_painting(id)?;
                    if path.metadata().map_err(err)?.len() != p.bytes || checksum(&path)? != p.sha256 {
                        return Err(format!("Waiting for a complete image: {}", p.album.title));
                    }
                    p
                }
            };
            let choices = catalog.candidates(&painting.album);
            let selected = match choices.as_slice() {
                [one] => Some(one.album.id.clone()),
                [] => {
                    status.unmatched += 1;
                    None
                }
                many => {
                    let saved: Option<String> = c
                        .query_row(
                            "SELECT album_id FROM shared_album_matches WHERE store=?1 AND source=?2",
                            (&store.id, &painting.album.id),
                            |r| r.get(0),
                        )
                        .optional()
                        .map_err(err)?;
                    if let Some(a) = saved.filter(|a| many.iter().any(|m| &m.album.id == a)) {
                        Some(a)
                    } else {
                        if !status.conflicts.contains_key(&painting.album.id) {
                            let mut options = Vec::new();
                            for a in many {
                                let tracks = db::tracks(c, &a.album.id).map_err(err)?;
                                let folder = tracks
                                    .first()
                                    .and_then(|t| Path::new(&t.path).parent())
                                    .unwrap_or(Path::new(""));
                                options.push((
                                    a.album.id.clone(),
                                    format!("{} · {}", a.album.title, folder.display()),
                                ));
                            }
                            status.conflicts.insert(
                                painting.album.id.clone(),
                                Conflict {
                                    title: format!("{} · {}", painting.album.artist, painting.album.title),
                                    choices: options,
                                },
                            );
                        }
                        None
                    }
                }
            }
            .unwrap_or_default();
            if !prior.as_ref().is_some_and(|(album, s, _)| album == &selected && s == &stamp) {
                index(c, &painting, &selected, &stamp)?;
                status.changed.insert(selected);
                if let Some((album, _, _)) = &prior {
                    status.changed.insert(album.clone());
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            status.pending += 1;
            status.error.get_or_insert(e);
            if let Some((album, _, _)) = prior {
                c.execute("DELETE FROM shared_paintings WHERE file=?1", [&file]).map_err(err)?;
                status.changed.insert(album);
            }
        }
    }
    for (file, (album, _, _)) in existing {
        c.execute("DELETE FROM shared_paintings WHERE file=?1", [file]).map_err(err)?;
        status.changed.insert(album);
    }
    status.changed.remove("");
    Ok(status)
}

pub fn sync(c: &mut Connection) -> R<Status> {
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(err)?;
    let Some(store) = Store::active(&tx).map_err(err)? else { return Ok(Status::default()) };
    let status = reconcile(&tx, &store)?;
    tx.commit().map_err(err)?;
    Ok(status)
}

fn copy_local(c: &Connection, data: &Path, dest: &Store) -> R<()> {
    let catalog = db::albums(c).map_err(err)?;
    let mut collections =
        c.prepare("SELECT album_id,style FROM active_paintings GROUP BY album_id,style").map_err(err)?;
    let pairs =
        collections.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(err)?;
    for pair in pairs {
        let (id, style) = pair.map_err(err)?;
        // A disconnected music drive can leave saved paintings whose album has not been scanned again.
        let key = catalog.iter().find(|a| a.album.id == id).map(AlbumKey::from).unwrap_or_else(|| AlbumKey {
            id: id.clone(),
            artist: String::new(),
            title: String::new(),
            members: Vec::new(),
        });
        let files = db::atmospheres(c, &id, &style).map_err(err)?;
        let mut series = Series::new(&style);
        series.label = paintings::style_label(&style)
            .map(str::to_owned)
            .or(auto_label(c, &id).map_err(err)?)
            .unwrap_or_else(|| "AI choice".into());
        for (i, file) in files.iter().enumerate() {
            let name = file.strip_prefix("atmos/").ok_or("Invalid local image path")?;
            if !paintings::valid_filename(name) {
                return Err("Invalid local image filename".into());
            }
            let marker: Option<(String, Option<String>)> = c
                .query_row(
                    "SELECT label,technique FROM auto_style_choices
                WHERE first_file=?1 ORDER BY seq DESC LIMIT 1",
                    [file],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(err)?;
            if i == 0 || marker.is_some() {
                series.id = digest(file.as_bytes());
                series.created = i as i64;
            }
            if let Some((label, technique)) = marker.filter(|_| style == "auto") {
                series.label = label;
                series.technique = technique;
            }
            let prompt = c
                .query_row("SELECT prompt FROM painting_prompts WHERE file=?1", [file], |r| r.get(0))
                .optional()
                .map_err(err)?;
            dest.publish(&data.join(file), &key, &series, i, prompt, file)?;
        }
    }
    Ok(())
}

pub fn select(c: &mut Connection, data: &Path, folder: Option<PathBuf>, copy: bool) -> R<Status> {
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(err)?;
    let old = Store::active(&tx).map_err(err)?;
    if let Some(folder) = folder {
        if fs::canonicalize(&folder).map_err(err)?.starts_with(fs::canonicalize(data).map_err(err)?) {
            return Err("Choose a folder outside Muso’s app data.".into());
        }
        let dest = Store::open(folder, copy)?;
        if copy && old.as_ref() != Some(&dest) {
            if let Some(old) = &old {
                let (metas, images, deleted) = inventory(old)?;
                if metas.symmetric_difference(&images).any(|id| !deleted.contains(id)) {
                    return Err(
                        "Wait for the current Atmosphere folder to finish syncing before moving it.".into()
                    );
                }
                for id in &deleted {
                    write_once(&dest.side(id, "deleted"), b"1\n")?;
                }
                for id in metas.difference(&deleted) {
                    dest.copy_pair(&old.side(id, "jpg"), &old.read_painting(id)?)?;
                }
                tx.execute(
                    "INSERT OR IGNORE INTO shared_album_matches SELECT ?1,source,album_id
                    FROM shared_album_matches WHERE store=?2",
                    (&dest.id, &old.id),
                )
                .map_err(err)?;
            } else {
                copy_local(&tx, data, &dest)?;
            }
        }
        if old.as_ref() != Some(&dest) {
            tx.execute("DELETE FROM shared_paintings", []).map_err(err)?;
        }
        db::set_setting(&tx, FOLDER, dest.folder.to_str().ok_or("Invalid folder name")?).map_err(err)?;
        db::set_setting(&tx, STORE_ID, &dest.id).map_err(err)?;
        let status = reconcile(&tx, &dest)?;
        tx.commit().map_err(err)?;
        Ok(status)
    } else {
        tx.execute("DELETE FROM settings WHERE key IN (?1,?2)", [FOLDER, STORE_ID]).map_err(err)?;
        tx.execute("DELETE FROM shared_paintings", []).map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(Status::default())
    }
}

#[tauri::command]
pub async fn atmosphere_storage(
    app: tauri::AppHandle,
    action: Option<String>,
    source: Option<String>,
    album: Option<String>,
) -> R<Status> {
    use tauri::Manager;
    use tauri_plugin_dialog::DialogExt;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<crate::AppState>();
        let action = action.as_deref().unwrap_or("sync");
        let folder = if matches!(action, "move" | "use") {
            app.dialog()
                .file()
                .set_title("Choose the shared Atmosphere folder")
                .blocking_pick_folder()
                .map(|p| p.into_path().map_err(err))
                .transpose()?
        } else {
            None
        };
        let mut c = state.storage_db.lock().map_err(err)?;
        if action == "local" || folder.is_some() {
            let _guard = state
                .storage_gate
                .try_write()
                .map_err(|_| "Wait for the current generation to finish before switching folders.")?;
            return select(&mut c, &state.data_dir, folder, action == "move");
        }
        if action == "match" {
            let store = Store::active(&c).map_err(err)?.ok_or("No shared Atmosphere folder selected")?;
            let (source, album) = (source.ok_or("Missing source album")?, album.ok_or("Missing local album")?);
            c.execute("INSERT OR REPLACE INTO shared_album_matches VALUES(?1,?2,?3)", (&store.id, source, album))
                .map_err(err)?;
        }
        let store = Store::active(&c).map_err(err)?;
        match sync(&mut c) {
            Ok(status) => Ok(status),
            Err(error) => Ok(Status { folder: store.map(|s| s.folder), error: Some(error), ..Default::default() }),
        }
    })
    .await
    .map_err(err)?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn library(data: &Path, music: &str, editions: usize) -> (Connection, String) {
        fs::create_dir_all(data.join("atmos")).unwrap();
        let mut c = db::open(&data.join("muso.db")).unwrap();
        for artist in ["guest", "band"] {
            c.execute("INSERT INTO albums(id,artist,title,sort_key) VALUES(?1,?1,'Concert',?1)", [artist])
                .unwrap();
            for n in 0..editions {
                c.execute(
                    "INSERT INTO tracks(path,album_id,title,mtime) VALUES(?1,?2,'Track',0)",
                    (format!("{music}/edition{n}/{artist}.mp3"), artist),
                )
                .unwrap();
            }
        }
        let tx = c.transaction().unwrap();
        crate::releases::rebuild(&tx).unwrap();
        tx.commit().unwrap();
        let id = db::albums(&c).unwrap().into_iter().find(|a| !a.members.is_empty()).unwrap().album.id;
        (c, id)
    }
    fn put_local(c: &Connection, data: &Path, album: &str) {
        image::RgbImage::new(12, 16).save(data.join("atmos/first.jpg")).unwrap();
        db::replace_atmospheres(c, album, "auto", &["atmos/first.jpg".into()]).unwrap();
        c.execute("INSERT INTO painting_prompts VALUES('atmos/first.jpg','Paint the evening light.')", [])
            .unwrap();
        crate::style_history::record(c, "atmos/first.jpg", "Ink", Some("ink"), false).unwrap();
    }
    fn replicate(from: &Path, to: &Path) {
        for entry in fs::read_dir(from).unwrap() {
            let e = entry.unwrap();
            fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
    #[test]
    fn two_music_roots_merge_series_and_honor_deletions_after_an_offline_period() {
        let temp = tempfile::tempdir().unwrap();
        let (a, b) = (temp.path().join("a"), temp.path().join("b"));
        let (mut ca, aid) = library(&a, "C:/music", 1);
        let (mut cb, bid) = library(&b, "D:/records", 1);
        assert_ne!(aid, bid);
        let (one, two) = (temp.path().join("OneDrive A"), temp.path().join("OneDrive B"));
        for d in [&one, &two] {
            fs::create_dir(d).unwrap();
        }
        put_local(&ca, &a, &aid);
        select(&mut ca, &a, Some(one.clone()), true).unwrap();
        let shared = Store::active(&ca).unwrap().unwrap();
        let first = db::atmospheres(&ca, &aid, "auto").unwrap().remove(0);
        let meta = lookup(&ca, &first).unwrap().unwrap();
        assert_eq!(meta.prompt.as_deref(), Some("Paint the evening light."));
        assert_eq!(meta.series.label, "Ink");
        // Metadata arrives before the full JPEG.
        fs::copy(one.join(MARKER), two.join(MARKER)).unwrap();
        fs::copy(shared.side(&meta.id, "json"), two.join(format!("muso-{}.json", meta.id))).unwrap();
        assert_eq!(select(&mut cb, &b, Some(two.clone()), false).unwrap().pending, 1);
        assert!(!db::has_atmospheres(&cb, &bid).unwrap());
        let second = Store::active(&cb).unwrap().unwrap();
        fs::write(second.side(&meta.id, "jpg"), b"partial").unwrap();
        assert_eq!(sync(&mut cb).unwrap().pending, 1);
        replicate(&one, &two);
        assert!(sync(&mut cb).unwrap().changed.contains(&bid));
        assert_eq!(db::atmospheres(&cb, &bid, "auto").unwrap(), [first.clone()]);
        assert!(sync(&mut cb).unwrap().changed.is_empty());
        let credit = serde_json::to_value(paintings::credit(&cb, &bid, "auto", &first).unwrap()).unwrap();
        assert_eq!(credit["style"], "Ink");
        assert_eq!(credit["prompt"], "Paint the evening light.");
        // Each machine publishes a new series before receiving the other's files.
        for (c, store, album) in [(&ca, &shared, &aid), (&cb, &second, &bid)] {
            let key = AlbumKey::from(db::albums(c).unwrap().iter().find(|a| &a.album.id == album).unwrap());
            let mut series = Series::new("auto");
            series.label = "Charcoal".into();
            let (p, _) = store
                .publish(&a.join("atmos/first.jpg"), &key, &series, 0, Some("Another view".into()), &series.id)
                .unwrap();
            store.index_published(c, &p, album).unwrap();
        }
        replicate(&one, &two);
        replicate(&two, &one);
        sync(&mut ca).unwrap();
        sync(&mut cb).unwrap();
        let list = db::atmospheres(&ca, &aid, "auto").unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(db::atmospheres(&cb, &bid, "auto").unwrap(), list);
        shared.delete(&ca, &aid, "auto", &first).unwrap();
        fs::rename(&two, temp.path().join("offline")).unwrap();
        assert!(sync(&mut cb).is_err());
        assert_eq!(db::atmospheres(&cb, &bid, "auto").unwrap().len(), 3);
        fs::rename(temp.path().join("offline"), &two).unwrap();
        // Even if the stale JPEG and JSON arrive again, the tombstone wins.
        replicate(&one, &two);
        replicate(&two, &one);
        sync(&mut ca).unwrap();
        sync(&mut cb).unwrap();
        assert_eq!(db::atmospheres(&cb, &bid, "auto").unwrap(), list[1..]);
        let reopened = db::open(&b.join("muso.db")).unwrap();
        assert_eq!(Store::active(&reopened).unwrap().unwrap().folder, two);
        // The normal collection archive reads and writes the selected folder, including prompts.
        let archive = temp.path().join("collection.zip");
        crate::archive::export(&mut cb, &b, &archive).unwrap();
        let third = temp.path().join("third");
        let (mut cc, cid) = library(&third, "Network/music", 1);
        let third_store = temp.path().join("third shared");
        fs::create_dir(&third_store).unwrap();
        select(&mut cc, &third, Some(third_store), true).unwrap();
        crate::archive::import(&mut cc, &third, &archive).unwrap();
        let imported = db::atmospheres(&cc, &cid, "auto").unwrap();
        assert_eq!(imported.len(), 2);
        assert!(imported
            .iter()
            .all(|f| lookup(&cc, f).unwrap().unwrap().prompt.as_deref() == Some("Another view")));
        assert!(sync(&mut cc).unwrap().changed.is_empty());
        select(&mut ca, &a, None, false).unwrap();
        assert_eq!(db::atmospheres(&ca, &aid, "auto").unwrap(), ["atmos/first.jpg"]);
        assert!(a.join("atmos/first.jpg").exists());
    }
    #[test]
    fn ambiguous_editions_need_one_remembered_choice_and_failed_moves_keep_the_source() {
        let temp = tempfile::tempdir().unwrap();
        let (a, b) = (temp.path().join("a"), temp.path().join("b"));
        let (mut ca, aid) = library(&a, "Music", 1);
        let (mut cb, bid) = library(&b, "Albums", 2);
        let dest = temp.path().join("shared");
        fs::create_dir(&dest).unwrap();
        put_local(&ca, &a, &aid);
        fs::rename(a.join("atmos/first.jpg"), a.join("backup.jpg")).unwrap();
        assert!(select(&mut ca, &a, Some(dest.clone()), true).is_err());
        assert!(Store::active(&ca).unwrap().is_none());
        fs::rename(a.join("backup.jpg"), a.join("atmos/first.jpg")).unwrap();
        select(&mut ca, &a, Some(dest.clone()), true).unwrap();
        let status = select(&mut cb, &b, Some(dest.clone()), false).unwrap();
        let conflict = status.conflicts.get(&aid).unwrap();
        assert!(status.pause_automatic(&bid));
        assert!(!status.pause_automatic("some-other-album"));
        assert_eq!(conflict.choices.len(), 2);
        assert!(!db::has_atmospheres(&cb, &bid).unwrap());
        let store = Store::active(&cb).unwrap().unwrap();
        cb.execute("INSERT INTO shared_album_matches VALUES(?1,?2,?3)", (&store.id, &aid, &bid)).unwrap();
        assert!(sync(&mut cb).unwrap().conflicts.is_empty());
        assert!(db::has_atmospheres(&cb, &bid).unwrap());
        let moved = temp.path().join("moved");
        fs::create_dir(&moved).unwrap();
        select(&mut cb, &b, Some(moved.clone()), true).unwrap();
        assert!(db::has_atmospheres(&cb, &bid).unwrap());
        assert!(dest.join(MARKER).is_file() && moved.join(MARKER).is_file());
        assert!(store.path("atmos/../extra.jpg").is_err());
    }
}
