use crate::{db, scanner, R};
use image::{imageops::FilterType, DynamicImage, GenericImageView, ImageReader};
use reqwest::{Client, Response, StatusCode};
use serde::Deserialize;
use std::{io::Cursor, path::Path, time::Duration};
use tokio::{sync::Mutex, time::Instant};

const MAX_BYTES: usize = 24 * 1024 * 1024;
const MAX_SIDE: u32 = 3000;

pub struct Search {
    client: Client,
    next_request: Mutex<Instant>,
}

#[derive(Deserialize)]
struct Results {
    #[serde(rename = "release-groups")]
    groups: Vec<Group>,
}

#[derive(Deserialize)]
struct Group {
    id: String,
    title: String,
    #[serde(rename = "artist-credit")]
    credit: Vec<Credit>,
    #[serde(default, rename = "first-release-date")]
    date: String,
}

#[derive(Deserialize)]
struct Credit {
    name: String,
    #[serde(default)]
    joinphrase: String,
}

fn normalized(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn match_group(groups: &[Group], album: &db::Album) -> Option<String> {
    let title = normalized(&album.title);
    let artist = normalized(&album.artist);
    let matches: Vec<_> = groups
        .iter()
        .filter(|g| {
            let credit: String = g
                .credit
                .iter()
                .flat_map(|c| [c.name.as_str(), c.joinphrase.as_str()])
                .collect();
            normalized(&g.title) == title && normalized(&credit) == artist
        })
        .collect();
    let group = match matches.as_slice() {
        [only] => only,
        _ => {
            let year = album.year?.to_string();
            let mut dated = matches
                .iter()
                .filter(|g| g.date.split('-').next() == Some(&year));
            let first = dated.next()?;
            if dated.next().is_some() {
                return None;
            }
            first
        }
    };
    Some(group.id.clone())
}

async fn bounded_bytes(mut response: Response, limit: usize) -> R<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err("Artwork service response is too large".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(crate::err)? {
        if chunk.len() > limit - bytes.len() {
            return Err("Artwork service response is too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

impl Search {
    pub fn new() -> reqwest::Result<Self> {
        Ok(Self {
            client: Client::builder()
                .user_agent(concat!(
                    "Muso/",
                    env!("CARGO_PKG_VERSION"),
                    " (desktop music player)"
                ))
                .timeout(Duration::from_secs(35))
                .build()?,
            next_request: Mutex::new(Instant::now()),
        })
    }

    pub async fn find(&self, album: &db::Album) -> R<Option<Vec<u8>>> {
        if album.artist == "Unknown Artist" {
            return Ok(None);
        }
        let query = format!(
            "releasegroup:{} AND artist:{}",
            quoted(&album.title),
            quoted(&album.artist)
        );
        let mut next = self.next_request.lock().await;
        let mut retries = 0;
        let results = loop {
            if next.saturating_duration_since(Instant::now()) > Duration::from_secs(10) {
                return Err("MusicBrainz is busy. Try again later.".into());
            }
            tokio::time::sleep_until(*next).await;
            *next = Instant::now() + Duration::from_millis(1100);
            let response = self
                .client
                .get("https://musicbrainz.org/ws/2/release-group/")
                .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "8")])
                .send()
                .await
                .map_err(|_| "Could not reach MusicBrainz")?;
            if !matches!(
                response.status(),
                StatusCode::SERVICE_UNAVAILABLE | StatusCode::TOO_MANY_REQUESTS
            ) {
                break response.error_for_status().map_err(crate::err)?;
            }
            retries += 1;
            let delay = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(1 << retries)
                .clamp(1, 3600);
            *next = Instant::now() + Duration::from_secs(delay);
            if retries >= 4 || delay > 10 {
                return Err("MusicBrainz is busy. Try again later.".into());
            }
        };
        drop(next);
        let results: Results =
            serde_json::from_slice(&bounded_bytes(results, 2 * 1024 * 1024).await?)
                .map_err(crate::err)?;
        let Some(id) = match_group(&results.groups, album) else {
            return Ok(None);
        };
        if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            return Err("Invalid MusicBrainz release group".into());
        }
        let response = self
            .client
            .get(format!(
                "https://coverartarchive.org/release-group/{id}/front"
            ))
            .send()
            .await
            .map_err(crate::err)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let bytes =
            bounded_bytes(response.error_for_status().map_err(crate::err)?, MAX_BYTES).await?;
        Ok(Some(bytes))
    }
}

pub fn decode(bytes: &[u8]) -> R<DynamicImage> {
    let reader = || {
        ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(crate::err)
    };
    let (w, h) = reader()?.into_dimensions().map_err(crate::err)?;
    if u64::from(w) * u64::from(h) > 32_000_000 {
        return Err("Cover exceeds the image size limit".into());
    }
    let img = reader()?.decode().map_err(crate::err)?;
    Ok(if w.max(h) > MAX_SIDE {
        img.resize(MAX_SIDE, MAX_SIDE, FilterType::Triangle)
    } else {
        img
    })
}

fn improves((w, h): (u32, u32), (old_w, old_h): (u32, u32)) -> bool {
    w >= old_w && h >= old_h && (w > old_w || h > old_h)
}

pub fn install(
    conn: &mut rusqlite::Connection,
    dir: &Path,
    id: &str,
    img: &DynamicImage,
) -> R<Option<String>> {
    // Recheck under the write lock in case a library scan completed during download.
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(crate::err)?;
    let Some(album) = db::album(&tx, id).map_err(crate::err)? else {
        return Ok(None);
    };
    let old = album.cover_file.unwrap_or_else(|| format!("{id}.jpg"));
    let size = img.dimensions();
    if album.art {
        let path = dir.join("art").join(&old);
        if path.exists() && !improves(size, image::image_dimensions(path).map_err(crate::err)?) {
            return Ok(None);
        }
    }
    let file = format!("{id}_{}x{}.jpg", size.0, size.1);
    let art = dir.join("art").join(&file);
    let thumb = dir.join("thumb").join(&file);
    let result: R<()> = (|| {
        scanner::save_jpeg(img, MAX_SIDE, &art).map_err(crate::err)?;
        scanner::save_jpeg(img, 560, &thumb).map_err(crate::err)?;
        tx.execute(
            "UPDATE albums SET has_art=1, cover_file=?2 WHERE id=?1",
            [id, &file],
        )
        .map_err(crate::err)?;
        tx.commit().map_err(crate::err)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(art);
        let _ = std::fs::remove_file(thumb);
        return Err(e);
    }
    if old != file && old != format!("{id}.jpg") {
        let _ = std::fs::remove_file(dir.join("art").join(&old));
        let _ = std::fs::remove_file(dir.join("thumb").join(&old));
    }
    Ok(Some(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album() -> db::Album {
        db::Album {
            id: "abcd".into(),
            title: "Abbey Road".into(),
            artist: "The Beatles".into(),
            year: Some(1969),
            genre: None,
            art: true,
            cover_file: None,
        }
    }

    #[test]
    fn ambiguous_matches_require_a_unique_year() {
        let results: Results = serde_json::from_str(r#"{"release-groups":[
          {"id":"a","title":"Abbey Road","artist-credit":[{"name":"The Beatles"}],"first-release-date":"1969-09"},
          {"id":"b","title":"Abbey Road","artist-credit":[{"name":"The Beatles"}],"first-release-date":"1972"},
          {"id":"c","title":"Abbey Road","artist-credit":[{"name":"Other"}],"first-release-date":"1969"}
        ]}"#).unwrap();
        let mut album = album();
        assert_eq!(match_group(&results.groups, &album).as_deref(), Some("a"));
        album.year = None;
        assert_eq!(match_group(&results.groups, &album), None);
    }

    #[test]
    fn upgrades_existing_libraries_only_when_resolution_improves() {
        let dir = std::env::temp_dir().join(format!("muso-cover-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("art")).unwrap();
        std::fs::create_dir_all(dir.join("thumb")).unwrap();
        let db_path = dir.join("index.db");
        let legacy = rusqlite::Connection::open(&db_path).unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE albums(id TEXT PRIMARY KEY, artist TEXT, title TEXT,
            year INTEGER, genre TEXT, has_art INTEGER, sort_key TEXT);
            INSERT INTO albums VALUES('abcd', 'The Beatles', 'Abbey Road', 1969, NULL, 1, 'a')",
            )
            .unwrap();
        drop(legacy);
        let mut conn = db::open(&db_path).unwrap();
        let old = dir.join("art/abcd.jpg");
        scanner::save_jpeg(&DynamicImage::new_rgb8(100, 100), 1400, &old).unwrap();
        for (w, h) in [(100, 100), (98, 120), (80, 80)] {
            assert!(
                install(&mut conn, &dir, "abcd", &DynamicImage::new_rgb8(w, h))
                    .unwrap()
                    .is_none()
            );
        }
        let file = install(&mut conn, &dir, "abcd", &DynamicImage::new_rgb8(640, 640))
            .unwrap()
            .unwrap();
        assert_eq!(
            image::image_dimensions(dir.join("art").join(&file)).unwrap(),
            (640, 640)
        );
        assert_eq!(
            image::image_dimensions(dir.join("thumb").join(&file)).unwrap(),
            (560, 560)
        );
        assert_eq!(image::image_dimensions(old).unwrap(), (100, 100));
        drop(conn);
        let conn = db::open(&db_path).unwrap();
        assert_eq!(
            db::album(&conn, "abcd").unwrap().unwrap().cover_file,
            Some(file)
        );
        drop(conn);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "requires access to MusicBrainz and the Cover Art Archive"]
    fn online_front_cover() {
        let bytes = tauri::async_runtime::block_on(async {
            Search::new()
                .unwrap()
                .find(&album())
                .await
                .unwrap()
                .unwrap()
        });
        assert!(decode(&bytes).unwrap().width() > 0);
    }
}
