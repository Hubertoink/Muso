use percent_encoding::percent_decode_str;
use std::borrow::Cow;
use rusqlite::{Connection, OptionalExtension};

#[derive(serde::Serialize)]
pub struct Credit {
    title: String,
    style: Option<String>,
    persona: Option<String>,
}

pub fn credit(c: &Connection, album: &str, style: &str, file: &str) -> rusqlite::Result<Option<Credit>> {
    if let Some(token) = file.strip_prefix("personal/") {
        let Some((id, rev)) = token.split_once('-') else { return Ok(None) };
        let source = c.query_row("SELECT source FROM personal_images WHERE id=?1 AND rev=?2",
            (id, rev), |r| r.get::<_, String>(0)).optional()?;
        return Ok(source.and_then(|path| Some(Credit {
            title: std::path::Path::new(&path).file_stem()?.to_str()?.to_owned(),
            style: None, persona: None,
        })));
    }
    let files = crate::db::atmospheres(c, album, style)?;
    let Some(index) = files.iter().position(|f| f == file) else { return Ok(None) };
    let Some(record) = crate::db::album(c, album)? else { return Ok(None) };
    let medium = if style == "auto" {
        crate::db::get_setting(c, &format!("atmos_auto_style.{album}"))?
    } else { style_label(style).map(str::to_owned) };
    let prompt = c.query_row("SELECT prompt FROM painting_prompts WHERE file=?1", [file], |r| r.get::<_, String>(0)).optional()?;
    let persona = prompt.as_deref().and_then(|text| text.lines().find_map(|line| {
        let commission = line.strip_prefix("You are the ")?;
        let (artist, _) = commission.split_once(" commissioned for this entire album series.")?;
        Some(artist.to_owned())
    }));
    Ok(Some(Credit { title: format!("{} · {:02}", record.title, index + 1), style: medium, persona }))
}

pub fn style_label(style: &str) -> Option<&'static str> {
    Some(match style {
        "cinematic" => "Cinematic",
        "dreamscape" => "Dreamscape",
        "ambient" => "Ambient Drift",
        "tape" => "Tape Decay",
        "photogram" => "Silver Photogram",
        "cel" => "Midnight Cel",
        "ukiyoe" => "Ukiyo-e Woodcut",
        "dither" => "Dithered Worlds",
        "pixelart" => "Pixel Art",
        "postmodern" => "Postmodern Realism",
        "surreal" => "Surreal Realism",
        "urbanphoto" => "Urban Photography",
        "landscapephoto" => "Landscape Photography",
        "nouveau" => "Art Nouveau",
        "bauhaus" => "Bauhaus Study",
        "streetart" => "Modern Street Art",
        "sumi" => "Sumi-e Ink",
        "raytrace" => "Early Raytracing",
        "futurism" => "Futurist Motion",
        "risograph" => "Risograph Layers",
        "symbolism" => "Symbolist Nocturne",
        "biomechanical" => "Biomechanical",
        _ => return None,
    })
}

fn forbidden(c: char) -> bool {
    c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
}

fn reserved(file: &str) -> bool {
    let stem = file.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ["COM", "LPT"].iter().any(|prefix| stem.strip_prefix(prefix)
            .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit() || matches!(n, "¹" | "²" | "³")))
}

fn label(input: &str, fallback: &str) -> String {
    let cleaned = input.replace(forbidden, " ").split_whitespace().collect::<Vec<_>>().join(" ");
    let mut end = cleaned.len().min(72);
    while !cleaned.is_char_boundary(end) { end -= 1; }
    let s = cleaned[..end].trim_matches([' ', '.']);
    if s.is_empty() { fallback.into() } else { s.into() }
}

pub fn filename(title: &str, style: &str, number: usize, stamp: u128) -> String {
    let mut name = format!("{} - {} - {number:02} [{stamp:x}].jpg", label(title, "Album"), label(style, "Atmosphere"));
    if reserved(&name) { name.insert(0, '_'); }
    name
}

pub fn image_path(uri_path: &str) -> Option<Cow<'_, str>> {
    let rel = percent_decode_str(uri_path.strip_prefix('/')?).decode_utf8().ok()?;
    let (dir, file) = rel.split_once('/')?;
    (matches!(dir, "art" | "thumb" | "atmos") && file.ends_with(".jpg")
        && !file.chars().any(forbidden) && !reserved(file)).then_some(rel)
}

/// Give indexed paintings from earlier releases readable names without regenerating them.
pub fn rename_legacy(conn: &mut rusqlite::Connection, data_dir: &std::path::Path) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut query = tx.prepare("SELECT a.id, a.title, p.style, p.file, p.idx, s.value FROM
        (SELECT album_id, 'cinematic' AS style, file, idx FROM atmospheres
         UNION ALL SELECT album_id, style, file, idx FROM atmosphere_variants) p
        JOIN albums a ON a.id=p.album_id LEFT JOIN settings s ON s.key=('atmos_auto_style.' || a.id)
        WHERE p.file GLOB 'atmos/*_*_*.jpg'")?;
    let rows = query.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?,
        r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, usize>(4)?, r.get::<_, Option<String>>(5)?)))?;
    let mut renamed = Vec::new();
    for row in rows {
        let (id, title, style, old, idx, auto_name) = row?;
        if id.len() != 16 || !id.bytes().all(|b| b.is_ascii_hexdigit()) { continue; }
        let Some(stamp) = old.strip_prefix(&format!("atmos/{id}_"))
            .and_then(|s| s.strip_suffix(&format!("_{idx}.jpg")))
            .and_then(|s| s.parse::<u128>().ok()) else { continue };
        let medium = if style == "auto" { auto_name.as_deref() } else { style_label(&style) };
        let Some(medium) = medium else { continue };
        let new = format!("atmos/{}", filename(&title, medium, idx + 1, stamp));
        // Link first. A collision or unsupported filesystem leaves the original usable.
        if std::fs::hard_link(data_dir.join(&old), data_dir.join(&new)).is_ok() {
            renamed.push((old, new));
        }
    }
    drop(query);
    for (old, new) in &renamed {
        tx.execute("UPDATE atmospheres SET file=?2 WHERE file=?1", [old, new])?;
        tx.execute("UPDATE atmosphere_variants SET file=?2 WHERE file=?1", [old, new])?;
    }
    tx.commit()?;
    for (old, _) in renamed { let _ = std::fs::remove_file(data_dir.join(old)); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credits_follow_the_saved_series_and_only_show_a_commissioned_persona() {
        let c = crate::db::open(std::path::Path::new(":memory:")).unwrap();
        c.execute("INSERT INTO albums(id,artist,title,sort_key) VALUES('a','Artist','Album','a')", []).unwrap();
        let files = ["atmos/first.jpg".into(), "atmos/second.jpg".into()];
        crate::db::replace_atmospheres(&c,"a","auto",&files).unwrap();
        crate::db::set_setting(&c,"atmos_auto_style.a","Silver ink").unwrap();
        c.execute("INSERT INTO painting_prompts(file,prompt) VALUES(?1,?2)",
            (&files[1], "Medium: Ink\nYou are the Editorial illustrator commissioned for this entire album series. Brush ink.")).unwrap();
        let caption = credit(&c,"a","auto",&files[1]).unwrap().unwrap();
        assert_eq!(caption.title, "Album · 02");
        assert_eq!(caption.style.as_deref(), Some("Silver ink"));
        assert_eq!(caption.persona.as_deref(), Some("Editorial illustrator"));
        assert!(credit(&c,"a","auto",&files[0]).unwrap().unwrap().persona.is_none());
        assert!(credit(&c,"a","tape",&files[1]).unwrap().is_none());
        c.execute("INSERT INTO personal_images(id,source,stamp) VALUES(17,'photos/Winter dusk.png','1')",[]).unwrap();
        let own = credit(&c,"a","personal","personal/17-1").unwrap().unwrap();
        assert_eq!(own.title, "Winter dusk");
        assert!(own.style.is_none() && own.persona.is_none());
    }
    #[test]
    fn old_paintings_keep_their_bytes_and_actual_ai_style() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("atmos")).unwrap();
        let mut c = crate::db::open(std::path::Path::new(":memory:")).unwrap();
        let id = "0123456789abcdef";
        let old = format!("atmos/{id}_31_0.jpg");
        c.execute("INSERT INTO albums(id,artist,title,sort_key) VALUES (?1,'Artist','Album','a')", [id]).unwrap();
        crate::db::replace_atmospheres(&c, id, "auto", std::slice::from_ref(&old)).unwrap();
        crate::db::set_setting(&c, &format!("atmos_auto_style.{id}"), "Silver ink").unwrap();
        std::fs::write(dir.path().join(&old), b"saved painting").unwrap();
        for _ in 0..2 { rename_legacy(&mut c, dir.path()).unwrap(); }
        let files = crate::db::atmospheres(&c, id, "auto").unwrap();
        assert_eq!(files, ["atmos/Album - Silver ink - 01 [1f].jpg"]);
        assert_eq!(std::fs::read(dir.path().join(&files[0])).unwrap(), b"saved painting");
        assert!(!dir.path().join(old).exists());
    }
    #[test]
    fn readable_portable_jpeg_names() {
        assert_eq!(filename("A/B: 雨", "Salt & Ink", 2, 31), "A B 雨 - Salt & Ink - 02 [1f].jpg");
        assert!(filename("CON.txt", "Style", 1, 0).starts_with('_'));
        assert!(filename(&"🎵".repeat(100), &"絵".repeat(100), 3, u128::MAX).len() < 255);
        assert_eq!(image_path("/atmos/%E9%9B%A8%20%23%25.jpg").as_deref(), Some("atmos/雨 #%.jpg"));
        for bad in ["/atmos/..%2Fart/a.jpg", "/atmos/%5Cserver.jpg", "/atmos/C%3Afile.jpg", "/other/a.jpg", "/atmos/CON.jpg"] {
            assert!(image_path(bad).is_none());
        }
    }
}
