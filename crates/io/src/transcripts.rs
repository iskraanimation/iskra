//! Transcripts read from files (plan §4.7): what is spoken in a sound, with when. Subtitle
//! files, SubRip (`.srt`) and WebVTT (`.vtt`), whose cues are a few words each, and a plain
//! list of words as JSON (`.json`), each with its `text` and its `start` and `end` in seconds.

use std::path::Path;

use iskra_model::Spoken;

/// The extensions of the files a transcript is read from.
pub const TRANSCRIPT_EXTENSIONS: [&str; 3] = ["srt", "vtt", "json"];

/// A time as a subtitle file writes it, `01:02:03,450`, `02:03.450` or `3.45`, in thousandths
/// of a second.
fn time(text: &str) -> Option<u32> {
    let mut parts = text.trim().rsplit(':');
    let seconds: f64 = parts.next()?.replace(',', ".").parse().ok()?;
    let minutes: f64 = parts.next().map_or(Some(0.0), |part| part.trim().parse().ok())?;
    let hours: f64 = parts.next().map_or(Some(0.0), |part| part.trim().parse().ok())?;
    let all = (hours * 3600.0 + minutes * 60.0 + seconds) * 1000.0;
    (all.is_finite() && all >= 0.0 && parts.next().is_none()).then_some(all.round().min(f64::from(u32::MAX)) as u32)
}

/// A cue's words without what marks them up: `<i>`, `<v Anna>`, `{\an8}`.
fn plain(text: &str) -> String {
    let (mut out, mut inside) = (String::new(), 0u32);
    for c in text.chars() {
        match c {
            '<' | '{' => inside += 1,
            '>' | '}' => inside = inside.saturating_sub(1),
            c if inside == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The cues of a SubRip or WebVTT file: each a line of two times with `-->` between them,
/// and the lines under it up to an empty one.
pub fn transcript_of_subtitles(text: &str) -> Vec<Spoken> {
    let mut spoken = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some((start, end)) = line.split_once("-->") else { continue };
        // What follows the second time on its line places the cue on the screen.
        let (Some(start), Some(end)) = (time(start), end.split_whitespace().next().and_then(time)) else { continue };
        let words: Vec<&str> = lines.by_ref().take_while(|line| !line.trim().is_empty()).collect();
        let text = plain(&words.join(" "));
        if !text.is_empty() {
            spoken.push(Spoken { text, start, end: end.max(start) });
        }
    }
    spoken.sort_by_key(|spoken| spoken.start);
    spoken
}

/// The words of a JSON list: each an object with `text`, and `start` and `end` in seconds.
pub fn transcript_of_json(text: &str) -> Result<Vec<Spoken>, String> {
    let words: Vec<serde_json::Value> = serde_json::from_str(text).map_err(|_| "not a list of words, each with `text`, `start` and `end`".to_string())?;
    let mut spoken = Vec::new();
    for word in &words {
        let (Some(text), Some(start)) = (word["text"].as_str(), word["start"].as_f64()) else {
            return Err("a word has no `text` or no `start`".to_string());
        };
        let ms = |seconds: f64| (seconds.max(0.0) * 1000.0).round().min(f64::from(u32::MAX)) as u32;
        let (start, text) = (ms(start), text.split_whitespace().collect::<Vec<_>>().join(" "));
        if !text.is_empty() {
            spoken.push(Spoken { text, start, end: word["end"].as_f64().map_or(start, ms).max(start) });
        }
    }
    spoken.sort_by_key(|spoken| spoken.start);
    Ok(spoken)
}

/// Read a transcript from the file at `path`, by its extension.
pub fn read_transcript(path: &Path) -> Result<Vec<Spoken>, String> {
    let extension = path.extension().map(|extension| extension.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !TRANSCRIPT_EXTENSIONS.contains(&extension.as_str()) {
        return Err(format!("{}: a transcript is read from a .srt, .vtt or .json file", path.display()));
    }
    let text = std::fs::read_to_string(path).map_err(|why| format!("{}: {why}", path.display()))?;
    let spoken =
        if extension == "json" { transcript_of_json(&text).map_err(|why| format!("{}: {why}", path.display()))? } else { transcript_of_subtitles(&text) };
    if spoken.is_empty() {
        return Err(format!("{}: nothing spoken was found in it", path.display()));
    }
    Ok(spoken)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cues_and_words_are_read_with_their_times() {
        let srt = "1\n00:00:01,000 --> 00:00:02,500\n<i>Hello?</i>\nIs anyone there?\n\n2\n00:01:03,250 --> 00:01:04,000 align:start\n{\\an8}No.\n";
        let said = |spoken: &[Spoken]| spoken.iter().map(|spoken| (spoken.text.clone(), spoken.start, spoken.end)).collect::<Vec<_>>();
        assert_eq!(said(&transcript_of_subtitles(srt)), [("Hello? Is anyone there?".to_string(), 1000, 2500), ("No.".to_string(), 63_250, 64_000)]);
        // WebVTT: a header, times with a full stop, and hours left out.
        let vtt = "WEBVTT\n\nNOTE made by hand\n\n00:01.500 --> 00:02.000\n<v Anna>Yes\n";
        assert_eq!(said(&transcript_of_subtitles(vtt)), [("Yes".to_string(), 1500, 2000)]);
        // A list of words, in seconds, in any order.
        let json = r#"[{ "text": "there", "start": 1.25, "end": 1.5 }, { "text": " Hello ", "start": 0.5 }]"#;
        assert_eq!(said(&transcript_of_json(json).unwrap()), [("Hello".to_string(), 500, 500), ("there".to_string(), 1250, 1500)]);
        assert!(transcript_of_json("{}").is_err() && transcript_of_json(r#"[{ "text": "x" }]"#).is_err());
        assert_eq!((time("1:02:03,5"), time("x"), time("1:2:3:4")), (Some(3_723_500), None, None));
    }
}
