use crate::{db, AppState, R};
use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Default, Deserialize, Serialize)]
pub struct Collection {
    active: Option<u64>,
    lists: Vec<AlbumCrate>,
}

#[derive(Deserialize, Serialize)]
struct AlbumCrate {
    id: u64,
    name: String,
    albums: Vec<Entry>,
}

#[derive(Deserialize, Serialize)]
struct Entry {
    id: String,
    artist: String,
    title: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Edit {
    Create,
    Select { id: u64 },
    Rename { id: u64, name: String },
    Delete { id: u64 },
    Add { id: Option<u64>, album: String },
    Remove { id: u64, album: String },
    Reorder { id: u64, album: String, index: usize },
}

impl Collection {
    fn create(&mut self) -> u64 {
        let mut id = self.lists.iter().map(|c| c.id).max().unwrap_or(0) + 1;
        while self.lists.iter().any(|c| c.name == format!("Crate {id}")) { id += 1; }
        self.lists.push(AlbumCrate { id, name: format!("Crate {id}"), albums: Vec::new() });
        self.active = Some(id);
        id
    }

    fn list(&mut self, id: u64) -> R<&mut AlbumCrate> {
        self.lists.iter_mut().find(|c| c.id == id).ok_or_else(|| "Crate no longer exists.".into())
    }

    fn edit(&mut self, change: Edit, db: &Connection) -> R<()> {
        match change {
            Edit::Create => { self.create(); }
            Edit::Select { id } => { self.list(id)?; self.active = Some(id); }
            Edit::Rename { id, name } => {
                let name = name.trim();
                if name.is_empty() { return Err("Enter a crate name.".into()); }
                self.list(id)?.name = name.to_owned();
            }
            Edit::Delete { id } => {
                self.lists.retain(|c| c.id != id);
                if self.active == Some(id) { self.active = self.lists.first().map(|c| c.id); }
            }
            Edit::Add { id, album } => {
                let a = db::album(db, &album).map_err(crate::err)?.ok_or("Album unavailable.")?;
                let id = id.unwrap_or_else(|| self.create());
                let list = self.list(id)?;
                if !list.albums.iter().any(|entry| entry.id == a.id) {
                    list.albums.push(Entry { id:a.id, title:a.title, artist:a.artist });
                }
            }
            Edit::Remove { id, album } => self.list(id)?.albums.retain(|a| a.id != album),
            Edit::Reorder { id, album, index } => {
                let albums = &mut self.list(id)?.albums;
                if let Some(i) = albums.iter().position(|a| a.id == album) {
                    let entry = albums.remove(i);
                    albums.insert(index.min(albums.len()), entry);
                }
            }
        }
        Ok(())
    }
}

pub(super) fn relink_album(db: &Connection, old: &str, new: &str) -> rusqlite::Result<()> {
    let Some(json) = db::get_setting(db, "library.crates")? else { return Ok(()) };
    if !json.contains(old) { return Ok(()) }
    let mut crates: Collection = serde_json::from_str(&json).map_err(|e|
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?;
    let (artist, title): (String, String) = db.query_row(
        "SELECT artist,title FROM albums WHERE id=?1", [new], |r| Ok((r.get(0)?, r.get(1)?)))?;
    for list in &mut crates.lists {
        let mut seen = false;
        list.albums.retain_mut(|entry| {
            if entry.id != old && entry.id != new { return true }
            if seen { return false }
            seen = true;
            entry.id = new.into();
            entry.artist.clone_from(&artist);
            entry.title.clone_from(&title);
            true
        });
    }
    let json = serde_json::to_string(&crates).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    db::set_setting(db, "library.crates", &json)
}

#[tauri::command]
pub fn library_crates(state: State<'_, AppState>, change: Option<Edit>) -> R<Collection> {
    let mut conn = state.db.lock().map_err(crate::err)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(crate::err)?;
    let mut data: Collection = db::get_setting(&tx, "library.crates").map_err(crate::err)?
        .map(|s| serde_json::from_str(&s)).transpose().map_err(crate::err)?.unwrap_or_default();
    if let Some(change) = change {
        data.edit(change, &tx)?;
        db::set_setting(&tx, "library.crates", &serde_json::to_string(&data).map_err(crate::err)?).map_err(crate::err)?;
    }
    tx.commit().map_err(crate::err)?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_on_a_new_crate_creates_it_and_reorder_moves_to_any_position() {
        let db = db::open(std::path::Path::new(":memory:")).unwrap();
        db.execute_batch("INSERT INTO albums(id, artist, title, sort_key) VALUES
            ('a','Artist','One','a'), ('b','Artist','Two','b')").unwrap();
        let mut crates = Collection::default();
        crates.edit(Edit::Add { id:None, album:"a".into() }, &db).unwrap();
        for album in ["b","a"] {
            crates.edit(Edit::Add { id:Some(1), album:album.into() }, &db).unwrap();
        }
        assert_eq!(crates.lists.len(), 1);
        assert_eq!(crates.lists[0].name, "Crate 1");
        crates.edit(Edit::Reorder { id:1, album:"a".into(), index:9 }, &db).unwrap();
        let saved = serde_json::to_string(&crates).unwrap();
        let mut crates: Collection = serde_json::from_str(&saved).unwrap();
        assert_eq!(crates.lists[0].albums.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["b","a"]);
        crates.edit(Edit::Rename { id:1, name:"Night".into() }, &db).unwrap();
        crates.edit(Edit::Create, &db).unwrap();
        crates.edit(Edit::Delete { id:2 }, &db).unwrap();
        assert_eq!(crates.active, Some(1));
        assert_eq!(crates.lists[0].name, "Night");
        crates.edit(Edit::Add { id:None, album:"a".into() }, &db).unwrap();
        assert_eq!(crates.lists.iter().map(|c| c.albums.len()).collect::<Vec<_>>(), [2, 1]);
        crates.edit(Edit::Remove { id:1, album:"a".into() }, &db).unwrap();
        assert_eq!(crates.lists[0].albums.len(), 1);
    }
}
