//! Opus, 48 kHz mono, 20 ms frames: the audio format of the voice protocol.
//!
//! `unsafe-libopus` is libopus translated to Rust, with libopus's C-style API. These two
//! wrappers own the encoder and decoder states and keep the unsafe calls in one place.

use crate::protocol::{Codec, FRAME_SIZE, MAX_OPUS_PAYLOAD, SAMPLE_RATE};
use unsafe_libopus::{
    OPUS_APPLICATION_AUDIO, OPUS_APPLICATION_RESTRICTED_LOWDELAY, OPUS_APPLICATION_VOIP,
    OPUS_RESET_STATE, OPUS_SET_BITRATE_REQUEST, OpusDecoder, OpusEncoder, opus_decode_float,
    opus_decoder_create, opus_decoder_ctl, opus_decoder_destroy, opus_encode_float,
    opus_encoder_create, opus_encoder_ctl, opus_encoder_destroy,
};

/// Bits per second of encoded speech.
const BITRATE: i32 = 32_000;

/// Encodes microphone frames.
pub struct Encoder(*mut OpusEncoder);

// The state is plain heap memory used by one thread at a time.
unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new(codec: Codec) -> Option<Self> {
        let application = match codec {
            Codec::Voip => OPUS_APPLICATION_VOIP,
            Codec::Audio => OPUS_APPLICATION_AUDIO,
            Codec::RestrictedLowDelay => OPUS_APPLICATION_RESTRICTED_LOWDELAY,
        };
        let mut error = 0;
        // SAFETY: the arguments are a supported rate and channel count; a null state is refused.
        let state = unsafe { opus_encoder_create(SAMPLE_RATE as i32, 1, application, &mut error) };
        if state.is_null() || error != 0 {
            return None;
        }
        // SAFETY: `state` is a live encoder.
        unsafe { opus_encoder_ctl!(state, OPUS_SET_BITRATE_REQUEST, BITRATE) };
        Some(Self(state))
    }

    /// One frame of [`FRAME_SIZE`] samples in -1..1; None when the encoder refuses it.
    pub fn encode(&mut self, frame: &[f32; FRAME_SIZE]) -> Option<Vec<u8>> {
        let mut out = vec![0u8; MAX_OPUS_PAYLOAD];
        // SAFETY: `frame` holds FRAME_SIZE samples and `out` holds the length passed.
        let len = unsafe {
            opus_encode_float(
                self.0,
                frame.as_ptr(),
                FRAME_SIZE as i32,
                out.as_mut_ptr(),
                out.len() as i32,
            )
        };
        if len <= 0 {
            return None;
        }
        out.truncate(len as usize);
        Some(out)
    }

    /// Forgets the previous frames, before a new talk burst.
    pub fn reset(&mut self) {
        // SAFETY: `self.0` is a live encoder.
        unsafe { opus_encoder_ctl!(self.0, OPUS_RESET_STATE) };
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: created by `opus_encoder_create`, destroyed once.
        unsafe { opus_encoder_destroy(self.0) }
    }
}

/// Decodes one speaker's frames.
pub struct Decoder(*mut OpusDecoder);

// The state is plain heap memory used by one thread at a time.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new() -> Option<Self> {
        let mut error = 0;
        // SAFETY: the arguments are a supported rate and channel count; a null state is refused.
        let state = unsafe { opus_decoder_create(SAMPLE_RATE as i32, 1, &mut error) };
        (!state.is_null() && error == 0).then_some(Self(state))
    }

    /// The samples of one received frame; None for bytes that are not an Opus frame.
    pub fn decode(&mut self, data: &[u8]) -> Option<Vec<f32>> {
        if data.is_empty() || data.len() > MAX_OPUS_PAYLOAD {
            return None;
        }
        self.run(data.as_ptr(), data.len() as i32)
    }

    /// A frame made up to cover one that was lost.
    pub fn conceal(&mut self) -> Option<Vec<f32>> {
        self.run(std::ptr::null(), 0)
    }

    fn run(&mut self, data: *const u8, len: i32) -> Option<Vec<f32>> {
        // Room for the longest Opus frame (120 ms), so a peer's odd frame size cannot overflow.
        let mut out = vec![0f32; FRAME_SIZE * 6];
        let capacity = if data.is_null() { FRAME_SIZE } else { out.len() };
        // SAFETY: `data` is null or holds `len` bytes, and `out` holds at least `capacity` samples.
        let samples =
            unsafe { opus_decode_float(self.0, data, len, out.as_mut_ptr(), capacity as i32, 0) };
        if samples <= 0 {
            return None;
        }
        out.truncate(samples as usize);
        Some(out)
    }

    /// Forgets the previous frames, after a talk burst ended.
    pub fn reset(&mut self) {
        // SAFETY: `self.0` is a live decoder.
        unsafe { opus_decoder_ctl!(self.0, OPUS_RESET_STATE) };
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: created by `opus_decoder_create`, destroyed once.
        unsafe { opus_decoder_destroy(self.0) }
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
