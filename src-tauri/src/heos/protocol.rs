use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::{timeout, timeout_at, Instant};

const TARGET: &str = "urn:schemas-denon-com:device:ACT-Denon:1";
async fn multicast(ip: Ipv4Addr) -> Result<Vec<Ipv4Addr>, String> {
    let socket = std::net::UdpSocket::bind((ip, 0)).map_err(crate::err)?;
    socket2::SockRef::from(&socket)
        .set_multicast_if_v4(&ip)
        .map_err(crate::err)?;
    socket.set_nonblocking(true).map_err(crate::err)?;
    let socket = UdpSocket::from_std(socket).map_err(crate::err)?;
    let query = format!("M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {TARGET}\r\n\r\n");
    socket
        .send_to(query.as_bytes(), "239.255.255.250:1900")
        .await
        .map_err(crate::err)?;
    let deadline = Instant::now() + Duration::from_millis(2300);
    let mut hosts = Vec::new();
    let mut buffer = [0; 8192];
    while let Ok(reply) = timeout_at(deadline, socket.recv_from(&mut buffer)).await {
        let (size, from) = reply.map_err(crate::err)?;
        let packet = String::from_utf8_lossy(&buffer[..size]);
        if packet.lines().any(|line| {
            line.split_once(':').is_some_and(|(k, v)| {
                k.eq_ignore_ascii_case("ST") && v.trim().eq_ignore_ascii_case(TARGET)
            })
        }) {
            if let IpAddr::V4(ip) = from.ip() {
                if !hosts.contains(&ip) {
                    hosts.push(ip);
                }
            }
        }
    }
    Ok(hosts)
}

pub(super) async fn discover() -> Result<Option<(Ipv4Addr, Vec<Room>, Vec<Group>)>, String> {
    use std::collections::BTreeSet;
    use tokio::task::JoinSet;
    let adapters = if_addrs::get_if_addrs().map_err(crate::err)?;
    let mut searches = JoinSet::new();
    let mut candidates = BTreeSet::new();
    for iface in adapters {
        let if_addrs::IfAddr::V4(addr) = iface.addr else {
            continue;
        };
        if addr.ip.is_loopback() || addr.ip.is_unspecified() {
            continue;
        }
        searches.spawn(multicast(addr.ip));
        if addr.ip.is_private() {
            // Heos_O fallback for networks that filter multicast. Stay on-link and
            // within the local /24 even on interfaces advertising a larger prefix.
            let mask = u32::from(addr.netmask) | 0xffffff00;
            let net = u32::from(addr.ip) & mask;
            candidates.extend(
                (net + 1..(net | !mask))
                    .map(Ipv4Addr::from)
                    .filter(|ip| *ip != addr.ip),
            );
        }
    }
    let mut advertised = BTreeSet::new();
    while let Some(result) = searches.join_next().await {
        if let Ok(Ok(hosts)) = result {
            advertised.extend(hosts);
        }
    }
    if let Some(found) = first_system(advertised.into_iter()).await {
        return Ok(Some(found));
    }
    Ok(first_system(candidates.into_iter()).await)
}

async fn first_system(
    mut hosts: impl Iterator<Item = Ipv4Addr>,
) -> Option<(Ipv4Addr, Vec<Room>, Vec<Group>)> {
    let mut jobs = tokio::task::JoinSet::new();
    loop {
        while jobs.len() < 32 {
            let Some(ip) = hosts.next() else { break };
            jobs.spawn(async move {
                let mut client = timeout(
                    Duration::from_millis(650),
                    Client::connect(SocketAddr::new(ip.into(), 1255)),
                )
                .await
                .ok()?
                .ok()?;
                let (players, groups) = client.rooms().await.ok()?;
                (!players.is_empty()).then_some((ip, players, groups))
            });
        }
        if let Ok(Some(found)) = jobs.join_next().await? {
            return Some(found);
        }
    }
}

// Some firmware returns IDs as JSON strings, including negative player IDs.
fn id<'de, D: Deserializer<'de>>(de: D) -> Result<i64, D::Error> {
    let v = Value::deserialize(de)?;
    v.as_i64()
        .or_else(|| v.as_str()?.parse().ok())
        .ok_or_else(|| serde::de::Error::custom("Invalid HEOS ID"))
}
#[derive(Serialize, Deserialize)]
pub(super) struct Room {
    #[serde(deserialize_with = "id")]
    pub pid: i64,
    pub name: String,
    #[serde(default)]
    pub model: String,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Member {
    #[serde(deserialize_with = "id")]
    pub pid: i64,
    pub role: String,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Group {
    #[serde(deserialize_with = "id")]
    pub gid: i64,
    pub name: String,
    pub players: Vec<Member>,
}
impl Group {
    pub fn leader(&self) -> i64 {
        self.players
            .iter()
            .find(|p| p.role.eq_ignore_ascii_case("leader"))
            .map_or(self.gid, |p| p.pid)
    }
}

pub(super) struct Client(BufReader<TcpStream>);
impl Client {
    pub async fn connect(host: SocketAddr) -> Result<Self, String> {
        let socket = timeout(Duration::from_secs(3), TcpStream::connect(host))
            .await
            .map_err(|_| format!("HEOS did not respond at {host}"))?
            .map_err(crate::err)?;
        Ok(Self(BufReader::new(socket)))
    }
    pub fn local_ip(&self) -> Result<IpAddr, String> {
        Ok(self.0.get_ref().local_addr().map_err(crate::err)?.ip())
    }
    pub async fn command(&mut self, command: &str) -> Result<Value, String> {
        Ok(self.reply(command).await?["payload"].take())
    }
    pub(super) async fn reply(&mut self, command: &str) -> Result<Value, String> {
        let name = command.split('?').next().unwrap();
        timeout(Duration::from_secs(8), async {
            self.0
                .get_mut()
                .write_all(format!("heos://{command}\r\n").as_bytes())
                .await
                .map_err(crate::err)?;
            loop {
                let mut line = String::new();
                let n = (&mut self.0)
                    .take(256 * 1024)
                    .read_line(&mut line)
                    .await
                    .map_err(crate::err)?;
                if n == 0 || !line.ends_with('\n') {
                    return Err("HEOS closed the connection or sent an incomplete reply.".into());
                }
                if line.trim().is_empty() {
                    continue;
                }
                let response: Value = serde_json::from_str(&line).map_err(crate::err)?;
                let header = &response["heos"];
                let echoed = header["command"].as_str().unwrap_or("").trim();
                // Older firmware can label group replies with the player prefix.
                if echoed != name
                    && !(name.starts_with("group/")
                        && echoed.strip_prefix("player/") == name.strip_prefix("group/"))
                {
                    continue;
                }
                let message = header["message"].as_str().unwrap_or("");
                if header["result"] != "success" {
                    return Err(format!("HEOS: {message}"));
                }
                if message
                    .to_ascii_lowercase()
                    .contains("command under process")
                {
                    continue;
                }
                return Ok(response);
            }
        })
        .await
        .map_err(|_| format!("HEOS timed out on {name}."))?
    }
    pub async fn rooms(&mut self) -> Result<(Vec<Room>, Vec<Group>), String> {
        let rooms = serde_json::from_value(self.command("player/get_players").await?)
            .map_err(crate::err)?;
        let groups =
            serde_json::from_value(self.command("group/get_groups").await?).map_err(crate::err)?;
        Ok((rooms, groups))
    }
}
