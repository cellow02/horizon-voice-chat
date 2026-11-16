//! Opus, 48 kHz mono, 20 ms frames: the audio format of the voice protocol.
//!
//! `rusty-opus` is the Opus implementation Horizon's lock file offers to mods. These two wrappers
//! give it the interface the rest of the mod uses.

use crate::protocol::{Codec, FRAME_SIZE, MAX_OPUS_PAYLOAD, SAMPLE_RATE};
use rusty_opus::{Application, OpusDecoder, OpusEncoder};

/// Bits per second of encoded speech.
const BITRATE: i32 = 32_000;

/// Encodes microphone frames.
pub struct Encoder {
    state: OpusEncoder,
    codec: Codec,
}

fn encoder(codec: Codec) -> Option<OpusEncoder> {
    let application = match codec {
        Codec::Voip => Application::Voip,
        Codec::Audio => Application::Audio,
        Codec::RestrictedLowDelay => Application::RestrictedLowDelay,
    };
    let mut state = OpusEncoder::new(SAMPLE_RATE as i32, 1, application).ok()?;
    state.bitrate_bps = BITRATE;
    Some(state)
}

impl Encoder {
    pub fn new(codec: Codec) -> Option<Self> {
        Some(Self {
            state: encoder(codec)?,
            codec,
        })
    }

    /// One frame of [`FRAME_SIZE`] samples in -1..1; None when the encoder refuses it.
    pub fn encode(&mut self, frame: &[f32; FRAME_SIZE]) -> Option<Vec<u8>> {
        let mut out = vec![0u8; MAX_OPUS_PAYLOAD];
        let len = self.state.encode(frame, FRAME_SIZE, &mut out).ok()?;
        if len == 0 {
            return None;
        }
        out.truncate(len);
        Some(out)
    }

    /// Forgets the previous frames, before a new talk burst.
    pub fn reset(&mut self) {
        if let Some(state) = encoder(self.codec) {
            self.state = state;
        }
    }
}

/// Decodes one speaker's frames.
pub struct Decoder(OpusDecoder);

impl Decoder {
    pub fn new() -> Option<Self> {
        OpusDecoder::new(SAMPLE_RATE as i32, 1).ok().map(Self)
    }

    /// The samples of one received frame; None for bytes that are not an Opus frame.
    pub fn decode(&mut self, data: &[u8]) -> Option<Vec<f32>> {
        if data.is_empty() || data.len() > MAX_OPUS_PAYLOAD {
            return None;
        }
        // Room for the longest Opus frame (120 ms), so a peer's odd frame size cannot overflow.
        self.run(data, FRAME_SIZE * 6)
    }

    /// A frame made up to cover one that was lost.
    pub fn conceal(&mut self) -> Option<Vec<f32>> {
        self.run(&[], FRAME_SIZE)
    }

    fn run(&mut self, data: &[u8], capacity: usize) -> Option<Vec<f32>> {
        let mut out = vec![0f32; capacity];
        let samples = self.0.decode(data, capacity, &mut out).ok()?;
        if samples == 0 {
            return None;
        }
        out.truncate(samples);
        Some(out)
    }

    /// Forgets the previous frames, after a talk burst ended.
    pub fn reset(&mut self) {
        if let Some(state) = Self::new() {
            *self = state;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frame: usize) -> [f32; FRAME_SIZE] {
        std::array::from_fn(|i| {
            let t = (frame * FRAME_SIZE + i) as f32 / SAMPLE_RATE as f32;
            (t * 440.0 * std::f32::consts::TAU).sin() * 0.5
        })
    }

    fn energy(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn a_tone_survives_encoding_and_decoding() {
        let mut encoder = Encoder::new(Codec::Voip).unwrap();
        let mut decoder = Decoder::new().unwrap();
        let mut last = Vec::new();
        for frame in 0..10 {
            let data = encoder.encode(&tone(frame)).unwrap();
            assert!(data.len() < 400, "about 80 bytes at 32 kbit/s, got {}", data.len());
            last = decoder.decode(&data).unwrap();
            assert_eq!(last.len(), FRAME_SIZE);
        }
        // A 0.5 amplitude sine has an RMS of about 0.35.
        let rms = energy(&last);
        assert!((0.25..0.45).contains(&rms), "rms {rms}");
    }

    #[test]
    fn a_lost_frame_is_concealed_and_garbage_is_refused() {
        let mut encoder = Encoder::new(Codec::Voip).unwrap();
        let mut decoder = Decoder::new().unwrap();
        for frame in 0..5 {
            decoder.decode(&encoder.encode(&tone(frame)).unwrap()).unwrap();
        }
        assert_eq!(decoder.conceal().unwrap().len(), FRAME_SIZE);
        assert!(decoder.decode(&[]).is_none());
        assert!(decoder.decode(&[0u8; 2000]).is_none());
        decoder.reset();
        encoder.reset();
        assert!(decoder.decode(&encoder.encode(&tone(0)).unwrap()).is_some());
    }
}
