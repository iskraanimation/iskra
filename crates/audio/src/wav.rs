//! WAV files: RIFF and RF64, PCM of 8, 16, 24 and 32 bits and floats of 32 and 64.
//!
//! The reading of the chunks and of the samples is ported from FilmCraft's
//! `crates/media/src/wav.rs` (MIT OR Apache-2.0; see `ATTRIBUTION.md`), without its Broadcast
//! Wave start time and its resampling, and made to refuse a damaged file instead of trusting it.

use crate::{Sound, SoundError, SoundInfo};

/// Whether `bytes` begin as a WAV file does.
pub fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && (&bytes[0..4] == b"RIFF" || &bytes[0..4] == b"RF64") && &bytes[8..12] == b"WAVE"
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Where the samples are and how they are written.
struct Layout {
    info: SoundInfo,
    offset: usize,
    bits: u16,
    float: bool,
}

/// Find the format and the samples among the chunks of `b`, of which only the first `have`
/// bytes need to be there: `whole` is how long the file is.
fn layout(b: &[u8], whole: u64) -> Result<Layout, SoundError> {
    if !is_wav(b) {
        return Err(SoundError::NotSound);
    }
    let bad = |why: &str| SoundError::Unreadable(format!("the WAV file {why}"));
    let (mut pos, mut format, mut data) = (12usize, None, None);
    while pos + 8 <= b.len() {
        let (id, len, body) = (&b[pos..pos + 4], le32(b, pos + 4) as u64, pos + 8);
        match id {
            b"fmt " if len >= 16 && body + 16 <= b.len() => {
                let (mut tag, channels, rate, bits) = (le16(b, body), le16(b, body + 2), le32(b, body + 4), le16(b, body + 14));
                // The extensible form says what it is further on.
                if tag == 0xFFFE && len >= 40 && body + 26 <= b.len() {
                    tag = le16(b, body + 24);
                }
                format = Some((tag, channels, rate, bits));
            }
            b"data" => {
                // A length that is not known, or runs past the end, is to the end of the file.
                let rest = whole.saturating_sub(body as u64);
                data = Some((body, if len == 0xFFFF_FFFF || len > rest { rest } else { len }));
                break;
            }
            _ => {}
        }
        let Some(next) = (body as u64).checked_add(len + (len & 1)).filter(|next| *next <= b.len() as u64) else { break };
        pos = next as usize;
    }
    let (tag, channels, rate, bits) = format.ok_or_else(|| bad("has no format"))?;
    let (offset, len) = data.ok_or_else(|| bad("has no samples"))?;
    let float = tag == 3;
    let known = (tag == 1 && matches!(bits, 8 | 16 | 24 | 32)) || (float && matches!(bits, 32 | 64));
    if !known || channels == 0 || rate == 0 {
        return Err(SoundError::Unreadable(format!("the WAV file is of a kind that is not read (format {tag}, {bits} bits, {channels} channels)")));
    }
    let frame_bytes = u64::from(channels) * u64::from(bits) / 8;
    Ok(Layout { info: SoundInfo { sample_rate: rate, channels, frames: len / frame_bytes }, offset, bits, float })
}

/// What a WAV file says of itself, from its first bytes: `head` is the beginning of a file
/// that is `whole` bytes long. A few kilobytes are enough unless the file has much before
/// its samples.
pub fn read_wav_info(head: &[u8], whole: u64) -> Result<SoundInfo, SoundError> {
    layout(head, whole).map(|layout| layout.info)
}

/// Read a WAV file's samples.
pub fn read_wav(bytes: &[u8]) -> Result<Sound, SoundError> {
    let Layout { info, offset, bits, float } = layout(bytes, bytes.len() as u64)?;
    let width = usize::from(bits) / 8;
    let count = (info.frames as usize).saturating_mul(usize::from(info.channels));
    let data = bytes.get(offset..offset + count * width).ok_or_else(|| SoundError::Unreadable("the WAV file ends before its samples do".to_string()))?;
    let sample = |b: &[u8]| match (bits, float) {
        (8, _) => (f32::from(b[0]) - 128.0) / 128.0,
        (16, _) => f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0,
        (24, _) => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
        (32, false) => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
        (32, true) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        _ => f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32,
    };
    Ok(Sound { info, samples: data.chunks_exact(width).map(sample).collect() })
}

/// Samples as a WAV file of 16 bits: `samples` has the channels of each moment one after
/// another.
pub fn write_wav16(samples: &[f32], channels: u16, rate: u32) -> Vec<u8> {
    write_wav(samples, channels, rate, 16)
}

/// Samples as little-endian bytes of `bits` each: 16 or 24 bits of integers, or with 32 a
/// float. An integer is held to full scale.
pub fn pcm_bytes(samples: &[f32], bits: u16) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * usize::from(bits / 8));
    for sample in samples {
        let held = sample.clamp(-1.0, 1.0);
        match bits {
            24 => bytes.extend_from_slice(&((held * 8_388_607.0).round() as i32).to_le_bytes()[..3]),
            32 => bytes.extend_from_slice(&sample.to_le_bytes()),
            _ => bytes.extend_from_slice(&((held * 32767.0).round() as i16).to_le_bytes()),
        }
    }
    bytes
}

/// Samples as a WAV file of `bits` a sample: 16 or 24 bits of integers, or 32-bit floats.
/// Another number is 16.
pub fn write_wav(samples: &[f32], channels: u16, rate: u32, bits: u16) -> Vec<u8> {
    let bits = if matches!(bits, 24 | 32) { bits } else { 16 };
    let data = pcm_bytes(samples, bits);
    let (width, tag) = (bits / 8, if bits == 32 { 3u16 } else { 1 });
    let mut file = Vec::with_capacity(44 + data.len());
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
    file.extend_from_slice(b"WAVEfmt ");
    file.extend_from_slice(&16u32.to_le_bytes());
    file.extend_from_slice(&tag.to_le_bytes());
    file.extend_from_slice(&channels.to_le_bytes());
    file.extend_from_slice(&rate.to_le_bytes());
    file.extend_from_slice(&(rate * u32::from(channels) * u32::from(width)).to_le_bytes());
    file.extend_from_slice(&(channels * width).to_le_bytes());
    file.extend_from_slice(&bits.to_le_bytes());
    file.extend_from_slice(b"data");
    file.extend_from_slice(&(data.len() as u32).to_le_bytes());
    file.extend_from_slice(&data);
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wav_file_is_read_back_as_it_was_written() {
        // A second of a tone in two channels, the right one half as loud.
        let samples: Vec<f32> = (0..8000).flat_map(|i| [(i as f32 * 0.05).sin() * 0.8, (i as f32 * 0.05).sin() * 0.4]).collect();
        let file = write_wav16(&samples, 2, 8000);
        assert!(is_wav(&file));
        let info = SoundInfo { sample_rate: 8000, channels: 2, frames: 8000 };
        assert_eq!(read_wav_info(&file[..64], file.len() as u64), Ok(info), "its first bytes say what it is");
        let sound = read_wav(&file).unwrap();
        assert_eq!((sound.info, sound.samples.len()), (info, 16_000));
        assert!(sound.samples.iter().zip(&samples).all(|(read, written)| (read - written).abs() < 1.0 / 16_000.0));
        // Deeper, the same: 24 bits closer still, floats as they were.
        let deep = read_wav(&write_wav(&samples, 2, 8000, 24)).unwrap();
        assert!(deep.samples.iter().zip(&samples).all(|(read, written)| (read - written).abs() < 1.0 / 4_000_000.0));
        assert_eq!(read_wav(&write_wav(&samples, 2, 8000, 32)).unwrap().samples, samples);
    }

    #[test]
    fn other_widths_are_read_and_what_is_not_a_wav_file_is_refused() {
        // 24 bits and 32-bit floats, one channel, written by hand.
        let wav = |tag: u16, bits: u16, data: &[u8]| {
            let mut file = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
            file.extend_from_slice(&16u32.to_le_bytes());
            for field in [tag, 1] {
                file.extend_from_slice(&field.to_le_bytes());
            }
            file.extend_from_slice(&44_100u32.to_le_bytes());
            file.extend_from_slice(&0u32.to_le_bytes());
            file.extend_from_slice(&(bits / 8).to_le_bytes());
            file.extend_from_slice(&bits.to_le_bytes());
            // A chunk the reader doesn't know, of an odd length, before the samples.
            file.extend_from_slice(b"note\x03\0\0\0abc\0data");
            file.extend_from_slice(&(data.len() as u32).to_le_bytes());
            file.extend_from_slice(data);
            file
        };
        let deep = read_wav(&wav(1, 24, &[0, 0, 0x40, 0, 0, 0xC0])).unwrap();
        assert_eq!((deep.info.frames, &deep.samples[..]), (2, &[0.5, -0.5][..]));
        let floats: Vec<u8> = [0.25f32, -1.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(read_wav(&wav(3, 32, &floats)).unwrap().samples, [0.25, -1.0]);

        assert_eq!(read_wav(b"ID3 this is an mp3"), Err(SoundError::NotSound));
        assert!(matches!(read_wav(&wav(85, 16, &[0, 0])), Err(SoundError::Unreadable(why)) if why.contains("format 85")));
        assert!(matches!(read_wav(b"RIFF\0\0\0\0WAVE"), Err(SoundError::Unreadable(_))));
        // A file cut short gives what it has.
        let mut short = wav(1, 16, &[0, 0x40, 0, 0x20]);
        short.truncate(short.len() - 2);
        assert_eq!(read_wav(&short).unwrap().samples, [0.5]);
    }
}
