# The voice protocol

This describes the network protocol the two mods speak. It is the protocol of the
[Simple Voice Chat](https://github.com/henkelmax/simple-voice-chat) mod by Max Henkel, compatibility
version **21** (Simple Voice Chat 2.6.x / 2.7.x for Minecraft 26.3). The code in this repository was
written from this description; it contains no code of that mod.

All numbers are big-endian. `VarInt` is Minecraft's variable-length integer. A `byte array` is a
VarInt length followed by that many bytes, a `string` is a byte array holding UTF-8, and a `UUID` is
16 bytes, most significant first.

## Overview

1. The client joins the game server and sends its compatibility version on a plugin channel.
2. The server answers with a random 16-byte secret, a connection id and its UDP port.
3. The client opens a UDP connection to that port. Every datagram is encrypted with the secret.
4. A two-step handshake proves the client holds the secret and receives packets at its address.
5. The client sends microphone frames; the server relays them to the players in range.

## Plugin channels

Vanilla custom payloads in the play phase. The body is exactly the fields below.

| Channel | Direction | Body |
| --- | --- | --- |
| `voicechat:request_secret` | client to server | `int` compatibility version |
| `voicechat:secret` | server to client | see below |
| `voicechat:states` | server to client | `int` count, then that many player states |
| `voicechat:state` | server to client | one player state |
| `voicechat:remove_state` | server to client | `UUID` of the player who left |
| `voicechat:client_state` | client to server | `bool` muted, `bool` disabled |

Simple Voice Chat has more channels for groups and volume categories (`voicechat:add_group`,
`set_group`, `create_group`, `leave_group`, `joined_group`, `remove_group`, `add_category`,
`remove_category`). This implementation does not use them: the server announces
`groups_enabled = false` and ignores them, and the client ignores them.

### `voicechat:secret`

| Field | Type | Meaning |
| --- | --- | --- |
| secret | 16 bytes | AES-128 key of this connection |
| connection id | UUID | names the connection in every datagram |
| port | int | the server's UDP port |
| player | UUID | the player the secret is for |
| codec | byte | Opus application: 0 VOIP, 1 AUDIO, 2 RESTRICTED_LOWDELAY |
| MTU size | int | largest voice packet, 512 to 2048 |
| distance | double | how far speech carries, in blocks |
| keep alive | int | milliseconds between keep-alives |
| groups enabled | bool | |
| voice host | string | empty, a port, or `host[:port]` |
| allow recording | bool | |

The client sends voice to the address it joined the game with and `port`, unless `voice host` says
otherwise: a number replaces the port, `host` or `host:port` replaces the address.

### Player state

| Field | Type | Meaning |
| --- | --- | --- |
| flags | byte | bit 0 muted, 1 force-muted, 2 disabled, 3 force-disabled, 4 disconnected, 5 has group |
| player | UUID | |
| name | string | at most 256 characters |
| group | UUID | only when bit 5 is set |

When a player joins the game the server creates their state with `disconnected` set and tells every
client. When a client asks for a secret, the server first sends it `voicechat:states` with everyone,
then the secret. `disconnected` is cleared when the UDP handshake completes. Plugin messages are only
sent to clients that asked for a secret with the right version.

## UDP datagrams

| Direction | Layout |
| --- | --- |
| client to server | `0xFF`, connection id (UUID), byte array: encrypted payload |
| server to client | `0xFF`, byte array: encrypted payload |

The encrypted payload is a random 12-byte IV followed by AES-128-GCM ciphertext with its 16-byte
tag (no additional data). A datagram is at most 2048 bytes. The plaintext is one type byte and the
packet's fields:

| Type | Packet | Direction | Fields |
| --- | --- | --- | --- |
| `0x01` | Mic | client to server | byte array Opus frame, `long` sequence, `bool` whispering |
| `0x02` | PlayerSound | server to client | UUID channel, UUID sender, byte array Opus frame, `long` sequence, `float` distance, flags byte, optional category |
| `0x03` | GroupSound | server to client | UUID channel, UUID sender, byte array Opus frame, `long` sequence, flags byte, optional category |
| `0x04` | LocationSound | server to client | UUID channel, UUID sender, three `double` x y z, byte array Opus frame, `long` sequence, `float` distance, flags byte, optional category |
| `0x05` | Authenticate | client to server | none |
| `0x06` | AuthenticateAck | server to client | 16-byte nonce |
| `0x07` | Ping | both | UUID id, `long` timestamp |
| `0x08` | KeepAlive | both | none |
| `0x09` | ConnectionCheck | client to server | 16-byte nonce |
| `0x0A` | ConnectionCheckAck | server to client | none |

Sound packet flags: bit 0 whispering (PlayerSound only), bit 1 a category follows as a string of at
most 16 characters.

### Handshake

1. The client sends `Authenticate` once a second until it receives `AuthenticateAck`.
2. The server answers each one with a nonce that depends on the connection id and the address the
   datagram came from. Only the server can compute it (this implementation uses HMAC-SHA256 with a
   key made at start, truncated to 16 bytes).
3. The client sends `ConnectionCheck` with that nonce once a second until it receives
   `ConnectionCheckAck`.
4. The server accepts the check only when the nonce matches the sender's address, so a connection
   is bound to an address the client really receives at. From then on, voice packets are accepted
   from that address only.

### Keeping alive

The server sends `KeepAlive` every `keep alive` milliseconds, and the client answers each with a
`KeepAlive`. A server that hears nothing for ten intervals forgets the secret and sends the player a
new one on `voicechat:secret`. A client that hears nothing for ten intervals closes its connection
and asks for a secret again. A `Ping` is echoed unchanged.

### Audio

Opus, 48 kHz, mono, 20 ms frames (960 samples), at most 1275 bytes a frame. The sequence number
counts up by one per packet. A `Mic` packet with an empty frame ends a talk burst: the listener
resets its decoder and the next burst may start at any sequence number. Within a burst, a packet
whose sequence number is not above the last one is dropped, and a gap is filled by the decoder's
loss concealment.

### Relaying

For each `Mic` packet the server builds a `PlayerSound` (channel and sender are the speaker's UUID,
distance is the speech or whisper distance) and sends it to every connected player in the same
dimension within `max(distance + 1, packet distance)` blocks of the speaker, except the speaker and
players whose state is `disabled`. The listener hears it at the speaker's position: full volume at
the speaker, falling linearly to silence at `distance` blocks. More than 100 voice packets a second
from one client are dropped.
