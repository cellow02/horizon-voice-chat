//! The client's and the server's network code, compiled together and run against each other
//! over the loopback interface.
#![allow(dead_code)]

#[path = "../../shared/protocol/mod.rs"]
pub mod protocol;

#[path = "../../client/src/net.rs"]
pub mod client_net;

#[path = "../../server/src/udp.rs"]
pub mod server_udp;

/// AES-GCM as the mods for the older mod API do it.
#[path = "../../shared/legacy/gcm.rs"]
pub mod legacy_gcm;

/// AES-GCM as the other mods do it (the same file as `protocol::gcm`, which is private).
#[path = "../../shared/protocol/gcm.rs"]
pub mod crate_gcm;

/// Opus through libopus, as the mods for the mod SDK use it.
#[path = "../../client/src/codec.rs"]
pub mod codec;

/// Opus through rusty-opus, as the mods for the older mod API use it.
#[path = "../../legacy/client/api/src/codec.rs"]
pub mod legacy_codec;

#[cfg(test)]
mod legacy_tests {
    use super::protocol::{Codec, FRAME_SIZE, SAMPLE_RATE};
    use super::{codec, crate_gcm, legacy_codec, legacy_gcm};

    /// Both AES-GCM implementations produce the same bytes and open each other's packets,
    /// for every length around the 16-byte block size.
    #[test]
    fn the_legacy_aes_gcm_matches_the_aes_gcm_crate() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut random = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        };
        for len in (0..70).chain([255, 256, 1275, 2000]) {
            let key: [u8; 16] = std::array::from_fn(|_| random());
            let iv: [u8; 12] = std::array::from_fn(|_| random());
            let plain: Vec<u8> = (0..len).map(|_| random()).collect();
            let sealed = legacy_gcm::seal(&key, &iv, &plain);
            assert_eq!(sealed, crate_gcm::seal(&key, &iv, &plain), "length {len}");
            assert_eq!(legacy_gcm::open(&key, &iv, &sealed), Some(plain.clone()));
            assert_eq!(crate_gcm::open(&key, &iv, &sealed), Some(plain));
            for at in [0, sealed.len() / 2, sealed.len() - 1] {
                let mut changed = sealed.clone();
                changed[at] ^= 0x40;
                assert_eq!(legacy_gcm::open(&key, &iv, &changed), None);
            }
            assert_eq!(legacy_gcm::open(&key, &iv, &sealed[..sealed.len().min(15)]), None);
        }
    }

    fn tone(frame: usize) -> [f32; FRAME_SIZE] {
        std::array::from_fn(|i| {
            let t = (frame * FRAME_SIZE + i) as f32 / SAMPLE_RATE as f32;
            (t * 440.0 * std::f32::consts::TAU).sin() * 0.5
        })
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    /// Speech encoded by either Opus implementation plays in the other, in every codec mode a
    /// server can ask for. Simple Voice Chat itself uses libopus.
    #[test]
    fn both_opus_implementations_understand_each_other() {
        for mode in [Codec::Voip, Codec::Audio, Codec::RestrictedLowDelay] {
            let mut rusty_encoder = legacy_codec::Encoder::new(mode).unwrap();
            let mut libopus_encoder = codec::Encoder::new(mode).unwrap();
            let mut rusty_decoder = legacy_codec::Decoder::new().unwrap();
            let mut libopus_decoder = codec::Decoder::new().unwrap();
            let (mut from_rusty, mut from_libopus) = (Vec::new(), Vec::new());
            for frame in 0..15 {
                let data = rusty_encoder.encode(&tone(frame)).unwrap();
                assert!(data.len() < 400, "about 80 bytes at 32 kbit/s, got {}", data.len());
                from_rusty = libopus_decoder.decode(&data).unwrap();
                let data = libopus_encoder.encode(&tone(frame)).unwrap();
                from_libopus = rusty_decoder.decode(&data).unwrap();
            }
            // A 0.5 amplitude sine has an RMS of about 0.35.
            for (name, samples) in [("libopus decoder", from_rusty), ("rusty decoder", from_libopus)] {
                assert_eq!(samples.len(), FRAME_SIZE, "{name}, {mode:?}");
                let level = rms(&samples);
                assert!((0.25..0.45).contains(&level), "{name}, {mode:?}: rms {level}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::client_net::{Place, Status, Target, VoiceClient};
    use super::server_udp::{Event, Position, Settings, VoiceServer};
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    const ALEX: u128 = 0xa1;
    const STEVE: u128 = 0xb2;

    fn server(keep_alive_ms: u32) -> VoiceServer {
        VoiceServer::start(Settings {
            port: 0,
            bind_address: "127.0.0.1".into(),
            keep_alive_ms,
            ..Settings::default()
        })
        .unwrap()
    }

    fn join(server: &VoiceServer, player: u128) -> VoiceClient {
        let (connection, secret) = server.create_connection(player).unwrap();
        VoiceClient::connect(Target {
            host: "127.0.0.1".into(),
            port: server.port(),
            connection,
            secret,
            keep_alive_ms: server.settings().keep_alive_ms,
        })
        .unwrap()
    }

    fn wait(what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn place(server: &VoiceServer, players: &[(u128, u64, [f64; 3])]) {
        server.set_positions(
            players
                .iter()
                .map(|(player, level, position)| {
                    (*player, Position { level: *level, position: *position })
                })
                .collect::<HashMap<_, _>>(),
        );
    }

    #[test]
    fn two_players_connect_and_hear_each_other_only_in_range() {
        let server = server(1000);
        let alex = join(&server, ALEX);
        let steve = join(&server, STEVE);
        assert!(server.create_connection(ALEX).is_none(), "one secret per player");
        wait("both handshakes", || {
            alex.status() == Status::Connected && steve.status() == Status::Connected
        });
        let mut events = server.take_events();
        events.sort_by_key(|event| match event {
            Event::Connected(player) | Event::TimedOut(player) => *player,
        });
        assert_eq!(events, [Event::Connected(ALEX), Event::Connected(STEVE)]);
        assert!(server.is_connected(ALEX) && server.is_connected(STEVE));

        // Ten blocks apart in the same dimension: Steve hears Alex, Alex does not hear herself.
        place(&server, &[(ALEX, 0, [0.0, 64.0, 0.0]), (STEVE, 0, [10.0, 64.0, 0.0])]);
        alex.send_mic(&[1, 2, 3], 0, false);
        let mut heard = Vec::new();
        wait("Steve to hear Alex", || {
            heard.extend(steve.sounds());
            !heard.is_empty()
        });
        assert_eq!(heard.len(), 1);
        assert_eq!(heard[0].sender, ALEX);
        assert_eq!(heard[0].channel, ALEX);
        assert_eq!(heard[0].data, [1, 2, 3]);
        assert_eq!(heard[0].sequence, 0);
        assert_eq!(heard[0].place, Place::Player { distance: 48.0 });
        assert!(!heard[0].whispering);

        // Whispering carries the whisper distance; the empty frame that ends a burst is relayed.
        alex.send_mic(&[], 1, true);
        heard.clear();
        wait("the end of the burst", || {
            heard.extend(steve.sounds());
            !heard.is_empty()
        });
        assert!(heard[0].data.is_empty() && heard[0].whispering);
        assert_eq!(heard[0].place, Place::Player { distance: 24.0 });

        // Out of range, in another dimension, or with voice chat turned off: silence. A frame
        // sent afterwards from in range arrives first, which proves the others were not sent.
        place(&server, &[(ALEX, 0, [0.0, 64.0, 0.0]), (STEVE, 0, [60.0, 64.0, 0.0])]);
        alex.send_mic(&[4], 2, false);
        place(&server, &[(ALEX, 0, [0.0, 64.0, 0.0]), (STEVE, 1, [1.0, 64.0, 0.0])]);
        alex.send_mic(&[5], 3, false);
        place(&server, &[(ALEX, 0, [0.0, 64.0, 0.0]), (STEVE, 0, [1.0, 64.0, 0.0])]);
        server.set_disabled(STEVE, true);
        alex.send_mic(&[6], 4, false);
        std::thread::sleep(Duration::from_millis(100));
        server.set_disabled(STEVE, false);
        alex.send_mic(&[7], 5, false);
        heard.clear();
        wait("the frame sent in range", || {
            heard.extend(steve.sounds());
            !heard.is_empty()
        });
        assert_eq!(heard[0].data, [7]);
        assert_eq!(alex.sounds().count(), 0);
    }

    #[test]
    fn groups_hear_each_other_anywhere_and_isolated_groups_only_themselves() {
        use super::protocol::GroupType;
        let server = server(1000);
        let alex = join(&server, ALEX);
        let steve = join(&server, STEVE);
        wait("both handshakes", || {
            alex.status() == Status::Connected && steve.status() == Status::Connected
        });
        // Same group, different dimensions: Steve hears Alex without a position.
        place(&server, &[(ALEX, 0, [0.0, 64.0, 0.0]), (STEVE, 1, [5000.0, 64.0, 0.0])]);
        server.set_groups(HashMap::from([
            (ALEX, (7, GroupType::Normal)),
            (STEVE, (7, GroupType::Normal)),
        ]));
        alex.send_mic(&[1], 0, false);
        let mut heard = Vec::new();
        wait("Steve to hear the group", || {
            heard.extend(steve.sounds());
            !heard.is_empty()
        });
        assert_eq!(heard[0].place, Place::Everywhere);
        assert_eq!(heard[0].sender, ALEX);

        // Alex alone in an isolated group, Steve next to her: he hears nothing, she hears
        // nothing of him. The frame sent after the groups are gone arrives first.
        place(&server, &[(ALEX, 0, [0.0, 64.0, 0.0]), (STEVE, 0, [1.0, 64.0, 0.0])]);
        server.set_groups(HashMap::from([(ALEX, (8, GroupType::Isolated))]));
        alex.send_mic(&[2], 1, false);
        steve.send_mic(&[3], 0, false);
        std::thread::sleep(Duration::from_millis(100));
        server.set_groups(HashMap::new());
        alex.send_mic(&[4], 2, false);
        heard.clear();
        wait("the frame sent without groups", || {
            heard.extend(steve.sounds());
            !heard.is_empty()
        });
        assert_eq!(heard[0].data, [4]);
        assert_eq!(heard[0].place, Place::Player { distance: 48.0 });
        assert_eq!(alex.sounds().count(), 0);
    }

    #[test]
    fn a_wrong_secret_never_connects() {
        let server = server(1000);
        let (connection, _) = server.create_connection(ALEX).unwrap();
        let intruder = VoiceClient::connect(Target {
            host: "127.0.0.1".into(),
            port: server.port(),
            connection,
            secret: super::protocol::Secret::random(),
            keep_alive_ms: 1000,
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(intruder.status(), Status::Connecting);
        assert!(server.take_events().is_empty());
        assert!(!server.is_connected(ALEX));
    }

    #[test]
    fn both_sides_notice_when_the_other_goes_away() {
        // The client goes away: the server times the player out and forgets the secret.
        let server = server(50);
        let alex = join(&server, ALEX);
        wait("the handshake", || alex.status() == Status::Connected);
        assert_eq!(server.take_events(), [Event::Connected(ALEX)]);
        drop(alex);
        let mut events = Vec::new();
        wait("the server to time Alex out", || {
            events.extend(server.take_events());
            !events.is_empty()
        });
        assert_eq!(events, [Event::TimedOut(ALEX)]);
        assert!(server.create_connection(ALEX).is_some(), "the old secret is gone");

        // The server goes away: the client reports a timeout.
        let steve = join(&server, STEVE);
        wait("the handshake", || steve.status() == Status::Connected);
        drop(server);
        wait("the client to time out", || steve.status() == Status::TimedOut);
    }

    #[test]
    fn removing_a_player_stops_their_speech() {
        let server = server(1000);
        let alex = join(&server, ALEX);
        let steve = join(&server, STEVE);
        wait("both handshakes", || {
            alex.status() == Status::Connected && steve.status() == Status::Connected
        });
        place(&server, &[(ALEX, 0, [0.0; 3]), (STEVE, 0, [1.0, 0.0, 0.0])]);
        server.remove_player(ALEX);
        alex.send_mic(&[1], 0, false);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(steve.sounds().count(), 0);
    }
}
