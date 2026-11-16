//! The voice server: a UDP socket and one thread that authenticates clients and relays speech.
//!
//! This file uses only the standard library and `crate::protocol`, so the interop tests can
//! run it without the game. The game's main thread talks to it through [`VoiceServer`]'s
//! methods; the socket thread never touches the game.

use crate::protocol::{
    self, NONCE_SIZE, Secret, VoicePacket, crypto, frame_to_client, open_at_server,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Voice packets a client may send per second before the rest are dropped.
const MAX_PACKETS_PER_SECOND: u32 = 100;
/// A client is dropped after this many keep-alive intervals without an answer.
const KEEP_ALIVE_MISSES: u32 = 10;

/// What the server operator can set.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// UDP port; 0 picks a free one.
    pub port: u16,
    /// Address to bind; empty binds every address.
    pub bind_address: String,
    /// How far normal speech carries, in blocks.
    pub distance: f64,
    /// How far whispering carries, in blocks.
    pub whisper_distance: f64,
    /// Milliseconds between keep-alives.
    pub keep_alive_ms: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            port: protocol::DEFAULT_PORT,
            bind_address: String::new(),
            distance: 48.0,
            whisper_distance: 24.0,
            keep_alive_ms: 1000,
        }
    }
}

/// Where a player is, as the game's main thread last saw them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    /// An opaque id of the player's dimension; only equal ids hear each other.
    pub level: u64,
    pub position: [f64; 3],
}

/// What the socket thread tells the game's main thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The player's voice connection is established.
    Connected(u128),
    /// The player stopped answering keep-alives; their secret is gone.
    TimedOut(u128),
}

struct Connection {
    id: u128,
    secret: Secret,
    /// The address the client proved it receives at; None until the connection check.
    address: Option<SocketAddr>,
    last_keep_alive: Instant,
    /// The player turned voice chat off: they receive nothing.
    disabled: bool,
    window_start: Instant,
    window_packets: u32,
}

#[derive(Default)]
struct Connections {
    by_player: HashMap<u128, Connection>,
    player_by_id: HashMap<u128, u128>,
}

impl Connections {
    fn remove(&mut self, player: u128) -> bool {
        match self.by_player.remove(&player) {
            Some(connection) => {
                self.player_by_id.remove(&connection.id);
                true
            }
            None => false,
        }
    }
}

struct Shared {
    socket: UdpSocket,
    settings: Settings,
    nonce_key: [u8; 32],
    running: AtomicBool,
    connections: Mutex<Connections>,
    positions: RwLock<HashMap<u128, Position>>,
    events: Mutex<Vec<Event>>,
}

/// A running voice server. Dropping it closes the socket thread.
pub struct VoiceServer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    port: u16,
}

impl VoiceServer {
    /// Binds the socket and starts the thread.
    pub fn start(settings: Settings) -> io::Result<Self> {
        let bind = settings.bind_address.trim();
        let socket = if bind.is_empty() || bind == "*" {
            // IPv4 on every interface. A dual-stack socket would behave differently per
            // operating system; an IPv6 server sets `bind_address` to its IPv6 address.
            UdpSocket::bind(("0.0.0.0", settings.port))?
        } else {
            let ip: IpAddr = bind.parse().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("bind_address {bind:?} is not an IP address"),
                )
            })?;
            UdpSocket::bind((ip, settings.port))?
        };
        socket.set_read_timeout(Some(Duration::from_millis(50)))?;
        let port = socket.local_addr()?.port();
        let mut nonce_key = [0u8; 32];
        crypto::random_bytes(&mut nonce_key);
        let shared = Arc::new(Shared {
            socket,
            settings,
            nonce_key,
            running: AtomicBool::new(true),
            connections: Mutex::default(),
            positions: RwLock::default(),
            events: Mutex::default(),
        });
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("horizon-voice-server".into())
            .spawn(move || worker.run())?;
        Ok(Self {
            shared,
            thread: Some(thread),
            port,
        })
    }

    /// The UDP port the socket is bound to.
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn settings(&self) -> &Settings {
        &self.shared.settings
    }

    /// A new connection id and secret for `player`, or None when they already have one.
    pub fn create_connection(&self, player: u128) -> Option<(u128, Secret)> {
        let mut connections = self.shared.connections.lock().unwrap();
        if connections.by_player.contains_key(&player) {
            return None;
        }
        let id = crypto::random_uuid();
        let secret = Secret::random();
        let now = Instant::now();
        connections.player_by_id.insert(id, player);
        connections.by_player.insert(
            player,
            Connection {
                id,
                secret: secret.clone(),
                address: None,
                last_keep_alive: now,
                disabled: false,
                window_start: now,
                window_packets: 0,
            },
        );
        Some((id, secret))
    }

    /// Forgets the player's connection and secret (they left the game).
    pub fn remove_player(&self, player: u128) {
        self.shared.connections.lock().unwrap().remove(player);
        self.shared.positions.write().unwrap().remove(&player);
    }

    /// Whether the player's voice connection is established.
    pub fn is_connected(&self, player: u128) -> bool {
        let connections = self.shared.connections.lock().unwrap();
        connections
            .by_player
            .get(&player)
            .is_some_and(|connection| connection.address.is_some())
    }

    /// The player turned voice chat off or on again.
    pub fn set_disabled(&self, player: u128, disabled: bool) {
        if let Some(connection) = self
            .shared
            .connections
            .lock()
            .unwrap()
            .by_player
            .get_mut(&player)
        {
            connection.disabled = disabled;
        }
    }

    /// Replaces the positions of all players; called once a server tick.
    pub fn set_positions(&self, positions: HashMap<u128, Position>) {
        *self.shared.positions.write().unwrap() = positions;
    }

    /// What happened on the socket thread since the last call.
    pub fn take_events(&self) -> Vec<Event> {
        std::mem::take(&mut *self.shared.events.lock().unwrap())
    }
}

impl Drop for VoiceServer {
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Compares without stopping at the first difference.
fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl Shared {
    fn run(&self) {
        let keep_alive = Duration::from_millis(u64::from(self.settings.keep_alive_ms.max(50)));
        let mut last_keep_alive = Instant::now();
        let mut buffer = [0u8; protocol::MAX_PACKET_SIZE + 64];
        while self.running.load(Ordering::SeqCst) {
            match self.socket.recv_from(&mut buffer) {
                Ok((len, address)) => self.on_datagram(&buffer[..len], address),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                            // Windows reports an ICMP "port unreachable" for an earlier send here.
                            | io::ErrorKind::ConnectionReset
                    ) => {}
                Err(error) => {
                    log::warn!("Voice socket read failed: {error}");
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            if last_keep_alive.elapsed() >= keep_alive {
                last_keep_alive = Instant::now();
                self.keep_alives(keep_alive * KEEP_ALIVE_MISSES);
            }
        }
    }

    /// The nonce only this server can make for this connection and address. A client that
    /// sends it back from the same address has proved it receives packets there.
    fn nonce(&self, connection: u128, address: SocketAddr) -> [u8; NONCE_SIZE] {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.nonce_key).expect("any key length");
        mac.update(&connection.to_be_bytes());
        match address.ip() {
            IpAddr::V4(ip) => mac.update(&ip.octets()),
            IpAddr::V6(ip) => mac.update(&ip.octets()),
        }
        mac.update(&u32::from(address.port()).to_be_bytes());
        let mut nonce = [0u8; NONCE_SIZE];
        nonce.copy_from_slice(&mac.finalize().into_bytes()[..NONCE_SIZE]);
        nonce
    }

    fn send(&self, address: SocketAddr, secret: &Secret, packet: &VoicePacket) {
        let _ = self.socket.send_to(&frame_to_client(secret, packet), address);
    }

    fn on_datagram(&self, datagram: &[u8], address: SocketAddr) {
        let Ok((id, sealed)) = open_at_server(datagram) else {
            return;
        };
        // Look the secret up, then decrypt outside the lock.
        let (player, secret) = {
            let connections = self.connections.lock().unwrap();
            let Some(player) = connections.player_by_id.get(&id).copied() else {
                return;
            };
            let Some(connection) = connections.by_player.get(&player) else {
                return;
            };
            (player, connection.secret.clone())
        };
        let Some(plain) = secret.decrypt(sealed) else {
            return;
        };
        let Ok(packet) = VoicePacket::decode(&plain) else {
            return;
        };
        match packet {
            VoicePacket::Authenticate => {
                let nonce = self.nonce(id, address);
                self.send(address, &secret, &VoicePacket::AuthenticateAck { nonce });
            }
            VoicePacket::ConnectionCheck { nonce } => {
                if !same_bytes(&nonce, &self.nonce(id, address)) {
                    return;
                }
                let acknowledge = {
                    let mut connections = self.connections.lock().unwrap();
                    let Some(connection) = connections.by_player.get_mut(&player) else {
                        return;
                    };
                    if connection.id != id {
                        return;
                    }
                    match connection.address {
                        // Already established: only its own address is answered.
                        Some(known) => known == address,
                        None => {
                            connection.address = Some(address);
                            connection.last_keep_alive = Instant::now();
                            self.events.lock().unwrap().push(Event::Connected(player));
                            true
                        }
                    }
                };
                if acknowledge {
                    self.send(address, &secret, &VoicePacket::ConnectionCheckAck);
                }
            }
            VoicePacket::Mic {
                data,
                sequence,
                whispering,
            } => {
                if self.admit(player, id, address) {
                    self.relay(player, data, sequence, whispering);
                }
            }
            VoicePacket::KeepAlive => {
                let mut connections = self.connections.lock().unwrap();
                if let Some(connection) = connections.by_player.get_mut(&player) {
                    if connection.id == id && connection.address == Some(address) {
                        connection.last_keep_alive = Instant::now();
                    }
                }
            }
            // Pings are only answered by clients; sound packets only travel to clients.
            _ => {}
        }
    }

    /// Whether a voice packet comes from the established address and is within the rate limit.
    fn admit(&self, player: u128, id: u128, address: SocketAddr) -> bool {
        let mut connections = self.connections.lock().unwrap();
        let Some(connection) = connections.by_player.get_mut(&player) else {
            return false;
        };
        if connection.id != id || connection.address != Some(address) {
            return false;
        }
        if connection.window_start.elapsed() >= Duration::from_secs(1) {
            connection.window_start = Instant::now();
            connection.window_packets = 0;
        }
        connection.window_packets += 1;
        connection.window_packets <= MAX_PACKETS_PER_SECOND
    }

    /// Sends one frame of `sender`'s speech to every connected player in range.
    fn relay(&self, sender: u128, data: Vec<u8>, sequence: i64, whispering: bool) {
        let distance = if whispering {
            self.settings.whisper_distance
        } else {
            self.settings.distance
        };
        // A little further than the hearing distance, so a listener walking in hears no gap.
        let range = (self.settings.distance + 1.0).max(distance);
        let in_range: Vec<u128> = {
            let positions = self.positions.read().unwrap();
            let Some(from) = positions.get(&sender) else {
                return;
            };
            positions
                .iter()
                .filter(|(player, at)| {
                    **player != sender
                        && at.level == from.level
                        && (0..3)
                            .map(|axis| (at.position[axis] - from.position[axis]).powi(2))
                            .sum::<f64>()
                            <= range * range
                })
                .map(|(player, _)| *player)
                .collect()
        };
        if in_range.is_empty() {
            return;
        }
        let targets: Vec<(SocketAddr, Secret)> = {
            let connections = self.connections.lock().unwrap();
            in_range
                .iter()
                .filter_map(|player| connections.by_player.get(player))
                .filter(|connection| !connection.disabled)
                .filter_map(|connection| Some((connection.address?, connection.secret.clone())))
                .collect()
        };
        let packet = VoicePacket::PlayerSound {
            channel: sender,
            sender,
            data,
            sequence,
            distance: distance as f32,
            whispering,
            category: None,
        };
        for (address, secret) in targets {
            self.send(address, &secret, &packet);
        }
    }

    fn keep_alives(&self, timeout: Duration) {
        let mut timed_out = Vec::new();
        let targets: Vec<(SocketAddr, Secret)> = {
            let mut connections = self.connections.lock().unwrap();
            for (player, connection) in &connections.by_player {
                if connection.address.is_some() && connection.last_keep_alive.elapsed() >= timeout {
                    timed_out.push(*player);
                }
            }
            for player in &timed_out {
                connections.remove(*player);
            }
            connections
                .by_player
                .values()
                .filter_map(|connection| Some((connection.address?, connection.secret.clone())))
                .collect()
        };
        if !timed_out.is_empty() {
            let mut events = self.events.lock().unwrap();
            events.extend(timed_out.into_iter().map(Event::TimedOut));
        }
        for (address, secret) in targets {
            self.send(address, &secret, &VoicePacket::KeepAlive);
        }
    }
}
