//! The voice chat menus: the main menu (V), the settings, the player volumes, the microphone
//! and speaker lists, and the group menus (G). They follow the layout of the Simple Voice Chat
//! mod's menus (sizes, positions, order of the controls) on the game's own buttons and sliders,
//! with panels and icons drawn by this mod.
//!
//! The menus are drawn immediately: each frame lays a page out from the client's state and
//! remembers where its controls are, and a click looks the control up there. The screen hooks
//! only see the game through `&dyn ClientHost`, so what needs more (sending, opening and
//! closing the screen) is queued as an [`Action`] for the next client tick.

use crate::icons::{self, Icon};
use crate::protocol::{self, ClientGroup, GroupType, channel};
use crate::settings::Activation;
use crate::{Client, MOD_ID};
use mod_api::client::widgets::{Outcome, TextField, WidgetId, Widgets};
use mod_api::{ClientHost, DrawContext, SoundPlay, Text};
use std::sync::atomic::Ordering;

/// Text on the light panels.
const FONT_COLOR: u32 = 0xff40_4040;
const WHITE: u32 = 0xffff_ffff;
const GRAY: u32 = 0xffaa_aaaa;
const ROW_FILL: u32 = 0xff4a_4a4a;
const ROW_FILL_HOVERED: u32 = 0xff5a_5a5a;

/// List pages: header, unit and footer heights, and the height of a row with its gap.
const HEADER: f32 = 16.0;
const UNIT: f32 = 18.0;
const ROW: f32 = 36.0;
const LIST_WIDTH: f32 = 236.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Page {
    Main,
    Settings,
    Volumes,
    /// The microphones (`true`) or the speakers.
    Devices(bool),
    JoinGroup,
    Group,
    CreateGroup,
    Password(u128),
}

/// What the next client tick does for the menus.
pub enum Action {
    Close,
    /// Tell the server the mute and off state.
    SendState,
    Send(&'static str, Vec<u8>),
    /// Reopen the audio devices (another microphone or speaker was picked).
    RestartAudio,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Slider {
    VoiceVolume,
    MicAmplification,
    Threshold,
    Player(u128),
}

#[derive(Debug, Clone, PartialEq)]
enum Hit {
    Page(Page),
    Mute,
    Disable,
    HideIcons,
    GroupHud,
    Leave,
    Create,
    CycleType,
    Join(u128),
    SubmitPassword,
    Slider(Slider),
    MicTest,
    Activation,
    Stereo,
    Device(Option<String>),
    /// Only shows its tooltip.
    Nothing,
}

#[derive(Default)]
pub struct Ui {
    pub page: Option<Page>,
    pub actions: Vec<Action>,
    /// Where the controls of the page were drawn last frame.
    hits: Vec<([f32; 4], Hit, Option<String>)>,
    fields: Widgets,
    /// The text fields of the page: name and password, or password alone.
    field_ids: Vec<WidgetId>,
    fields_page: Option<Page>,
    dragging: Option<([f32; 4], Slider)>,
    scroll: f32,
    max_scroll: f32,
    list: [f32; 4],
    group_type: GroupType,
    pub wrong_password: bool,
    devices: Vec<String>,
}

fn over([x, y, w, h]: [f32; 4], [px, py]: [f32; 2]) -> bool {
    px >= x && py >= y && px < x + w && py < y + h
}

/// `key`'s text in the game's language, `%s` replaced by `arg`.
fn tr(host: &dyn ClientHost, key: &str, arg: &str) -> String {
    let key = format!("horizon_voice.gui.{key}");
    host.translate(&key).unwrap_or(key).replace("%s", arg)
}

/// A readable name for a key id the game could not translate:
/// `key.keyboard.caps.lock` becomes `Caps Lock`.
fn key_name(id: &str) -> String {
    let rest = id
        .strip_prefix("key.keyboard.")
        .or_else(|| id.strip_prefix("key.mouse."))
        .unwrap_or(id);
    rest.split(['.', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn percent(value: f32) -> String {
    format!("{:.0}%", value * 100.0)
}

/// A panel as the game's containers have it: light grey, a white top left edge and a dark
/// bottom right one inside a black outline with cut corners.
fn panel(draw: &mut dyn DrawContext, [x, y, w, h]: [f32; 4]) {
    draw.fill([x + 2.0, y, w - 4.0, 1.0], 0xff00_0000);
    draw.fill([x + 2.0, y + h - 1.0, w - 4.0, 1.0], 0xff00_0000);
    draw.fill([x, y + 2.0, 1.0, h - 4.0], 0xff00_0000);
    draw.fill([x + w - 1.0, y + 2.0, 1.0, h - 4.0], 0xff00_0000);
    for corner in [
        [x + 1.0, y + 1.0],
        [x + w - 2.0, y + 1.0],
        [x + 1.0, y + h - 2.0],
        [x + w - 2.0, y + h - 2.0],
    ] {
        draw.fill([corner[0], corner[1], 1.0, 1.0], 0xff00_0000);
    }
    draw.fill([x + 1.0, y + 2.0, w - 2.0, h - 4.0], 0xffc6_c6c6);
    draw.fill([x + 2.0, y + 1.0, w - 4.0, h - 2.0], 0xffc6_c6c6);
    draw.fill([x + 2.0, y + 1.0, w - 5.0, 2.0], WHITE);
    draw.fill([x + 1.0, y + 2.0, 2.0, h - 5.0], WHITE);
    draw.fill([x + 3.0, y + 3.0, 1.0, 1.0], WHITE);
    draw.fill([x + 3.0, y + h - 3.0, w - 5.0, 2.0], 0xff55_5555);
    draw.fill([x + w - 3.0, y + 3.0, 2.0, h - 5.0], 0xff55_5555);
    draw.fill([x + w - 4.0, y + h - 4.0, 1.0, 1.0], 0xff55_5555);
}

/// A sunken dark area (the lists): black top left, white bottom right.
fn inset(draw: &mut dyn DrawContext, [x, y, w, h]: [f32; 4]) {
    draw.fill([x, y, w, h], WHITE);
    draw.fill([x, y, w - 1.0, h - 1.0], 0xff00_0000);
    draw.fill([x + 1.0, y + 1.0, w - 2.0, h - 2.0], 0xff37_3737);
}

fn centered(draw: &mut dyn DrawContext, text: &str, center_x: f32, y: f32, color: u32, shadow: bool) {
    let width = draw.text_width(text);
    draw.text(text, [(center_x - width / 2.0).round(), y], color, shadow);
}

/// A player's head: the API gives mods no skins, so a face in colours of the player's own.
pub fn head(draw: &mut dyn DrawContext, uuid: u128, [x, y]: [f32; 2], size: f32) {
    let p = size / 8.0;
    let hash = (uuid ^ (uuid >> 64)) as u64;
    let hair = [0xff3b_2a1a, 0xff6a_4a2a, 0xffc8_a050, 0xff20_2020, 0xff8a_3a1a][(hash % 5) as usize];
    let skin = [0xffe0_b090, 0xffc0_8a60, 0xff8a_5a3a][((hash >> 8) % 3) as usize];
    let eyes = [0xff30_60c0, 0xff3a_8a3a, 0xff5a_3a1a][((hash >> 16) % 3) as usize];
    draw.fill([x, y, size, size], skin);
    draw.fill([x, y, size, 2.0 * p], hair);
    draw.fill([x, y + 2.0 * p, p, p], hair);
    draw.fill([x + 7.0 * p, y + 2.0 * p, p, p], hair);
    draw.fill([x + p, y + 4.0 * p, p, p], WHITE);
    draw.fill([x + 2.0 * p, y + 4.0 * p, p, p], eyes);
    draw.fill([x + 5.0 * p, y + 4.0 * p, p, p], eyes);
    draw.fill([x + 6.0 * p, y + 4.0 * p, p, p], WHITE);
    draw.fill([x + 3.0 * p, y + 6.0 * p, 2.0 * p, p], 0xff6a_3a2a);
}

/// The green frame around the head of a player who is talking.
pub fn talk_outline(draw: &mut dyn DrawContext, [x, y]: [f32; 2], size: f32) {
    icons::draw(draw, &icons::TALK, [x, y], size * 1.6);
}

impl Ui {
    fn hit(&mut self, rect: [f32; 4], hit: Hit, tooltip: Option<String>) {
        self.hits.push((rect, hit, tooltip));
    }

    /// A game button; inactive buttons take no clicks.
    fn button(
        &mut self,
        draw: &mut dyn DrawContext,
        mouse: [f32; 2],
        rect: [f32; 4],
        label: &str,
        active: bool,
        hit: Hit,
    ) {
        let _ = draw.widget_button(rect, &Text::literal(label), active, active && over(rect, mouse));
        if active {
            self.hit(rect, hit, None);
        }
    }

    /// A 20 by 20 button with an icon, crossed out when `on`, greyed when not `active`.
    #[allow(clippy::too_many_arguments)]
    fn icon_button(
        &mut self,
        draw: &mut dyn DrawContext,
        mouse: [f32; 2],
        [x, y]: [f32; 2],
        icon: &Icon,
        on: bool,
        active: bool,
        hit: Hit,
        tooltip: String,
    ) {
        let rect = [x, y, 20.0, 20.0];
        let _ = draw.widget_button(rect, &Text::literal(""), active, active && over(rect, mouse));
        let white = if active { WHITE } else { 0xff90_9090 };
        icons::draw_tinted(draw, icon, [x + 2.0, y + 2.0], 16.0, white);
        if on {
            icons::draw(draw, &icons::SLASH, [x + 2.0, y + 2.0], 16.0);
        }
        self.hit(rect, if active { hit } else { Hit::Nothing }, Some(tooltip));
    }

    /// The game's slider: `value` from 0 to 1, `label` on it.
    fn slider(
        &mut self,
        draw: &mut dyn DrawContext,
        mouse: [f32; 2],
        rect: [f32; 4],
        value: f32,
        label: &str,
        slider: Slider,
    ) {
        let hovered = over(rect, mouse) || self.dragging.is_some_and(|(_, s)| s == slider);
        draw.gui_nine_slice("widget/slider", rect, WHITE);
        let handle = [rect[0] + value.clamp(0.0, 1.0) * (rect[2] - 8.0), rect[1], 8.0, rect[3]];
        let sprite = if hovered {
            "widget/slider_handle_highlighted"
        } else {
            "widget/slider_handle"
        };
        draw.gui_nine_slice(sprite, handle, WHITE);
        centered(draw, label, rect[0] + rect[2] / 2.0, rect[1] + (rect[3] - 8.0) / 2.0, WHITE, true);
        self.hit(rect, Hit::Slider(slider), None);
    }

    /// Makes the text fields of `page` when it was just opened.
    fn ensure_fields(&mut self, page: Page) {
        if self.fields_page == Some(page) {
            return;
        }
        self.fields = Widgets::new();
        self.field_ids.clear();
        let count = match page {
            Page::CreateGroup => 2,
            Page::Password(_) => 1,
            _ => 0,
        };
        for index in 0..count {
            // Name, then password.
            let max = if page == Page::CreateGroup && index == 0 {
                protocol::MAX_GROUP_NAME
            } else {
                protocol::MAX_GROUP_PASSWORD
            };
            self.field_ids.push(self.fields.add_text_field(TextField::new([0.0; 4], max)));
        }
        self.fields_page = Some(page);
    }

    fn field(&self, index: usize) -> String {
        self.field_ids
            .get(index)
            .and_then(|id| self.fields.text_field(*id))
            .map(|field| field.value().to_owned())
            .unwrap_or_default()
    }

    fn place_field(&mut self, index: usize, rect: [f32; 4]) {
        if let Some(field) = self.field_ids.get(index).and_then(|id| self.fields.text_field_mut(*id)) {
            field.rect = rect;
        }
    }

    pub fn show(&mut self, page: Page) {
        if self.page != Some(page) {
            self.scroll = 0.0;
            self.wrong_password = false;
            self.dragging = None;
            if page == Page::CreateGroup {
                self.group_type = GroupType::Normal;
            }
        }
        self.page = Some(page);
    }

    pub fn closed(&mut self) {
        self.page = None;
        self.fields_page = None;
        self.dragging = None;
        self.hits.clear();
    }
}

/// The height of a list page's list for a GUI `height` pixels high, in units.
fn units(height: f32, footer: f32, extra: f32) -> f32 {
    let min = ((ROW + 4.0 + extra) / UNIT).ceil();
    ((height - HEADER - footer - 64.0) / UNIT).floor().max(min)
}

impl Client {
    fn own_uuid(&self) -> Option<u128> {
        self.secret.as_ref().map(|secret| secret.player)
    }

    pub(crate) fn groups_enabled(&self) -> bool {
        self.secret.as_ref().is_some_and(|secret| secret.groups_enabled)
    }

    /// The members of `group`, ourselves first.
    pub(crate) fn members(&self, group: u128) -> Vec<(u128, String, bool)> {
        let own = self.own_uuid();
        let mut members: Vec<(u128, String, bool)> = self
            .states
            .values()
            .filter(|state| state.group == Some(group))
            .map(|state| (state.uuid, state.name.clone(), state.disabled || state.force_disabled))
            .collect();
        if let Some(own) = own {
            if self.own_group == Some(group) && !members.iter().any(|(uuid, ..)| *uuid == own) {
                let name = self.states.get(&own).map(|s| s.name.clone()).unwrap_or_default();
                members.push((own, name, self.settings.disabled));
            }
        }
        members.sort_by(|a, b| (Some(a.0) != own, a.1.to_lowercase()).cmp(&(Some(b.0) != own, b.1.to_lowercase())));
        members
    }

    fn visible_groups(&self) -> Vec<&ClientGroup> {
        let mut groups: Vec<&ClientGroup> = self
            .groups
            .values()
            .filter(|group| !group.hidden || self.own_group == Some(group.id))
            .collect();
        groups.sort_by_key(|group| group.name.to_lowercase());
        groups
    }

    fn group_type_name(host: &dyn ClientHost, kind: GroupType) -> String {
        tr(
            host,
            match kind {
                GroupType::Normal => "group_type.normal",
                GroupType::Open => "group_type.open",
                GroupType::Isolated => "group_type.isolated",
            },
            "",
        )
    }

    /// Draws the open page over the world.
    pub(crate) fn draw_screen(&mut self, host: &dyn ClientHost, draw: &mut dyn DrawContext, mouse: [f32; 2]) {
        let Some(page) = self.ui.page else {
            return;
        };
        self.ui.hits.clear();
        self.ui.ensure_fields(page);
        let [width, height] = draw.size();
        draw.draw_menu_background([0.0, 0.0, width, height], true);
        match page {
            Page::Main => self.draw_main(host, draw, mouse, width, height),
            Page::Settings => self.draw_settings(host, draw, mouse, width, height),
            Page::CreateGroup => self.draw_create(host, draw, mouse, width, height),
            Page::Password(group) => self.draw_password(host, draw, mouse, width, height, group),
            Page::Volumes | Page::Devices(_) | Page::JoinGroup | Page::Group => {
                self.draw_list_page(host, draw, mouse, width, height, page)
            }
        }
        let _ = self.ui.fields.draw(draw, host, mouse);
        // The tooltip of what the pointer is over, on top of everything.
        if let Some(tooltip) = self
            .ui
            .hits
            .iter()
            .rev()
            .find(|(rect, _, tooltip)| tooltip.is_some() && over(*rect, mouse))
            .and_then(|(_, _, tooltip)| tooltip.clone())
        {
            let lines: Vec<Text> = tooltip.lines().map(Text::literal).collect();
            let _ = draw.gui_tooltip(&lines, mouse);
        }
    }

    fn draw_main(&mut self, host: &dyn ClientHost, draw: &mut dyn DrawContext, mouse: [f32; 2], width: f32, height: f32) {
        let (w, h) = (195.0, 76.0);
        let (left, top) = (((width - w) / 2.0).floor(), ((height - h) / 2.0).floor());
        panel(draw, [left, top, w, h]);
        centered(draw, &tr(host, "title", ""), left + w / 2.0, top + 7.0, FONT_COLOR, false);
        let ui = &mut self.ui;
        ui.button(draw, mouse, [left + 6.0, top + 21.0, 75.0, 20.0], &tr(host, "settings", ""), true, Hit::Page(Page::Settings));
        let group_page = if self.own_group.is_some() { Page::Group } else { Page::JoinGroup };
        let groups = self.secret.as_ref().is_some_and(|secret| secret.groups_enabled);
        ui.button(draw, mouse, [left + w - 6.0 - 75.0 + 1.0, top + 21.0, 75.0, 20.0], &tr(host, "group", ""), groups, Hit::Page(group_page));
        let y = top + h - 6.0 - 20.0;
        let settings = &self.settings;
        let mute_tip = tr(host, if settings.muted { "mute.on" } else { "mute.off" }, "");
        ui.icon_button(draw, mouse, [left + 6.0, y], &icons::MICROPHONE, settings.muted, !settings.disabled, Hit::Mute, mute_tip);
        let disable_tip = tr(host, if settings.disabled { "disable.on" } else { "disable.off" }, "");
        ui.icon_button(draw, mouse, [left + 28.0, y], &icons::SPEAKER, settings.disabled, true, Hit::Disable, disable_tip);
        ui.icon_button(draw, mouse, [left + 50.0, y], &icons::VOLUMES, false, true, Hit::Page(Page::Volumes), tr(host, "adjust_volumes", ""));
        let hide_tip = tr(host, if settings.hide_icons { "hide_icons.on" } else { "hide_icons.off" }, "");
        ui.icon_button(draw, mouse, [left + w - 6.0 - 20.0, y], &icons::EYE, settings.hide_icons, true, Hit::HideIcons, hide_tip);
    }

    fn draw_settings(&mut self, host: &dyn ClientHost, draw: &mut dyn DrawContext, mouse: [f32; 2], width: f32, height: f32) {
        let (w, h) = (248.0, 153.0);
        let (left, top) = (((width - w) / 2.0).floor(), ((height - h) / 2.0).floor());
        panel(draw, [left, top, w, h]);
        centered(draw, &tr(host, "settings_title", ""), left + w / 2.0, top + 7.0, FONT_COLOR, false);
        let full = w - 20.0;
        let x = left + 10.0;
        let mut y = top + 20.0;
        let settings = self.settings.clone();
        let ui = &mut self.ui;
        ui.slider(draw, mouse, [x, y, full, 20.0], settings.voice_volume / 2.0, &tr(host, "voice_volume", &percent(settings.voice_volume)), Slider::VoiceVolume);
        y += 21.0;
        ui.slider(draw, mouse, [x, y, full, 20.0], settings.microphone_amplification / 3.0, &tr(host, "mic_amplification", &percent(settings.microphone_amplification)), Slider::MicAmplification);
        y += 21.0;
        let testing = self.audio.as_ref().is_some_and(|audio| audio.controls.monitor.load(Ordering::Relaxed));
        let has_audio = self.audio.is_some();
        let test_tip = tr(host, if testing { "mic_test.on" } else { "mic_test.off" }, "");
        ui.icon_button(draw, mouse, [x, y], &icons::MICROPHONE, false, has_audio, Hit::MicTest, test_tip);
        if testing {
            // A green frame while the test plays the microphone back.
            draw.fill([x, y, 20.0, 1.0], 0xff3c_cb3c);
            draw.fill([x, y + 19.0, 20.0, 1.0], 0xff3c_cb3c);
            draw.fill([x, y, 1.0, 20.0], 0xff3c_cb3c);
            draw.fill([x + 19.0, y, 1.0, 20.0], 0xff3c_cb3c);
        }
        let activation = match settings.activation {
            Activation::PushToTalk => tr(host, "activation.ptt", ""),
            Activation::Voice => tr(host, "activation.voice", ""),
        };
        ui.button(draw, mouse, [x + 21.0, y, full - 21.0, 20.0], &tr(host, "activation", &activation), true, Hit::Activation);
        y += 21.0;
        match settings.activation {
            Activation::PushToTalk => {
                let key = host
                    .mod_key(MOD_ID, "push_to_talk")
                    .map(|(key, _)| host.translate(&key).unwrap_or_else(|| key_name(&key)))
                    .unwrap_or_default();
                let rect = [x, y, full, 20.0];
                let _ = draw.widget_button(rect, &Text::literal(tr(host, "ptt_key", &key)), true, over(rect, mouse));
                ui.hit(rect, Hit::Nothing, Some(tr(host, "ptt_key.tooltip", "")));
            }
            Activation::Voice => {
                let rect = [x, y, full, 20.0];
                // -100 dB at the left, 0 dB at the right; the microphone's level shows beneath.
                let value = (settings.threshold_db + 100.0) / 100.0;
                draw.gui_nine_slice("widget/slider", rect, WHITE);
                if let Some(audio) = &self.audio {
                    let level = f32::from_bits(audio.controls.level_db.load(Ordering::Relaxed));
                    let fraction = ((level + 100.0) / 100.0).clamp(0.0, 1.0);
                    let color = if level >= settings.threshold_db { 0xc03c_cb3c } else { 0xc0e0_c030 };
                    draw.fill([x + 1.0, y + 1.0, (full - 2.0) * fraction, 18.0], color);
                }
                let label = tr(host, "threshold", &format!("{:.0} dB", settings.threshold_db));
                let handle = [x + value.clamp(0.0, 1.0) * (full - 8.0), y, 8.0, 20.0];
                let hovered = over(rect, mouse) || ui.dragging.is_some_and(|(_, s)| s == Slider::Threshold);
                draw.gui_nine_slice(if hovered { "widget/slider_handle_highlighted" } else { "widget/slider_handle" }, handle, WHITE);
                centered(draw, &label, x + full / 2.0, y + 6.0, WHITE, true);
                ui.hit(rect, Hit::Slider(Slider::Threshold), Some(tr(host, "threshold.tooltip", "")));
            }
        }
        y += 21.0;
        let stereo = tr(host, if settings.stereo { "audio_type.stereo" } else { "audio_type.mono" }, "");
        ui.button(draw, mouse, [x, y, full, 20.0], &tr(host, "audio_type", &stereo), true, Hit::Stereo);
        y += 21.0;
        let half = (full / 2.0).floor() - 1.0;
        ui.button(draw, mouse, [x, y, half, 20.0], &tr(host, "select_microphone", ""), true, Hit::Page(Page::Devices(true)));
        ui.button(draw, mouse, [left + w / 2.0 + 1.0, y, half, 20.0], &tr(host, "select_speaker", ""), true, Hit::Page(Page::Devices(false)));
    }

    fn draw_create(&mut self, host: &dyn ClientHost, draw: &mut dyn DrawContext, mouse: [f32; 2], width: f32, height: f32) {
        let (w, h) = (195.0, 124.0);
        let (left, top) = (((width - w) / 2.0).floor(), ((height - h) / 2.0).floor());
        panel(draw, [left, top, w, h]);
        centered(draw, &tr(host, "create_group", ""), left + w / 2.0, top + 7.0, FONT_COLOR, false);
        draw.text(&tr(host, "group_name", ""), [left + 8.0, top + 21.0], FONT_COLOR, false);
        draw.text(&tr(host, "optional_password", ""), [left + 8.0, top + 47.0], FONT_COLOR, false);
        self.ui.place_field(0, [left + 7.0, top + 30.0, w - 14.0, 14.0]);
        self.ui.place_field(1, [left + 7.0, top + 56.0, w - 14.0, 14.0]);
        let kind = Self::group_type_name(host, self.ui.group_type);
        let tip = tr(host, match self.ui.group_type {
            GroupType::Normal => "group_type.normal.tooltip",
            GroupType::Open => "group_type.open.tooltip",
            GroupType::Isolated => "group_type.isolated.tooltip",
        }, "");
        let rect = [left + 6.0, top + 74.0, w - 12.0, 20.0];
        self.ui.button(draw, mouse, rect, &tr(host, "group_type", &kind), true, Hit::CycleType);
        self.ui.hit(rect, Hit::Nothing, Some(tip));
        let name = self.ui.field(0);
        let can_create = protocol::valid_group_name(&name);
        self.ui.button(draw, mouse, [left + 6.0, top + h - 27.0, w - 12.0, 20.0], &tr(host, "create", ""), can_create, Hit::Create);
    }

    fn draw_password(&mut self, host: &dyn ClientHost, draw: &mut dyn DrawContext, mouse: [f32; 2], width: f32, height: f32, group: u128) {
        let (w, h) = (195.0, 74.0);
        let (left, top) = (((width - w) / 2.0).floor(), ((height - h) / 2.0).floor());
        panel(draw, [left, top, w, h]);
        let name = self.groups.get(&group).map(|g| g.name.clone()).unwrap_or_default();
        centered(draw, &tr(host, "enter_password", &name), left + w / 2.0, top + 7.0, FONT_COLOR, false);
        draw.text(&tr(host, "password", ""), [left + 8.0, top + 21.0], FONT_COLOR, false);
        if self.ui.wrong_password {
            let text = tr(host, "wrong_password", "");
            let width = draw.text_width(&text);
            draw.text(&text, [left + w - 8.0 - width, top + 21.0], 0xffc0_2020, false);
        }
        self.ui.place_field(0, [left + 7.0, top + 30.0, w - 14.0, 14.0]);
        let can_join = !self.ui.field(0).is_empty();
        self.ui.button(draw, mouse, [left + 7.0, top + h - 27.0, w - 14.0, 20.0], &tr(host, "join_group", ""), can_join, Hit::SubmitPassword);
    }

    /// The pages with a scrolling list: volumes, devices, joining a group, the group.
    fn draw_list_page(&mut self, host: &dyn ClientHost, draw: &mut dyn DrawContext, mouse: [f32; 2], width: f32, height: f32, page: Page) {
        let footer = match page {
            Page::Volumes => 8.0,
            _ => 32.0,
        };
        let units = units(height, footer, 0.0);
        let w = LIST_WIDTH;
        let h = HEADER + units * UNIT + footer;
        let left = ((width - w) / 2.0).floor() + 2.0;
        let top = 32.0;
        panel(draw, [left, top, w, h]);
        let list = [left + 7.0, top + HEADER - 1.0, w - 14.0, units * UNIT + 2.0];
        inset(draw, list);
        self.ui.list = list;
        let title = match page {
            Page::Volumes => tr(host, "volumes_title", ""),
            Page::Devices(true) => tr(host, "select_microphone", ""),
            Page::Devices(false) => tr(host, "select_speaker", ""),
            Page::JoinGroup => tr(host, "join_create_group", ""),
            _ => {
                let group = self.own_group.and_then(|id| self.groups.get(&id));
                match group {
                    Some(group) if group.kind == GroupType::Normal => tr(host, "group_title", &group.name),
                    Some(group) => format!("{} ({})", tr(host, "group_title", &group.name), Self::group_type_name(host, group.kind)),
                    None => tr(host, "group", ""),
                }
            }
        };
        centered(draw, &title, left + w / 2.0, top + 5.0, FONT_COLOR, false);

        // The rows, 32 high with 4 between, inside the dark area.
        let rows = self.rows(page);
        let inner = [list[0] + 1.0, list[1] + 1.0, list[2] - 2.0, list[3] - 2.0];
        let content = rows.len() as f32 * ROW;
        self.ui.max_scroll = (content - inner[3] + 4.0).max(0.0);
        self.ui.scroll = self.ui.scroll.clamp(0.0, self.ui.max_scroll);
        if rows.is_empty() {
            let empty = match page {
                Page::JoinGroup => tr(host, "no_groups", ""),
                Page::Devices(_) => tr(host, "no_devices", ""),
                _ => tr(host, "no_players", ""),
            };
            centered(draw, &empty, left + w / 2.0, inner[1] + inner[3] / 2.0 - 4.0, GRAY, true);
        }
        let _ = draw.push_clip(inner);
        let talking = self.talking.clone();
        let own = self.own_uuid();
        for (index, row) in rows.into_iter().enumerate() {
            let y = inner[1] + 2.0 + index as f32 * ROW - self.ui.scroll;
            if y + ROW < inner[1] || y > inner[1] + inner[3] {
                continue;
            }
            let rect = [inner[0] + 2.0, y, inner[2] - 4.0 - if self.ui.max_scroll > 0.0 { 6.0 } else { 0.0 }, ROW - 4.0];
            let hovered = over(rect, mouse) && over(inner, mouse);
            self.draw_row(host, draw, mouse, rect, hovered, row, &talking, own);
        }
        let _ = draw.pop_clip();
        // Hits of rows scrolled out of the list do not count.
        self.ui.hits.retain(|(rect, hit, _)| !matches!(hit, Hit::Join(_) | Hit::Device(_) | Hit::Slider(Slider::Player(_))) || {
            let [x, y, _, h] = *rect;
            y + h > inner[1] && y < inner[1] + inner[3] && x < inner[0] + inner[2]
        });
        if self.ui.max_scroll > 0.0 {
            // The scroll bar at the list's right edge.
            let bar_h = (inner[3] * inner[3] / content).max(32.0).min(inner[3]);
            let bar_y = inner[1] + (inner[3] - bar_h) * self.ui.scroll / self.ui.max_scroll;
            draw.fill([inner[0] + inner[2] - 6.0, inner[1], 6.0, inner[3]], 0xff00_0000);
            draw.fill([inner[0] + inner[2] - 6.0, bar_y, 6.0, bar_h], 0xff80_8080);
            draw.fill([inner[0] + inner[2] - 6.0, bar_y, 5.0, bar_h - 1.0], 0xffc0_c0c0);
        }

        let y = top + h - 20.0 - 7.0;
        let settings = self.settings.clone();
        let ui = &mut self.ui;
        match page {
            Page::JoinGroup => ui.button(draw, mouse, [left + 7.0, y, w - 14.0, 20.0], &tr(host, "create_group_button", ""), true, Hit::Page(Page::CreateGroup)),
            Page::Devices(_) => ui.button(draw, mouse, [left + 7.0, y, w - 14.0, 20.0], &tr(host, "back", ""), true, Hit::Page(Page::Settings)),
            Page::Group => {
                let mute_tip = tr(host, if settings.muted { "mute.on" } else { "mute.off" }, "");
                ui.icon_button(draw, mouse, [left + 7.0, y], &icons::MICROPHONE, settings.muted, !settings.disabled, Hit::Mute, mute_tip);
                let disable_tip = tr(host, if settings.disabled { "disable.on" } else { "disable.off" }, "");
                ui.icon_button(draw, mouse, [left + 30.0, y], &icons::SPEAKER, settings.disabled, true, Hit::Disable, disable_tip);
                let hud_tip = tr(host, if settings.show_group_hud { "group_hud.on" } else { "group_hud.off" }, "");
                ui.icon_button(draw, mouse, [left + 53.0, y], &icons::GROUP, !settings.show_group_hud, !settings.hide_icons, Hit::GroupHud, hud_tip);
                ui.icon_button(draw, mouse, [left + w - 20.0 - 7.0, y], &icons::LEAVE, false, true, Hit::Leave, tr(host, "leave_group", ""));
            }
            _ => {}
        }
    }

    fn rows(&self, page: Page) -> Vec<Row> {
        let own = self.own_uuid();
        match page {
            Page::Volumes => {
                let mut players: Vec<Row> = self
                    .states
                    .values()
                    .filter(|state| Some(state.uuid) != own)
                    .map(|state| Row::Player(state.uuid, state.name.clone()))
                    .collect();
                players.sort_by_key(|row| match row {
                    Row::Player(_, name) => name.to_lowercase(),
                    _ => String::new(),
                });
                players
            }
            Page::Devices(input) => {
                let chosen = if input { &self.settings.microphone } else { &self.settings.speaker };
                std::iter::once(Row::Device(None, chosen.is_none()))
                    .chain(self.ui.devices.iter().map(|name| Row::Device(Some(name.clone()), chosen.as_deref() == Some(name.as_str()))))
                    .collect()
            }
            Page::JoinGroup => self
                .visible_groups()
                .into_iter()
                .map(|group| Row::Group(group.clone(), self.members(group.id)))
                .collect(),
            Page::Group => self
                .own_group
                .map(|group| self.members(group).into_iter().map(|(uuid, name, disabled)| Row::Member(uuid, name, disabled)).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_row(
        &mut self,
        host: &dyn ClientHost,
        draw: &mut dyn DrawContext,
        mouse: [f32; 2],
        rect: [f32; 4],
        hovered: bool,
        row: Row,
        talking: &[u128],
        own: Option<u128>,
    ) {
        let [x, y, w, h] = rect;
        let pad = 4.0;
        match row {
            Row::Player(uuid, name) => {
                draw.fill(rect, ROW_FILL);
                head(draw, uuid, [x + pad, y + (h - 24.0) / 2.0], 24.0);
                draw.text(&name, [x + pad + 24.0 + pad, y + (h - 8.0) / 2.0], WHITE, false);
                let volume = self.settings.player_volumes.get(&uuid).copied().unwrap_or(1.0);
                let slider = [x + w - 90.0 - pad, y + (h - 20.0) / 2.0, 90.0, 20.0];
                self.ui.slider(draw, mouse, slider, volume / 4.0, &tr(host, "player_volume", &percent(volume)), Slider::Player(uuid));
            }
            Row::Member(uuid, name, disabled) => {
                draw.fill(rect, ROW_FILL);
                let size = h - pad * 2.0;
                let at = [x + pad, y + pad];
                if !disabled && talking.contains(&uuid) {
                    icons::draw(draw, &icons::TALK, at, size * 1.6);
                }
                let face = size * 0.8;
                head(draw, uuid, [at[0] + size * 0.1, at[1] + size * 0.1], face);
                if disabled {
                    icons::draw(draw, &icons::SPEAKER, [at[0] + size * 0.1, at[1] + size * 0.1], face / 2.0);
                    icons::draw(draw, &icons::SLASH, [at[0] + size * 0.1, at[1] + size * 0.1], face / 2.0);
                }
                draw.text(&name, [x + pad + size + pad, y + (h - 8.0) / 2.0], WHITE, false);
                if hovered && Some(uuid) != own {
                    let width = (w - (pad + size + pad + draw.text_width(&name) + pad * 2.0)).min(100.0);
                    let volume = self.settings.player_volumes.get(&uuid).copied().unwrap_or(1.0);
                    let slider = [x + w - width - pad, y + (h - 20.0) / 2.0, width, 20.0];
                    self.ui.slider(draw, mouse, slider, volume / 4.0, &tr(host, "player_volume", &percent(volume)), Slider::Player(uuid));
                }
            }
            Row::Group(group, members) => {
                draw.fill(rect, if hovered { ROW_FILL_HOVERED } else { ROW_FILL });
                let mut text_x = x + pad;
                if group.has_password {
                    icons::draw(draw, &icons::LOCK, [x + pad, y + h / 2.0 - 8.0], 16.0);
                    text_x += 16.0 + pad;
                }
                draw.text(&group.name, [text_x, y + (h - 8.0) / 2.0], WHITE, false);
                // Members' heads from the right, two rows.
                let skin = 12.0;
                let text_end = text_x + draw.text_width(&group.name) + pad;
                let per_row = ((x + w - pad - text_end) / (skin + 1.0)).floor().max(0.0) as usize;
                for (index, (uuid, ..)) in members.iter().enumerate().take(per_row * 2) {
                    let column = (index / 2) as f32;
                    let line = (index % 2) as f32;
                    let hx = x + w - skin - pad - column * (skin + 1.0);
                    let hy = y + h / 2.0 - 13.0 + 13.0 * line;
                    head(draw, *uuid, [hx, hy], skin);
                }
                let mut tooltip = if group.kind == GroupType::Normal {
                    tr(host, "group_title", &group.name)
                } else {
                    format!("{} ({})", tr(host, "group_title", &group.name), Self::group_type_name(host, group.kind))
                };
                if members.is_empty() {
                    tooltip.push('\n');
                    tooltip.push_str(&tr(host, "no_group_members", ""));
                } else {
                    tooltip.push('\n');
                    tooltip.push_str(&tr(host, "group_members", ""));
                    for (_, name, _) in members.iter().take(10) {
                        tooltip.push_str("\n  ");
                        tooltip.push_str(name);
                    }
                    if members.len() > 10 {
                        tooltip.push('\n');
                        tooltip.push_str(&tr(host, "more_members", &(members.len() - 10).to_string()));
                    }
                }
                self.ui.hit(rect, Hit::Join(group.id), Some(tooltip));
            }
            Row::Device(name, selected) => {
                draw.fill(rect, if hovered { ROW_FILL_HOVERED } else { ROW_FILL });
                let label = name.clone().unwrap_or_else(|| tr(host, "default_device", ""));
                let icon = if self.ui.page == Some(Page::Devices(true)) { &icons::MICROPHONE } else { &icons::SPEAKER };
                icons::draw(draw, icon, [x + pad, y + h / 2.0 - 8.0], 16.0);
                draw.text(&label, [x + pad + 16.0 + pad, y + (h - 8.0) / 2.0], WHITE, false);
                if selected {
                    draw.fill([x + w - pad - 10.0, y + h / 2.0 - 5.0, 10.0, 10.0], 0xff20_2020);
                    draw.fill([x + w - pad - 9.0, y + h / 2.0 - 4.0, 8.0, 8.0], 0xff3c_cb3c);
                }
                self.ui.hit(rect, Hit::Device(name), None);
            }
        }
    }

    // ---- input ----

    /// A press on the open page; true when the page took it.
    pub(crate) fn screen_pressed(&mut self, host: &dyn ClientHost, at: [f32; 2], button: i32, modifiers: u32) -> bool {
        let Some(page) = self.ui.page else {
            return false;
        };
        if button == 0 {
            if let Some((rect, hit, _)) = self.ui.hits.iter().rev().find(|(rect, hit, _)| over(*rect, at) && *hit != Hit::Nothing).cloned() {
                if !matches!(hit, Hit::Slider(_) | Hit::Join(_) | Hit::Device(_)) {
                    host.play_local_sound(&SoundPlay::ui("minecraft:ui.button.click"), 0);
                }
                self.ui.fields.set_focus(host, None);
                self.on_hit(host, page, hit, rect, at);
                return true;
            }
        }
        let outcome = self.ui.fields.mouse_pressed(host, at, button, modifiers);
        outcome.handled() || button == 0
    }

    pub(crate) fn screen_released(&mut self, button: i32) -> bool {
        if self.ui.page.is_none() {
            return false;
        }
        if button == 0 && self.ui.dragging.take().is_some() {
            self.save_settings_later();
            return true;
        }
        self.ui.fields.mouse_released(button).handled()
    }

    pub(crate) fn screen_dragged(&mut self, host: &dyn ClientHost, at: [f32; 2], button: i32) -> bool {
        if self.ui.page.is_none() {
            return false;
        }
        if let Some((rect, slider)) = self.ui.dragging {
            self.set_slider(slider, rect, at[0]);
            return true;
        }
        self.ui.fields.mouse_dragged(host, at, button).handled()
    }

    pub(crate) fn screen_scrolled(&mut self, at: [f32; 2], amount: f64) -> bool {
        if self.ui.page.is_none() || !over(self.ui.list, at) {
            return false;
        }
        self.ui.scroll = (self.ui.scroll - amount as f32 * ROW / 2.0).clamp(0.0, self.ui.max_scroll);
        true
    }

    pub(crate) fn screen_key(&mut self, host: &dyn ClientHost, key: &str, modifiers: u32) -> bool {
        let Some(page) = self.ui.page else {
            return false;
        };
        match self.ui.fields.key_pressed(host, key, modifiers) {
            Outcome::Submitted(_) => {
                match page {
                    Page::CreateGroup => self.create_group(),
                    Page::Password(group) => self.submit_password(group),
                    _ => {}
                }
                return true;
            }
            Outcome::Ignored => {}
            _ => return true,
        }
        // The menu's own key closes it again, as the group key does its menus.
        let bound = |id: &str| host.mod_key(MOD_ID, id).is_some_and(|(bound, _)| bound == key);
        let closes = match page {
            Page::Main => bound("menu"),
            Page::JoinGroup | Page::Group => bound("groups"),
            _ => false,
        };
        if closes {
            self.ui.actions.push(Action::Close);
            return true;
        }
        false
    }

    pub(crate) fn screen_char(&mut self, host: &dyn ClientHost, c: char) -> bool {
        self.ui.page.is_some() && self.ui.fields.char_typed(host, c).handled()
    }

    fn set_slider(&mut self, slider: Slider, rect: [f32; 4], x: f32) {
        let value = ((x - rect[0] - 4.0) / (rect[2] - 8.0)).clamp(0.0, 1.0);
        let settings = &mut self.settings;
        match slider {
            Slider::VoiceVolume => settings.voice_volume = (value * 40.0).round() / 20.0,
            Slider::MicAmplification => settings.microphone_amplification = (value * 60.0).round() / 20.0,
            Slider::Threshold => settings.threshold_db = (value * 100.0 - 100.0).round(),
            Slider::Player(uuid) => {
                let volume = (value * 80.0).round() / 20.0;
                if (volume - 1.0).abs() < f32::EPSILON {
                    settings.player_volumes.remove(&uuid);
                } else {
                    settings.player_volumes.insert(uuid, volume);
                }
            }
        }
    }

    fn create_group(&mut self) {
        let name = self.ui.field(0);
        let password = self.ui.field(1);
        if !protocol::valid_group_name(&name) {
            return;
        }
        let password = Some(password.as_str()).filter(|p| protocol::valid_group_password(p));
        self.ui.actions.push(Action::Send(
            channel::CREATE_GROUP,
            protocol::encode_create_group(&name, password, self.ui.group_type),
        ));
    }

    fn submit_password(&mut self, group: u128) {
        let password = self.ui.field(0);
        if password.is_empty() {
            return;
        }
        self.ui.wrong_password = false;
        self.ui.actions.push(Action::Send(channel::SET_GROUP, protocol::encode_set_group(group, Some(&password))));
    }

    fn on_hit(&mut self, host: &dyn ClientHost, page: Page, hit: Hit, rect: [f32; 4], at: [f32; 2]) {
        let _ = host;
        match hit {
            Hit::Page(target) => {
                if let Page::Devices(input) = target {
                    self.ui.devices = crate::audio::device_names(input);
                }
                if page == Page::Settings {
                    self.stop_mic_test();
                }
                self.ui.show(target);
            }
            Hit::Mute => {
                self.settings.muted = !self.settings.muted;
                self.ui.actions.push(Action::SendState);
                self.save_settings_later();
            }
            Hit::Disable => {
                self.settings.disabled = !self.settings.disabled;
                self.ui.actions.push(Action::SendState);
                self.save_settings_later();
            }
            Hit::HideIcons => {
                self.settings.hide_icons = !self.settings.hide_icons;
                self.save_settings_later();
            }
            Hit::GroupHud => {
                self.settings.show_group_hud = !self.settings.show_group_hud;
                self.save_settings_later();
            }
            Hit::Leave => self.ui.actions.push(Action::Send(channel::LEAVE_GROUP, Vec::new())),
            Hit::Create => self.create_group(),
            Hit::CycleType => {
                let all = GroupType::ALL;
                let index = all.iter().position(|kind| *kind == self.ui.group_type).unwrap_or(0);
                self.ui.group_type = all[(index + 1) % all.len()];
            }
            Hit::Join(group) => {
                let Some(info) = self.groups.get(&group) else {
                    return;
                };
                host.play_local_sound(&SoundPlay::ui("minecraft:ui.button.click"), 0);
                if info.has_password {
                    self.ui.show(Page::Password(group));
                } else {
                    self.ui.actions.push(Action::Send(channel::SET_GROUP, protocol::encode_set_group(group, None)));
                }
            }
            Hit::SubmitPassword => {
                if let Page::Password(group) = page {
                    self.submit_password(group);
                }
            }
            Hit::Slider(slider) => {
                self.ui.dragging = Some((rect, slider));
                self.set_slider(slider, rect, at[0]);
            }
            Hit::MicTest => {
                if let Some(audio) = &self.audio {
                    let on = !audio.controls.monitor.load(Ordering::Relaxed);
                    audio.controls.monitor.store(on, Ordering::Relaxed);
                }
            }
            Hit::Activation => {
                self.settings.activation = match self.settings.activation {
                    Activation::PushToTalk => Activation::Voice,
                    Activation::Voice => Activation::PushToTalk,
                };
                self.save_settings_later();
            }
            Hit::Stereo => {
                self.settings.stereo = !self.settings.stereo;
                self.save_settings_later();
            }
            Hit::Device(name) => {
                host.play_local_sound(&SoundPlay::ui("minecraft:ui.button.click"), 0);
                if let Page::Devices(input) = page {
                    if input {
                        self.settings.microphone = name;
                    } else {
                        self.settings.speaker = name;
                    }
                    self.save_settings_later();
                    self.ui.actions.push(Action::RestartAudio);
                }
            }
            Hit::Nothing => {}
        }
    }

    pub(crate) fn stop_mic_test(&self) {
        if let Some(audio) = &self.audio {
            audio.controls.monitor.store(false, Ordering::Relaxed);
        }
    }
}

/// One row of a list page.
enum Row {
    /// A player and their volume slider.
    Player(u128, String),
    /// A member of our group: uuid, name, voice chat off.
    Member(u128, String, bool),
    Group(ClientGroup, Vec<(u128, String, bool)>),
    /// A device by name (None: the system default), and whether it is the chosen one.
    Device(Option<String>, bool),
}

/// Draws every page into a list of what was drawn, for looking at the layout without the game:
/// `HV_PREVIEW=<folder> cargo test preview -- --ignored` writes one file per page.
#[cfg(test)]
mod preview {
    use super::*;
    use crate::protocol::{PlayerState, SecretMessage};
    use std::collections::HashMap;
    use mod_api::client::widgets::{TextFieldLayout, TextFieldView};
    use mod_api::prelude::{ClientInputClientHost, ConfigClientHost, EffectsClientHost};
    use mod_api::{ClientModifiers, SoundRef};
    use std::fmt::Write;

    struct Fake(HashMap<String, String>);
    impl mod_api::Host for Fake {}
    mod_api::impl_host_areas!(Fake);
    impl ClientHost for Fake {}
    mod_api::impl_client_host_areas!(Fake, except(client_input, effects, config));
    impl ClientInputClientHost for Fake {
        fn millis(&self) -> u64 {
            0
        }
        fn mod_key(&self, _: &str, _: &str) -> Option<(String, ClientModifiers)> {
            Some(("key.keyboard.caps.lock".into(), ClientModifiers::new(0)))
        }
    }
    impl EffectsClientHost for Fake {
        fn play_local_sound(&self, _: &SoundPlay, _: u32) -> Option<SoundRef> {
            None
        }
    }
    impl ConfigClientHost for Fake {
        fn translate(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
    }

    #[derive(Default)]
    struct Recorder(String);
    impl DrawContext for Recorder {
        fn size(&self) -> [f32; 2] {
            [427.0, 240.0]
        }
        fn fill(&mut self, [x, y, w, h]: [f32; 4], argb: u32) {
            let _ = writeln!(self.0, "fill\t{x}\t{y}\t{w}\t{h}\t{argb:08x}");
        }
        fn text(&mut self, text: &str, [x, y]: [f32; 2], argb: u32, shadow: bool) {
            let _ = writeln!(self.0, "text\t{x}\t{y}\t{argb:08x}\t{shadow}\t{text}");
        }
        fn text_width(&self, text: &str) -> f32 {
            text.chars().count() as f32 * 6.0
        }
        fn widget_button(&mut self, [x, y, w, h]: [f32; 4], label: &Text, active: bool, highlighted: bool) -> Result<(), mod_api::TextFormError> {
            let label = format!("{label:?}");
            let label = label.split("Literal(\"").nth(1).and_then(|rest| rest.split('"').next()).unwrap_or("");
            let _ = writeln!(self.0, "button\t{x}\t{y}\t{w}\t{h}\t{active}\t{highlighted}\t{label}");
            Ok(())
        }
        fn gui_nine_slice(&mut self, sprite: &str, [x, y, w, h]: [f32; 4], _: u32) {
            let _ = writeln!(self.0, "sprite\t{x}\t{y}\t{w}\t{h}\t{sprite}");
        }
        fn widget_text_field(&mut self, field: &TextFieldView) -> TextFieldLayout {
            let [x, y, w, h] = field.rect;
            let _ = writeln!(self.0, "field\t{x}\t{y}\t{w}\t{h}\t{}", field.value);
            TextFieldLayout::new(0, Vec::new())
        }
        fn gui_tooltip(&mut self, _: &[Text], _: [f32; 2]) -> Result<(), mod_api::TextFormError> {
            Ok(())
        }
        fn draw_menu_background(&mut self, [x, y, w, h]: [f32; 4], _: bool) {
            let _ = writeln!(self.0, "fill\t{x}\t{y}\t{w}\t{h}\t80101010");
        }
        fn push_clip(&mut self, _: [f32; 4]) -> Result<(), mod_api::GuiClipRefusal> {
            Ok(())
        }
        fn pop_clip(&mut self) -> Result<(), mod_api::GuiClipRefusal> {
            Ok(())
        }
    }

    fn lang(text: &str) -> HashMap<String, String> {
        text.lines()
            .filter_map(|line| {
                let line = line.trim().trim_end_matches(',');
                let (key, value) = line.split_once("\": \"")?;
                Some((key.trim_start_matches('"').to_owned(), value.trim_end_matches('"').replace("\\n", "\n")))
            })
            .collect()
    }

    #[test]
    #[ignore]
    fn preview() {
        let Ok(folder) = std::env::var("HV_PREVIEW") else {
            return;
        };
        let host = Fake(lang(include_str!("../assets/horizon_voice/lang/de_de.json")));
        let mut client = Client::default();
        client.secret = Some(SecretMessage {
            secret: crate::protocol::Secret([0; 16]),
            connection: 0,
            port: 24454,
            player: 1,
            codec: crate::protocol::Codec::Voip,
            mtu_size: 1024,
            distance: 48.0,
            keep_alive: 1000,
            groups_enabled: true,
            voice_host: String::new(),
            allow_recording: true,
        });
        for (uuid, name) in [(1u128, "Oscar"), (2, "Alex"), (3, "Steve"), (4, "Notch")] {
            let mut state = PlayerState::disconnected(uuid, name);
            state.disconnected = false;
            state.group = (uuid <= 3).then_some(10);
            client.states.insert(uuid, state);
        }
        client.groups.insert(10, ClientGroup { id: 10, name: "Höhlenforscher".into(), has_password: true, persistent: false, hidden: false, kind: GroupType::Open });
        client.groups.insert(11, ClientGroup { id: 11, name: "Bauteam".into(), has_password: false, persistent: false, hidden: false, kind: GroupType::Normal });
        client.talking = vec![2];
        client.settings.player_volumes.insert(3, 1.5);
        client.ui.devices = vec!["USB-Mikrofon".into(), "Headset".into()];
        let pages = [
            ("main", Page::Main),
            ("settings", Page::Settings),
            ("volumes", Page::Volumes),
            ("devices", Page::Devices(true)),
            ("join", Page::JoinGroup),
            ("create", Page::CreateGroup),
            ("password", Page::Password(10)),
        ];
        for (name, page) in pages {
            client.own_group = None;
            client.ui.show(page);
            let mut draw = Recorder::default();
            client.draw_screen(&host, &mut draw, [0.0, 0.0]);
            std::fs::write(format!("{folder}/{name}.tsv"), draw.0).unwrap();
        }
        client.own_group = Some(10);
        client.ui.show(Page::Group);
        let mut draw = Recorder::default();
        // The pointer over Alex's row shows the volume slider.
        client.draw_screen(&host, &mut draw, [200.0, 70.0]);
        std::fs::write(format!("{folder}/group.tsv"), draw.0).unwrap();
        client.update_hud(&host, true, false);
        let mut draw = Recorder::default();
        client.draw_hud(&mut draw);
        std::fs::write(format!("{folder}/hud.tsv"), draw.0).unwrap();
    }
}
