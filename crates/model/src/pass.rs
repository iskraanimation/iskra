//! Passes (plan §7.14): the parts of a shot that are rendered apart, its backgrounds, its
//! characters, its effects. A pass is a node of its own, a *Pass*, wired from any node of the
//! shot's graph: what flows into it is the pass, effects and all, and the shot's picture is as
//! it was. Passes are matched across shots by their names, so that *Character* is one output
//! of every shot that has it; a shot that has none is rendered whole.
//!
//! A Pass is kept among the shot's displays, marked as a pass ([`Display::pass`]): it is wired,
//! named, placed, grouped and copied as a display is. It is not one to the eye: the viewer's
//! choice of display leaves passes out, and the Export page lists passes alone.

use crate::{Display, LayerId, Shot, Source};

impl Shot {
    /// The shot's passes, in the order they were made.
    pub fn passes(&self) -> impl Iterator<Item = &Display> {
        self.displays.iter().filter(|display| display.pass)
    }

    /// The shot's displays that are not passes, beside its main one.
    pub fn looks(&self) -> impl Iterator<Item = &Display> {
        self.displays.iter().filter(|display| !display.pass)
    }

    /// The pass named `name`, if the shot has one.
    pub fn pass_named(&self, name: &str) -> Option<&Display> {
        self.passes().find(|pass| pass.name == name)
    }

    /// The names of the passes a layer flows into, by any wire: straight, through the
    /// composites that stack it, or the effects that take its picture.
    pub fn passes_fed_by(&self, layer: LayerId) -> Vec<&str> {
        let fed = |shows: Source| Source::outputs(layer).any(|output| self.flows_into(output, shows));
        self.passes().filter(|pass| pass.shows.is_some_and(fed)).map(|pass| pass.name.as_str()).collect()
    }

    /// The shot as its pass named `name` sees it: what flows into the pass, as what the shot
    /// shows. `None` where the shot has no such pass.
    pub fn seen_as_pass(&self, name: &str) -> Option<Shot> {
        let shows = self.pass_named(name)?.shows;
        Some(Shot { display: shows, ..self.clone() })
    }
}

#[cfg(test)]
mod tests {
    use crate::{Display, DisplayId, Project, Source};

    #[test]
    fn a_pass_shows_what_flows_into_it_and_is_no_display_to_look_through() {
        let mut project = Project::default();
        let mut shot = (*project.shots[0]).clone();
        let layer = Source::Layer(shot.layers[0].id);
        let (pass, look) = (DisplayId(project.new_id()), DisplayId(project.new_id()));
        shot.displays.push(Display { id: pass, name: "Character".to_string(), shows: Some(layer), pass: true });
        shot.displays.push(Display { id: look, name: "Matte".to_string(), shows: Some(layer), pass: false });
        assert_eq!(shot.passes().map(|pass| pass.id).collect::<Vec<_>>(), [pass]);
        assert_eq!(shot.looks().map(|look| look.id).collect::<Vec<_>>(), [look]);
        let seen = shot.seen_as_pass("Character").expect("the shot has it");
        assert_eq!((seen.display, shot.display != Some(layer)), (Some(layer), true), "the shot's own picture is as it was");
        assert!(shot.seen_as_pass("Matte").is_none() && shot.seen_as_pass("Background").is_none());
        // A pass is never the main display.
        assert!(!shot.make_main(pass));
        // A layer feeds the passes it flows into, by any wire; the shot's main composite stacks
        // every layer, so a pass of it is fed by all.
        let main = Source::Composite(shot.main_composite().expect("a shot stacks its layers"));
        let all = DisplayId(project.new_id());
        shot.displays.push(Display { id: all, name: "Everything".to_string(), shows: Some(main), pass: true });
        assert_eq!(shot.passes_fed_by(shot.layers[0].id), ["Character", "Everything"]);
        assert!(shot.passes_fed_by(crate::LayerId(999)).is_empty());
    }
}
