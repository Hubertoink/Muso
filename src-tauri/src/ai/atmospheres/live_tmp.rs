use super::*;

#[test]
#[ignore = "live OpenAI run"]
fn live_three_albums() {
    let data = std::path::PathBuf::from(std::env::var("APPDATA").unwrap()).join("app.muso.player");
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../output/Harness-Test/Neu-Harness");
    std::fs::create_dir_all(&out).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::copy(data.join("muso.db"), tmp.path().join("muso.db")).unwrap();
    for side in ["muso.db-wal", "muso.db-shm"] {
        let _ = std::fs::copy(data.join(side), tmp.path().join(side));
    }
    let conn = crate::db::open(&tmp.path().join("muso.db")).unwrap();
    let key = crate::db::get_setting(&conn, "openai_key").unwrap().unwrap();
    let ids: Vec<String> = std::env::var("MUSO_LIVE_IDS").unwrap().split(',').map(str::to_owned).collect();
    for id in ids {
        let album = crate::db::album(&conn, &id).unwrap().unwrap();
        let tracks = crate::db::tracks(&conn, &id).unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("art")).unwrap();
        let cover = album.cover_file.clone().unwrap_or_else(|| format!("{id}.jpg"));
        let _ = std::fs::copy(data.join("art").join(&cover), dir.path().join("art").join(&cover));
        let refs = crate::visual_context::References::load(&conn, &album).unwrap();
        let result = tauri::async_runtime::block_on(async {
            let mut context = AlbumContext::default();
            context.research = research_album(&key, &album, &tracks, &context).await?;
            std::fs::write(out.join(format!("{id}-research.txt")), &context.research).unwrap();
            let mut log = String::new();
            let files = generate(&key, &album, &tracks, dir.path(), &context, refs, None, 3, false,
                |step, msg| eprintln!("[{}] {step} {msg}", album.title),
                |files, name, prompt| { log += &format!("== {} · {name}\n{prompt}\n\n", files.len()); Ok(()) }).await?;
            std::fs::write(out.join(format!("{id}-prompts.txt")), &log).unwrap();
            for (i, f) in files.iter().enumerate() {
                std::fs::copy(dir.path().join(f), out.join(format!("{id}-{}.jpg", i + 1))).unwrap();
            }
            Ok::<_, String>(files.len())
        });
        result.unwrap_or_else(|error| panic!("{}: {error}", album.title));
    }
}
