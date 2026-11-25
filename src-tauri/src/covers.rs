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
    audiodb: Mutex<Instant>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    OfferResearch { reason: Option<String> },
    ReviewCovers { choices: Vec<Candidate> },
    Ambiguous,
    Updated { file: String, source: Origin, tags: TagWrites },
    AlreadyLargeEnough,
    NoMatch,
    NoFrontCover,
    UnknownArtist,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Step {
    Lookup { offer_ai: bool },
    Research,
    UseRelease { location: Location },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum Location {
    MusicBrainz { id: String },
    TheAudioDb { id: String, image: String },
}

#[derive(Serialize)]
pub struct Candidate {
    pub label: String,
    pub location: Location,
}

#[derive(Serialize, Deserialize)]
pub struct Origin {
    pub provider: String,
    pub album_url: String,
}

impl Location {
    pub fn origin(&self) -> Origin {
        match self {
            Self::MusicBrainz { id } => Origin {
                provider: "Cover Art Archive".into(),
                album_url: format!("https://musicbrainz.org/release-group/{id}"),
            },
            Self::TheAudioDb { id, .. } => Origin {
                provider: "TheAudioDB".into(),
                album_url: format!("https://www.theaudiodb.com/album/{id}"),
            },
        }
    }
}

pub enum Lookup {
    Image(Vec<u8>, Origin),
    Result(Outcome),
}

#[derive(Deserialize)]
struct AudioDbAlbums {
    album: Option<Vec<AudioDbAlbum>>,
}

#[derive(Deserialize)]
struct AudioDbAlbum {
    #[serde(rename = "idAlbum")]
    id: String,
    #[serde(rename = "strArtist")]
    artist: String,
    #[serde(rename = "strAlbum")]
    title: String,
    #[serde(rename = "intYearReleased")]
    year: Option<String>,
    #[serde(rename = "strAlbumThumbHQ")]
    high_res: Option<String>,
    #[serde(rename = "strAlbumThumb")]
    cover: Option<String>,
}

fn valid_audio_image(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.port().is_none()
            && u.username().is_empty()
            && u.password().is_none()
            && u.host_str()
                .is_some_and(|host| host == "theaudiodb.com" || host.ends_with(".theaudiodb.com"))
            && u.path().starts_with("/images/media/album/")
    })
}

impl AudioDbAlbums {
    fn matches(self, album: &db::Album) -> Vec<Candidate> {
        let mut found = Vec::new();
        let mut ids = std::collections::HashSet::new();
        for row in self.album.unwrap_or_default() {
            if normalized(&row.artist) != normalized(&album.artist)
                || normalized(&row.title) != normalized(&album.title)
            {
                continue;
            }
            let Some(image) = row
                .high_res
                .into_iter()
                .chain(row.cover)
                .find(|u| valid_audio_image(u))
            else {
                continue;
            };
            if row.id.is_empty()
                || !row.id.bytes().all(|c| c.is_ascii_digit())
                || !ids.insert(row.id.clone())
            {
                continue;
            }
            found.push(Candidate {
                label: format!(
                    "{} — {} {}",
                    row.artist,
                    row.title,
                    row.year.unwrap_or_default()
                )
                .trim_end()
                .to_owned(),
                location: Location::TheAudioDb { id: row.id, image },
            });
        }
        found
    }
}

#[derive(Deserialize)]
struct Results {
    #[serde(rename = "release-groups")]
    groups: Vec<Group>,
}

#[derive(Deserialize, Clone)]
pub struct Group {
    pub id: String,
    title: String,
    #[serde(rename = "artist-credit")]
    credit: Vec<Credit>,
    #[serde(default, rename = "first-release-date")]
    date: String,
}

#[derive(Deserialize, Clone)]
struct Credit {
    name: String,
    artist: Option<Artist>,
    #[serde(default)]
    joinphrase: String,
}

#[derive(Deserialize, Clone)]
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

fn matching_groups(groups: Vec<Group>, album: &db::Album) -> Vec<Group> {
    let title = normalized(&album.title);
    let artist = normalized(&album.artist);
    let matches: Vec<_> = groups
        .into_iter()
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
    if let Some(year) = album.year {
        let year = year.to_string();
        let mut dated = matches
            .iter()
            .filter(|g| g.date.split('-').next() == Some(&year));
        if let Some(group) = dated.next() {
            if dated.next().is_none() {
                return vec![group.clone()];
            }
        }
    }
    matches
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
    AudioDb,
}

impl Service {
    fn name(self) -> &'static str {
        match self {
            Self::MusicBrainz => "MusicBrainz",
            Self::Archive => "Cover Art Archive",
            Self::AudioDb => "TheAudioDB",
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
            audiodb: Mutex::new(Instant::now()),
        })
    }

    async fn fetch(
        &self,
        service: Service,
        build: impl Fn() -> RequestBuilder,
        limit: usize,
        progress: &impl Fn(&str, String),
    ) -> R<Option<Vec<u8>>> {
        let name = service.name();
        let message = match service {
            Service::MusicBrainz => "Looking up the release on MusicBrainz…",
            Service::Archive => "Checking artwork in the Cover Art Archive…",
            Service::AudioDb => "Checking TheAudioDB…",
        };
        progress(name, message.into());
        let schedule = match service {
            Service::MusicBrainz => &self.musicbrainz,
            Service::Archive => &self.archive,
            Service::AudioDb => &self.audiodb,
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
            } else if matches!(service, Service::AudioDb) {
                *next = Instant::now() + Duration::from_millis(2100);
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
                    if !matches!(service, Service::MusicBrainz)
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
                    } else if matches!(service, Service::AudioDb)
                        && r.status() == StatusCode::TOO_MANY_REQUESTS
                    {
                        delay = Duration::from_secs(60);
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
            if attempt == 2 || delay > Duration::from_secs(30) {
                return Err(format!(
                    "{name} {reason}. Try again in {} seconds.",
                    delay.as_secs()
                ));
            }
            progress(name, format!(
                "{name} {reason}. Retrying in {} seconds…",
                delay.as_secs()
            ));
        }
    }

    async fn catalog<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
        progress: &impl Fn(&str, String),
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

    async fn groups(&self, album: &db::Album, progress: &impl Fn(&str, String)) -> R<Vec<Group>> {
        if album.artist == "Unknown Artist" {
            return Ok(Vec::new());
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
            let groups = matching_groups(results.groups, album);
            if !groups.is_empty() {
                return Ok(groups);
            }
        }
        Ok(Vec::new())
    }

    async fn audio_candidates(
        &self,
        album: &db::Album,
        progress: &impl Fn(&str, String),
    ) -> R<Vec<Candidate>> {
        let bytes = self
            .fetch(
                Service::AudioDb,
                || {
                    self.client
                        .get("https://www.theaudiodb.com/api/v1/json/123/searchalbum.php")
                        .query(&[("s", &album.artist), ("a", &album.title)])
                },
                2 * 1024 * 1024,
                progress,
            )
            .await?;
        let Some(bytes) = bytes else {
            return Ok(vec![]);
        };
        Ok(serde_json::from_slice::<AudioDbAlbums>(&bytes)
            .map_err(crate::err)?
            .matches(album))
    }

    pub async fn download(
        &self,
        location: &Location,
        progress: &impl Fn(&str, String),
    ) -> R<Option<Vec<u8>>> {
        match location {
            Location::MusicBrainz { id } => self.artwork(id, progress).await,
            Location::TheAudioDb { id, image } => {
                if !valid_audio_image(image)
                    || id.is_empty()
                    || !id.bytes().all(|c| c.is_ascii_digit())
                {
                    return Err("Invalid TheAudioDB cover.".into());
                }
                self.fetch(
                    Service::AudioDb,
                    || self.client.get(image),
                    MAX_BYTES,
                    progress,
                )
                .await
            }
        }
    }

    pub async fn lookup(
        &self,
        album: &db::Album,
        review: bool,
        progress: &impl Fn(&str, String),
    ) -> R<Lookup> {
        if album.artist == "Unknown Artist" {
            return Ok(Lookup::Result(Outcome::UnknownArtist));
        }
        let mut choices = Vec::new();
        let mut error = None;
        let mut missing = Outcome::NoMatch;
        match self.groups(album, progress).await {
            Ok(groups) => {
                choices = groups
                    .into_iter()
                    .map(|g| Candidate {
                        label: g.label(),
                        location: Location::MusicBrainz { id: g.id },
                    })
                    .collect();
                if choices.len() == 1 {
                    let candidate = choices.pop().unwrap();
                    match self.download(&candidate.location, progress).await {
                        Ok(Some(bytes)) if !review => {
                            return Ok(Lookup::Image(bytes, candidate.location.origin()))
                        }
                        Ok(Some(_)) => {
                            return Ok(Lookup::Result(Outcome::ReviewCovers {
                                choices: vec![candidate],
                            }))
                        }
                        Ok(None) => missing = Outcome::NoFrontCover,
                        Err(e) => error = Some(e),
                    }
                }
            }
            Err(e) => error = Some(e),
        }
        match self.audio_candidates(album, progress).await {
            Ok(found) => choices.extend(found),
            Err(e) => {
                error = Some(match error {
                    Some(first) => format!("{first} {e}"),
                    None => e,
                })
            }
        }
        if !choices.is_empty() {
            if review || choices.len() > 1 {
                return Ok(Lookup::Result(Outcome::ReviewCovers { choices }));
            }
            let candidate = choices.pop().unwrap();
            if let Some(bytes) = self.download(&candidate.location, progress).await? {
                return Ok(Lookup::Image(bytes, candidate.location.origin()));
            }
            missing = Outcome::NoFrontCover;
        }
        match error {
            Some(e) => Err(e),
            None => Ok(Lookup::Result(missing)),
        }
    }

    async fn front(&self, kind: &str, id: &str, progress: &impl Fn(&str, String)) -> R<Option<Vec<u8>>> {
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

    pub async fn artwork(&self, group_id: &str, progress: &impl Fn(&str, String)) -> R<Option<Vec<u8>>> {
        if let Some(bytes) = self.front("release-group", group_id, progress).await? {
            return Ok(Some(bytes));
        }
        let editions: Editions = self
            .catalog(
                "release",
                &[("release-group", group_id), ("limit", "100")],
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
    source: &Origin,
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
        source.save(&tx, &file).map_err(crate::err)?;
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

#[derive(Default, Serialize)]
pub struct TagWrites {
    written: usize,
    failed: usize,
    first_error: Option<String>,
}

pub fn write_mp3_covers(tracks: &[db::Track], art: &Path) -> TagWrites {
    let mut report = TagWrites::default();
    let mut jpeg = None;
    for track in tracks {
        let path = Path::new(&track.path);
        if !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("mp3")) { continue; }
        let bytes = jpeg.get_or_insert_with(|| std::fs::read(art).map_err(crate::err));
        match bytes.as_ref().map_err(Clone::clone).and_then(|bytes| write_front_cover(path, bytes)) {
            Ok(()) => report.written += 1,
            Err(error) => {
                report.failed += 1;
                report.first_error.get_or_insert_with(|| format!("{}: {error}", path.display()));
            }
        }
    }
    report
}

fn write_front_cover(path: &Path, jpeg: &[u8]) -> R<()> {
    use id3::{frame::{Picture, PictureType}, Tag, TagLike};
    use lofty::config::ParseOptions;
    use lofty::file::AudioFile;
    use lofty::mpeg::MpegFile;
    use std::io::Seek;

    let path = path.canonicalize().map_err(crate::err)?;
    let mut source = std::fs::File::open(&path).map_err(crate::err)?;
    let metadata = source.metadata().map_err(crate::err)?;
    if metadata.permissions().readonly() { return Err("File is read-only".into()); }
    // Replace the original only after its complete tagged copy is on disk.
    let mut copy = tempfile::NamedTempFile::new_in(path.parent().ok_or("Missing parent folder")?).map_err(crate::err)?;
    let file = copy.as_file_mut();
    std::io::copy(&mut source, file).map_err(crate::err)?;
    drop(source);
    file.rewind().map_err(crate::err)?;
    MpegFile::read_from(file, ParseOptions::new().read_tags(false)).map_err(crate::err)?;
    file.rewind().map_err(crate::err)?;
    let mut tag = id3::no_tag_ok(id3::v1v2::read_from(&mut *file)).map_err(crate::err)?.unwrap_or_else(Tag::new);
    tag.remove_picture_by_type(PictureType::CoverFront);
    tag.add_frame(Picture {
        picture_type: PictureType::CoverFront, mime_type: "image/jpeg".into(),
        description: String::new(), data: jpeg.to_vec(),
    });
    file.rewind().map_err(crate::err)?;
    tag.write_to_file(&mut *file, tag.version()).map_err(crate::err)?;
    file.set_permissions(metadata.permissions()).map_err(crate::err)?;
    file.sync_all().map_err(crate::err)?;
    let current = std::fs::metadata(&path).map_err(crate::err)?;
    if current.len() != metadata.len() || current.modified().ok() != metadata.modified().ok() {
        return Err("File changed while updating its cover".into());
    }
    copy.persist(&path).map_err(crate::err)?;
    Ok(())
}

impl Origin {
    pub fn save(&self, conn: &rusqlite::Connection, file: &str) -> rusqlite::Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO cover_sources(file,provider,album_url) VALUES(?1,?2,?3)",
            (file, &self.provider, &self.album_url),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_cover_writes_preserve_audio_other_pictures_and_text_and_report_partial_failures() {
        use id3::TagLike;
        use lofty::config::ParseOptions;
        use lofty::file::AudioFile;
        use lofty::id3::v2::{Frame, Id3v2Version};
        use lofty::mpeg::MpegFile;
        use lofty::picture::PictureType;
        use lofty::tag::Accessor;
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("track.MP3");
        let original = include_bytes!("../../assets/Muso_Needle_drop2.mp3");
        fs::write(&path, original).unwrap();
        let art = dir.path().join("art.jpg");
        scanner::save_jpeg(&DynamicImage::new_rgb8(24, 24), 24, &art).unwrap();
        let jpeg = fs::read(&art).unwrap();
        let mut tag = id3::Tag::new();
        tag.set_title("Keep title");
        for kind in [id3::frame::PictureType::CoverFront, id3::frame::PictureType::CoverBack] {
            tag.add_frame(id3::frame::Picture {
                picture_type: kind, mime_type: "image/jpeg".into(), description: String::new(), data: jpeg.clone(),
            });
        }
        tag.write_to_path(&path, id3::Version::Id3v23).unwrap();
        scanner::save_jpeg(&DynamicImage::new_rgb8(64, 64), 64, &art).unwrap();
        let tracks: Vec<_> = ["track.MP3", "unavailable.mp3", "other.flac"].iter().map(|name| db::Track {
            path: dir.path().join(name).to_string_lossy().into_owned(), title: String::new(),
            artist: None, track_no: None, disc_no: None, duration: None,
        }).collect();
        let report = write_mp3_covers(&tracks, &art);
        assert_eq!((report.written, report.failed), (1, 1), "{:?}", report.first_error);
        let tag = MpegFile::read_from(&mut fs::File::open(&path).unwrap(), ParseOptions::new())
            .unwrap().remove_id3v2().unwrap();
        assert_eq!(tag.original_version(), Id3v2Version::V3);
        assert_eq!(tag.title().unwrap(), "Keep title");
        let pics: Vec<_> = (&tag).into_iter().filter_map(|f| if let Frame::Picture(p) = f { Some(&p.picture) } else { None }).collect();
        assert_eq!(pics.len(), 2);
        assert_eq!(pics.iter().find(|p| p.pic_type() == PictureType::CoverBack).unwrap().data(), jpeg);
        assert_eq!(pics.iter().find(|p| p.pic_type() == PictureType::CoverFront).unwrap().data(), fs::read(&art).unwrap());
        let audio_offset = if original.starts_with(b"ID3") {
            10 + original[6..10].iter().fold(0usize, |n, b| (n << 7) | *b as usize)
        } else { 0 };
        assert!(fs::read(path).unwrap().ends_with(&original[audio_offset..]));
    }

    #[test]
    fn an_artwork_only_id3_tag_keeps_ape_albums_in_the_library() {
        use lofty::{config::WriteOptions, tag::{Accessor, Tag, TagExt, TagType}};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("track.mp3");
        std::fs::write(&path, include_bytes!("../../assets/Muso_Needle_drop2.mp3")).unwrap();
        id3::Tag::remove_from_path(&path).unwrap();
        let mut ape = Tag::new(TagType::Ape);
        ape.set_artist("Artist".into());
        ape.set_album("Album".into());
        ape.set_title("Track".into());
        ape.save_to_path(&path, WriteOptions::default()).unwrap();
        let art = dir.path().join("front.jpg");
        scanner::save_jpeg(&DynamicImage::new_rgb8(24, 24), 24, &art).unwrap();
        write_front_cover(&path, &std::fs::read(art).unwrap()).unwrap();
        let index = dir.path().join("index.db");
        let conn = db::open(&index).unwrap();
        db::add_folder(&conn, dir.path().to_str().unwrap()).unwrap();
        scanner::scan(&index, dir.path(), |_| {}).unwrap();
        let album = db::albums(&conn).unwrap().remove(0).album;
        assert_eq!((album.artist.as_str(), album.title.as_str()), ("Artist", "Album"));
        assert_eq!(db::tracks(&conn, &album.id).unwrap()[0].title, "Track");
    }

    #[test]
    fn audiodb_only_offers_distinct_album_covers_with_matching_credits() {
        let base = serde_json::json!({"idAlbum":"123","strArtist":"The Beatles","strAlbum":"Abbey Road",
            "strAlbumThumb":"https://r2.theaudiodb.com/images/media/album/thumb/test.jpg"});
        let mut other_artist = base.clone();
        other_artist["strArtist"] = "Tribute Band".into();
        let mut bad_image = base.clone();
        bad_image["strAlbumThumb"] = "https://example.org/cover.jpg".into();
        let mut second = base.clone();
        second["idAlbum"] = "456".into();
        second["intYearReleased"] = "2019".into();
        let rows: AudioDbAlbums = serde_json::from_value(
            serde_json::json!({"album":[other_artist,bad_image,base,base,second]}),
        )
        .unwrap();
        let matches = rows.matches(&album());
        assert_eq!(matches.len(), 2);
        assert!(matches[1].label.ends_with("2019"));
        assert!(matches[0].location.origin().album_url.ends_with("/123"));
    }

    #[test]
    #[ignore = "requires access to TheAudioDB"]
    fn online_fallback_works_during_a_musicbrainz_cooldown() {
        tauri::async_runtime::block_on(async {
            let search = Search::new().unwrap();
            *search.musicbrainz.lock().await = Instant::now() + Duration::from_secs(600);
            let album = db::Album {
                artist: "David Sylvian".into(),
                title: "Brilliant Trees".into(),
                ..album()
            };
            let Lookup::Image(bytes, source) = search.lookup(&album, false, &|_, _| {}).await.unwrap()
            else {
                panic!("Expected a front cover from TheAudioDB");
            };
            assert_eq!(source.provider, "TheAudioDB");
            assert_eq!(source.album_url, "https://www.theaudiodb.com/album/2152383");
            assert!(decode(&bytes).unwrap().width() > 0);
        });
    }

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
            assert_eq!(matching_groups(results.groups.clone(), &album)[0].id, "f");
        }
        album.artist = "Jim O'Rourke".into();
        assert!(matching_groups(results.groups, &album).is_empty());
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
                .fetch(Service::Archive, get, 8, &|_, message| {
                    if message.contains("Retrying") { attempts.set(attempts.get() + 1); }
                })
                .await;
            assert_eq!(result.unwrap().as_deref(), Some(b"art".as_slice()));
            assert_eq!(attempts.get(), 1);
            assert!(search
                .fetch(Service::MusicBrainz, get, 8, &|_, _| {})
                .await
                .unwrap_err()
                .contains("600 seconds"));
            assert!(search
                .fetch(Service::MusicBrainz, get, 8, &|_, _| {})
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
        assert_eq!(matching_groups(results.groups.clone(), &album)[0].id, "a");
        album.year = None;
        assert_eq!(matching_groups(results.groups, &album).len(), 2);
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
        let source = Location::MusicBrainz { id: "test".into() }.origin();
        for (w, h) in [(100, 100), (98, 120), (80, 80)] {
            assert!(install(
                &mut conn,
                &dir,
                "abcd",
                &DynamicImage::new_rgb8(w, h),
                &source
            )
            .unwrap()
            .is_none());
        }
        let file = install(
            &mut conn,
            &dir,
            "abcd",
            &DynamicImage::new_rgb8(640, 640),
            &source,
        )
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
                .groups(&album, &|_, s| println!("{s}"))
                .await
                .unwrap()
                .pop()
                .unwrap();
            assert_eq!(g.id, "421969db-0492-430e-a8d2-3ef8fce42d01");
            search.artwork(&g.id, &|_, _| {}).await.unwrap().unwrap()
        });
        let size = decode(&bytes).unwrap().dimensions();
        println!("Verified front cover: {} × {}", size.0, size.1);
        assert!(size.0 > 0 && size.1 > 0);
    }
}
