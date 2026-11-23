//! `iskra-render`: renders the shots of an Iskra project to image sequences, without the app
//! (plan §7.14, R6). It runs the engine's own command, so what it writes is what the Export
//! page writes. The project file is only read.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use iskra_engine::Session;
use serde_json::{Value, json};

const USAGE: &str = "usage: iskra-render <project.iskra> [options]

Renders shots to image sequences, each in a folder of its own: <out>/Shot_01/Shot_01.0001.png
or to movies, all together in the folder: <out>/Shot_01.mov

  --out <folder>     where the sequences' folders go (default: Renders beside the project file)
  --shot <name>      a shot to render; give it again for more (default: every shot)
  --format <kind>    png, jpeg, tiff or exr; mov (ProRes) or mp4 (H.264) for movies (default: png)
  --film             the storyboard as one movie, with its sound: <out>/<Storyboard>.mov
  --no-sound         movies without their sound
  --wav              an image sequence with its sound beside its frames: <out>/Shot_01/Shot_01.wav
  --sound-bits <n>   uncompressed sound of 16 or 24 bits, or 32 for floats (default: 16)
  --edit-list <kind> also write the storyboard's cut as an edit list: edl, otio or fcpxml
  --bits <n>         bits a channel: 8, 16, or 32 for floats (default: the format's least)
  --no-alpha         no transparency: the picture on white
  --scale <s>        of the shot's size (default: 1)
  --fps <rate>       another frame rate than the project's: 24, 25, 30, 23.976, 29.97…; each
                     frame is the project's on screen at its moment, repeated or left out
  --whole            all of each shot, not only the part in the cut
  --boards           the shots' boards, as an animatic has them, not their animation
  --pass <name>      a pass to render, each an output of its own, a folder down under its
                     name: <out>/Shot_01/Character/Shot_01_Character.0001.png; give it again
                     for more. A shot with none of them is rendered whole, as Final
  --final            with --pass, each shot's whole picture too: <out>/Shot_01/Final/…
  --handles <n>      frames of the handles to render before and after the cut (default: 0)
  --replace          write over a shot's frames that are there already
  --version          write beside them, as Shot_01_v002
  --skip             leave shots that are rendered already

Without --replace, --version or --skip, a shot whose folder has frames already is an error.

Or prints a storyboard's panels on pages, as a PDF, rendering nothing else:

  --pdf <file.pdf>       the PDF to write (written over if it is there)
  --storyboard <name>    the storyboard to print (default: the project's first)
  --mode <kind>          sheets, script, presentation or thumbnails (default: sheets)
  --paper <size>         a3, a4, a5, letter, legal, tabloid, or <width>x<height> in mm
                         (default: a4)
  --landscape            the paper on its side; --portrait upright (default: as the mode has it)
  --grid <c>x<r>         panels across and down a page (Sheets, Thumbnails); rows for Script
  --page-per-shot        each shot starts a page
  --label <text>         a version, in the header
  --dpi <n>              pixels an inch of the pictures, and of the PNGs (default: 150)
  --grey                 the pictures in grey
  --pngs                 a PNG of each page beside the PDF: <file>_01.png…";

/// What the command line asks for.
#[derive(Debug, PartialEq)]
struct Asked {
    project: PathBuf,
    out: Option<PathBuf>,
    shots: Vec<String>,
    settings: Value,
    if_exists: &'static str,
    /// The storyboard as one movie, not a file a shot.
    film: bool,
    /// The kind of edit list to write after the render, if one was asked for.
    edit_list: Option<String>,
    /// The passes to render, by their names, and whether the whole picture is rendered beside
    /// them as Final.
    passes: Vec<String>,
    with_final: bool,
    /// The PDF to print the storyboard into, its storyboard by name, the pages' options as the
    /// engine's `export.pdf` takes them, and whether a PNG of each page is written too.
    pdf: Option<PathBuf>,
    storyboard: Option<String>,
    pages: Value,
    pngs: bool,
}

fn parse(args: &[String]) -> Result<Asked, String> {
    let mut asked = Asked {
        project: PathBuf::new(),
        out: None,
        shots: Vec::new(),
        settings: json!({}),
        if_exists: "fail",
        film: false,
        edit_list: None,
        passes: Vec::new(),
        with_final: false,
        pdf: None,
        storyboard: None,
        pages: json!({}),
        pngs: false,
    };
    let (mut whole, mut handles) = (false, 0u32);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |what: &str| args.next().cloned().ok_or_else(|| format!("{arg} needs {what}"));
        match arg.as_str() {
            "--out" => asked.out = Some(PathBuf::from(value("a folder")?)),
            "--shot" => asked.shots.push(value("a shot's name")?),
            "--format" => {
                let format = value("a format")?.to_lowercase();
                asked.settings["format"] = json!(match format.as_str() {
                    "jpg" => "jpeg",
                    "tif" => "tiff",
                    other => other,
                });
            }
            "--bits" => asked.settings["bits"] = json!(value("a number")?.parse::<u8>().map_err(|_| "--bits needs 8, 16 or 32".to_string())?),
            "--scale" => asked.settings["scale"] = json!(value("a number")?.parse::<f64>().map_err(|_| "--scale needs a number".to_string())?),
            "--handles" => handles = value("a number")?.parse().map_err(|_| "--handles needs a number of frames".to_string())?,
            "--no-alpha" => asked.settings["alpha"] = json!(false),
            "--boards" => asked.settings["work"] = json!("boards"),
            "--no-sound" => asked.settings["sound"] = json!(false),
            "--fps" => {
                let said = value("a frame rate")?;
                // The NTSC rates are written short: 23.976 is 24000/1001.
                let rate = match said.as_str() {
                    "23.976" | "23.98" => json!({ "num": 24000, "den": 1001 }),
                    "29.97" => json!({ "num": 30000, "den": 1001 }),
                    "59.94" => json!({ "num": 60000, "den": 1001 }),
                    whole => {
                        json!({ "num": whole.parse::<u32>().ok().filter(|fps| *fps > 0).ok_or_else(|| format!("--fps needs a rate, not {whole}"))?, "den": 1 })
                    }
                };
                asked.settings["frameRate"] = rate;
            }
            "--wav" => asked.settings["soundBeside"] = json!(true),
            "--sound-bits" => {
                let bits = value("16, 24 or 32")?.parse::<u16>().ok().filter(|bits| [16, 24, 32].contains(bits));
                asked.settings["soundBits"] = json!(bits.ok_or_else(|| "--sound-bits needs 16, 24 or 32".to_string())?);
            }
            "--film" => asked.film = true,
            "--pass" => asked.passes.push(value("a pass's name")?),
            "--final" => asked.with_final = true,
            "--edit-list" => asked.edit_list = Some(value("edl, otio or fcpxml")?.to_lowercase()),
            "--pdf" => asked.pdf = Some(PathBuf::from(value("a file")?)),
            "--storyboard" => asked.storyboard = Some(value("a storyboard's name")?),
            "--mode" => {
                let mode = value("a mode")?.to_lowercase();
                if !matches!(mode.as_str(), "sheets" | "script" | "presentation" | "thumbnails") {
                    return Err(format!("--mode needs sheets, script, presentation or thumbnails, not {mode}"));
                }
                asked.pages["mode"] = json!(mode);
            }
            "--paper" => {
                let paper = value("a paper size")?.to_lowercase();
                asked.pages["paper"] = match paper.split_once('x') {
                    Some((width, height)) => {
                        let mm = |side: &str| {
                            side.parse::<f32>().ok().filter(|mm| *mm >= 50.0).ok_or_else(|| format!("--paper needs a size in mm of 50 or more, not {paper}"))
                        };
                        json!({ "own": { "width": mm(width)?, "height": mm(height)? } })
                    }
                    None if matches!(paper.as_str(), "a3" | "a4" | "a5" | "letter" | "legal" | "tabloid") => json!(paper),
                    None => return Err(format!("--paper needs a3, a4, a5, letter, legal, tabloid or a size, not {paper}")),
                };
            }
            "--landscape" => asked.pages["landscape"] = json!(true),
            "--portrait" => asked.pages["landscape"] = json!(false),
            "--grid" => {
                let grid = value("columns and rows")?;
                let (columns, rows) = match grid.split_once('x') {
                    Some((columns, rows)) => (columns.parse::<u32>().ok(), rows.parse::<u32>().ok()),
                    None => (Some(1), grid.parse::<u32>().ok()),
                };
                let (Some(columns @ 1..), Some(rows @ 1..)) = (columns, rows) else { return Err(format!("--grid needs columns x rows, as 2x3, not {grid}")) };
                (asked.pages["columns"], asked.pages["rows"]) = (json!(columns), json!(rows));
            }
            "--page-per-shot" => asked.pages["pagePerShot"] = json!(true),
            "--label" => asked.pages["version"] = json!(value("a version")?),
            "--dpi" => {
                asked.pages["dpi"] = json!(value("a number")?.parse::<u32>().ok().filter(|dpi| *dpi > 0).ok_or_else(|| "--dpi needs a number".to_string())?)
            }
            "--grey" => asked.pages["grey"] = json!(true),
            "--pngs" => asked.pngs = true,
            "--whole" => whole = true,
            "--replace" => asked.if_exists = "replace",
            "--version" => asked.if_exists = "version",
            "--skip" => asked.if_exists = "skip",
            option if option.starts_with('-') => return Err(format!("unknown option {option}")),
            project if asked.project.as_os_str().is_empty() => asked.project = PathBuf::from(project),
            extra => return Err(format!("one project file at a time: {extra}")),
        }
    }
    if asked.project.as_os_str().is_empty() {
        return Err("no project file given".to_string());
    }
    if asked.pdf.is_none() && (asked.storyboard.is_some() || asked.pngs || asked.pages != json!({})) {
        return Err("the pages' options are for --pdf: give it too".to_string());
    }
    if asked.with_final && asked.passes.is_empty() {
        return Err("--final is the whole picture beside passes: give a --pass too".to_string());
    }
    asked.settings["range"] = if whole { json!("whole") } else { json!({ "cut": { "handles": handles } }) };
    Ok(asked)
}

/// What is rendered of a shot whose passes are named `has`, as the Export page does it: without
/// passes asked for, the shot as it is (`None`); else each pass asked for that it has, an
/// output of its own, and Final beside them if it is asked for, or if the shot has none.
fn picks(asked: &Asked, has: &[&str]) -> Vec<Option<Value>> {
    if asked.passes.is_empty() {
        return vec![None];
    }
    let on: Vec<&String> = asked.passes.iter().filter(|pass| has.contains(&pass.as_str())).collect();
    let mut picks: Vec<Option<Value>> = on.iter().map(|pass| Some(json!({ "pass": pass, "named": pass }))).collect();
    if asked.with_final || on.is_empty() {
        picks.push(Some(json!({ "named": "Final" })));
    }
    picks
}

/// Render what was asked for. Returns how many files were written.
fn run(asked: &Asked) -> Result<u64, String> {
    let mut session = Session::new();
    session.execute("file.open", json!({ "path": asked.project })).map_err(|e| e.to_string())?;
    let project = session.project().clone();
    if let Some(file) = &asked.pdf {
        let storyboard = match &asked.storyboard {
            Some(name) => {
                Some(project.storyboards.iter().find(|board| &board.name == name).ok_or_else(|| format!("the project has no storyboard named {name}"))?.id.0)
            }
            None => None,
        };
        let params = json!({ "storyboard": storyboard, "file": file, "options": asked.pages, "pngs": asked.pngs });
        let made = session.execute("export.pdf", params).map_err(|e| e.to_string())?;
        println!("{}", made["file"].as_str().unwrap_or(""));
        let pictures = made["pictures"].as_array().map_or(0, Vec::len);
        for picture in made["pictures"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            println!("{picture}");
        }
        return Ok(1 + pictures as u64);
    }
    let shots: Vec<u64> = if asked.shots.is_empty() {
        project.shots.iter().map(|shot| shot.id.0).collect()
    } else {
        let named = |name: &String| {
            project.shots.iter().find(|shot| &shot.name == name).map(|shot| shot.id.0).ok_or_else(|| format!("the project has no shot named {name}"))
        };
        asked.shots.iter().map(named).collect::<Result<_, _>>()?
    };
    let out = asked.out.clone().unwrap_or_else(|| asked.project.parent().unwrap_or(Path::new("")).join("Renders"));
    if asked.film {
        // A film is a movie: ProRes unless another kind was asked for.
        let mut settings = asked.settings.clone();
        if settings["format"].is_null() {
            settings["format"] = json!("mov");
        }
        // A movie of each pass asked for, every shot through it; Final beside them if asked.
        let mut picks: Vec<Option<Value>> = asked.passes.iter().map(|pass| Some(json!({ "pass": pass, "named": pass }))).collect();
        if picks.is_empty() || asked.with_final {
            picks.push((!picks.is_empty()).then(|| json!({ "named": "Final" })));
        }
        let mut written = 0;
        for pick in picks {
            let params = json!({ "folder": out, "settings": settings, "ifExists": asked.if_exists, "pick": pick });
            let made = session.execute("export.storyboard", params).map_err(|e| e.to_string())?;
            match made["file"].as_str() {
                Some(file) => {
                    println!("{file}: {} frames", made["frames"][1].as_u64().unwrap_or(0));
                    written += 1;
                }
                None => println!("the film is there already, left as it is"),
            }
        }
        return Ok(written);
    }
    let mut written = 0;
    // A shot at a time, so that what is done is said as it is done.
    for shot in shots {
        let has: Vec<&str> = project.shots.iter().find(|own| own.id.0 == shot).map_or(Vec::new(), |own| own.passes().map(|pass| pass.name.as_str()).collect());
        for pick in picks(asked, &has) {
            let params = json!({ "shot": shot, "folder": out, "settings": asked.settings, "ifExists": asked.if_exists, "pick": pick });
            let made = session.execute("export.sequence", params).map_err(|e| e.to_string())?;
            let sequence = &made["sequences"][0];
            let (files, folder) = (sequence["files"].as_u64().unwrap_or(0), sequence["folder"].as_str().unwrap_or(""));
            match (files, sequence["file"].as_str()) {
                (0, _) => println!("{folder}: there already, left as it is"),
                (_, Some(movie)) => println!("{movie}"),
                (files, None) => println!("{folder}: {files} frames"),
            }
            for note in sequence["notes"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                eprintln!("  {note}");
            }
            written += files;
        }
    }
    // The cut for the editor, naming the movies that were just rendered.
    if let Some(kind) = &asked.edit_list {
        let movies = asked.settings["format"].as_str().filter(|format| matches!(*format, "mov" | "mp4")).unwrap_or("mov");
        let made = session.execute("export.editList", json!({ "folder": out, "format": kind, "movies": movies })).map_err(|e| e.to_string())?;
        println!("{}", made["file"].as_str().unwrap_or(""));
    }
    Ok(written)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let asked = match parse(&args) {
        Ok(asked) => asked,
        Err(why) => {
            eprintln!("iskra-render: {why}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&asked) {
        Ok(_) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("iskra-render: {why}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_command_line_says_what_to_render_and_how() {
        let asked = parse(&args("Film.iskra --shot Shot_02 --format tif --bits 16 --handles 4 --out out --version")).unwrap();
        assert_eq!(
            (asked.project.as_path(), asked.out.as_deref(), &asked.shots[..], asked.if_exists),
            (Path::new("Film.iskra"), Some(Path::new("out")), &["Shot_02".to_string()][..], "version")
        );
        assert_eq!(asked.settings, json!({ "format": "tiff", "bits": 16, "range": { "cut": { "handles": 4 } } }));
        assert_eq!(parse(&args("Film.iskra --whole --no-alpha")).unwrap().settings, json!({ "alpha": false, "range": "whole" }));
        assert_eq!(parse(&args("Film.iskra --boards")).unwrap().settings["work"], "boards");
        let film = parse(&args("Film.iskra --film --no-sound --format mp4")).unwrap();
        assert!(film.film && film.settings["sound"] == json!(false) && film.settings["format"] == json!("mp4"));
        assert_eq!(parse(&args("Film.iskra --fps 29.97")).unwrap().settings["frameRate"], json!({ "num": 30000, "den": 1001 }));
        assert_eq!(parse(&args("Film.iskra --fps 30")).unwrap().settings["frameRate"], json!({ "num": 30, "den": 1 }));
        assert!(parse(&args("Film.iskra --fps fast")).is_err());
        let wav = parse(&args("Film.iskra --wav --sound-bits 24")).unwrap();
        assert_eq!((&wav.settings["soundBeside"], &wav.settings["soundBits"]), (&json!(true), &json!(24)));
        assert!(parse(&args("Film.iskra --sound-bits 20")).is_err());
        for wrong in ["", "--format png", "a.iskra b.iskra", "a.iskra --bits many", "a.iskra --shot", "a.iskra --fast"] {
            assert!(parse(&args(wrong)).is_err(), "{wrong}");
        }
    }

    #[test]
    fn a_projects_shots_are_rendered_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Film.iskra");
        let mut session = Session::new();
        session.execute("project.setResolution", json!({ "width": 64, "height": 36 })).unwrap();
        session.execute("shot.setDuration", json!({ "frames": 3 })).unwrap();
        session.execute("shot.new", json!({})).unwrap();
        session.execute("file.saveAs", json!({ "path": file })).unwrap();

        let all = parse(&[file.to_string_lossy().into_owned()]).unwrap();
        // The second shot is as long as a new shot is.
        let second = session.project().shots[1].duration;
        assert_eq!(run(&all), Ok(3 + u64::from(second)));
        assert!(dir.path().join("Renders/Shot_01/Shot_01.0003.png").is_file() && dir.path().join("Renders/Shot_02/Shot_02.0001.png").is_file());
        // Again without saying what to do with what is there: refused. Skipped: nothing written.
        assert!(run(&all).is_err_and(|why| why.contains("already")));
        assert_eq!(run(&Asked { if_exists: "skip", ..parse(&[file.to_string_lossy().into_owned()]).unwrap() }), Ok(0));
        // Movies, and the film as one.
        let movies =
            Asked { settings: json!({ "format": "mp4", "range": { "cut": { "handles": 0 } } }), ..parse(&[file.to_string_lossy().into_owned()]).unwrap() };
        assert_eq!(run(&movies), Ok(2));
        assert!(dir.path().join("Renders/Shot_01.mp4").is_file());
        let film = Asked { film: true, ..parse(&[file.to_string_lossy().into_owned()]).unwrap() };
        assert_eq!(run(&film), Ok(1));
        assert!(dir.path().join("Renders/Storyboard.mov").is_file());
        // With the cut for the editor beside them.
        let again =
            Asked { settings: json!({ "format": "mp4", "range": { "cut": { "handles": 0 } } }), ..parse(&[file.to_string_lossy().into_owned()]).unwrap() };
        let listed = Asked { if_exists: "replace", edit_list: Some("edl".to_string()), ..again };
        assert_eq!(run(&listed), Ok(2));
        assert!(std::fs::read_to_string(dir.path().join("Renders/Storyboard.edl")).unwrap().contains("Shot_01.mp4"));
        // One shot by its name; a name the project lacks is said.
        let one = Asked { shots: vec!["Shot_01".to_string()], out: Some(dir.path().join("one")), ..parse(&[file.to_string_lossy().into_owned()]).unwrap() };
        assert_eq!(run(&one), Ok(3));
        let none = Asked { shots: vec!["Shot_99".to_string()], ..parse(&[file.to_string_lossy().into_owned()]).unwrap() };
        assert!(run(&none).is_err_and(|why| why.contains("no shot named Shot_99")));
    }

    #[test]
    fn a_storyboard_is_printed_as_a_pdf() {
        let asked = parse(&args("Film.iskra --pdf boards.pdf --mode Script --paper letter --landscape --grid 4 --label v2 --dpi 100 --grey --pngs")).unwrap();
        assert_eq!((asked.pdf.as_deref(), asked.pngs), (Some(Path::new("boards.pdf")), true));
        assert_eq!(
            asked.pages,
            json!({ "mode": "script", "paper": "letter", "landscape": true, "columns": 1, "rows": 4, "version": "v2", "dpi": 100, "grey": true })
        );
        assert_eq!(
            parse(&args("Film.iskra --pdf b.pdf --paper 100x150 --grid 3x5")).unwrap().pages,
            json!({ "paper": { "own": { "width": 100.0, "height": 150.0 } }, "columns": 3, "rows": 5 })
        );
        for wrong in
            ["a.iskra --pdf b.pdf --mode comic", "a.iskra --pdf b.pdf --paper b2", "a.iskra --pdf b.pdf --grid 0x2", "a.iskra --pngs", "a.iskra --mode script"]
        {
            assert!(parse(&args(wrong)).is_err(), "{wrong}");
        }

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Film.iskra");
        let mut session = Session::new();
        session.execute("project.setResolution", json!({ "width": 64, "height": 36 })).unwrap();
        session.execute("file.saveAs", json!({ "path": file })).unwrap();
        let pdf = dir.path().join("Film boards.pdf");
        let asked =
            parse(&args(&format!("{} --pdf {} --mode thumbnails --pngs --dpi 30", file.display(), pdf.display()).replace("Film boards", "Film_boards")))
                .unwrap();
        assert_eq!(run(&asked), Ok(2), "the PDF and the picture of its one page");
        assert!(std::fs::read(dir.path().join("Film_boards.pdf")).unwrap().starts_with(b"%PDF-"));
        assert!(dir.path().join("Film_boards_01.png").is_file());
        let named = Asked { storyboard: Some("Trailer".to_string()), ..asked };
        assert!(run(&named).is_err_and(|why| why.contains("no storyboard named Trailer")));
    }

    #[test]
    fn passes_are_asked_for_by_name_and_a_shot_without_them_is_rendered_whole() {
        let asked = parse(&args("Film.iskra --pass Character --pass Background --final")).unwrap();
        assert_eq!((&asked.passes[..], asked.with_final), (&["Character".to_string(), "Background".to_string()][..], true));
        assert!(parse(&args("Film.iskra --final")).is_err_and(|why| why.contains("give a --pass")));
        let named = |pass: &str| Some(json!({ "pass": pass, "named": pass }));
        let whole = Some(json!({ "named": "Final" }));
        assert_eq!(picks(&asked, &["Character"]), [named("Character"), whole.clone()]);
        assert_eq!(picks(&parse(&args("Film.iskra --pass Character")).unwrap(), &[]), [whole]);
        assert_eq!(picks(&parse(&args("Film.iskra")).unwrap(), &["Character"]), [None]);

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Film.iskra");
        let mut session = Session::new();
        session.execute("project.setResolution", json!({ "width": 64, "height": 36 })).unwrap();
        session.execute("shot.setDuration", json!({ "frames": 2 })).unwrap();
        let layer = session.project().shots[0].layers[0].id.0;
        session.execute("display.new", json!({ "node": { "layer": layer }, "pass": true, "name": "Character" })).unwrap();
        session.execute("shot.new", json!({})).unwrap();
        session.execute("file.saveAs", json!({ "path": file })).unwrap();
        let passes = parse(&args(&format!("{} --pass Character --whole", file.display()))).unwrap();
        run(&passes).unwrap();
        assert!(dir.path().join("Renders/Shot_01/Character/Shot_01_Character.0001.png").is_file());
        assert!(dir.path().join("Renders/Shot_02/Final").is_dir() && !dir.path().join("Renders/Shot_01/Final").exists());
        let film = Asked { film: true, settings: json!({ "format": "mp4", "range": "whole" }), ..passes };
        assert_eq!(run(&film), Ok(1));
        assert!(dir.path().join("Renders/Character/Storyboard_Character.mp4").is_file());
    }
}
