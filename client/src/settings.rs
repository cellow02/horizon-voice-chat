//! The player's voice chat settings, kept in `config/horizon_voice-client.properties` as
//! `key=value` lines. Everything the voice chat menu changes lives here.

use std::collections::HashMap;

/// The file in the game's `config/` folder.
pub const FILE: &str = "horizon_voice-client.properties";

/// How the microphone decides to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    PushToTalk,
    Voice,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Everyone's volume, 0 to 2.
    pub voice_volume: f32,
    /// Microphone gain, 0 to 3.
    pub microphone_amplification: f32,
    pub activation: Activation,
    /// Voice activation threshold in dB, -100 to 0.
    pub threshold_db: f32,
    /// Hear players left and right by where they stand.
    pub stereo: bool,
    /// Hide the voice chat icons on the screen.
    pub hide_icons: bool,
    /// Show the members of the group at the top left.
    pub show_group_hud: bool,
    pub muted: bool,
    pub disabled: bool,
    /// The microphone and speakers by name; None is the system default.
    pub microphone: Option<String>,
    pub speaker: Option<String>,
    /// Each player's own volume, 0 to 4.
    pub player_volumes: HashMap<u128, f32>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            voice_volume: 1.0,
            microphone_amplification: 1.0,
            activation: Activation::PushToTalk,
            threshold_db: -50.0,
            stereo: true,
            hide_icons: false,
            show_group_hud: true,
            muted: false,
            disabled: false,
            microphone: None,
            speaker: None,
            player_volumes: HashMap::new(),
        }
    }
}

fn number(value: &str, min: f32, max: f32) -> Option<f32> {
    value
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(min, max))
}

impl Settings {
    /// Reads the file's text; unknown keys and bad values keep their defaults.
    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            let flag = value == "true";
            match key {
                "voice_volume" => {
                    if let Some(v) = number(value, 0.0, 2.0) {
                        settings.voice_volume = v;
                    }
                }
                "microphone_amplification" => {
                    if let Some(v) = number(value, 0.0, 3.0) {
                        settings.microphone_amplification = v;
                    }
                }
                "activation" => {
                    settings.activation = if value == "voice" {
                        Activation::Voice
                    } else {
                        Activation::PushToTalk
                    }
                }
                "threshold" => {
                    if let Some(v) = number(value, -100.0, 0.0) {
                        settings.threshold_db = v;
                    }
                }
                "stereo" => settings.stereo = flag,
                "hide_icons" => settings.hide_icons = flag,
                "show_group_hud" => settings.show_group_hud = flag,
                "muted" => settings.muted = flag,
                "disabled" => settings.disabled = flag,
                "microphone" => settings.microphone = Some(value.to_owned()).filter(|v| !v.is_empty()),
                "speaker" => settings.speaker = Some(value.to_owned()).filter(|v| !v.is_empty()),
                _ => {
                    if let (Some(uuid), Some(volume)) = (
                        key.strip_prefix("volume.")
                            .and_then(|hex| u128::from_str_radix(hex, 16).ok()),
                        number(value, 0.0, 4.0),
                    ) {
                        settings.player_volumes.insert(uuid, volume);
                    }
                }
            }
        }
        settings
    }

    pub fn write(&self) -> String {
        let mut out = String::from("# Horizon Voice Chat: changed by the voice chat menu (V)\n");
        let mut line = |key: &str, value: String| {
            out.push_str(key);
            out.push('=');
            out.push_str(&value);
            out.push('\n');
        };
        line("voice_volume", format!("{:.2}", self.voice_volume));
        line(
            "microphone_amplification",
            format!("{:.2}", self.microphone_amplification),
        );
        line(
            "activation",
            match self.activation {
                Activation::PushToTalk => "push_to_talk",
                Activation::Voice => "voice",
            }
            .into(),
        );
        line("threshold", format!("{:.0}", self.threshold_db));
        line("stereo", self.stereo.to_string());
        line("hide_icons", self.hide_icons.to_string());
        line("show_group_hud", self.show_group_hud.to_string());
        line("muted", self.muted.to_string());
        line("disabled", self.disabled.to_string());
        line("microphone", self.microphone.clone().unwrap_or_default());
        line("speaker", self.speaker.clone().unwrap_or_default());
        let mut volumes: Vec<_> = self.player_volumes.iter().collect();
        volumes.sort_by_key(|(uuid, _)| **uuid);
        for (uuid, volume) in volumes {
            line(&format!("volume.{uuid:032x}"), format!("{volume:.2}"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_the_file_and_bad_values_keep_their_default() {
        let mut settings = Settings::default();
        settings.voice_volume = 1.5;
        settings.activation = Activation::Voice;
        settings.microphone = Some("USB Mic".into());
        settings.player_volumes.insert(0xab, 0.25);
        settings.muted = true;
        assert_eq!(Settings::parse(&settings.write()), settings);

        let parsed = Settings::parse("voice_volume=loud\nthreshold=-500\nnonsense\n");
        assert_eq!(parsed.voice_volume, 1.0);
        assert_eq!(parsed.threshold_db, -100.0);
    }
}
