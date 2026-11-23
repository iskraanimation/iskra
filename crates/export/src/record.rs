//! Records of renders (plan §7.14): what the project keeps of a render that was made, and
//! whether what it shows has changed since.

use crate::Settings;

/// How what a render with `settings` shows is now (plan §7.14): what a record of that render,
/// made now, would hold. `None` if the project has no such shot or storyboard.
pub fn mark_of(project: &iskra_model::Project, what: iskra_model::Rendered, settings: &Settings) -> Option<iskra_model::RenderMark> {
    let settings = settings.fitted();
    let frames = match what {
        iskra_model::Rendered::Shot(shot) => crate::frames_of(project, shot, settings.range)?,
        iskra_model::Rendered::Film(_) => (0, 0),
    };
    project.render_mark(what, frames, settings.sound && settings.format.is_movie())
}

/// The record of a render that was made with `settings` and came out as `report`, of the
/// project as `mark` has it: taken before the render began, since the project may be another
/// by the time it ends. `pick`, the pass it was, if it was one of a shot's outputs. `None` for
/// a render that wrote nothing.
pub fn record_of(
    what: iskra_model::Rendered,
    settings: &Settings,
    mark: iskra_model::RenderMark,
    report: &crate::Report,
    pick: Option<&crate::Pick>,
) -> Option<iskra_model::RenderRecord> {
    let settings = settings.fitted();
    let at = if settings.format.is_movie() { report.files.first()?.clone() } else { report.folder.clone() };
    report.files.first()?;
    let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let pass = pick.and_then(|pick| pick.named.clone().or_else(|| pick.pass.clone()));
    Some(iskra_model::RenderRecord { what, at, settings: serde_json::to_string(&settings).unwrap_or_default(), mark, when, pass })
}

/// The settings a recorded render was made with, if they can still be read.
pub fn settings_of(record: &iskra_model::RenderRecord) -> Option<Settings> {
    serde_json::from_str(&record.settings).ok()
}

/// Whether what a recorded render shows has changed since: the shot, the project's palette or
/// settings, the frames in the cut, or the sound under them. A record whose shot is gone, or
/// whose settings can't be read, is stale too.
pub fn is_stale(project: &iskra_model::Project, record: &iskra_model::RenderRecord) -> bool {
    settings_of(record).and_then(|settings| mark_of(project, record.what, &settings)) != Some(record.mark)
}
