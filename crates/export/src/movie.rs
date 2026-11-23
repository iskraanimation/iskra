//! Movies (plan §7.14, R6): a shot as one file with its sound. A QuickTime file holds ProRes
//! of any kind, or H.264, with uncompressed or AAC sound, for post; an MP4 holds H.264 and
//! AAC, to send and to watch. The encoders and the container writer are FilmCraft's, vendored.
//!
//! The picture is made as for a sequence, on the matte, or with transparency in ProRes 4444,
//! and is BT.709 in video range. The two colour conversions are ported from FilmCraft's
//! `crates/export/src/lib.rs` (MIT OR Apache-2.0; see `ATTRIBUTION.md`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use filmcraft_isobmff::{AvcConfig, Brand, FourCc, Mp4Writer, PcmConfig, SampleEntry, TrackConfig, WriteSample, WriterOptions};
use iskra_model::{MediaKind, Project, ShotId};
use rayon::prelude::*;

use crate::{
    Canvas, Codec, ExportError, Format, IfExists, Naming, Pixels, ProRes, Quality, Report, Settings, SoundCodec, file_name, frames_of, nothing_of, pixels,
};

/// How many frames are rendered at once before they are encoded in their order.
const BATCH: usize = 8;

/// How good an H.264 picture is, as the encoder counts: lower is better and larger.
fn h264_quality(quality: Quality) -> f32 {
    match quality {
        Quality::Best => 14.0,
        Quality::High => 18.0,
        Quality::Medium => 23.0,
        Quality::Low => 28.0,
    }
}

/// What the H.264 encoder aims at: the data rate the picture is held to, or else a quality.
fn h264_rate(settings: &Settings) -> filmcraft_h264enc::RateControl {
    match settings.video_kbps {
        Some(kbps) => filmcraft_h264enc::RateControl::Cbr { kbps },
        None => filmcraft_h264enc::RateControl::Crf(h264_quality(settings.quality)),
    }
}

fn prores_profile(kind: ProRes) -> filmcraft_prores::Profile {
    use filmcraft_prores::Profile;
    match kind {
        ProRes::Proxy => Profile::Proxy,
        ProRes::Lt => Profile::Lt,
        ProRes::Standard => Profile::Standard,
        ProRes::Hq => Profile::Hq,
        ProRes::P4444 => Profile::P4444,
        ProRes::P4444Xq => Profile::P4444Xq,
    }
}

/// The file a shot's movie is, in `dest`: all movies lie together there (plan §7.14), a later
/// version as `Shot_01_v002.mov`.
pub fn movie_file(dest: &Path, shot_name: &str, version: u32, format: Format) -> PathBuf {
    let name = file_name(shot_name);
    dest.join(if version > 1 { format!("{name}_v{version:03}.{}", format.extension()) } else { format!("{name}.{}", format.extension()) })
}

/// BT.709 video-range 8-bit 4:2:0 from sRGB pixels of three bytes: each 2 × 2 of them share
/// their colour.
fn rgb_to_yuv420_8(rgb: &[u8], w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let (mut y, mut u, mut v) = (vec![0u8; w * h], vec![0u8; cw * ch], vec![0u8; cw * ch]);
    y.par_chunks_mut(w * 2).zip(u.par_chunks_mut(cw).zip(v.par_chunks_mut(cw))).enumerate().for_each(|(cy, (yr, (ur, vr)))| {
        let rows = yr.len() / w;
        let (mut us, mut vs, mut count) = (vec![0f32; cw], vec![0f32; cw], vec![0f32; cw]);
        for dy in 0..rows {
            let src = &rgb[(cy * 2 + dy) * w * 3..(cy * 2 + dy + 1) * w * 3];
            for x in 0..w {
                let (r, g, b) = (f32::from(src[x * 3]) / 255.0, f32::from(src[x * 3 + 1]) / 255.0, f32::from(src[x * 3 + 2]) / 255.0);
                let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                yr[dy * w + x] = (16.0 + 219.0 * luma).round().clamp(1.0, 254.0) as u8;
                us[x / 2] += (b - luma) / 1.8556;
                vs[x / 2] += (r - luma) / 1.5748;
                count[x / 2] += 1.0;
            }
        }
        for cx in 0..cw {
            ur[cx] = (128.0 + 224.0 * us[cx] / count[cx]).round().clamp(1.0, 254.0) as u8;
            vr[cx] = (128.0 + 224.0 * vs[cx] / count[cx]).round().clamp(1.0, 254.0) as u8;
        }
    });
    (y, u, v)
}

/// BT.709 video-range 10-bit 4:2:2 from sRGB pixels of three bytes: each two across share
/// their colour.
fn rgb_to_yuv422_10(rgb: &[u8], w: usize, y: &mut [u16], cb: &mut [u16], cr: &mut [u16]) {
    let cw = w.div_ceil(2);
    y.par_chunks_mut(w).zip(cb.par_chunks_mut(cw).zip(cr.par_chunks_mut(cw))).enumerate().for_each(|(row, (yr, (cbr, crr)))| {
        let src = &rgb[row * w * 3..(row + 1) * w * 3];
        let (mut us, mut vs) = (vec![0f32; w], vec![0f32; w]);
        for x in 0..w {
            let (r, g, b) = (f32::from(src[x * 3]) / 255.0, f32::from(src[x * 3 + 1]) / 255.0, f32::from(src[x * 3 + 2]) / 255.0);
            let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            yr[x] = (64.0 + 876.0 * luma).round().clamp(4.0, 1019.0) as u16;
            (us[x], vs[x]) = ((b - luma) / 1.8556, (r - luma) / 1.5748);
        }
        for cx in 0..cw {
            let (first, second) = (cx * 2, (cx * 2 + 1).min(w - 1));
            cbr[cx] = (512.0 + 896.0 * (us[first] + us[second]) * 0.5).round().clamp(4.0, 1019.0) as u16;
            crr[cx] = (512.0 + 896.0 * (vs[first] + vs[second]) * 0.5).round().clamp(4.0, 1019.0) as u16;
        }
    });
}

/// BT.709 video-range 10-bit 4:4:4 from sRGB pixels of `channels` bytes, a colour for every
/// pixel. The fourth byte, if there is one, is the alpha, which is full range.
fn rgb_to_yuv444_10(pixels: &[u8], w: usize, channels: usize, picture: &mut filmcraft_prores::Frame) {
    let (y, cb, cr) = (&mut picture.y, &mut picture.cb, &mut picture.cr);
    y.par_chunks_mut(w).zip(cb.par_chunks_mut(w).zip(cr.par_chunks_mut(w))).enumerate().for_each(|(row, (yr, (cbr, crr)))| {
        let src = &pixels[row * w * channels..(row + 1) * w * channels];
        for x in 0..w {
            let at = x * channels;
            let (r, g, b) = (f32::from(src[at]) / 255.0, f32::from(src[at + 1]) / 255.0, f32::from(src[at + 2]) / 255.0);
            let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            yr[x] = (64.0 + 876.0 * luma).round().clamp(4.0, 1019.0) as u16;
            cbr[x] = (512.0 + 896.0 * (b - luma) / 1.8556).round().clamp(4.0, 1019.0) as u16;
            crr[x] = (512.0 + 896.0 * (r - luma) / 1.5748).round().clamp(4.0, 1019.0) as u16;
        }
    });
    if let (Some(alpha), 4) = (picture.alpha.as_mut(), channels) {
        for (to, from) in alpha.iter_mut().zip(pixels.chunks(4)) {
            *to = ((u32::from(from[3]) * 1023 + 127) / 255) as u16;
        }
    }
}

/// One encoded picture, as the container takes it.
struct Packet {
    data: Vec<u8>,
    key: bool,
    /// How much later it is shown than it is decoded, in the track's time.
    shown_after: i32,
}

/// The picture's encoder, of either kind.
enum Video {
    H264(Box<filmcraft_h264enc::Encoder>),
    /// With the kind of ProRes it makes, and whether the picture has transparency.
    ProRes(Box<filmcraft_prores::Encoder>, filmcraft_prores::Profile, bool),
}

impl Video {
    fn new(settings: &Settings, width: u32, height: u32, rate: (u32, u32)) -> Result<Video, ExportError> {
        if settings.codec == Codec::ProRes {
            let profile = prores_profile(settings.prores);
            let mut config = filmcraft_prores::EncoderConfig::new(profile, width, height);
            config.encode_alpha = settings.alpha;
            return Ok(Video::ProRes(Box::new(filmcraft_prores::Encoder::with_config(config)), profile, settings.alpha));
        }
        let mut config = filmcraft_h264enc::EncoderConfig::new(width, height, rate.0, rate.1);
        config.rate = h264_rate(settings);
        // Each picture with its length before it, as an MP4 file holds them.
        config.format = filmcraft_h264enc::PacketFormat::LengthPrefixed;
        let encoder = filmcraft_h264enc::Encoder::new(config).map_err(|e| ExportError::Invalid(format!("the H.264 encoder: {e}")))?;
        Ok(Video::H264(Box::new(encoder)))
    }

    fn sample_entry(&self, width: u32, height: u32) -> SampleEntry {
        match self {
            Video::H264(encoder) => {
                let config = AvcConfig::parse(&encoder.avcc()).unwrap_or_else(|_| {
                    let (sps, pps) = encoder.sps_pps();
                    AvcConfig::new(vec![sps], vec![pps], 4)
                });
                SampleEntry::avc(config, width as u16, height as u16)
            }
            Video::ProRes(_, profile, _) => SampleEntry::prores(FourCc(profile.fourcc()), width as u16, height as u16),
        }
    }

    /// With pictures that are shown out of the order they are decoded in, decoding starts a
    /// frame before showing does.
    fn media_start(&self, frame: u32) -> Option<i64> {
        match self {
            Video::H264(encoder) if encoder.delay() > 0 => Some(i64::from(frame)),
            _ => None,
        }
    }

    /// Encode the picture `index` of the movie, of sRGB pixels of three bytes, or of four
    /// with their alpha where the picture has transparency. A frame of the film lasts `frame`
    /// of the track's time.
    fn encode(&mut self, rgb: &[u8], width: u32, height: u32, index: usize, frame: u32) -> Result<Vec<Packet>, ExportError> {
        let failed = |why: &dyn std::fmt::Display| ExportError::Invalid(format!("the picture could not be encoded: {why}"));
        match self {
            Video::H264(encoder) => {
                let (y, u, v) = rgb_to_yuv420_8(rgb, width as usize, height as usize);
                let picture = filmcraft_h264enc::YuvFrame { y: &y, u: &u, v: &v, y_stride: width as usize, uv_stride: (width as usize).div_ceil(2) };
                let packets = encoder.try_encode(&picture, index as i64 * i64::from(frame)).map_err(|e| failed(&e))?;
                Ok(packets
                    .into_iter()
                    .map(|packet| Packet { data: packet.data, key: packet.keyframe, shown_after: (packet.pts - packet.dts) as i32 })
                    .collect())
            }
            Video::ProRes(encoder, profile, alpha) => {
                let chroma = profile.chroma();
                let mut picture = filmcraft_prores::Frame::new(width, height, chroma, 10, *alpha);
                if chroma == filmcraft_prores::ChromaFormat::Yuv444 {
                    rgb_to_yuv444_10(rgb, width as usize, if *alpha { 4 } else { 3 }, &mut picture);
                } else {
                    rgb_to_yuv422_10(rgb, width as usize, &mut picture.y, &mut picture.cb, &mut picture.cr);
                }
                let data = encoder.encode(&picture).map_err(|e| failed(&e))?;
                Ok(vec![Packet { data, key: true, shown_after: 0 }])
            }
        }
    }

    fn flush(&mut self) -> Vec<Packet> {
        match self {
            Video::H264(encoder) => encoder
                .flush()
                .into_iter()
                .map(|packet| Packet { data: packet.data, key: packet.keyframe, shown_after: (packet.pts - packet.dts) as i32 })
                .collect(),
            Video::ProRes(..) => Vec::new(),
        }
    }
}

/// The sound a shot hears between two of its frames (plan §7.2): the clips of its storyboard's
/// sound tracks, read from their files, and the time in the film, in seconds, at which the
/// first of the frames is. A sound whose file can't be read is said in `notes` and left out.
fn shot_sound(project: &Project, shot: ShotId, first: u32, folder: Option<&Path>, notes: &mut Vec<String>) -> (Vec<iskra_audio::Clip>, f64) {
    let fps = project.settings.frame_rate.as_f64();
    let Some((board, clip)) = project.storyboard_of(shot).and_then(|board| Some((board, *board.clip(shot)?))) else { return (Vec::new(), 0.0) };
    let from = (f64::from(clip.start) + f64::from(first) - f64::from(clip.in_point)) / fps;
    let mut clips = Vec::new();
    let mut read: Vec<(iskra_model::MediaId, Option<Arc<iskra_audio::Sound>>)> = Vec::new();
    for sound in board.sound.iter().filter(|track| !track.mute).flat_map(|track| &track.clips) {
        let Some(media) = project.media(sound.media).filter(|media| matches!(media.kind, MediaKind::Sound { .. })) else { continue };
        // A sound that several clips are of is read once.
        if !read.iter().any(|(id, _)| *id == media.id) {
            let file = media.file(folder);
            let samples = std::fs::read(&file).ok().and_then(|bytes| iskra_audio::read_wav(&bytes).ok()).map(Arc::new);
            if samples.is_none() {
                notes.push(format!("the sound `{}` could not be read from {}, and is not in the movie", media.name, file.display()));
            }
            read.push((media.id, samples));
        }
        let Some(samples) = read.iter().find(|(id, _)| *id == media.id).and_then(|(_, samples)| samples.clone()) else { continue };
        let seconds = |frames: u32| f64::from(frames) / fps;
        clips.push(iskra_audio::Clip {
            sound: samples,
            start: seconds(sound.start),
            offset: seconds(sound.offset),
            length: seconds(sound.length),
            gain: sound.gain * project.settings.volume,
        });
    }
    (clips, from)
}

/// The sound under a shot's `frames`, as a WAV file of both channels at the settings' rate and
/// depth: what a movie of them would have. `None` where nothing is heard under them.
pub(crate) fn sound_file(
    project: &Project,
    shot: ShotId,
    frames: (u32, u32),
    settings: &Settings,
    folder: Option<&Path>,
    notes: &mut Vec<String>,
) -> Option<Vec<u8>> {
    let (clips, from) = shot_sound(project, shot, frames.0, folder, notes);
    if clips.is_empty() {
        return None;
    }
    let rate = project.settings.frame_rate;
    let (per_second, frame) = (u64::try_from(rate.num).ok()?, u64::try_from(rate.den).ok()?);
    let moments = (u64::from(frames.1 - frames.0) * frame * u64::from(settings.sound_rate)).div_ceil(per_second.max(1)) as usize;
    let mut heard = vec![0.0f32; moments * 2];
    iskra_audio::mix(&clips, from, settings.sound_rate, &mut heard);
    heard.iter_mut().for_each(|sample| *sample = sample.clamp(-1.0, 1.0));
    Some(iskra_audio::write_wav(&heard, 2, settings.sound_rate, settings.sound_bits))
}

/// What a movie is made of: the pictures in their order, and the sound under them.
/// What one frame of a movie shows.
#[derive(Clone, Copy)]
enum Seen<'a> {
    /// Between shots the film is black.
    Black,
    /// A frame of a shot.
    Shot(&'a iskra_model::Shot, u32),
    /// A frame inside a transition: a frame of each shot, mixed as it is that far through.
    Mixed { kind: iskra_model::TransitionKind, at: f64, outgoing: Option<(&'a iskra_model::Shot, u32)>, incoming: Option<(&'a iskra_model::Shot, u32)> },
}

struct Film<'a> {
    /// What each frame of the movie shows.
    frames: Vec<Seen<'a>>,
    clips: Vec<iskra_audio::Clip>,
    /// The time, in seconds, at which the movie starts among the clips.
    from: f64,
    /// The count of its first frame, from 0, where one is burned in: the shot's frame, or
    /// the film's.
    first: u32,
}

impl Seen<'_> {
    /// The name of the shot that is seen: inside a transition, the one more of it is of.
    fn name(&self) -> Option<&str> {
        match self {
            Seen::Black => None,
            Seen::Shot(shot, _) => Some(&shot.name),
            Seen::Mixed { at, outgoing, incoming, .. } => {
                if *at < 0.5 { outgoing.or(*incoming) } else { incoming.or(*outgoing) }.map(|(shot, _)| shot.name.as_str())
            }
        }
    }
}

/// Where a movie goes: `named` in `dest`, or beside what is there as the next version. `None`
/// if it is there already and is to be left.
fn place(dest: &Path, named: &str, format: Format, if_exists: IfExists) -> Result<Option<PathBuf>, ExportError> {
    let path = movie_file(dest, named, 1, format);
    if !path.exists() {
        return Ok(Some(path));
    }
    match if_exists {
        IfExists::Fail => Err(ExportError::Exists(path)),
        IfExists::Skip => Ok(None),
        IfExists::Replace => Ok(Some(path)),
        IfExists::Version => Ok((2..).map(|version| movie_file(dest, named, version, format)).find(|path| !path.exists())),
    }
}

/// A movie's picture: the size the settings ask for, with an even number of pixels each way,
/// and the project's frame rate as frames a second over the time of one.
fn picture(project: &Project, settings: &Settings) -> Result<(Canvas, u32, u32), ExportError> {
    if !settings.format.is_movie() {
        return Err(ExportError::Invalid("a movie is a MOV or an MP4 file".to_string()));
    }
    let view = Canvas::of(project, settings, true)?;
    let rate = crate::rate_of(project, settings);
    match (u32::try_from(rate.num), u32::try_from(rate.den)) {
        (Ok(per_second), Ok(frame)) if per_second > 0 && frame > 0 => Ok((view, per_second, frame)),
        _ => Err(ExportError::Invalid("the project's frame rate can't be written to a movie".to_string())),
    }
}

/// Write `film` as the movie file at `path`. A movie that is not finished is taken away again.
fn write_movie(project: &Project, film: &Film, path: &Path, settings: &Settings, progress: &(dyn Fn(usize, usize) -> bool + Sync)) -> Result<(), ExportError> {
    // Transparency only where the movie can hold it; else the picture is on the matte.
    let settings = Settings { bits: 8, ..settings.fitted() };
    let (channels, rate) = (if settings.alpha { 4 } else { 3 }, settings.sound_rate);
    let (view, per_second, frame) = picture(project, &settings)?;
    let io = |path: &Path, why: &dyn std::fmt::Display| ExportError::Io { path: path.to_path_buf(), why: why.to_string() };
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|e| io(folder, &e))?;
    }
    // At another rate than the project's, each frame is the one on screen at its moment.
    // Each with the frame of the film it is, for its count.
    let shown: Vec<(u32, Seen)> =
        crate::resampled(project, film.frames.len() as u32, crate::rate_of(project, &settings)).into_iter().map(|at| (at, film.frames[at as usize])).collect();
    let first = film.first;
    let film = &Film { frames: shown.iter().map(|(_, seen)| *seen).collect(), clips: film.clips.clone(), from: film.from, first };
    let (width, height, total) = (view.width, view.height, film.frames.len());
    let space = project.settings.working_space;
    let mut video = Video::new(&settings, width, height, (per_second, frame))?;
    let written = (|| -> Result<(), ExportError> {
        // Read as well as written: an MP4 is put in order at its end.
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path).map_err(|e| io(path, &e))?;
        let brand = if settings.format == Format::Mov { Brand::Mov } else { Brand::Mp4 };
        let mut movie = Mp4Writer::new(file, WriterOptions::new(brand)).map_err(|e| io(path, &e))?;
        let mut pictures = TrackConfig::new(video.sample_entry(width, height), per_second);
        pictures.media_start = video.media_start(frame);
        let pictures = movie.add_track(pictures).map_err(|e| io(path, &e))?;
        let put = |movie: &mut Mp4Writer<std::fs::File>, packets: Vec<Packet>| -> Result<(), ExportError> {
            for packet in packets {
                let sample = WriteSample { data: &packet.data, duration: frame, composition_offset: packet.shown_after, is_sync: packet.key };
                movie.write_sample(pictures, sample).map_err(|e| io(path, &e))?;
            }
            Ok(())
        };
        // Several frames are rendered at once; they are encoded one after another.
        let mut done = 0;
        for batch in shown.chunks(BATCH) {
            let rendered: Vec<Vec<u8>> = batch
                .par_iter()
                .map(|(index, seen)| {
                    let of = |shot: &iskra_model::Shot, at: u32| view.composite(shot, &project.palette, space, at);
                    let mut composited = match *seen {
                        Seen::Black if !settings.burn_in.any() => return vec![0u8; width as usize * height as usize * channels],
                        Seen::Black => vec![[0.0, 0.0, 0.0, 1.0]; width as usize * height as usize],
                        Seen::Shot(shot, at) => of(shot, at),
                        Seen::Mixed { kind, at, outgoing, incoming } => {
                            let (outgoing, incoming) = (outgoing.map(|(shot, at)| of(shot, at)), incoming.map(|(shot, at)| of(shot, at)));
                            // A shot is on its paper before it is mixed: black is black, not a hole.
                            let on_paper = |picture: Option<Vec<[f32; 4]>>| picture.map(|picture| crate::on_matte(&picture, space, &settings));
                            let (outgoing, incoming) = (on_paper(outgoing), on_paper(incoming));
                            iskra_render::mix(kind, at, outgoing.as_deref(), incoming.as_deref(), width as usize * height as usize)
                        }
                    };
                    view.burn(&mut composited, project, &settings, seen.name(), i64::from(first + index));
                    match pixels(&composited, space, &settings) {
                        Pixels::Eight(rgb) => rgb,
                        _ => Vec::new(),
                    }
                })
                .collect();
            for rgb in &rendered {
                let packets = video.encode(rgb, width, height, done, frame)?;
                put(&mut movie, packets)?;
                done += 1;
                if !progress(done, total) {
                    return Err(ExportError::Stopped);
                }
            }
        }
        put(&mut movie, video.flush())?;

        // The sound: what lies under the frames that were rendered, left and right.
        if !film.clips.is_empty() {
            let moments = (total as u64 * u64::from(frame) * u64::from(rate)).div_ceil(u64::from(per_second)) as usize;
            let mut heard = vec![0.0f32; moments * 2];
            iskra_audio::mix(&film.clips, film.from, rate, &mut heard);
            heard.iter_mut().for_each(|sample| *sample = sample.clamp(-1.0, 1.0));
            if settings.sound_codec == SoundCodec::Pcm {
                let bits = settings.sound_bits;
                let pcm = PcmConfig { bits, float: bits == 32, big_endian: false, signed: true, channels: 2, sample_rate: f64::from(rate) };
                let track = movie.add_track(TrackConfig::new(SampleEntry::pcm(pcm), rate)).map_err(|e| io(path, &e))?;
                // A second at a time.
                for second in heard.chunks(rate as usize * 2) {
                    let bytes = iskra_audio::pcm_bytes(second, bits);
                    let sample = WriteSample { data: &bytes, duration: (second.len() / 2) as u32, composition_offset: 0, is_sync: true };
                    movie.write_sample(track, sample).map_err(|e| io(path, &e))?;
                }
            } else {
                let mut encoder = filmcraft_aac::Encoder::new(filmcraft_aac::EncoderConfig::cbr(rate, 2, settings.sound_kbps * 1000))
                    .map_err(|e| ExportError::Invalid(format!("the AAC encoder: {e}")))?;
                let mut sound = TrackConfig::new(SampleEntry::aac(encoder.audio_specific_config(), 2, rate), rate);
                // The encoder's first samples are its own run-up, and are not heard.
                sound.media_start = Some(i64::from(encoder.priming_samples()));
                let track = movie.add_track(sound).map_err(|e| io(path, &e))?;
                let (left, right): (Vec<f32>, Vec<f32>) = heard.chunks(2).map(|pair| (pair[0], pair[1])).unzip();
                let mut units = encoder.encode(&[&left, &right]);
                units.extend(encoder.flush());
                for unit in units {
                    movie.write_sample(track, WriteSample { data: &unit, duration: 1024, composition_offset: 0, is_sync: true }).map_err(|e| io(path, &e))?;
                }
            }
        }
        // An MP4 with what a player needs first at its start.
        let finished = if settings.format == Format::Mp4 { movie.finish_faststart() } else { movie.finish() };
        finished.map(drop).map_err(|e| io(path, &e))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(path);
    }
    written
}

/// Render `shot` of `project` to a movie file in `dest` (plan §7.14): `Shot_01.mov` with
/// ProRes 422 HQ and uncompressed sound, or `Shot_01.mp4` with H.264 and AAC, as the settings'
/// format says. With `settings.sound` it has the shot's sound, the stretch of its
/// storyboard's sound tracks under the frames that are rendered; `folder` is the folder the
/// project's file is in, which sound files in it are named from. The shot's drawings have to
/// be loaded. `progress` is as for [`crate::export_sequence`]; a render that is stopped
/// leaves no file.
pub fn export_movie(
    project: &Project,
    shot: ShotId,
    settings: &Settings,
    dest: &Path,
    if_exists: IfExists,
    folder: Option<&Path>,
    progress: &(dyn Fn(usize, usize) -> bool + Sync),
) -> Result<Report, ExportError> {
    export_movie_picked(project, shot, settings, None, &Naming::default(), dest, if_exists, folder, progress)
}

/// [`export_movie`] of some of the shot's passes: only what `pick` names is in the picture,
/// and a pass that is rendered as an output of its own lies with the other shots' movies of
/// that pass, in a folder of the pass's name. `naming` says what it is called, if not as the
/// shot is, and the folder under `dest`.
#[allow(clippy::too_many_arguments)]
pub fn export_movie_picked(
    project: &Project,
    shot: ShotId,
    settings: &Settings,
    pick: Option<&crate::Pick>,
    naming: &Naming,
    dest: &Path,
    if_exists: IfExists,
    folder: Option<&Path>,
    progress: &(dyn Fn(usize, usize) -> bool + Sync),
) -> Result<Report, ExportError> {
    let Some(of) = project.shot(shot) else { return Err(ExportError::Invalid(format!("no shot {}", shot.0))) };
    if !of.loaded {
        return Err(ExportError::Invalid(format!("the drawings of `{}` are not loaded: load the shot first", of.name)));
    }
    picture(project, settings)?;
    let frames = frames_of(project, shot, settings.range).ok_or_else(|| ExportError::Invalid(format!("no shot {}", shot.0)))?;
    // A pass of its own lies in the folder of its name, named for the shot and the pass.
    let (dest, own) = (naming.dest(dest), naming.named(&of.name));
    let (dest, named) = match pick.and_then(|pick| pick.named.as_deref()) {
        Some(pass) => (dest.join(file_name(pass)), format!("{}_{}", file_name(&own), file_name(pass))),
        None => (dest, own),
    };
    let Some(path) = place(&dest, &named, settings.format, if_exists)? else {
        return Ok(Report { folder: dest, frames, files: Vec::new(), notes: Vec::new() });
    };
    let mut notes: Vec<String> = nothing_of(of, settings.work).into_iter().collect();
    let (clips, from) = if settings.sound { shot_sound(project, shot, frames.0, folder, &mut notes) } else { (Vec::new(), 0.0) };
    // Only what was picked of the shot is seen; all of it, if nothing was.
    let part = of.only(settings.work);
    let picked = pick.and_then(|pick| pick.of(&part));
    let seen = picked.as_ref().unwrap_or(&part);
    let film = Film { frames: (frames.0..frames.1).map(|at| Seen::Shot(seen, at)).collect(), clips, from, first: frames.0 };
    write_movie(project, &film, &path, settings, progress)?;
    Ok(Report { folder: dest, frames, files: vec![path], notes })
}

/// Render a storyboard of `project` to one movie file in `dest`, named for the storyboard: its
/// shots in the cut, one after another as they lie, black where there is none, with all of
/// its sound (plan §7.14), and its transitions mixed in: this is the film to watch, where the
/// shots' own renders have none and leave them to the edit list. Every
/// shot of the storyboard has to be loaded. The rest is as for [`export_movie`]; the frames
/// that are reported are the film's.
pub fn export_storyboard(
    project: &Project,
    storyboard: iskra_model::StoryboardId,
    settings: &Settings,
    dest: &Path,
    if_exists: IfExists,
    folder: Option<&Path>,
    progress: &(dyn Fn(usize, usize) -> bool + Sync),
) -> Result<Report, ExportError> {
    export_storyboard_named(project, storyboard, settings, None, &Naming::default(), dest, if_exists, folder, progress)
}

/// [`export_storyboard`] with a name that is not the storyboard's, or into a folder under
/// `dest`, as `naming` says; and with `pick`, of one pass: each shot through its pass of that
/// name, and a shot that has none whole. A movie of a pass lies in a folder of the pass's
/// name, named for the storyboard and the pass, beside the shots' movies of it.
#[allow(clippy::too_many_arguments)]
pub fn export_storyboard_named(
    project: &Project,
    storyboard: iskra_model::StoryboardId,
    settings: &Settings,
    pick: Option<&crate::Pick>,
    naming: &Naming,
    dest: &Path,
    if_exists: IfExists,
    folder: Option<&Path>,
    progress: &(dyn Fn(usize, usize) -> bool + Sync),
) -> Result<Report, ExportError> {
    let Some(board) = project.storyboard(storyboard) else { return Err(ExportError::Invalid(format!("no storyboard {}", storyboard.0))) };
    let (dest, own) = (naming.dest(dest), naming.named(&board.name));
    let (dest, named) = match pick.and_then(|pick| pick.named.as_deref()) {
        Some(pass) => (dest.join(file_name(pass)), format!("{}_{}", file_name(&own), file_name(pass))),
        None => (dest, own),
    };
    let dest = &dest;
    picture(project, settings)?;
    let length = board.end();
    if length == 0 {
        return Err(ExportError::Invalid(format!("`{}` has no shots to render", board.name)));
    }
    // Each shot as the part of it that is rendered: its animation, or for an animatic its boards.
    let mut shots = Vec::new();
    for clip in board.clips() {
        let Some(shot) = project.shot(clip.shot) else { continue };
        if !shot.loaded {
            return Err(ExportError::Invalid(format!("the drawings of `{}` are not loaded: load the shot first", shot.name)));
        }
        let part = shot.only(settings.work);
        let part = pick.and_then(|pick| pick.of(&part)).unwrap_or(part);
        shots.push((*clip, part, nothing_of(shot, settings.work)));
    }
    let mut frames = vec![Seen::Black; length as usize];
    for (clip, shot, _) in &shots {
        for at in clip.start..clip.end() {
            frames[at as usize] = Seen::Shot(shot, clip.in_point + (at - clip.start));
        }
    }
    // Inside a transition, both shots, mixed (plan §7.2b).
    let part = |of: Option<(iskra_model::ShotId, u32)>| of.and_then(|(id, at)| shots.iter().find(|(clip, ..)| clip.shot == id).map(|(_, shot, _)| (shot, at)));
    for lying in board.lying() {
        for at in lying.start..lying.end.min(length) {
            if let Some(mixed) = board.mixed_at(at) {
                frames[at as usize] = Seen::Mixed { kind: mixed.kind, at: mixed.at, outgoing: part(mixed.outgoing), incoming: part(mixed.incoming) };
            }
        }
    }
    let Some(path) = place(dest, &named, settings.format, if_exists)? else {
        return Ok(Report { folder: dest.to_path_buf(), frames: (0, length), files: Vec::new(), notes: Vec::new() });
    };
    let mut notes: Vec<String> = shots.iter().filter_map(|(.., nothing)| nothing.clone()).collect();
    // All of the storyboard's sound, from the film's first frame.
    let clips = match board.clips().first().filter(|_| settings.sound) {
        Some(first) => shot_sound(project, first.shot, first.in_point, folder, &mut notes).0,
        None => Vec::new(),
    };
    write_movie(project, &Film { frames, clips, from: 0.0, first: 0 }, &path, settings, progress)?;
    Ok(Report { folder: dest.to_path_buf(), frames: (0, length), files: vec![path], notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aac_is_written_at_up_to_96000_samples_a_second() {
        // A tone of 1 kHz, a second of it, through the encoder and back through the decoder.
        for rate in crate::SOUND_RATES {
            let tone: Vec<f32> = (0..rate as usize).map(|at| 0.5 * (std::f32::consts::TAU * 1000.0 * at as f32 / rate as f32).sin()).collect();
            let mut encoder = filmcraft_aac::Encoder::new(filmcraft_aac::EncoderConfig::cbr(rate, 2, 256_000)).unwrap();
            let mut decoder = filmcraft_aac::Decoder::new(&encoder.audio_specific_config()).unwrap();
            let mut units = encoder.encode(&[&tone, &tone]);
            units.extend(encoder.flush());
            let heard: Vec<f32> = units.iter().flat_map(|unit| decoder.decode(unit).unwrap().swap_remove(0)).collect();
            let skip = encoder.priming_samples() as usize;
            let (signal, noise) = tone
                .iter()
                .zip(&heard[skip..])
                .skip(2048)
                .fold((0.0f64, 0.0f64), |(signal, noise), (want, got)| (signal + f64::from(*want).powi(2), noise + f64::from(want - got).powi(2)));
            let snr = 10.0 * (signal / noise).log10();
            assert!(snr > 30.0, "{rate}: {snr:.1} dB");
        }
    }

    #[test]
    fn h264_is_held_to_a_data_rate_or_else_made_at_a_quality() {
        use filmcraft_h264enc::RateControl;
        let settings = Settings { format: Format::Mp4, quality: Quality::Low, ..Settings::default() };
        assert_eq!(h264_rate(&settings), RateControl::Crf(28.0));
        assert_eq!(h264_rate(&Settings { video_kbps: Some(6000), ..settings }), RateControl::Cbr { kbps: 6000 });
    }
}
