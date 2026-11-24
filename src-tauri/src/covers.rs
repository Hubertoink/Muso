use crate::{db, scanner, R};
use image::{imageops::FilterType, DynamicImage, GenericImageView, ImageReader};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::{
    io::Cursor,
    path::Path,
    time::{Duration, SystemTime},
};
use tokio::{sync::Mutex, time::Instant};

const MAX_BYTES: usize = 24 * 1024 * 1024;
const MAX_SIDE: u32 = 3000;

pub struct Search {
    client: Client,
    musicbrainz: Mutex<Instant>,
    archive: Mutex<Instant>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Cancelled,
    Updated { file: String },
    AlreadyLargeEnough,
    NoMatch,
    NoFrontCover,
    UnknownArtist,
}

#[derive(Deserialize)]
struct Results {
    #[serde(rename = "release-groups")]
    groups: Vec<Group>,
}

#[derive(Deserialize)]
pub struct Group {
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
    artist: Option<Artist>,
    #[serde(default)]
    joinphrase: String,
}

#[derive(Deserialize)]
struct Artist {
    name: String,
}

fn normalized(s: &str) -> String {
    s.replace(['‘', '’', 'ʼ', '＇'], "'")
        .replace(['‐', '‑', '–', '—'], "-")
        .split_whitespace()
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
            normalized(&g.title) == title
                && [false, true].iter().any(|canonical| {
                    let credit: String = g
                        .credit
                        .iter()
                        .flat_map(|c| {
                            let name = if *canonical {
                                c.artist.as_ref().map_or(&c.name, |a| &a.name)
                            } else {
                                &c.name
                            };
                            [name.as_str(), c.joinphrase.as_str()]
                        })
                        .collect();
                    normalized(&credit) == artist
                })
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

enum ReadError {
    Transport(reqwest::Error),
    TooLarge,
}

async fn bounded_bytes(mut response: Response, limit: usize) -> Result<Vec<u8>, ReadError> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(ReadError::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(ReadError::Transport)? {
        if chunk.len() > limit - bytes.len() {
            return Err(ReadError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[derive(Clone, Copy)]
enum Service {
    MusicBrainz,
    Archive,
}

impl Service {
    fn name(self) -> &'static str {
        match self {
            Self::MusicBrainz => "MusicBrainz",
            Self::Archive => "Cover Art Archive",
        }
    }
}

fn retry_after(value: &str) -> Option<Duration> {
    value.parse().ok().map(Duration::from_secs).or_else(|| {
        httpdate::parse_http_date(value)
            .ok()
            .map(|date| date.duration_since(SystemTime::now()).unwrap_or_default())
    })
}

fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

#[derive(Deserialize)]
struct Editions {
    releases: Vec<Edition>,
}

#[derive(Deserialize)]
struct Edition {
    id: String,
    #[serde(rename = "cover-art-archive")]
    art: Option<ArchiveInfo>,
}

#[derive(Deserialize)]
struct ArchiveInfo {
    front: bool,
}

impl Search {
    pub fn new() -> reqwest::Result<Self> {
        Ok(Self {
            client: Client::builder()
                .user_agent(concat!(
                    "Muso/",
                    env!("CARGO_PKG_VERSION"),
                    " (https://github.com/Hubertoink/Muso)"
                ))
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(25))
                .build()?,
            musicbrainz: Mutex::new(Instant::now()),
            archive: Mutex::new(Instant::now()),
        })
    }

    async fn fetch(
        &self,
        service: Service,
        build: impl Fn() -> RequestBuilder,
        limit: usize,
        progress: &impl Fn(String),
    ) -> R<Option<Vec<u8>>> {
        let name = service.name();
        let schedule = match service {
            Service::MusicBrainz => &self.musicbrainz,
            Service::Archive => &self.archive,
        };
        let mut attempt = 0;
        loop {
            let mut next = schedule.lock().await;
            let wait = next.saturating_duration_since(Instant::now());
            if wait > Duration::from_secs(30) {
                return Err(format!(
                    "{name} is busy. Try again in {} seconds.",
                    wait.as_secs() + 1
                ));
            }
            tokio::time::sleep_until(*next).await;
            if matches!(service, Service::MusicBrainz) {
                *next = Instant::now() + Duration::from_millis(1100);
            }
            let mut delay = Duration::from_secs(3 << attempt);
            let reason = match build().send().await {
                Ok(r) if r.status().is_success() => match bounded_bytes(r, limit).await {
                    Ok(bytes) => return Ok(Some(bytes)),
                    Err(ReadError::TooLarge) => {
                        return Err(format!("{name} response is too large."))
                    }
                    Err(ReadError::Transport(e)) => {
                        if e.is_timeout() {
                            "timed out"
                        } else {
                            "interrupted the download"
                        }
                    }
                },
                Ok(r)
                    if matches!(service, Service::Archive)
                        && r.status() == StatusCode::NOT_FOUND =>
                {
                    return Ok(None)
                }
                Ok(r) if matches!(r.status().as_u16(), 408 | 429 | 500 | 502 | 503 | 504) => {
                    if let Some(retry) = r
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(retry_after)
                    {
                        delay = delay.max(retry);
                    }
                    "is temporarily unavailable"
                }
                Ok(r) => return Err(format!("{name} returned HTTP {}.", r.status())),
                Err(e) if e.is_timeout() => "timed out",
                Err(e) if e.is_connect() || e.is_request() => "could not be reached",
                Err(e) => return Err(format!("{name}: {e}")),
            };
            // Keep the server's cooldown for subsequent album searches as well.
            *next = Instant::now() + delay.min(Duration::from_secs(86400));
            attempt += 1;
            if attempt == 4 || delay > Duration::from_secs(30) {
                return Err(format!(
                    "{name} {reason}. Try again in {} seconds.",
                    delay.as_secs()
                ));
            }
            progress(format!(
                "{name} {reason}. Retrying in {} seconds…",
                delay.as_secs()
            ));
        }
    }

    async fn catalog<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
        progress: &impl Fn(String),
    ) -> R<T> {
        let bytes = self
            .fetch(
                Service::MusicBrainz,
                || {
                    self.client
                        .get(format!("https://musicbrainz.org/ws/2/{path}"))
                        .query(&[("fmt", "json")])
                        .query(params)
                },
                2 * 1024 * 1024,
                progress,
            )
            .await?
            .ok_or("MusicBrainz returned no data")?;
        serde_json::from_slice(&bytes).map_err(crate::err)
    }

    pub async fn group(&self, album: &db::Album, progress: &impl Fn(String)) -> R<Option<Group>> {
        if album.artist == "Unknown Artist" {
            return Ok(None);
        }
        let by_title = format!("releasegroup:{}", quoted(&normalized(&album.title)));
        let by_both = format!(
            "{by_title} AND artist:{}",
            quoted(&normalized(&album.artist))
        );
        for (query, limit) in [(&by_both, "10"), (&by_title, "50")] {
            let results: Results = self
                .catalog(
                    "release-group/",
                    &[("query", query), ("limit", limit)],
                    progress,
                )
                .await?;
            if let Some(id) = match_group(&results.groups, album) {
                return Ok(results.groups.into_iter().find(|g| g.id == id));
            }
        }
        Ok(None)
    }

    async fn front(&self, kind: &str, id: &str, progress: &impl Fn(String)) -> R<Option<Vec<u8>>> {
        if !valid_id(id) {
            return Err("Invalid MusicBrainz identifier".into());
        }
        self.fetch(
            Service::Archive,
            || {
                self.client
                    .get(format!("https://coverartarchive.org/{kind}/{id}/front"))
            },
            MAX_BYTES,
            progress,
        )
        .await
    }

    pub async fn artwork(&self, group: &Group, progress: &impl Fn(String)) -> R<Option<Vec<u8>>> {
        if let Some(bytes) = self.front("release-group", &group.id, progress).await? {
            return Ok(Some(bytes));
        }
        let editions: Editions = self
            .catalog(
                "release",
                &[("release-group", &group.id), ("limit", "100")],
                progress,
            )
            .await?;
        for edition in editions
            .releases
            .iter()
            .filter(|e| e.art.as_ref().is_some_and(|a| a.front))
            .take(3)
        {
            if let Some(bytes) = self.front("release", &edition.id, progress).await? {
                return Ok(Some(bytes));
            }
        }
        Ok(None)
    }
}

impl Group {
    pub fn label(&self) -> String {
        let artist: String = self
            .credit
            .iter()
            .flat_map(|c| [&*c.name, &*c.joinphrase])
            .collect();
        format!(
            "{artist} — {} {}",
            self.title,
            self.date.split('-').next().unwrap_or("")
        )
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
        return Err("This album is no longer in your library.".into());
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
    if old != file && old.starts_with(&format!("{id}_")) {
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
    fn differing_apostrophes_and_artist_credits_identify_the_same_record() {
        let results: Results = serde_json::from_str(
            r#"{"release-groups":[{"id":"f",
            "title":"It’s Hard for Me to Say I’m Sorry","artist-credit":[
              {"name":"Christian Fennesz","artist":{"name":"Fennesz"},"joinphrase":" & "},
              {"name":"Jim O’Rourke","artist":{"name":"Jim O’Rourke"}}]}]}"#,
        )
        .unwrap();
        let mut album = album();
        album.title = "It's Hard For Me To Say I’m Sorry".into();
        for artist in ["Christian Fennesz & Jim O'Rourke", "Fennesz & Jim O'Rourke"] {
            album.artist = artist.into();
            assert_eq!(match_group(&results.groups, &album).as_deref(), Some("f"));
        }
        album.artist = "Jim O'Rourke".into();
        assert_eq!(match_group(&results.groups, &album), None);
    }

    #[test]
    fn temporary_outage_retries_and_long_cooldowns_survive_the_next_lookup() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for reply in [
                "503 Service Unavailable\r\nRetry-After: 1\r\nContent-Length: 0\r\n\r\n",
                "200 OK\r\nContent-Length: 3\r\n\r\nart",
                "429 Too Many Requests\r\nRetry-After: 600\r\nContent-Length: 0\r\n\r\n",
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 2048];
                stream.read(&mut request).unwrap();
                write!(stream, "HTTP/1.1 {reply}").unwrap();
            }
        });
        tauri::async_runtime::block_on(async {
            let search = Search::new().unwrap();
            let get = || search.client.get(&url);
            let attempts = std::cell::Cell::new(0);
            let result = search
                .fetch(Service::Archive, get, 8, &|_| {
                    attempts.set(attempts.get() + 1)
                })
                .await;
            assert_eq!(result.unwrap().as_deref(), Some(b"art".as_slice()));
            assert_eq!(attempts.get(), 1);
            assert!(search
                .fetch(Service::MusicBrainz, get, 8, &|_| {})
                .await
                .unwrap_err()
                .contains("600 seconds"));
            assert!(search
                .fetch(Service::MusicBrainz, get, 8, &|_| {})
                .await
                .unwrap_err()
                .contains("busy"));
        });
        server.join().unwrap();
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
            let mut album = album();
            album.title = "It's Hard For Me To Say I’m Sorry".into();
            album.artist = "Christian Fennesz & Jim O'Rourke".into();
            album.year = Some(2016);
            let search = Search::new().unwrap();
            let g = search
                .group(&album, &|s| println!("{s}"))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(g.id, "421969db-0492-430e-a8d2-3ef8fce42d01");
            search.artwork(&g, &|_| {}).await.unwrap().unwrap()
        });
        let size = decode(&bytes).unwrap().dimensions();
        println!("Verified front cover: {} × {}", size.0, size.1);
        assert!(size.0 > 0 && size.1 > 0);
    }
}
