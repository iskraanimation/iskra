//! The fonts a text can be set in (plan §4.19): those installed on the machine, found by
//! reading the system's font folders, and Inter, built in, which stands in for a font the
//! machine lacks.
//!
//! Finding them reads only the table of names of each file, not the file: some are hundreds of
//! megabytes. A font's file is read whole the first time a text is set in it.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// The font that is built in, and stands in for one the machine lacks.
pub const BUILT_IN_FAMILY: &str = "Inter";
pub const REGULAR: &str = "Regular";

const INTER_REGULAR: &[u8] = include_bytes!("../../ui/assets/fonts/Inter-Regular.ttf");
const INTER_SEMI_BOLD: &[u8] = include_bytes!("../../ui/assets/fonts/Inter-SemiBold.ttf");

/// One face of a font family: its regular, its bold, its italic…
#[derive(Debug)]
pub struct Face {
    pub family: String,
    pub style: String,
    source: Source,
    data: OnceLock<Option<Arc<[u8]>>>,
}

#[derive(Debug)]
enum Source {
    BuiltIn(&'static [u8]),
    /// A file, and the face's place in it where it is a collection.
    File(PathBuf, u32),
}

impl Face {
    fn built_in(style: &str, bytes: &'static [u8]) -> Face {
        Face { family: BUILT_IN_FAMILY.to_string(), style: style.to_string(), source: Source::BuiltIn(bytes), data: OnceLock::new() }
    }

    /// The bytes of its file, read now and not kept, and its place in it: for a look at many
    /// faces once, as a list of previews, which would keep them all.
    pub fn read(&self) -> Option<(Arc<[u8]>, u32)> {
        match (&self.source, self.data.get()) {
            (_, Some(Some(data))) => Some((data.clone(), self.index())),
            (Source::BuiltIn(bytes), _) => Some((Arc::from(*bytes), 0)),
            (Source::File(path, index), _) => std::fs::read(path).ok().map(|data| (Arc::from(data), *index)),
        }
    }

    fn index(&self) -> u32 {
        match &self.source {
            Source::BuiltIn(_) => 0,
            Source::File(_, index) => *index,
        }
    }

    /// The bytes of its file, and its place in it. `None` if the file can no longer be read.
    pub fn data(&self) -> Option<(Arc<[u8]>, u32)> {
        let (data, index) = match &self.source {
            Source::BuiltIn(bytes) => (self.data.get_or_init(|| Some(Arc::from(*bytes))), 0),
            Source::File(path, index) => (self.data.get_or_init(|| std::fs::read(path).ok().map(Arc::from)), *index),
        };
        data.clone().map(|data| (data, index))
    }
}

/// The fonts that can be had, each family's faces together.
#[derive(Debug, Default)]
pub struct Fonts {
    faces: Vec<Face>,
}

impl Fonts {
    /// Inter alone: what every machine has, for tests that must not depend on the machine.
    pub fn built_in() -> Fonts {
        Fonts { faces: vec![Face::built_in(REGULAR, INTER_REGULAR), Face::built_in("SemiBold", INTER_SEMI_BOLD)] }
    }

    /// Inter and the fonts installed on this machine, found the first time they are asked for.
    pub fn system() -> &'static Fonts {
        static FONTS: OnceLock<Fonts> = OnceLock::new();
        FONTS.get_or_init(|| {
            let mut fonts = Fonts::built_in();
            for folder in font_folders() {
                fonts.find_in(&folder, 0);
            }
            fonts.faces.sort_by(|a, b| (a.family.to_lowercase(), &a.style).cmp(&(b.family.to_lowercase(), &b.style)));
            fonts.faces.dedup_by(|a, b| a.family == b.family && a.style == b.style);
            fonts
        })
    }

    fn find_in(&mut self, folder: &Path, depth: u32) {
        let Ok(entries) = std::fs::read_dir(folder) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < 4 {
                    self.find_in(&path, depth + 1);
                }
                continue;
            }
            let font = path.extension().and_then(|e| e.to_str()).is_some_and(|e| ["ttf", "otf", "ttc", "otc"].contains(&e.to_ascii_lowercase().as_str()));
            if !font {
                continue;
            }
            for (index, (family, style)) in names_in(&path).into_iter().enumerate() {
                // The built-in Inter comes first, so a text set in it looks the same everywhere.
                // A family whose name starts with a dot is the system's own, for its interface,
                // which macOS leaves out of every list of fonts.
                if family != BUILT_IN_FAMILY && !family.starts_with('.') {
                    self.faces.push(Face { family, style, source: Source::File(path.clone(), index as u32), data: OnceLock::new() });
                }
            }
        }
    }

    pub fn faces(&self) -> &[Face] {
        &self.faces
    }

    /// The names of the families, in order, each once.
    pub fn families(&self) -> Vec<&str> {
        let mut families: Vec<&str> = self.faces.iter().map(|face| face.family.as_str()).collect();
        families.dedup();
        families
    }

    /// The styles a family has.
    pub fn styles(&self, family: &str) -> Vec<&str> {
        self.faces.iter().filter(|face| face.family == family).map(|face| face.style.as_str()).collect()
    }

    /// The face of `family` in `style`, or else the family's regular, or else any of it.
    pub fn face(&self, family: &str, style: &str) -> Option<&Face> {
        let of = || self.faces.iter().filter(|face| face.family == family);
        of().find(|face| face.style == style).or_else(|| of().find(|face| face.style == REGULAR)).or_else(|| of().next())
    }

    /// The face to set a text in: [`Fonts::face`], or Inter where the machine lacks the family.
    /// Says which it is: `true` where it is a stand-in.
    pub fn face_or_stand_in(&self, family: &str, style: &str) -> Option<(&Face, bool)> {
        match self.face(family, style) {
            Some(face) => Some((face, false)),
            None => self.face(BUILT_IN_FAMILY, style).map(|face| (face, true)),
        }
    }
}

/// Where the system keeps its fonts and the user's own.
fn font_folders() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut folders = Vec::new();
    if cfg!(target_os = "macos") {
        folders.extend(["/System/Library/Fonts", "/Library/Fonts"].map(PathBuf::from));
        folders.extend(home.map(|home| home.join("Library/Fonts")));
    } else if cfg!(windows) {
        let windows = std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        folders.push(windows.join("Fonts"));
        folders.extend(std::env::var_os("LOCALAPPDATA").map(|local| PathBuf::from(local).join(r"Microsoft\Windows\Fonts")));
    } else {
        folders.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(PathBuf::from));
        folders.extend(home.into_iter().flat_map(|home| [home.join(".local/share/fonts"), home.join(".fonts")]));
    }
    folders
}

/// The family and style names of each face of the font file at `path`, in their order in the
/// file. Empty if it can't be read as one.
fn names_in(path: &Path) -> Vec<(String, String)> {
    let Ok(mut file) = File::open(path) else { return Vec::new() };
    let Some(head) = read_at(&mut file, 0, 12) else { return Vec::new() };
    let offsets = if &head[0..4] == b"ttcf" {
        let count = u32_at(&head, 8).min(256);
        let Some(table) = read_at(&mut file, 12, count as usize * 4) else { return Vec::new() };
        (0..count as usize).map(|i| u32_at(&table, i * 4)).collect()
    } else {
        vec![0]
    };
    offsets.into_iter().filter_map(|offset| face_names(&mut file, offset as u64)).collect()
}

/// The family and style names of the face whose table directory is at `offset` of `file`.
fn face_names(file: &mut File, offset: u64) -> Option<(String, String)> {
    let head = read_at(file, offset, 12)?;
    let tables = u16_at(&head, 4) as usize;
    let records = read_at(file, offset + 12, tables * 16)?;
    let name = (0..tables).map(|i| &records[i * 16..i * 16 + 16]).find(|record| &record[0..4] == b"name")?;
    let (at, length) = (u32_at(name, 8) as u64, (u32_at(name, 12) as usize).min(1 << 20));
    let table = read_at(file, at, length)?;
    let family = name_of(&table, 16).or_else(|| name_of(&table, 1))?;
    let style = name_of(&table, 17).or_else(|| name_of(&table, 2)).unwrap_or_else(|| REGULAR.to_string());
    Some((family, style))
}

/// The name `id` of a font's `name` table: in English where the font has it so, from the
/// Windows or Unicode names, or else from the Mac's.
fn name_of(table: &[u8], id: u16) -> Option<String> {
    if table.len() < 6 {
        return None;
    }
    let (count, strings) = (u16_at(table, 2) as usize, u16_at(table, 4) as usize);
    let records = (0..count).map(|i| 6 + i * 12).take_while(|at| at + 12 <= table.len());
    let records: Vec<[u16; 6]> = records.map(|at| std::array::from_fn(|field| u16_at(table, at + field * 2))).collect();
    let text = |[platform, encoding, _, _, length, offset]: [u16; 6]| {
        let bytes = table.get(strings + offset as usize..strings + offset as usize + length as usize)?;
        match (platform, encoding) {
            (0, _) | (3, 1) | (3, 10) => String::from_utf16(&bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_be_bytes(*pair)).collect::<Vec<_>>()).ok(),
            // Mac Roman: its first half is ASCII, which is what names are in.
            (1, 0) => Some(bytes.iter().map(|byte| if byte.is_ascii() { *byte as char } else { '?' }).collect()),
            _ => None,
        }
    };
    let wanted = records.iter().filter(|record| record[3] == id);
    let english = wanted
        .clone()
        .find(|r| r[0] == 3 && r[2] == 0x409)
        .or_else(|| wanted.clone().find(|r| r[0] == 0))
        .or_else(|| wanted.clone().find(|r| r[0] == 1 && r[2] == 0));
    english.into_iter().chain(wanted).find_map(|record| text(*record)).map(|name| name.trim().to_string()).filter(|name| !name.is_empty())
}

fn read_at(file: &mut File, at: u64, length: usize) -> Option<Vec<u8>> {
    file.seek(SeekFrom::Start(at)).ok()?;
    let mut bytes = vec![0; length];
    file.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_fonts_are_named_from_their_files() {
        let dir = tempfile_dir();
        let path = dir.join("inter.ttf");
        std::fs::write(&path, INTER_SEMI_BOLD).unwrap();
        assert_eq!(names_in(&path), vec![("Inter".to_string(), "SemiBold".to_string())]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_family_the_machine_lacks_is_set_in_inter() {
        let fonts = Fonts::built_in();
        let (face, stand_in) = fonts.face_or_stand_in("No Such Font", "Bold").unwrap();
        assert!(stand_in && face.family == "Inter" && face.style == REGULAR);
        let (face, stand_in) = fonts.face_or_stand_in("Inter", "SemiBold").unwrap();
        assert!(!stand_in && face.style == "SemiBold");
        assert_eq!(fonts.families(), vec!["Inter"]);
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("iskra-fonts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
