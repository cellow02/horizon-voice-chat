//! Horizon Voice Chat, client side, for Horizon's older mod API (`horizon_client::mod_api`).
//!
//! The same mod as `client/` in this repository, on the hooks of Horizon builds before the mod
//! SDK: it asks the server for a voice secret on the `voicechat:request_secret` plugin channel,
//! opens the encrypted UDP voice connection (`net.rs`), sends the microphone (`audio.rs`) and
//! plays everyone in range by where they stand (`mixer.rs`). Those three files and `protocol/`
//! are copies (`tools/sync-protocol.sh`); only this file and `codec.rs` differ.

mod audio;
mod codec;
mod mixer;
mod net;
#[allow(dead_code)]
mod protocol;

use audio::{Audio, Captured};
use horizon_client::mod_api::{
    ClientDisconnectingEvent, ClientPacketEvent, ClientPlayerNetworkLoggingInEvent,
    ClientTickPostEvent, ClientboundPlayPacket, GuiRenderPostEvent, Hooks, Host, KeyEvent,
    ModsEvent, NativeMod, ServerboundPlayPacket, SettingsClickEvent, SettingsEvent,
};
use mixer::Listener;
use net::{Status, Target, VoiceClient};
use protocol::wire::Writer;
use protocol::{COMPATIBILITY_VERSION, PlayerState, SecretMessage, channel};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};

const MOD_ID: &str = "horizon_voice";

/// Client ticks between two requests for a secret, and how many are made. A server without
/// voice chat never answers, so the mod stops asking.
const REQUEST_INTERVAL: u32 = 100;
const REQUEST_ATTEMPTS: u32 = 3;

/// The keys: name, label on the Mods page, default.
const KEYS: [[&str; 3]; 4] = [
    ["push_to_talk", "Push to talk", "key.keyboard.v"],
    ["whisper", "Whisper (hold while talking)", "key.keyboard.unknown"],
    ["mute", "Mute microphone", "key.keyboard.m"],
    ["disable", "Voice chat on/off", "key.keyboard.n"],
];

/// What the buttons of the mod's settings page set. Each click moves to the next value.
#[derive(Clone, Copy, PartialEq)]
struct Options {
    voice_activation: bool,
    threshold_db: f32,
    microphone_volume: f32,
    output_volume: f32,
    hud: bool,
}

const THRESHOLDS: [f32; 6] = [-60.0, -50.0, -40.0, -30.0, -20.0, -10.0];
const MICROPHONE_VOLUMES: [f32; 6] = [0.5, 0.75, 1.0, 1.5, 2.0, 3.0];
const OUTPUT_VOLUMES: [f32; 6] = [0.25, 0.5, 0.75, 1.0, 1.5, 2.0];

impl Default for Options {
    fn default() -> Self {
        Self {
            voice_activation: false,
            threshold_db: -40.0,
            microphone_volume: 1.0,
            output_volume: 1.0,
            hud: true,
        }
    }
}

/// The value after `current` in `values`, wrapping around.
fn next(values: &[f32], current: f32) -> f32 {
    let at = values.iter().position(|v| (v - current).abs() < 0.001);
    values[at.map_or(0, |at| (at + 1) % values.len())]
}

impl Options {
    fn load(host: &dyn Host) -> Self {
        let default = Self::default();
        let number = |key: &str, values: &[f32], default: f32| {
            host.setting(MOD_ID, key)
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|v| values.iter().any(|allowed| (allowed - v).abs() < 0.001))
                .unwrap_or(default)
        };
        let flag = |key: &str, default: bool| {
            host.setting(MOD_ID, key)
                .map_or(default, |v| v == "true")
        };
        Self {
            voice_activation: flag("voice_activation", default.voice_activation),
            threshold_db: number("threshold", &THRESHOLDS, default.threshold_db),
            microphone_volume: number(
                "microphone_volume",
                &MICROPHONE_VOLUMES,
                default.microphone_volume,
            ),
            output_volume: number("output_volume", &OUTPUT_VOLUMES, default.output_volume),
            hud: flag("hud", default.hud),
        }
    }

    fn labels(&self) -> Vec<String> {
        let percent = |v: f32| format!("{}%", (v * 100.0).round() as i32);
        vec![
            format!(
                "Activation: {}",
                if self.voice_activation {
                    "Voice"
                } else {
                    "Push to talk"
                }
            ),
            format!("Voice activation threshold: {} dB", self.threshold_db as i32),
            format!("Microphone volume: {}", percent(self.microphone_volume)),
            format!("Voice chat volume: {}", percent(self.output_volume)),
            format!("Show status on screen: {}", if self.hud { "On" } else { "Off" }),
        ]
    }

    /// Button `button` of the settings page was clicked: changes and saves its value.
    fn click(&mut self, host: &dyn Host, button: usize) {
        let (key, value) = match button {
            0 => {
                self.voice_activation = !self.voice_activation;
                ("voice_activation", self.voice_activation.to_string())
            }
            1 => {
                self.threshold_db = next(&THRESHOLDS, self.threshold_db);
                ("threshold", self.threshold_db.to_string())
            }
            2 => {
                self.microphone_volume = next(&MICROPHONE_VOLUMES, self.microphone_volume);
                ("microphone_volume", self.microphone_volume.to_string())
            }
            3 => {
                self.output_volume = next(&OUTPUT_VOLUMES, self.output_volume);
                ("output_volume", self.output_volume.to_string())
            }
            4 => {
                self.hud = !self.hud;
                ("hud", self.hud.to_string())
            }
            _ => return,
        };
        host.set_setting(MOD_ID, key, &value);
    }
}

#[derive(Default)]
struct Client {
    /// In a world, between logging in and disconnecting.
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
    /// The names of the mod's keys that are held.
    held: HashSet<&'static str>,
    /// Mute or disable was pressed since the last tick.
    toggled: bool,
    /// The settings; None until they are first needed, since reading them reads files.
    options: Option<Options>,
    /// The other players' voice states, for the names on the HUD.
    states: HashMap<u128, PlayerState>,
    /// What the HUD shows, worked out once a tick.
    hud: Vec<(String, u32)>,
}

#[derive(Default)]
struct Mod(Mutex<Client>);

/// A serverbound plugin message (a vanilla custom payload).
///
/// The packet's struct is not among the names `mod_api` exports, so it is decoded from its wire
/// form. The packet id is found by trying each: only the custom payload's decodes to that variant.
fn plugin_message(channel: &str, body: &[u8]) -> Option<ServerboundPlayPacket> {
    static ID: OnceLock<Option<i32>> = OnceLock::new();
    let mut wire = Writer::new();
    wire.string(channel).raw(body);
    let wire = wire.finish();
    let decode = |id: i32| {
        ServerboundPlayPacket::decode(id, &wire)
            .ok()
            .filter(|packet| matches!(packet, ServerboundPlayPacket::CustomPayload(_)))
    };
    let id = *ID.get_or_init(|| (0..256).find(|id| decode(*id).is_some()));
    decode(id?)
}

fn send_plugin_message(host: &mut dyn Host, channel: &str, body: &[u8]) -> bool {
    let Some(packet) = plugin_message(channel, body) else {
        log::warn!("Voice chat could not build a {channel} packet");
        return false;
    };
    match host.send_packet(packet) {
        Ok(()) => true,
        Err(error) => {
            log::debug!("Could not send {channel}: {error}");
            false
        }
    }
}

impl Client {
    fn leave(&mut self) {
        // The settings stay: they are the player's, not the server's.
        let options = self.options;
        *self = Self::default();
        self.options = options;
    }

    fn options(&mut self, host: &dyn Host) -> Options {
        *self.options.get_or_insert_with(|| Options::load(host))
    }

    /// The host the game joined: voice goes to the same machine unless the server says otherwise.
    fn game_host(host: &dyn Host) -> String {
        match host.client_state().and_then(|state| state.server_address) {
            Some(address) => protocol::split_host_port(address.trim()).0.to_owned(),
            // Single player or a LAN world this window hosts.
            None => "127.0.0.1".to_owned(),
        }
    }

    fn on_secret(&mut self, host: &dyn Host, message: SecretMessage) {
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

    fn on_key(&mut self, host: &dyn Host, key: &str, pressed: bool) {
        for [name, _, default] in KEYS {
            if host.mod_key(MOD_ID, name, default) != key {
                continue;
            }
            if !pressed {
                self.held.remove(name);
            // A key typed into chat or a menu is not a voice chat key.
            } else if host.playing() {
                self.held.insert(name);
                match name {
                    "mute" => self.muted = !self.muted,
                    "disable" => self.disabled = !self.disabled,
                    _ => continue,
                }
                self.toggled = true;
            }
        }
    }

    fn tick(&mut self, host: &mut dyn Host) {
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

        if std::mem::take(&mut self.toggled) {
            send_plugin_message(
                host,
                channel::CLIENT_STATE,
                &protocol::encode_client_state(self.muted, self.disabled),
            );
        }
        // A key released while a menu had the keyboard may never be told.
        if !host.playing() {
            self.held.clear();
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

        let options = self.options(host);
        let Some(audio) = &self.audio else {
            return;
        };
        let whispering = self.held.contains("whisper");
        let controls = &audio.controls;
        controls
            .push_to_talk
            .store(self.held.contains("push_to_talk"), Ordering::Relaxed);
        controls
            .voice_activation
            .store(options.voice_activation, Ordering::Relaxed);
        controls
            .muted
            .store(self.muted || self.disabled, Ordering::Relaxed);
        controls
            .threshold_db
            .store(options.threshold_db.to_bits(), Ordering::Relaxed);
        controls
            .microphone_volume
            .store(options.microphone_volume.to_bits(), Ordering::Relaxed);

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

        let mut names = HashMap::new();
        let own = self.secret.as_ref().map(|secret| secret.player);
        let talking = {
            let mut mixer = audio.mixer.lock().unwrap();
            if let Some(connection) = &self.connection {
                for sound in connection.sounds() {
                    // Our own voice never plays back, whatever a server relays.
                    if Some(sound.sender) == own {
                        continue;
                    }
                    mixer.feed(&sound);
                }
            }
            mixer.volume = options.output_volume;
            mixer.deafened = self.disabled;
            if let Some(camera) = host.camera() {
                mixer.listener = Listener {
                    position: camera.position,
                    yaw: camera.yaw,
                };
            }
            // Each talking player is heard where the game shows them.
            let senders = mixer.senders();
            if !senders.is_empty() {
                let mut positions = HashMap::new();
                for entity in host.client_entities() {
                    let Some(uuid) = entity.player_uuid else {
                        continue;
                    };
                    // A player's eyes are at nine tenths of their height.
                    let [x, y, z] = entity.position;
                    positions.insert(uuid, [x, y + f64::from(entity.height) * 0.9, z]);
                    if let Some(name) = entity.player_name {
                        names.insert(uuid, name);
                    }
                }
                for sender in senders {
                    mixer.set_position(sender, positions.get(&sender).copied());
                }
            }
            if self.ticks % 200 == 0 {
                mixer.forget_idle();
            }
            mixer.talking()
        };

        self.hud.clear();
        if !options.hud {
            return;
        }
        if self.disabled {
            self.hud.push(("Voice chat off".into(), 0xff_5555));
            return;
        }
        if status != Some(Status::Connected) {
            self.hud
                .push(("Voice chat: connecting...".into(), 0xaa_aaaa));
            return;
        }
        if self.muted {
            self.hud.push(("Microphone muted".into(), 0xff_5555));
        } else if !controls.has_microphone.load(Ordering::Relaxed) {
            self.hud
                .push(("Voice chat: no microphone".into(), 0xff_aa00));
        } else if controls.talking.load(Ordering::Relaxed) {
            self.hud.push(if whispering {
                ("Whispering".into(), 0x55_ffff)
            } else {
                ("Talking".into(), 0x55_ff55)
            });
        }
        for sender in talking.into_iter().take(8) {
            let name = match (self.states.get(&sender), names.remove(&sender)) {
                (Some(state), _) => state.name.clone(),
                (None, Some(name)) => name,
                (None, None) => "?".to_owned(),
            };
            self.hud.push((name, 0xff_ffff));
        }
    }

    fn on_plugin_message(&mut self, host: &dyn Host, channel: &str, body: &[u8]) {
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

impl Hooks for Mod {
    fn mods(&self, _: &mut dyn Host, event: &mut ModsEvent<'_>) {
        event.mods.push(NativeMod {
            id: MOD_ID.into(),
            title: concat!("Horizon Voice Chat ", env!("CARGO_PKG_VERSION")).into(),
            description: "Proximity voice chat, compatible with Simple Voice Chat.".into(),
            keys: KEYS.map(|key| key.map(str::to_owned)).to_vec(),
        });
    }

    fn settings(&self, host: &mut dyn Host, event: &mut SettingsEvent<'_>) {
        if event.mod_id == MOD_ID {
            let options = self.0.lock().unwrap().options(host);
            event.buttons.extend(options.labels());
        }
    }

    fn settings_click(&self, host: &mut dyn Host, event: &mut SettingsClickEvent<'_>) {
        if event.mod_id == MOD_ID {
            let mut client = self.0.lock().unwrap();
            let mut options = client.options(host);
            options.click(host, event.button);
            client.options = Some(options);
        }
    }

    fn key(&self, host: &mut dyn Host, event: &mut KeyEvent<'_>) {
        let mut client = self.0.lock().unwrap();
        if client.joined {
            client.on_key(host, event.key, event.pressed);
        }
    }

    fn client_player_network_logging_in(
        &self,
        _: &mut dyn Host,
        _: &mut ClientPlayerNetworkLoggingInEvent,
    ) {
        let mut client = self.0.lock().unwrap();
        client.leave();
        client.joined = true;
        // Give the connection a second to settle before the first request.
        client.next_request = 20;
    }

    fn client_disconnecting(&self, _: &dyn Host, _: &mut ClientDisconnectingEvent) {
        // Closes the voice connection and the audio devices.
        self.0.lock().unwrap().leave();
    }

    fn client_tick_post(&self, host: &mut dyn Host, _: &mut ClientTickPostEvent) {
        let mut client = self.0.lock().unwrap();
        if client.joined {
            client.tick(host);
        }
    }

    fn client_packet(&self, host: &mut dyn Host, event: &mut ClientPacketEvent<'_>) {
        let ClientboundPlayPacket::CustomPayload(payload) = event.packet else {
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

    fn gui_render_post(&self, host: &dyn Host, event: &mut GuiRenderPostEvent<'_>) {
        if event.gui_hidden {
            return;
        }
        let client = self.0.lock().unwrap();
        if client.hud.is_empty() {
            return;
        }
        let Some(state) = host.client_state() else {
            return;
        };
        // Bottom left, above the chat line, growing upwards.
        let mut y = state.gui_size[1] - 44.0 - 10.0 * client.hud.len() as f32;
        for (line, color) in &client.hud {
            let width = host.text_width(line);
            host.fill(event.batch, [2.0, y - 1.0, width + 4.0, 10.0], 0x8000_0000);
            host.text(event.batch, line, [4.0, y], *color, true);
            y += 10.0;
        }
    }
}

/// What the launcher's generated game calls to install the mod.
pub fn hooks() -> Box<dyn Hooks> {
    Box::new(Mod::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plugin_message_becomes_a_custom_payload_packet() {
        let packet = plugin_message(channel::REQUEST_SECRET, &[0, 0, 0, 21]).unwrap();
        let ServerboundPlayPacket::CustomPayload(payload) = packet else {
            panic!("not a custom payload");
        };
        assert_eq!(payload.channel, channel::REQUEST_SECRET);
        assert_eq!(payload.data, [0, 0, 0, 21]);
    }

    #[test]
    fn a_click_moves_a_setting_to_its_next_value_and_wraps() {
        assert_eq!(next(&THRESHOLDS, -40.0), -30.0);
        assert_eq!(next(&THRESHOLDS, -10.0), -60.0);
        assert_eq!(next(&OUTPUT_VOLUMES, 7.0), 0.25);
        assert_eq!(Options::default().labels().len(), 5);
    }
}
