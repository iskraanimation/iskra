use iskra_engine::{ScriptRequest, Session};
use serde_json::{Value, json};

use crate::{MAX_NESTING, Outcome, check_nesting};

/// Run `code` as the script `name`, with nothing in hand and no writing.
fn run(s: &mut Session, code: &str, name: &str) -> Outcome {
    crate::run(s, &ScriptRequest { code: code.to_string(), name: name.to_string(), ..Default::default() })
}

/// A session that runs scripts, with a layer of three drawings, exposed on frames 0, 1 and 2.
fn session() -> (Session, u64) {
    let mut s = Session::new();
    crate::install(&mut s);
    let layer = s.execute("layer.new", json!({ "name": "Ball" })).unwrap()["id"].as_u64().unwrap();
    for frame in 0..3 {
        s.execute("drawing.newDrawing", json!({ "layer": layer, "frame": frame })).unwrap();
    }
    (s, layer)
}

fn names(s: &Session, layer: u64) -> Vec<String> {
    let layer = s.project().shots[0].layers.iter().find(|l| l.id.0 == layer).unwrap();
    layer.drawings.iter().map(|d| d.name.clone()).collect()
}

const RENAME: &str = r#"
var project = iskra.run("project.get");
var renamed = 0;
project.shots.forEach(function (shot) {
    shot.layers.forEach(function (layer) {
        layer.drawings.forEach(function (d, i) {
            iskra.run("drawing.rename", { layer: layer.id, drawing: d.id, name: layer.name.toLowerCase() + "-" + (i + 1) });
            renamed++;
        });
    });
});
print("renamed", renamed, "drawings");
renamed
"#;

#[test]
fn a_script_batch_renames_drawings_in_one_undo_step() {
    let (mut s, layer) = session();
    assert_eq!(names(&s, layer), ["1", "2", "3"]);
    let steps = s.history().entries().len();
    let out = run(&mut s, RENAME, "Rename Drawings");
    assert!(out.ok(), "{:?}", out.error);
    assert_eq!(names(&s, layer), ["ball-1", "ball-2", "ball-3"]);
    assert_eq!(out.result, json!(3), "the last value is the outcome's result");
    assert_eq!(out.output, ["renamed 3 drawings"]);
    assert_eq!(s.history().entries().len(), steps + 1, "three renames are one step");
    assert_eq!(s.history().undo_label(), Some("Rename Drawings"), "named after the script");
    s.undo();
    assert_eq!(names(&s, layer), ["1", "2", "3"]);
}

#[test]
fn script_run_is_a_command_once_installed() {
    let (mut s, layer) = session();
    let done = s.execute("script.run", json!({ "code": RENAME, "name": "rename.js" })).unwrap();
    assert_eq!(done, json!({ "result": 3, "output": ["renamed 3 drawings"] }));
    assert_eq!(names(&s, layer), ["ball-1", "ball-2", "ball-3"]);
    assert_eq!(s.history().undo_label(), Some("rename.js"));

    let why = s.execute("script.run", json!({ "code": "nothing.here()", "name": "broken.js" })).unwrap_err().to_string();
    assert_eq!(why, "broken.js:1:1: ReferenceError: nothing is not defined");
    let mut bare = Session::new();
    assert!(bare.execute("script.run", json!({ "code": "1" })).unwrap_err().to_string().contains("no script engine"));
}

#[test]
fn a_script_that_throws_part_way_leaves_nothing_done() {
    let (mut s, layer) = session();
    let steps = s.history().entries().len();
    let code = "var p = iskra.run('project.get');\nprint('before');\niskra.run('drawing.rename', { layer: p.shots[0].layers[0].id, drawing: p.shots[0].layers[0].drawings[0].id, name: 'x' });\nthrow new Error('stop here');\nprint('after');";
    let out = run(&mut s, code, "stops.js");
    let error = out.error.as_ref().unwrap();
    assert_eq!((error.message.as_str(), error.line), ("stop here", Some(4)));
    assert_eq!(out.output, ["before"], "what was printed before it threw is kept");
    assert_eq!(names(&s, layer), ["1", "2", "3"], "the rename it made is taken back");
    assert_eq!(s.history().entries().len(), steps, "and nothing is left in the history");
    assert!(s.history().can_undo(), "what was there before the script is still to undo");
}

#[test]
fn a_command_that_fails_throws_what_the_engine_said() {
    let (mut s, layer) = session();
    let code =
        format!("var said;\ntry {{ iskra.run('drawing.rename', {{ layer: {layer}, drawing: 99999, name: 'x' }}); }} catch (e) {{ said = e.message; }}\nsaid");
    let out = run(&mut s, &code, "catch.js");
    assert!(out.ok(), "{:?}", out.error);
    assert!(out.result.as_str().is_some_and(|said| !said.is_empty()), "caught: {:?}", out.result);

    let out = run(&mut s, "\n\niskra.run('no.suchCommand', {})", "unknown.js");
    let error = out.error.unwrap();
    assert_eq!((error.message.as_str(), error.line), ("unknown command `no.suchCommand`", Some(3)));
}

#[test]
fn a_script_cannot_move_through_the_history_or_replace_the_project() {
    let (mut s, layer) = session();
    for id in ["edit.undo", "edit.redo", "edit.gotoHistory", "file.new", "file.open", "file.save", "script.run"] {
        let out = run(&mut s, &format!("iskra.run('{id}', {{}})"), "x.js");
        assert!(out.error.as_ref().is_some_and(|e| e.message.starts_with(&format!("`{id}`"))), "{id}: {:?}", out.error);
    }
    assert_eq!(names(&s, layer), ["1", "2", "3"]);
    // Reading the history is no move through it.
    let out = run(&mut s, "iskra.run('edit.history').length", "x.js");
    assert_eq!(out.result, json!(s.history().entries().len()));
}

#[test]
fn errors_say_their_line_and_column() {
    let mut s = Session::new();
    let out = run(&mut s, "var a = 1;\nvar b = ;", "syntax.js");
    let error = out.error.unwrap();
    assert_eq!(error.line, Some(2), "{error:?}");
    let out = run(&mut s, "var a = 1;\n  a.b.c = 2;", "type.js");
    let error = out.error.unwrap();
    assert!(error.message.starts_with("TypeError"), "{error:?}");
    assert_eq!(error.line, Some(2), "{error:?}");
    assert_eq!(error.to_string().split(':').next(), Some("type.js"));
}

#[test]
fn values_and_printing() {
    let mut s = Session::new();
    let out = run(&mut s, "print('a', 1, {x: [2]}, null); console.log('two\\nlines'); ({ n: 1 + 1, s: 'x' })", "p.js");
    assert_eq!(out.output, ["a 1 {\"x\":[2]} null", "two", "lines"]);
    assert_eq!(out.result, json!({ "n": 2, "s": "x" }));
    assert_eq!(run(&mut s, "var x = 1;", "p.js").result, Value::Null);
    assert_eq!(run(&mut s, "'text'", "p.js").result, json!("text"));
    // A script that changes nothing makes no step.
    assert!(!s.history().can_undo());
}

#[test]
fn deep_nesting_is_an_error_not_a_stack_overflow() {
    let mut s = Session::new();
    for code in ["(".repeat(100_000) + "1" + &")".repeat(100_000), "{".repeat(MAX_NESTING + 1)] {
        let out = run(&mut s, &code, "deep.js");
        assert!(out.error.is_some_and(|e| e.message.contains("nested too deeply")));
    }
    let quoted = format!("var s = '{}'; // {}\n/* {} */ 1 + 1", "(".repeat(500), "[".repeat(500), "{".repeat(500));
    assert!(check_nesting(&quoted).is_ok());
    assert_eq!(run(&mut s, &quoted, "ok.js").result, json!(2));
}

#[test]
fn a_loop_that_never_ends_stops() {
    let mut s = Session::new();
    let out = run(&mut s, "var i = 0; while (true) { i++; }", "forever.js");
    assert!(out.error.is_some());
}

/// The script `code` run with `in_hand`, and allowed to write files or not.
fn run_with(s: &mut Session, code: &str, in_hand: Value, write_files: bool) -> Outcome {
    crate::run(s, &ScriptRequest { code: code.to_string(), name: "test.js".to_string(), in_hand, write_files, ..Default::default() })
}

fn ok(out: Outcome) -> Value {
    assert!(out.ok(), "{:?} {:?}", out.error, out.output);
    out.result
}

#[test]
fn the_project_is_read_as_objects() {
    let (mut s, layer) = session();
    s.execute("exposure.set", json!({ "layer": layer, "frame": 3, "length": 2 })).unwrap();
    let steps = s.history().entries().len();
    let code = r#"
        var p = iskra.project;
        var shot = p.shots[0];
        var ball = shot.layer("Ball");
        ({
            size: [p.width, p.height, p.frameRate],
            shot: [shot.name, shot.duration, shot.status],
            layers: shot.layers.map((l) => l.name),
            byId: shot.layer(ball.id) === ball,
            drawings: ball.drawings.map((d) => d.name),
            at: [ball.drawingAt(0).name, ball.drawingAt(2).name, ball.drawingAt(3)],
            frames: ball.drawing("2").frames(),
            art: ball.drawings[0].artLayers.length > 0,
            pegs: shot.pegs.map((g) => g.name),
            camera: shot.peg(shot.camera.parent).name,
            red: p.palette.swatch("Red").color,
            boards: p.storyboards.map((b) => b.shots.map((s) => s.name)),
            missing: [p.shot("nope"), shot.layer(-1), ball.drawing("x")],
        })
    "#;
    let read = ok(run(&mut s, code, "read.js"));
    assert_eq!(read["size"], json!([1920, 1080, 24]));
    assert_eq!(read["layers"][0], json!("Ball"));
    assert_eq!(read["byId"], json!(true));
    assert_eq!(read["drawings"], json!(["1", "2", "3"]));
    assert_eq!(read["at"], json!(["1", "3", null]), "frames 3 and 4 were left empty");
    assert_eq!(read["frames"], json!([1]));
    assert_eq!(read["art"], json!(true));
    assert_eq!(read["red"], json!([220, 50, 47, 255]));
    assert_eq!(read["missing"], json!([null, null, null]));
    assert!(read["camera"].is_string(), "{read}");
    assert_eq!(s.history().entries().len(), steps, "reading is no step");
}

#[test]
fn a_shot_is_changed_through_its_objects_in_one_step() {
    let (mut s, layer) = session();
    let steps = s.history().entries().len();
    let code = r#"
        var shot = iskra.project.shots[0];
        var ball = shot.layer("Ball");
        ball.drawings.forEach((d, i) => d.rename("pose-" + (i + 1)));
        ball.rename("Hero");
        var extra = ball.newDrawing(10, "blink");
        extra.expose(11, 2);
        ball.drawing("pose-1").expose(5);
        var peg = shot.newPeg("Hero-P");
        ball.setParent(peg);
        peg.key(0, { position: [0, 0] });
        peg.key(12, { position: [100, 50], rotation: 30 });
        iskra.project.palette.swatch("Red").setColor([255, 0, 0, 255]);
        shot.setDuration(60);
        var again = iskra.project.shots[0].layer("Hero");
        [ball.name, again.name, again.drawingAt(12).name, again.drawingAt(5).name, again.peg.name, extra.layer.name]
    "#;
    let said = ok(run(&mut s, code, "Rig Hero"));
    assert_eq!(said, json!(["Hero", "Hero", "blink", "pose-1", "Hero-P", "Hero"]));
    let shot = &s.project().shots[0];
    let hero = shot.layers.iter().find(|l| l.id.0 == layer).unwrap();
    assert_eq!(hero.name, "Hero");
    assert_eq!(shot.duration, 60);
    let peg = shot.pegs.iter().find(|p| p.name == "Hero-P").unwrap();
    assert_eq!(json!(peg.animation.position).as_array().map(Vec::len), Some(2), "two keys of the position");
    assert_eq!(s.project().palette.swatches.iter().find(|w| w.name == "Red").unwrap().color, [255, 0, 0, 255]);
    assert_eq!(s.history().entries().len(), steps + 1, "all of it one step");
    assert_eq!(s.history().undo_label(), Some("Rig Hero"));
    s.undo();
    assert_eq!(names(&s, layer), ["1", "2", "3"]);
}

#[test]
fn what_is_in_hand_is_what_the_run_was_given() {
    let (mut s, layer) = session();
    let shot = s.project().shots[0].id.0;
    let code = "[iskra.shot && iskra.shot.name, iskra.layer && iskra.layer.name, iskra.frame, iskra.artLayer, iskra.tool, \
                iskra.swatch && iskra.swatch.name, iskra.picked.map((p) => p.name), iskra.peg]";
    let none = ok(run_with(&mut s, code, Value::Null, false));
    assert_eq!(none, json!([null, null, 0, null, null, null, [], null]));
    let hand = json!({ "shot": shot, "layer": layer, "frame": 7, "artLayer": "lineArt", "tool": "brush", "swatch": 3, "picked": [layer, 999] });
    let given = ok(run_with(&mut s, code, hand, false));
    assert_eq!(given, json!(["Shot_01", "Ball", 7, "lineArt", "brush", "Red", ["Ball"], null]));
    // The drawing under the playhead, renamed.
    ok(run_with(&mut s, "iskra.layer.drawingAt(iskra.frame).rename('here')", json!({ "shot": shot, "layer": layer, "frame": 1 }), false));
    assert_eq!(names(&s, layer), ["1", "here", "3"]);
}

#[test]
fn an_undo_group_is_a_step_of_its_own() {
    let (mut s, layer) = session();
    let steps = s.history().entries().len();
    let code = r#"
        var ball = iskra.project.shots[0].layer("Ball");
        ball.drawings[0].rename("a");
        var got = iskra.undoGroup("Rename the Rest", () => {
            ball.drawings[1].rename("b");
            iskra.undoGroup("Inner", () => ball.drawings[2].rename("c"));
            return 5;
        });
        ball.rename("After");
        got
    "#;
    assert_eq!(ok(run(&mut s, code, "Grouped")), json!(5));
    let labels: Vec<String> = s.history().entries().into_iter().skip(steps).map(|e| e.label).collect();
    assert_eq!(labels, ["Grouped", "Rename the Rest", "Grouped"], "the script's, the group's, the script's again");
    s.undo();
    s.undo();
    assert_eq!(names(&s, layer), ["a", "2", "3"], "the step after the group and the group are undone");
    // A script that fails after a group takes the group back too.
    let (mut s, layer) = session();
    let steps = s.history().entries().len();
    let out = run(&mut s, "var b = iskra.project.shots[0].layer('Ball'); iskra.undoGroup('G', () => b.drawings[0].rename('x')); throw 'no';", "fails.js");
    assert_eq!(out.error.unwrap().message, "no");
    assert_eq!((names(&s, layer), s.history().entries().len()), (vec!["1".to_string(), "2".into(), "3".into()], steps));
}

#[test]
fn files_are_read_in_the_projects_folder_and_written_only_when_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let (mut s, _) = session();
    let project = dir.path().join("film.iskra");
    s.execute("file.saveAs", json!({ "path": project })).unwrap();
    std::fs::write(dir.path().join("names.txt"), "a\nb").unwrap();
    std::fs::write(elsewhere.path().join("secret.txt"), "x").unwrap();
    let here = dir.path().join("names.txt").to_string_lossy().into_owned();
    let there = elsewhere.path().join("secret.txt").to_string_lossy().into_owned();
    let out_file = elsewhere.path().join("out.txt").to_string_lossy().into_owned();

    let read = ok(run(&mut s, &format!("[iskra.file.read({here:?}).split('\\n'), iskra.file.exists({there:?}), iskra.file.projectFolder !== null]"), "f.js"));
    assert_eq!(read, json!([["a", "b"], false, true]), "outside the folder is not even seen");
    let refused = run(&mut s, &format!("iskra.file.read({there:?})"), "f.js").error.unwrap().message;
    assert!(refused.contains("only in the project's folder") && refused.contains("Let scripts write files"), "{refused}");
    let refused = run(&mut s, &format!("iskra.file.write({out_file:?}, 'hi')"), "f.js").error.unwrap().message;
    assert!(refused.contains("can't be written"), "{refused}");
    assert!(!elsewhere.path().join("out.txt").exists());

    ok(run_with(
        &mut s,
        &format!("iskra.file.write({out_file:?}, 'hi'); iskra.file.append({out_file:?}, ' there'); iskra.file.read({there:?})"),
        Value::Null,
        true,
    ));
    assert_eq!(std::fs::read_to_string(elsewhere.path().join("out.txt")).unwrap(), "hi there");
    let listed = ok(run_with(&mut s, &format!("iskra.file.list({:?}).length", elsewhere.path().to_string_lossy()), Value::Null, true));
    assert_eq!(listed, json!(2));
}

const ADDS: &str = r#"
iskra.command({
    id: "my.renameDrawings",
    title: "Rename Drawings by Layer",
    menu: "Drawing",
    run: function () {
        var n = 0;
        iskra.project.shots[0].layers.forEach((l) => l.drawings.forEach((d, i) => { d.rename(l.name.toLowerCase() + "-" + (i + 1)); n++; }));
        return n;
    },
});
iskra.command({ id: "my.twice", title: "Twice", run: () => { iskra.run("my.renameDrawings"); return iskra.run("my.shout"); } });
"#;

/// A command a script adds, with the script that adds it.
fn script_command(id: &str, title: &str, code: &str, file: &str, added: bool) -> iskra_engine::ScriptCommand {
    iskra_engine::ScriptCommand { id: id.into(), title: title.into(), menu: None, code: code.into(), file: file.into(), added }
}

#[test]
fn a_scripts_commands_are_read_from_it_without_running_them() {
    let (mut s, layer) = session();
    let steps = s.history().entries().len();
    let added = crate::added_commands(&mut s, ADDS, "rename.js").unwrap();
    let listed: Vec<(&str, &str, Option<&str>)> = added.iter().map(|a| (a.id.as_str(), a.title.as_str(), a.menu.as_deref())).collect();
    assert_eq!(listed, [("my.renameDrawings", "Rename Drawings by Layer", Some("Drawing")), ("my.twice", "Twice", None)]);
    assert_eq!((names(&s, layer), s.history().entries().len()), (vec!["1".to_string(), "2".into(), "3".into()], steps), "nothing ran");

    // A file that edits as it is read is refused; one whose commands are wrong says so.
    let edits = crate::added_commands(&mut s, "iskra.run('layer.new', {});", "edits.js").unwrap_err();
    assert!(edits.message.contains("edits only in their `run`"), "{edits:?}");
    assert_eq!(s.history().entries().len(), steps);
    for (code, says) in [
        ("iskra.command({ id: 'nogroup', run: () => 1 })", "a group and a name"),
        ("iskra.command({ id: 'a.b' })", "has no `run`"),
        ("iskra.command({ id: 'a.b', run: () => 1 }); iskra.command({ id: 'a.b', run: () => 2 })", "added twice"),
    ] {
        let error = crate::added_commands(&mut s, code, "bad.js").unwrap_err();
        assert!(error.message.contains(says), "{code}: {error:?}");
    }
    // Reading is allowed: a script may look at the project to say what it adds.
    assert!(crate::added_commands(&mut s, "var n = iskra.project.shots.length;", "reads.js").unwrap().is_empty());
}

#[test]
fn a_script_command_runs_by_its_id_as_one_step_named_after_it() {
    let (mut s, layer) = session();
    let added = crate::added_commands(&mut s, ADDS, "rename.js").unwrap();
    let mut commands: Vec<_> = added.iter().map(|a| script_command(&a.id, &a.title, ADDS, "rename.js", true)).collect();
    commands.push(script_command("script.shout", "Shout", "print('HEY'); 'hey'", "shout.js", false));
    commands.push(script_command("my.shout", "Shout Too", "iskra.command({ id: 'my.shout', run: () => 'loud' })", "shout2.js", true));
    s.set_script_commands(commands);

    let done = s.execute("my.renameDrawings", Value::Null).unwrap();
    assert_eq!(done["result"], json!(3));
    assert_eq!(names(&s, layer), ["ball-1", "ball-2", "ball-3"]);
    assert_eq!(s.history().undo_label(), Some("Rename Drawings by Layer"));
    s.undo();

    // A whole file as a command; and commands listed with the built-in ones.
    assert_eq!(s.execute("script.shout", Value::Null).unwrap(), json!({ "result": "hey", "output": ["HEY"] }));
    let listed = s.execute("app.commands", Value::Null).unwrap();
    assert!(listed.as_array().unwrap().iter().any(|c| c["id"] == "my.twice" && c["script"] == "rename.js"));

    // From another script, inside its step: one command runs others.
    let steps = s.history().entries().len();
    let out = run(&mut s, "iskra.run('my.twice')", "outer.js");
    assert_eq!(ok(out), json!("loud"));
    assert_eq!(names(&s, layer), ["ball-1", "ball-2", "ball-3"]);
    assert_eq!((s.history().entries().len(), s.history().undo_label()), (steps + 1, Some("outer.js")));

    // A command that runs itself stops, and its error says where.
    s.set_script_commands(vec![script_command("my.loop", "Loop", "iskra.command({ id: 'my.loop', run: () => iskra.run('my.loop') })", "loop.js", true)]);
    let error = s.execute("my.loop", Value::Null).unwrap_err().to_string();
    assert!(error.contains("more than 16 deep"), "{error}");
    let error = run(&mut s, "iskra.command({ id: 'my.a', run: () => 1 }); 1", "x.js");
    assert!(error.ok(), "a plain run may add commands, which go nowhere");
}

/// A script that renames the layer in hand, then asks what to call its drawings.
const ASKS: &str = r#"
    iskra.layer.rename("Asked");
    var got = iskra.dialog({ title: "Rename Drawings", ok: "Rename", fields: [
        { name: "prefix", label: "Prefix", value: "pose" },
        { name: "from", kind: "number", value: 1, min: 1, max: 99 },
        { name: "pad", kind: "check" },
        { name: "case", kind: "choice", options: ["lower", { value: "upper", label: "UPPER" }] },
        { name: "layer", kind: "layer" },
        { name: "drawing", kind: "drawing" },
        { name: "swatch", kind: "swatch" },
    ] });
    if (got === null) { print("cancelled"); "none" } else {
        got.layer.drawings.forEach((d, i) => d.rename(got.prefix + (got.from + i)));
        print("renamed");
        [got.prefix, got.from, got.pad, got.case, got.layer.name, got.drawing.name, got.swatch.name]
    }
"#;

/// [`ASKS`] run with `answers`, on frame 1 of the layer in hand.
fn asks(s: &mut Session, layer: u64, answers: Vec<Value>) -> Outcome {
    let shot = s.project().shots[0].id.0;
    let in_hand = json!({ "shot": shot, "layer": layer, "frame": 1, "swatch": 3 });
    crate::run(s, &ScriptRequest { code: ASKS.to_string(), name: "Ask".to_string(), in_hand, answers, ..Default::default() })
}

#[test]
fn a_dialog_with_no_answer_stops_the_run_and_takes_back_what_it_did() {
    let (mut s, layer) = session();
    let steps = s.history().entries().len();
    let out = asks(&mut s, layer, Vec::new());
    assert!(out.ok() && out.output.is_empty(), "{out:?}");
    assert_eq!(s.history().entries().len(), steps, "the rename before it is taken back");
    assert_eq!(s.project().shots[0].layers.iter().find(|l| l.id.0 == layer).map(|l| l.name.as_str()), Some("Ball"));
    let asked = out.asks.unwrap();
    assert_eq!((&asked["title"], &asked["ok"]), (&json!("Rename Drawings"), &json!("Rename")));
    let fields = asked["fields"].as_array().unwrap();
    let field = |name: &str| fields.iter().find(|f| f["name"] == name).unwrap().clone();
    assert_eq!(field("prefix"), json!({ "name": "prefix", "label": "Prefix", "kind": "text", "value": "pose" }));
    assert_eq!(field("from"), json!({ "name": "from", "label": "from", "kind": "number", "value": 1, "min": 1, "max": 99 }));
    assert_eq!(field("pad")["value"], json!(false));
    assert_eq!(field("case")["options"], json!([{ "value": "lower", "label": "lower" }, { "value": "upper", "label": "UPPER" }]));
    assert_eq!(field("case")["value"], json!("lower"), "the first option where none is given");
    // The layers of the shot in hand as the script left them, the one in hand picked; the
    // drawing on the frame in hand.
    assert_eq!(field("layer")["value"], json!(layer));
    assert!(field("layer")["options"].as_array().unwrap().iter().any(|o| o["label"] == "Asked"));
    let drawings = s.project().shots[0].layers.iter().find(|l| l.id.0 == layer).unwrap().drawings.clone();
    assert_eq!(field("drawing")["value"], json!(drawings[1].id.0));
    assert_eq!(field("drawing")["options"].as_array().unwrap().len(), 3);
    assert_eq!(field("swatch")["value"], json!(3));
    assert_eq!(field("swatch")["options"][2], json!({ "value": 3, "label": "Red", "color": [220, 50, 47, 255] }));
}

#[test]
fn an_answered_dialog_gives_its_values_and_a_cancelled_one_null() {
    let (mut s, layer) = session();
    let steps = s.history().entries().len();
    // A number past its max, a choice that is none of the options and names left out: kept in
    // bounds, or as they started.
    let answer = json!({ "prefix": "walk", "from": 500, "case": "neither", "pad": 1 });
    let out = asks(&mut s, layer, vec![answer]);
    assert_eq!(ok(out.clone()), json!(["walk", 99, true, "lower", "Asked", "walk100", "Red"]));
    assert_eq!(out.output, ["renamed"]);
    assert_eq!(names(&s, layer), ["walk99", "walk100", "walk101"]);
    assert_eq!(s.history().entries().len(), steps + 1, "one step, the rename before the dialog in it");
    assert_eq!(s.history().undo_label(), Some("Ask"));

    let out = asks(&mut s, layer, vec![Value::Null]);
    assert_eq!((ok(out.clone()), out.output), (json!("none"), vec!["cancelled".to_string()]));

    // Two dialogs, answered in the order they are asked; the second with none left stops it.
    let two = "var a = iskra.dialog({ fields: [{ name: 'n', kind: 'number' }] }); \
               var b = iskra.dialog({ title: 'Second', fields: [{ name: 'n', kind: 'number' }] }); a.n + b.n";
    let request = |answers: Vec<Value>| ScriptRequest { code: two.to_string(), name: "two.js".to_string(), answers, ..Default::default() };
    assert_eq!(crate::run(&mut s, &request(vec![json!({ "n": 2 })])).asks.map(|d| d["title"].clone()), Some(json!("Second")));
    assert_eq!(ok(crate::run(&mut s, &request(vec![json!({ "n": 2 }), json!({ "n": 3 })]))), json!(5));
}

#[test]
fn a_dialog_is_asked_even_where_the_stop_is_caught_but_not_after_a_file_is_written() {
    let (mut s, _) = session();
    let caught = "try { iskra.dialog({ fields: [] }) } catch (e) {} iskra.run('layer.new', { name: 'After' }); print('went on')";
    let out = run(&mut s, caught, "caught.js");
    assert!(out.asks.is_some() && out.output.is_empty(), "{out:?}");
    assert!(!s.project().shots[0].layers.iter().any(|l| l.name == "After"), "it can't go on editing");

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out.txt");
    let code = format!("iskra.file.append({:?}, 'x'); iskra.dialog({{ fields: [] }})", file.to_string_lossy());
    let out = run_with(&mut s, &code, Value::Null, true);
    let error = out.error.map(|e| e.message).unwrap_or_default();
    assert!(error.contains("after a file was written"), "{error}");

    let bad = run(&mut s, "iskra.dialog({ fields: [{ name: 'a' }, { name: 'a' }] })", "bad.js");
    assert!(bad.error.is_some_and(|e| e.message.contains("two fields named `a`")));
    let bad = run(&mut s, "iskra.dialog({ fields: [{ name: 'a', kind: 'colour' }] })", "bad.js");
    assert!(bad.error.is_some_and(|e| e.message.contains("no kind a dialog has")));
}

#[test]
fn script_run_takes_the_answers_and_says_a_dialog_it_has_none_for() {
    let (mut s, _) = session();
    let code = "iskra.dialog({ title: 'Name', fields: [{ name: 'name', value: 'x' }] }).name";
    let done = s.execute("script.run", json!({ "code": code, "answers": [{ "name": "given" }] })).unwrap();
    assert_eq!(done["result"], json!("given"));
    let done = s.execute("script.run", json!({ "code": code, "answers": [{}] })).unwrap();
    assert_eq!(done["result"], json!("x"), "an answer without a name keeps its value");
    let why = s.execute("script.run", json!({ "code": code })).unwrap_err().to_string();
    assert!(why.contains("asks for a dialog, \"Name\""), "{why}");
}

#[test]
fn a_script_writes_lists_and_closes_retake_notes() {
    let (mut s, _) = session();
    let shot = s.project().shots[0].id.0;
    s.execute("shot.setDuration", json!({ "shot": shot, "frames": 48 })).unwrap();
    let code = r#"
        var shot = iskra.shot;
        var note = shot.addNote(42, "The arm pops");
        shot.addNote(3, "Too slow", 4).close();
        note.setText("The arm pops here");
        var read = iskra.project.shot(shot.id);
        [read.notes.map((n) => [n.frame, n.length, n.text, n.author, n.done]), read.note(note.id).written instanceof Date, iskra.user]
    "#;
    let out = run_with(&mut s, code, json!({ "shot": shot, "user": "Ana" }), false);
    assert_eq!(ok(out), json!([[[3, 4, "Too slow", "Ana", true], [42, 1, "The arm pops here", "Ana", false]], true, "Ana"]));
    assert_eq!(s.history().undo_label(), Some("test.js"), "one step");
    ok(run_with(&mut s, "iskra.shot.notes.forEach((n) => n.remove()); iskra.shot.notes.length", json!({ "shot": shot }), false));
    assert!(s.project().shots[0].notes.is_empty());
}

#[test]
fn a_script_drives_a_wheel_with_the_driver_commands_and_bakes_it() {
    let mut s = Session::new();
    crate::install(&mut s);
    let code = r#"
        var car = iskra.run("peg.new", { name: "Car-P" }).id;
        var wheel = iskra.run("peg.new", { name: "Wheel-P", parent: car }).id;
        iskra.run("peg.setTransform", { peg: car, frame: 0, position: [0, 0], animate: true });
        iskra.run("peg.setTransform", { peg: car, frame: 10, position: [100, 0], animate: true });
        var driver = iskra.run("driver.new", {
            peg: wheel,
            expressions: "rotation = parent.travelled / radius * 180 / PI",
            parameters: [{ name: "radius", value: 50 }]
        }).id;
        iskra.run("driver.bake", { driver: driver }).frames;
    "#;
    let outcome = run(&mut s, code, "wheel.js");
    assert!(outcome.ok(), "{:?}", outcome.error);
    assert_eq!(outcome.result, json!(48), "the whole shot is baked");
    let shot = &s.project().shots[0];
    assert!(shot.drivers.is_empty());
    let wheel = shot.pegs.iter().find(|peg| peg.name == "Wheel-P").unwrap();
    assert!((wheel.transform_at(10).rotation - 2f64.to_degrees()).abs() < 1e-9);
    // The whole run is one step.
    s.undo();
    assert!(s.project().shots[0].pegs.iter().all(|peg| peg.name != "Wheel-P"));
}
