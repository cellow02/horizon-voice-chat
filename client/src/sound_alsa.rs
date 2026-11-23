//! Microphone and speakers through ALSA, on Linux.
//!
//! The prebuilt game already links ALSA (through its own cpal and `alsa-sys`), and the launcher
//! refuses a mod that would compile a second provider of a native library the game links. So
//! this mod uses no `-sys` crate: it opens the system's `libasound.so.2` when the audio starts,
//! the same library the game has loaded, and calls the few functions it needs. Each stream runs
//! on a thread of its own with blocking reads and writes of 10 ms.

use super::{MakeSink, Render};
use libloading::Library;
use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

const PLAYBACK: c_int = 0;
const CAPTURE: c_int = 1;
const FORMAT_S16_LE: c_int = 2;
const ACCESS_RW_INTERLEAVED: c_int = 3;
const RATE: u32 = 48_000;
/// Frames per read or write: 10 ms.
const PERIOD: usize = (RATE / 100) as usize;

type Pcm = c_void;

/// The functions of libasound this mod calls.
struct Alsa {
    open: unsafe extern "C" fn(*mut *mut Pcm, *const c_char, c_int, c_int) -> c_int,
    set_params: unsafe extern "C" fn(*mut Pcm, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int,
    readi: unsafe extern "C" fn(*mut Pcm, *mut c_void, c_ulong) -> c_long,
    writei: unsafe extern "C" fn(*mut Pcm, *const c_void, c_ulong) -> c_long,
    recover: unsafe extern "C" fn(*mut Pcm, c_int, c_int) -> c_int,
    close: unsafe extern "C" fn(*mut Pcm) -> c_int,
    strerror: unsafe extern "C" fn(c_int) -> *const c_char,
    name_hint: unsafe extern "C" fn(c_int, *const c_char, *mut *mut *mut c_void) -> c_int,
    get_hint: unsafe extern "C" fn(*const c_void, *const c_char) -> *mut c_char,
    free_hint: unsafe extern "C" fn(*mut *mut c_void) -> c_int,
    /// Kept open while the function pointers above are used.
    _library: Library,
}

impl Alsa {
    fn load() -> Result<Arc<Self>, String> {
        // SAFETY: libasound's initialisers have no requirements; the symbols' types are those of
        // alsa-lib's headers (pcm.h, control.h, error.h).
        unsafe {
            let library = Library::new("libasound.so.2")
                .map_err(|e| format!("ALSA (libasound.so.2) is not installed: {e}"))?;
            macro_rules! get {
                ($name:literal) => {
                    *library
                        .get(concat!($name, "\0").as_bytes())
                        .map_err(|e| format!("libasound has no {}: {e}", $name))?
                };
            }
            Ok(Arc::new(Self {
                open: get!("snd_pcm_open"),
                set_params: get!("snd_pcm_set_params"),
                readi: get!("snd_pcm_readi"),
                writei: get!("snd_pcm_writei"),
                recover: get!("snd_pcm_recover"),
                close: get!("snd_pcm_close"),
                strerror: get!("snd_strerror"),
                name_hint: get!("snd_device_name_hint"),
                get_hint: get!("snd_device_name_get_hint"),
                free_hint: get!("snd_device_name_free_hint"),
                _library: library,
            }))
        }
    }

    fn error(&self, code: c_int) -> String {
        // SAFETY: snd_strerror returns a static string for any code.
        unsafe {
            let text = (self.strerror)(code);
            if text.is_null() {
                format!("ALSA error {code}")
            } else {
                CStr::from_ptr(text).to_string_lossy().into_owned()
            }
        }
    }

    /// Opens `name` (or "default") with the first of `layouts` (channels, rate) it takes.
    fn open_pcm(&self, name: &str, stream: c_int, layouts: &[(u32, u32)]) -> Result<Handle, String> {
        let device = CString::new(name).map_err(|_| "bad device name".to_owned())?;
        let mut pcm = std::ptr::null_mut();
        // SAFETY: `pcm` receives the handle; the name outlives the call.
        let code = unsafe { (self.open)(&mut pcm, device.as_ptr(), stream, 0) };
        if code < 0 {
            return Err(format!("{name}: {}", self.error(code)));
        }
        let mut last = 0;
        for &(channels, rate) in layouts {
            // 16-bit interleaved, resampled by ALSA where the device needs it, 60 ms of latency.
            // SAFETY: `pcm` is open.
            last = unsafe {
                (self.set_params)(pcm, FORMAT_S16_LE, ACCESS_RW_INTERLEAVED, channels, rate, 1, 60_000)
            };
            if last >= 0 {
                return Ok(Handle { pcm, channels: channels as usize, rate });
            }
        }
        // SAFETY: closes the handle opened above.
        unsafe { (self.close)(pcm) };
        Err(format!("{name}: {}", self.error(last)))
    }

    /// The PCM device names ALSA lists for capture (`input`) or playback.
    fn names(&self, input: bool) -> Vec<String> {
        let mut hints: *mut *mut c_void = std::ptr::null_mut();
        let mut names = Vec::new();
        // SAFETY: ALSA allocates `hints`, a null-terminated array freed below; the strings it
        // hands out are malloc'd and freed with libc's free.
        unsafe {
            if (self.name_hint)(-1, c"pcm".as_ptr(), &mut hints) < 0 || hints.is_null() {
                return names;
            }
            let take = |hint: *const c_void, id: &CStr| {
                let text = (self.get_hint)(hint, id.as_ptr());
                if text.is_null() {
                    return None;
                }
                let value = CStr::from_ptr(text).to_string_lossy().into_owned();
                libc::free(text.cast());
                Some(value)
            };
            let mut at = hints;
            while !(*at).is_null() {
                let hint = (*at).cast_const();
                // No IOID: the device does both.
                let direction = take(hint, c"IOID");
                let wanted = match direction.as_deref() {
                    Some("Input") => input,
                    Some("Output") => !input,
                    _ => true,
                };
                if wanted {
                    if let Some(name) = take(hint, c"NAME").filter(|name| name != "null") {
                        names.push(name);
                    }
                }
                at = at.add(1);
            }
            (self.free_hint)(hints);
        }
        names.dedup();
        names
    }
}

/// An open PCM.
struct Handle {
    pcm: *mut Pcm,
    channels: usize,
    rate: u32,
}

// SAFETY: an ALSA handle may be used from any thread, one at a time; each is used by one thread.
unsafe impl Send for Handle {}

/// An open stream; dropping it stops its thread and closes the device.
pub struct Stream {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The names of the system's microphones (`input`) or speakers.
pub fn device_names(input: bool) -> Vec<String> {
    match Alsa::load() {
        Ok(alsa) => alsa.names(input),
        Err(error) => {
            log::warn!("Voice chat: {error}");
            Vec::new()
        }
    }
}

/// Opens the named device, or the default one when it has no name or fails to open.
fn open(alsa: &Alsa, name: Option<&str>, stream: c_int, layouts: &[(u32, u32)]) -> Result<Handle, String> {
    if let Some(name) = name.filter(|name| *name != "default") {
        match alsa.open_pcm(name, stream, layouts) {
            Ok(handle) => return Ok(handle),
            Err(error) => log::warn!("Voice chat: {error}; using the default device"),
        }
    }
    alsa.open_pcm("default", stream, layouts)
}

fn spawn(
    alsa: Arc<Alsa>,
    handle: Handle,
    name: &str,
    run: impl FnOnce(&Alsa, &Handle, &AtomicBool) + Send + 'static,
) -> Result<Stream, String> {
    let running = Arc::new(AtomicBool::new(true));
    let thread = {
        let running = running.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                run(&alsa, &handle, &running);
                // SAFETY: the handle is closed once, after its last use.
                unsafe { (alsa.close)(handle.pcm) };
            })
            .map_err(|e| e.to_string())?
    };
    Ok(Stream {
        running,
        thread: Some(thread),
    })
}

/// Recovers from an underrun or overrun; false when the device is gone.
fn recover(alsa: &Alsa, handle: &Handle, code: c_long, what: &str) -> bool {
    // SAFETY: `handle` is open.
    let recovered = unsafe { (alsa.recover)(handle.pcm, code as c_int, 1) };
    if recovered < 0 {
        log::warn!("Voice chat {what}: {}", alsa.error(recovered));
        return false;
    }
    true
}

pub fn open_output(name: Option<&str>, mut render: Render) -> Result<Stream, String> {
    let alsa = Alsa::load()?;
    let handle = open(&alsa, name, PLAYBACK, &[(2, RATE), (1, RATE), (2, 44_100)])?;
    spawn(alsa, handle, "horizon-voice-speakers", move |alsa, handle, running| {
        let channels = handle.channels;
        let mut mix = vec![0.0f32; PERIOD * channels];
        let mut samples = vec![0i16; PERIOD * channels];
        while running.load(Ordering::Relaxed) {
            render(&mut mix, channels, handle.rate);
            for (sample, value) in samples.iter_mut().zip(&mix) {
                *sample = (value.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
            }
            let mut done = 0;
            while done < PERIOD && running.load(Ordering::Relaxed) {
                // SAFETY: `samples` holds PERIOD frames of `channels` samples.
                let written = unsafe {
                    (alsa.writei)(
                        handle.pcm,
                        samples[done * channels..].as_ptr().cast(),
                        (PERIOD - done) as c_ulong,
                    )
                };
                if written < 0 {
                    if !recover(alsa, handle, written, "output") {
                        return;
                    }
                } else {
                    done += written as usize;
                }
            }
        }
    })
}

pub fn open_input(name: Option<&str>, make: MakeSink) -> Result<Stream, String> {
    let alsa = Alsa::load()?;
    let handle = open(&alsa, name, CAPTURE, &[(1, RATE), (2, RATE), (2, 44_100), (1, 44_100)])?;
    let mut sink = match make(handle.rate) {
        Ok(sink) => sink,
        Err(error) => {
            // SAFETY: closes the handle opened above, which no thread uses.
            unsafe { (alsa.close)(handle.pcm) };
            return Err(error);
        }
    };
    spawn(alsa, handle, "horizon-voice-microphone", move |alsa, handle, running| {
        let channels = handle.channels;
        let mut samples = vec![0i16; PERIOD * channels];
        while running.load(Ordering::Relaxed) {
            // SAFETY: `samples` has room for PERIOD frames of `channels` samples.
            let read = unsafe {
                (alsa.readi)(handle.pcm, samples.as_mut_ptr().cast(), PERIOD as c_ulong)
            };
            if read < 0 {
                if !recover(alsa, handle, read, "input") {
                    return;
                }
                continue;
            }
            for frame in samples[..read as usize * channels].chunks(channels) {
                // Mix the device's channels down to mono.
                let sum: f32 = frame.iter().map(|&s| f32::from(s) / 32_768.0).sum();
                sink(sum / channels as f32);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// ALSA's "null" device takes and gives silence, so this runs without a sound card.
    #[test]
    fn streams_run_on_the_null_device_and_stop_when_dropped() {
        let Ok(alsa) = Alsa::load() else {
            eprintln!("no libasound here: skipped");
            return;
        };
        let _ = alsa.names(true);
        let rendered = Arc::new(AtomicUsize::new(0));
        let heard = Arc::new(AtomicUsize::new(0));
        let output = {
            let rendered = rendered.clone();
            open_output(
                Some("null"),
                Box::new(move |out, channels, rate| {
                    assert!((1..=2).contains(&channels) && rate > 0);
                    rendered.fetch_add(out.len() / channels, Ordering::SeqCst);
                }),
            )
            .unwrap()
        };
        let input = {
            let heard = heard.clone();
            open_input(
                Some("null"),
                Box::new(move |_rate| {
                    Ok(Box::new(move |_sample| {
                        heard.fetch_add(1, Ordering::SeqCst);
                    }))
                }),
            )
            .unwrap()
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        drop((output, input));
        assert!(rendered.load(Ordering::SeqCst) >= PERIOD);
        assert!(heard.load(Ordering::SeqCst) >= PERIOD);
    }
}
