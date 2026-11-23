//! The microphone and the speakers.
//!
//! One thread owns both audio streams (they cannot move between threads on every platform).
//! The microphone callback cuts the input into 20 ms frames, decides whether the player is
//! talking, and hands encoded frames to the game's main thread. The output callback pulls the
//! mix of everyone talking from the [`Mixer`]. The devices are opened through cpal, except on
//! Linux, where the game's own ALSA library is used (see `sound_alsa.rs`).

#[cfg(target_os = "linux")]
#[path = "sound_alsa.rs"]
mod backend;
#[cfg(not(target_os = "linux"))]
#[path = "sound_cpal.rs"]
mod backend;

use crate::codec::Encoder;
use crate::mixer::Mixer;
use crate::protocol::{Codec, FRAME_SIZE, SAMPLE_RATE};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Frames (20 ms each) voice activation keeps sending after the voice drops below the threshold.
const HANG_FRAMES: u32 = 20;

/// What the main thread sets and the audio callbacks read.
pub struct Controls {
    /// The push-to-talk key is held.
    pub push_to_talk: AtomicBool,
    /// Send whenever the voice is louder than the threshold, instead of on the key.
    pub voice_activation: AtomicBool,
    /// Nothing is sent while true.
    pub muted: AtomicBool,
    /// Voice activation threshold in dB (f32 bits), below 0.
    pub threshold_db: AtomicU32,
    /// Microphone gain (f32 bits), 1 is unchanged.
    pub microphone_volume: AtomicU32,
    /// Whether frames are being sent right now, for the HUD.
    pub talking: AtomicBool,
    /// Whether a microphone was opened.
    pub has_microphone: AtomicBool,
    /// Microphone test: frames go to `tested` (to be played back here), none to `captured`.
    pub monitor: AtomicBool,
    /// Loudness of the last microphone frame in dB (f32 bits), for the level meter.
    pub level_db: AtomicU32,
    running: AtomicBool,
}

impl Controls {
    fn new() -> Self {
        Self {
            push_to_talk: AtomicBool::new(false),
            voice_activation: AtomicBool::new(false),
            muted: AtomicBool::new(false),
            threshold_db: AtomicU32::new((-40f32).to_bits()),
            microphone_volume: AtomicU32::new(1f32.to_bits()),
            talking: AtomicBool::new(false),
            has_microphone: AtomicBool::new(false),
            monitor: AtomicBool::new(false),
            level_db: AtomicU32::new((-100f32).to_bits()),
            running: AtomicBool::new(true),
        }
    }
}

/// Which devices to open; None (or a name no device has) is the system default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Devices {
    pub microphone: Option<String>,
    pub speaker: Option<String>,
}

/// The names of the system's microphones (`input`) or speakers.
pub use backend::device_names;

/// What the output stream asks for: `channels` (1 or 2) interleaved samples at `rate` Hz.
pub type Render = Box<dyn FnMut(&mut [f32], usize, u32) + Send>;
/// Given the microphone's rate, what takes its mono samples.
pub type MakeSink = Box<dyn FnOnce(u32) -> Result<Box<dyn FnMut(f32) + Send>, String> + Send>;

/// What the microphone hands to the main thread.
pub enum Captured {
    /// One encoded 20 ms frame.
    Frame(Vec<u8>),
    /// The talk burst ended.
    End,
}

/// The open audio devices. Dropping it closes them.
pub struct Audio {
    pub controls: Arc<Controls>,
    pub mixer: Arc<Mutex<Mixer>>,
    pub captured: Receiver<Captured>,
    /// Encoded frames of the microphone test.
    pub tested: Receiver<Vec<u8>>,
    thread: Option<JoinHandle<()>>,
}

impl Audio {
    /// Opens the chosen microphone and speakers on a thread of their own. A missing device is
    /// logged and left out: without a microphone the player still hears the others.
    pub fn start_with(codec: Codec, devices: &Devices) -> Self {
        let controls = Arc::new(Controls::new());
        let mixer = Arc::new(Mutex::new(Mixer::new()));
        let (sender, captured) = channel();
        let (test_sender, tested) = channel();
        let devices = devices.clone();
        let thread = {
            let (controls, mixer) = (controls.clone(), mixer.clone());
            std::thread::Builder::new()
                .name("horizon-voice-audio".into())
                .spawn(move || {
                    let render: Render = Box::new(move |out, channels, rate| match mixer.lock() {
                        Ok(mut mixer) => mixer.render(out, channels, rate),
                        Err(_) => out.fill(0.0),
                    });
                    let output = backend::open_output(devices.speaker.as_deref(), render)
                        .map_err(|error| log::warn!("Voice chat has no speakers: {error}"))
                        .ok();
                    let capture_controls = controls.clone();
                    let make: MakeSink = Box::new(move |rate| {
                        let mut capture =
                            Capture::new(codec, rate, capture_controls, (sender, test_sender))
                                .ok_or("the Opus encoder could not be created")?;
                        Ok(Box::new(move |sample| capture.push(sample)))
                    });
                    let input = backend::open_input(devices.microphone.as_deref(), make)
                        .map_err(|error| log::warn!("Voice chat has no microphone: {error}"))
                        .ok();
                    controls.has_microphone.store(input.is_some(), Ordering::SeqCst);
                    // The streams run on the audio system's threads until they are dropped here.
                    while controls.running.load(Ordering::SeqCst) {
                        std::thread::park_timeout(std::time::Duration::from_millis(200));
                    }
                    drop((input, output));
                })
                .map_err(|error| log::warn!("Voice chat could not start its audio thread: {error}"))
                .ok()
        };
        Self {
            controls,
            mixer,
            captured,
            tested,
            thread,
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.controls.running.store(false, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

/// Turns the microphone's samples into encoded frames.
pub struct Capture {
    encoder: Encoder,
    controls: Arc<Controls>,
    sender: Sender<Captured>,
    test_sender: Sender<Vec<u8>>,
    /// Device samples per 48 kHz sample.
    step: f64,
    phase: f64,
    previous: f32,
    frame: [f32; FRAME_SIZE],
    filled: usize,
    /// Frames voice activation still sends after the voice dropped.
    hang: u32,
    sending: bool,
}

impl Capture {
    pub fn new(
        codec: Codec,
        device_rate: u32,
        controls: Arc<Controls>,
        (sender, test_sender): (Sender<Captured>, Sender<Vec<u8>>),
    ) -> Option<Self> {
        Some(Self {
            encoder: Encoder::new(codec)?,
            controls,
            sender,
            test_sender,
            step: f64::from(device_rate.max(1)) / f64::from(SAMPLE_RATE),
            phase: 0.0,
            previous: 0.0,
            frame: [0.0; FRAME_SIZE],
            filled: 0,
            hang: 0,
            sending: false,
        })
    }

    /// One mono sample at the device's rate.
    pub fn push(&mut self, sample: f32) {
        // Resample to 48 kHz by drawing a line between neighbouring samples.
        while self.phase < 1.0 {
            let value = self.previous + (sample - self.previous) * self.phase as f32;
            self.frame[self.filled] = value;
            self.filled += 1;
            if self.filled == FRAME_SIZE {
                self.filled = 0;
                self.finish_frame();
            }
            self.phase += self.step;
        }
        self.phase -= 1.0;
        self.previous = sample;
    }

    fn finish_frame(&mut self) {
        let gain = f32::from_bits(self.controls.microphone_volume.load(Ordering::Relaxed));
        let mut energy = 0.0;
        for sample in &mut self.frame {
            *sample = (*sample * gain).clamp(-1.0, 1.0);
            energy += *sample * *sample;
        }
        let level_db = 10.0 * (energy / FRAME_SIZE as f32).max(1e-10).log10();
        self.controls
            .level_db
            .store(level_db.to_bits(), Ordering::Relaxed);
        let threshold = f32::from_bits(self.controls.threshold_db.load(Ordering::Relaxed));
        let voiced = if level_db >= threshold {
            self.hang = HANG_FRAMES;
            true
        } else if self.hang > 0 {
            self.hang -= 1;
            true
        } else {
            false
        };
        let voice_activation = self.controls.voice_activation.load(Ordering::Relaxed);
        if self.controls.monitor.load(Ordering::Relaxed) {
            // The microphone test plays back what voice activation lets through, or everything.
            if self.sending {
                let _ = self.sender.send(Captured::End);
                self.sending = false;
            }
            let pass = !voice_activation || voiced;
            if pass {
                if let Some(data) = self.encoder.encode(&self.frame) {
                    let _ = self.test_sender.send(data);
                }
            }
            self.controls.talking.store(pass, Ordering::Relaxed);
            return;
        }
        let wanted = if self.controls.muted.load(Ordering::Relaxed) {
            false
        } else if voice_activation {
            voiced
        } else {
            self.controls.push_to_talk.load(Ordering::Relaxed)
        };
        if wanted {
            if !self.sending {
                self.encoder.reset();
            }
            if let Some(data) = self.encoder.encode(&self.frame) {
                let _ = self.sender.send(Captured::Frame(data));
            }
        } else if self.sending {
            self.hang = 0;
            let _ = self.sender.send(Captured::End);
        }
        self.sending = wanted;
        self.controls.talking.store(wanted, Ordering::Relaxed);
    }
}

#[cfg(test)]
impl Controls {
    pub fn for_tests() -> Arc<Self> {
        let controls = Self::new();
        controls.has_microphone.store(true, Ordering::SeqCst);
        Arc::new(controls)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speak(capture: &mut Capture, rate: u32, millis: u32, amplitude: f32) {
        for i in 0..rate * millis / 1000 {
            let t = i as f32 / rate as f32;
            capture.push((t * 300.0 * std::f32::consts::TAU).sin() * amplitude);
        }
    }

    fn drain(captured: &Receiver<Captured>) -> (usize, usize) {
        let (mut frames, mut ends) = (0, 0);
        for item in captured.try_iter() {
            match item {
                Captured::Frame(_) => frames += 1,
                Captured::End => ends += 1,
            }
        }
        (frames, ends)
    }

    #[test]
    fn push_to_talk_sends_only_while_held_and_ends_the_burst_once() {
        let controls = Controls::for_tests();
        let (sender, captured) = channel();
        let mut capture =
            Capture::new(Codec::Voip, 48_000, controls.clone(), (sender, channel().0)).unwrap();
        speak(&mut capture, 48_000, 100, 0.5);
        assert_eq!(drain(&captured), (0, 0));
        controls.push_to_talk.store(true, Ordering::SeqCst);
        speak(&mut capture, 48_000, 100, 0.5);
        assert_eq!(drain(&captured), (5, 0));
        assert!(controls.talking.load(Ordering::SeqCst));
        controls.push_to_talk.store(false, Ordering::SeqCst);
        speak(&mut capture, 48_000, 100, 0.5);
        assert_eq!(drain(&captured), (0, 1));
        // Muted wins over the key.
        controls.push_to_talk.store(true, Ordering::SeqCst);
        controls.muted.store(true, Ordering::SeqCst);
        speak(&mut capture, 48_000, 100, 0.5);
        assert_eq!(drain(&captured), (0, 0));
    }

    #[test]
    fn voice_activation_follows_the_voice_with_a_short_tail() {
        let controls = Controls::for_tests();
        controls.voice_activation.store(true, Ordering::SeqCst);
        let (sender, captured) = channel();
        let mut capture = Capture::new(Codec::Voip, 48_000, controls, (sender, channel().0)).unwrap();
        // Room noise at -60 dB stays below the -40 dB threshold.
        speak(&mut capture, 48_000, 200, 0.001);
        assert_eq!(drain(&captured), (0, 0));
        speak(&mut capture, 48_000, 200, 0.5);
        assert_eq!(drain(&captured), (10, 0));
        // One second of quiet: the 400 ms tail is sent, then the burst ends.
        speak(&mut capture, 48_000, 1000, 0.001);
        assert_eq!(drain(&captured), (HANG_FRAMES as usize, 1));
    }

    #[test]
    fn other_microphone_rates_are_resampled_to_twenty_millisecond_frames() {
        for rate in [16_000, 44_100, 96_000] {
            let controls = Controls::for_tests();
            controls.push_to_talk.store(true, Ordering::SeqCst);
            let (sender, captured) = channel();
            let mut capture = Capture::new(Codec::Voip, rate, controls, (sender, channel().0)).unwrap();
            speak(&mut capture, rate, 1000, 0.5);
            let (frames, _) = drain(&captured);
            assert!((49..=50).contains(&frames), "{rate} Hz gave {frames} frames a second");
        }
    }
}
