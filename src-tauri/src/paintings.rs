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
        c.query_row("SELECT h.label FROM atmosphere_variants v JOIN auto_style_choices h ON h.first_file=v.file
            WHERE v.album_id=?1 AND v.style='auto' AND v.idx <=
            (SELECT idx FROM atmosphere_variants WHERE album_id=?1 AND style='auto' AND file=?2)
            ORDER BY v.idx DESC,h.seq DESC LIMIT 1", (album,file), |r| r.get(0)).optional()?
            .or(crate::db::get_setting(c, &format!("atmos_auto_style.{album}"))?)
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

/// Publish one accepted image inside the caller's transaction. Read the current collection
/// on every call so deleting a painting during generation cannot bring it back.
pub fn append_generated(tx: &rusqlite::Transaction, album: &str, style: &str, batch: &[String],
    name: &str, technique: &str, prompt: &str, only_if_empty: bool) -> crate::R<Vec<String>> {
    use crate::{db, err, style_history};
    let (file, previous) = batch.split_last().ok_or("Missing generated painting.")?;
    if only_if_empty && previous.is_empty() && db::has_atmospheres(tx, album).map_err(err)? {
        return Err("This album already has saved atmospheres.".into());
    }
    let mut saved = db::atmospheres(tx, album, style).map_err(err)?;
    tx.execute("INSERT INTO atmosphere_variants(album_id,style,idx,file)
        SELECT ?1,?2,COALESCE(MAX(idx),-1)+1,?3 FROM (
          SELECT idx FROM atmosphere_variants WHERE album_id=?1 AND style=?2
          UNION ALL SELECT idx FROM atmospheres WHERE album_id=?1 AND ?2='cinematic')",
        (album,style,file)).map_err(err)?;
    tx.execute("INSERT INTO painting_prompts(file,prompt) VALUES(?1,?2)", (file,prompt)).map_err(err)?;
    if style == "auto" {
        let key = format!("atmos_auto_style.{album}");
        if previous.is_empty() {
            // Older collections can have only an album-wide label.
            if let Some(first) = saved.first() {
                let marked: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM auto_style_choices WHERE first_file=?1)",
                    [first], |r| r.get(0)).map_err(err)?;
                if !marked {
                    let label = db::get_setting(tx, &key).map_err(err)?;
                    style_history::record(tx, first, label.as_deref().unwrap_or("AI choice"), None, true).map_err(err)?;
                }
            }
            style_history::record(tx, file, name, Some(technique), false).map_err(err)?;
        } else if !previous.iter().any(|f| saved.contains(f)) {
            for deleted in previous {
                tx.execute("UPDATE auto_style_choices SET first_file=?2 WHERE first_file=?1", (deleted,file)).map_err(err)?;
            }
        }
        db::set_setting(tx, &key, name).map_err(err)?;
    }
    saved.push(file.clone());
    Ok(saved)
}

pub fn delete(conn: &mut Connection, data_dir: &std::path::Path, album: &str, style: &str, file: &str) -> crate::R<()> {
    use crate::err;
    use std::{fs, io::ErrorKind};
    let name = file.strip_prefix("atmos/").filter(|s| s.ends_with(".jpg") && !s.chars().any(forbidden) && !reserved(s))
        .ok_or("Invalid atmosphere image.")?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(err)?;
    let mut files = crate::db::atmospheres(&tx, album, style).map_err(err)?;
    let index = files.iter().position(|f| f == file).ok_or("This atmosphere is no longer in this series.")?;
    files.retain(|f| f != file);
    crate::db::replace_atmospheres(&tx, album, style, &files).map_err(err)?;
    if style == "auto" {
        if let Some(next) = files.get(index) {
            tx.execute("UPDATE auto_style_choices SET first_file=?2 WHERE first_file=?1
                AND NOT EXISTS(SELECT 1 FROM auto_style_choices WHERE first_file=?2)", (file, next)).map_err(err)?;
        }
        if let Some(label) = tx.query_row("SELECT h.label FROM atmosphere_variants v JOIN auto_style_choices h
            ON h.first_file=v.file WHERE v.album_id=?1 AND v.style='auto' ORDER BY v.idx DESC,h.seq DESC LIMIT 1",
            [album], |r| r.get::<_,String>(0)).optional().map_err(err)? {
            crate::db::set_setting(&tx, &format!("atmos_auto_style.{album}"), &label).map_err(err)?;
        }
    }
    let shared: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM atmosphere_variants WHERE file=?1)
        OR EXISTS(SELECT 1 FROM atmospheres WHERE file=?1)", [file], |row| row.get(0)).map_err(err)?;
    if shared { return tx.commit().map_err(err); }
    tx.execute("DELETE FROM painting_prompts WHERE file=?1", [file]).map_err(err)?;

    let dir = data_dir.join("atmos");
    let path = dir.join(name);
    let staged = match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() || meta.is_symlink() => {
            let temp = tempfile::tempdir_in(dir).map_err(err)?;
            fs::rename(&path, temp.path().join("image")).map_err(err)?;
            Some(temp)
        }
        Ok(_) => return Err("The atmosphere path is not an image file.".into()),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(err(e)),
    };
    if let Err(e) = tx.commit() {
        if let Some(staged) = staged {
            if let Err(restore) = fs::rename(staged.path().join("image"), &path) {
                return Err(format!("{e}. Could not restore image: {restore}. Backup: {}", staged.keep().display()));
            }
        }
        return Err(err(e));
    }
    Ok(())
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
    fn new_batches_append_and_deleting_during_generation_never_restores_a_painting() {
        let temp = tempfile::tempdir().unwrap();
        let mut c = crate::db::open(std::path::Path::new(":memory:")).unwrap();
        c.execute("INSERT INTO albums(id,artist,title,sort_key) VALUES('a','Artist','Album','a')", []).unwrap();
        let old = "atmos/old.jpg".to_owned();
        let first = "atmos/one.jpg".to_owned();
        let second = "atmos/two.jpg".to_owned();
        crate::db::replace_atmospheres(&c, "a", "auto", std::slice::from_ref(&old)).unwrap();
        crate::db::set_setting(&c, "atmos_auto_style.a", "Ink").unwrap();
        c.execute("INSERT INTO painting_prompts VALUES(?1,'old prompt')", [&old]).unwrap();
        std::fs::create_dir(temp.path().join("atmos")).unwrap();
        std::fs::write(temp.path().join(&old), b"old pixels").unwrap();
        let tx = c.transaction().unwrap();
        assert!(append_generated(&tx,"a","auto",std::slice::from_ref(&first),"Oil","painting","new",true).is_err());
        let saved = append_generated(&tx,"a","auto",std::slice::from_ref(&first),"Oil","painting","new",false).unwrap();
        tx.commit().unwrap();
        assert_eq!(saved, [old.clone(), first.clone()]);
        assert_eq!(credit(&c,"a","auto",&old).unwrap().unwrap().style.as_deref(),Some("Ink"));
        assert_eq!(credit(&c,"a","auto",&first).unwrap().unwrap().style.as_deref(),Some("Oil"));
        delete(&mut c,temp.path(),"a","auto",&first).unwrap();
        let tx = c.transaction().unwrap();
        let saved = append_generated(&tx,"a","auto",&[first,second.clone()],"Oil","painting","newer",false).unwrap();
        tx.commit().unwrap();
        assert_eq!(saved, [old.clone(), second.clone()]);
        assert_eq!(credit(&c,"a","auto",&second).unwrap().unwrap().style.as_deref(),Some("Oil"));
        assert_eq!(std::fs::read(temp.path().join(&old)).unwrap(), b"old pixels");
        assert_eq!(c.query_row("SELECT prompt FROM painting_prompts WHERE file=?1", [&old], |r| r.get::<_,String>(0)).unwrap(),"old prompt");
    }
    #[test]
    fn adding_to_a_legacy_cinematic_series_keeps_its_order_and_other_styles() {
        let mut c = crate::db::open(std::path::Path::new(":memory:")).unwrap();
        c.execute("INSERT INTO atmospheres VALUES('a',0,'atmos/legacy.jpg')", []).unwrap();
        crate::db::replace_atmospheres(&c, "a", "sumi", &["atmos/ink.jpg".into()]).unwrap();
        let batch = ["atmos/one.jpg".to_owned(), "atmos/two.jpg".to_owned()];
        for n in 1..=batch.len() {
            let tx = c.transaction().unwrap();
            append_generated(&tx, "a", "cinematic", &batch[..n], "Cinematic", "", "prompt", false).unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(crate::db::atmospheres(&c, "a", "cinematic").unwrap(), ["atmos/legacy.jpg","atmos/one.jpg","atmos/two.jpg"]);
        assert_eq!(crate::db::atmospheres(&c, "a", "sumi").unwrap(), ["atmos/ink.jpg"]);
    }
    #[test]
    fn deletion_targets_only_indexed_paintings_and_keeps_shared_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        std::fs::create_dir(path.join("atmos")).unwrap();
        let mut c = crate::db::open(std::path::Path::new(":memory:")).unwrap();
        let files = vec!["atmos/first.jpg".to_owned(), "atmos/second.jpg".to_owned()];
        crate::db::replace_atmospheres(&c, "a", "auto", &files).unwrap();
        crate::style_history::record(&c, &files[0], "Ink", None, false).unwrap();
        c.execute("INSERT INTO atmospheres VALUES('b',0,?1)", [&files[0]]).unwrap();
        c.execute("INSERT INTO painting_prompts VALUES(?1,'ink')", [&files[0]]).unwrap();
        for file in &files { std::fs::write(path.join(file), b"painting").unwrap(); }
        for (album, style, file) in [("a","cel",files[0].as_str()), ("b","auto",&files[0]),
            ("a","auto","atmos/../art/cover.jpg"), ("a","auto","personal/1-1")] {
            assert!(delete(&mut c, path, album, style, file).is_err());
        }
        delete(&mut c, path, "a", "auto", &files[0]).unwrap();
        assert_eq!(crate::db::atmospheres(&c, "a", "auto").unwrap(), files[1..]);
        assert_eq!(c.query_row("SELECT first_file FROM auto_style_choices", [], |r| r.get::<_,String>(0)).unwrap(), files[1]);
        assert!(path.join(&files[0]).is_file());
        delete(&mut c, path, "b", "cinematic", &files[0]).unwrap();
        assert!(!path.join(&files[0]).exists());
        assert!(path.join(&files[1]).is_file());
        assert_eq!(c.query_row("SELECT count(*) FROM painting_prompts", [], |r| r.get::<_,i64>(0)).unwrap(), 0);
    }
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
