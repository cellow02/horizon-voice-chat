//! Horizon Voice Chat, client side.
//!
//! Asks the server for a voice secret on the `voicechat:request_secret` plugin channel, opens
//! the encrypted UDP voice connection (`net.rs`), sends the microphone (`audio.rs`) and plays
//! everyone in range by where they stand (`mixer.rs`). The protocol is the one the Simple Voice
//! Chat mod speaks, so this works on Horizon servers running Horizon Voice Chat (Server) and
//! on Java servers running Simple Voice Chat.

mod audio;
mod codec;
mod mixer;
mod net;
#[allow(dead_code)]
mod protocol;

use audio::{Audio, Captured};
use mixer::Listener;
use mod_api::{
    ClientHost, ClientPacketEvent, ClientPlayerLoggingInEvent, ClientPlayerLoggingOutEvent,
    ClientTickPostEvent, GuiRenderPostEvent, Hooks, Identifier, KeyConflictContext,
    KeyRegistration, Registrar, RegistrationContext, ServerboundPlayPacket, SettingSpec,
};
use net::{Status, Target, VoiceClient};
use protocol::wire::Writer;
use protocol::{COMPATIBILITY_VERSION, PlayerState, SecretMessage, channel};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

const MOD_ID: &str = "horizon_voice";

/// Client ticks between two requests for a secret, and how many are made. A server without
/// voice chat never answers, so the mod stops asking.
const REQUEST_INTERVAL: u32 = 100;
const REQUEST_ATTEMPTS: u32 = 3;

#[derive(Default)]
struct Client {
    /// In a world, between logging in and logging out.
    joined: bool,
    ticks: u32,
    requests: u32,
    next_request: u32,
    /// What the server answered; None while it has not (or has no voice chat).
    secret: Option<SecretMessage>,
    connection: Option<VoiceClient>,
    audio: Option<Audio>,
    sequence: i64,
    muted: bool,
    disabled: bool,
    /// The other players' voice states, for the names on the HUD.
    states: HashMap<u128, PlayerState>,
    /// What the HUD shows, worked out once a tick.
    hud: Vec<(String, u32)>,
}

#[derive(Default)]
struct Mod(Mutex<Client>);

/// Sends a plugin message (a vanilla custom payload) to the server.
fn send_plugin_message(host: &mut dyn ClientHost, channel: &str, body: &[u8]) -> bool {
    let Ok(id) = Identifier::parse("minecraft:custom_payload") else {
        return false;
    };
    let mut packet = Writer::new();
    packet.string(channel).raw(body);
    match host.send_packet(ServerboundPlayPacket::new(id, packet.finish())) {
        Ok(()) => true,
        Err(error) => {
            log::debug!("Could not send {channel}: {error:?}");
            false
        }
    }
}

impl Client {
    fn leave(&mut self) {
        *self = Self::default();
    }

    /// The host the game joined: voice goes to the same machine unless the server says otherwise.
    fn game_host(host: &dyn ClientHost) -> String {
        let address = host
            .client_session()
            .client_server()
            .and_then(|server| server.address);
        match address {
            Some(address) => protocol::split_host_port(address.trim()).0.to_owned(),
            // Single player, a LAN world this window hosts, or a split-screen guest.
            None => "127.0.0.1".to_owned(),
        }
    }

    fn on_secret(&mut self, host: &dyn ClientHost, message: SecretMessage) {
        let (voice_host, port) = message.voice_address(&Self::game_host(host));
        log::info!("Connecting to voice chat at {voice_host}:{port}");
        // A new secret replaces an old connection (the server timed us out and asked again).
        self.connection = None;
        self.sequence = 0;
        match VoiceClient::connect(Target {
            host: voice_host,
            port,
            connection: message.connection,
            secret: message.secret.clone(),
            keep_alive_ms: u32::try_from(message.keep_alive).unwrap_or(1000),
        }) {
            Ok(connection) => self.connection = Some(connection),
            Err(error) => log::warn!("Voice chat could not start its connection: {error}"),
        }
        if self.audio.is_none() {
            self.audio = Some(Audio::start(message.codec));
        }
        self.secret = Some(message);
    }

    fn send_state(&self, host: &mut dyn ClientHost) {
        send_plugin_message(
            host,
            channel::CLIENT_STATE,
            &protocol::encode_client_state(self.muted, self.disabled),
        );
    }

    fn tick(&mut self, host: &mut dyn ClientHost) {
        self.ticks += 1;
        if self.secret.is_none() {
            // Ask the server whether it has voice chat. The first ticks after logging in the
            // connection may not accept packets yet, so a refused send is simply retried.
            if self.requests < REQUEST_ATTEMPTS && self.ticks >= self.next_request {
                let body = protocol::encode_request_secret(COMPATIBILITY_VERSION);
                if send_plugin_message(host, channel::REQUEST_SECRET, &body) {
                    self.requests += 1;
                    self.next_request = self.ticks + REQUEST_INTERVAL;
                } else {
                    self.next_request = self.ticks + 10;
                }
            }
            self.hud.clear();
            return;
        }

        if host.consume_click(MOD_ID, "mute") {
            self.muted = !self.muted;
            self.send_state(host);
        }
        if host.consume_click(MOD_ID, "disable") {
            self.disabled = !self.disabled;
            self.send_state(host);
        }

        let status = self.connection.as_ref().map(VoiceClient::status);
        if matches!(status, Some(Status::TimedOut | Status::Failed)) {
            // Start over: the server hands out a new secret when asked again.
            self.connection = None;
            self.secret = None;
            self.requests = 0;
            self.next_request = self.ticks + 40;
            if let Some(audio) = &self.audio {
                audio.mixer.lock().unwrap().clear();
            }
            return;
        }

        let Some(audio) = &self.audio else {
            return;
        };
        let whispering = host.is_down(MOD_ID, "whisper");
        let controls = &audio.controls;
        controls
            .push_to_talk
            .store(host.is_down(MOD_ID, "push_to_talk"), Ordering::Relaxed);
        controls
            .voice_activation
            .store(host.setting_choice("activation") == Some(1), Ordering::Relaxed);
        controls
            .muted
            .store(self.muted || self.disabled, Ordering::Relaxed);
        let threshold = host.setting_f64("threshold").unwrap_or(-40.0) as f32;
        controls
            .threshold_db
            .store(threshold.to_bits(), Ordering::Relaxed);
        let microphone = host.setting_f64("microphone_volume").unwrap_or(1.0) as f32;
        controls
            .microphone_volume
            .store(microphone.to_bits(), Ordering::Relaxed);

        // The microphone's frames go out; frames captured while not connected are dropped.
        for captured in audio.captured.try_iter() {
            let Some(connection) = &self.connection else {
                continue;
            };
            let data = match &captured {
                Captured::Frame(data) => data.as_slice(),
                Captured::End => &[],
            };
            connection.send_mic(data, self.sequence, whispering);
            self.sequence += 1;
        }

        let talking = {
            let mut mixer = audio.mixer.lock().unwrap();
            if let Some(connection) = &self.connection {
                for sound in connection.sounds() {
                    mixer.feed(&sound);
                }
            }
            mixer.volume = host.setting_f64("output_volume").unwrap_or(1.0) as f32;
            mixer.deafened = self.disabled;
            if let Some(camera) = host.camera() {
                mixer.listener = Listener {
                    position: camera.position,
                    yaw: camera.yaw,
                };
            }
            // Each talking player is heard where the game shows them.
            for sender in mixer.senders() {
                let position = host
                    .player(sender)
                    .and_then(|player| player.entity().eye_position(host));
                mixer.set_position(sender, position);
            }
            if self.ticks % 200 == 0 {
                mixer.forget_idle();
            }
            mixer.talking()
        };

        self.hud.clear();
        if host.setting_bool("hud") == Some(false) {
            return;
        }
        let text = |key: &str, fallback: &str| host.translate(key).unwrap_or_else(|| fallback.into());
        if self.disabled {
            self.hud
                .push((text("horizon_voice.hud.disabled", "Voice chat off"), 0xffff_5555));
            return;
        }
        if status != Some(Status::Connected) {
            self.hud.push((
                text("horizon_voice.hud.connecting", "Voice chat: connecting..."),
                0xffaa_aaaa,
            ));
            return;
        }
        if self.muted {
            self.hud
                .push((text("horizon_voice.hud.muted", "Microphone muted"), 0xffff_5555));
        } else if !controls.has_microphone.load(Ordering::Relaxed) {
            self.hud.push((
                text("horizon_voice.hud.no_microphone", "Voice chat: no microphone"),
                0xffff_aa00,
            ));
        } else if controls.talking.load(Ordering::Relaxed) {
            self.hud.push(if whispering {
                (text("horizon_voice.hud.whispering", "Whispering"), 0xff55_ffff)
            } else {
                (text("horizon_voice.hud.talking", "Talking"), 0xff55_ff55)
            });
        }
        for sender in talking.into_iter().take(8) {
            let name = match self.states.get(&sender) {
                Some(state) => state.name.clone(),
                None => "?".to_owned(),
            };
            self.hud.push((name, 0xffff_ffff));
        }
    }

    fn on_plugin_message(&mut self, host: &dyn ClientHost, channel: &str, body: &[u8]) {
        match channel {
            channel::SECRET => match SecretMessage::decode(body) {
                Ok(message) => self.on_secret(host, message),
                Err(error) => log::warn!("The server's voice chat secret is malformed: {error}"),
            },
            channel::STATES => {
                if let Ok(states) = protocol::decode_states(body) {
                    self.states = states.into_iter().map(|state| (state.uuid, state)).collect();
                }
            }
            channel::STATE => {
                if let Ok(state) = PlayerState::decode(body) {
                    self.states.insert(state.uuid, state);
                }
            }
            channel::REMOVE_STATE => {
                if let Ok(uuid) = protocol::decode_remove_state(body) {
                    self.states.remove(&uuid);
                }
            }
            _ => {}
        }
    }
}

#[mod_api::horizon_mod]
impl Hooks for Mod {
    fn register(&self, r: &mut dyn Registrar, _: &RegistrationContext) {
        let key = |id: &str, default: &str| {
            KeyRegistration::new(id, default)
                .label(&format!("horizon_voice.key.{id}"))
                .category("horizon_voice.key.category")
                .context(KeyConflictContext::InGame)
        };
        r.register_key_mapping(key("push_to_talk", "key.keyboard.v"));
        r.register_key_mapping(key("whisper", "key.keyboard.unknown"));
        r.register_key_mapping(key("mute", "key.keyboard.m"));
        r.register_key_mapping(key("disable", "key.keyboard.n"));

        let settings = [
            SettingSpec::choice(
                "activation",
                "horizon_voice.setting.activation",
                [
                    "horizon_voice.setting.activation.push_to_talk",
                    "horizon_voice.setting.activation.voice",
                ],
                0,
            )
            .tooltip("horizon_voice.setting.activation.tooltip"),
            SettingSpec::range(
                "threshold",
                "horizon_voice.setting.threshold",
                -60.0,
                -10.0,
                1.0,
                -40.0,
            )
            .tooltip("horizon_voice.setting.threshold.tooltip"),
            SettingSpec::range(
                "microphone_volume",
                "horizon_voice.setting.microphone_volume",
                0.0,
                3.0,
                0.05,
                1.0,
            ),
            SettingSpec::range(
                "output_volume",
                "horizon_voice.setting.output_volume",
                0.0,
                2.0,
                0.05,
                1.0,
            ),
            SettingSpec::toggle("hud", "horizon_voice.setting.hud", true),
        ];
        for setting in settings {
            if let Err(error) = r.register_setting(setting) {
                log::warn!("Voice chat setting refused: {error:?}");
            }
        }
    }

    fn client_player_logging_in(
        &self,
        _: &mut dyn ClientHost,
        _: &mut ClientPlayerLoggingInEvent,
    ) {
        let mut client = self.0.lock().unwrap();
        client.leave();
        client.joined = true;
        // Give the connection a second to settle before the first request.
        client.next_request = 20;
    }

    fn client_player_logging_out(
        &self,
        _: &mut dyn ClientHost,
        _: &mut ClientPlayerLoggingOutEvent,
    ) {
        // Closes the voice connection and the audio devices.
        self.0.lock().unwrap().leave();
    }

    fn client_tick_post(&self, host: &mut dyn ClientHost, _: &mut ClientTickPostEvent) {
        let mut client = self.0.lock().unwrap();
        if client.joined {
            client.tick(host);
        }
    }

    fn client_packet(&self, host: &mut dyn ClientHost, event: &mut ClientPacketEvent<'_>) {
        let Some(payload) = &event.packet.custom_payload else {
            return;
        };
        if !payload.channel.starts_with("voicechat:") {
            return;
        }
        let mut client = self.0.lock().unwrap();
        if client.joined {
            client.on_plugin_message(host, &payload.channel, &payload.data);
        }
    }

    fn gui_render_post(&self, _: &dyn ClientHost, event: &mut GuiRenderPostEvent<'_>) {
        if event.gui_hidden {
            return;
        }
        let client = self.0.lock().unwrap();
        if client.hud.is_empty() {
            return;
        }
        // Bottom left, above the chat line, growing upwards.
        let [_, height] = event.batch.size();
        let mut y = height - 44.0 - 10.0 * client.hud.len() as f32;
        for (line, color) in &client.hud {
            let width = event.batch.text_width(line);
            event.batch.fill([2.0, y - 1.0, width + 4.0, 10.0], 0x8000_0000);
            event.batch.text(line, [4.0, y], *color, true);
            y += 10.0;
        }
    }
}
