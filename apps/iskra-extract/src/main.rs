//! `iskra-extract`: recovers the drawings, timing and palette from an Iskra project or backup
//! file, as PNG and JSON files any program can read (plan §4.9).
//!
//! It doesn't need the app to be able to open the file: it reads whatever is still readable,
//! reports what it had to skip, and never writes to the project file.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: iskra-extract <project.iskra> [output folder]

Writes every drawing as a PNG, the timing as timing.json and the palette as palette.json.
The output folder defaults to \"<project name> extracted\" beside the project file.
The project file is only read, never changed.";

/// `<name> extracted` beside the project file.
fn default_output(project: &Path) -> PathBuf {
    let name = project.file_stem().map_or_else(|| "project".to_string(), |s| s.to_string_lossy().into_owned());
    project.with_file_name(format!("{name} extracted"))
}

fn run(project: &Path, out: &Path) -> Result<usize, String> {
    let (salvaged, load) = iskra_store::SqliteStore::salvage(project).map_err(|e| e.to_string())?;
    let export = iskra_io::export_project(&salvaged, out).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("{} drawings written to {}", export.drawings_written, out.display());
    if export.empty_drawings > 0 {
        println!("{} empty drawings have no image", export.empty_drawings);
    }
    let problems: Vec<&String> = load.skipped.iter().chain(&export.problems).collect();
    if !problems.is_empty() {
        eprintln!("{} problems:", problems.len());
        for problem in &problems {
            eprintln!("  {problem}");
        }
    }
    Ok(problems.len())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (project, out) = match &args[..] {
        [project] if !project.starts_with('-') => (PathBuf::from(project), default_output(Path::new(project))),
        [project, out] => (PathBuf::from(project), PathBuf::from(out)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&project, &out) {
        // Something was recovered, but not everything.
        Ok(problems) if problems > 0 => ExitCode::from(1),
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("iskra-extract: {e}");
            ExitCode::from(2)
        }
    }
}
