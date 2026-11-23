//! Wire format of the voice protocol, shared by the client and the server mod.
//!
//! This is an independent implementation of the network protocol spoken by the
//! "Simple Voice Chat" mod (compatibility version 21), written from the description
//! in `PROTOCOL.md`. It contains no code of that mod.
//!
//! The source of truth lives in `shared/protocol/`; `tools/sync-protocol.sh` copies it
//! into both mods, because a Horizon native mod may not reference files outside its folder.

pub mod crypto;
mod gcm;
pub mod packets;
pub mod wire;

pub use crypto::Secret;
pub use packets::*;

/// The protocol revision both ends must agree on.
pub const COMPATIBILITY_VERSION: i32 = 21;
/// The older revision a server also accepts (Simple Voice Chat 2.6.x). It differs in the secret
/// and state messages and in the handshake: see `encode_v20` and the server's socket code.
pub const LEGACY_VERSION: i32 = 20;
/// Default UDP port of a voice server.
pub const DEFAULT_PORT: u16 = 24454;
/// Audio format: 48 kHz mono, 20 ms Opus frames.
pub const SAMPLE_RATE: u32 = 48_000;
/// Samples in one 20 ms frame.
pub const FRAME_SIZE: usize = 960;
/// Largest Opus frame a packet may carry.
pub const MAX_OPUS_PAYLOAD: usize = 1275;
/// Largest UDP datagram either side accepts.
pub const MAX_PACKET_SIZE: usize = 2048;

/// Plugin channels (vanilla custom payloads) used for the handshake and player states.
pub mod channel {
    /// Client to server: the client's compatibility version.
    pub const REQUEST_SECRET: &str = "voicechat:request_secret";
    /// Server to client: key, connection id and server settings.
    pub const SECRET: &str = "voicechat:secret";
    /// Server to client: one player's state.
    pub const STATE: &str = "voicechat:state";
    /// Server to client: every player's state.
    pub const STATES: &str = "voicechat:states";
    /// Server to client: a player left.
    pub const REMOVE_STATE: &str = "voicechat:remove_state";
    /// Client to server: the client muted or disabled voice chat.
    pub const CLIENT_STATE: &str = "voicechat:client_state";
    /// Server to client: a group exists (or changed).
    pub const ADD_GROUP: &str = "voicechat:add_group";
    /// Server to client: a group is gone.
    pub const REMOVE_GROUP: &str = "voicechat:remove_group";
    /// Server to client: the group the player is in now, or a wrong password.
    pub const JOINED_GROUP: &str = "voicechat:joined_group";
    /// Client to server: join a group.
    pub const SET_GROUP: &str = "voicechat:set_group";
    /// Client to server: create a group and join it.
    pub const CREATE_GROUP: &str = "voicechat:create_group";
    /// Client to server: leave the group.
    pub const LEAVE_GROUP: &str = "voicechat:leave_group";
}

/// Longest group name, in characters.
pub const MAX_GROUP_NAME: usize = 24;
/// Longest group password, in characters.
pub const MAX_GROUP_PASSWORD: usize = 256;
