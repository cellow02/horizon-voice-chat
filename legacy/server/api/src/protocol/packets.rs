//! The packets: UDP voice packets, their datagram framing, and the plugin-channel messages.

use super::crypto::Secret;
use super::wire::{Reader, WireError, Writer};
use super::{MAX_OPUS_PAYLOAD, MAX_PACKET_SIZE};

/// First byte of every voice datagram.
pub const MAGIC: u8 = 0xff;
/// Bytes of the address-check nonce.
pub const NONCE_SIZE: usize = 16;

const FLAG_WHISPER: u8 = 0b01;
const FLAG_CATEGORY: u8 = 0b10;

/// A packet inside an encrypted voice datagram. The first plain byte is its type.
#[derive(Debug, Clone, PartialEq)]
pub enum VoicePacket {
    /// 0x01, client to server: one Opus frame from the microphone. Empty `data` ends a talk burst.
    Mic {
        data: Vec<u8>,
        sequence: i64,
        whispering: bool,
    },
    /// 0x02, server to client: a player's voice, heard at that player's position.
    PlayerSound {
        channel: u128,
        sender: u128,
        data: Vec<u8>,
        sequence: i64,
        distance: f32,
        whispering: bool,
        category: Option<String>,
    },
    /// 0x03, server to client: voice without a position (groups).
    GroupSound {
        channel: u128,
        sender: u128,
        data: Vec<u8>,
        sequence: i64,
        category: Option<String>,
    },
    /// 0x04, server to client: voice at a fixed position.
    LocationSound {
        channel: u128,
        sender: u128,
        location: [f64; 3],
        data: Vec<u8>,
        sequence: i64,
        distance: f32,
        category: Option<String>,
    },
    /// 0x05, client to server: "I hold the secret of this connection id".
    Authenticate,
    /// 0x06, server to client: the nonce to send back from the same address.
    AuthenticateAck { nonce: [u8; NONCE_SIZE] },
    /// 0x07, both ways: echoed unchanged.
    Ping { id: u128, timestamp: i64 },
    /// 0x08, both ways: the server sends it, the client answers with one.
    KeepAlive,
    /// 0x09, client to server: the nonce from `AuthenticateAck`.
    ConnectionCheck { nonce: [u8; NONCE_SIZE] },
    /// 0x0a, server to client: the connection is established.
    ConnectionCheckAck,
}

fn category(reader: &mut Reader<'_>, flags: u8) -> Result<Option<String>, WireError> {
    if flags & FLAG_CATEGORY == 0 {
        return Ok(None);
    }
    reader.string(16).map(Some)
}

fn write_category(writer: &mut Writer, mut flags: u8, category: &Option<String>) {
    if category.is_some() {
        flags |= FLAG_CATEGORY;
    }
    writer.u8(flags);
    if let Some(category) = category {
        writer.string(category);
    }
}

impl VoicePacket {
    /// The plain bytes: type, then fields.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        match self {
            Self::Mic {
                data,
                sequence,
                whispering,
            } => {
                w.u8(0x01).byte_array(data).i64(*sequence).bool(*whispering);
            }
            Self::PlayerSound {
                channel,
                sender,
                data,
                sequence,
                distance,
                whispering,
                category,
            } => {
                w.u8(0x02)
                    .uuid(*channel)
                    .uuid(*sender)
                    .byte_array(data)
                    .i64(*sequence)
                    .f32(*distance);
                let flags = if *whispering { FLAG_WHISPER } else { 0 };
                write_category(&mut w, flags, category);
            }
            Self::GroupSound {
                channel,
                sender,
                data,
                sequence,
                category,
            } => {
                w.u8(0x03)
                    .uuid(*channel)
                    .uuid(*sender)
                    .byte_array(data)
                    .i64(*sequence);
                write_category(&mut w, 0, category);
            }
            Self::LocationSound {
                channel,
                sender,
                location,
                data,
                sequence,
                distance,
                category,
            } => {
                w.u8(0x04).uuid(*channel).uuid(*sender);
                for axis in location {
                    w.f64(*axis);
                }
                w.byte_array(data).i64(*sequence).f32(*distance);
                write_category(&mut w, 0, category);
            }
            Self::Authenticate => {
                w.u8(0x05);
            }
            Self::AuthenticateAck { nonce } => {
                w.u8(0x06).raw(nonce);
            }
            Self::Ping { id, timestamp } => {
                w.u8(0x07).uuid(*id).i64(*timestamp);
            }
            Self::KeepAlive => {
                w.u8(0x08);
            }
            Self::ConnectionCheck { nonce } => {
                w.u8(0x09).raw(nonce);
            }
            Self::ConnectionCheckAck => {
                w.u8(0x0a);
            }
        }
        w.finish()
    }

    /// Reads the plain bytes of one packet. Bytes after the last field are ignored.
    pub fn decode(plain: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(plain);
        Ok(match r.u8()? {
            0x01 => Self::Mic {
                data: r.byte_array(MAX_OPUS_PAYLOAD)?.to_vec(),
                sequence: r.i64()?,
                whispering: r.bool()?,
            },
            0x02 => {
                let channel = r.uuid()?;
                let sender = r.uuid()?;
                let data = r.byte_array(MAX_OPUS_PAYLOAD)?.to_vec();
                let sequence = r.i64()?;
                let distance = r.f32()?;
                let flags = r.u8()?;
                Self::PlayerSound {
                    channel,
                    sender,
                    data,
                    sequence,
                    distance,
                    whispering: flags & FLAG_WHISPER != 0,
                    category: category(&mut r, flags)?,
                }
            }
            0x03 => {
                let channel = r.uuid()?;
                let sender = r.uuid()?;
                let data = r.byte_array(MAX_OPUS_PAYLOAD)?.to_vec();
                let sequence = r.i64()?;
                let flags = r.u8()?;
                Self::GroupSound {
                    channel,
                    sender,
                    data,
                    sequence,
                    category: category(&mut r, flags)?,
                }
            }
            0x04 => {
                let channel = r.uuid()?;
                let sender = r.uuid()?;
                let location = [r.f64()?, r.f64()?, r.f64()?];
                let data = r.byte_array(MAX_OPUS_PAYLOAD)?.to_vec();
                let sequence = r.i64()?;
                let distance = r.f32()?;
                let flags = r.u8()?;
                Self::LocationSound {
                    channel,
                    sender,
                    location,
                    data,
                    sequence,
                    distance,
                    category: category(&mut r, flags)?,
                }
            }
            0x05 => Self::Authenticate,
            0x06 => Self::AuthenticateAck {
                nonce: r.bytes16()?,
            },
            0x07 => Self::Ping {
                id: r.uuid()?,
                timestamp: r.i64()?,
            },
            0x08 => Self::KeepAlive,
            0x09 => Self::ConnectionCheck {
                nonce: r.bytes16()?,
            },
            0x0a => Self::ConnectionCheckAck,
            _ => return Err(WireError::InvalidValue),
        })
    }
}

/// A datagram from a client: magic, connection id, then the encrypted packet as a byte array.
pub fn frame_to_server(connection: u128, secret: &Secret, packet: &VoicePacket) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(MAGIC)
        .uuid(connection)
        .byte_array(&secret.encrypt(&packet.encode()));
    w.finish()
}

/// A datagram from the server: magic, then the encrypted packet as a byte array.
pub fn frame_to_client(secret: &Secret, packet: &VoicePacket) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(MAGIC).byte_array(&secret.encrypt(&packet.encode()));
    w.finish()
}

/// The connection id and the still encrypted payload of a datagram a server received.
pub fn open_at_server(datagram: &[u8]) -> Result<(u128, &[u8]), WireError> {
    let mut r = Reader::new(datagram);
    if r.u8()? != MAGIC {
        return Err(WireError::InvalidValue);
    }
    let connection = r.uuid()?;
    Ok((connection, r.byte_array(MAX_PACKET_SIZE)?))
}

/// The packet of a datagram a client received, or None when it is not for this secret.
pub fn open_at_client(secret: &Secret, datagram: &[u8]) -> Option<VoicePacket> {
    let mut r = Reader::new(datagram);
    if r.u8().ok()? != MAGIC {
        return None;
    }
    let plain = secret.decrypt(r.byte_array(MAX_PACKET_SIZE).ok()?)?;
    VoicePacket::decode(&plain).ok()
}

/// The Opus application the server asks clients to encode with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Codec {
    #[default]
    Voip,
    Audio,
    RestrictedLowDelay,
}

/// `voicechat:request_secret`: the client's compatibility version.
pub fn encode_request_secret(version: i32) -> Vec<u8> {
    let mut w = Writer::new();
    w.i32(version);
    w.finish()
}

pub fn decode_request_secret(body: &[u8]) -> Result<i32, WireError> {
    Reader::new(body).i32()
}

/// `voicechat:secret`: everything a client needs to open its voice connection.
#[derive(Debug, Clone, PartialEq)]
pub struct SecretMessage {
    pub secret: Secret,
    pub connection: u128,
    /// The server's UDP port.
    pub port: i32,
    pub player: u128,
    pub codec: Codec,
    pub mtu_size: i32,
    /// How far normal speech carries, in blocks.
    pub distance: f64,
    /// Milliseconds between the server's keep-alives.
    pub keep_alive: i32,
    pub groups_enabled: bool,
    /// Empty (use the game server's address and `port`), a port, or `host[:port]`.
    pub voice_host: String,
    pub allow_recording: bool,
}

impl SecretMessage {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.raw(&self.secret.0)
            .uuid(self.connection)
            .i32(self.port)
            .uuid(self.player)
            .u8(self.codec as u8)
            .i32(self.mtu_size)
            .f64(self.distance)
            .i32(self.keep_alive)
            .bool(self.groups_enabled)
            .string(&self.voice_host)
            .bool(self.allow_recording);
        w.finish()
    }

    /// The same message for a client that speaks protocol 20: no connection id, since those
    /// clients put their player UUID in front of their datagrams.
    pub fn encode_v20(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.raw(&self.secret.0)
            .i32(self.port)
            .uuid(self.player)
            .u8(self.codec as u8)
            .i32(self.mtu_size)
            .f64(self.distance)
            .i32(self.keep_alive)
            .bool(self.groups_enabled)
            .string(&self.voice_host)
            .bool(self.allow_recording);
        w.finish()
    }

    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(body);
        Ok(Self {
            secret: Secret(r.bytes16()?),
            connection: r.uuid()?,
            port: r.i32()?,
            player: r.uuid()?,
            codec: match r.u8()? {
                0 => Codec::Voip,
                1 => Codec::Audio,
                2 => Codec::RestrictedLowDelay,
                _ => return Err(WireError::InvalidValue),
            },
            mtu_size: r.i32()?.clamp(512, 2048),
            distance: r.f64()?,
            keep_alive: r.i32()?,
            groups_enabled: r.bool()?,
            voice_host: r.string(32767)?,
            allow_recording: r.bool()?,
        })
    }

    /// The host and port to send voice datagrams to, given the address the game joined.
    pub fn voice_address(&self, game_host: &str) -> (String, u16) {
        let default_port = u16::try_from(self.port).unwrap_or(super::DEFAULT_PORT);
        let host = self.voice_host.trim();
        if host.is_empty() {
            return (game_host.to_owned(), default_port);
        }
        if let Ok(port) = host.parse::<u32>() {
            return match u16::try_from(port) {
                Ok(port) if port > 0 => (game_host.to_owned(), port),
                _ => (game_host.to_owned(), default_port),
            };
        }
        let (name, port) = split_host_port(host);
        (
            if name.is_empty() { game_host } else { name }.to_owned(),
            port.unwrap_or(default_port),
        )
    }
}

/// Splits `host`, `host:port`, `[v6]` and `[v6]:port`. A bare IPv6 address has no port.
pub fn split_host_port(address: &str) -> (&str, Option<u16>) {
    if let Some(rest) = address.strip_prefix('[') {
        if let Some((host, tail)) = rest.split_once(']') {
            return (host, tail.strip_prefix(':').and_then(|p| p.parse().ok()));
        }
    }
    match address.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => match port.parse() {
            Ok(port) => (host, Some(port)),
            Err(_) => (address, None),
        },
        _ => (address, None),
    }
}

/// One player's voice state, as the server tells it to every client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerState {
    pub uuid: u128,
    pub name: String,
    /// The player muted their microphone.
    pub muted: bool,
    /// The server muted the player.
    pub force_muted: bool,
    /// The player turned voice chat off.
    pub disabled: bool,
    /// The server turned voice chat off for the player.
    pub force_disabled: bool,
    /// The player has no voice connection (yet).
    pub disconnected: bool,
    pub group: Option<u128>,
}

impl PlayerState {
    /// The state of a player who joined the game but has no voice connection yet.
    pub fn disconnected(uuid: u128, name: &str) -> Self {
        Self {
            uuid,
            name: name.to_owned(),
            muted: false,
            force_muted: false,
            disabled: false,
            force_disabled: false,
            disconnected: true,
            group: None,
        }
    }

    pub fn write(&self, w: &mut Writer) {
        let flags = u8::from(self.muted)
            | u8::from(self.force_muted) << 1
            | u8::from(self.disabled) << 2
            | u8::from(self.force_disabled) << 3
            | u8::from(self.disconnected) << 4
            | u8::from(self.group.is_some()) << 5;
        w.u8(flags).uuid(self.uuid).string(&self.name);
        if let Some(group) = self.group {
            w.uuid(group);
        }
    }

    /// The state as protocol 20 writes it: two flags as booleans, no mute flags.
    pub fn write_v20(&self, w: &mut Writer) {
        w.bool(self.disabled || self.force_disabled)
            .bool(self.disconnected)
            .uuid(self.uuid)
            .string(&self.name)
            .bool(self.group.is_some());
        if let Some(group) = self.group {
            w.uuid(group);
        }
    }

    pub fn encode_v20(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.write_v20(&mut w);
        w.finish()
    }

    pub fn read(r: &mut Reader<'_>) -> Result<Self, WireError> {
        let flags = r.u8()?;
        let uuid = r.uuid()?;
        let name = r.string(256)?;
        Ok(Self {
            uuid,
            name,
            muted: flags & 1 != 0,
            force_muted: flags & 2 != 0,
            disabled: flags & 4 != 0,
            force_disabled: flags & 8 != 0,
            disconnected: flags & 16 != 0,
            group: if flags & 32 != 0 { Some(r.uuid()?) } else { None },
        })
    }

    /// `voicechat:state`.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.write(&mut w);
        w.finish()
    }

    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        Self::read(&mut Reader::new(body))
    }
}

/// `voicechat:states`: a count, then that many states.
pub fn encode_states(states: &[PlayerState]) -> Vec<u8> {
    let mut w = Writer::new();
    w.i32(states.len() as i32);
    for state in states {
        state.write(&mut w);
    }
    w.finish()
}

/// Every player's state for a client that speaks protocol 20.
pub fn encode_states_v20(states: &[PlayerState]) -> Vec<u8> {
    let mut w = Writer::new();
    w.i32(states.len() as i32);
    for state in states {
        state.write_v20(&mut w);
    }
    w.finish()
}

pub fn decode_states(body: &[u8]) -> Result<Vec<PlayerState>, WireError> {
    let mut r = Reader::new(body);
    let count = usize::try_from(r.i32()?).map_err(|_| WireError::BadLength)?;
    // A state is at least 18 bytes, so a count the body cannot hold is refused before allocating.
    if count > r.remaining() / 18 {
        return Err(WireError::BadLength);
    }
    (0..count).map(|_| PlayerState::read(&mut r)).collect()
}

/// `voicechat:remove_state`: the player who left.
pub fn encode_remove_state(uuid: u128) -> Vec<u8> {
    let mut w = Writer::new();
    w.uuid(uuid);
    w.finish()
}

pub fn decode_remove_state(body: &[u8]) -> Result<u128, WireError> {
    Reader::new(body).uuid()
}

/// `voicechat:client_state`: muted, then disabled.
pub fn encode_client_state(muted: bool, disabled: bool) -> Vec<u8> {
    let mut w = Writer::new();
    w.bool(muted).bool(disabled);
    w.finish()
}

pub fn decode_client_state(body: &[u8]) -> Result<(bool, bool), WireError> {
    let mut r = Reader::new(body);
    Ok((r.bool()?, r.bool()?))
}

/// How a group hears the players around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroupType {
    /// Members hear each other anywhere, and the players near them as usual.
    #[default]
    Normal,
    /// Like normal, and players nearby outside the group hear the members too.
    Open,
    /// Members hear only each other.
    Isolated,
}

impl GroupType {
    pub const ALL: [GroupType; 3] = [GroupType::Normal, GroupType::Open, GroupType::Isolated];

    pub fn code(self) -> i16 {
        match self {
            GroupType::Normal => 0,
            GroupType::Open => 1,
            GroupType::Isolated => 2,
        }
    }

    /// Unknown codes are normal groups, as the original reads them.
    pub fn from_code(code: i16) -> Self {
        match code {
            1 => GroupType::Open,
            2 => GroupType::Isolated,
            _ => GroupType::Normal,
        }
    }
}

/// `voicechat:add_group`: a group as clients see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientGroup {
    pub id: u128,
    pub name: String,
    pub has_password: bool,
    /// Kept when its last member leaves.
    pub persistent: bool,
    /// Not listed for players outside it.
    pub hidden: bool,
    pub kind: GroupType,
}

impl ClientGroup {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.uuid(self.id)
            .string(&self.name)
            .bool(self.has_password)
            .bool(self.persistent)
            .bool(self.hidden)
            .i16(self.kind.code());
        w.finish()
    }

    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(body);
        Ok(Self {
            id: r.uuid()?,
            name: r.string(super::MAX_GROUP_NAME)?,
            has_password: r.bool()?,
            persistent: r.bool()?,
            hidden: r.bool()?,
            kind: GroupType::from_code(r.i16()?),
        })
    }
}

/// `voicechat:remove_group`: the group's id.
pub fn encode_remove_group(id: u128) -> Vec<u8> {
    let mut w = Writer::new();
    w.uuid(id);
    w.finish()
}

pub fn decode_remove_group(body: &[u8]) -> Result<u128, WireError> {
    Reader::new(body).uuid()
}

/// `voicechat:joined_group`: the player's group now, and whether a join failed on its password.
pub fn encode_joined_group(group: Option<u128>, wrong_password: bool) -> Vec<u8> {
    let mut w = Writer::new();
    w.bool(group.is_some());
    if let Some(group) = group {
        w.uuid(group);
    }
    w.bool(wrong_password);
    w.finish()
}

pub fn decode_joined_group(body: &[u8]) -> Result<(Option<u128>, bool), WireError> {
    let mut r = Reader::new(body);
    let group = if r.bool()? { Some(r.uuid()?) } else { None };
    Ok((group, r.bool()?))
}

/// `voicechat:set_group`: join `group`, with a password when it has one.
pub fn encode_set_group(group: u128, password: Option<&str>) -> Vec<u8> {
    let mut w = Writer::new();
    w.uuid(group).bool(password.is_some());
    if let Some(password) = password {
        w.string(password);
    }
    w.finish()
}

pub fn decode_set_group(body: &[u8]) -> Result<(u128, Option<String>), WireError> {
    let mut r = Reader::new(body);
    let group = r.uuid()?;
    let password = if r.bool()? {
        Some(r.string(super::MAX_GROUP_PASSWORD)?)
    } else {
        None
    };
    Ok((group, password))
}

/// `voicechat:create_group`: name, optional password, type.
pub fn encode_create_group(name: &str, password: Option<&str>, kind: GroupType) -> Vec<u8> {
    let mut w = Writer::new();
    w.string(name).bool(password.is_some());
    if let Some(password) = password {
        w.string(password);
    }
    w.i16(kind.code());
    w.finish()
}

pub fn decode_create_group(body: &[u8]) -> Result<(String, Option<String>, GroupType), WireError> {
    let mut r = Reader::new(body);
    let name = r.string(super::MAX_GROUP_NAME)?;
    let password = if r.bool()? {
        Some(r.string(super::MAX_GROUP_PASSWORD)?)
    } else {
        None
    };
    Ok((name, password, GroupType::from_code(r.i16()?)))
}

/// Whether `name` may name a group: 1 to 24 characters, no control or format characters, and
/// not starting with whitespace.
pub fn valid_group_name(name: &str) -> bool {
    let count = name.chars().count();
    (1..=super::MAX_GROUP_NAME).contains(&count)
        && !name.chars().next().is_some_and(char::is_whitespace)
        && name.chars().all(|c| !is_other_char(c))
}

/// Whether `password` may be a group password: 1 to 256 characters, none of them control or
/// format characters.
pub fn valid_group_password(password: &str) -> bool {
    let count = password.chars().count();
    (1..=super::MAX_GROUP_PASSWORD).contains(&count) && password.chars().all(|c| !is_other_char(c))
}

/// Unicode's "other" characters as far as std can tell them: control characters and the
/// common format and private-use ranges.
fn is_other_char(c: char) -> bool {
    c.is_control()
        || matches!(c as u32,
            0x00ad | 0x0600..=0x0605 | 0x061c | 0x06dd | 0x070f | 0x180e | 0x200b..=0x200f
            | 0x202a..=0x202e | 0x2060..=0x2064 | 0x2066..=0x206f | 0xfeff | 0xfff9..=0xfffb
            | 0xe000..=0xf8ff | 0xf0000..=0x10ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_packets() -> Vec<VoicePacket> {
        vec![
            VoicePacket::Mic {
                data: vec![1, 2, 3],
                sequence: 7,
                whispering: true,
            },
            VoicePacket::PlayerSound {
                channel: 1,
                sender: 2,
                data: vec![9; 40],
                sequence: 1 << 40,
                distance: 48.0,
                whispering: false,
                category: None,
            },
            VoicePacket::PlayerSound {
                channel: 1,
                sender: 2,
                data: vec![],
                sequence: 3,
                distance: 24.0,
                whispering: true,
                category: Some("music".into()),
            },
            VoicePacket::GroupSound {
                channel: 3,
                sender: 4,
                data: vec![5],
                sequence: 9,
                category: None,
            },
            VoicePacket::LocationSound {
                channel: 5,
                sender: 6,
                location: [1.5, -64.0, 3.25],
                data: vec![7, 7],
                sequence: 10,
                distance: 16.0,
                category: Some("blocks".into()),
            },
            VoicePacket::Authenticate,
            VoicePacket::AuthenticateAck { nonce: [3; 16] },
            VoicePacket::Ping {
                id: 99,
                timestamp: 1234,
            },
            VoicePacket::KeepAlive,
            VoicePacket::ConnectionCheck { nonce: [4; 16] },
            VoicePacket::ConnectionCheckAck,
        ]
    }

    #[test]
    fn every_packet_round_trips() {
        for packet in all_packets() {
            assert_eq!(VoicePacket::decode(&packet.encode()), Ok(packet));
        }
    }

    #[test]
    fn a_mic_packet_has_the_documented_bytes() {
        let packet = VoicePacket::Mic {
            data: vec![0xaa, 0xbb],
            sequence: 5,
            whispering: true,
        };
        assert_eq!(
            packet.encode(),
            [0x01, 2, 0xaa, 0xbb, 0, 0, 0, 0, 0, 0, 0, 5, 1]
        );
    }

    #[test]
    fn truncated_and_unknown_packets_are_refused() {
        for packet in all_packets() {
            let bytes = packet.encode();
            for len in 0..bytes.len() {
                // Shorter than a whole packet never decodes to the same packet, and never panics.
                assert_ne!(VoicePacket::decode(&bytes[..len]), Ok(packet.clone()));
            }
        }
        assert!(VoicePacket::decode(&[0x0b]).is_err());
        assert!(VoicePacket::decode(&[]).is_err());
        // An Opus frame above the protocol's limit.
        let mut big = Writer::new();
        big.u8(0x01).byte_array(&[0; 1276]).i64(0).bool(false);
        assert!(VoicePacket::decode(&big.finish()).is_err());
    }

    #[test]
    fn datagrams_round_trip_in_both_directions() {
        let secret = Secret::random();
        let packet = VoicePacket::ConnectionCheck { nonce: [8; 16] };
        let datagram = frame_to_server(42, &secret, &packet);
        assert_eq!(datagram[0], MAGIC);
        let (connection, sealed) = open_at_server(&datagram).unwrap();
        assert_eq!(connection, 42);
        assert_eq!(
            VoicePacket::decode(&secret.decrypt(sealed).unwrap()),
            Ok(packet)
        );

        let datagram = frame_to_client(&secret, &VoicePacket::KeepAlive);
        assert_eq!(
            open_at_client(&secret, &datagram),
            Some(VoicePacket::KeepAlive)
        );
        assert_eq!(open_at_client(&Secret::random(), &datagram), None);
        assert_eq!(open_at_client(&secret, &datagram[1..]), None);
        assert!(open_at_server(&[0x00, 1, 2]).is_err());
    }

    #[test]
    fn the_secret_message_round_trips_and_has_the_documented_layout() {
        let message = SecretMessage {
            secret: Secret([7; 16]),
            connection: 0x1111,
            port: 24454,
            player: 0x2222,
            codec: Codec::Voip,
            mtu_size: 1024,
            distance: 48.0,
            keep_alive: 1000,
            groups_enabled: false,
            voice_host: "voice.example.org:1234".into(),
            allow_recording: true,
        };
        let body = message.encode();
        assert_eq!(SecretMessage::decode(&body), Ok(message));
        assert_eq!(&body[..16], &[7; 16]);
        // The port follows the 16-byte secret and the 16-byte connection id.
        assert_eq!(&body[32..36], &24454_i32.to_be_bytes());
        // 16 + 16 + 4 + 16 + 1 + 4 + 8 + 4 + 1 + (1 + 22) + 1
        assert_eq!(body.len(), 94);
    }

    #[test]
    fn the_voice_address_follows_the_voice_host_setting() {
        let mut message = SecretMessage {
            secret: Secret([0; 16]),
            connection: 0,
            port: 24454,
            player: 0,
            codec: Codec::Voip,
            mtu_size: 1024,
            distance: 48.0,
            keep_alive: 1000,
            groups_enabled: false,
            voice_host: String::new(),
            allow_recording: true,
        };
        assert_eq!(message.voice_address("mc.example"), ("mc.example".into(), 24454));
        message.voice_host = "30000".into();
        assert_eq!(message.voice_address("mc.example"), ("mc.example".into(), 30000));
        message.voice_host = "voice.example".into();
        assert_eq!(message.voice_address("mc.example"), ("voice.example".into(), 24454));
        message.voice_host = "voice.example:5".into();
        assert_eq!(message.voice_address("mc.example"), ("voice.example".into(), 5));
        message.voice_host = "[::1]:7".into();
        assert_eq!(message.voice_address("mc.example"), ("::1".into(), 7));
        assert_eq!(split_host_port("play.example:25565"), ("play.example", Some(25565)));
        assert_eq!(split_host_port("::1"), ("::1", None));
    }

    #[test]
    fn states_round_trip() {
        let mut talking = PlayerState::disconnected(5, "Alex");
        talking.disconnected = false;
        talking.muted = true;
        talking.group = Some(77);
        let states = vec![PlayerState::disconnected(1, "Steve"), talking.clone()];
        assert_eq!(decode_states(&encode_states(&states)), Ok(states));
        assert_eq!(PlayerState::decode(&talking.encode()), Ok(talking));
        // muted (bit 0) and has-group (bit 5).
        assert_eq!(PlayerState::disconnected(1, "S").encode()[0], 0b1_0000);
        // A count the body cannot hold.
        assert!(decode_states(&[0x7f, 0xff, 0xff, 0xff]).is_err());
        assert_eq!(decode_client_state(&encode_client_state(true, false)), Ok((true, false)));
        assert_eq!(decode_remove_state(&encode_remove_state(9)), Ok(9));
        assert_eq!(decode_request_secret(&encode_request_secret(21)), Ok(21));
    }

    #[test]
    fn group_messages_round_trip() {
        let group = ClientGroup {
            id: 0xabc,
            name: "Höhle".into(),
            has_password: true,
            persistent: false,
            hidden: false,
            kind: GroupType::Isolated,
        };
        let body = group.encode();
        assert_eq!(ClientGroup::decode(&body), Ok(group));
        // The type is a big-endian short at the end.
        assert_eq!(&body[body.len() - 2..], &[0, 2]);
        assert_eq!(decode_joined_group(&encode_joined_group(Some(4), false)), Ok((Some(4), false)));
        assert_eq!(encode_joined_group(None, true), [0, 1]);
        assert_eq!(decode_set_group(&encode_set_group(9, Some("pw"))), Ok((9, Some("pw".into()))));
        assert_eq!(decode_set_group(&encode_set_group(9, None)), Ok((9, None)));
        assert_eq!(
            decode_create_group(&encode_create_group("A", None, GroupType::Open)),
            Ok(("A".into(), None, GroupType::Open))
        );
        assert_eq!(decode_remove_group(&encode_remove_group(3)), Ok(3));
        assert!(valid_group_name("Mine crew"));
        assert!(!valid_group_name(" leading"));
        assert!(!valid_group_name(""));
        assert!(!valid_group_name(&"x".repeat(25)));
        assert!(!valid_group_password("a\u{7}"));
        assert!(valid_group_password("geheim 123"));
    }
}
