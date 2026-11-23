//! Mixes everyone who is talking into the stereo output, by where they stand.
//!
//! The game's main thread feeds received frames and positions in; the audio output thread
//! pulls mixed samples out. Nothing here touches the game or the audio device.

use crate::codec::Decoder;
use crate::net::{Place, Sound};
use crate::protocol::SAMPLE_RATE;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Samples buffered before a speaker starts playing: 60 ms absorbs uneven packet arrival.
const START_SAMPLES: usize = SAMPLE_RATE as usize * 60 / 1000;
/// Most samples kept per speaker; older ones are dropped so delay cannot build up.
const MAX_SAMPLES: usize = SAMPLE_RATE as usize / 2;
/// Lost frames in a row that are concealed; a longer gap is left as silence.
const MAX_CONCEALED: i64 = 5;
/// A speaker is forgotten after this long without a frame.
const FORGET_AFTER: Duration = Duration::from_secs(30);
/// A speaker counts as talking for this long after their last frame.
const TALKING_FOR: Duration = Duration::from_millis(300);

/// Where the listener is and which way they look.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Listener {
    pub position: [f64; 3],
    /// Minecraft yaw in degrees: 0 looks south (+z), 90 looks west (-x).
    pub yaw: f32,
}

struct Speaker {
    decoder: Decoder,
    samples: VecDeque<f32>,
    /// Position between `samples[0]` and `samples[1]`, for output rates other than 48 kHz.
    fraction: f64,
    playing: bool,
    last_sequence: i64,
    last_heard: Instant,
    sender: u128,
    place: Place,
    /// Where the sending player is, when the game knows; updated every tick.
    position: Option<[f64; 3]>,
}

/// The left and right gain of a sound at `at` that carries `distance` blocks.
pub fn gains(listener: &Listener, at: [f64; 3], distance: f32) -> [f32; 2] {
    let offset = [
        at[0] - listener.position[0],
        at[1] - listener.position[1],
        at[2] - listener.position[2],
    ];
    let away = offset.iter().map(|axis| axis * axis).sum::<f64>().sqrt() as f32;
    let distance = distance.max(1.0);
    // Linear falloff: full volume at the speaker, silent at the hearing distance.
    let volume = 1.0 - away.min(distance) / distance;
    let flat = (offset[0] * offset[0] + offset[2] * offset[2]).sqrt();
    if flat < 0.5 {
        // Beside or inside the listener's head: no direction.
        return [volume, volume];
    }
    let yaw = f64::from(listener.yaw).to_radians();
    // The listener's right-hand direction on the ground: looking south (+z), right is west (-x).
    let right = [-yaw.cos(), -yaw.sin()];
    let pan = ((offset[0] * right[0] + offset[2] * right[1]) / flat) as f32;
    // Up to 70 % quieter in the ear facing away, so a voice never vanishes from one ear.
    [
        volume * (1.0 - 0.7 * pan.max(0.0)),
        volume * (1.0 + 0.7 * pan.min(0.0)),
    ]
}

/// Everyone who is talking, and the listener.
#[derive(Default)]
pub struct Mixer {
    speakers: HashMap<u128, Speaker>,
    pub listener: Listener,
    /// Output volume, 0 to 2.
    pub volume: f32,
    /// Nothing is played while true (the player turned voice chat off).
    pub deafened: bool,
    /// Left and right by direction; off plays everyone in the middle.
    pub stereo: bool,
    /// Each player's own volume, 1 when not set.
    pub player_volumes: HashMap<u128, f32>,
}

impl Mixer {
    pub fn new() -> Self {
        Self {
            volume: 1.0,
            stereo: true,
            ..Self::default()
        }
    }

    /// Decodes one received frame into its speaker's buffer.
    pub fn feed(&mut self, sound: &Sound) {
        let now = Instant::now();
        let speaker = match self.speakers.entry(sound.channel) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let Some(decoder) = Decoder::new() else {
                    return;
                };
                entry.insert(Speaker {
                    decoder,
                    samples: VecDeque::new(),
                    fraction: 0.0,
                    playing: false,
                    last_sequence: -1,
                    last_heard: now,
                    sender: sound.sender,
                    place: sound.place,
                    position: None,
                })
            }
        };
        speaker.sender = sound.sender;
        speaker.place = sound.place;
        if sound.data.is_empty() {
            // The talk burst ended: what is buffered plays out, the next burst starts clean.
            speaker.decoder.reset();
            speaker.last_sequence = -1;
            return;
        }
        if speaker.last_sequence >= 0 {
            if sound.sequence <= speaker.last_sequence {
                // Late or repeated.
                return;
            }
            let lost = sound.sequence - speaker.last_sequence - 1;
            if lost <= MAX_CONCEALED {
                for _ in 0..lost {
                    if let Some(samples) = speaker.decoder.conceal() {
                        speaker.samples.extend(samples);
                    }
                }
            }
        }
        speaker.last_sequence = sound.sequence;
        speaker.last_heard = now;
        if let Some(samples) = speaker.decoder.decode(&sound.data) {
            speaker.samples.extend(samples);
        }
        if speaker.samples.len() > MAX_SAMPLES {
            let excess = speaker.samples.len() - MAX_SAMPLES;
            speaker.samples.drain(..excess);
        }
    }

    /// The players whose positions the mixer wants this tick.
    pub fn senders(&self) -> Vec<u128> {
        self.speakers.values().map(|speaker| speaker.sender).collect()
    }

    /// Where a talking player is now; None when the game does not know them.
    pub fn set_position(&mut self, sender: u128, position: Option<[f64; 3]>) {
        for speaker in self.speakers.values_mut() {
            if speaker.sender == sender && position.is_some() {
                speaker.position = position;
            }
        }
    }

    /// The players heard within the last moments, for the HUD.
    pub fn talking(&self) -> Vec<u128> {
        let mut talking: Vec<u128> = self
            .speakers
            .values()
            .filter(|speaker| speaker.last_heard.elapsed() < TALKING_FOR)
            .map(|speaker| speaker.sender)
            .collect();
        talking.sort_unstable();
        talking.dedup();
        talking
    }

    /// Forgets speakers who have been quiet for a long time.
    pub fn forget_idle(&mut self) {
        self.speakers
            .retain(|_, speaker| speaker.last_heard.elapsed() < FORGET_AFTER);
    }

    /// Forgets everyone (the connection closed).
    pub fn clear(&mut self) {
        self.speakers.clear();
    }

    /// Fills `out` (interleaved, `channels` per frame, at `rate` Hz) with the mix.
    pub fn render(&mut self, out: &mut [f32], channels: usize, rate: u32) {
        out.fill(0.0);
        if channels == 0 {
            return;
        }
        let step = f64::from(SAMPLE_RATE) / f64::from(rate.max(1));
        let master = if self.deafened { 0.0 } else { self.volume };
        for speaker in self.speakers.values_mut() {
            if !speaker.playing {
                if speaker.samples.len() < START_SAMPLES {
                    continue;
                }
                speaker.playing = true;
                speaker.fraction = 0.0;
            }
            let player = self.player_volumes.get(&speaker.sender).copied().unwrap_or(1.0);
            let [left, right] = match (speaker.place, speaker.position) {
                (Place::Everywhere, _) => [1.0, 1.0],
                (Place::Location { at, distance }, _) => gains(&self.listener, at, distance),
                (Place::Player { distance }, Some(at)) => gains(&self.listener, at, distance),
                // A player the game does not show us: heard, without distance or direction.
                (Place::Player { .. }, None) => [0.6, 0.6],
            };
            let [left, right] = if self.stereo {
                [left * player, right * player]
            } else {
                let both = (left + right) * 0.5 * player;
                [both, both]
            };
            for frame in out.chunks_exact_mut(channels) {
                if speaker.samples.len() < 2 {
                    // Ran dry: wait for a full start buffer again.
                    speaker.playing = false;
                    speaker.samples.clear();
                    break;
                }
                let mix = speaker.fraction as f32;
                let sample = speaker.samples[0] * (1.0 - mix) + speaker.samples[1] * mix;
                if channels == 1 {
                    frame[0] += sample * master * (left + right) * 0.5;
                } else {
                    frame[0] += sample * master * left;
                    frame[1] += sample * master * right;
                }
                speaker.fraction += step;
                while speaker.fraction >= 1.0 && !speaker.samples.is_empty() {
                    speaker.samples.pop_front();
                    speaker.fraction -= 1.0;
                }
            }
        }
        for sample in out.iter_mut() {
            *sample = sample.clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::Encoder;
    use crate::protocol::{Codec, FRAME_SIZE};

    fn frames(count: usize) -> Vec<Vec<u8>> {
        let mut encoder = Encoder::new(Codec::Voip).unwrap();
        (0..count)
            .map(|frame| {
                let pcm: [f32; FRAME_SIZE] = std::array::from_fn(|i| {
                    let t = (frame * FRAME_SIZE + i) as f32 / SAMPLE_RATE as f32;
                    (t * 440.0 * std::f32::consts::TAU).sin() * 0.5
                });
                encoder.encode(&pcm).unwrap()
            })
            .collect()
    }

    fn sound(data: &[u8], sequence: i64, place: Place) -> Sound {
        Sound {
            channel: 7,
            sender: 7,
            data: data.to_vec(),
            sequence,
            place,
            whispering: false,
        }
    }

    fn loudness(samples: &[f32], channel: usize) -> f32 {
        let picked: Vec<f32> = samples.chunks(2).map(|frame| frame[channel]).collect();
        (picked.iter().map(|s| s * s).sum::<f32>() / picked.len() as f32).sqrt()
    }

    #[test]
    fn volume_falls_with_distance_and_direction_picks_the_ear() {
        // Looking south (+z): west (-x) is to the right.
        let listener = Listener::default();
        assert_eq!(gains(&listener, [0.0, 0.0, 0.0], 48.0), [1.0, 1.0]);
        let [l, r] = gains(&listener, [0.0, 0.0, 24.0], 48.0);
        assert!((l - 0.5).abs() < 1e-5 && (r - 0.5).abs() < 1e-5, "ahead: {l} {r}");
        assert_eq!(gains(&listener, [0.0, 0.0, 48.0], 48.0), [0.0, 0.0]);
        assert_eq!(gains(&listener, [0.0, 0.0, 500.0], 48.0), [0.0, 0.0]);
        let [l, r] = gains(&listener, [-10.0, 0.0, 0.0], 48.0);
        assert!(r > l * 2.0, "to the right: {l} {r}");
        let [l, r] = gains(&listener, [10.0, 0.0, 0.0], 48.0);
        assert!(l > r * 2.0, "to the left: {l} {r}");
        // Turned to look west (-x): what was to the right is now ahead.
        let turned = Listener { yaw: 90.0, ..listener };
        let [l, r] = gains(&turned, [-10.0, 0.0, 0.0], 48.0);
        assert!((l - r).abs() < 1e-4, "ahead after turning: {l} {r}");
        // North (-z) is to the right when looking west.
        let [l, r] = gains(&turned, [0.0, 0.0, -10.0], 48.0);
        assert!(r > l * 2.0, "north while looking west: {l} {r}");
    }

    #[test]
    fn speech_plays_after_the_start_buffer_and_from_the_right_side() {
        let mut mixer = Mixer::new();
        let place = Place::Player { distance: 48.0 };
        let data = frames(10);
        let mut out = vec![0f32; FRAME_SIZE * 2];

        // Two frames (40 ms) are below the 60 ms start buffer: silence.
        mixer.feed(&sound(&data[0], 0, place));
        mixer.feed(&sound(&data[1], 1, place));
        mixer.set_position(7, Some([-5.0, 0.0, 0.0]));
        mixer.render(&mut out, 2, SAMPLE_RATE);
        assert!(out.iter().all(|s| *s == 0.0));

        for (sequence, frame) in data.iter().enumerate().skip(2) {
            mixer.feed(&sound(frame, sequence as i64, place));
        }
        assert_eq!(mixer.talking(), [7]);
        // Skip the codec's start-up, then listen.
        mixer.render(&mut out, 2, SAMPLE_RATE);
        mixer.render(&mut out, 2, SAMPLE_RATE);
        let (left, right) = (loudness(&out, 0), loudness(&out, 1));
        assert!(right > 0.05, "audible on the right: {right}");
        assert!(right > left * 2.0, "the speaker stands to the right: {left} {right}");

        // Deafened: silence, and the buffer still drains.
        mixer.deafened = true;
        mixer.render(&mut out, 2, SAMPLE_RATE);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn late_frames_are_dropped_and_gaps_are_concealed() {
        let mut mixer = Mixer::new();
        let data = frames(6);
        mixer.feed(&sound(&data[0], 0, Place::Everywhere));
        mixer.feed(&sound(&data[1], 1, Place::Everywhere));
        let buffered = |mixer: &Mixer| mixer.speakers[&7].samples.len();
        assert_eq!(buffered(&mixer), 2 * FRAME_SIZE);
        // A repeat and a late frame add nothing.
        mixer.feed(&sound(&data[1], 1, Place::Everywhere));
        mixer.feed(&sound(&data[0], 0, Place::Everywhere));
        assert_eq!(buffered(&mixer), 2 * FRAME_SIZE);
        // Frame 2 is lost: frame 3 brings a concealed frame with it.
        mixer.feed(&sound(&data[3], 3, Place::Everywhere));
        assert_eq!(buffered(&mixer), 4 * FRAME_SIZE);
        // The end of the burst resets the sequence, so the next burst may start at any number.
        mixer.feed(&sound(&[], 4, Place::Everywhere));
        mixer.feed(&sound(&data[4], 0, Place::Everywhere));
        assert_eq!(buffered(&mixer), 5 * FRAME_SIZE);
        // The buffer never grows beyond half a second.
        for sequence in 1..100 {
            mixer.feed(&sound(&data[5], sequence, Place::Everywhere));
        }
        assert_eq!(buffered(&mixer), MAX_SAMPLES);
    }

    #[test]
    fn other_output_rates_and_mono_are_resampled() {
        let mut mixer = Mixer::new();
        for (sequence, frame) in frames(10).iter().enumerate() {
            mixer.feed(&sound(frame, sequence as i64, Place::Everywhere));
        }
        // 20 ms at 44.1 kHz mono takes 20 ms of the 48 kHz buffer.
        let mut out = vec![0f32; 882];
        mixer.render(&mut out, 1, 44_100);
        let left = mixer.speakers[&7].samples.len();
        assert!((left as i64 - 9 * FRAME_SIZE as i64).abs() <= 2, "left {left}");
        mixer.render(&mut out, 1, 44_100);
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms > 0.1, "rms {rms}");
    }
}
