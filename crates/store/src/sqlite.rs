//! The SQLite project file.
//!
//! - `document`: the project's structure as JSON (storyboards, shots, pegs, layers,
//!   exposures, palette), without pixels.
//! - `tile`: one row per 256×256 drawing tile, keyed by drawing id, art layer index and tile
//!   position, LZ4-compressed.
//!
//! A save runs in one transaction and writes only what changed since the last save. Tiles are
//! shared between snapshots until they are painted on, so "changed" is a pointer comparison.
//!
//! Opening reads the structure only. A shot's pixels are read when the shot is loaded, and a
//! save leaves the tiles of shots that are not loaded as they are.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use iskra_drawing::{TILE_SIZE, TileCoord};
use iskra_model::{ArtContent, Drawing, DrawingId, PoolDrawing, Project};
use rusqlite::{Connection, OpenFlags, params};

use crate::{LoadReport, ProjectStore, Result, SaveReport, StoreError, backup, migrate};

// Tiles are stored as the bytes they have in memory, which the format defines as little-endian.
#[cfg(target_endian = "big")]
compile_error!("the project file stores tiles little-endian; a big-endian build needs a conversion here");

/// "ISKR": marks the SQLite file as an Iskra project (`PRAGMA application_id`).
const APPLICATION_ID: i64 = 0x4953_4B52;

/// The format version this build writes, and the newest it reads (`PRAGMA user_version`). Older
/// files are converted as they are read (see `migrate`).
pub(crate) const FORMAT_VERSION: i64 = 62;

/// Bytes in a stored tile, once decompressed: both pixel kinds use 4 bytes a pixel.
const TILE_BYTES: usize = (TILE_SIZE * TILE_SIZE) as usize * 4;

const SCHEMA: &str = "
    CREATE TABLE document (name TEXT PRIMARY KEY, json TEXT NOT NULL) WITHOUT ROWID;
    CREATE TABLE tile (
        drawing INTEGER NOT NULL,
        layer   INTEGER NOT NULL,
        tx      INTEGER NOT NULL,
        ty      INTEGER NOT NULL,
        data    BLOB NOT NULL,
        PRIMARY KEY (drawing, layer, tx, ty)
    ) WITHOUT ROWID;
    CREATE TABLE vector (
        drawing INTEGER NOT NULL,
        layer   INTEGER NOT NULL,
        data    BLOB NOT NULL,
        PRIMARY KEY (drawing, layer)
    ) WITHOUT ROWID;
";

/// The table of vector art (plan §4.16), which files before version 47 don't have: made in
/// them by their first save in this build.
const VECTOR_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS vector (
        drawing INTEGER NOT NULL,
        layer   INTEGER NOT NULL,
        data    BLOB NOT NULL,
        PRIMARY KEY (drawing, layer)
    ) WITHOUT ROWID;
";

/// The tips that vector strokes are stamped with (the user's of 2026-11-23), each once, by its
/// key: its width and height and its samples, `u16` in native byte order, compressed. Files
/// without it have no stamped strokes, and it is made in them by their first save.
const TIP_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS tip (
        key    TEXT PRIMARY KEY,
        width  INTEGER NOT NULL,
        height INTEGER NOT NULL,
        data   BLOB NOT NULL
    ) WITHOUT ROWID;
";

/// Whether the file has the table of tips.
fn has_tip_table(conn: &Connection) -> bool {
    conn.query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'tip'", [], |_| Ok(())).is_ok()
}

/// Put the tips the file keeps in the table of tips, where vector strokes find them. One that
/// can't be read is skipped and reported: its strokes are drawn as plain lines.
fn read_tips(conn: &Connection, report: &mut LoadReport) -> Result<()> {
    if !has_tip_table(conn) {
        return Ok(());
    }
    let mut select = conn.prepare("SELECT key, width, height, data FROM tip")?;
    let rows = select.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, Vec<u8>>(3)?)))?;
    for row in rows {
        let Ok((key, width, height, data)) = row else { continue };
        let samples = (width.max(0) * height.max(0)) as usize;
        let tile = (width > 0 && height > 0 && width <= 4096 && height <= 4096)
            .then(|| lz4_flex::decompress_size_prepended(&data).ok())
            .flatten()
            .filter(|bytes| bytes.len() == samples * 2)
            .map(|bytes| iskra_drawing::GrayTile {
                width: width as u32,
                height: height as u32,
                data: bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect::<Vec<u16>>().into(),
            });
        match tile {
            Some(tile) => iskra_drawing::remember_tip_as(&key, &tile),
            None => report.skipped.push(format!("tip {key}: it can't be read, so its strokes are drawn as plain lines")),
        }
    }
    Ok(())
}

/// The most a stored art layer of vector art may hold once decompressed, so that a damaged size
/// can't ask for gigabytes.
const MOST_VECTOR_BYTES: usize = 256 << 20;

pub struct SqliteStore {
    path: PathBuf,
    pub(crate) conn: Connection,
    /// Every drawing the file has, with its pixels as last saved or loaded: what the next save
    /// is compared with. `None` for a drawing whose pixels are in the file and not in memory.
    saved: HashMap<DrawingId, Option<Arc<Drawing>>>,
    /// The snapshot last saved, to know it again. It is not kept alive.
    last: Weak<Project>,
    /// The structure JSON as last written.
    saved_json: String,
    /// The format version the file has. An older file gets this build's with its next save.
    version: i64,
    /// The backup is a copy of the file as it is now.
    backed_up: bool,
}

/// Whether two art layers hold the same kind of art, stored the same way.
fn same_kind(a: &ArtContent, b: &ArtContent) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// Whether the file has the table of vector art.
fn has_vector_table(conn: &Connection) -> bool {
    conn.query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'vector'", [], |_| Ok(())).is_ok()
}

impl SqliteStore {
    /// A new, empty project file at `path`, replacing any file there.
    pub(crate) fn new_file(path: &Path) -> Result<Self> {
        match std::fs::remove_file(path) {
            Err(source) if source.kind() != std::io::ErrorKind::NotFound => return Err(StoreError::Io { path: path.to_path_buf(), source }),
            _ => {}
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "application_id", APPLICATION_ID)?;
        conn.pragma_update(None, "user_version", FORMAT_VERSION)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            path: path.to_path_buf(),
            conn,
            saved: HashMap::new(),
            last: Weak::new(),
            saved_json: String::new(),
            version: FORMAT_VERSION,
            backed_up: false,
        })
    }

    /// Create a project file at `path`, replacing any file there, and save `project` into it.
    /// Every shot of `project` must be loaded: a project that has shots still in its file is
    /// saved elsewhere through that file's [`ProjectStore::save_as`].
    pub fn create(path: &Path, project: &Arc<Project>) -> Result<Self> {
        let mut store = Self::new_file(path)?;
        store.save(project)?;
        Ok(store)
    }

    /// Open the project file at `path` after checking that it is one and that it is sound. Only
    /// the structure is read: no shot of the project is loaded yet.
    pub fn open(path: &Path) -> Result<(Self, Arc<Project>)> {
        let io = |source| StoreError::Io { path: path.to_path_buf(), source };
        std::fs::metadata(path).map_err(io)?;
        let damaged = |why: String| StoreError::Damaged { path: path.to_path_buf(), why, backup: backup::backup_of(path) };
        // Without `CREATE`, opening a missing file is an error and never makes an empty one.
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(|e| damaged(e.to_string()))?;
        let version = checked_version(&conn, path)?;
        let check: String = conn.pragma_query_value(None, "integrity_check", |row| row.get(0)).map_err(|e| damaged(e.to_string()))?;
        if check != "ok" {
            return Err(damaged(check));
        }
        let (json, project) = read_structure(&conn, version).map_err(|why| damaged(format!("the project structure can't be read: {why}")))?;
        let project = Arc::new(project);
        let saved = project.shots.iter().flat_map(|s| s.drawings()).map(|d| (d.id, None)).collect();
        let last = Arc::downgrade(&project);
        Ok((Self { path: path.to_path_buf(), conn, saved, last, saved_json: json, version, backed_up: false }, project))
    }

    /// Open a project file and load every shot: for tools and tests that want the whole project
    /// in memory. The app loads shots one at a time.
    pub fn open_whole(path: &Path) -> Result<(Self, Arc<Project>, LoadReport)> {
        let (mut store, project) = Self::open(path)?;
        let drawings: Vec<PoolDrawing> = project.shots.iter().flat_map(|s| s.drawings()).cloned().collect();
        let (pixels, report) = store.load(&drawings)?;
        let mut project = Project::clone(&project);
        for shot in &mut project.shots {
            *shot = Arc::new(shot.with_pixels(&pixels));
        }
        Ok((store, Arc::new(project), report))
    }

    /// Read whatever can still be read from a project or backup file, for recovery. Unlike
    /// [`SqliteStore::open`] this skips the integrity check and carries on past what is damaged:
    /// the report says what was skipped. If the project's structure can't be read, every drawing
    /// found in the file is returned in one layer, named by its id, with the default palette.
    /// The file is opened read-only, and the whole project is loaded.
    pub fn salvage(path: &Path) -> Result<(Project, LoadReport)> {
        std::fs::metadata(path).map_err(|source| StoreError::Io { path: path.to_path_buf(), source })?;
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut report = LoadReport::default();
        // A version that can't be read is taken for this build's.
        let version = conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)).unwrap_or(FORMAT_VERSION);
        let mut project = match read_structure(&conn, version) {
            Ok((_, project)) => project,
            Err(why) => {
                report.skipped.push(format!("the project structure can't be read ({why}); drawings are listed by id, without names or timing"));
                let mut project = Project::default();
                let mut found = conn.prepare("SELECT drawing, max(layer) FROM tile GROUP BY drawing")?;
                let found = found.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
                let layer = Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).layers[0]);
                layer.name = "recovered".to_string();
                for (id, top_layer) in found.flatten() {
                    // The kind of each layer is in the structure; without it, the default is assumed.
                    let art_layer = |_| iskra_model::ArtLayer { kind: iskra_model::ArtLayerKind::LineArt, content: ArtContent::Indexed(Default::default()) };
                    let drawing = Drawing { art_layers: (0..=top_layer.clamp(0, 16)).map(art_layer).collect(), texts: Vec::new(), placements: Vec::new() };
                    layer.drawings.push(PoolDrawing { id: DrawingId(id as u64), name: id.to_string(), drawing: Arc::new(drawing) });
                }
                project
            }
        };
        let drawings: Vec<PoolDrawing> = project.shots.iter().flat_map(|s| s.drawings()).cloned().collect();
        let pixels = read_drawings(&conn, &drawings, &mut report)?;
        for shot in &mut project.shots {
            *shot = Arc::new(shot.with_pixels(&pixels));
        }
        Ok((project, report))
    }

    /// Before a file of an older format is rewritten in this build's: keep it as it is, as
    /// `<name> (format <n>).iskra` beside it. An earlier such copy stands.
    /// Write `project` into the file: what changed since the last write.
    pub(crate) fn write(&mut self, project: &Arc<Project>) -> Result<SaveReport> {
        if Weak::ptr_eq(&self.last, &Arc::downgrade(project)) {
            return Ok(SaveReport::default());
        }
        if self.version != FORMAT_VERSION {
            self.keep_old_format()?;
        }
        self.backed_up = false;
        let mut report = SaveReport::default();
        let json = serde_json::to_string(&**project)?;
        let mut saved = HashMap::new();
        let tx = self.conn.transaction()?;
        {
            tx.execute_batch(VECTOR_TABLE)?;
            tx.execute_batch(TIP_TABLE)?;
            let mut has_tip = tx.prepare("SELECT 1 FROM tip WHERE key = ?1")?;
            let mut insert_tip = tx.prepare("INSERT OR IGNORE INTO tip (key, width, height, data) VALUES (?1, ?2, ?3, ?4)")?;
            // The tips of the vector art written: each once, kept by the file for good.
            let mut tips_known = std::collections::HashSet::new();
            let mut write_tips = |art: &iskra_model::VectorArt| -> rusqlite::Result<()> {
                for key in art.tips() {
                    if !tips_known.insert(key.clone()) || has_tip.exists([&key])? {
                        continue;
                    }
                    let Some(tile) = iskra_drawing::tip(&key) else { continue };
                    let bytes: Vec<u8> = tile.data.iter().flat_map(|sample| sample.to_le_bytes()).collect();
                    insert_tip.execute(params![key, i64::from(tile.width), i64::from(tile.height), lz4_flex::compress_prepend_size(&bytes)])?;
                }
                Ok(())
            };
            let mut upsert = tx.prepare("INSERT OR REPLACE INTO tile (drawing, layer, tx, ty, data) VALUES (?1, ?2, ?3, ?4, ?5)")?;
            let mut delete = tx.prepare("DELETE FROM tile WHERE drawing = ?1 AND layer = ?2 AND tx = ?3 AND ty = ?4")?;
            let mut delete_tile_layers = tx.prepare("DELETE FROM tile WHERE drawing = ?1 AND layer >= ?2")?;
            let mut upsert_vector = tx.prepare("INSERT OR REPLACE INTO vector (drawing, layer, data) VALUES (?1, ?2, ?3)")?;
            let mut delete_vector_layers = tx.prepare("DELETE FROM vector WHERE drawing = ?1 AND layer >= ?2")?;
            // Every art layer of a drawing from `from` on: its tiles and its vector art.
            let mut delete_layers = |id: DrawingId, from: usize| -> rusqlite::Result<usize> {
                delete_vector_layers.execute(params![id.0 as i64, from as i64])?;
                delete_tile_layers.execute(params![id.0 as i64, from as i64])
            };
            for shot in &project.shots {
                for PoolDrawing { id, drawing, .. } in shot.drawings() {
                    if !shot.loaded {
                        // Its pixels are the ones in the file.
                        saved.insert(*id, None);
                        continue;
                    }
                    saved.insert(*id, Some(drawing.clone()));
                    let old = match self.saved.get(id) {
                        Some(Some(old)) if Arc::ptr_eq(old, drawing) => continue,
                        Some(Some(old)) => Some(old),
                        // In the file, but what it holds there is not in memory to compare with.
                        Some(None) => {
                            report.tiles_deleted += delete_layers(*id, 0)?;
                            None
                        }
                        None => None,
                    };
                    for (index, layer) in drawing.art_layers.iter().enumerate() {
                        // The same layer in the last save, if it holds the same kind of art.
                        let before = old.and_then(|d| d.art_layers.get(index)).map(|l| &l.content).filter(|c| same_kind(c, &layer.content));
                        if before.is_none() && old.is_some() {
                            report.tiles_deleted += tx.execute("DELETE FROM tile WHERE drawing = ?1 AND layer = ?2", params![id.0 as i64, index as i64])?;
                            tx.execute("DELETE FROM vector WHERE drawing = ?1 AND layer = ?2", params![id.0 as i64, index as i64])?;
                        }
                        if let ArtContent::Vector(art) = &layer.content {
                            // Its shapes are written whole, as they are small.
                            if before != Some(&layer.content) {
                                upsert_vector.execute(params![id.0 as i64, index as i64, lz4_flex::compress_prepend_size(art.to_json().as_bytes())])?;
                                write_tips(art)?;
                                report.vectors_written += 1;
                            }
                            continue;
                        }
                        for (coord, tile) in layer.content.tiles().tiles() {
                            if before.and_then(|b| b.tiles().tile(*coord)).is_some_and(|b| Arc::ptr_eq(b, tile)) {
                                continue;
                            }
                            upsert.execute(params![id.0 as i64, index as i64, coord.tx, coord.ty, lz4_flex::compress_prepend_size(tile.bytes())])?;
                            report.tiles_written += 1;
                        }
                        for (coord, _) in before.into_iter().flat_map(|b| b.tiles().tiles()) {
                            if layer.content.tiles().tile(*coord).is_none() {
                                report.tiles_deleted += delete.execute(params![id.0 as i64, index as i64, coord.tx, coord.ty])?;
                            }
                        }
                    }
                    report.tiles_deleted += delete_layers(*id, drawing.art_layers.len())?;
                }
            }
            for id in self.saved.keys().filter(|id| !saved.contains_key(id)) {
                report.tiles_deleted += delete_layers(*id, 0)?;
            }
            if json != self.saved_json {
                tx.execute("INSERT OR REPLACE INTO document (name, json) VALUES ('project', ?1)", [&json])?;
                report.structure_written = true;
            }
            if self.version != FORMAT_VERSION {
                tx.pragma_update(None, "user_version", FORMAT_VERSION)?;
            }
        }
        tx.commit()?;
        // Only now is this what the file holds.
        self.version = FORMAT_VERSION;
        self.saved_json = json;
        self.saved = saved;
        self.last = Arc::downgrade(project);
        Ok(report)
    }

    fn keep_old_format(&self) -> Result<()> {
        let name = self.path.file_stem().map_or_else(|| "project".to_string(), |s| s.to_string_lossy().into_owned());
        let kept = self.path.with_file_name(format!("{name} (format {}).iskra", self.version));
        if kept.exists() { Ok(()) } else { backup::copy_to(&self.conn, &kept) }
    }
}

/// The format version of the file that `conn` has open, which is at `path`, once it is known to
/// be an Iskra file that this build can read.
pub(crate) fn checked_version(conn: &Connection, path: &Path) -> Result<i64> {
    let damaged = |e: rusqlite::Error| StoreError::Damaged { path: path.to_path_buf(), why: e.to_string(), backup: backup::backup_of(path) };
    let pragma = |name: &str| conn.pragma_query_value(None, name, |row| row.get::<_, i64>(0));
    let (application, version) = (pragma("application_id").map_err(damaged)?, pragma("user_version")?);
    if application != APPLICATION_ID {
        return Err(StoreError::NotAProject { path: path.to_path_buf(), why: "not an Iskra project file".to_string() });
    }
    if version > FORMAT_VERSION {
        let why = format!("made by a newer version of Iskra (file format {version}; this version reads up to {FORMAT_VERSION})");
        return Err(StoreError::NotAProject { path: path.to_path_buf(), why });
    }
    Ok(version)
}

/// The project's structure as the file has it, and as a project without pixels. The structure of
/// an older file is converted to the current shape.
pub(crate) fn read_structure(conn: &Connection, version: i64) -> std::result::Result<(String, Project), String> {
    let json: String = conn.query_row("SELECT json FROM document WHERE name = 'project'", [], |row| row.get(0)).map_err(|e| e.to_string())?;
    let document = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    let project = serde_json::from_value(migrate::to_current(version, document)).map_err(|e| e.to_string())?;
    Ok((json, project))
}

/// Read the pixels of `drawings` from their tiles, and their vector art: each drawing's layers,
/// filled. A tile or a layer of vector art that can't be read is skipped and reported, so one
/// bad tile doesn't cost the rest.
pub(crate) fn read_drawings(conn: &Connection, drawings: &[PoolDrawing], report: &mut LoadReport) -> Result<HashMap<DrawingId, Arc<Drawing>>> {
    read_tips(conn, report)?;
    let mut select = conn.prepare("SELECT layer, tx, ty, data FROM tile WHERE drawing = ?1")?;
    let mut select_vector = if has_vector_table(conn) { Some(conn.prepare("SELECT layer, data FROM vector WHERE drawing = ?1")?) } else { None };
    let mut read = HashMap::new();
    for entry in drawings {
        let mut drawing = entry.drawing.without_pixels();
        let rows = select
            .query_map([entry.id.0 as i64], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i32>(1)?, row.get::<_, i32>(2)?, row.get::<_, Vec<u8>>(3)?)))?;
        for row in rows {
            let (layer, tx, ty, data) = match row {
                Ok(row) => row,
                Err(e) => {
                    report.skipped.push(format!("drawing {} ({}): its tiles can't be read further: {e}", entry.name, entry.id.0));
                    break;
                }
            };
            let skip = |why: &str| format!("drawing {} ({}), layer {layer}, tile ({tx}, {ty}): {why}", entry.name, entry.id.0);
            let Some(content) = usize::try_from(layer).ok().and_then(|i| drawing.art_layers.get_mut(i)).map(|l| &mut l.content) else {
                report.skipped.push(skip("the drawing has no such layer"));
                continue;
            };
            let Some(tiles) = content.raw_tiles_mut() else {
                report.skipped.push(skip("the layer is vector art, which has no tiles"));
                continue;
            };
            // The size is checked before decompressing: a damaged size could ask for gigabytes.
            let size = data.first_chunk::<4>().map(|b| u32::from_le_bytes(*b) as usize);
            if size != Some(TILE_BYTES) {
                report.skipped.push(skip(&format!("{} bytes instead of {TILE_BYTES}", size.map_or("no".to_string(), |s| s.to_string()))));
                continue;
            }
            match lz4_flex::decompress_size_prepended(&data) {
                Ok(bytes) if bytes.len() == TILE_BYTES => tiles.tile_mut(TileCoord::new(tx, ty)).bytes_mut().copy_from_slice(&bytes),
                Ok(_) => report.skipped.push(skip("it is shorter than it says")),
                Err(e) => report.skipped.push(skip(&e.to_string())),
            }
        }
        if let Some(select) = &mut select_vector {
            read_vector(select, entry, &mut drawing, report)?;
        }
        read.insert(entry.id, Arc::new(drawing));
    }
    Ok(read)
}

/// Read the vector art of `entry`'s drawing into `drawing`'s layers.
fn read_vector(select: &mut rusqlite::Statement, entry: &PoolDrawing, drawing: &mut Drawing, report: &mut LoadReport) -> Result<()> {
    let rows = select.query_map([entry.id.0 as i64], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)))?;
    for row in rows {
        let (layer, data) = match row {
            Ok(row) => row,
            Err(e) => {
                report.skipped.push(format!("drawing {} ({}): its vector art can't be read further: {e}", entry.name, entry.id.0));
                break;
            }
        };
        let skip = |why: &str| format!("drawing {} ({}), layer {layer}, vector art: {why}", entry.name, entry.id.0);
        let Some(content) = usize::try_from(layer).ok().and_then(|i| drawing.art_layers.get_mut(i)).map(|l| &mut l.content) else {
            report.skipped.push(skip("the drawing has no such layer"));
            continue;
        };
        if !content.is_vector() {
            report.skipped.push(skip("the layer is not vector art"));
            continue;
        }
        let size = data.first_chunk::<4>().map(|b| u32::from_le_bytes(*b) as usize);
        if size.is_none_or(|size| size > MOST_VECTOR_BYTES) {
            report.skipped.push(skip("its size is damaged"));
            continue;
        }
        let art = lz4_flex::decompress_size_prepended(&data)
            .map_err(|e| e.to_string())
            .and_then(|bytes| String::from_utf8(bytes).map_err(|e| e.to_string()))
            .and_then(|json| iskra_model::VectorArt::from_json(&json).map_err(|e| e.to_string()));
        match art {
            Ok(art) => *content = ArtContent::Vector(art),
            Err(why) => report.skipped.push(skip(&why)),
        }
    }
    Ok(())
}

impl ProjectStore for SqliteStore {
    fn save(&mut self, project: &Arc<Project>) -> Result<SaveReport> {
        let report = self.write(project)?;
        // The save stands whether or not its copy could be made; the error says the copy wasn't.
        if !self.backed_up {
            backup::copy_to(&self.conn, &backup::backup_path(&self.path))?;
            self.backed_up = true;
        }
        Ok(report)
    }

    fn autosave(&mut self, project: &Arc<Project>) -> Result<SaveReport> {
        self.write(project)
    }

    fn load(&mut self, drawings: &[PoolDrawing]) -> Result<(HashMap<DrawingId, Arc<Drawing>>, LoadReport)> {
        let mut report = LoadReport::default();
        let read = read_drawings(&self.conn, drawings, &mut report)?;
        for (id, drawing) in &read {
            if let Some(saved) = self.saved.get_mut(id) {
                *saved = Some(drawing.clone());
            }
        }
        Ok((read, report))
    }

    fn peek(&mut self, drawings: &[PoolDrawing]) -> Result<HashMap<DrawingId, Arc<Drawing>>> {
        read_drawings(&self.conn, drawings, &mut LoadReport::default())
    }

    fn release(&mut self, drawings: &[DrawingId]) {
        for id in drawings {
            if let Some(saved) = self.saved.get_mut(id) {
                *saved = None;
            }
        }
    }

    fn save_as(&mut self, path: &Path, project: &Arc<Project>) -> Result<Box<dyn ProjectStore>> {
        let same = |a: &Path, b: &Path| a == b || a.canonicalize().is_ok_and(|a| b.canonicalize().is_ok_and(|b| a == b));
        if same(path, &self.path) {
            return Err(StoreError::NotAProject { path: path.to_path_buf(), why: "it is the file the project is already saved in".to_string() });
        }
        let mut copy = Self::new_file(path)?;
        // The shots that are not loaded have their pixels in this file only.
        let in_file: Vec<DrawingId> = project.shots.iter().filter(|s| !s.loaded).flat_map(|s| s.drawings()).map(|d| d.id).collect();
        if !in_file.is_empty() {
            copy.conn.execute("ATTACH DATABASE ?1 AS source", [self.path.to_string_lossy()])?;
            let tx = copy.conn.transaction()?;
            {
                let mut tiles = tx.prepare("INSERT INTO tile SELECT drawing, layer, tx, ty, data FROM source.tile WHERE drawing = ?1")?;
                // A file before version 47 has no vector art to copy.
                let has_vector = tx.query_row("SELECT 1 FROM source.sqlite_master WHERE type = 'table' AND name = 'vector'", [], |_| Ok(())).is_ok();
                let mut vector =
                    if has_vector { Some(tx.prepare("INSERT INTO vector SELECT drawing, layer, data FROM source.vector WHERE drawing = ?1")?) } else { None };
                // Their strokes' tips, which only this file keeps.
                if tx.query_row("SELECT 1 FROM source.sqlite_master WHERE type = 'table' AND name = 'tip'", [], |_| Ok(())).is_ok() {
                    tx.execute_batch(TIP_TABLE)?;
                    tx.execute("INSERT OR IGNORE INTO tip SELECT key, width, height, data FROM source.tip", [])?;
                }
                for id in &in_file {
                    tiles.execute([id.0 as i64])?;
                    if let Some(vector) = &mut vector {
                        vector.execute([id.0 as i64])?;
                    }
                    copy.saved.insert(*id, None);
                }
            }
            tx.commit()?;
            copy.conn.execute("DETACH DATABASE source", [])?;
        }
        copy.save(project)?;
        Ok(Box::new(copy))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use iskra_drawing::{BrushSettings, IndexedSurface, Rect, Stroke, StrokePoint, paint_stroke};
    use iskra_model::{ArtLayerKind, PoolDrawing, Shot, SwatchId};

    use super::*;

    /// The line art of the project's `index`th drawing.
    fn line_art(project: &mut Project, index: usize) -> &mut IndexedSurface {
        let layer = Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).layers[0]);
        let drawing = Arc::make_mut(&mut layer.drawings[index].drawing);
        match &mut drawing.layer_mut(ArtLayerKind::LineArt).unwrap().content {
            ArtContent::Indexed(surface) => surface,
            _ => panic!("indexed"),
        }
    }

    /// A project with `count` drawings, each with a stroke across four tiles.
    fn project(count: u64) -> Project {
        let mut project = Project::default();
        for n in 0..count {
            let id = DrawingId(project.new_id());
            let layer = Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).layers[0]);
            layer.drawings.push(PoolDrawing { id, name: (n + 1).to_string(), drawing: Arc::default() });
            layer.exposure.set(n as u32, 1, Some(id));
            let points = (0..=10).map(|i| StrokePoint::new(-200.0 + 40.0 * f64::from(i), -100.0 + 20.0 * f64::from(i), 1.0)).collect();
            paint_stroke(line_art(&mut project, n as usize), &Stroke { brush: BrushSettings::default(), points }, SwatchId(1));
        }
        project
    }

    fn file() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Film.iskra");
        (dir, path)
    }

    #[test]
    fn a_project_comes_back_as_it_was_saved() {
        let (_dir, path) = file();
        let saved = Arc::new(project(2));
        drop(SqliteStore::create(&path, &saved).unwrap());
        let (_, loaded, report) = SqliteStore::open_whole(&path).unwrap();
        assert_eq!(report, LoadReport::default());
        assert_eq!(loaded, saved, "structure and every pixel");
    }

    #[test]
    fn a_save_writes_only_what_changed() {
        let (_dir, path) = file();
        let first = Arc::new(project(2));
        let mut store = SqliteStore::create(&path, &first).unwrap();
        assert_eq!(store.save(&first).unwrap(), SaveReport::default(), "nothing changed");

        // One dab on one drawing: one tile, and no structure.
        let mut next = Project::clone(&first);
        line_art(&mut next, 1).paint(Rect::new(10, 10, 11, 11), &[1.0], SwatchId(2));
        let next = Arc::new(next);
        assert_eq!(store.save(&next).unwrap(), SaveReport { tiles_written: 1, tiles_deleted: 0, vectors_written: 0, structure_written: false });

        // Erasing a whole tile's paint deletes its row; renaming writes only the structure.
        let mut erased = Project::clone(&next);
        let surface = line_art(&mut erased, 0);
        let tile = Rect::new(0, 0, 256, 256);
        surface.erase(tile, &vec![1.0; 256 * 256]);
        surface.prune();
        Arc::make_mut(&mut Arc::make_mut(&mut erased.shots[0]).layers[0]).name = "Ball".to_string();
        let erased = Arc::new(erased);
        assert_eq!(store.save(&erased).unwrap(), SaveReport { tiles_written: 0, tiles_deleted: 1, vectors_written: 0, structure_written: true });

        drop(store);
        assert_eq!(SqliteStore::open_whole(&path).unwrap().1, erased);
    }

    /// The project's `index`th drawing with its line art made vector art of one stroke.
    fn with_vector_line(project: &mut Project, index: usize, width: f64) {
        let layer = Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).layers[0]);
        let drawing = Arc::make_mut(&mut layer.drawings[index].drawing);
        let mut path = iskra_model::kurbo::BezPath::new();
        path.move_to((-40.0, 0.0));
        path.curve_to((-10.0, -30.0), (10.0, 30.0), (40.0, 0.0));
        let stroke = iskra_model::VectorShape {
            id: iskra_model::ShapeId(1),
            swatch: SwatchId(1),
            kind: iskra_model::ShapeKind::Stroke(iskra_model::VectorStroke::even(path, width)),
        };
        drawing.layer_mut(ArtLayerKind::LineArt).unwrap().content = ArtContent::Vector(iskra_model::VectorArt::new(vec![stroke]));
    }

    #[test]
    fn a_stamped_strokes_tip_is_kept_once_in_the_file_and_read_back() {
        let (_dir, path) = file();
        let mut first = project(2);
        with_vector_line(&mut first, 0, 12.0);
        with_vector_line(&mut first, 1, 20.0);
        // A tip of its own, which no other test keeps: both strokes stamped with it.
        let tile = iskra_drawing::GrayTile::from_fn(37, 23, |x, y| ((x * 7 + y * 3) % 11) as f32 / 10.0);
        let key = iskra_drawing::remember_tip(&tile);
        for index in [0, 1] {
            let layer = Arc::make_mut(&mut Arc::make_mut(&mut first.shots[0]).layers[0]);
            let content = &mut Arc::make_mut(&mut layer.drawings[index].drawing).layer_mut(ArtLayerKind::LineArt).unwrap().content;
            let ArtContent::Vector(art) = content else { panic!("vector art") };
            *art = art.edited(|shapes| {
                let iskra_model::ShapeKind::Stroke(stroke) = &mut shapes[0].kind else { panic!("a stroke") };
                stroke.stamp =
                    Some(Box::new(iskra_drawing::Stamp { tip: key.clone(), spacing: 0.3, align: true, jitter: 20.0, drift: 0.0, build_up: false, seed: 4 }));
            });
        }
        let first = Arc::new(first);
        let store = SqliteStore::create(&path, &first).unwrap();
        let (width, height, data): (i64, i64, Vec<u8>) =
            store.conn.query_row("SELECT width, height, data FROM tip WHERE key = ?1", [&key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        assert_eq!((width, height), (37, 23));
        let samples: Vec<u16> = lz4_flex::decompress_size_prepended(&data).unwrap().as_chunks::<2>().0.iter().map(|p| u16::from_le_bytes(*p)).collect();
        assert_eq!(&samples[..], &tile.data[..]);
        assert_eq!(store.conn.query_row("SELECT count(*) FROM tip", [], |r| r.get::<_, i64>(0)).unwrap(), 1, "kept once for both strokes");
        drop(store);
        // Read back, the strokes are stamped as they were drawn.
        assert_eq!(SqliteStore::open_whole(&path).unwrap().1, first);
        // A tip the file alone keeps is put in the table of tips when the file is read.
        let conn = Connection::open(&path).unwrap();
        let only = lz4_flex::compress_prepend_size(&[0u8, 0, 255, 255]);
        conn.execute("INSERT INTO tip (key, width, height, data) VALUES ('only-in-the-file', 2, 1, ?1)", [only]).unwrap();
        conn.execute("INSERT INTO tip (key, width, height, data) VALUES ('damaged', 2, 1, x'00')", []).unwrap();
        drop(conn);
        let mut report = LoadReport::default();
        read_tips(&Connection::open(&path).unwrap(), &mut report).unwrap();
        assert_eq!(iskra_drawing::tip("only-in-the-file").unwrap().data[..], [0, 65535]);
        assert!(iskra_drawing::tip("damaged").is_none() && report.skipped.len() == 1, "{:?}", report.skipped);
    }

    #[test]
    fn vector_art_is_saved_as_its_shapes_and_comes_back() {
        let (_dir, path) = file();
        let mut first = project(2);
        with_vector_line(&mut first, 1, 4.0);
        let first = Arc::new(first);
        let mut store = SqliteStore::create(&path, &first).unwrap();
        let count = |store: &SqliteStore, table: &str| {
            store
                .conn
                .query_row(&format!("SELECT count(*) FROM {table} WHERE drawing = ?1"), [first.shots[0].layers[0].drawings[1].id.0 as i64], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap()
        };
        assert_eq!((count(&store, "vector"), count(&store, "tile")), (1, 0), "a row of shapes, and no tiles for the line art that was painted before");

        // A stroke made wider writes the layer's shapes again, and nothing else.
        let mut next = Project::clone(&first);
        with_vector_line(&mut next, 1, 9.0);
        let next = Arc::new(next);
        assert_eq!(store.save(&next).unwrap(), SaveReport { tiles_written: 0, tiles_deleted: 0, vectors_written: 1, structure_written: false });
        assert_eq!(store.save(&next).unwrap(), SaveReport::default());
        drop(store);

        let (_, loaded, report) = SqliteStore::open_whole(&path).unwrap();
        assert_eq!(report, LoadReport::default());
        assert_eq!(loaded, next, "the shapes, and the pixels they are drawn as");
        let art = loaded.shots[0].layers[0].drawings[1].drawing.layer(ArtLayerKind::LineArt).unwrap().content.indexed().unwrap().bounds();
        assert!(art.width() > 80 && art.height() > 9, "drawn when read: {art:?}");
    }

    #[test]
    fn a_layer_that_stops_being_vector_art_loses_its_shapes() {
        let (_dir, path) = file();
        let mut first = project(1);
        with_vector_line(&mut first, 0, 4.0);
        let first = Arc::new(first);
        let mut store = SqliteStore::create(&path, &first).unwrap();
        let mut next = Project::clone(&first);
        let layer = Arc::make_mut(&mut Arc::make_mut(&mut next.shots[0]).layers[0]);
        let line = &mut Arc::make_mut(&mut layer.drawings[0].drawing).layer_mut(ArtLayerKind::LineArt).unwrap().content;
        *line = line.to_indexed(|_| SwatchId(1));
        let next = Arc::new(next);
        store.save(&next).unwrap();
        assert_eq!(store.conn.query_row("SELECT count(*) FROM vector", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        drop(store);
        assert_eq!(SqliteStore::open_whole(&path).unwrap().1, next);
    }

    #[test]
    fn deleting_a_drawing_deletes_its_tiles() {
        let (_dir, path) = file();
        let first = Arc::new(project(2));
        let mut store = SqliteStore::create(&path, &first).unwrap();
        let count = |store: &SqliteStore| store.conn.query_row("SELECT count(*) FROM tile", [], |r| r.get::<_, i64>(0)).unwrap();
        let before = count(&store);
        let mut next = Project::clone(&first);
        let layer = Arc::make_mut(&mut Arc::make_mut(&mut next.shots[0]).layers[0]);
        let gone = layer.drawings.remove(0).id;
        layer.exposure.remove_drawing(gone);
        let report = store.save(&Arc::new(next)).unwrap();
        assert_eq!(report.tiles_deleted as i64, before / 2);
        assert_eq!(count(&store), before / 2);
    }

    #[test]
    fn a_save_after_loading_is_incremental_too() {
        let (_dir, path) = file();
        drop(SqliteStore::create(&path, &Arc::new(project(2))).unwrap());
        let (mut store, loaded, _) = SqliteStore::open_whole(&path).unwrap();
        let mut next = Project::clone(&loaded);
        line_art(&mut next, 0).paint(Rect::new(10, 10, 11, 11), &[1.0], SwatchId(2));
        assert_eq!(store.save(&Arc::new(next)).unwrap().tiles_written, 1);
    }

    #[test]
    fn a_tile_that_cannot_be_read_is_skipped_and_reported() {
        let (_dir, path) = file();
        let saved = Arc::new(project(1));
        let store = SqliteStore::create(&path, &saved).unwrap();
        store.conn.execute("UPDATE tile SET data = x'00010203' WHERE tx = 0 AND ty = 0", []).unwrap();
        drop(store);
        let (_, loaded, report) = SqliteStore::open_whole(&path).unwrap();
        let [line] = &report.skipped[..] else { panic!("one tile skipped: {report:?}") };
        assert!(line.contains("drawing 1") && line.contains("tile (0, 0)"), "{line}");
        let mut loaded = Project::clone(&loaded);
        let mut saved = Project::clone(&saved);
        assert_eq!(line_art(&mut loaded, 0).pixel(-100, -50), line_art(&mut saved, 0).pixel(-100, -50), "the other tiles are loaded");
        assert!(line_art(&mut loaded, 0).tiles().tile(TileCoord::new(0, 0)).is_none());
    }

    #[test]
    fn files_that_are_not_sound_projects_are_refused() {
        let (dir, path) = file();
        assert!(matches!(SqliteStore::open(&path), Err(StoreError::Io { .. })), "a missing file");
        assert!(!path.exists(), "opening never creates a file");

        let other = dir.path().join("other.db");
        Connection::open(&other).unwrap().execute_batch("CREATE TABLE t (x)").unwrap();
        assert!(matches!(SqliteStore::open(&other), Err(StoreError::NotAProject { .. })));

        let store = SqliteStore::create(&path, &Arc::new(project(1))).unwrap();
        store.conn.pragma_update(None, "user_version", FORMAT_VERSION + 1).unwrap();
        drop(store);
        assert!(matches!(SqliteStore::open(&path), Err(StoreError::NotAProject { why, .. }) if why.contains("newer version")));

        std::fs::write(&path, b"this is not a database at all, just some text that is long enough").unwrap();
        assert!(matches!(SqliteStore::open(&path), Err(StoreError::Damaged { .. })));
    }

    #[test]
    fn salvage_reads_what_open_refuses() {
        let (_dir, path) = file();
        let saved = Arc::new(project(2));
        let store = SqliteStore::create(&path, &saved).unwrap();
        let (whole, report) = SqliteStore::salvage(&path).unwrap();
        assert_eq!((&whole, report.skipped.len()), (&*saved, 0), "a sound file salvages whole");

        // The structure is lost: the drawings still come out, by id.
        store.conn.execute("UPDATE document SET json = '{ not json'", []).unwrap();
        store.conn.execute("UPDATE tile SET data = x'ffffffff00' WHERE drawing = (SELECT min(drawing) FROM tile) AND tx = 0 AND ty = 0", []).unwrap();
        drop(store);
        assert!(matches!(SqliteStore::open(&path), Err(StoreError::Damaged { .. })));
        let (mut salvaged, report) = SqliteStore::salvage(&path).unwrap();
        assert_eq!(report.skipped.len(), 2, "{report:?}");
        assert!(report.skipped[0].contains("structure") && report.skipped[1].contains("tile (0, 0)"));
        let mut saved = Project::clone(&saved);
        let ids = |p: &Project| p.shots[0].layers[0].drawings.iter().map(|d| d.id).collect::<Vec<_>>();
        assert_eq!(ids(&salvaged), ids(&saved));
        assert_eq!(salvaged.shots[0].layers[0].drawings[1].name, ids(&saved)[1].0.to_string());
        // The stand-in drawing's top layer is the saved line art.
        let layer = Arc::make_mut(&mut Arc::make_mut(&mut salvaged.shots[0]).layers[0]);
        let ArtContent::Indexed(top) = &Arc::make_mut(&mut layer.drawings[1].drawing).art_layers[1].content else { panic!("indexed") };
        assert_eq!(top, line_art(&mut saved, 1));
    }

    /// A file as version 1 of the format wrote it: two shots in a list, one painted drawing.
    fn version_1_file(path: &Path) -> Arc<Project> {
        let current = Arc::new(project(1));
        let store = SqliteStore::create(path, &current).unwrap();
        // The drawing keeps the id its tiles are stored under.
        let drawing = &current.shots[0].layers[0].drawings[0];
        let old = serde_json::json!({
            "settings": current.settings,
            // Until version 23 the palette was the list of its swatches.
            "palette": current.palette.swatches,
            "shots": [
                { "id": 1, "name": "Shot 1", "duration": 48, "elements": [{
                    "id": 2, "name": "Drawing", "density": 2.0,
                    "drawings": [{ "id": drawing.id, "name": "1", "drawing": drawing.drawing }],
                    "exposure": [{ "start": 0, "len": 1, "drawing": drawing.id }],
                }] },
                { "id": 7, "name": "Shot 2", "duration": 12, "elements": [] },
            ],
            "nextId": 8,
        });
        store.conn.execute("UPDATE document SET json = ?1", [old.to_string()]).unwrap();
        store.conn.pragma_update(None, "user_version", 1).unwrap();
        current
    }

    fn layer_id(project: &Project) -> iskra_model::LayerId {
        project.shots[0].layers[0].id
    }

    #[test]
    fn a_version_1_file_opens_with_its_shots_on_one_storyboard() {
        let (dir, path) = file();
        let current = version_1_file(&path);
        let version = |path: &Path| Connection::open(path).unwrap().pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap();

        let (mut store, opened, report) = SqliteStore::open_whole(&path).unwrap();
        assert_eq!(report, LoadReport::default());
        let [storyboard] = &opened.storyboards[..] else { panic!("one storyboard") };
        let clips = storyboard.clips().iter().map(|c| (c.shot.0, c.start, c.in_point, c.out_point)).collect::<Vec<_>>();
        assert_eq!(clips, [(1, 0, 0, 48), (7, 48, 0, 12)], "the shots end to end, whole");
        assert_eq!((storyboard.id.0, opened.shots[1].name.as_str()), (8, "Shot 2"), "names are kept; the storyboard takes the next id");
        // Each shot gets a composite that stacks its layers as they were ordered, on display.
        let [composite] = &opened.shots[0].composites[..] else { panic!("one composite") };
        assert_eq!((composite.id.0, opened.shots[0].stack(), opened.shots[1].composites[0].id.0), (9, vec![layer_id(&opened)], 10));
        assert_eq!(opened.shots[1].display, Some(iskra_model::Source::Composite(opened.shots[1].composites[0].id)));
        // And each a camera at home, which sees the shot as it was.
        let cameras: Vec<_> = opened.shots.iter().map(|shot| (shot.camera.id.0, shot.camera.parent, shot.camera.zoom)).collect();
        assert_eq!(cameras, [(11, None, 1.0), (12, None, 1.0)]);
        assert_eq!(opened.shots[0].camera, iskra_model::Camera::new(opened.shots[0].camera.id, current.settings.height));
        let layer = &opened.shots[0].layers[0];
        assert_eq!(layer.role, iskra_model::Role::Animation);
        assert_eq!(layer.drawings[0].drawing, current.shots[0].layers[0].drawings[0].drawing, "with every pixel");
        assert!(Project::clone(&opened).new_id() > 8, "and ids stay unique");

        // Opening converts in memory only. The first save rewrites the file, and keeps the old.
        assert_eq!(version(&path), 1);
        let mut next = Project::clone(&opened);
        line_art(&mut next, 0).paint(Rect::new(10, 10, 11, 11), &[1.0], SwatchId(2));
        let next = Arc::new(next);
        assert_eq!(store.save(&next).unwrap(), SaveReport { tiles_written: 1, tiles_deleted: 0, vectors_written: 0, structure_written: true });
        drop(store);
        assert_eq!(version(&path), FORMAT_VERSION);
        assert_eq!(SqliteStore::open_whole(&path).unwrap().1, next);
        assert_eq!(version(&dir.path().join("Film (format 1).iskra")), 1, "the version 1 file is kept beside it");
        assert_eq!(version(&backup::backup_path(&path)), FORMAT_VERSION, "and the backup is of the save");
        drop(dir);
    }

    #[test]
    fn a_version_2_file_opens_with_no_pegs_and_pegs_are_saved() {
        let (dir, path) = file();
        let current = Arc::new(project(1));
        let store = SqliteStore::create(&path, &current).unwrap();
        // As version 2 wrote it: shots had no pegs and layers no parent.
        let mut old = serde_json::to_value(&*current).unwrap();
        old["shots"][0].as_object_mut().unwrap().remove("pegs");
        old["shots"][0]["layers"][0].as_object_mut().unwrap().remove("parent");
        let elements = old["shots"][0].as_object_mut().unwrap().remove("layers").unwrap();
        old["shots"][0]["elements"] = elements;
        store.conn.execute("UPDATE document SET json = ?1", [old.to_string()]).unwrap();
        store.conn.pragma_update(None, "user_version", 2).unwrap();
        drop(store);

        let (mut store, opened, _) = SqliteStore::open_whole(&path).unwrap();
        assert_eq!(opened, current, "it is the same project, with no pegs");
        let mut next = Project::clone(&opened);
        let peg = iskra_model::PegId(next.new_id());
        let transform = iskra_model::Transform { position: [12.5, -3.0], rotation: 30.0, ..Default::default() };
        let shot = Arc::make_mut(&mut next.shots[0]);
        let mut animation = iskra_model::Animation::default();
        animation.position.set(0, [12.5, -3.0]);
        animation.position.set(24, [300.0, -3.0]);
        let curve = iskra_model::Interp::Curve(iskra_model::Slope { reach: 0.6, speed: 0.25 });
        animation.set_interp(24, None, Some(iskra_model::Interp::Ease), None);
        animation.set_interp(0, None, None, Some(curve));
        animation.position.key_mut(24).unwrap().bend = Some(iskra_model::Bend { into: [-40.0, 20.0], out: [0.0, 0.0] });
        shot.pegs.push(Arc::new(iskra_model::Peg {
            id: peg,
            name: "Bounce".to_string(),
            parent: None,
            transform,
            animation,
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        }));
        Arc::make_mut(&mut shot.layers[0]).parent = Some(peg);
        let next = Arc::new(next);
        assert_eq!(store.save(&next).unwrap(), SaveReport { tiles_written: 0, tiles_deleted: 0, vectors_written: 0, structure_written: true });
        drop(store);
        assert_eq!(SqliteStore::open_whole(&path).unwrap().1, next, "the peg, its transform, its keys and what it carries");
        // A version 3 peg, which had no keyframes, reads as one with none.
        let mut old = serde_json::to_value(&*next).unwrap();
        old["shots"][0]["pegs"][0].as_object_mut().unwrap().remove("animation");
        let read: Project = serde_json::from_value(migrate::to_current(3, old)).unwrap();
        assert!(read.shots[0].pegs[0].animation == iskra_model::Animation::default() && read.shots[0].pegs[0].transform == transform);
        assert!(dir.path().join("Film (format 2).iskra").exists(), "the version 2 file is kept beside it");
    }

    #[test]
    fn salvage_reads_version_1_files_too() {
        let (_dir, path) = file();
        version_1_file(&path);
        let (salvaged, report) = SqliteStore::salvage(&path).unwrap();
        assert_eq!(report.skipped, Vec::<String>::new());
        assert_eq!((salvaged.shots.len(), salvaged.storyboards.len()), (2, 1));
    }

    #[test]
    fn creating_over_a_file_replaces_it() {
        let (_dir, path) = file();
        drop(SqliteStore::create(&path, &Arc::new(project(2))).unwrap());
        let empty = Arc::new(Project::default());
        drop(SqliteStore::create(&path, &empty).unwrap());
        assert_eq!(SqliteStore::open_whole(&path).unwrap().1, empty);
    }

    /// A film of two shots, each with one painted drawing, saved at `path` and opened again.
    fn film(path: &Path) -> (SqliteStore, Arc<Project>, Arc<Project>) {
        let mut whole = project(1);
        let mut second = whole.duplicate_shot(whole.shots[0].id, "Shot_02").unwrap();
        // Its drawing differs from the first shot's, to tell them apart.
        let drawing = Arc::make_mut(&mut Arc::make_mut(&mut second.layers[0]).drawings[0].drawing);
        let ArtContent::Indexed(surface) = &mut drawing.layer_mut(ArtLayerKind::LineArt).unwrap().content else { panic!("indexed") };
        surface.paint(Rect::new(600, 600, 601, 601), &[1.0], SwatchId(3));
        whole.shots.push(Arc::new(second));
        let whole = Arc::new(whole);
        drop(SqliteStore::create(path, &whole).unwrap());
        let (store, opened) = SqliteStore::open(path).unwrap();
        (store, opened, whole)
    }

    /// `project` with the shot at `index` loaded from `store`.
    fn with_shot_loaded(store: &mut SqliteStore, project: &Project, index: usize) -> Project {
        let drawings: Vec<PoolDrawing> = project.shots[index].drawings().cloned().collect();
        let (pixels, report) = store.load(&drawings).unwrap();
        assert_eq!(report, LoadReport::default());
        let mut project = project.clone();
        project.shots[index] = Arc::new(project.shots[index].with_pixels(&pixels));
        project
    }

    fn tile_count(path: &Path) -> i64 {
        Connection::open(path).unwrap().query_row("SELECT count(*) FROM tile", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn opening_reads_no_pixels_and_a_shot_is_loaded_on_demand() {
        let (_dir, path) = file();
        let (mut store, opened, whole) = film(&path);
        assert!(opened.shots.iter().all(|s| !s.loaded && s.drawings().all(|d| d.drawing.bounds().is_empty())), "structure only");
        assert_eq!(opened.shots[1].layers[0].exposure, whole.shots[1].layers[0].exposure, "with the timing");

        let loaded = with_shot_loaded(&mut store, &opened, 1);
        assert_eq!(loaded.shots[1], whole.shots[1], "the second shot, with every pixel");
        assert!(!loaded.shots[0].loaded, "and only that shot");
    }

    #[test]
    fn a_save_leaves_the_shots_that_are_not_loaded_alone() {
        let (_dir, path) = file();
        let (mut store, opened, whole) = film(&path);
        let tiles = tile_count(&path);

        // Painting on the loaded shot writes one tile; renaming the other writes structure only.
        let mut next = with_shot_loaded(&mut store, &opened, 1);
        let drawing = Arc::make_mut(&mut Arc::make_mut(&mut Arc::make_mut(&mut next.shots[1]).layers[0]).drawings[0].drawing);
        let ArtContent::Indexed(surface) = &mut drawing.layer_mut(ArtLayerKind::LineArt).unwrap().content else { panic!("indexed") };
        surface.paint(Rect::new(10, 10, 11, 11), &[1.0], SwatchId(2));
        Arc::make_mut(&mut next.shots[0]).name = "Opening".to_string();
        let next = Arc::new(next);
        assert_eq!(store.save(&next).unwrap(), SaveReport { tiles_written: 1, tiles_deleted: 0, vectors_written: 0, structure_written: true });
        assert_eq!(tile_count(&path), tiles);

        // Released and saved again: nothing is written, and the pixels stay in the file.
        let mut released = Project::clone(&next);
        released.shots[1] = Arc::new(released.shots[1].without_pixels());
        store.release(&next.shots[1].drawings().map(|d| d.id).collect::<Vec<_>>());
        assert_eq!(store.save(&Arc::new(released)).unwrap(), SaveReport::default());
        drop(store);
        let (_, reopened, _) = SqliteStore::open_whole(&path).unwrap();
        assert_eq!(reopened.shots[1], next.shots[1]);
        assert_eq!(reopened.shots[0].layers, whole.shots[0].layers, "the shot that was never loaded kept every pixel");
    }

    #[test]
    fn a_drawing_saved_without_its_last_saved_pixels_in_memory_is_written_whole() {
        let (_dir, path) = file();
        let (mut store, opened, whole) = film(&path);
        // A version of the second shot from before it was released, say after an undo: the
        // store no longer has what the file holds to compare it with.
        let mut next = Project::clone(&opened);
        let mut shot = Shot::clone(&whole.shots[1]);
        let drawing = Arc::make_mut(&mut Arc::make_mut(&mut shot.layers[0]).drawings[0].drawing);
        let ArtContent::Indexed(surface) = &mut drawing.layer_mut(ArtLayerKind::LineArt).unwrap().content else { panic!("indexed") };
        surface.erase(Rect::new(0, 0, 256, 256), &vec![1.0; 256 * 256]);
        surface.prune();
        next.shots[1] = Arc::new(shot);
        let next = Arc::new(next);
        let report = store.save(&next).unwrap();
        assert!(report.tiles_written >= 3 && report.tiles_deleted > report.tiles_written, "{report:?}");
        drop(store);
        let (_, reopened, _) = SqliteStore::open_whole(&path).unwrap();
        assert_eq!(reopened.shots[1], next.shots[1], "the erased tile is gone from the file too");
    }

    #[test]
    fn deleting_a_shot_that_is_not_loaded_deletes_its_tiles() {
        let (_dir, path) = file();
        let (mut store, opened, _) = film(&path);
        let tiles = tile_count(&path);
        let mut next = Project::clone(&opened);
        next.shots.remove(0);
        assert!(store.save(&Arc::new(next)).unwrap().tiles_deleted > 0);
        assert!(tile_count(&path) < tiles && tile_count(&path) > 0);
    }

    #[test]
    fn save_as_takes_the_shots_that_are_not_loaded_from_the_old_file() {
        let (dir, path) = file();
        let (mut store, opened, whole) = film(&path);
        // A tip kept by the old file, for a shot that is not loaded: the copy keeps it too.
        store.conn.execute_batch(TIP_TABLE).unwrap();
        let one = lz4_flex::compress_prepend_size(&[255u8, 255]);
        store.conn.execute("INSERT INTO tip (key, width, height, data) VALUES ('of-the-old-file', 1, 1, ?1)", [one]).unwrap();
        let mut next = with_shot_loaded(&mut store, &opened, 1);
        Arc::make_mut(&mut next.shots[1]).name = "Closing".to_string();
        let next = Arc::new(next);
        let copy_path = dir.path().join("Copy.iskra");
        let mut copy = store.save_as(&copy_path, &next).unwrap();
        assert_eq!(copy.path(), copy_path);
        assert_eq!(copy.save(&next).unwrap(), SaveReport::default(), "the copy holds this snapshot");
        assert!(matches!(store.save_as(&path, &next), Err(StoreError::NotAProject { .. })), "never onto the file it reads from");
        drop((store, copy));

        let tips = Connection::open(&copy_path).unwrap().query_row("SELECT count(*) FROM tip WHERE key = 'of-the-old-file'", [], |r| r.get::<_, i64>(0));
        assert_eq!(tips.unwrap(), 1);
        let (_, reopened, report) = SqliteStore::open_whole(&copy_path).unwrap();
        assert_eq!(report, LoadReport::default());
        assert_eq!(reopened.shots[0].layers, whole.shots[0].layers, "the shot that was only in the old file");
        assert_eq!(reopened.shots[1], next.shots[1]);
        assert_eq!(SqliteStore::open_whole(&path).unwrap().1.shots[1].name, "Shot_02", "the old file is as it was");
    }
}
