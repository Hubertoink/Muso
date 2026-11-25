use rusqlite::{params, Connection};
use serde::Serialize;

// Older series only have a display name. Recognise processes in that name
// without treating a subject, colour or mood as a technique.
fn technique(term: &str, inferred: bool) -> Option<String> {
    let words: Vec<_> = term
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    let normalized = words.join(" ");
    let padded = format!(" {normalized} ");
    let aliases: &[(&str, &[&str])] = &[
        ("monotype", &["monotype", "monotypie", "monotypy"]),
        ("monoprint", &["monoprint", "monoprinting"]),
        ("woodcut", &["woodcut", "woodblock", "ukiyo e"]),
        ("linocut", &["linocut", "linoleum cut"]),
        (
            "screen printing",
            &["screen printing", "screenprint", "silkscreen", "serigraphy"],
        ),
        ("risograph", &["risograph", "risography"]),
        (
            "gelatin silver print",
            &["gelatin silver", "silver gelatin"],
        ),
        ("collage", &["collage", "photocollage"]),
        ("watercolor", &["watercolor", "watercolour", "aquarelle"]),
        ("oil painting", &["oil painting", "oil on canvas"]),
        (
            "acrylic painting",
            &["acrylic painting", "acrylic on canvas"],
        ),
        ("ink wash", &["ink wash", "sumi e"]),
        ("gouache", &["gouache"]),
        (
            "photography",
            &["photograph", "photography", "photographic"],
        ),
        ("pastel", &["pastel painting", "soft pastel"]),
        ("pixel mosaic", &["pixel mosaic"]),
        ("pixel art", &["pixel art"]),
        ("printmaking", &["printmaking"]),
        ("painting", &["painting"]),
    ];
    for (name, spellings) in aliases {
        if spellings.iter().any(|s| padded.contains(&format!(" {s} "))) {
            // New plans already identify the process. Preserve more specific
            // names such as "encaustic painting" and "cyanotype photography".
            if !inferred
                && matches!(*name, "painting" | "printmaking" | "photography")
                && normalized != *name
            {
                return Some(normalized);
            }
            return Some((*name).to_owned());
        }
    }
    (!inferred && !normalized.is_empty()).then_some(normalized)
}

pub fn record(
    db: &Connection,
    file: &str,
    label: &str,
    process: Option<&str>,
    imported: bool,
) -> rusqlite::Result<()> {
    let medium = process
        .and_then(|p| technique(p, false))
        .or_else(|| technique(label, true));
    db.execute(
        "INSERT INTO auto_style_choices(first_file,label,technique,imported) VALUES(?1,?2,?3,?4)",
        params![file, label, medium, imported],
    )?;
    Ok(())
}

// Called inside the library migration transaction. Reconstruct the available
// choices once, in saved order, counting a whole series as one decision.
pub fn migrate(db: &Connection) -> rusqlite::Result<()> {
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='auto_style_choices')",
        [],
        |r| r.get(0),
    )?;
    if exists {
        return Ok(());
    }
    db.execute_batch("CREATE TABLE auto_style_choices(
        seq INTEGER PRIMARY KEY, first_file TEXT NOT NULL, label TEXT NOT NULL, technique TEXT, imported INTEGER NOT NULL);
        CREATE INDEX auto_style_first_file ON auto_style_choices(first_file);")?;
    let mut query = db.prepare(
        "SELECT v.file,s.value FROM atmosphere_variants v
        JOIN settings s ON s.key='atmos_auto_style.'||v.album_id
        WHERE v.style='auto' AND v.idx=0 AND trim(s.value)<>'' ORDER BY v.rowid",
    )?;
    let rows = query.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (file, label) = row?;
        record(db, &file, &label, None, false)?;
    }
    Ok(())
}

#[derive(Default, Serialize)]
pub struct StyleHistory {
    frequent_techniques: Vec<Frequency>,
    recent_choices_newest_first: Vec<Choice>,
}

#[derive(Serialize)]
struct Frequency {
    technique: String,
    series: usize,
}
#[derive(Serialize)]
struct Choice {
    style_name: String,
    technique: Option<String>,
}

impl StyleHistory {
    pub fn load(db: &Connection) -> rusqlite::Result<Self> {
        let mut counts = db.prepare(
            "SELECT technique,count(*) FROM auto_style_choices WHERE technique IS NOT NULL
            GROUP BY technique ORDER BY count(*) DESC, max(seq) DESC LIMIT 8",
        )?;
        let frequent_techniques = counts
            .query_map([], |r| {
                Ok(Frequency {
                    technique: r.get(0)?,
                    series: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut choices = db.prepare("SELECT label,technique FROM auto_style_choices WHERE imported=0 ORDER BY seq DESC LIMIT 8")?;
        let recent_choices_newest_first = choices
            .query_map([], |r| {
                Ok(Choice {
                    style_name: r.get(0)?,
                    technique: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self {
            frequent_techniques,
            recent_choices_newest_first,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_choice_per_series_with_free_names_and_a_chronological_history() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT);
            CREATE TABLE atmosphere_variants(album_id TEXT,style TEXT,idx INTEGER,file TEXT);
            INSERT INTO settings VALUES('atmos_auto_style.a','Pigment Monotype'),('atmos_auto_style.b','Tonal Monotype');
            INSERT INTO atmosphere_variants VALUES('a','auto',0,'a0'),('a','auto',1,'a1'),
                ('b','auto',0,'b0'),('a','ukiyoe',0,'manual');").unwrap();
        migrate(&db).unwrap();
        migrate(&db).unwrap();
        record(&db, "new", "Roadside Monotype", Some("monotype"), false).unwrap();
        record(&db, "newer", "After the Rain", Some("cyanotype"), false).unwrap();
        record(&db, "import", "Digital Collage", None, true).unwrap();
        let history = StyleHistory::load(&db).unwrap();
        let recent: Vec<_> = history
            .recent_choices_newest_first
            .iter()
            .map(|c| c.style_name.as_str())
            .collect();
        assert_eq!(
            recent,
            [
                "After the Rain",
                "Roadside Monotype",
                "Tonal Monotype",
                "Pigment Monotype"
            ]
        );
        assert_eq!(history.frequent_techniques[0].technique, "monotype");
        assert_eq!(history.frequent_techniques[0].series, 3);
        assert_eq!(
            history.recent_choices_newest_first[0].technique.as_deref(),
            Some("cyanotype")
        );
        assert!(history
            .frequent_techniques
            .iter()
            .any(|f| f.technique == "collage" && f.series == 1));
        assert_eq!(
            technique("Monochrome photographic print", true).as_deref(),
            Some("photography")
        );
        assert_eq!(technique("Ambient Drift", true), None);
    }
}
