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
    Move { id: u64, album: String, direction: i8 },
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
                let id = id.or(self.active).unwrap_or_else(|| self.create());
                let list = self.list(id)?;
                if !list.albums.iter().any(|entry| entry.id == a.id) {
                    list.albums.push(Entry { id:a.id, title:a.title, artist:a.artist });
                }
            }
            Edit::Remove { id, album } => self.list(id)?.albums.retain(|a| a.id != album),
            Edit::Move { id, album, direction } => {
                let albums = &mut self.list(id)?.albums;
                if let Some(i) = albums.iter().position(|a| a.id == album) {
                    let dest = i.saturating_add_signed(direction.signum() as isize).min(albums.len()-1);
                    albums.swap(i, dest);
                }
            }
        }
        Ok(())
    }
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
    fn first_drop_creates_a_crate_and_edits_keep_its_album_order() {
        let db = db::open(std::path::Path::new(":memory:")).unwrap();
        db.execute_batch("INSERT INTO albums(id, artist, title, sort_key) VALUES
            ('a','Artist','One','a'), ('b','Artist','Two','b')").unwrap();
        let mut crates = Collection::default();
        for album in ["a","b","a"] {
            crates.edit(Edit::Add { id:None, album:album.into() }, &db).unwrap();
        }
        assert_eq!(crates.lists[0].name, "Crate 1");
        crates.edit(Edit::Move { id:1, album:"b".into(), direction:-1 }, &db).unwrap();
        let saved = serde_json::to_string(&crates).unwrap();
        let mut crates: Collection = serde_json::from_str(&saved).unwrap();
        assert_eq!(crates.lists[0].albums.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["b","a"]);
        crates.edit(Edit::Rename { id:1, name:"Night".into() }, &db).unwrap();
        crates.edit(Edit::Create, &db).unwrap();
        crates.edit(Edit::Delete { id:2 }, &db).unwrap();
        assert_eq!(crates.active, Some(1));
        assert_eq!(crates.lists[0].name, "Night");
        crates.edit(Edit::Remove { id:1, album:"a".into() }, &db).unwrap();
        assert_eq!(crates.lists[0].albums.len(), 1);
    }
}
