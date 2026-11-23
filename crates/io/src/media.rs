//! Bringing pictures and sound in (plan §4.1, R7): what a file is, read from its beginning, and
//! which files are the frames of one image sequence. Nothing here keeps a file's pixels or
//! samples: a media item refers to its file.

use std::io::Read;
use std::path::{Path, PathBuf};

use iskra_model::MediaKind;

/// The file name endings of the pictures that are read. A Photoshop file is read as the one
/// flat picture it holds of itself; its layers come with the character import (plan §4.14).
/// A TGA has nothing at its head that says what it is, and is known by its name alone.
pub const IMAGE_EXTENSIONS: [&str; 10] = ["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp", "psd", "psb", "tga"];

/// A reader of the picture at `path`, of the kind its bytes say, or a TGA by its name.
fn picture_reader(path: &Path) -> std::io::Result<image::ImageReader<std::io::BufReader<std::fs::File>>> {
    let file = std::fs::File::open(path)?;
    let mut reader = image::ImageReader::new(std::io::BufReader::new(file)).with_guessed_format()?;
    if reader.format().is_none() && has_extension(path, &["tga"]) {
        reader.set_format(image::ImageFormat::Tga);
    }
    Ok(reader)
}

/// Whether `head` begins as a Photoshop file does, PSD or PSB.
fn is_photoshop(head: &[u8]) -> bool {
    head.len() >= 26 && &head[0..4] == b"8BPS" && matches!(head[4..6], [0, 1] | [0, 2])
}

/// The file name endings of the sound that is read.
pub const SOUND_EXTENSIONS: [&str; 2] = ["wav", "wave"];

fn has_extension(path: &Path, of: &[&str]) -> bool {
    path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| of.iter().any(|known| extension.eq_ignore_ascii_case(known)))
}

/// Whether a file's name says it is a picture or a sound that can be brought in.
pub fn is_media(path: &Path) -> bool {
    has_extension(path, &IMAGE_EXTENSIONS) || has_extension(path, &SOUND_EXTENSIONS)
}

/// How much of a sound file is read to find out what it is.
const SOUND_HEAD: u64 = 64 * 1024;

/// What the file at `path` is: a picture with its size, or a sound with its length. It is told
/// by what the file holds, not by its name, and only its beginning is read.
pub fn probe(path: &Path) -> Result<MediaKind, String> {
    let unreadable = |why: &dyn std::fmt::Display| format!("{} could not be read: {why}", path.display());
    let file = std::fs::File::open(path).map_err(|e| unreadable(&e))?;
    let whole = file.metadata().map_err(|e| unreadable(&e))?.len();
    let mut head = Vec::new();
    file.take(SOUND_HEAD).read_to_end(&mut head).map_err(|e| unreadable(&e))?;
    if iskra_audio::is_wav(&head) {
        let info = iskra_audio::read_wav_info(&head, whole).map_err(|e| unreadable(&e))?;
        return Ok(MediaKind::Sound { sample_rate: info.sample_rate, channels: info.channels, frames: info.frames });
    }
    // A Photoshop file says its size at its head: height, then width.
    if is_photoshop(&head) {
        let number = |at: usize| u32::from_be_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]);
        return Ok(MediaKind::Image { width: number(18), height: number(14) });
    }
    // By what it holds: a reader that is given the name would believe its ending.
    let reader = picture_reader(path).map_err(|e| unreadable(&e))?;
    if reader.format().is_none() {
        return Err(format!("{} is not a picture or a sound that Iskra reads (PNG, JPEG, TIFF, BMP, WebP, PSD, TGA; WAV)", path.display()));
    }
    let (width, height) = reader.into_dimensions().map_err(|e| unreadable(&e))?;
    Ok(MediaKind::Image { width, height })
}

/// A picture's pixels: its width and height, and its pixels as straight RGBA bytes, the top
/// row first.
pub fn read_picture(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let unreadable = |why: &dyn std::fmt::Display| format!("{} could not be read: {why}", path.display());
    let mut file = std::fs::File::open(path).map_err(|e| unreadable(&e))?;
    let mut head = [0u8; 26];
    let photoshop = file.read_exact(&mut head).is_ok() && is_photoshop(&head);
    if photoshop {
        // The flat picture Photoshop keeps of the whole document, beside its layers.
        let bytes = std::fs::read(path).map_err(|e| unreadable(&e))?;
        let flat = photocraft_psd::PsdFile::from_bytes(&bytes).and_then(|file| file.composite_rgba8()).map_err(|e| unreadable(&e))?;
        return Ok((flat.width, flat.height, flat.data));
    }
    let reader = picture_reader(path).map_err(|e| unreadable(&e))?;
    let picture = reader.decode().map_err(|e| unreadable(&e))?.into_rgba8();
    Ok((picture.width(), picture.height(), picture.into_raw()))
}

/// A file name with a frame number in it: what is before the number, the number as written,
/// and what is after. Ported from FilmCraft's `crates/media/src/sequence.rs` (MIT OR
/// Apache-2.0; see `ATTRIBUTION.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Numbered {
    prefix: String,
    number: u64,
    /// How many digits the number is written with.
    digits: usize,
    /// Everything after the digits, the ending included.
    suffix: String,
}

impl Numbered {
    /// Split a file name at the last run of digits before its ending.
    fn parse(name: &str) -> Option<Numbered> {
        let ending = name.rfind('.').filter(|at| *at > 0)?;
        let stem = &name[..ending];
        let end = stem.rfind(|c: char| c.is_ascii_digit())? + 1;
        let start = stem[..end].rfind(|c: char| !c.is_ascii_digit()).map_or(0, |at| at + 1);
        let digits = &stem[start..end];
        if digits.len() > 18 {
            return None;
        }
        Some(Numbered { prefix: stem[..start].to_string(), number: digits.parse().ok()?, digits: digits.len(), suffix: name[end..].to_string() })
    }

    /// The number of another file of the same sequence: the same name around a number that is
    /// written as wide, or wider without zeros in front.
    fn number_of(&self, name: &str) -> Option<u64> {
        let number = name.strip_prefix(self.prefix.as_str())?.strip_suffix(self.suffix.as_str())?;
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) || number.len() > 18 {
            return None;
        }
        if number.len() != self.digits && (number.len() < self.digits || number.starts_with('0')) {
            return None;
        }
        number.parse().ok()
    }
}

/// The most frames a sequence is taken to have: longer, and the number in the name is
/// something else, a date say.
const LONGEST_SEQUENCE: u64 = 100_000;

/// The files of the image sequence that the picture at `first` starts: itself and the files
/// beside it that have its name with a higher number, in order, with `None` where a number has
/// no file. `None` if `first` has no number in its name or no such files beside it.
pub fn sequence_from(first: &Path) -> Option<Vec<Option<PathBuf>>> {
    if !has_extension(first, &IMAGE_EXTENSIONS) {
        return None;
    }
    let (folder, name) = (first.parent()?, first.file_name()?.to_str()?);
    let numbered = Numbered::parse(name)?;
    let beside = std::fs::read_dir(folder).ok()?.flatten().filter_map(|entry| entry.file_name().into_string().ok());
    let mut later: Vec<(u64, String)> =
        beside.filter_map(|name| Some((numbered.number_of(&name)?, name))).filter(|(number, _)| *number > numbered.number).collect();
    later.sort();
    later.dedup_by_key(|(number, _)| *number);
    let last = later.last()?.0;
    let mut frames = vec![None; (last - numbered.number).min(LONGEST_SEQUENCE - 1) as usize + 1];
    frames[0] = Some(first.to_path_buf());
    for (number, name) in later {
        if let Some(frame) = frames.get_mut((number - numbered.number) as usize) {
            *frame = Some(folder.join(name));
        }
    }
    Some(frames)
}

/// The image sequence that the picture at `first` starts, as a media item has it: the size of
/// its first picture, and how many frames it runs over.
pub fn probe_sequence(first: &Path) -> Result<MediaKind, String> {
    let frames = sequence_from(first).ok_or_else(|| format!("{} is not the first of numbered pictures", first.display()))?;
    match probe(first)? {
        MediaKind::Image { width, height } => Ok(MediaKind::ImageSequence { width, height, frames: frames.len() as u32 }),
        _ => Err(format!("{} is not a picture", first.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(path: &Path, width: u32, height: u32) {
        image::RgbaImage::from_pixel(width, height, image::Rgba([200, 40, 40, 255])).save(path).unwrap();
    }

    #[test]
    fn a_file_is_what_it_holds_whatever_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let picture = dir.path().join("sky.png");
        png(&picture, 64, 36);
        assert_eq!(probe(&picture), Ok(MediaKind::Image { width: 64, height: 36 }));
        let sound = dir.path().join("line.wav");
        std::fs::write(&sound, iskra_audio::write_wav16(&vec![0.0; 2 * 12_000], 2, 48_000)).unwrap();
        assert_eq!(probe(&sound), Ok(MediaKind::Sound { sample_rate: 48_000, channels: 2, frames: 12_000 }));
        // A picture under a sound's name is still a picture; a text is neither.
        let misnamed = dir.path().join("really a picture.wav");
        std::fs::copy(&picture, &misnamed).unwrap();
        assert_eq!(probe(&misnamed), Ok(MediaKind::Image { width: 64, height: 36 }));
        let notes = dir.path().join("notes.png");
        std::fs::write(&notes, "the ball bounces twice").unwrap();
        assert!(probe(&notes).is_err_and(|why| why.contains("not a picture or a sound")));
        assert!(probe(&dir.path().join("gone.png")).is_err());
        assert!(is_media(&picture) && is_media(Path::new("LINE.WAV")) && !is_media(Path::new("film.iskra")));
    }

    #[test]
    fn a_photoshop_file_is_read_as_the_flat_picture_it_holds() {
        use photocraft_psd::{LayerSpec, PixelData, PsdBuilder};
        let dir = tempfile::tempdir().unwrap();
        // Three by two: a red layer over all of it, and a flat picture that is half clear.
        let layer: Vec<u8> = [255, 0, 0, 255].repeat(6);
        let flat: Vec<u8> = (0..6).flat_map(|pixel| [200, 40, 40, if pixel < 3 { 255 } else { 128 }]).collect();
        let mut builder = PsdBuilder::new(3, 2);
        builder.push_layer(LayerSpec::new("Red", 0, 0, 3, 2, PixelData::Rgba8(layer)));
        builder.composite(PixelData::Rgba8(flat.clone()));
        let path = dir.path().join("design.psd");
        std::fs::write(&path, builder.to_bytes().unwrap()).unwrap();
        assert_eq!(probe(&path), Ok(MediaKind::Image { width: 3, height: 2 }), "its size is read from its head");
        // The file holds a half clear colour matted against white, in 8 bits, so it comes back
        // within a step of what it was.
        let (width, height, read) = read_picture(&path).unwrap();
        assert_eq!((width, height), (3, 2));
        assert!(read.iter().zip(&flat).all(|(read, was)| read.abs_diff(*was) <= 1), "{read:?}");
        assert!(is_media(&path) && is_media(Path::new("big.PSB")));
        // A file that only starts like one is said to be unreadable, not taken for a picture.
        let cut = dir.path().join("cut.psd");
        std::fs::write(&cut, &std::fs::read(&path).unwrap()[..40]).unwrap();
        assert!(probe(&cut).is_ok() && read_picture(&cut).is_err_and(|why| why.contains("could not be read")));
    }

    #[test]
    fn numbered_pictures_beside_each_other_are_a_sequence() {
        let dir = tempfile::tempdir().unwrap();
        // walk_0001 to walk_0005 without 0003, a picture of another name, and one before the first.
        for name in ["walk_0000.png", "walk_0001.png", "walk_0002.png", "walk_0004.png", "walk_0005.png", "run_0002.png"] {
            png(&dir.path().join(name), 32, 18);
        }
        std::fs::write(dir.path().join("walk_0003.txt"), "notes on frame 3").unwrap();
        let first = dir.path().join("walk_0001.png");
        let frames = sequence_from(&first).unwrap();
        let names: Vec<Option<String>> = frames.iter().map(|frame| Some(frame.as_ref()?.file_name()?.to_string_lossy().into_owned())).collect();
        let named = |name: &str| Some(name.to_string());
        assert_eq!(names, [named("walk_0001.png"), named("walk_0002.png"), None, named("walk_0004.png"), named("walk_0005.png")]);
        assert_eq!(probe_sequence(&first), Ok(MediaKind::ImageSequence { width: 32, height: 18, frames: 5 }));
        // The last of them starts no sequence, nor does a picture with no number.
        assert_eq!(sequence_from(&dir.path().join("walk_0005.png")), None);
        png(&dir.path().join("sky.png"), 8, 8);
        assert!(sequence_from(&dir.path().join("sky.png")).is_none() && probe_sequence(&dir.path().join("sky.png")).is_err());
        // Numbers that grow past their zeros still belong: 8, 9, 10.
        for name in ["f8.png", "f9.png", "f10.png", "f011.png"] {
            png(&dir.path().join(name), 8, 8);
        }
        assert_eq!(sequence_from(&dir.path().join("f8.png")).map(|frames| frames.len()), Some(3));
    }
}
