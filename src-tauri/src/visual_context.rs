use crate::db::Album;
use base64::Engine;
use image::{imageops, DynamicImage, RgbImage};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct References {
    cover: Option<PathBuf>,
    paintings: Vec<String>,
    common_styles: Vec<String>,
}

impl References {
    pub fn load(db: &Connection, album: &Album) -> rusqlite::Result<Self> {
        let cover = album.art.then(|| {
            PathBuf::from(album.cover_file.clone().unwrap_or_else(|| format!("{}.jpg", album.id)))
        }).filter(|file| file.file_name() == Some(file.as_os_str()));
        // Interleave albums before taking a second picture from any one of them.
        // Give the current release and the latest saved sets the first slots.
        let mut query = db.prepare("WITH files AS (
            SELECT album_id,file,1 AS styled,rowid AS seq FROM atmosphere_variants
            UNION ALL SELECT album_id,file,0,rowid FROM atmospheres
        ), ranked AS (
            SELECT *,row_number() OVER (PARTITION BY album_id ORDER BY styled DESC,seq DESC) AS slot FROM files
        ), sample AS (
            SELECT *,row_number() OVER (PARTITION BY slot ORDER BY album_id=?1 DESC,styled DESC,seq DESC) AS priority FROM ranked
        ) SELECT file FROM sample ORDER BY slot,min(priority,4),random() LIMIT 120")?;
        let paintings = query.query_map([&album.id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut styles = db.prepare("SELECT CASE WHEN style='auto' THEN COALESCE(s.value,'') ELSE style END AS medium
            FROM (SELECT DISTINCT album_id,style FROM atmosphere_variants) v
            LEFT JOIN settings s ON s.key='atmos_auto_style.'||v.album_id
            GROUP BY medium COLLATE NOCASE HAVING medium<>'' ORDER BY count(*) DESC,medium LIMIT 8")?;
        let common_styles = styles.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(Self { cover, paintings, common_styles })
    }

    pub fn content(self, directory: &Path) -> Vec<Value> {
        let mut parts = Vec::new();
        if let Some(cover) = self.cover.and_then(|f| image::open(directory.join("art").join(f)).ok()) {
            attach(&mut parts, "Current album cover: primary visual reference. Observe its palette, surface and composition.", cover.thumbnail(512, 512));
        }
        let mut tiles = Vec::new();
        let mut seen = HashSet::new();
        for rel in self.paintings {
            let path = Path::new(&rel);
            if path.parent() != Some(Path::new("atmos")) || !seen.insert(rel.clone()) { continue; }
            if let Ok(picture) = image::open(directory.join(path)) {
                tiles.push(picture.thumbnail(192, 128).to_rgb8());
                if tiles.len() == 6 { break; }
            }
        }
        if !tiles.is_empty() {
            let mut sheet = RgbImage::new(196 * tiles.len().min(3) as u32, 132 * tiles.len().div_ceil(3) as u32);
            for (i, tile) in tiles.iter().enumerate() {
                let x = (i as u32 % 3) * 196 + (196 - tile.width()) / 2;
                let y = (i as u32 / 3) * 132 + (132 - tile.height()) / 2;
                imageops::replace(&mut sheet, tile, x.into(), y.into());
            }
            attach(&mut parts, "Previously generated pictures sampled across the library. Compare recurring subjects, framing and light; this sheet is for avoiding repetition only.", DynamicImage::ImageRgb8(sheet));
            parts.push(json!({"type":"text","text":format!("Frequently saved media across the collection, for comparison: {}", self.common_styles.join(", "))}));
        }
        parts
    }
}

fn attach(parts: &mut Vec<Value>, label: &str, image: DynamicImage) {
    let mut bytes = Vec::new();
    if image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 78).encode_image(&image.to_rgb8()).is_ok() {
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        parts.push(json!({"type":"text","text":label}));
        parts.push(json!({"type":"image_url","image_url":{"url":format!("data:image/jpeg;base64,{data}"),"detail":"low"}}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_and_new_pictures_share_the_review_even_without_a_cover() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::open(&dir.path().join("library.db")).unwrap();
        let album = Album { id:"current".into(), artist:String::new(), title:String::new(),
            year:None, genre:None, art:true, cover_file:Some("missing.jpg".into()) };
        std::fs::create_dir(dir.path().join("atmos")).unwrap();
        for file in ["earlier","later","legacy"] {
            RgbImage::new(30,20).save(dir.path().join(format!("atmos/{file}.jpg"))).unwrap();
        }
        db.execute_batch("INSERT INTO atmospheres VALUES('old',0,'atmos/legacy.jpg');
            INSERT INTO atmosphere_variants VALUES('current','auto',0,'atmos/earlier.jpg'),
            ('current','auto',1,'atmos/later.jpg'),('third','auto',0,'atmos/missing.jpg');").unwrap();
        let refs = References::load(&db,&album).unwrap();
        assert!(refs.paintings.iter().position(|p| p.ends_with("legacy.jpg")).unwrap()
            < refs.paintings.iter().position(|p| p.ends_with("earlier.jpg")).unwrap());
        let parts = refs.content(dir.path());
        assert_eq!(parts.iter().filter(|p| p["type"]=="image_url").count(), 1);
        assert!(parts[0]["text"].as_str().unwrap().starts_with("Previously generated"));
    }
}
