//! Microphone and speakers through cpal, on Windows and macOS.
//!
//! On Linux the prebuilt game already links ALSA through its own cpal, and the launcher refuses
//! a mod that would compile a second provider of it; there `sound_alsa.rs` is used instead.

use super::{MakeSink, Render};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};

/// An open stream; dropping it closes the device.
pub struct Stream(#[allow(dead_code)] cpal::Stream);

/// The names of the system's microphones (`input`) or speakers.
pub fn device_names(input: bool) -> Vec<String> {
    let host = cpal::default_host();
    let devices = if input {
        host.input_devices()
    } else {
        host.output_devices()
    };
    let mut names: Vec<String> = devices
        .map(|devices| devices.filter_map(|device| device.name().ok()).collect())
        .unwrap_or_default();
    names.dedup();
    names
}

fn find_device(host: &cpal::Host, input: bool, name: Option<&str>) -> Option<cpal::Device> {
    let wanted = name.and_then(|name| {
        let mut devices = if input {
            host.input_devices().ok()?
        } else {
            host.output_devices().ok()?
        };
        devices.find(|device| device.name().is_ok_and(|n| n == name))
    });
    wanted.or_else(|| {
        if input {
            host.default_input_device()
        } else {
            host.default_output_device()
        }
    })
}

pub fn open_output(name: Option<&str>, render: Render) -> Result<Stream, String> {
    let host = cpal::default_host();
    let device = find_device(&host, false, name).ok_or("no output device")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let format = supported.sample_format();
    let config = supported.config();
    let stream = match format {
        SampleFormat::F32 => output_stream::<f32>(&device, &config, render),
        SampleFormat::I16 => output_stream::<i16>(&device, &config, render),
        SampleFormat::U16 => output_stream::<u16>(&device, &config, render),
        SampleFormat::I32 => output_stream::<i32>(&device, &config, render),
        other => return Err(format!("unsupported output sample format {other:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(Stream(stream))
}

fn output_stream<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut render: Render,
) -> Result<cpal::Stream, String> {
    let channels = usize::from(config.channels).max(1);
    let rate = config.sample_rate.0;
    let mut mix = Vec::new();
    device
        .build_output_stream(
            config,
            move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
                let frames = out.len() / channels;
                // The mixer writes mono or stereo; further channels stay silent.
                let mixed = channels.min(2);
                mix.resize(frames * mixed, 0.0);
                render(&mut mix, mixed, rate);
                for (frame, mixed_frame) in out.chunks_mut(channels).zip(mix.chunks(mixed)) {
                    for (index, sample) in frame.iter_mut().enumerate() {
                        *sample = T::from_sample(mixed_frame.get(index).copied().unwrap_or(0.0));
                    }
                }
            },
            |error| log::warn!("Voice chat output: {error}"),
            None,
        )
        .map_err(|e| e.to_string())
}

pub fn open_input(name: Option<&str>, make: MakeSink) -> Result<Stream, String> {
    let host = cpal::default_host();
    let device = find_device(&host, true, name).ok_or("no input device")?;
    let supported = device.default_input_config().map_err(|e| e.to_string())?;
    let format = supported.sample_format();
    let config = supported.config();
    let sink = make(config.sample_rate.0)?;
    let stream = match format {
        SampleFormat::F32 => input_stream::<f32>(&device, &config, sink),
        SampleFormat::I16 => input_stream::<i16>(&device, &config, sink),
        SampleFormat::U16 => input_stream::<u16>(&device, &config, sink),
        SampleFormat::I32 => input_stream::<i32>(&device, &config, sink),
        other => return Err(format!("unsupported input sample format {other:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(Stream(stream))
}

fn input_stream<T: SizedSample>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut sink: Box<dyn FnMut(f32) + Send>,
) -> Result<cpal::Stream, String>
where
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels).max(1);
    device
        .build_input_stream(
            config,
            move |input: &[T], _: &cpal::InputCallbackInfo| {
                for frame in input.chunks(channels) {
                    // Mix the device's channels down to mono.
                    let sum: f32 = frame.iter().map(|sample| f32::from_sample(*sample)).sum();
                    sink(sum / frame.len() as f32);
                }
            },
            |error| log::warn!("Voice chat input: {error}"),
            None,
        )
        .map_err(|e| e.to_string())
}
