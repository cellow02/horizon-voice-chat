//! Horizon Voice Chat, server side.
//!
//! Hands out a secret to every client that asks on the `voicechat:request_secret` plugin
//! channel, tells clients who is in voice chat, and runs the UDP voice server (`udp.rs`) that
//! relays speech to players in range. The plugin channels are raw channels, so vanilla clients
//! running the Simple Voice Chat mod and Horizon clients running Horizon Voice Chat both work.

mod config;
mod groups;
#[allow(dead_code)]
mod protocol;
#[allow(dead_code)]
mod udp;

use mod_api::{
    Config as ConfigFileSpec, ConfigKind, ConfigLoadingEvent, ConfigReloadingEvent, Hooks,
    HostSide, IPayloadContext, IPayloadHandler, PayloadBytes, PayloadHandleError, PayloadId,
    PlayerLoggedInEvent, PlayerLoggedOutEvent, PlayerRef, RegisterConfigsEvent, Registrar,
    RegistrationContext, ServerHost, ServerStartedEvent, ServerStoppingEvent, ServerTickEvent,
    ServerTickPhase, Text,
};
use groups::{Effect, Groups};
use protocol::{COMPATIBILITY_VERSION, LEGACY_VERSION, Codec, PlayerState, SecretMessage, channel};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex};
use udp::{Event, Position, VoiceServer};

const MOD_ID: &str = "horizon_voice_server";

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
    /// Those of them who speak protocol 20 and get the older message formats. They get no
    /// groups.
    v20: HashSet<u128>,
    groups: Groups,
}

#[derive(Default)]
struct Inner(Mutex<State>);

fn failed(error: impl std::fmt::Debug) -> PayloadHandleError {
    PayloadHandleError::Failed(format!("{error:?}"))
}

fn send(host: &mut dyn ServerHost, player: PlayerRef, channel: &str, body: Vec<u8>) {
    if let Err(error) = player.send_payload(host, &PayloadId(channel.into()), PayloadBytes(body)) {
        log::debug!("Could not send {channel} to a player: {error:?}");
    }
}

impl State {
    /// Sends `body` on `channel` to every player with a compatible voice chat client.
    fn broadcast(&self, host: &mut dyn ServerHost, channel: &str, body: &[u8]) {
        for uuid in &self.compatible {
            if let Some(player) = host.player(*uuid) {
                send(host, player, channel, body.to_vec());
            }
        }
    }

    fn broadcast_state(&self, host: &mut dyn ServerHost, uuid: u128) {
        if let Some(state) = self.states.get(&uuid) {
            for uuid in &self.compatible {
                let body = if self.v20.contains(uuid) {
                    state.encode_v20()
                } else {
                    state.encode()
                };
                if let Some(player) = host.player(*uuid) {
                    send(host, player, channel::STATE, body);
                }
            }
        }
    }

    /// Gives `uuid` a fresh secret and sends it; nothing when they already hold one.
    fn send_secret(&self, host: &mut dyn ServerHost, uuid: u128) {
        let (Some(server), Some(player)) = (&self.server, host.player(uuid)) else {
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
            groups_enabled: self.config.groups && !self.v20.contains(&uuid),
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
        send(host, player, channel::SECRET, body);
    }

    /// Sends what a group change means to the clients, and tells the voice server.
    fn apply(&self, host: &mut dyn ServerHost, effects: Vec<Effect>) {
        if effects.is_empty() {
            return;
        }
        for effect in effects {
            match effect {
                Effect::Joined {
                    player,
                    group,
                    wrong_password,
                } => {
                    if let Some(player) = host.player(player) {
                        let body = protocol::encode_joined_group(group, wrong_password);
                        send(host, player, channel::JOINED_GROUP, body);
                    }
                }
                Effect::Added(group) => self.broadcast_v21(host, channel::ADD_GROUP, &group.encode()),
                Effect::Removed(id) => {
                    self.broadcast_v21(host, channel::REMOVE_GROUP, &protocol::encode_remove_group(id))
                }
                Effect::State(player) => self.broadcast_state(host, player),
            }
        }
        if let Some(server) = &self.server {
            server.set_groups(self.groups.membership(&self.states));
        }
    }

    /// Sends `body` on `channel` to every compatible client that speaks protocol 21.
    fn broadcast_v21(&self, host: &mut dyn ServerHost, channel: &str, body: &[u8]) {
        for uuid in self.compatible.difference(&self.v20) {
            if let Some(player) = host.player(*uuid) {
                send(host, player, channel, body.to_vec());
            }
        }
    }

    /// A group message from a client: `set_group`, `create_group` or `leave_group`.
    fn on_group_message(&mut self, host: &mut dyn ServerHost, uuid: u128, channel: &str, body: &[u8]) {
        if !self.config.groups || !self.compatible.contains(&uuid) || self.v20.contains(&uuid) {
            return;
        }
        let effects = match channel {
            channel::SET_GROUP => match protocol::decode_set_group(body) {
                Ok((group, password)) => {
                    self.groups
                        .join(&mut self.states, uuid, group, password.as_deref())
                }
                Err(_) => return,
            },
            channel::CREATE_GROUP => match protocol::decode_create_group(body) {
                Ok((name, password, kind)) => {
                    self.groups
                        .create(&mut self.states, uuid, &name, password.as_deref(), kind)
                }
                Err(_) => return,
            },
            _ => self.groups.leave(&mut self.states, uuid),
        };
        self.apply(host, effects);
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

    fn load_config(&mut self, text: &str) {
        let (config, warnings) = config::parse(text);
        for warning in warnings {
            log::warn!("horizon_voice_server config: {warning}");
        }
        if self.server.is_some() && config.udp != self.config.udp {
            log::info!("Voice chat port, distances and keep-alive apply after a server restart");
        }
        self.config = config;
    }
}

/// `voicechat:request_secret`: a client with voice chat joined and says which protocol it speaks.
struct RequestSecret(Arc<Inner>);

impl IPayloadHandler for RequestSecret {
    fn handle(
        &self,
        bytes: PayloadBytes,
        context: &mut dyn IPayloadContext,
    ) -> Result<(), PayloadHandleError> {
        let version = protocol::decode_request_secret(&bytes.0).map_err(failed)?;
        let player = context.player().map_err(failed)?;
        let host = context.server_host().map_err(failed)?;
        let uuid = player.uuid();
        if version != COMPATIBILITY_VERSION && version != LEGACY_VERSION {
            log::warn!(
                "A client has an incompatible voice chat version (server {COMPATIBILITY_VERSION}, client {version})"
            );
            let _ = player.tell(
                host,
                Text::literal(format!(
                    "Your voice chat version is not compatible with this server (protocol {version}, the server speaks {COMPATIBILITY_VERSION})."
                )),
            );
            return Ok(());
        }
        let mut state = self.0.0.lock().unwrap();
        if state.server.is_none() {
            return Ok(());
        }
        if version == LEGACY_VERSION {
            state.v20.insert(uuid);
        }
        if state.compatible.insert(uuid) {
            // The newcomer learns everyone's state before its own connection starts.
            let states: Vec<PlayerState> = state.states.values().cloned().collect();
            let body = if version == LEGACY_VERSION {
                protocol::encode_states_v20(&states)
            } else {
                protocol::encode_states(&states)
            };
            send(host, player, channel::STATES, body);
            if version != LEGACY_VERSION && state.config.groups {
                for group in state.groups.all() {
                    send(host, player, channel::ADD_GROUP, group.encode());
                }
            }
        }
        state.send_secret(host, uuid);
        Ok(())
    }
}

/// `voicechat:client_state`: a client muted its microphone or turned voice chat off.
struct ClientState(Arc<Inner>);

impl IPayloadHandler for ClientState {
    fn handle(
        &self,
        bytes: PayloadBytes,
        context: &mut dyn IPayloadContext,
    ) -> Result<(), PayloadHandleError> {
        let (muted, disabled) = protocol::decode_client_state(&bytes.0).map_err(failed)?;
        let uuid = context.player().map_err(failed)?.uuid();
        let host = context.server_host().map_err(failed)?;
        let mut state = self.0.0.lock().unwrap();
        let Some(player_state) = state.states.get_mut(&uuid) else {
            return Ok(());
        };
        if player_state.muted == muted && player_state.disabled == disabled {
            return Ok(());
        }
        player_state.muted = muted;
        player_state.disabled = disabled;
        if let Some(server) = &state.server {
            server.set_disabled(uuid, disabled);
        }
        state.broadcast_state(host, uuid);
        Ok(())
    }
}

/// `voicechat:set_group`, `voicechat:create_group` and `voicechat:leave_group`.
struct GroupMessage(Arc<Inner>, &'static str);

impl IPayloadHandler for GroupMessage {
    fn handle(
        &self,
        bytes: PayloadBytes,
        context: &mut dyn IPayloadContext,
    ) -> Result<(), PayloadHandleError> {
        let uuid = context.player().map_err(failed)?.uuid();
        let host = context.server_host().map_err(failed)?;
        let mut state = self.0.0.lock().unwrap();
        state.on_group_message(host, uuid, self.1, &bytes.0);
        Ok(())
    }
}

#[derive(Default)]
struct Mod(Arc<Inner>);

fn level_id(dimension: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    dimension.hash(&mut hasher);
    hasher.finish()
}

#[mod_api::horizon_mod]
impl Hooks for Mod {
    fn register(&self, r: &mut dyn Registrar, _: &RegistrationContext) {
        let mut payloads = r.payload_handlers();
        let mut raw = payloads.registrar("1");
        raw.play_raw(
            PayloadId(channel::REQUEST_SECRET.into()),
            Some(Arc::new(RequestSecret(self.0.clone()))),
        )
        .play_raw(
            PayloadId(channel::CLIENT_STATE.into()),
            Some(Arc::new(ClientState(self.0.clone()))),
        );
        for group_channel in [channel::SET_GROUP, channel::CREATE_GROUP, channel::LEAVE_GROUP] {
            raw.play_raw(
                PayloadId(group_channel.into()),
                Some(Arc::new(GroupMessage(self.0.clone(), group_channel))),
            );
        }
        for send_only in [
            channel::SECRET,
            channel::STATE,
            channel::STATES,
            channel::REMOVE_STATE,
            channel::ADD_GROUP,
            channel::REMOVE_GROUP,
            channel::JOINED_GROUP,
        ] {
            raw.play_raw(PayloadId(send_only.into()), None);
        }
    }

    fn register_configs(&self, _: HostSide, event: &mut RegisterConfigsEvent<'_>) {
        event
            .configs
            .push(ConfigFileSpec::new(MOD_ID, ConfigKind::Server, config::DEFAULT));
    }

    fn config_loading(&self, _: HostSide, event: &mut ConfigLoadingEvent<'_>) {
        if event.config.mod_id == MOD_ID {
            self.0.0.lock().unwrap().load_config(&event.config.text);
        }
    }

    fn config_reloading(&self, _: HostSide, event: &mut ConfigReloadingEvent<'_>) {
        if event.config.mod_id == MOD_ID {
            self.0.0.lock().unwrap().load_config(&event.config.text);
        }
    }

    fn server_started(&self, _: &mut dyn ServerHost, event: &mut ServerStartedEvent) {
        self.0.0.lock().unwrap().start(event.dedicated);
    }

    fn server_stopping(&self, _: &mut dyn ServerHost, _: &mut ServerStoppingEvent) {
        let mut state = self.0.0.lock().unwrap();
        state.server = None;
        state.states.clear();
        state.compatible.clear();
        state.v20.clear();
        state.groups.clear();
    }

    fn player_logged_in(&self, host: &mut dyn ServerHost, event: &mut PlayerLoggedInEvent<'_>) {
        let mut state = self.0.0.lock().unwrap();
        state
            .states
            .insert(event.player, PlayerState::disconnected(event.player, event.name));
        state.broadcast_state(host, event.player);
    }

    fn player_logged_out(&self, host: &mut dyn ServerHost, event: &mut PlayerLoggedOutEvent<'_>) {
        let mut state = self.0.0.lock().unwrap();
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
            let State { groups, states, .. } = &mut *state;
            let effects = groups.clean_up(states);
            state.apply(host, effects);
        }
    }

    fn server_tick(&self, host: &mut dyn ServerHost, event: &mut ServerTickEvent<'_>) {
        if !matches!(event.phase, ServerTickPhase::ServerTickEnd { .. }) {
            return;
        }
        let mut state = self.0.0.lock().unwrap();
        let Some(server) = &state.server else {
            return;
        };
        // The socket thread relays speech by where players were at the end of the last tick.
        let mut positions = HashMap::new();
        for player in host.players().server_players() {
            let (Some(position), Some(dimension)) =
                (player.entity().eye_position(host), player.dimension(host))
            else {
                continue;
            };
            positions.insert(
                player.uuid(),
                Position {
                    level: level_id(&dimension.to_string()),
                    position,
                },
            );
        }
        server.set_positions(positions);
        for event in server.take_events() {
            match event {
                Event::Connected(uuid) => {
                    if let Some(player_state) = state.states.get_mut(&uuid) {
                        player_state.disconnected = false;
                        state.broadcast_state(host, uuid);
                    }
                }
                Event::TimedOut(uuid) => {
                    if let Some(player_state) = state.states.get_mut(&uuid) {
                        player_state.disconnected = true;
                        state.broadcast_state(host, uuid);
                        // Still in the game: let the client connect again with a new secret.
                        if state.compatible.contains(&uuid) {
                            state.send_secret(host, uuid);
                        }
                    }
                }
            }
        }
    }
}
