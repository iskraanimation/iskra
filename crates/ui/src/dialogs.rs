//! The operating system's file and message dialogs. They block until answered.

use std::path::PathBuf;

const EXTENSION: &str = "iskra";

pub fn open_project() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Open Project").add_filter("Iskra project", &[EXTENSION]).pick_file()
}

pub fn save_project(name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Save Project").add_filter("Iskra project", &[EXTENSION]).set_file_name(format!("{name}.{EXTENSION}")).save_file()
}

/// Ask for the folder a new project's own folder is made in.
pub fn project_location() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Where to Keep the Project").pick_folder()
}

/// Ask for a script to run once.
pub fn script_file() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Run Script File").add_filter("JavaScript", &["js"]).pick_file()
}

/// Ask for an image file to show on a script button.
pub fn button_icon() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Choose an Icon for the Button").add_filter("Icon", &["png", "svg"]).pick_file()
}

/// Ask for a folder to add to the library.
pub fn library_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Add a Folder to the Library").pick_folder()
}

/// Ask for the folder to write the files of the shots that are sent out into.
pub fn send_out_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Send Shots Out: Choose a Folder").pick_folder()
}

/// Ask for the file of a shot that has come back.
pub fn returned_shot() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Bring a Shot Back").add_filter("Iskra project", &[EXTENSION]).pick_file()
}

/// Ask for a file of brushes to bring in: the app's own, or Photoshop's.
pub fn import_brushes() -> Option<PathBuf> {
    let both = [iskra_io::BRUSHES_EXTENSION, iskra_io::ABR];
    rfd::FileDialog::new().set_title("Import Brushes").add_filter("Brushes", &both).pick_file()
}

/// Ask where to write brushes, offering `name` for the file. The file's kind is told by its
/// extension.
pub fn export_brushes(name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Export Brushes")
        .add_filter("Iskra brushes", &[iskra_io::BRUSHES_EXTENSION])
        .add_filter("Photoshop brushes", &[iskra_io::ABR])
        .set_file_name(format!("{name}.{}", iskra_io::BRUSHES_EXTENSION))
        .save_file()
}

/// Ask where to write brushes as a Photoshop brush file of the old kind, which holds the tips
/// and is read by every program that reads brush files.
pub fn export_old_brushes(name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Export Brushes for Older Programs")
        .add_filter("Photoshop brushes, the old kind", &[iskra_io::ABR])
        .set_file_name(format!("{name}.{}", iskra_io::ABR))
        .save_file()
}

/// Ask for palette files to bring in: the app's own, GIMP's or Adobe's. It starts in `folder`
/// if there is one.
pub fn import_palettes(folder: Option<&std::path::Path>, title: &str) -> Option<Vec<PathBuf>> {
    let dialog = rfd::FileDialog::new().set_title(title).add_filter("Palettes", &iskra_io::PALETTE_EXTENSIONS);
    folder.map_or(dialog.clone(), |folder| dialog.set_directory(folder)).pick_files()
}

/// Ask where to write the palette. The file's kind is told by its extension.
pub fn export_palette(folder: Option<&std::path::Path>, name: &str) -> Option<PathBuf> {
    let dialog = rfd::FileDialog::new()
        .set_title("Export Palette")
        .add_filter("Iskra palette, with its styles", &[iskra_io::PALETTE_EXTENSION])
        .add_filter("GIMP palette, names and colours", &[iskra_io::GPL])
        .set_file_name(format!("{name}.{}", iskra_io::PALETTE_EXTENSION));
    folder.map_or(dialog.clone(), |folder| dialog.set_directory(folder)).save_file()
}

/// Ask for pictures and sound to bring into the project, starting in `folder` if there is one.
pub fn import_media(folder: Option<&std::path::Path>) -> Option<Vec<PathBuf>> {
    let all: Vec<&str> = iskra_io::IMAGE_EXTENSIONS.iter().chain(&iskra_io::SOUND_EXTENSIONS).copied().collect();
    let dialog = rfd::FileDialog::new()
        .set_title("Import Media")
        .add_filter("Pictures and sound", &all)
        .add_filter("Pictures", &iskra_io::IMAGE_EXTENSIONS)
        .add_filter("Sound", &iskra_io::SOUND_EXTENSIONS);
    folder.map_or(dialog.clone(), |folder| dialog.set_directory(folder)).pick_files()
}

/// Ask for a layered file to bring a character in from, starting in `folder` if there is one.
pub fn import_character(folder: Option<&std::path::Path>) -> Option<PathBuf> {
    let dialog = rfd::FileDialog::new()
        .set_title("Import Character")
        .add_filter("Layered files", &iskra_io::CHARACTER_EXTENSIONS)
        .add_filter("Photoshop files", &["psd", "psb"])
        .add_filter("OpenRaster files", &["ora"]);
    folder.map_or(dialog.clone(), |folder| dialog.set_directory(folder)).pick_file()
}

/// Ask for any one file, of whatever kind, in a window called `title`.
pub fn pick_any(title: &str) -> Option<PathBuf> {
    rfd::FileDialog::new().set_title(title).pick_file()
}

/// Ask for one picture file, in a window called `title`.
pub fn pick_picture(title: &str) -> Option<PathBuf> {
    rfd::FileDialog::new().set_title(title).add_filter("Pictures", &iskra_io::IMAGE_EXTENSIONS).pick_file()
}

/// Ask for the file a media item is to refer to from now on.
pub fn relink_media(sound: bool) -> Option<PathBuf> {
    let (name, endings): (&str, &[&str]) = if sound { ("Sound", &iskra_io::SOUND_EXTENSIONS) } else { ("Pictures", &iskra_io::IMAGE_EXTENSIONS) };
    rfd::FileDialog::new().set_title("Relink Media").add_filter(name, endings).pick_file()
}

/// Ask for the file a sound's transcript is read from.
pub fn import_transcript() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Import Transcript").add_filter("Subtitles or words", &iskra_io::TRANSCRIPT_EXTENSIONS).pick_file()
}

/// Ask for a render preset's file to bring in.
pub fn import_render_preset() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Import Render Preset").add_filter("Iskra render preset", &[iskra_export::PRESET_EXTENSION]).pick_file()
}

/// Ask where to write a render preset as a file.
pub fn export_render_preset(name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Export Render Preset")
        .add_filter("Iskra render preset", &[iskra_export::PRESET_EXTENSION])
        .set_file_name(format!("{name}.{}", iskra_export::PRESET_EXTENSION))
        .save_file()
}

/// Ask for a folder of pictures of the same names as a layer's, to be a skin of it.
pub fn skin_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Add Skin From Folder").pick_folder()
}

/// Ask for the folder renders go into.
pub fn render_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Render To").pick_folder()
}

/// Ask what to do about files that are already where a render would write (plan §7.14):
/// write over them, write a new version beside them, or leave them and go on. `None` stops.
pub fn files_exist(at: &std::path::Path) -> Option<iskra_export::IfExists> {
    let (replace, version, skip) = ("Replace", "New Version", "Skip");
    let buttons = rfd::MessageButtons::YesNoCancelCustom(replace.to_string(), version.to_string(), skip.to_string());
    let question =
        format!("{} is rendered already.\n\nReplace it, write a new version beside it, or skip it? The answer holds for the rest of this job.", at.display());
    let answer =
        rfd::MessageDialog::new().set_level(rfd::MessageLevel::Warning).set_title("Already Rendered").set_description(question).set_buttons(buttons).show();
    match answer {
        rfd::MessageDialogResult::Yes => Some(iskra_export::IfExists::Replace),
        rfd::MessageDialogResult::No => Some(iskra_export::IfExists::Version),
        rfd::MessageDialogResult::Custom(chosen) if chosen == replace => Some(iskra_export::IfExists::Replace),
        rfd::MessageDialogResult::Custom(chosen) if chosen == version => Some(iskra_export::IfExists::Version),
        rfd::MessageDialogResult::Custom(chosen) if chosen == skip => Some(iskra_export::IfExists::Skip),
        rfd::MessageDialogResult::Cancel => Some(iskra_export::IfExists::Skip),
        _ => None,
    }
}

/// Ask where to write a storyboard's pages, offering `name` for the file.
pub fn export_pdf(name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Export Storyboard PDF").add_filter("PDF", &["pdf"]).set_file_name(format!("{name}.pdf")).save_file()
}

/// Open a file in the program the system opens its kind with: a PDF in its viewer.
pub fn open_file(path: &std::path::Path) {
    let (program, before): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("open", &[])
    } else if cfg!(windows) {
        ("cmd", &["/C", "start", ""])
    } else {
        ("xdg-open", &[])
    };
    let _ = std::process::Command::new(program).args(before).arg(path).spawn();
}

/// Show a file or a folder in the system's file browser.
pub fn reveal(path: &std::path::Path) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    // A file is shown in its folder; where the browser can't be started, nothing happens.
    let folder = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
    let _ = std::process::Command::new(program).arg(folder).spawn();
}

/// Ask a yes-or-no question. `yes` names the action, so the button says what it does.
pub fn confirm(title: &str, question: &str, yes: &str) -> bool {
    let buttons = rfd::MessageButtons::OkCancelCustom(yes.to_string(), "Cancel".to_string());
    let answer = rfd::MessageDialog::new().set_level(rfd::MessageLevel::Warning).set_title(title).set_description(question).set_buttons(buttons).show();
    match answer {
        rfd::MessageDialogResult::Custom(chosen) => chosen == yes,
        answer => answer == rfd::MessageDialogResult::Ok,
    }
}

/// What to do with unsaved changes before the project is left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsaved {
    Save,
    Discard,
    /// Don't leave the project after all.
    Cancel,
}

/// Ask what to do with the unsaved changes of the project named `name`.
pub fn unsaved_changes(name: &str) -> Unsaved {
    let (save, discard) = ("Save", "Don't Save");
    let buttons = rfd::MessageButtons::YesNoCancelCustom(save.to_string(), discard.to_string(), "Cancel".to_string());
    let question = format!("\"{name}\" has changes that are not saved. Save them?");
    let answer =
        rfd::MessageDialog::new().set_level(rfd::MessageLevel::Warning).set_title("Unsaved Changes").set_description(question).set_buttons(buttons).show();
    match answer {
        rfd::MessageDialogResult::Yes => Unsaved::Save,
        rfd::MessageDialogResult::No => Unsaved::Discard,
        rfd::MessageDialogResult::Custom(chosen) if chosen == save => Unsaved::Save,
        rfd::MessageDialogResult::Custom(chosen) if chosen == discard => Unsaved::Discard,
        _ => Unsaved::Cancel,
    }
}
