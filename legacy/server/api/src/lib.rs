//! Horizon Voice Chat, server side, for Horizon's older mod API (`horizon_client::mod_api`).
//!
//! The same mod as `server/` in this repository, on the hooks of Horizon builds before the mod
//! SDK: it hands out a secret to every client that asks on the `voicechat:request_secret`
//! plugin channel, tells clients who is in voice chat, and runs the UDP voice server (`udp.rs`)
//! that relays speech to players in range. `udp.rs`, `config.rs` and `protocol/` are copies
//! (`tools/sync-protocol.sh`); only this file differs.
//!
//! On this API the server hooks see the players who connect to the server, not the player of
//! the window that hosts a LAN world: that host cannot take part in voice chat.

mod config;
#[allow(dead_code)]
mod protocol;
#[allow(dead_code)]
mod udp;

use horizon_client::mod_api::{
    CustomPayloadFromClientEvent, Hooks, Host, PlayerLoggedInEvent, PlayerLoggedOutEvent,
    ServerAboutToStartEvent, ServerStoppingEvent, ServerTickEvent, ServerTickPhase, Text,
};
use protocol::{COMPATIBILITY_VERSION, LEGACY_VERSION, Codec, PlayerState, SecretMessage, channel};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;
use udp::{Event, Position, VoiceServer};

/// The config file in the game's or the server's `config/` folder.
const CONFIG_FILE: &str = "horizon_voice_server-server.toml";

#[derive(Default)]
struct State {
    config: config::Config,
    server: Option<VoiceServer>,
    dedicated: bool,
    /// Every player in the game, in voice chat or not.
    states: HashMap<u128, PlayerState>,
    /// Players whose client asked for a secret with the right protocol version. Only they
    /// are sent voice chat plugin messages.
    compatible: HashSet<u128>,
    /// Those of them who speak protocol 20 and get the older message formats.
    v20: HashSet<u128>,
}

#[derive(Default)]
struct Mod(Mutex<State>);

fn send(host: &mut dyn Host, player: u128, channel: &str, body: &[u8]) {
    if let Err(error) = host.send_custom_payload_to_player(player, channel, body) {
        log::debug!("Could not send {channel} to a player: {error}");
    }
}

fn level_id(dimension: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    dimension.hash(&mut hasher);
    hasher.finish()
}

impl State {
    /// Sends `body` on `channel` to every player with a compatible voice chat client.
    fn broadcast(&self, host: &mut dyn Host, channel: &str, body: &[u8]) {
        for uuid in &self.compatible {
            send(host, *uuid, channel, body);
        }
    }

    fn broadcast_state(&self, host: &mut dyn Host, uuid: u128) {
        if let Some(state) = self.states.get(&uuid) {
            for player in &self.compatible {
                let body = if self.v20.contains(player) {
                    state.encode_v20()
                } else {
                    state.encode()
                };
                send(host, *player, channel::STATE, &body);
            }
        }
    }

    /// Gives `uuid` a fresh secret and sends it; nothing when they already hold one.
    fn send_secret(&self, host: &mut dyn Host, uuid: u128) {
        let Some(server) = &self.server else {
            return;
        };
        let Some((connection, secret)) = server.create_connection(uuid) else {
            log::debug!("A player asked for a second voice secret; ignored");
            return;
        };
        let message = SecretMessage {
            secret,
            connection,
            port: i32::from(server.port()),
            player: uuid,
            codec: Codec::Voip,
            mtu_size: self.config.mtu_size,
            distance: self.config.udp.distance,
            keep_alive: self.config.udp.keep_alive_ms as i32,
            groups_enabled: false,
            // Only a dedicated server is reached under a configured address.
            voice_host: if self.dedicated {
                self.config.voice_host.clone()
            } else {
                String::new()
            },
            allow_recording: self.config.allow_recording,
        };
        let body = if self.v20.contains(&uuid) {
            message.encode_v20()
        } else {
            message.encode()
        };
        send(host, uuid, channel::SECRET, &body);
    }

    /// Reads the config file, writing the default one when there is none.
    fn load_config(&mut self, host: &dyn Host) {
        let text = host.config_dir().and_then(|dir| {
            let path = dir.join(CONFIG_FILE);
            if !path.exists() {
                std::fs::write(&path, config::DEFAULT).map_err(|e| e.to_string())?;
            }
            std::fs::read_to_string(&path).map_err(|e| e.to_string())
        });
        let text = text.unwrap_or_else(|error| {
            log::warn!("Voice chat uses its default settings: {CONFIG_FILE}: {error}");
            config::DEFAULT.to_owned()
        });
        let (config, warnings) = config::parse(&text);
        for warning in warnings {
            log::warn!("{CONFIG_FILE}: {warning}");
        }
        self.config = config;
    }

    fn start(&mut self, dedicated: bool) {
        self.dedicated = dedicated;
        self.server = None;
        let mut settings = self.config.udp.clone();
        if !dedicated {
            // A single-player world or LAN host is reached on whatever address the host has.
            settings.bind_address.clear();
        }
        let server = VoiceServer::start(settings.clone()).or_else(|error| {
            if dedicated {
                return Err(error);
            }
            // Another world or game already uses the port on this computer.
            settings.port = 0;
            VoiceServer::start(settings)
        });
        match server {
            Ok(server) => {
                log::info!("Voice chat server started on UDP port {}", server.port());
                self.server = Some(server);
            }
            Err(error) => log::error!(
                "Voice chat is off: could not open UDP port {}: {error}",
                self.config.udp.port
            ),
        }
    }

    /// `voicechat:request_secret`: a client with voice chat joined and says which protocol it
    /// speaks.
    fn on_request_secret(&mut self, host: &mut dyn Host, uuid: u128, body: &[u8]) {
        let Ok(version) = protocol::decode_request_secret(body) else {
            return;
        };
        if version != COMPATIBILITY_VERSION && version != LEGACY_VERSION {
            log::warn!(
                "A client has an incompatible voice chat version (server {COMPATIBILITY_VERSION}, client {version})"
            );
            host.tell(
                uuid,
                Text::literal(format!(
                    "Your voice chat version is not compatible with this server (protocol {version}, the server speaks {COMPATIBILITY_VERSION})."
                )),
            );
            return;
        }
        if self.server.is_none() {
            return;
        }
        if version == LEGACY_VERSION {
            self.v20.insert(uuid);
        }
        if self.compatible.insert(uuid) {
            // The newcomer learns everyone's state before its own connection starts.
            let states: Vec<PlayerState> = self.states.values().cloned().collect();
            let body = if version == LEGACY_VERSION {
                protocol::encode_states_v20(&states)
            } else {
                protocol::encode_states(&states)
            };
            send(host, uuid, channel::STATES, &body);
        }
        self.send_secret(host, uuid);
    }

    /// `voicechat:client_state`: a client muted its microphone or turned voice chat off.
    fn on_client_state(&mut self, host: &mut dyn Host, uuid: u128, body: &[u8]) {
        let Ok((muted, disabled)) = protocol::decode_client_state(body) else {
            return;
        };
        let Some(player_state) = self.states.get_mut(&uuid) else {
            return;
        };
        if player_state.muted == muted && player_state.disabled == disabled {
            return;
        }
        player_state.muted = muted;
        player_state.disabled = disabled;
        if let Some(server) = &self.server {
            server.set_disabled(uuid, disabled);
        }
        self.broadcast_state(host, uuid);
    }

    fn tick(&mut self, host: &mut dyn Host) {
        let Some(server) = &self.server else {
            return;
        };
        // The socket thread relays speech by where players were at the end of the last tick.
        let mut positions = HashMap::new();
        for player in host.server_players() {
            let [x, y, z] = player.position;
            positions.insert(
                player.uuid,
                Position {
                    level: level_id(&player.dimension),
                    position: [x, y + f64::from(player.eye_height), z],
                },
            );
        }
        server.set_positions(positions);
        for event in server.take_events() {
            match event {
                Event::Connected(uuid) => {
                    if let Some(player_state) = self.states.get_mut(&uuid) {
                        player_state.disconnected = false;
                        self.broadcast_state(host, uuid);
                    }
                }
                Event::TimedOut(uuid) => {
                    if let Some(player_state) = self.states.get_mut(&uuid) {
                        player_state.disconnected = true;
                        self.broadcast_state(host, uuid);
                        // Still in the game: let the client connect again with a new secret.
                        if self.compatible.contains(&uuid) {
                            self.send_secret(host, uuid);
                        }
                    }
                }
            }
        }
    }
}

impl Hooks for Mod {
    fn server_about_to_start(&self, host: &mut dyn Host, event: &mut ServerAboutToStartEvent) {
        let mut state = self.0.lock().unwrap();
        state.load_config(host);
        state.start(event.dedicated);
    }

    fn server_stopping(&self, _: &mut dyn Host, _: &mut ServerStoppingEvent) {
        let mut state = self.0.lock().unwrap();
        state.server = None;
        state.states.clear();
        state.compatible.clear();
        state.v20.clear();
    }

    fn custom_payload_from_client(
        &self,
        host: &mut dyn Host,
        event: &mut CustomPayloadFromClientEvent<'_>,
    ) {
        let mut state = self.0.lock().unwrap();
        match event.channel {
            channel::REQUEST_SECRET => state.on_request_secret(host, event.player, event.data),
            channel::CLIENT_STATE => state.on_client_state(host, event.player, event.data),
            _ => {}
        }
    }

    fn player_logged_in(&self, host: &mut dyn Host, event: &mut PlayerLoggedInEvent<'_>) {
        let mut state = self.0.lock().unwrap();
        state
            .states
            .insert(event.player, PlayerState::disconnected(event.player, event.name));
        state.broadcast_state(host, event.player);
    }

    fn player_logged_out(&self, host: &mut dyn Host, event: &mut PlayerLoggedOutEvent<'_>) {
        let mut state = self.0.lock().unwrap();
        if let Some(server) = &state.server {
            server.remove_player(event.player);
        }
        state.compatible.remove(&event.player);
        state.v20.remove(&event.player);
        if state.states.remove(&event.player).is_some() {
            state.broadcast(
                host,
                channel::REMOVE_STATE,
                &protocol::encode_remove_state(event.player),
            );
        }
    }

    fn server_tick(&self, host: &mut dyn Host, event: &mut ServerTickEvent<'_>) {
        if matches!(event.phase, ServerTickPhase::ServerTickEnd { .. }) {
            self.0.lock().unwrap().tick(host);
        }
    }
}

/// What the launcher's generated game calls to install the mod.
pub fn hooks() -> Box<dyn Hooks> {
    Box::new(Mod::default())
}
