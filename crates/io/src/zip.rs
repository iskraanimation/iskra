//! Reads a ZIP archive, as far as the layered files that are kept in one need (OpenRaster):
//! the files it lists, each stored as it is or deflated. Archives of the 64-bit kind, in
//! several parts or encrypted are not read.

use std::io::Read;

/// The most bytes one file of an archive is unpacked to: a picture far larger than any layer.
const MOST: u64 = 1 << 31;

/// A file of an archive: how it is packed, and where.
struct Entry {
    name: String,
    deflated: bool,
    /// Where its local header starts.
    header: usize,
    packed: usize,
    size: usize,
}

/// An archive that was read whole.
pub(crate) struct Zip {
    bytes: Vec<u8>,
    entries: Vec<Entry>,
}

fn u16_at(bytes: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?)))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<usize> {
    usize::try_from(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?)).ok()
}

impl Zip {
    /// The archive in `bytes`: the list of its files, from the directory at its end.
    pub(crate) fn open(bytes: Vec<u8>) -> Result<Zip, String> {
        let entries = Self::directory(&bytes).ok_or("it is not a ZIP archive that can be read")?;
        Ok(Zip { bytes, entries })
    }

    fn directory(bytes: &[u8]) -> Option<Vec<Entry>> {
        // The end record is the last thing in the file, but for a comment of at most 65535 bytes.
        let last = bytes.len().checked_sub(22)?;
        let end = (last.saturating_sub(65535)..=last).rev().find(|at| bytes[*at..*at + 4] == [0x50, 0x4b, 0x05, 0x06])?;
        let (count, mut at) = (u16_at(bytes, end + 10)?, u32_at(bytes, end + 16)?);
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            if bytes.get(at..at + 4)? != [0x50, 0x4b, 0x01, 0x02] {
                return None;
            }
            let (flags, method) = (u16_at(bytes, at + 8)?, u16_at(bytes, at + 10)?);
            let (packed, size) = (u32_at(bytes, at + 20)?, u32_at(bytes, at + 24)?);
            let (name, extra, comment) = (u16_at(bytes, at + 28)?, u16_at(bytes, at + 30)?, u16_at(bytes, at + 32)?);
            let header = u32_at(bytes, at + 42)?;
            // Encrypted, or packed another way than the two that are read.
            if flags & 1 != 0 || !matches!(method, 0 | 8) {
                return None;
            }
            let name = String::from_utf8_lossy(bytes.get(at + 46..at + 46 + name)?).into_owned();
            at += 46 + name.len() + extra + comment;
            entries.push(Entry { name, deflated: method == 8, header, packed, size });
        }
        Some(entries)
    }

    /// The bytes of the file called `name`.
    pub(crate) fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        let entry = self.entries.iter().find(|entry| entry.name == name).ok_or_else(|| format!("it has no `{name}`"))?;
        let broken = || format!("`{name}` is cut off");
        let header = entry.header;
        let (named, extra) = (u16_at(&self.bytes, header + 26).ok_or_else(broken)?, u16_at(&self.bytes, header + 28).ok_or_else(broken)?);
        let from = header + 30 + named + extra;
        let packed = self.bytes.get(from..from + entry.packed).ok_or_else(broken)?;
        if !entry.deflated {
            return Ok(packed.to_vec());
        }
        let mut bytes = Vec::with_capacity(entry.size.min(1 << 24));
        flate2::read::DeflateDecoder::new(packed).take(MOST).read_to_end(&mut bytes).map_err(|why| format!("`{name}` could not be unpacked: {why}"))?;
        Ok(bytes)
    }
}

/// An archive of `files`, each stored as it is: what the tests read.
#[cfg(test)]
pub(crate) fn stored(files: &[(&str, &[u8])]) -> Vec<u8> {
    let crc = |bytes: &[u8]| {
        let mut crc = !0u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & (!(crc & 1)).wrapping_add(1));
            }
        }
        !crc
    };
    let (mut bytes, mut directory) = (Vec::new(), Vec::new());
    for (name, data) in files {
        let (at, size) = (bytes.len() as u32, data.len() as u32);
        // Version, flags, method, time, date; then the sum, both sizes, and the name's length.
        let mut fields = vec![20, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        fields.extend(crc(data).to_le_bytes());
        fields.extend([size.to_le_bytes(), size.to_le_bytes()].concat());
        fields.extend((name.len() as u16).to_le_bytes());
        bytes.extend([0x50, 0x4b, 0x03, 0x04]);
        bytes.extend(&fields);
        bytes.extend([0, 0]);
        bytes.extend(name.as_bytes());
        bytes.extend(*data);
        directory.extend([0x50, 0x4b, 0x01, 0x02, 20, 0]);
        directory.extend(&fields);
        // No extra field, comment, disk or attributes; then where the file starts.
        directory.extend([0; 12]);
        directory.extend(at.to_le_bytes());
        directory.extend(name.as_bytes());
    }
    let (start, count) = (bytes.len() as u32, files.len() as u16);
    let length = directory.len() as u32;
    bytes.extend(directory);
    bytes.extend([0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
    bytes.extend([count.to_le_bytes(), count.to_le_bytes()].concat());
    bytes.extend([length.to_le_bytes(), start.to_le_bytes()].concat());
    bytes.extend([0, 0]);
    bytes
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn the_files_of_an_archive_are_read_stored_or_deflated() {
        let mut bytes = stored(&[("mimetype", b"image/openraster"), ("data/a.txt", b"hello")]);
        let zip = Zip::open(bytes.clone()).unwrap();
        assert_eq!((zip.read("mimetype").unwrap(), zip.read("data/a.txt").unwrap()), (b"image/openraster".to_vec(), b"hello".to_vec()));
        assert!(zip.read("none").is_err_and(|why| why.contains("no `none`")));
        // A deflated file: the same archive with its one file packed, and marked so.
        let text = b"again and again and again and again and again".to_vec();
        let mut packer = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        packer.write_all(&text).unwrap();
        let packed = packer.finish().unwrap();
        let mut deflated = stored(&[("t", &packed)]);
        let directory = deflated.len() - 22 - 47;
        (deflated[8], deflated[directory + 10]) = (8, 8);
        deflated[directory + 24..directory + 28].copy_from_slice(&(text.len() as u32).to_le_bytes());
        assert_eq!(Zip::open(deflated).unwrap().read("t").unwrap(), text);
        // What is no archive, or is cut off, says so.
        assert!(Zip::open(b"not an archive at all, only text".to_vec()).is_err());
        bytes.truncate(bytes.len() - 30);
        assert!(Zip::open(bytes).is_err());
    }
}
