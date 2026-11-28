//! The drawings of a rig's scene as pictures (plan §4.17). The scene's drawings are in a format
//! of their own, which Iskra doesn't read: the user's own drawing converter, a program that
//! comes with the scene's, turns each into PNG files, one an art layer, which Iskra then reads.
//!
//! For each drawing the converter is asked for the drawing's box, and then, for each art layer
//! wanted, makes a copy of the drawing with its other art layers cleared and turns that copy
//! into a picture. The picture covers the scene's field chart, 4 by 3, at as many pixels a
//! unit of the scene's grid as the shot's drawings have, made larger where the drawing goes
//! past the chart's edge, with the drawing's origin at its middle. What the converter is told
//! is built here, and every command line is kept for the log, as only the user's own
//! converter can show whether it is right.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::rig_import::{Element, RigScene};

/// An art layer of a drawing, which is made into a picture of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pass {
    Line,
    Colour,
    Under,
    Over,
}

impl Pass {
    /// The name the converter gives the art layer.
    fn layer(self) -> &'static str {
        match self {
            Pass::Line => "lineart",
            Pass::Colour => "colorart",
            Pass::Under => "underlayart",
            Pass::Over => "overlayart",
        }
    }

    /// The end of the name of the picture of this art layer.
    pub fn suffix(self) -> &'static str {
        match self {
            Pass::Line => "line",
            Pass::Colour => "colour",
            Pass::Under => "underlay",
            Pass::Over => "overlay",
        }
    }

    const ALL: [Pass; 4] = [Pass::Line, Pass::Colour, Pass::Under, Pass::Over];
}

/// A program that converts the scene's drawings, run with arguments: what it printed, or why it
/// failed. Behind a trait, so that tests can stand in for the user's converter.
pub trait DrawingConverter: Sync {
    fn run(&self, args: &[String]) -> Result<String, String>;
}

/// The user's converter, at its path.
///
/// Its licence may let fewer copies run at once than there are threads, and a copy turned down
/// stops at once and says so ("Licensed number of users already reached"); a seat is not freed
/// the moment a copy ends either. Once a copy is turned down so, the copies are run one at a
/// time, and one turned down is run again after a wait, for up to [`LICENCE_WAIT`].
pub struct Program {
    path: PathBuf,
    one_at_a_time: AtomicBool,
    turn: Mutex<()>,
}

/// How long a command turned down by the converter's licence is tried again for.
const LICENCE_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

impl Program {
    pub fn new(path: PathBuf) -> Self {
        Self { path, one_at_a_time: AtomicBool::new(false), turn: Mutex::new(()) }
    }
}

/// Whether the converter stopped because its licence let no more copies run.
fn licence_refused(why: &str) -> bool {
    let why = why.to_lowercase();
    why.contains("licensing error") || why.contains("number of users")
}

impl DrawingConverter for Program {
    fn run(&self, args: &[String]) -> Result<String, String> {
        if !self.one_at_a_time.load(Ordering::Relaxed) {
            match self.once(args) {
                Err(why) if licence_refused(&why) => self.one_at_a_time.store(true, Ordering::Relaxed),
                said => return said,
            }
        }
        let _turn = self.turn.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let until = std::time::Instant::now() + LICENCE_WAIT;
        let mut wait = std::time::Duration::from_millis(100);
        loop {
            match self.once(args) {
                Err(why) if licence_refused(&why) && std::time::Instant::now() < until => {
                    std::thread::sleep(wait);
                    wait = (wait * 2).min(std::time::Duration::from_secs(2));
                }
                said => return said,
            }
        }
    }
}

impl Program {
    /// Run the converter once.
    fn once(&self, args: &[String]) -> Result<String, String> {
        // Started in its own folder, where it may look for what it needs.
        let mut command = std::process::Command::new(&self.path);
        if let Some(folder) = self.path.parent().filter(|folder| folder.is_dir()) {
            command.current_dir(folder);
        }
        // On Windows, without a console window of its own opened for every command.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let output = command.args(args).output().map_err(|why| format!("the converter could not be started: {why}"))?;
        let said = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() {
            Ok(said)
        } else {
            // All it said, on stderr and stdout, a line each, as the reason may be anywhere in it.
            let error = String::from_utf8_lossy(&output.stderr);
            let lines: Vec<&str> = error.lines().chain(said.lines()).map(str::trim).filter(|line| !line.is_empty()).collect();
            let mut all = lines.join(" / ");
            if all.len() > 2000 {
                all.truncate(all.char_indices().nth(2000).map_or(all.len(), |(at, _)| at));
                all.push('…');
            }
            Err(format!("the converter stopped with {}: {all}", output.status))
        }
    }
}

/// The picture of one art layer of a drawing, in the folder the pictures are made in.
pub fn picture_path(out: &Path, element: &Element, drawing: &str, pass: Pass) -> PathBuf {
    out.join(&element.folder).join(format!("{}-{drawing}.{}.png", element.name, pass.suffix()))
}

/// The art layers to make pictures of, by element: the line art, the colour art and the overlay
/// art of every drawing, as a drawing node shows them, and the underlay art of the drawings of
/// a drawing node that a node takes it out of.
pub fn passes(scene: &RigScene) -> HashMap<u32, Vec<Pass>> {
    let wired = scene.wired();
    let mut passes: HashMap<u32, Vec<Pass>> = scene.elements.iter().map(|element| (element.id, vec![Pass::Line, Pass::Colour, Pass::Over])).collect();
    for node in &wired {
        let pass = match node.node.kind.as_str() {
            "UNDERLAY" => Pass::Under,
            "OVERLAY" => Pass::Over,
            _ => continue,
        };
        let Some(&(_, from)) = node.inputs.first() else { continue };
        if let Some((element, _)) = scene.drawing(wired[from].node)
            && let Some(wanted) = passes.get_mut(&element.id)
            && !wanted.contains(&pass)
        {
            wanted.push(pass);
        }
    }
    passes
}

/// The units the converter places a drawing in: this many across and up are one field of the
/// field chart, which is 12 fields to each side of the middle (the converter's own figures).
const MODEL_PER_FIELD: [f64; 2] = [2500.0 / 12.0, 1875.0 / 12.0];

/// The box a drawing's art is in, in the converter's units, from what it printed: the last
/// four numbers, its left, bottom, right and top.
fn printed_box(said: &str) -> Option<[f64; 4]> {
    let numbers: Vec<f64> = said.split(|c: char| c.is_whitespace() || c == ',').filter_map(|word| word.parse().ok()).collect();
    let [.., x1, y1, x2, y2] = numbers[..] else { return None };
    Some([x1, y1, x2, y2])
}

/// How many times larger than the field chart a picture must be to hold a drawing whose art is
/// in `art`: 1 for one within it, with a little room at the edge for one past it.
fn enlarged(art: Option<[f64; 4]>) -> f64 {
    let Some([x1, y1, x2, y2]) = art else { return 1.0 };
    let across = x1.abs().max(x2.abs()) / MODEL_PER_FIELD[0] / 12.0;
    let up = y1.abs().max(y2.abs()) / MODEL_PER_FIELD[1] / 12.0;
    let most = across.max(up);
    if most.is_finite() && most > 1.0 { most * 1.02 } else { 1.0 }
}

/// The converter's arguments for a picture of `tvg` covering the field chart `times` over, at
/// `pixels` a field across: 4 by 3, the art scaled down by as much as the chart is made larger.
/// The converter draws a drawing's underlay and overlay art only when asked to.
fn picture_args(tvg: &Path, png: &Path, pass: Pass, pixels: f64, times: f64) -> Vec<String> {
    let width = (24.0 * pixels * times).round().max(1.0) as u32;
    let height = (18.0 * pixels * times).round().max(1.0) as u32;
    let mut args = vec!["-outformat".into(), "PNG4".into(), "-outfile".into(), png.display().to_string()];
    match pass {
        Pass::Under => args.push("-useunderlayart".into()),
        Pass::Over => args.push("-useoverlayart".into()),
        Pass::Line | Pass::Colour => {}
    }
    args.extend(["-resolution".into(), width.to_string(), height.to_string(), "-bgalpha".into(), "0".into()]);
    if times > 1.0 {
        args.extend(["-scale".into(), format!("{:.6}", 1.0 / times)]);
    }
    args.push(tvg.display().to_string());
    args
}

/// The converter's arguments for a copy of `tvg` with every art layer but `pass`'s cleared.
fn copy_args(tvg: &Path, copy: &Path, pass: Pass) -> Vec<String> {
    let cleared: Vec<&str> = Pass::ALL.iter().filter(|other| **other != pass).map(|other| other.layer()).collect();
    vec!["-outformat".into(), "TVG".into(), "-outfile".into(), copy.display().to_string(), "-clearlayers".into(), cleared.join(","), tvg.display().to_string()]
}

/// One drawing to convert.
struct Job<'a> {
    element: &'a Element,
    drawing: &'a str,
    passes: &'a [Pass],
}

/// How the pictures were made: for the log, the first drawing's commands and what went wrong.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Converted {
    pub drawings: usize,
    pub pictures: usize,
    pub log: Vec<String>,
    /// Stopped before the end, as asked.
    pub cancelled: bool,
    /// Stopped because the first drawing gave no picture: the converter can't be used so, and
    /// what it said of the first drawing is in the log.
    pub failed: bool,
}

/// Convert one drawing: its box, then a picture of each art layer wanted.
/// With `debug`, the converter is asked to say what it does and what goes wrong.
fn convert(job: &Job, scene: &RigScene, converter: &dyn DrawingConverter, out: &Path, pixels: f64, debug: bool, log: &mut Vec<String>) -> usize {
    let tvg = job.element.drawing_file(scene, job.drawing);
    if !tvg.is_file() {
        log.push(format!("{}: no such drawing file", tvg.display()));
        return 0;
    }
    let run = |mut args: Vec<String>, log: &mut Vec<String>| {
        if debug {
            args.insert(0, "-debug".into());
        }
        let said = converter.run(&args);
        // The first drawing's commands, to show what the converter is told; of the others,
        // those that fail.
        if debug || said.is_err() {
            log.push(format!("converter {}{}", args.join(" "), said.as_ref().err().map_or(String::new(), |why| format!(": {why}"))));
        }
        said
    };
    let art = run(vec!["-printbox".into(), tvg.display().to_string()], log).ok().and_then(|said| printed_box(&said));
    if art.is_none() {
        log.push(format!("{}: its box was not said, so it is made the size of the field chart", tvg.display()));
    }
    let times = enlarged(art);
    // A drawing of an element of another field chart is that much larger or smaller.
    let pixels = pixels * job.element.field_chart / 12.0;
    let folder = out.join(&job.element.folder);
    if let Err(why) = std::fs::create_dir_all(&folder) {
        log.push(format!("{}: {why}", folder.display()));
        return 0;
    }
    let mut made = 0;
    for pass in job.passes {
        let png = picture_path(out, job.element, job.drawing, *pass);
        let copy = png.with_extension("tvg");
        if run(copy_args(&tvg, &copy, *pass), log).is_err() {
            continue;
        }
        let pictured = run(picture_args(&copy, &png, *pass, pixels, times), log);
        _ = std::fs::remove_file(&copy);
        if pictured.is_ok() && png.is_file() {
            made += 1;
        } else if pictured.is_ok() {
            log.push(format!("{}: the converter said nothing went wrong, and made no picture", png.display()));
        }
    }
    made
}

/// Make the pictures of every drawing of the scene in `out`, at `pixels` drawing pixels a unit
/// of the scene's grid across, on `threads` threads at once. `done` counts the drawings as
/// they are converted, of [`drawing_count`]; `cancel` stops it after the drawings under way.
pub fn convert_drawings(
    scene: &RigScene,
    converter: &dyn DrawingConverter,
    out: &Path,
    pixels: f64,
    threads: usize,
    done: &AtomicUsize,
    cancel: &AtomicBool,
) -> Converted {
    let passes = passes(scene);
    let mut jobs: Vec<Job> = scene
        .elements
        .iter()
        .flat_map(|element| {
            let wanted = passes.get(&element.id).map_or(&[][..], Vec::as_slice);
            element.drawings.iter().map(move |drawing| Job { element, drawing, passes: wanted })
        })
        .collect();
    // A scene may name a drawing whose file is gone, which its own program passes over: the
    // converter is tried first on a drawing that is there, so that one missing file doesn't
    // stop the rest.
    if let Some(there) = jobs.iter().position(|job| job.element.drawing_file(scene, job.drawing).is_file()) {
        jobs.swap(0, there);
    }
    // The first drawing alone, the converter saying what goes wrong: if it gives no picture,
    // the rest would fail as it did, and are not tried.
    let mut first = Vec::new();
    let made_first = jobs.first().filter(|_| !cancel.load(Ordering::Relaxed)).map(|job| convert(job, scene, converter, out, pixels, true, &mut first));
    if made_first == Some(0) {
        first.push("the converter made no picture of the first drawing, so the others were not tried".to_string());
        return Converted { drawings: jobs.len(), pictures: 0, log: first, cancelled: false, failed: true };
    }
    done.fetch_add(usize::from(made_first.is_some()), Ordering::Relaxed);
    let next = AtomicUsize::new(1);
    let made = AtomicUsize::new(made_first.unwrap_or(0));
    let log = Mutex::new(first);
    std::thread::scope(|scope| {
        for _ in 0..threads.clamp(1, 16) {
            scope.spawn(|| {
                while !cancel.load(Ordering::Relaxed) {
                    let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) else { break };
                    let mut said = Vec::new();
                    made.fetch_add(convert(job, scene, converter, out, pixels, false, &mut said), Ordering::Relaxed);
                    log.lock().expect("the log").extend(said);
                    done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    Converted {
        drawings: jobs.len(),
        pictures: made.into_inner(),
        log: log.into_inner().expect("the log"),
        cancelled: cancel.load(Ordering::Relaxed) && done.load(Ordering::Relaxed) < jobs.len(),
        failed: false,
    }
}

/// How many drawings [`convert_drawings`] converts.
pub fn drawing_count(scene: &RigScene) -> usize {
    scene.elements.iter().map(|element| element.drawings.len()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A converter that does what the tests expect the user's to: says a box, copies a drawing,
    /// and makes a picture of the size it is asked, with a pixel in its middle of the art layer
    /// that was kept. It keeps every command it was given.
    struct Stand {
        said: Mutex<Vec<Vec<String>>>,
    }

    impl DrawingConverter for Stand {
        fn run(&self, args: &[String]) -> Result<String, String> {
            self.said.lock().unwrap().push(args.to_vec());
            let after = |flag: &str, n: usize| args.iter().position(|arg| arg == flag).map(|at| args[at + 1..at + 1 + n].to_vec());
            if args.contains(&"-printbox".to_string()) {
                // The arm reaches 18 fields across, half again past the chart.
                return Ok(if args.last().unwrap().contains("ARM-1") {
                    "-3750 -100 200 500\n".to_string()
                } else {
                    "Bounding box: -100 -100 100 100".to_string()
                });
            }
            let out = PathBuf::from(&after("-outfile", 1).unwrap()[0]);
            let input = args.last().unwrap();
            match after("-outformat", 1).unwrap()[0].as_str() {
                "TVG" => std::fs::write(&out, format!("{input} kept {}", after("-clearlayers", 1).unwrap()[0])).map_err(|why| why.to_string())?,
                _ => {
                    let size = after("-resolution", 2).unwrap();
                    let (width, height): (u32, u32) = (size[0].parse().unwrap(), size[1].parse().unwrap());
                    let mut pixels = vec![[0u8; 4]; (width * height) as usize];
                    pixels[(height / 2 * width + width / 2) as usize] = [255, 0, 0, 255];
                    std::fs::write(&out, crate::encode_png(width, height, &pixels)?).map_err(|why| why.to_string())?;
                }
            }
            Ok(String::new())
        }
    }

    #[test]
    fn each_art_layer_wanted_of_each_drawing_is_made_a_picture_by_the_converter() {
        let dir = tempfile::tempdir().unwrap();
        let text = r#"<project>
         <elements>
          <element id="2" elementName="ARM" elementFolder="ARM" fieldChart="12"><drawings><dwg name="1"/><dwg name="2"/></drawings></element>
          <element id="3" elementName="HAND" elementFolder="HAND" fieldChart="6"><drawings><dwg name="1"/></drawings></element>
         </elements>
         <options><resolution size="1920,1080"/></options>
         <scenes><scene startFrame="1"><columns>
          <column type="0" name="A"><elementSeq exposures="1" val="1" id="2"/></column>
         </columns><rootgroup name="Top"><nodeslist>
          <module type="READ" name="ARM"><attrs><drawing><element col="A"/></drawing></attrs></module>
          <module type="UNDERLAY" name="UL"/>
         </nodeslist><linkedlist><link out="ARM" in="UL"/></linkedlist></rootgroup></scene></scenes></project>"#;
        let scene = RigScene::parse(text, dir.path()).unwrap();
        for file in ["elements/ARM/ARM-1.tvg", "elements/ARM/ARM-2.tvg", "elements/HAND/HAND-1.tvg"] {
            std::fs::create_dir_all(dir.path().join(file).parent().unwrap()).unwrap();
            std::fs::write(dir.path().join(file), "drawing").unwrap();
        }
        let wanted = passes(&scene);
        assert_eq!(
            (wanted[&2].clone(), wanted[&3].clone()),
            (vec![Pass::Line, Pass::Colour, Pass::Over, Pass::Under], vec![Pass::Line, Pass::Colour, Pass::Over])
        );

        let out = dir.path().join("pictures");
        let stand = Stand { said: Mutex::new(Vec::new()) };
        let (done, cancel) = (AtomicUsize::new(0), AtomicBool::new(false));
        let converted = convert_drawings(&scene, &stand, &out, 10.0, 2, &done, &cancel);
        assert_eq!((converted.drawings, converted.pictures, done.into_inner(), converted.cancelled), (3, 11, 3, false));
        assert_eq!(drawing_count(&scene), 3);
        let arm = scene.element(2).unwrap();
        for pass in [Pass::Line, Pass::Colour, Pass::Over, Pass::Under] {
            assert!(picture_path(&out, arm, "1", pass).is_file(), "{pass:?}");
        }
        assert!(!picture_path(&out, scene.element(3).unwrap(), "1", Pass::Under).exists(), "no node takes the hand's underlay");
        assert!(!out.join("elements/ARM/ARM-1.line.tvg").exists(), "the copies are taken away");

        let said = stand.said.lock().unwrap().clone();
        let find = |what: &str, of: &str| said.iter().find(|args| args.join(" ").contains(what) && args.last().unwrap().contains(of)).cloned().unwrap();
        // The line art alone: every other art layer cleared from a copy first.
        let copy = find("-clearlayers", "ARM-1.tvg");
        assert!(copy.join(" ").contains("-outformat TVG"));
        let line = find("-clearlayers colorart,underlayart,overlayart", "ARM-1.tvg");
        assert!(line.join(" ").contains("ARM-1.line.tvg"), "the copy is the line art's: {line:?}");
        // A picture covers the chart, 24 by 18 fields at 10 pixels a field; the arm, reaching 18
        // fields to the left, a chart half again as large and more, scaled down by as much.
        let picture = |of: &str| said.iter().find(|args| args.contains(&"PNG4".to_string()) && args.last().unwrap().contains(of)).cloned().unwrap();
        let arm_line = picture("ARM-1.line.tvg");
        let at = arm_line.iter().position(|arg| arg == "-resolution").unwrap();
        assert_eq!((&arm_line[at + 1], &arm_line[at + 2]), (&"367".to_string(), &"275".to_string()));
        assert!(arm_line.join(" ").contains("-scale 0.653595") && arm_line.join(" ").contains("-bgalpha 0"), "{arm_line:?}");
        let plain = picture("ARM-2.line.tvg");
        assert!(plain.join(" ").contains("-resolution 240 180") && !plain.join(" ").contains("-scale"), "{plain:?}");
        // The hand's element draws its 12 fields as 6: half the pixels a field.
        assert!(picture("HAND-1.colour.tvg").join(" ").contains("-resolution 120 90"));
        // Underlay and overlay art are drawn only when the converter is asked to.
        assert!(picture("ARM-1.underlay.tvg").contains(&"-useunderlayart".to_string()) && picture("ARM-1.overlay.tvg").contains(&"-useoverlayart".to_string()));
        assert!(!plain.iter().any(|arg| arg.starts_with("-use")), "{plain:?}");
        assert!(
            converted.log.iter().all(|line| !line.starts_with("converter -printbox")),
            "only the first drawing's commands are in the log: {:?}",
            converted.log
        );
        assert!(converted.log.iter().any(|line| line.starts_with("converter -debug -printbox")));
        // The first drawing is asked to say what goes wrong, the others not.
        assert!(said[0][0] == "-debug" && said.iter().any(|args| args[0] != "-debug"), "{:?}", said[0]);
    }

    #[test]
    fn a_converter_that_fails_or_is_cancelled_is_said_so() {
        struct Broken;
        impl DrawingConverter for Broken {
            fn run(&self, _: &[String]) -> Result<String, String> {
                Err("the converter stopped with exit status: 1: no licence".to_string())
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let text = r#"<project><elements><element id="2" elementName="A" elementFolder="A"><drawings><dwg name="1"/><dwg name="2"/></drawings></element></elements>
            <options><resolution size="640,480"/></options><scenes><scene><rootgroup name="Top"/></scene></scenes></project>"#;
        let scene = RigScene::parse(text, dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join("elements/A")).unwrap();
        std::fs::write(dir.path().join("elements/A/A-1.tvg"), "drawing").unwrap();
        let (done, cancel) = (AtomicUsize::new(0), AtomicBool::new(false));
        let converted = convert_drawings(&scene, &Broken, &dir.path().join("out"), 10.0, 1, &done, &cancel);
        assert_eq!((converted.pictures, converted.failed, done.into_inner()), (0, true, 0), "the second drawing is not tried");
        assert!(converted.log.iter().all(|line| !line.contains("A-2.tvg")), "{:?}", converted.log);
        assert!(converted.log.iter().any(|line| line.contains("no licence")), "{:?}", converted.log);
        assert!(converted.log.iter().any(|line| line.contains("made the size of the field chart")));
        let cancel = AtomicBool::new(true);
        let converted = convert_drawings(&scene, &Broken, &dir.path().join("out"), 10.0, 1, &AtomicUsize::new(0), &cancel);
        assert!(converted.cancelled && converted.log.is_empty());
        assert_eq!(printed_box("Bounding box: 1 -2.5 3e2 4"), Some([1.0, -2.5, 300.0, 4.0]));
        assert_eq!(printed_box("nothing"), None);
    }

    #[test]
    fn a_first_drawing_whose_file_is_gone_does_not_stop_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let text = r#"<project><elements>
            <element id="2" elementName="SKETCH" elementFolder="SKETCH"><drawings><dwg name="1"/></drawings></element>
            <element id="3" elementName="ARM" elementFolder="ARM"><drawings><dwg name="1"/></drawings></element></elements>
            <options><resolution size="640,480"/></options><scenes><scene><rootgroup name="Top"/></scene></scenes></project>"#;
        let scene = RigScene::parse(text, dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join("elements/ARM")).unwrap();
        std::fs::write(dir.path().join("elements/ARM/ARM-1.tvg"), "drawing").unwrap();
        let stand = Stand { said: Mutex::new(Vec::new()) };
        let (done, cancel) = (AtomicUsize::new(0), AtomicBool::new(false));
        let converted = convert_drawings(&scene, &stand, &dir.path().join("out"), 10.0, 1, &done, &cancel);
        assert_eq!((converted.failed, converted.pictures, done.into_inner()), (false, 3, 2), "{:?}", converted.log);
        assert!(converted.log.iter().any(|line| line.contains("SKETCH-1.tvg: no such drawing file")), "{:?}", converted.log);
        // The drawing that is there is the one asked to say what goes wrong.
        assert!(stand.said.lock().unwrap()[0].join(" ").contains("-debug -printbox") && stand.said.lock().unwrap()[0].last().unwrap().contains("ARM-1.tvg"));
    }

    /// A converter whose licence lets one copy run at a time: the others, turned down, are run
    /// again in turn, and every command gets through.
    #[cfg(unix)]
    #[test]
    fn copies_turned_down_by_the_licence_are_run_again_in_turn() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("converter");
        // The seat is a folder: a copy that can't make it is turned down, as a licence does.
        let text = "#!/bin/sh\nif mkdir seat 2>/dev/null; then sleep 0.05; rmdir seat; echo \"made $1\"; else echo 'Licensing error (Utilities) Licensed number of users already reached.' >&2; exit 1; fi\n";
        std::fs::write(&script, text).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let program = Program::new(script);
        let said: Vec<Result<String, String>> = std::thread::scope(|scope| {
            let runs: Vec<_> = (0..8)
                .map(|n| {
                    let program = &program;
                    scope.spawn(move || program.run(&[n.to_string()]))
                })
                .collect();
            runs.into_iter().map(|run| run.join().unwrap()).collect()
        });
        for (n, said) in said.iter().enumerate() {
            assert_eq!(said.as_deref(), Ok(format!("made {n}\n").as_str()));
        }
        assert!(program.one_at_a_time.load(Ordering::Relaxed), "turned down at least once, so run one at a time");
        assert!(licence_refused("the converter stopped with exit status: 1: Licensing error (Utilities) / Licensed number of users already reached."));
        assert!(!licence_refused("the converter stopped with exit status: 1: no such file"));
    }
}
