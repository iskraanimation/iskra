//! Render presets (plan §7.14): settings under a name, to render alike again, and to hand to
//! someone else as a file so that they render alike too.

use serde::{Deserialize, Serialize};

use crate::Settings;

/// The ending of a preset file's name.
pub const PRESET_EXTENSION: &str = "iskrarender";

/// The version of the preset file that is written.
const VERSION: u32 = 1;

/// Render settings under a name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub name: String,
    pub settings: Settings,
    /// The storyboard as one movie, not a file a shot.
    #[serde(default)]
    pub film: bool,
    /// The shot in the viewer alone, not a file for each of the shots.
    #[serde(default)]
    pub this_shot: bool,
    /// How its files are named. None in a file of before: the naming in hand stays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub naming: Option<crate::FileNaming>,
    /// The passes it renders. None in a file of before: they stay as they are switched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passes: Option<PresetPasses>,
}

/// The passes a render preset renders (plan §7.14): those switched on, by their names, and
/// whether the whole picture, Final, is left out beside them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PresetPasses {
    pub on: Vec<String>,
    pub without_final: bool,
}

/// A preset as its file holds it.
#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    #[serde(flatten)]
    preset: Preset,
}

impl Preset {
    /// The preset as the text of a preset file.
    pub fn to_json(&self) -> String {
        // A preset is plain numbers and words: it can always be written.
        serde_json::to_string_pretty(&File { version: VERSION, preset: self.clone() }).unwrap_or_default()
    }

    /// The preset in the text of a preset file. Settings a later version added are ignored,
    /// and those this file lacks have their defaults; a file of a later version that this one
    /// can't read, or one that is no preset, is said to be so.
    pub fn from_json(text: &str) -> Result<Preset, String> {
        let file: File = serde_json::from_str(text).map_err(|_| "not a file of an Iskra render preset".to_string())?;
        if file.preset.name.trim().is_empty() {
            return Err("the preset has no name".to_string());
        }
        Ok(Preset { settings: file.preset.settings.fitted(), ..file.preset })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Format, Range};

    #[test]
    fn a_preset_goes_to_a_file_and_comes_back() {
        let settings = Settings { format: Format::Tiff, bits: 16, scale: 0.5, range: Range::Cut { handles: 8 }, ..Settings::default() };
        let naming = crate::FileNaming { from: crate::NameFrom::Parts, parts: [true, false, true], digits: 6, ..crate::FileNaming::default() };
        let passes = PresetPasses { on: vec!["Character".to_string()], without_final: true };
        let preset = Preset { name: "Comp plates".to_string(), settings, film: false, this_shot: true, naming: Some(naming), passes: Some(passes) };
        let text = preset.to_json();
        assert!(text.contains("\"version\": 1") && text.contains("\"format\": \"tiff\"") && text.contains("\"handles\": 8"), "{text}");
        assert_eq!(Preset::from_json(&text), Ok(preset));
        // A file with less in it has the defaults for the rest; one that asks for what its
        // format can't hold is fitted to it.
        let short = Preset::from_json(r#"{ "version": 1, "name": "Proxy", "settings": { "format": "jpeg", "bits": 16, "alpha": true } }"#).unwrap();
        assert_eq!((short.settings.format, short.settings.bits, short.settings.alpha, short.settings.scale, short.film), (Format::Jpeg, 8, false, 1.0, false));
        assert_eq!((short.naming, short.passes), (None, None), "a file of before leaves the naming and the passes as they are");
        assert!(Preset::from_json("{ \"brushes\": [] }").is_err() && Preset::from_json("not json").is_err());
        assert!(Preset::from_json(r#"{ "version": 1, "name": " ", "settings": {} }"#).is_err());
    }
}
