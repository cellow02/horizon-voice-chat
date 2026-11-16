//! The client's and the server's network code, compiled together and run against each other
//! over the loopback interface.
#![allow(dead_code)]

#[path = "../../shared/protocol/mod.rs"]
pub mod protocol;

#[path = "../../client/src/net.rs"]
pub mod client_net;

#[path = "../../server/src/udp.rs"]
pub mod server_udp;

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
