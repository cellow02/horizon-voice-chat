//! The voice connection: a UDP socket and one thread that authenticates and receives speech.
//!
//! This file uses only the standard library and `crate::protocol`, so the interop tests can
//! run it without the game.

use crate::protocol::{Secret, VoicePacket, frame_to_server, open_at_client};
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// A client gives up after this many keep-alive intervals of silence from the server.
const KEEP_ALIVE_MISSES: u32 = 10;
/// How often the handshake packets are repeated until the server answers.
const HANDSHAKE_INTERVAL: Duration = Duration::from_secs(1);

/// Where the connection stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    /// Resolving the address or shaking hands.
    Connecting = 0,
    /// Speech flows.
    Connected = 1,
    /// The address did not resolve or the socket failed.
    Failed = 2,
    /// The server stopped sending keep-alives.
    TimedOut = 3,
}

/// Where a received frame is heard.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Place {
    /// At the sending player, up to `distance` blocks away.
    Player { distance: f32 },
    /// At a fixed position, up to `distance` blocks away.
    Location { at: [f64; 3], distance: f32 },
    /// Everywhere, at full volume.
    Everywhere,
}

/// One Opus frame someone said. Empty `data` ends their talk burst.
#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    /// The audio stream; a player's own voice uses the player's UUID.
    pub channel: u128,
    pub sender: u128,
    pub data: Vec<u8>,
    pub sequence: i64,
    pub place: Place,
    pub whispering: bool,
}

/// What a connection needs; all of it comes from the server's secret message.
#[derive(Debug, Clone)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub connection: u128,
    pub secret: Secret,
    pub keep_alive_ms: u32,
}

struct Shared {
    target: Target,
    socket: OnceLock<UdpSocket>,
    running: AtomicBool,
    status: AtomicU8,
}

/// A voice connection. Dropping it closes the socket thread.
pub struct VoiceClient {
    shared: Arc<Shared>,
    sounds: Receiver<Sound>,
    thread: Option<JoinHandle<()>>,
}

impl VoiceClient {
    /// Starts connecting in the background; never blocks on the network.
    pub fn connect(target: Target) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            target,
            socket: OnceLock::new(),
            running: AtomicBool::new(true),
            status: AtomicU8::new(Status::Connecting as u8),
        });
        let (sender, sounds) = channel();
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("horizon-voice-client".into())
            .spawn(move || {
                if let Err(error) = worker.run(&sender) {
                    if worker.running.load(Ordering::SeqCst) {
                        log::warn!("Voice connection failed: {error}");
                        worker.set_status(Status::Failed);
                    }
                }
            })?;
        Ok(Self {
            shared,
            sounds,
            thread: Some(thread),
        })
    }

    pub fn status(&self) -> Status {
        match self.shared.status.load(Ordering::SeqCst) {
            0 => Status::Connecting,
            1 => Status::Connected,
            2 => Status::Failed,
            _ => Status::TimedOut,
        }
    }

    /// Sends one Opus frame of the microphone; dropped while not connected.
    pub fn send_mic(&self, data: &[u8], sequence: i64, whispering: bool) {
        if self.status() != Status::Connected {
            return;
        }
        self.shared.send(&VoicePacket::Mic {
            data: data.to_vec(),
            sequence,
            whispering,
        });
    }

    /// The frames received since the last call, oldest first.
    pub fn sounds(&self) -> impl Iterator<Item = Sound> + '_ {
        self.sounds.try_iter()
    }
}

impl Drop for VoiceClient {
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Shared {
    fn set_status(&self, status: Status) {
        self.status.store(status as u8, Ordering::SeqCst);
    }

    fn send(&self, packet: &VoicePacket) {
        if let Some(socket) = self.socket.get() {
            let datagram = frame_to_server(self.target.connection, &self.target.secret, packet);
            let _ = socket.send(&datagram);
        }
    }

    fn run(&self, sounds: &Sender<Sound>) -> io::Result<()> {
        let address: SocketAddr = (self.target.host.as_str(), self.target.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the voice host has no address"))?;
        let socket = UdpSocket::bind(if address.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })?;
        // Only the voice server's datagrams are received.
        socket.connect(address)?;
        socket.set_read_timeout(Some(Duration::from_millis(50)))?;
        let socket = self.socket.get_or_init(|| socket);

        let timeout = Duration::from_millis(u64::from(self.target.keep_alive_ms.max(50)))
            * KEEP_ALIVE_MISSES;
        let mut nonce = None;
        let mut connected = false;
        let mut last_handshake: Option<Instant> = None;
        let mut last_keep_alive = Instant::now();
        let mut buffer = [0u8; crate::protocol::MAX_PACKET_SIZE + 64];
        while self.running.load(Ordering::SeqCst) {
            if !connected && last_handshake.is_none_or(|at| at.elapsed() >= HANDSHAKE_INTERVAL) {
                last_handshake = Some(Instant::now());
                match nonce {
                    None => self.send(&VoicePacket::Authenticate),
                    Some(nonce) => self.send(&VoicePacket::ConnectionCheck { nonce }),
                }
            }
            if connected && last_keep_alive.elapsed() > timeout {
                self.set_status(Status::TimedOut);
                return Ok(());
            }
            let len = match socket.recv(&mut buffer) {
                Ok(len) => len,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                            // The server is not listening (yet); the handshake retries.
                            | io::ErrorKind::ConnectionRefused
                            | io::ErrorKind::ConnectionReset
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            };
            let Some(packet) = open_at_client(&self.target.secret, &buffer[..len]) else {
                continue;
            };
            let sound = match packet {
                VoicePacket::AuthenticateAck { nonce: received } => {
                    if nonce.is_none() {
                        nonce = Some(received);
                        // Answer at once instead of waiting for the next handshake interval.
                        last_handshake = None;
                    }
                    continue;
                }
                VoicePacket::ConnectionCheckAck => {
                    if nonce.is_some() && !connected {
                        connected = true;
                        last_keep_alive = Instant::now();
                        self.set_status(Status::Connected);
                    }
                    continue;
                }
                VoicePacket::KeepAlive => {
                    last_keep_alive = Instant::now();
                    self.send(&VoicePacket::KeepAlive);
                    continue;
                }
                VoicePacket::Ping { id, timestamp } => {
                    self.send(&VoicePacket::Ping { id, timestamp });
                    continue;
                }
                VoicePacket::PlayerSound {
                    channel,
                    sender,
                    data,
                    sequence,
                    distance,
                    whispering,
                    ..
                } => Sound {
                    channel,
                    sender,
                    data,
                    sequence,
                    place: Place::Player { distance },
                    whispering,
                },
                VoicePacket::GroupSound {
                    channel,
                    sender,
                    data,
                    sequence,
                    ..
                } => Sound {
                    channel,
                    sender,
                    data,
                    sequence,
                    place: Place::Everywhere,
                    whispering: false,
                },
                VoicePacket::LocationSound {
                    channel,
                    sender,
                    location,
                    data,
                    sequence,
                    distance,
                    ..
                } => Sound {
                    channel,
                    sender,
                    data,
                    sequence,
                    place: Place::Location {
                        at: location,
                        distance,
                    },
                    whispering: false,
                },
                // Packets only a server receives.
                _ => continue,
            };
            if sounds.send(sound).is_err() {
                return Ok(());
            }
        }
        Ok(())
    }
}
