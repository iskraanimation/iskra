//! The sound card. The app's player (`iskra-audio`) says what is heard next; this asks it for
//! samples whenever the card wants more. Nothing below the app knows a sound card.

use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use iskra_audio::Player;
use iskra_ui::log::Log;

fn run<T: SizedSample + FromSample<f32>>(device: &cpal::Device, config: &cpal::StreamConfig, player: Arc<Player>, log: &Log) -> Option<cpal::Stream> {
    let (channels, rate) = (usize::from(config.channels), config.sample_rate.0);
    let mut heard: Vec<f32> = Vec::new();
    let said = log.clone();
    let fill = move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
        heard.resize(out.len(), 0.0);
        player.fill(&mut heard, channels, rate);
        for (sample, value) in out.iter_mut().zip(&heard) {
            *sample = T::from_sample(*value);
        }
    };
    device.build_output_stream(config, fill, move |problem| said.say(format!("sound output: {problem}")), None).ok()
}

/// Open the system's sound output and let `player` be heard through it. The sound plays for as
/// long as what is returned is kept. `None` on a machine with no sound output, or one whose
/// output can't be opened: the app then plays silently, by its own clock.
pub fn open(player: Arc<Player>, log: &Log) -> Option<cpal::Stream> {
    let device = cpal::default_host().default_output_device()?;
    let supported = device.default_output_config().ok()?;
    let config: cpal::StreamConfig = supported.config();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => run::<f32>(&device, &config, player, log),
        cpal::SampleFormat::I16 => run::<i16>(&device, &config, player, log),
        cpal::SampleFormat::U16 => run::<u16>(&device, &config, player, log),
        cpal::SampleFormat::I32 => run::<i32>(&device, &config, player, log),
        _ => None,
    }?;
    stream.play().ok()?;
    Some(stream)
}
