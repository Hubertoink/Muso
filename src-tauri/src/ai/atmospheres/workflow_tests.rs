use super::*;
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};

// Exercise the real request bodies, pixel decoding, review and publish path without API billing.
fn read_request(stream: &TcpStream) -> Option<Value> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap() == 0 {
        return None;
    }
    let mut size = None;
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("content-length") {
                size = Some(value.trim().parse::<usize>().unwrap());
            }
        }
    }
    let mut bytes = vec![0; size.unwrap()];
    reader.read_exact(&mut bytes).unwrap();
    Some(serde_json::from_slice(&bytes).unwrap())
}

/// Each verdict is (near_duplicate, grounded, craft_ok). An empty `expected` means the set must fail.
fn run_case(with_cover: bool, allow_repaint: bool, verdicts: &[(bool, bool, bool)], expected_paints: usize, expected: &[&str]) {
    run_styled_case(with_cover, allow_repaint, verdicts, expected_paints, expected, None, None, ImageSettings::default());
}

fn run_styled_case(with_cover: bool, allow_repaint: bool, verdicts: &[(bool, bool, bool)], expected_paints: usize, expected: &[&str], style: Option<Preset>, persona: Option<&'static personas::Persona>, images: ImageSettings) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");
    let mut reviews = verdicts.to_vec().into_iter();
    let (requested, saved, api_size) = match images.resolution {
        Resolution::FullHd => ((1920, 1088), (1920, 1080), "1920x1088"),
        Resolution::UltraHd => ((3840, 2160), (3840, 2160), "3840x2160"),
    };
    let server = std::thread::spawn(move || {
        let mut jpeg = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(requested.0, requested.1).write_to(&mut jpeg, image::ImageFormat::Jpeg).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(jpeg.into_inner());
        let mut steps = Vec::new();
        let mut qualities = Vec::new();
        loop {
            let (mut stream, _) = listener.accept().unwrap();
            let Some(body) = read_request(&stream) else { break };
            let stage =
                body.pointer("/response_format/json_schema/name").and_then(Value::as_str).unwrap_or("paint");
            steps.push(stage.to_owned());
            let model = match images.model {
                ImageModel::Flare => "gpt-image-2.6-flare",
                ImageModel::Sunburst => "gpt-image-2.6-sunburst",
            };
            assert_eq!(body["model"], if stage == "paint" { model } else { "gpt-6.2-sol" });
            let parts = &body["messages"][1]["content"];
            if let Some(artist) = persona {
                let brief = if stage == "album_scenes" { artist.planning_brief(style.is_some()) } else { artist.commission() };
                let request = match stage {
                    "paint" => &body["prompt"],
                    "image_review" => &parts[0]["text"],
                    _ => &parts[1]["text"],
                };
                assert!(request.as_str().unwrap().contains(&brief), "Artist commission missing at {stage}");
            }
            let response = match stage {
                "album_scenes" => {
                    assert_eq!(parts.as_array().unwrap().iter().any(|v| v["type"] == "image_url"), with_cover && style.is_none());
                    assert!(parts[0]["text"].as_str().unwrap().contains("Die zwei Achsen"));
                    assert!(!parts[0]["text"].as_str().unwrap().contains("## Anwendung an den drei Pilotplatten"));
                    assert!(parts[1]["text"].as_str().unwrap().contains(if style.is_some() { "supplied" } else { "catalogue" }));
                    let input: Value = serde_json::from_str(parts[1]["text"].as_str().unwrap()).unwrap();
                    let history = input.pointer("/medium/technique_history");
                    assert_eq!(input.get("artist_commission").is_some(), persona.is_some());
                    if style.is_some() {
                        assert!(history.is_none());
                    } else {
                        let history = history.unwrap();
                        assert_eq!(history["recent_choices_newest_first"][0]["style_name"], "Pigment Monotype");
                        assert_eq!(history["frequent_techniques"][0], json!({"technique":"monotype","series":2}));
                    }
                    json!({"album_mood":"Tense","album_portrait":"Alternating marks",
                        "style_reason":"Documented printing","style_name":"Monotype","technique":"monotype","style_direction":"Layered ink",
                        "scenes":[tests::fixture_scene("First scene"),tests::fixture_scene("Second scene"),tests::fixture_scene("Third scene")]})
                }
                "scene_review" => {
                    assert_eq!(parts.as_array().unwrap().iter().any(|p| p["type"] == "image_url"), with_cover);
                    let edits = [3, 1, 2].map(|i| json!({"scene_number":i,"visible_detail":"Spacing between marks","visual_prompt":format!("Scene {i}")}));
                    json!({"scenes":edits})
                }
                "paint" => {
                    assert_eq!(body["size"], api_size);
                    let prompt = body["prompt"].as_str().unwrap();
                    for i in 1..=3 { assert!(prompt.contains(&format!("\n{i}. "))); }
                    assert!(prompt.contains("Render only scene "));
                    assert!(prompt.contains("Composition: Two figures stand shoulder to shoulder inside a narrow doorway."));
                    assert!(prompt.contains("Emotional reading: Their shoulders almost touch, making the pause between them feel tentative."));
                    assert!(prompt.contains("Mood: restrained. Light: low side light. Palette: ochre, slate."));
                    assert!(prompt.contains(style.map_or(CUSTOM_SURFACE, |p| p.surface)));
                    assert!(prompt.contains(style.map_or("Layered ink", |p| p.look)));
                    qualities.push(body["quality"].as_str().unwrap().to_owned());
                    json!({"data":[{"b64_json":encoded}]})
                }
                "image_review" => {
                    let parts = parts.as_array().unwrap();
                    assert_eq!(
                        parts.iter().filter(|p| p["type"] == "image_url").count(),
                        1 + usize::from(with_cover)
                    );
                    assert!(parts[parts.len() - 2]["text"].as_str().unwrap().starts_with("B · NEW RENDER"));
                    let (duplicate, grounded, craft) = reviews.next().expect("review budget exceeded");
                    json!({"near_duplicate":duplicate,"grounded":grounded,"mood_matches":true,"craft_ok":craft,"explicit_violation":!craft,
                        "reason":"Visual audit","replacement":"Revised scene"})
                }
                _ => panic!("Unexpected stage {stage}"),
            };
            let reply = if stage == "paint" {
                response
            } else {
                json!({"choices":[{"message":{"content":response.to_string()}}]})
            }
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            )
            .unwrap();
        }
        assert!(reviews.next().is_none());
        (steps, qualities)
    });
    let temp = tempfile::tempdir().unwrap();
    let api = AtmosphereApi {
        client: reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(10)).build().unwrap(),
        key: "test",
        base: &base,
        images,
    };
    let mut refs = VisualInputs::default();
    let db = crate::db::open(&temp.path().join("library.db")).unwrap();
    for name in ["Tonal Monotype", "Pigment Monotype"] {
        crate::style_history::record(&db, name, name, None, false).unwrap();
    }
    refs.style_history = crate::style_history::StyleHistory::load(&db).unwrap();
    if with_cover {
        refs.cover = Some(image_part(&DynamicImage::new_rgb8(32, 32)).unwrap());
    }
    let mut prompts = Vec::new();
    let result = tauri::async_runtime::block_on(generate_set(
        &api,
        "Test",
        &json!({"tracks":["a","b","c"]}),
        &refs,
        temp.path(),
        style,
        3,
        allow_repaint,
        persona,
        |_, _| {},
        |files, medium, technique, prompt| {
            assert_eq!(technique, "monotype");
            assert_eq!(medium, style.map_or("Monotype", |p| p.label));
            assert_eq!(files.len(), prompts.len() + 1);
            assert_eq!(image::image_dimensions(temp.path().join(files.last().unwrap())).unwrap(), saved);
            prompts.push(prompt.to_owned());
            Ok(())
        },
    ));
    drop(TcpStream::connect(addr).unwrap());
    let (stages, qualities) = server.join().unwrap();
    assert_eq!(qualities.len(), expected_paints);
    // Only repaints after a failed review use the more expensive quality.
    assert_eq!(qualities.iter().filter(|q| q.as_str() == "high").count(), expected_paints - 3);
    assert_eq!(stages[0..2], ["album_scenes", "scene_review"]);
    if expected.is_empty() {
        assert!(result.unwrap_err().contains("No painting passed"));
        assert!(prompts.is_empty());
        assert_eq!(temp.path().join("atmos").read_dir().unwrap().count(), 0);
    } else {
        assert_eq!(result.unwrap().len(), expected.len());
        for (prompt, scene) in prompts.iter().zip(expected) {
            assert!(prompt.split("Keep the other scenes out of this frame. ").nth(1).unwrap().starts_with(scene), "{prompt} should contain {scene}");
        }
    }
}

#[test]
fn every_painting_is_reviewed_and_repainted_at_most_once() {
    let ok = (false, true, true);
    run_case(false, false, &[(false, false, true), ok, ok], 3, &["Scene 2", "Scene 3"]);
    // A cover restaging is replaced by the revised scene.
    run_case(true, true, &[(true, true, true), ok, ok, ok], 4, &["Revised scene", "Scene 2", "Scene 3"]);
    // Both ungrounded versions are skipped; a later scene is repaired.
    run_case(false, true, &[(false, false, true), (false, false, true), ok, (false, true, false), ok], 5,
        &["Scene 2", "Revised scene"]);
    run_case(false, true, &[ok, ok, ok], 3, &["Scene 1", "Scene 2", "Scene 3"]);
}

#[test]
fn restagings_of_the_cover_are_never_published() {
    let copy = (true, true, true);
    run_case(true, true, &[copy; 6], 6, &[]);
    run_case(false, true, &[(false, true, false); 6], 6, &[]);
}

#[test]
fn full_hd_crop_preserves_the_entire_width_and_centres_vertically() {
    use image::{GenericImage, GenericImageView, Rgba};
    let mut original = DynamicImage::new_rgb8(1920, 1088);
    original.put_pixel(0, 4, Rgba([12, 34, 56, 255]));
    original.put_pixel(1919, 1083, Rgba([56, 34, 12, 255]));
    let result = Resolution::FullHd.finish(original).unwrap();
    assert_eq!(result.dimensions(), (1920, 1080));
    assert_eq!(result.get_pixel(0, 0), Rgba([12, 34, 56, 255]));
    assert_eq!(result.get_pixel(1919, 1079), Rgba([56, 34, 12, 255]));
    assert!(Resolution::FullHd.finish(DynamicImage::new_rgb8(1536, 1024)).is_err());
    assert!(Resolution::UltraHd.finish(result).is_err());
}

#[test]
fn every_render_has_the_full_ordered_series_and_one_target() {
    let scene: Scene = serde_json::from_value(tests::fixture_scene("Ochre strata")).unwrap();
    let look = Look { medium: "Ink".into(), surface: "Matte", portrait: "Ochre".into() };
    for count in 3..=5 {
        let series: Vec<_> = (1..=count).map(|n| format!("Composition {n}")).collect();
        for index in 0..count {
            let prompt = render_prompt(&series, index, &scene, &look);
            let (outline, instruction) = prompt.split_once("Render only scene ").unwrap();
            let descriptions: Vec<_> = outline.lines().skip(1).filter(|s| !s.is_empty()).collect();
            assert_eq!(descriptions.len(), count);
            for (i, description) in descriptions.iter().enumerate() {
                assert_eq!(*description, format!("{}. {}", i + 1, series[i]));
            }
            assert!(instruction.starts_with(&format!("{} as one widescreen", index + 1)));
        }
    }
}

#[test]
fn manual_style_applies_its_look_and_surface_even_when_the_planner_proposes_another_medium() {
    let artist = resolve_persona(Some("Editorial illustrator"), false).unwrap().unwrap();
    assert_eq!(artist.name, "Editorial illustrator");
    run_styled_case(true, false, &[(false, true, true); 3], 3, &["Scene 1", "Scene 2", "Scene 3"], preset("landscapephoto"), Some(artist), ImageSettings::default());
}

#[test]
fn automatic_series_keeps_its_commission_through_a_repaint() {
    let ok = (false, true, true);
    let images = serde_json::from_value(json!({"model":"sunburst","resolution":"4k"})).unwrap();
    run_styled_case(true, true, &[(true, true, true), ok, ok, ok], 4,
        &["Revised scene", "Scene 2", "Scene 3"], None, Some(personas::draw()), images);
}
