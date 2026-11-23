//! Horizon Voice Chat, client side.
//!
//! Asks the server for a voice secret on the `voicechat:request_secret` plugin channel, opens
//! the encrypted UDP voice connection (`net.rs`), sends the microphone (`audio.rs`) and plays
//! everyone in range by where they stand (`mixer.rs`). The protocol is the one the Simple Voice
//! Chat mod speaks, so this works on Horizon servers running Horizon Voice Chat (Server) and
//! on Java servers running Simple Voice Chat. The menus (`ui.rs`) follow that mod's layout.

mod audio;
mod codec;
mod icons;
mod mixer;
mod net;
#[allow(dead_code)]
mod protocol;
mod settings;
mod ui;

use audio::{Audio, Captured, Devices};
use mixer::Listener;
use mod_api::{
    ClientHost, ClientPacketEvent, ClientPlayerLoggingInEvent, ClientPlayerLoggingOutEvent,
    ClientTickPostEvent, ConfigPath, Flow, GuiRenderPostEvent, Hooks, Identifier,
    KeyConflictContext, KeyRegistration, Registrar, RegistrationContext,
    ScreenCharTypedPreEvent, ScreenClosingEvent, ScreenKeyPressedPreEvent,
    ScreenMouseButtonPressedPreEvent, ScreenMouseButtonReleasedPreEvent,
    ScreenMouseDraggedPreEvent, ScreenMouseScrolledPreEvent, ScreenRenderPostEvent,
    ServerboundPlayPacket,
};
use net::{Place, Sound, Status, Target, VoiceClient};
use protocol::wire::Writer;
use protocol::{COMPATIBILITY_VERSION, ClientGroup, PlayerState, SecretMessage, channel};
use settings::{Activation, Settings};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use ui::{Action, Page, Ui};

const MOD_ID: &str = "horizon_voice";

/// Client ticks between two requests for a secret, and how many are made. A server without
/// voice chat never answers, so the mod stops asking.
const REQUEST_INTERVAL: u32 = 100;
const REQUEST_ATTEMPTS: u32 = 3;

/// The microphone icon of the HUD.
#[derive(Clone, Copy, PartialEq)]
enum HudIcon {
    Disconnected,
    Disabled,
    Muted,
    Talking,
    Whispering,
}

/// What the HUD shows, worked out once a tick.
#[derive(Default)]
struct Hud {
    status: Option<HudIcon>,
    /// The group's members: uuid, talking, voice chat off.
    group: Vec<(u128, bool, bool)>,
    /// Who is talking nearby.
    talking: Vec<(u128, String)>,
    /// A line of text under the icon (no microphone).
    note: Option<String>,
}

#[derive(Default)]
pub(crate) struct Client {
    /// In a world, between logging in and logging out.
    joined: bool,
    ticks: u32,
    requests: u32,
    next_request: u32,
    /// What the server answered; None while it has not (or has no voice chat).
    pub(crate) secret: Option<SecretMessage>,
    connection: Option<VoiceClient>,
    pub(crate) audio: Option<Audio>,
    sequence: i64,
    /// Sequence numbers of the microphone test's frames.
    test_sequence: i64,
    /// Every player's voice state.
    pub(crate) states: HashMap<u128, PlayerState>,
    /// The groups the server told us about.
    pub(crate) groups: HashMap<u128, ClientGroup>,
    /// The group we are in.
    pub(crate) own_group: Option<u128>,
    pub(crate) settings: Settings,
    settings_loaded: bool,
    settings_dirty: bool,
    /// Our mute and off state still has to reach the server.
    state_unsent: bool,
    /// Who was heard in the last moments.
    pub(crate) talking: Vec<u128>,
    pub(crate) ui: Ui,
    hud: Hud,
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
        // The settings stay; everything else belongs to the world we left.
        let settings = std::mem::take(&mut self.settings);
        let loaded = self.settings_loaded;
        *self = Self::default();
        self.settings = settings;
        self.settings_loaded = loaded;
    }

    pub(crate) fn save_settings_later(&mut self) {
        self.settings_dirty = true;
    }

    fn load_settings(&mut self, host: &dyn ClientHost) {
        if self.settings_loaded {
            return;
        }
        self.settings_loaded = true;
        let Ok(path) = ConfigPath::new(settings::FILE) else {
            return;
        };
        if let Ok(bytes) = host.config_files().read(&path) {
            self.settings = Settings::parse(&String::from_utf8_lossy(&bytes));
        }
    }

    fn save_settings(&mut self, host: &dyn ClientHost) {
        if !self.settings_dirty {
            return;
        }
        self.settings_dirty = false;
        if let Ok(path) = ConfigPath::new(settings::FILE) {
            if let Err(error) = host.config_files().write(&path, self.settings.write().as_bytes()) {
                log::warn!("Could not save the voice chat settings: {error:?}");
            }
        }
    }

    fn devices(&self) -> Devices {
        Devices {
            microphone: self.settings.microphone.clone(),
            speaker: self.settings.speaker.clone(),
        }
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
            self.audio = Some(Audio::start_with(message.codec, &self.devices()));
        }
        self.secret = Some(message);
        // The server keeps no state of ours across connections: tell it again.
        self.state_unsent = true;
    }

    fn send_state(&self, host: &mut dyn ClientHost) {
        send_plugin_message(
            host,
            channel::CLIENT_STATE,
            &protocol::encode_client_state(self.settings.muted, self.settings.disabled),
        );
    }

    /// Runs what the menus queued.
    fn run_actions(&mut self, host: &mut dyn ClientHost) {
        for action in std::mem::take(&mut self.ui.actions) {
            match action {
                Action::Close => {
                    host.mod_screen_close(MOD_ID);
                    self.stop_mic_test();
                    self.ui.closed();
                }
                Action::SendState => self.send_state(host),
                Action::Send(channel, body) => {
                    send_plugin_message(host, channel, &body);
                }
                Action::RestartAudio => {
                    if let Some(secret) = &self.secret {
                        let codec = secret.codec;
                        // Close the old devices before opening the new ones.
                        self.audio = None;
                        self.audio = Some(Audio::start_with(codec, &self.devices()));
                    }
                }
            }
        }
    }

    /// Opens the menus at `page`.
    fn open(&mut self, host: &mut dyn ClientHost, page: Page) {
        if host.mod_screen_open(MOD_ID) {
            self.ui.show(page);
        }
    }

    /// The keys that work while playing: the menus, mute, off, hiding the icons.
    fn keys(&mut self, host: &mut dyn ClientHost) {
        if host.consume_click(MOD_ID, "menu") {
            self.open(host, Page::Main);
        }
        if host.consume_click(MOD_ID, "groups") && self.groups_enabled() {
            let page = if self.own_group.is_some() {
                Page::Group
            } else {
                Page::JoinGroup
            };
            self.open(host, page);
        }
        if host.consume_click(MOD_ID, "hide_icons") {
            self.settings.hide_icons = !self.settings.hide_icons;
            self.save_settings_later();
        }
        if host.consume_click(MOD_ID, "mute") && !self.settings.disabled {
            self.settings.muted = !self.settings.muted;
            self.save_settings_later();
            if self.secret.is_some() {
                self.send_state(host);
            }
        }
        if host.consume_click(MOD_ID, "disable") {
            self.settings.disabled = !self.settings.disabled;
            self.save_settings_later();
            if self.secret.is_some() {
                self.send_state(host);
            }
        }
    }

    fn tick(&mut self, host: &mut dyn ClientHost) {
        self.ticks += 1;
        self.load_settings(host);
        self.run_actions(host);
        self.keys(host);
        self.save_settings(host);
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
            self.hud = Hud::default();
            return;
        }
        if std::mem::take(&mut self.state_unsent) {
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

        let own = self.secret.as_ref().map(|secret| secret.player);
        let Some(audio) = &self.audio else {
            return;
        };
        let whispering = host.is_down(MOD_ID, "whisper");
        let settings = &self.settings;
        let controls = &audio.controls;
        controls
            .push_to_talk
            .store(host.is_down(MOD_ID, "push_to_talk"), Ordering::Relaxed);
        controls
            .voice_activation
            .store(settings.activation == Activation::Voice, Ordering::Relaxed);
        controls
            .muted
            .store(settings.muted || settings.disabled, Ordering::Relaxed);
        controls
            .threshold_db
            .store(settings.threshold_db.to_bits(), Ordering::Relaxed);
        controls
            .microphone_volume
            .store(settings.microphone_amplification.to_bits(), Ordering::Relaxed);

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
                    // Our own voice never plays back, whatever a server relays.
                    if Some(sound.sender) == own {
                        continue;
                    }
                    mixer.feed(&sound);
                }
            }
            // The microphone test plays our voice here, never sent anywhere.
            for data in audio.tested.try_iter() {
                mixer.feed(&Sound {
                    channel: 0,
                    sender: 0,
                    data,
                    sequence: self.test_sequence,
                    place: Place::Everywhere,
                    whispering: false,
                });
                self.test_sequence += 1;
            }
            mixer.volume = settings.voice_volume;
            mixer.deafened = settings.disabled;
            mixer.stereo = settings.stereo;
            if mixer.player_volumes != settings.player_volumes {
                mixer.player_volumes = settings.player_volumes.clone();
            }
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
        self.talking = talking.into_iter().filter(|sender| *sender != 0).collect();
        self.update_hud(host, status == Some(Status::Connected), whispering);
    }

    fn update_hud(&mut self, host: &dyn ClientHost, connected: bool, whispering: bool) {
        self.hud = Hud::default();
        if self.settings.hide_icons {
            return;
        }
        let controls = self.audio.as_ref().map(|audio| &audio.controls);
        let talking_now = controls.is_some_and(|c| {
            c.talking.load(Ordering::Relaxed) && !c.monitor.load(Ordering::Relaxed)
        });
        self.hud.status = if !connected {
            Some(HudIcon::Disconnected)
        } else if self.settings.disabled {
            Some(HudIcon::Disabled)
        } else if self.settings.muted {
            Some(HudIcon::Muted)
        } else if talking_now {
            Some(if whispering {
                HudIcon::Whispering
            } else {
                HudIcon::Talking
            })
        } else {
            None
        };
        if connected
            && !self.settings.disabled
            && !controls.is_some_and(|c| c.has_microphone.load(Ordering::Relaxed))
        {
            self.hud.note = host.translate("horizon_voice.hud.no_microphone");
        }
        if self.settings.disabled {
            return;
        }
        let own = self.secret.as_ref().map(|secret| secret.player);
        let mut members = Vec::new();
        if let Some(group) = self.own_group.filter(|_| self.settings.show_group_hud) {
            for (uuid, _, disabled) in self.members(group) {
                let talking = if Some(uuid) == own {
                    talking_now
                } else {
                    self.talking.contains(&uuid)
                };
                members.push((uuid, talking, disabled));
            }
        }
        self.hud.group = members;
        // Players heard nearby (group members show in the group list instead).
        self.hud.talking = self
            .talking
            .iter()
            .filter(|uuid| !self.hud.group.iter().any(|(member, ..)| member == *uuid))
            .take(8)
            .map(|uuid| {
                let name = self.states.get(uuid).map(|state| state.name.clone());
                (*uuid, name.unwrap_or_else(|| "?".to_owned()))
            })
            .collect();
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
            channel::ADD_GROUP => {
                if let Ok(group) = ClientGroup::decode(body) {
                    self.groups.insert(group.id, group);
                }
            }
            channel::REMOVE_GROUP => {
                if let Ok(id) = protocol::decode_remove_group(body) {
                    self.groups.remove(&id);
                }
            }
            channel::JOINED_GROUP => {
                if let Ok((group, wrong_password)) = protocol::decode_joined_group(body) {
                    self.own_group = group;
                    if wrong_password {
                        self.ui.wrong_password = true;
                        return;
                    }
                    // The open group menu follows: into the group, or back to the list.
                    match (self.ui.page, group) {
                        (
                            Some(Page::JoinGroup | Page::CreateGroup | Page::Password(_)),
                            Some(_),
                        ) => self.ui.show(Page::Group),
                        (Some(Page::Group), None) => self.ui.show(Page::JoinGroup),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// Draws the voice chat icons over the game: the microphone at the bottom left, the
    /// group's members at the top left, and who is talking nearby.
    fn draw_hud(&self, draw: &mut dyn mod_api::DrawContext) {
        let [_, height] = draw.size();
        let at = [16.0, height - 32.0];
        match self.hud.status {
            Some(HudIcon::Disconnected) => icons::draw(draw, &icons::DISCONNECTED, at, 16.0),
            Some(HudIcon::Disabled) => {
                icons::draw(draw, &icons::SPEAKER, at, 16.0);
                icons::draw(draw, &icons::SLASH, at, 16.0);
            }
            Some(HudIcon::Muted) => {
                icons::draw(draw, &icons::MICROPHONE, at, 16.0);
                icons::draw(draw, &icons::SLASH, at, 16.0);
            }
            Some(HudIcon::Talking) => icons::draw(draw, &icons::MICROPHONE, at, 16.0),
            Some(HudIcon::Whispering) => {
                icons::draw_tinted(draw, &icons::MICROPHONE, at, 16.0, 0xff80_c0ff)
            }
            None => {}
        }
        if let Some(note) = &self.hud.note {
            draw.text(note, [36.0, height - 28.0], 0xffff_aa00, true);
        }
        // Who talks nearby, above the microphone.
        let mut y = height - 44.0;
        for (uuid, name) in &self.hud.talking {
            ui::head(draw, *uuid, [16.0, y], 8.0);
            draw.text(name, [27.0, y], 0xffff_ffff, true);
            y -= 11.0;
        }
        // The group, top left: twice the size of a head, one under the other.
        for (index, (uuid, talking, disabled)) in self.hud.group.iter().enumerate() {
            let at = [4.0, 4.0 + index as f32 * 22.0];
            if *talking && !*disabled {
                ui::talk_outline(draw, at, 20.0);
            }
            ui::head(draw, *uuid, [at[0] + 2.0, at[1] + 2.0], 16.0);
            if *disabled {
                icons::draw(draw, &icons::SPEAKER, [at[0] + 2.0, at[1] + 2.0], 8.0);
                icons::draw(draw, &icons::SLASH, [at[0] + 2.0, at[1] + 2.0], 8.0);
            }
        }
    }
}

/// Whether the screen of a screen hook is this mod's own.
fn ours(host: &dyn ClientHost, screen: &mod_api::ScreenView<'_>) -> bool {
    screen.id == "mod" && host.mod_screen() == Some(MOD_ID)
}

fn flow(taken: bool) -> Flow {
    if taken { Flow::Cancel } else { Flow::Continue }
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
        // The keys of the Simple Voice Chat mod.
        r.register_key_mapping(key("menu", "key.keyboard.v"));
        r.register_key_mapping(key("groups", "key.keyboard.g"));
        r.register_key_mapping(key("push_to_talk", "key.keyboard.caps.lock"));
        r.register_key_mapping(key("whisper", "key.keyboard.unknown"));
        r.register_key_mapping(key("mute", "key.keyboard.m"));
        r.register_key_mapping(key("disable", "key.keyboard.n"));
        r.register_key_mapping(key("hide_icons", "key.keyboard.h"));
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
        self.0.lock().unwrap().draw_hud(event.batch);
    }

    fn screen_render_post(&self, host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
        if ours(host, event.screen) {
            let mouse = event.screen.mouse;
            self.0.lock().unwrap().draw_screen(host, event.draw, mouse);
        }
    }

    fn screen_mouse_button_pressed_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        if !ours(host, event.screen) {
            return Flow::Continue;
        }
        let at = event.screen.mouse;
        flow(self.0.lock().unwrap().screen_pressed(host, at, event.button, event.modifiers))
    }

    fn screen_mouse_button_released_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
    ) -> Flow {
        if !ours(host, event.screen) {
            return Flow::Continue;
        }
        flow(self.0.lock().unwrap().screen_released(event.button))
    }

    fn screen_mouse_dragged_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseDraggedPreEvent<'_>,
    ) -> Flow {
        if !ours(host, event.screen) {
            return Flow::Continue;
        }
        let at = event.screen.mouse;
        flow(self.0.lock().unwrap().screen_dragged(host, at, event.button))
    }

    fn screen_mouse_scrolled_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseScrolledPreEvent<'_>,
    ) -> Flow {
        if !ours(host, event.screen) {
            return Flow::Continue;
        }
        let at = event.screen.mouse;
        flow(self.0.lock().unwrap().screen_scrolled(at, event.scroll[1]))
    }

    fn screen_key_pressed_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenKeyPressedPreEvent<'_>,
    ) -> Flow {
        if !ours(host, event.screen) {
            return Flow::Continue;
        }
        flow(self.0.lock().unwrap().screen_key(host, event.key, event.modifiers))
    }

    fn screen_char_typed_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenCharTypedPreEvent<'_>,
    ) -> Flow {
        if !ours(host, event.screen) {
            return Flow::Continue;
        }
        flow(self.0.lock().unwrap().screen_char(host, event.c))
    }

    fn screen_closing(&self, _: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
        if event.screen.id != "mod" {
            return;
        }
        let mut client = self.0.lock().unwrap();
        if client.ui.page.is_some() {
            client.stop_mic_test();
            client.ui.closed();
        }
    }
}
