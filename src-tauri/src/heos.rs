mod protocol;
mod stream;
#[cfg(test)]
mod tests;

use protocol::{Client, Group, Room};
use serde::Serialize;
use std::{
    collections::HashSet,
    net::{Ipv4Addr, SocketAddr},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use stream::Transport;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;

#[derive(Default)]
pub(crate) struct Heos(Mutex<Controller>, AtomicBool);
#[derive(Default)]
struct Controller {
    host: Option<Ipv4Addr>,
    session: Option<stream::Session>,
}
#[derive(Serialize)]
pub(crate) struct Rooms {
    host: Option<Ipv4Addr>,
    players: Vec<Room>,
    groups: Vec<Group>,
    output: Option<i64>,
}
impl Controller {
    fn rooms(&self, players: Vec<Room>, groups: Vec<Group>) -> Rooms {
        Rooms {
            host: self.host,
            players,
            groups,
            output: self.session.as_ref().map(|s| s.pid),
        }
    }
    async fn client(&self) -> Result<Client, String> {
        connect(self.host.ok_or("Find your HEOS speakers first.")?).await
    }
}
async fn connect(ip: Ipv4Addr) -> Result<Client, String> {
    Client::connect(SocketAddr::new(ip.into(), 1255)).await
}

#[tauri::command]
pub(crate) async fn heos_rooms(
    state: State<'_, Heos>,
    address: Option<Ipv4Addr>,
    hint: Option<Ipv4Addr>,
) -> Result<Rooms, String> {
    let mut controller = state.0.lock().await;
    if let Some(host) = address.or(controller.host).or(hint) {
        let result = async { connect(host).await?.rooms().await }.await;
        match result {
            Ok((rooms, groups)) if !rooms.is_empty() => {
                controller.host = Some(host);
                return Ok(controller.rooms(rooms, groups));
            }
            Err(e) if address.is_some() || controller.session.is_some() => return Err(e),
            _ => controller.host = None,
        }
    }
    if let Some((host, rooms, groups)) = protocol::discover().await? {
        controller.host = Some(host);
        return Ok(controller.rooms(rooms, groups));
    }
    Ok(controller.rooms(vec![], vec![]))
}

async fn route(
    app: &AppHandle,
    controller: &mut Controller,
    client: &mut Client,
    pid: i64,
) -> Result<(), String> {
    let bus = app.state::<crate::AppState>().player.bus.clone();
    let ip = client.local_ip()?;
    let server = match &controller.session {
        Some(s) if s.server.addr.ip() == ip => s.server.clone(),
        _ => stream::StreamServer::bind(SocketAddr::new(ip, stream::STREAM_PORT)).await?,
    };
    let mut session = stream::Session::serve(server, pid, bus)?;
    if app.state::<Heos>().1.load(Ordering::Acquire) {
        start_stream(client, &mut session).await?;
    } else {
        session.reserve();
    }
    let id = session.id;
    controller.session = Some(session);
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut missed = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let state = handle.state::<Heos>();
            let mut ctl = state.0.lock().await;
            let Some(s) = &ctl.session else { break };
            if s.id != id {
                break;
            }
            missed = if s.transport != Transport::Playing || s.listening() {
                0
            } else {
                missed + 1
            };
            if missed >= 5 {
                let error = s.connection_error("The speaker stopped reading the audio stream.");
                ctl.session = None;
                let _ = handle.emit("heos-disconnected", error);
                break;
            }
        }
    });
    Ok(())
}

async fn start_stream(client: &mut Client, session: &mut stream::Session) -> Result<(), String> {
    session.hold(false);
    client
        .command(&format!(
            "browse/play_stream?pid={}&url={}",
            session.pid, session.url
        ))
        .await
        .map_err(|e| session.connection_error(&e))?;
    session
        .activate()
        .await
        .map_err(|e| session.connection_error(&e))?;
    session.transport = Transport::Playing;
    Ok(())
}

async fn set_transport(
    client: &mut Client,
    s: &mut stream::Session,
    play: bool,
) -> Result<(), String> {
    let prefix = format!("player/set_play_state?pid={}&state=", s.pid);
    if play {
        if s.transport == Transport::Paused && s.listening() {
            s.hold(false);
            if client.command(&format!("{prefix}play")).await.is_ok() && s.listening() {
                s.transport = Transport::Playing;
                return Ok(());
            }
        }
        start_stream(client, s).await
    } else {
        // Freeze the encoder as well so a warm resume has music in the receiver's buffer.
        s.hold(true);
        if client.command(&format!("{prefix}pause")).await.is_ok() {
            s.transport = Transport::Paused;
        } else {
            // Some firmware treats an HTTP stream as live radio and only supports stop.
            if let Err(e) = client.command(&format!("{prefix}stop")).await {
                s.hold(false);
                return Err(e);
            }
            s.transport = Transport::Ready;
        }
        Ok(())
    }
}

impl Heos {
    pub fn ended(&self) {
        self.1.store(false, Ordering::Release);
    }
    pub async fn playback(&self, app: &AppHandle, play: bool, new_track: bool) {
        self.1.store(play, Ordering::Release);
        // A route can be switching while a play/pause arrives. Reconcile the latest
        // intent under the same lock even when the previous output was local.
        let mut ctl = self.0.lock().await;
        let play = self.1.load(Ordering::Acquire);
        let Some(s) = &mut ctl.session else { return };
        if new_track && play && s.transport == Transport::Paused {
            s.transport = Transport::Ready;
        }
        if (s.transport == Transport::Playing) == play {
            return;
        }
        let result = async {
            let mut cli = ctl.client().await?;
            set_transport(&mut cli, ctl.session.as_mut().unwrap(), play).await
        }
        .await;
        if let Err(error) = result {
            if play {
                ctl.session = None;
                let _ = app.emit("heos-disconnected", error);
            } else {
                let _ = app.emit("heos-control-error", error);
            }
        }
    }
}

#[tauri::command]
pub(crate) async fn heos_output(
    app: AppHandle,
    state: State<'_, Heos>,
    pid: Option<i64>,
) -> Result<(), String> {
    let mut ctl = state.0.lock().await;
    let Some(pid) = pid else {
        ctl.session = None;
        return Ok(());
    };
    let mut client = ctl.client().await?;
    let (rooms, groups) = client.rooms().await?;
    if !rooms.iter().any(|r| r.pid == pid) {
        return Err("This room is no longer available.".into());
    }
    let leader = groups
        .iter()
        .find(|g| g.players.iter().any(|p| p.pid == pid))
        .map_or(pid, Group::leader);
    if ctl
        .session
        .as_ref()
        .is_some_and(|s| s.pid == leader && (s.transport != Transport::Playing || s.listening()))
    {
        return Ok(());
    }
    route(&app, &mut ctl, &mut client, leader).await
}

fn membership(groups: &[Group], leader: i64) -> Vec<i64> {
    let mut members: Vec<_> = groups
        .iter()
        .find(|g| g.leader() == leader)
        .into_iter()
        .flat_map(|g| &g.players)
        .map(|r| r.pid)
        .collect();
    members.sort_unstable();
    members
}

#[derive(Serialize)]
pub(crate) struct Levels {
    session: String,
    controls: Vec<Level>,
}
#[derive(Serialize)]
struct Level {
    id: i64,
    group: bool,
    name: String,
    value: u8,
}
impl Level {
    fn command(&self, action: &str) -> String {
        let (scope, key) = if self.group {
            ("group", "gid")
        } else {
            ("player", "pid")
        };
        format!("{scope}/{action}_volume?{key}={}", self.id)
    }
}
async fn levels(
    client: &mut Client,
    pid: i64,
    session: String,
    change: Option<(bool, i64, u8)>,
) -> Result<Levels, String> {
    let (rooms, groups) = client.rooms().await?;
    let group = groups
        .iter()
        .find(|g| g.players.iter().any(|p| p.pid == pid));
    let mut controls = vec![];
    if let Some(g) = group {
        controls.push(Level {
            id: g.gid,
            group: true,
            name: g.name.clone(),
            value: 0,
        });
    }
    for room in rooms {
        if room.pid == pid || group.is_some_and(|g| g.players.iter().any(|p| p.pid == room.pid)) {
            controls.push(Level {
                id: room.pid,
                group: false,
                name: room.name,
                value: 0,
            });
        }
    }
    if controls.is_empty() {
        return Err("This HEOS room is unavailable.".into());
    }
    if let Some((group, id, value)) = change {
        let target = controls
            .iter()
            .find(|c| c.group == group && c.id == id)
            .ok_or("This speaker is no longer part of the active output.")?;
        if value > 100 {
            return Err("Volume must be between 0 and 100.".into());
        }
        client
            .command(&format!("{}&level={value}", target.command("set")))
            .await?;
    }
    for c in &mut controls {
        let reply = client.reply(&c.command("get")).await?;
        c.value = reply["heos"]["message"]
            .as_str()
            .unwrap_or("")
            .split('&')
            .find_map(|p| p.strip_prefix("level=")?.parse::<u8>().ok())
            .filter(|n| *n <= 100)
            .ok_or("Invalid volume reported by HEOS.")?;
    }
    Ok(Levels { session, controls })
}
#[tauri::command]
pub(crate) async fn heos_levels(state: State<'_, Heos>) -> Result<Levels, String> {
    let ctl = state.0.lock().await;
    let s = ctl.session.as_ref().ok_or("Select a HEOS output first.")?;
    levels(&mut ctl.client().await?, s.pid, s.id.to_string(), None).await
}
#[tauri::command]
pub(crate) async fn heos_set_level(
    state: State<'_, Heos>,
    session: String,
    group: bool,
    id: i64,
    value: u8,
) -> Result<Levels, String> {
    let ctl = state.0.lock().await;
    let s = ctl
        .session
        .as_ref()
        .filter(|s| s.id.to_string() == session)
        .ok_or("The HEOS output has changed.")?;
    levels(
        &mut ctl.client().await?,
        s.pid,
        session,
        Some((group, id, value)),
    )
    .await
}
#[tauri::command]
pub(crate) async fn heos_group(
    app: AppHandle,
    state: State<'_, Heos>,
    members: Vec<i64>,
    previous: Vec<i64>,
) -> Result<Rooms, String> {
    let mut ctl = state.0.lock().await;
    let mut client = ctl.client().await?;
    let (rooms, groups) = configure_group(&mut client, &members, previous).await?;
    let leader = members[0];
    if ctl
        .session
        .as_ref()
        .is_some_and(|s| s.pid != leader && members.contains(&s.pid))
    {
        route(&app, &mut ctl, &mut client, leader).await?;
    }
    Ok(ctl.rooms(rooms, groups))
}

async fn configure_group(
    client: &mut Client,
    members: &[i64],
    mut previous: Vec<i64>,
) -> Result<(Vec<Room>, Vec<Group>), String> {
    let (rooms, groups) = client.rooms().await?;
    let leader = *members.first().ok_or("Select a main room.")?;
    previous.sort_unstable();
    if membership(&groups, leader) != previous {
        return Err("This group changed. Refresh the rooms and try again.".into());
    }
    let selected: HashSet<_> = members.iter().copied().collect();
    if selected.len() != members.len()
        || members.iter().any(|id| !rooms.iter().any(|r| r.pid == *id))
    {
        return Err("One of the selected rooms is no longer available.".into());
    }
    if groups
        .iter()
        .any(|g| g.leader() != leader && g.players.iter().any(|r| selected.contains(&r.pid)))
    {
        return Err("A selected room belongs to another group. Refresh the room list.".into());
    }
    let ids = members
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    client
        .command(&format!("group/set_group?pid={ids}"))
        .await?;
    let mut expected = members.to_vec();
    if expected.len() == 1 {
        expected.clear();
    }
    expected.sort_unstable();
    for _ in 0..12 {
        let (rooms, groups) = client.rooms().await?;
        if membership(&groups, leader) == expected {
            return Ok((rooms, groups));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err("HEOS has not yet confirmed the group change. Refresh to see the current rooms.".into())
}
