//! The sound input: what the microphone hears, handed to the app's recorder. Like the output,
//! this is the only place that knows of sound devices. The input is opened when a take is
//! started and closed when it ends, so the system asks for the microphone only then.

use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SizedSample};
use iskra_audio::Recorder;
use iskra_ui::log::Log;

fn run<T: SizedSample>(device: &cpal::Device, config: &cpal::StreamConfig, recorder: Arc<Recorder>, log: &Log) -> Option<cpal::Stream>
where
    f32: FromSample<T>,
{
    let (channels, rate) = (config.channels, config.sample_rate.0);
    let mut heard: Vec<f32> = Vec::new();
    let said = log.clone();
    let take = move |input: &[T], _: &cpal::InputCallbackInfo| {
        heard.clear();
        heard.extend(input.iter().map(|sample| f32::from_sample(*sample)));
        recorder.feed(&heard, channels, rate);
    };
    device.build_input_stream(config, take, move |problem| said.say(format!("sound input: {problem}")), None).ok()
}

/// Open the system's sound input and let `recorder` hear it, for as long as what is returned
/// is kept. `None` on a machine with no input, or one whose input can't be opened, or where
/// the app was not allowed the microphone.
pub fn open(recorder: Arc<Recorder>, log: &Log) -> Option<cpal::Stream> {
    let device = cpal::default_host().default_input_device()?;
    let supported = device.default_input_config().ok()?;
    let config: cpal::StreamConfig = supported.config();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => run::<f32>(&device, &config, recorder, log),
        cpal::SampleFormat::I16 => run::<i16>(&device, &config, recorder, log),
        cpal::SampleFormat::U16 => run::<u16>(&device, &config, recorder, log),
        cpal::SampleFormat::I32 => run::<i32>(&device, &config, recorder, log),
        _ => None,
    }?;
    stream.play().ok()?;
    Some(stream)
}
