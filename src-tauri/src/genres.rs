use crate::{db, err, AppState, R};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

pub fn schema(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS genres(name TEXT PRIMARY KEY COLLATE NOCASE, parent TEXT, pos INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS genre_map(tag TEXT PRIMARY KEY, genre TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS album_genre(album_id TEXT PRIMARY KEY, genre TEXT NOT NULL, source TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS genre_suggestions(album_id TEXT PRIMARY KEY, genre TEXT NOT NULL, parent TEXT,
            new_genre INTEGER NOT NULL DEFAULT 0, confidence REAL NOT NULL, reason TEXT NOT NULL,
            researched INTEGER NOT NULL DEFAULT 0);",
    )
}

/// Tag spellings that only differ in case, separators or "&" share one key.
pub fn normalize(tag: &str) -> String {
    let lower = tag.to_lowercase().replace('&', " and ");
    lower
        .split(|c: char| c.is_whitespace() || matches!(c, '-' | '_' | '.'))
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Genre {
    pub name: String,
    pub parent: Option<String>,
}

pub struct Book {
    pub genres: Vec<Genre>,
    pub map: HashMap<String, String>,
    overrides: HashMap<String, (String, String)>,
}

impl Book {
    pub fn load(c: &Connection) -> rusqlite::Result<Self> {
        let mut st = c.prepare("SELECT name, parent FROM genres ORDER BY pos, name")?;
        let genres = st
            .query_map([], |r| Ok(Genre { name: r.get(0)?, parent: r.get(1)? }))?
            .collect::<rusqlite::Result<_>>()?;
        let mut st = c.prepare("SELECT tag, genre FROM genre_map")?;
        let map = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut st = c.prepare("SELECT album_id, genre, source FROM album_genre")?;
        let overrides = st
            .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self { genres, map, overrides })
    }

    /// Own choice, then the release's choice, then the tag mapping, then the raw tag.
    fn resolve(&self, id: &str, release: Option<&str>, tag: Option<&str>) -> Assigned {
        let chosen = self.overrides.get(id).or_else(|| release.and_then(|r| self.overrides.get(r)));
        if let Some((genre, source)) = chosen {
            let origin = if source == "user" { "user" } else { "ai" };
            return Assigned { genre: Some(genre.clone()), origin, tag: tag.map(str::to_owned) };
        }
        let tag = tag.map(str::trim).filter(|t| !t.is_empty());
        if let Some(genre) = tag.and_then(|t| self.map.get(&normalize(t))) {
            return Assigned { genre: Some(genre.clone()), origin: "map", tag: tag.map(str::to_owned) };
        }
        if self.genres.is_empty() {
            if let Some(t) = tag {
                return Assigned { genre: Some(t.to_owned()), origin: "tag", tag: Some(t.to_owned()) };
            }
        }
        Assigned { genre: None, origin: "none", tag: tag.map(str::to_owned) }
    }
}

#[derive(Serialize, Clone)]
pub struct Assigned {
    pub genre: Option<String>,
    pub origin: &'static str,
    pub tag: Option<String>,
}

#[derive(Serialize)]
pub struct GenreState {
    genres: Vec<Genre>,
    albums: HashMap<String, Assigned>,
    suggestions: usize,
}

/// The library as records: releases plus albums that do not belong to one.
pub struct Library {
    pub albums: Vec<db::LibraryAlbum>,
    pub release_of: HashMap<String, String>,
}

impl Library {
    pub fn load(c: &Connection) -> rusqlite::Result<Self> {
        let albums = db::albums(c)?;
        let mut release_of = HashMap::new();
        for a in &albums {
            for m in &a.members {
                release_of.entry(m.clone()).or_insert_with(|| a.album.id.clone());
            }
        }
        Ok(Self { albums, release_of })
    }
    pub fn records(&self) -> impl Iterator<Item = &db::LibraryAlbum> {
        self.albums.iter().filter(|a| !self.release_of.contains_key(&a.album.id))
    }
    pub fn assigned(&self, book: &Book, id: &str) -> Option<Assigned> {
        let a = self.albums.iter().find(|a| a.album.id == id)?;
        Some(book.resolve(id, self.release_of.get(id).map(String::as_str), a.album.genre.as_deref()))
    }
}

fn state_of(c: &Connection) -> R<GenreState> {
    let book = Book::load(c).map_err(err)?;
    let lib = Library::load(c).map_err(err)?;
    let albums = lib
        .albums
        .iter()
        .map(|a| {
            let id = &a.album.id;
            (id.clone(), book.resolve(id, lib.release_of.get(id).map(String::as_str), a.album.genre.as_deref()))
        })
        .collect();
    let suggestions = c.query_row("SELECT COUNT(*) FROM genre_suggestions", [], |r| r.get::<_, i64>(0)).map_err(err)?;
    Ok(GenreState { genres: book.genres, albums, suggestions: suggestions as usize })
}

#[tauri::command]
pub fn genre_state(state: State<'_, AppState>) -> R<GenreState> {
    state_of(&*state.db.lock().map_err(err)?)
}

#[derive(Serialize)]
pub struct TagInfo {
    pub key: String,
    pub variants: Vec<String>,
    pub albums: usize,
    pub artists: Vec<String>,
    pub genre: Option<String>,
}

#[derive(Serialize)]
pub struct Inventory {
    pub tags: Vec<TagInfo>,
    pub untagged: usize,
    pub records: usize,
    pub broad: Vec<String>,
}

pub fn inventory(c: &Connection) -> R<Inventory> {
    let book = Book::load(c).map_err(err)?;
    let lib = Library::load(c).map_err(err)?;
    let mut tags: BTreeMap<String, TagInfo> = BTreeMap::new();
    let (mut untagged, mut records) = (0, 0);
    for a in lib.records() {
        records += 1;
        let Some(raw) = a.album.genre.as_deref().map(str::trim).filter(|t| !t.is_empty()) else {
            untagged += 1;
            continue;
        };
        let key = normalize(raw);
        let info = tags.entry(key.clone()).or_insert_with(|| TagInfo {
            genre: book.map.get(&key).cloned(),
            key,
            variants: Vec::new(),
            albums: 0,
            artists: Vec::new(),
        });
        info.albums += 1;
        if !info.variants.iter().any(|v| v == raw) {
            info.variants.push(raw.to_owned());
        }
        if info.artists.len() < 6 && !info.artists.contains(&a.album.artist) {
            info.artists.push(a.album.artist.clone());
        }
    }
    let mut tags: Vec<_> = tags.into_values().collect();
    tags.sort_by(|a, b| b.albums.cmp(&a.albums).then_with(|| a.key.cmp(&b.key)));
    let broad = db::get_setting(c, "genres.broad")
        .map_err(err)?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    Ok(Inventory { tags, untagged, records, broad })
}

#[tauri::command]
pub fn genre_inventory(state: State<'_, AppState>) -> R<Inventory> {
    inventory(&*state.db.lock().map_err(err)?)
}

fn clean_name(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(60).collect()
}

/// Two levels only: a parent must itself be a top-level genre.
pub fn validate(genres: &[Genre]) -> R<Vec<Genre>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for g in genres {
        let name = clean_name(&g.name);
        if name.is_empty() {
            return Err("Every genre needs a name.".into());
        }
        if !seen.insert(name.to_lowercase()) {
            return Err(format!("“{name}” appears twice."));
        }
        out.push(Genre { name, parent: g.parent.as_deref().map(clean_name).filter(|p| !p.is_empty()) });
    }
    let tops: HashSet<String> =
        out.iter().filter(|g| g.parent.is_none()).map(|g| g.name.to_lowercase()).collect();
    for g in &mut out {
        if let Some(p) = &g.parent {
            if p.eq_ignore_ascii_case(&g.name) || !tops.contains(&p.to_lowercase()) {
                g.parent = None;
            } else if let Some(top) = genres.iter().find(|t| t.name.trim().eq_ignore_ascii_case(p)) {
                g.parent = Some(clean_name(&top.name));
            }
        }
    }
    Ok(out)
}

fn canonical<'a>(genres: &'a [Genre], name: &str) -> Option<&'a str> {
    genres.iter().find(|g| g.name.eq_ignore_ascii_case(name.trim())).map(|g| g.name.as_str())
}

pub fn append_genre(c: &Connection, name: &str, parent: Option<&str>) -> rusqlite::Result<String> {
    let name = clean_name(name);
    if let Some(existing) =
        c.query_row("SELECT name FROM genres WHERE name=?1", [&name], |r| r.get::<_, String>(0)).optional()?
    {
        return Ok(existing);
    }
    let parent = match parent {
        Some(p) => c
            .query_row("SELECT name FROM genres WHERE name=?1 AND parent IS NULL", [p], |r| r.get::<_, String>(0))
            .optional()?,
        None => None,
    };
    c.execute(
        "INSERT INTO genres(name, parent, pos) VALUES(?1, ?2, (SELECT COALESCE(MAX(pos), -1) + 1 FROM genres))",
        params![name, parent],
    )?;
    Ok(name)
}

pub fn save_book(c: &mut Connection, genres: &[Genre], map: &BTreeMap<String, String>, renames: &BTreeMap<String, String>) -> R<()> {
    let genres = validate(genres)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(err)?;
    for (old, new) in renames {
        if let Some(new) = canonical(&genres, new) {
            for table in ["album_genre", "genre_suggestions"] {
                tx.execute(&format!("UPDATE {table} SET genre=?2 WHERE genre=?1 COLLATE NOCASE"), [old, new]).map_err(err)?;
            }
        }
    }
    tx.execute("DELETE FROM genres", []).map_err(err)?;
    for (pos, g) in genres.iter().enumerate() {
        tx.execute("INSERT INTO genres(name, parent, pos) VALUES(?1, ?2, ?3)", params![g.name, g.parent, pos as i64])
            .map_err(err)?;
    }
    tx.execute("DELETE FROM genre_map", []).map_err(err)?;
    for (tag, genre) in map {
        let key = normalize(tag);
        if key.is_empty() {
            continue;
        }
        let genre = canonical(&genres, genre).ok_or_else(|| format!("Unknown genre “{genre}” in the tag list."))?;
        tx.execute("INSERT OR REPLACE INTO genre_map(tag, genre) VALUES(?1, ?2)", [&key, genre]).map_err(err)?;
    }
    // Your own choices survive a reorganisation; AI choices for removed genres are dropped.
    let orphans: Vec<String> = {
        let mut st = tx
            .prepare("SELECT DISTINCT genre FROM album_genre WHERE source='user' AND genre NOT IN (SELECT name FROM genres)")
            .map_err(err)?;
        let rows = st.query_map([], |r| r.get(0)).map_err(err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(err)?
    };
    for name in orphans {
        append_genre(&tx, &name, None).map_err(err)?;
    }
    tx.execute("DELETE FROM album_genre WHERE genre NOT IN (SELECT name FROM genres)", []).map_err(err)?;
    tx.execute("DELETE FROM genre_suggestions WHERE new_genre=0 AND genre NOT IN (SELECT name FROM genres)", [])
        .map_err(err)?;
    tx.commit().map_err(err)
}

#[tauri::command]
pub fn genre_save(
    state: State<'_, AppState>,
    genres: Vec<Genre>,
    map: BTreeMap<String, String>,
    renames: Option<BTreeMap<String, String>>,
    broad: Option<Vec<String>>,
) -> R<GenreState> {
    let mut c = state.db.lock().map_err(err)?;
    save_book(&mut c, &genres, &map, &renames.unwrap_or_default())?;
    if let Some(broad) = broad {
        db::set_setting(&c, "genres.broad", &serde_json::to_string(&broad).map_err(err)?).map_err(err)?;
    }
    state_of(&c)
}

pub fn set_choice(c: &Connection, album: &str, genre: Option<&str>) -> R<()> {
    match genre.map(str::trim).filter(|g| !g.is_empty()) {
        Some(g) => {
            let name = append_genre(c, g, None).map_err(err)?;
            c.execute(
                "INSERT INTO album_genre(album_id, genre, source) VALUES(?1, ?2, 'user')
                 ON CONFLICT(album_id) DO UPDATE SET genre=excluded.genre, source='user'",
                [album, &name],
            )
            .map_err(err)?;
        }
        None => {
            c.execute("DELETE FROM album_genre WHERE album_id=?1", [album]).map_err(err)?;
        }
    }
    c.execute("DELETE FROM genre_suggestions WHERE album_id=?1", [album]).map_err(err)?;
    Ok(())
}

#[tauri::command]
pub fn set_album_genre(state: State<'_, AppState>, album_id: String, genre: Option<String>) -> R<GenreState> {
    let c = state.db.lock().map_err(err)?;
    set_choice(&c, &album_id, genre.as_deref())?;
    state_of(&c)
}

#[derive(Serialize)]
pub struct Suggestion {
    album_id: String,
    artist: String,
    title: String,
    current: Option<String>,
    tag: Option<String>,
    genre: String,
    parent: Option<String>,
    new_genre: bool,
    confidence: f64,
    reason: String,
    researched: bool,
}

#[tauri::command]
pub fn genre_suggestions(state: State<'_, AppState>) -> R<Vec<Suggestion>> {
    let c = state.db.lock().map_err(err)?;
    let book = Book::load(&c).map_err(err)?;
    let lib = Library::load(&c).map_err(err)?;
    let mut st = c
        .prepare("SELECT album_id, genre, parent, new_genre, confidence, reason, researched FROM genre_suggestions")
        .map_err(err)?;
    let rows = st
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, bool>(3)?,
                r.get::<_, f64>(4)?, r.get::<_, String>(5)?, r.get::<_, bool>(6)?))
        })
        .map_err(err)?;
    let mut out = Vec::new();
    for row in rows {
        let (id, genre, parent, new_genre, confidence, reason, researched) = row.map_err(err)?;
        let Some(a) = lib.albums.iter().find(|a| a.album.id == id) else { continue };
        let now = lib.assigned(&book, &id);
        out.push(Suggestion {
            artist: a.album.artist.clone(),
            title: a.album.title.clone(),
            current: now.as_ref().and_then(|n| n.genre.clone()),
            tag: a.album.genre.clone(),
            album_id: id,
            genre,
            parent,
            new_genre,
            confidence,
            reason,
            researched,
        });
    }
    out.sort_by(|a, b| b.confidence.total_cmp(&a.confidence).then_with(|| a.artist.cmp(&b.artist)));
    Ok(out)
}

#[derive(Deserialize)]
pub struct Accept {
    album_id: String,
    genre: String,
}

pub fn review(c: &mut Connection, accept: &[Accept], reject: &[String]) -> R<()> {
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(err)?;
    for a in accept {
        let suggested: Option<(String, Option<String>)> = tx
            .query_row("SELECT genre, parent FROM genre_suggestions WHERE album_id=?1", [&a.album_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(err)?;
        let same = suggested.as_ref().is_some_and(|(g, _)| g.eq_ignore_ascii_case(a.genre.trim()));
        let parent = suggested.as_ref().filter(|_| same).and_then(|(_, p)| p.as_deref());
        let name = append_genre(&tx, &a.genre, parent).map_err(err)?;
        tx.execute(
            "INSERT INTO album_genre(album_id, genre, source) VALUES(?1, ?2, ?3)
             ON CONFLICT(album_id) DO UPDATE SET genre=excluded.genre, source=excluded.source",
            [a.album_id.as_str(), &name, if same { "ai" } else { "user" }],
        )
        .map_err(err)?;
        tx.execute("DELETE FROM genre_suggestions WHERE album_id=?1", [&a.album_id]).map_err(err)?;
    }
    for id in reject {
        tx.execute("DELETE FROM genre_suggestions WHERE album_id=?1", [id]).map_err(err)?;
    }
    tx.commit().map_err(err)
}

#[tauri::command]
pub fn genre_review(state: State<'_, AppState>, accept: Vec<Accept>, reject: Vec<String>) -> R<GenreState> {
    let mut c = state.db.lock().map_err(err)?;
    review(&mut c, &accept, &reject)?;
    state_of(&c)
}

#[tauri::command]
pub async fn genre_propose(state: State<'_, AppState>) -> R<crate::ai::GenreProposal> {
    let (key, inv, current) = {
        let c = state.db.lock().map_err(err)?;
        let key = crate::openai_key(&c)?.ok_or("Add an OpenAI API key in Settings first.")?;
        (key, inventory(&c)?, Book::load(&c).map_err(err)?.genres)
    };
    if inv.tags.is_empty() {
        return Err("None of your albums has a genre tag yet.".into());
    }
    let mut p = crate::ai::propose_genres(&key, &inv.tags, &current).await?;
    p.genres = validate(&p.genres)?;
    let keys: HashSet<&str> = inv.tags.iter().map(|t| t.key.as_str()).collect();
    let genres = p.genres.clone();
    p.map = p
        .map
        .into_iter()
        .filter_map(|(k, g)| {
            let k = normalize(&k);
            let g = canonical(&genres, &g)?.to_owned();
            keys.contains(k.as_str()).then_some((k, g))
        })
        .collect();
    p.broad.retain(|k| keys.contains(normalize(k).as_str()));
    Ok(p)
}

static SORTING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
const BATCH: usize = 25;

#[derive(Serialize, Clone)]
struct Progress {
    done: usize,
    total: usize,
    stage: &'static str,
    uncertain: usize,
}

#[derive(Default, Serialize)]
pub struct SortResult {
    suggestions: usize,
    uncertain: usize,
    attempted: usize,
}

async fn allow_research(count: usize, confirmation: impl std::future::Future<Output = bool>) -> bool {
    count <= 50 || confirmation.await
}

async fn confirm_research(app: &AppHandle, count: usize) -> bool {
    let Some(window) = app.get_webview_window("main") else { return false };
    let (send, answer) = tokio::sync::oneshot::channel();
    app.dialog().message(format!("{count} albums have an uncertain genre. Looking them up may use up to {count} additional paid web searches. You can skip this step and review the suggestions already saved."))
        .title("Look up uncertain albums?").parent(&window)
        .buttons(MessageDialogButtons::OkCancelCustom(format!("Look up {count} albums"), "Skip web research".into()))
        .show(move |yes| { let _ = send.send(yes); });
    answer.await.unwrap_or(false)
}

#[tauri::command]
pub fn genre_cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

pub struct Candidate {
    pub id: String,
    pub album: db::Album,
    pub current: Option<String>,
    pub tracks: Vec<String>,
    pub notes: Option<String>,
}

fn store(c: &Connection, cand: &Candidate, s: &crate::ai::GenreVerdict, genres: &[Genre], researched: bool) -> R<bool> {
    let (genre, new_genre, parent) = match canonical(genres, &s.genre) {
        Some(g) => (g.to_owned(), false, None),
        None => (clean_name(&s.genre), true, s.parent.as_deref().and_then(|p| canonical(genres, p)).map(str::to_owned)),
    };
    if genre.is_empty() || cand.current.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(&genre)) {
        c.execute("DELETE FROM genre_suggestions WHERE album_id=?1", [&cand.id]).map_err(err)?;
        return Ok(false);
    }
    let reason: String = s.reason.chars().take(200).collect();
    c.execute(
        "INSERT OR REPLACE INTO genre_suggestions(album_id, genre, parent, new_genre, confidence, reason, researched)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![cand.id, genre, parent, new_genre, s.confidence.clamp(0.0, 1.0), reason, researched],
    )
    .map_err(err)?;
    Ok(true)
}

#[tauri::command]
pub async fn genre_classify(app: AppHandle, state: State<'_, AppState>, album_ids: Vec<String>, research: bool, instructions: Option<String>) -> R<SortResult> {
    if SORTING.swap(true, Ordering::SeqCst) {
        return Err("Albums are already being sorted.".into());
    }
    CANCEL.store(false, Ordering::SeqCst);
    let result = classify(&app, &state, album_ids, research, instructions.as_deref().unwrap_or("").trim()).await;
    SORTING.store(false, Ordering::SeqCst);
    result
}

async fn classify(app: &AppHandle, state: &AppState, album_ids: Vec<String>, research: bool, instructions: &str) -> R<SortResult> {
    let (key, genres, candidates) = {
        let c = state.db.lock().map_err(err)?;
        let key = crate::openai_key(&c)?.ok_or("Add an OpenAI API key in Settings first.")?;
        let book = Book::load(&c).map_err(err)?;
        if book.genres.is_empty() {
            return Err("Create your genre list first.".into());
        }
        let lib = Library::load(&c).map_err(err)?;
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        for id in album_ids.into_iter().filter(|id| seen.insert(id.clone())) {
            let Some(a) = lib.albums.iter().find(|a| a.album.id == id) else { continue };
            let current = lib.assigned(&book, &id).and_then(|x| x.genre);
            let tracks = db::tracks(&c, &id).map_err(err)?.into_iter().take(14).map(|t| t.title).collect();
            let notes = db::research(&c, &id)
                .map_err(err)?
                .or(db::note(&c, &id).map_err(err)?)
                .map(|n| n.chars().take(700).collect());
            candidates.push(Candidate { id, album: a.album.clone(), current, tracks, notes });
        }
        (key, book.genres, candidates)
    };
    let total = candidates.len();
    let progress = |stage, done, total, uncertain| {
        let _ = app.emit("genre-progress", Progress { done, total, stage, uncertain });
    };
    progress("Sorting", 0, total, 0);
    let mut result = SortResult::default();
    let mut uncertain = Vec::new();
    for (n, batch) in candidates.chunks(BATCH).enumerate() {
        if CANCEL.load(Ordering::SeqCst) {
            break;
        }
        let verdicts = crate::ai::classify_genres(&key, &genres, batch, instructions).await?;
        let c = state.db.lock().map_err(err)?;
        let mut seen = HashSet::new();
        for (i, v) in verdicts.into_iter().filter(|(i, _)| seen.insert(*i)) {
            let Some(cand) = batch.get(i) else { continue };
            if research && v.confidence < 0.6 {
                uncertain.push(n * BATCH + i);
            }
            result.suggestions += usize::from(store(&c, cand, &v, &genres, false)?);
        }
        drop(c);
        progress("Sorting", (n * BATCH + batch.len()).min(total), total, uncertain.len());
    }
    result.uncertain = uncertain.len();
    if CANCEL.load(Ordering::SeqCst) || !allow_research(uncertain.len(), confirm_research(app, uncertain.len())).await {
        return Ok(result);
    }
    for (k, i) in uncertain.iter().enumerate() {
        if CANCEL.load(Ordering::SeqCst) {
            break;
        }
        progress("Researching", k, uncertain.len(), uncertain.len());
        let cand = &candidates[*i];
        result.attempted += 1;
        match crate::ai::research_genre(&key, &genres, cand, instructions).await {
            Ok(v) => {
                let c = state.db.lock().map_err(err)?;
                let before = c
                    .query_row("SELECT 1 FROM genre_suggestions WHERE album_id=?1", [&cand.id], |_| Ok(()))
                    .optional()
                    .map_err(err)?
                    .is_some();
                let now = store(&c, cand, &v, &genres, true)?;
                result.suggestions = (result.suggestions + usize::from(now)).saturating_sub(usize::from(before));
            }
            Err(e) => eprintln!("Genre research for {} failed: {e}", cand.id),
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_research_batches_require_explicit_approval_before_any_lookup() {
        tauri::async_runtime::block_on(async {
            for count in [0, 1, 50] {
                assert!(allow_research(count, async { panic!("Unexpected confirmation") }).await);
            }
            let (send, answer) = tokio::sync::oneshot::channel();
            let mut gate = Box::pin(allow_research(51, async { answer.await.unwrap_or(false) }));
            assert!(tokio::time::timeout(std::time::Duration::from_millis(5), &mut gate).await.is_err());
            send.send(false).unwrap();
            assert!(!gate.await);
            assert!(allow_research(51, async { true }).await);
        });
    }

    fn library() -> Connection {
        let mut c = db::open(std::path::Path::new(":memory:")).unwrap();
        c.execute_batch(
            "INSERT INTO albums(id,artist,title,genre,sort_key) VALUES
               ('a','Nils Frahm','Spaces','Dance & DJ','a'), ('b','Moderat','II','Dance and DJ','b'),
               ('c','Pixies','Doolittle','Alternative-Rock','c'), ('d','Low','C''mon',NULL,'d');
             INSERT INTO tracks(path,album_id,title,mtime) VALUES ('1','a','x',0),('2','b','x',0),('3','c','x',0),('4','d','x',0);",
        )
        .unwrap();
        let tx = c.transaction().unwrap();
        crate::releases::rebuild(&tx).unwrap();
        tx.commit().unwrap();
        c
    }

    fn genre(name: &str, parent: Option<&str>) -> Genre {
        Genre { name: name.into(), parent: parent.map(Into::into) }
    }

    #[test]
    fn spelling_variants_share_a_key_and_the_inventory_counts_records() {
        assert_eq!(normalize(" Alternative-Rock "), normalize("alternative rock"));
        assert_eq!(normalize("Dance & DJ"), normalize("dance and dj"));
        let inv = inventory(&library()).unwrap();
        let dj = inv.tags.iter().find(|t| t.key == "dance and dj").unwrap();
        assert_eq!((dj.albums, dj.variants.len(), inv.untagged), (2, 2, 1));
    }

    #[test]
    fn your_choice_beats_ai_beats_tag_mapping_and_survives_reorganising() {
        let mut c = library();
        let book = [genre("Electronic", None), genre("Modern Classical", Some("Electronic")), genre("Rock", None)];
        let map = BTreeMap::from([("Dance & DJ".to_string(), "electronic".to_string()), ("alternative rock".into(), "Rock".into())]);
        save_book(&mut c, &book, &map, &BTreeMap::new()).unwrap();
        let s = state_of(&c).unwrap();
        assert_eq!(s.albums["b"].genre.as_deref(), Some("Electronic"));
        assert_eq!((s.albums["d"].genre.as_deref(), s.albums["d"].origin), (None, "none"));

        c.execute("INSERT INTO genre_suggestions(album_id,genre,confidence,reason) VALUES('a','Modern Classical',.9,'piano')", []).unwrap();
        c.execute("INSERT INTO genre_suggestions(album_id,genre,confidence,reason) VALUES('c','Rock',.4,'guess')", []).unwrap();
        review(&mut c, &[Accept { album_id: "a".into(), genre: "modern classical".into() }], &["c".into()]).unwrap();
        set_choice(&c, "d", Some("Slowcore")).unwrap();
        let s = state_of(&c).unwrap();
        assert_eq!((s.albums["a"].genre.as_deref(), s.albums["a"].origin), (Some("Modern Classical"), "ai"));
        assert_eq!((s.albums["d"].genre.as_deref(), s.albums["d"].origin), (Some("Slowcore"), "user"));
        assert_eq!(s.suggestions, 0);

        let renamed = [genre("Electronic", None), genre("Neoclassical", Some("Electronic"))];
        save_book(&mut c, &renamed, &BTreeMap::new(), &BTreeMap::from([("Modern Classical".into(), "Neoclassical".into())])).unwrap();
        let s = state_of(&c).unwrap();
        assert_eq!(s.albums["a"].genre.as_deref(), Some("Neoclassical"));
        assert_eq!(s.albums["d"].genre.as_deref(), Some("Slowcore"));
        assert!(s.genres.iter().any(|g| g.name == "Slowcore" && g.parent.is_none()));
        assert_eq!(s.albums["c"].genre, None);
    }

    #[test]
    fn an_ambient_suggestion_can_replace_electronic_and_create_a_top_level_genre_on_review() {
        let mut c = library();
        let existing = [genre("Electronic", None)];
        save_book(&mut c, &existing, &BTreeMap::from([("Dance & DJ".into(), "Electronic".into())]), &BTreeMap::new()).unwrap();
        let candidate = Candidate { id:"a".into(), album:db::album(&c,"a").unwrap().unwrap(),
            current:Some("Electronic".into()), tracks:vec![], notes:None };
        let verdict = serde_json::from_value(serde_json::json!({"genre":"Ambient","parent":null,"confidence":0.9})).unwrap();
        assert!(store(&c, &candidate, &verdict, &existing, false).unwrap());
        assert_eq!(state_of(&c).unwrap().albums["a"].genre.as_deref(), Some("Electronic"));
        review(&mut c, &[Accept { album_id:"a".into(), genre:"Ambient".into() }], &[]).unwrap();
        let state = state_of(&c).unwrap();
        assert_eq!(state.albums["a"].genre.as_deref(), Some("Ambient"));
        assert!(state.genres.iter().any(|g| g.name == "Ambient" && g.parent.is_none()));
    }

    #[test]
    fn genres_stay_two_levels_deep() {
        let g = validate(&[genre("A", None), genre("B", Some("a")), genre("C", Some("B")), genre("D", Some("missing"))]).unwrap();
        assert_eq!(g[1].parent.as_deref(), Some("A"));
        assert_eq!((g[2].parent.as_deref(), g[3].parent.as_deref()), (None, None));
        assert!(validate(&[genre("Rock", None), genre("rock ", None)]).is_err());
    }
}
