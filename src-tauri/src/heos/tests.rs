use super::*;
use crate::audio::AudioBus;
use rodio::{Decoder, Source};
use serde_json::{json, Value};
use std::io::Cursor;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[tokio::test]
async fn switching_rooms_reuses_the_audio_port_and_retires_only_the_old_url() {
    let ip = Ipv4Addr::LOCALHOST.into();
    let server = stream::StreamServer::bind(SocketAddr::new(ip, stream::STREAM_PORT))
        .await
        .unwrap();
    #[cfg(target_os = "linux")]
    assert_eq!(server.addr.port(), 48125);
    let bus = AudioBus::with_format(2, 44100);
    let first = stream::Session::serve(server.clone(), 1, bus.clone()).unwrap();
    let second = stream::Session::serve(server, 2, bus).unwrap();
    let old_url = first.url.clone();
    assert_ne!(old_url, second.url);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    assert!(http
        .head(&old_url)
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    assert!(!first.listening());
    drop(first);
    assert_eq!(http.get(old_url).send().await.unwrap().status(), 404);
    let audio = http.get(&second.url).send().await.unwrap();
    assert!(audio.status().is_success());
    second.activate().await.unwrap();
}

#[tokio::test]
async fn creates_edits_and_dissolves_groups_preserving_leader_and_detecting_stale_edits() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fake = tokio::spawn(async move {
        let mut socket = BufReader::new(listener.accept().await.unwrap().0);
        let mut group: Vec<i64> = vec![];
        let mut commands = vec![];
        loop {
            let mut line = String::new();
            if socket.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            let command = line.trim().strip_prefix("heos://").unwrap();
            let (name, arg) = command.split_once('?').unwrap_or((command, ""));
            let payload = match name {
                "player/get_players" => {
                    json!([{"pid":"-12","name":"Desk"},{"pid":34,"name":"Den"},{"pid":56,"name":"Kitchen"}])
                }
                "group/get_groups" if group.is_empty() => json!([]),
                "group/get_groups" => {
                    json!([{"gid":group[0],"name":"Den group","players":group.iter().enumerate().map(|(i,id)| json!({"pid":id,"role":if i==0 {"leader"} else {"member"}})).collect::<Vec<_>>()}])
                }
                "group/set_group" => {
                    group = arg
                        .strip_prefix("pid=")
                        .unwrap()
                        .split(',')
                        .map(|s| s.parse().unwrap())
                        .collect();
                    commands.push(group.clone());
                    if group.len() == 1 {
                        group.clear();
                    }
                    Value::Null
                }
                _ => panic!("Unexpected command {name}"),
            };
            let response =
                json!({"heos":{"command":name,"result":"success","message":""},"payload":payload});
            socket
                .get_mut()
                .write_all(format!("{response}\r\n").as_bytes())
                .await
                .unwrap();
        }
        commands
    });
    let mut cli = Client::connect(address).await.unwrap();
    let (_, groups) = configure_group(&mut cli, &[34, -12], vec![]).await.unwrap();
    assert_eq!(groups[0].leader(), 34);
    assert!(configure_group(&mut cli, &[34, 56], vec![]).await.is_err());
    configure_group(&mut cli, &[34, -12, 56], vec![34, -12])
        .await
        .unwrap();
    configure_group(&mut cli, &[34, 56], vec![34, 56, -12])
        .await
        .unwrap();
    let (_, groups) = configure_group(&mut cli, &[34], vec![56, 34])
        .await
        .unwrap();
    assert!(groups.is_empty());
    drop(cli);
    assert_eq!(
        fake.await.unwrap(),
        [vec![34, -12], vec![34, -12, 56], vec![34, 56], vec![34]]
    );
}

#[tokio::test]
async fn speaker_fetches_live_decodable_music_and_disconnect_returns_the_same_sink_to_local() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let speaker = tokio::spawn(async move {
        let mut cli = BufReader::new(listener.accept().await.unwrap().0);
        let mut command = String::new();
        cli.read_line(&mut command).await.unwrap();
        let url = command
            .trim()
            .strip_prefix("heos://browse/play_stream?pid=-12&url=")
            .unwrap();
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        assert_eq!(
            http.head(url).send().await.unwrap().headers()["content-type"],
            "audio/mpeg"
        );
        let pending = br#"{"heos":{"command":"browse/play_stream","result":"success","message":"command under process"}}"#;
        cli.get_mut().write_all(pending).await.unwrap();
        cli.get_mut().write_all(b"\r\n").await.unwrap();
        let mut stream = http
            .get(url)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        cli.get_mut()
            .write_all(
                b"{\"heos\":{\"command\":\"browse/play_stream\",\"result\":\"success\"}}\r\n",
            )
            .await
            .unwrap();
        let mut data = Vec::new();
        while data.len() < 20000 {
            data.extend(stream.chunk().await.unwrap().expect("live frame"));
        }
        data
    });
    let bus = AudioBus::with_format(2, 48000);
    let sink = bus.sink();
    sink.append(rodio::source::SineWave::new(440.).amplify(0.3));
    let mut cli = Client::connect(addr).await.unwrap();
    let server = stream::StreamServer::bind(SocketAddr::new(cli.local_ip().unwrap(), 0))
        .await
        .unwrap();
    let mut remote = stream::Session::serve(server, -12, bus.clone()).unwrap();
    start_stream(&mut cli, &mut remote).await.unwrap();
    assert!(!bus.local());
    assert!(bus.reader(0).take(512).all(|s| s == 0.));
    let bytes = tokio::time::timeout(Duration::from_secs(8), speaker)
        .await
        .unwrap()
        .unwrap();
    let decoder = Decoder::new(Cursor::new(bytes)).unwrap();
    assert_eq!((decoder.channels(), decoder.sample_rate()), (2, 44100));
    assert!(decoder.map(i16::unsigned_abs).max().unwrap() > 2000);
    drop(remote);
    assert!(bus.local());
    assert!(bus.reader(0).take(1024).any(|s| s.abs() > 0.01));
}

#[tokio::test]
async fn hardware_pause_keeps_the_stream_warm_and_radio_only_players_restart_the_url() {
    for native_pause in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let fake = tokio::spawn(async move {
            let mut cli = BufReader::new(listener.accept().await.unwrap().0);
            let mut stream = None;
            let mut commands = vec![];
            let http = reqwest::Client::builder().no_proxy().build().unwrap();
            loop {
                let mut line = String::new();
                if cli.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                let (name, arg) = line
                    .trim()
                    .strip_prefix("heos://")
                    .unwrap()
                    .split_once('?')
                    .unwrap();
                let mut result = "success";
                if name == "browse/play_stream" {
                    stream = Some(
                        http.get(arg.split_once("&url=").unwrap().1)
                            .send()
                            .await
                            .unwrap(),
                    );
                    commands.push("stream".to_owned());
                } else {
                    assert_eq!(name, "player/set_play_state");
                    let action = arg.strip_prefix("pid=-12&state=").unwrap();
                    commands.push(action.to_owned());
                    if action == "pause" && !native_pause {
                        result = "fail";
                    }
                    if action == "stop" {
                        stream = None;
                    }
                }
                let response = json!({"heos":{"command":name,"result":result}});
                cli.get_mut()
                    .write_all(format!("{response}\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            drop(stream);
            commands
        });
        let mut cli = Client::connect(address).await.unwrap();
        let bus = AudioBus::with_format(2, 44100);
        let server = stream::StreamServer::bind(SocketAddr::new(cli.local_ip().unwrap(), 0))
            .await
            .unwrap();
        let mut session = stream::Session::serve(server, -12, bus).unwrap();
        set_transport(&mut cli, &mut session, true).await.unwrap();
        set_transport(&mut cli, &mut session, false).await.unwrap();
        assert!(session.transport != Transport::Playing);
        set_transport(&mut cli, &mut session, true).await.unwrap();
        assert!(session.transport == Transport::Playing);
        drop(cli);
        assert_eq!(
            fake.await.unwrap(),
            if native_pause {
                vec!["stream", "pause", "play"]
            } else {
                vec!["stream", "pause", "stop", "stream"]
            }
        );
    }
}

#[tokio::test]
async fn volume_reads_and_writes_only_the_selected_group_and_its_rooms() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fake = tokio::spawn(async move {
        let mut cli = BufReader::new(listener.accept().await.unwrap().0);
        let mut writes = vec![];
        loop {
            let mut line = String::new();
            if cli.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            let cmd = line.trim().strip_prefix("heos://").unwrap();
            let (name, args) = cmd.split_once('?').unwrap_or((cmd, ""));
            let mut message = args.to_string();
            let payload = match name {
                "player/get_players" => {
                    json!([{"pid":-1,"name":"Desk"},{"pid":2,"name":"Kitchen"},{"pid":3,"name":"Office"}])
                }
                "group/get_groups" => {
                    json!([{"gid":42,"name":"Downstairs","players":[{"pid":-1,"role":"leader"},{"pid":2,"role":"member"}]}])
                }
                "group/get_volume" | "player/get_volume" => {
                    message.push_str("&level=31");
                    Value::Null
                }
                "group/set_volume" | "player/set_volume" => {
                    writes.push(cmd.to_string());
                    Value::Null
                }
                _ => panic!("Unexpected command {cmd}"),
            };
            let response = json!({"heos":{"command":name,"result":"success","message":message},"payload":payload});
            cli.get_mut()
                .write_all(format!("{response}\r\n").as_bytes())
                .await
                .unwrap();
        }
        writes
    });
    let mut cli = Client::connect(address).await.unwrap();
    let result = levels(&mut cli, -1, "test".into(), None).await.unwrap();
    assert_eq!(
        result
            .controls
            .iter()
            .map(|c| (c.id, c.value))
            .collect::<Vec<_>>(),
        [(42, 31), (-1, 31), (2, 31)]
    );
    for change in [(true, 42, 40), (false, 2, 18)] {
        levels(&mut cli, -1, "test".into(), Some(change))
            .await
            .unwrap();
    }
    for invalid in [(false, 3, 50), (true, -1, 50), (false, -1, 101)] {
        assert!(levels(&mut cli, -1, "test".into(), Some(invalid))
            .await
            .is_err());
    }
    drop(cli);
    assert_eq!(
        fake.await.unwrap(),
        [
            "group/set_volume?gid=42&level=40",
            "player/set_volume?pid=2&level=18"
        ]
    );
}

#[tokio::test]
#[ignore = "requires a HEOS system on the local network; read-only"]
async fn finds_real_rooms_and_groups_without_a_configured_address() {
    let (host, rooms, groups) = protocol::discover()
        .await
        .unwrap()
        .expect("No HEOS controller found");
    println!(
        "Discovered {} rooms and {} groups",
        rooms.len(),
        groups.len()
    );
    let mut client = connect(host).await.unwrap();
    for group in &groups {
        assert!(group.players.iter().any(|p| p.pid == group.leader()));
        let snapshot = levels(&mut client, group.leader(), "read-only".into(), None)
            .await
            .unwrap();
        assert_eq!(snapshot.controls.len(), 1 + group.players.len());
        println!(
            "{}: group and {} room levels read",
            group.name,
            snapshot.controls.len() - 1
        );
    }
    if let Some(room) = rooms.iter().find(|room| {
        !groups
            .iter()
            .any(|g| g.players.iter().any(|p| p.pid == room.pid))
    }) {
        let snapshot = levels(&mut client, room.pid, "read-only".into(), None)
            .await
            .unwrap();
        assert_eq!(snapshot.controls.len(), 1);
        println!("{}: standalone room level read", room.name);
    }
}

#[tokio::test]
#[ignore = "plays a quiet test tone on the explicitly selected HEOS room"]
async fn plays_short_tone_on_selected_speaker() -> Result<(), String> {
    let pid: i64 = std::env::var("MUSO_HEOS_TEST_PID")
        .map_err(crate::err)?
        .parse()
        .map_err(crate::err)?;
    let (ip, rooms, groups) = protocol::discover().await?.ok_or("No HEOS rooms found")?;
    let room = rooms
        .iter()
        .find(|r| r.pid == pid)
        .ok_or("Requested room not found")?;
    if groups
        .iter()
        .any(|g| g.players.iter().any(|r| r.pid == pid))
    {
        return Err("Select a standalone speaker".into());
    }
    let mut cli = connect(ip).await?;
    let state = format!("player/get_play_state?pid={pid}");
    let before = cli.reply(&state).await?;
    if !before["heos"]["message"]
        .as_str()
        .unwrap_or("")
        .split('&')
        .any(|p| p == "state=stop")
    {
        return Err("The test room is already in use.".into());
    }
    let bus = AudioBus::with_format(2, 44100);
    let sink = bus.sink();
    let result = async {
        let server =
            stream::StreamServer::bind(SocketAddr::new(cli.local_ip()?, stream::STREAM_PORT))
                .await?;
        let mut session = stream::Session::serve(server, pid, bus)?;
        start_stream(&mut cli, &mut session).await?;
        let length = 52920;
        let samples: Vec<f32> = (0..length)
            .map(|i| {
                let fade = ((length - i).min(i) as f32 / 4410.).min(1.);
                (std::f32::consts::TAU * 330. * i as f32 / 44100.).sin() * 0.035 * fade
            })
            .collect();
        sink.append(rodio::buffer::SamplesBuffer::new(1, 44100, samples));
        tokio::time::sleep(Duration::from_secs(4)).await;
        let response = cli.reply(&state).await?;
        let playing = response["heos"]["message"]
            .as_str()
            .unwrap_or("")
            .split('&')
            .any(|p| p == "state=play");
        if !session.listening() || !sink.empty() || !playing {
            return Err(format!("Speaker did not confirm playback: {response}"));
        }
        println!(
            "{} fetched the stream, consumed the test tone and reported play.",
            room.name
        );
        Ok(())
    }
    .await;
    cli.command(&format!("player/set_play_state?pid={pid}&state=stop"))
        .await?;
    result
}
