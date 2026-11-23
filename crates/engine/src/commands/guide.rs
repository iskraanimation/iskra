//! A shot's drawing guides (plan §5, M7): rulers and perspective. The engine keeps them; the
//! window holds a stroke to the one in hand.

use std::sync::Arc;

use iskra_model::{DrawGuide, GuideId, GuideKind, Project, Shot};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{invalid, parse, shot_id, title};
use crate::{Result, Session};

/// The shot that has the guide, to change.
fn shot_of(project: &mut Project, guide: u64) -> Result<&mut Shot> {
    match project.shots.iter_mut().find(|shot| shot.guides.iter().any(|g| g.id == GuideId(guide))) {
        Some(shot) => Ok(Arc::make_mut(shot)),
        None => invalid(format!("no drawing guide {guide}")),
    }
}

/// Adds a drawing guide to a shot: a `ruler`, or `perspective` with one to three vanishing
/// `points`, placed for the shot's picture. Without a name it is named after its kind, with a
/// number where the shot has one of the name.
pub(super) fn add(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    enum Kind {
        Ruler,
        Perspective,
    }
    #[derive(Deserialize)]
    struct Params {
        shot: Option<u64>,
        kind: Kind,
        #[serde(default = "one")]
        points: usize,
        name: Option<String>,
    }
    fn one() -> usize {
        1
    }
    let id = "guide.add";
    let Params { shot, kind, points, name } = parse(id, params)?;
    if !(1..=3).contains(&points) {
        return invalid("perspective has one, two or three vanishing points");
    }
    let guide = s.edit(title(id), |p| {
        let shot = shot_id(p, shot)?;
        let kind = match kind {
            Kind::Ruler => GuideKind::ruler(),
            Kind::Perspective => GuideKind::perspective(points, f64::from(p.settings.width), f64::from(p.settings.height)),
        };
        let id = GuideId(p.new_id());
        let Some(shot) = p.shot_mut(shot) else { return invalid("no such shot") };
        let name = match name.as_deref().map(str::trim) {
            Some("") => return invalid("a name can't be empty"),
            Some(name) => name.to_string(),
            None => {
                let free = |name: &String| shot.guides.iter().all(|guide| guide.name != *name);
                std::iter::once(kind.title().to_string()).chain((2..).map(|n| format!("{} {n}", kind.title()))).find(free).expect("a free name")
            }
        };
        shot.guides.push(DrawGuide { id, name, kind });
        Ok(id)
    })?;
    Ok(json!({ "guide": guide }))
}

/// Changes a drawing guide: its name; a ruler's place (`at`) or `angle` in degrees; or the
/// vanishing `points` of perspective, as many as it has.
pub(super) fn set(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        guide: u64,
        name: Option<String>,
        at: Option<[f64; 2]>,
        angle: Option<f64>,
        points: Option<Vec<[f64; 2]>>,
    }
    let id = "guide.set";
    let Params { guide, name, at, angle, points } = parse(id, params)?;
    s.edit(title(id), |p| {
        let shot = shot_of(p, guide)?;
        let Some(found) = shot.guides.iter_mut().find(|g| g.id == GuideId(guide)) else { return invalid("no such guide") };
        if let Some(name) = name.as_deref().map(str::trim) {
            if name.is_empty() {
                return invalid("a name can't be empty");
            }
            found.name = name.to_string();
        }
        let kind = match (&found.kind, at, angle, points) {
            (kind, None, None, None) => kind.clone(),
            (GuideKind::Ruler { at: was, angle: turned }, at, angle, None) => GuideKind::Ruler { at: at.unwrap_or(*was), angle: angle.unwrap_or(*turned) },
            (GuideKind::Perspective { points: were }, None, None, Some(points)) if points.len() == were.len() => GuideKind::Perspective { points },
            (GuideKind::Ruler { .. }, ..) => return invalid("a ruler has a place and an angle, and no vanishing points"),
            (GuideKind::Perspective { points, .. }, ..) => {
                return invalid(format!("this perspective has {} vanishing points, and nothing else to set", points.len()));
            }
        };
        if !kind.is_sound() {
            return invalid("a guide's places and angle are finite numbers");
        }
        found.kind = kind;
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Removes a drawing guide from its shot.
pub(super) fn remove(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        guide: u64,
    }
    let id = "guide.remove";
    let Params { guide } = parse(id, params)?;
    s.edit(title(id), |p| {
        shot_of(p, guide)?.guides.retain(|g| g.id != GuideId(guide));
        Ok(())
    })?;
    Ok(Value::Null)
}
