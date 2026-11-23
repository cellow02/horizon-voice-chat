//! The server's config file: `config/horizon_voice_server-server.toml`, flat `key = value` lines.

use crate::udp::Settings;

/// The file the game writes when none exists.
pub const DEFAULT: &str = r#"# Horizon Voice Chat (Server)

# UDP port of the voice server. Open or forward it in your firewall and router, like the game port.
# 0 picks a free port; in single player and on a LAN host a busy port falls back to a free one.
port = 24454

# IP address to bind the voice socket to. Empty binds every IPv4 address; for IPv6, put the
# server's IPv6 address here.
bind_address = ""

# What clients connect to for voice, when it differs from the address they joined the game with:
# empty (the game address and the port above), a port, or host[:port].
# Set it when the voice port is reachable under another address or port (proxy, port mapping).
voice_host = ""

# How far normal speech carries, in blocks.
distance = 48.0

# How far whispering carries, in blocks.
whisper_distance = 24.0

# Milliseconds between keep-alive packets. A client that misses ten in a row is disconnected.
keep_alive = 1000

# Largest voice packet clients should send, in bytes.
mtu_size = 1024

# Whether clients may record voice chat (clients decide how to honour it).
allow_recording = true

# Whether players may create and join groups.
groups = true
"#;

/// Everything the config file sets.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub udp: Settings,
    pub voice_host: String,
    pub mtu_size: i32,
    pub allow_recording: bool,
    pub groups: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            udp: Settings::default(),
            voice_host: String::new(),
            mtu_size: 1024,
            allow_recording: true,
            groups: true,
        }
    }
}

/// Reads the config text. An unknown key or a bad value keeps its default and is reported in
/// the returned warnings, so a typo never stops the server.
pub fn parse(text: &str) -> (Config, Vec<String>) {
    let mut config = Config::default();
    let mut warnings = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            warnings.push(format!("line {}: expected key = value", index + 1));
            continue;
        };
        let key = key.trim();
        // A value ends at a comment; quotes around a string are optional.
        let value = match value.trim().strip_prefix('"') {
            Some(rest) => rest.split('"').next().unwrap_or(""),
            None => value.split('#').next().unwrap_or("").trim(),
        };
        let ok = match key {
            "port" => value.parse().map(|v| config.udp.port = v).is_ok(),
            "bind_address" => {
                config.udp.bind_address = value.to_owned();
                true
            }
            "voice_host" => {
                config.voice_host = value.to_owned();
                true
            }
            "distance" => parse_distance(value).map(|v| config.udp.distance = v).is_some(),
            "whisper_distance" => parse_distance(value)
                .map(|v| config.udp.whisper_distance = v)
                .is_some(),
            "keep_alive" => value
                .parse::<u32>()
                .ok()
                .filter(|v| (100..=60_000).contains(v))
                .map(|v| config.udp.keep_alive_ms = v)
                .is_some(),
            "mtu_size" => value
                .parse::<i32>()
                .ok()
                .filter(|v| (512..=2048).contains(v))
                .map(|v| config.mtu_size = v)
                .is_some(),
            "allow_recording" => value.parse().map(|v| config.allow_recording = v).is_ok(),
            "groups" => value.parse().map(|v| config.groups = v).is_ok(),
            _ => {
                warnings.push(format!("line {}: unknown key {key:?}", index + 1));
                continue;
            }
        };
        if !ok {
            warnings.push(format!(
                "line {}: {value:?} is not a valid value for {key}",
                index + 1
            ));
        }
    }
    (config, warnings)
}

fn parse_distance(value: &str) -> Option<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && (1.0..=1_000_000.0).contains(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_file_parses_to_the_defaults_without_warnings() {
        assert_eq!(parse(DEFAULT), (Config::default(), vec![]));
    }

    #[test]
    fn values_are_read_and_bad_ones_keep_their_default() {
        let (config, warnings) = parse(
            "port = 30000 # custom\nvoice_host = \"voice.example:1\"\ndistance = 16\n\
             keep_alive = 5\nnonsense = 1\nallow_recording = false\nbroken line\n",
        );
        assert_eq!(config.udp.port, 30000);
        assert_eq!(config.voice_host, "voice.example:1");
        assert_eq!(config.udp.distance, 16.0);
        assert_eq!(config.udp.keep_alive_ms, 1000);
        assert!(!config.allow_recording);
        assert_eq!(warnings.len(), 3);
    }
}
