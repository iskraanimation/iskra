// What every script starts with (plan §9.3): the `iskra` object over the engine's commands, the
// project read from its snapshot as plain objects, dialogs, and printing. `__exec`, `__print`,
// `__inHand`, `__undo`, `__file` and `__dialog` are the runtime's (runtime.rs).
//
// Objects are the project as it was when read: after an edit, `iskra.project` reads it again.
// What an object's own method changes, its name for one, it changes in the object too. Frames
// count from 0, as the commands' do.

function __text(v) {
    if (typeof v === "string") return v;
    try {
        var json = JSON.stringify(v);
        return json === undefined ? String(v) : json;
    } catch (e) {
        return String(v);
    }
}

// The script's last value, for its outcome: JSON where it is one.
function __result(v) {
    return __text(v);
}

// The one of `list` whose id, or else name, is `key`; null if none.
function __find(list, key) {
    if (key instanceof Object && "id" in key) key = key.id;
    for (var i = 0; i < list.length; i++) if (list[i].id === key) return list[i];
    for (var i = 0; i < list.length; i++) if (list[i].name === key) return list[i];
    return null;
}

// The id of what is given as an object of the model, or as its id.
function __id(of) {
    return of instanceof Object ? of.id : of;
}

class Project {
    constructor(p) {
        this.settings = p.settings;
        this.width = p.settings.width;
        this.height = p.settings.height;
        this.frameRate = p.settings.frameRate.num / p.settings.frameRate.den;
        this.palette = new Palette(p.palette);
        this.shots = p.shots.map((s) => new Shot(s, this));
        this.storyboards = p.storyboards.map((b) => new Storyboard(b, this));
        this.media = p.media;
    }
    shot(key) {
        return __find(this.shots, key);
    }
    storyboard(key) {
        return __find(this.storyboards, key);
    }
    // A new shot, with a layer and the camera.
    newShot(name) {
        var made = iskra.run("shot.new", name === undefined ? {} : { name: name });
        return iskra.project.shot(made.id);
    }
}

class Shot {
    constructor(s, project) {
        this.project = project;
        this.id = s.id;
        this.name = s.name;
        this.duration = s.duration;
        this.status = s.status;
        this.camera = s.camera;
        this.pegs = s.pegs.map((p) => new Peg(p, this));
        this.layers = s.layers.map((l) => new Layer(l, this));
        this.composites = s.composites;
        this.groups = s.groups;
        // Its retake notes, in the order of their frames.
        this.notes = (s.notes || []).map((n) => new Note(n, this));
    }
    note(id) {
        for (var n of this.notes) if (n.id === id) return n;
        return null;
    }
    // A retake note on `length` frames (1 if not said) from `frame`, by the user of the window.
    addNote(frame, text, length) {
        var params = { shot: this.id, frame: frame, text: String(text), author: iskra.user };
        if (length !== undefined) params.length = length;
        var made = iskra.run("note.add", params);
        return iskra.project.shot(this.id).note(made.id);
    }
    layer(key) {
        return __find(this.layers, key);
    }
    peg(key) {
        return __find(this.pegs, key);
    }
    rename(name) {
        iskra.run("shot.rename", { shot: this.id, name: name });
        this.name = name;
    }
    setDuration(frames) {
        iskra.run("shot.setDuration", { shot: this.id, frames: frames });
        this.duration = frames;
    }
    // A new layer on top, of `kind` "bitmap" (the default), "vector" or "text".
    newLayer(name, kind) {
        var params = { shot: this.id, kind: kind === undefined ? "bitmap" : kind };
        if (name !== undefined) params.name = name;
        var made = iskra.run("layer.new", params);
        return iskra.project.shot(this.id).layer(made.id);
    }
    // A new peg; `parent`, a peg, holds it.
    newPeg(name, parent) {
        var params = { shot: this.id };
        if (name !== undefined) params.name = name;
        if (parent !== undefined && parent !== null) params.parent = __id(parent);
        var made = iskra.run("peg.new", params);
        return iskra.project.shot(this.id).peg(made.id);
    }
}

class Layer {
    constructor(l, shot) {
        this.shot = shot;
        this.id = l.id;
        this.name = l.name;
        // "animation" or "board".
        this.role = l.role;
        this.density = l.density;
        this.pegId = l.parent;
        this.drawings = l.drawings.map((d) => new Drawing(d, this));
        // Runs of frames, each { drawing, start, len }: the drawing's id, its first frame, how many.
        this.exposure = l.exposure;
        // The art layers that are free colour, and that are vector art.
        this.freeColour = l.freeColour || [];
        this.vector = l.vector || [];
    }
    get peg() {
        return this.pegId === null || this.pegId === undefined ? null : this.shot.peg(this.pegId);
    }
    drawing(key) {
        return __find(this.drawings, key);
    }
    // The drawing exposed on `frame`, or null for an empty cell.
    drawingAt(frame) {
        for (var run of this.exposure) {
            if (frame >= run.start && frame < run.start + run.len) return this.drawing(run.drawing);
        }
        return null;
    }
    rename(name) {
        iskra.run("layer.rename", { layer: this.id, name: name });
        this.name = name;
    }
    // A new empty drawing, exposed from `frame`.
    newDrawing(frame, name) {
        var params = { layer: this.id, frame: frame };
        if (name !== undefined) params.name = name;
        var made = iskra.run("drawing.newDrawing", params);
        return iskra.project.shot(this.shot.id).layer(this.id).drawing(made.id);
    }
    // Expose `drawing` on `length` frames (1 if not said) from `frame`.
    expose(drawing, frame, length) {
        iskra.run("exposure.set", { layer: this.id, frame: frame, length: length === undefined ? 1 : length, drawing: __id(drawing) });
    }
    // Leave `length` frames (1 if not said) from `frame` empty.
    clear(frame, length) {
        iskra.run("exposure.set", { layer: this.id, frame: frame, length: length === undefined ? 1 : length });
    }
    // Hang it from `peg`, or from none with null.
    setParent(peg) {
        iskra.run("layer.setParent", { layer: this.id, parent: peg === null ? null : __id(peg) });
        this.pegId = peg === null ? null : __id(peg);
    }
    delete() {
        iskra.run("layer.delete", { layer: this.id });
    }
}

class Note {
    constructor(n, shot) {
        this.shot = shot;
        this.id = n.id;
        this.frame = n.frame;
        this.length = n.length;
        this.text = n.text;
        this.author = n.author || "";
        // When it was written, as a Date.
        this.written = new Date(n.written * 1000);
        this.done = n.done;
    }
    __set(values) {
        iskra.run("note.set", Object.assign({ shot: this.shot.id, note: this.id }, values));
        Object.assign(this, values);
    }
    setText(text) {
        this.__set({ text: String(text) });
    }
    // Put it on `length` frames (as it was if not said) from `frame`.
    move(frame, length) {
        this.__set(length === undefined ? { frame: frame } : { frame: frame, length: length });
    }
    // Tick it done, or open again.
    close() {
        this.__set({ done: true });
    }
    reopen() {
        this.__set({ done: false });
    }
    remove() {
        iskra.run("note.remove", { shot: this.shot.id, note: this.id });
    }
}

class Drawing {
    constructor(d, layer) {
        this.layer = layer;
        this.id = d.id;
        this.name = d.name;
        // The kinds of its art layers: "lineArt", "colourArt", …
        this.artLayers = d.drawing.artLayers.map((a) => a.kind);
    }
    rename(name) {
        iskra.run("drawing.rename", { layer: this.layer.id, drawing: this.id, name: name });
        this.name = name;
    }
    expose(frame, length) {
        this.layer.expose(this, frame, length);
    }
    // The frames it is exposed on.
    frames() {
        var frames = [];
        for (var run of this.layer.exposure) {
            if (run.drawing !== this.id) continue;
            for (var f = run.start; f < run.start + run.len; f++) frames.push(f);
        }
        return frames;
    }
}

class Peg {
    constructor(p, shot) {
        this.shot = shot;
        this.id = p.id;
        this.name = p.name;
        this.parentId = p.parent;
        // Its values where they are not keyed: position, rotation, scale, skew, pivot, depth, tilt.
        this.transform = p.transform;
        // Each value's keys.
        this.animation = p.animation;
    }
    get parent() {
        return this.parentId === null || this.parentId === undefined ? null : this.shot.peg(this.parentId);
    }
    rename(name) {
        iskra.run("peg.rename", { peg: this.id, name: name });
        this.name = name;
    }
    delete() {
        iskra.run("peg.delete", { peg: this.id });
    }
    // Key the values given, { position: [x, y], rotation, scale: [x, y], skew, depth, tilt }, on `frame`.
    key(frame, values) {
        iskra.run("peg.setTransform", Object.assign({}, values, { peg: this.id, frame: frame, animate: true }));
    }
    // Set the values given: a value with keys is set as a key on `frame`, one with none on every frame.
    set(values, frame) {
        var params = Object.assign({}, values, { peg: this.id });
        if (frame !== undefined) params.frame = frame;
        iskra.run("peg.setTransform", params);
    }
}

class Palette {
    constructor(p) {
        this.swatches = p.swatches.map((s) => new Swatch(s));
        this.sets = p.sets;
    }
    swatch(key) {
        return __find(this.swatches, key);
    }
}

class Swatch {
    constructor(s) {
        this.id = s.id;
        this.name = s.name;
        // [r, g, b, a], 0 to 255.
        this.color = s.color;
        this.kind = s.kind || "solid";
    }
    rename(name) {
        iskra.run("palette.set", { swatch: this.id, name: name });
        this.name = name;
    }
    setColor(color) {
        iskra.run("palette.set", { swatch: this.id, color: color });
        this.color = color;
    }
}

class Storyboard {
    constructor(b, project) {
        this.project = project;
        this.id = b.id;
        this.name = b.name;
        // Each { shot, start, in, out }: the shot's id, where it starts, the frames of it seen.
        this.clips = b.clips;
    }
    // The shots, in the order they are placed.
    get shots() {
        return this.clips.map((c) => this.project.shot(c.shot));
    }
}

// The kinds of a dialog's fields (plan §9.5).
var __kinds = ["text", "number", "check", "choice", "layer", "drawing", "swatch"];

// A dialog as the window draws it: each field with its label, its value, and for those that
// pick one of several, its options, each { value, label } (and a swatch's `color`). Beside it,
// for each field, the objects of the model its options are, to give back.
function __describe(def) {
    if (!(def instanceof Object) || !Array.isArray(def.fields)) throw new Error("a dialog is { title, fields: [ … ] }");
    var described = { title: def.title === undefined ? "Script" : String(def.title), ok: def.ok === undefined ? "OK" : String(def.ok), fields: [] };
    var objects = [];
    var names = [];
    for (var f of def.fields) {
        if (!(f instanceof Object) || typeof f.name !== "string" || f.name === "") throw new Error("each field of a dialog has a `name`");
        if (names.indexOf(f.name) >= 0) throw new Error("the dialog has two fields named `" + f.name + "`");
        names.push(f.name);
        var kind = f.kind === undefined ? "text" : f.kind;
        if (__kinds.indexOf(kind) < 0) throw new Error("the field `" + f.name + "` is of no kind a dialog has: " + __kinds.join(", "));
        var field = { name: f.name, label: f.label === undefined ? f.name : String(f.label), kind: kind };
        var of = null;
        if (kind === "text") field.value = f.value === undefined ? "" : String(f.value);
        if (kind === "check") field.value = !!f.value;
        if (kind === "number") {
            for (var key of ["min", "max", "step"]) if (typeof f[key] === "number") field[key] = f[key];
            field.value = typeof f.value === "number" ? f.value : typeof f.min === "number" ? f.min : 0;
        }
        if (kind === "choice") {
            if (!Array.isArray(f.options) || f.options.length === 0) throw new Error("the choice `" + f.name + "` has no `options`");
            field.options = f.options.map((o) => (o instanceof Object ? { value: o.value, label: String(o.label === undefined ? o.value : o.label) } : { value: o, label: String(o) }));
        }
        if (kind === "layer") {
            var shot = f.shot === undefined ? iskra.shot : iskra.project.shot(__id(f.shot));
            of = shot === null ? [] : shot.layers;
            field.value = f.value !== undefined ? __id(f.value) : iskra.layer !== null ? iskra.layer.id : null;
        }
        if (kind === "drawing") {
            var layer = f.layer === undefined ? iskra.layer : iskra.shot === null ? null : iskra.shot.layer(__id(f.layer));
            of = layer === null ? [] : layer.drawings;
            var at = layer === null ? null : layer.drawingAt(iskra.frame);
            field.value = f.value !== undefined ? __id(f.value) : at !== null ? at.id : null;
        }
        if (kind === "swatch") {
            of = iskra.project.palette.swatches;
            field.value = f.value !== undefined ? __id(f.value) : iskra.swatch !== null ? iskra.swatch.id : null;
        }
        if (of !== null) field.options = of.map((o) => (kind === "swatch" ? { value: o.id, label: o.name, color: o.color } : { value: o.id, label: o.name }));
        if (kind === "choice") field.value = f.value;
        // A value that is none of the options is the first, or null where there are none.
        if (field.options !== undefined && !field.options.some((o) => o.value === field.value)) {
            field.value = field.options.length > 0 ? field.options[0].value : null;
        }
        described.fields.push(field);
        objects.push(of);
    }
    return { described: described, objects: objects };
}

// A field's value as the script is given it, from what was answered: of its kind, one of its
// options, or else as it started; a layer, drawing or swatch as its object.
function __answered(field, of, v) {
    if (v === undefined) v = field.value;
    if (field.kind === "text") return v === null ? "" : String(v);
    if (field.kind === "check") return !!v;
    if (field.kind === "number") {
        var n = Number(v);
        if (!isFinite(n)) n = field.value;
        if (field.min !== undefined) n = Math.max(field.min, n);
        if (field.max !== undefined) n = Math.min(field.max, n);
        return n;
    }
    if (!field.options.some((o) => o.value === v)) v = field.value;
    if (of === null) return v;
    return v === null ? null : __find(of, v);
}

// The commands the script adds, by id, in the order added.
var __commands = [];

// What the script adds, as JSON, for the app to list.
function __added() {
    return __commands.map((c) => ({ id: c.id, title: c.title, menu: c.menu === undefined ? null : c.menu }));
}

// Run the `run` of the command `id` the script added.
function __run(id) {
    for (var c of __commands) if (c.id === id) return c.run();
    throw new Error("the script adds no command `" + id + "`");
}

var iskra = (function () {
    var read = null;
    var hand = JSON.parse(__inHand());
    // What is in hand, as the window has it: by its id, looked up in the project.
    function inShot() {
        return hand.shot === undefined || hand.shot === null ? null : iskra.project.shot(hand.shot);
    }
    return {
        // Run the command `id` with `params` and give back its result.
        run: function (id, params) {
            read = null;
            return JSON.parse(__exec(String(id), JSON.stringify(params === undefined ? null : params)));
        },
        // The project, read from its snapshot.
        get project() {
            if (read === null) read = new Project(JSON.parse(__exec("project.get", "null")));
            return read;
        },
        // What is in hand: the shot, layer and peg being worked on, the frame, the art layer,
        // the tool, the swatch, and what is picked in the layer list (layers and pegs). In a
        // run without a window, what the run was given; null where nothing is.
        get shot() {
            return inShot();
        },
        get layer() {
            var shot = inShot();
            return shot === null || hand.layer === undefined || hand.layer === null ? null : shot.layer(hand.layer);
        },
        get peg() {
            var shot = inShot();
            return shot === null || hand.peg === undefined || hand.peg === null ? null : shot.peg(hand.peg);
        },
        get frame() {
            return hand.frame === undefined ? 0 : hand.frame;
        },
        get artLayer() {
            return hand.artLayer === undefined ? null : hand.artLayer;
        },
        get tool() {
            return hand.tool === undefined ? null : hand.tool;
        },
        get swatch() {
            return hand.swatch === undefined || hand.swatch === null ? null : iskra.project.palette.swatch(hand.swatch);
        },
        // The name of who uses the window, as its settings say; "" where none is.
        get user() {
            return typeof hand.user === "string" ? hand.user : "";
        },
        get picked() {
            var shot = inShot();
            if (shot === null || !Array.isArray(hand.picked)) return [];
            return hand.picked.map((id) => shot.layer(id) || shot.peg(id)).filter((p) => p !== null);
        },
        // Add a command (plan §9.4): { id: "group.name", title, menu, run }. It is in `menu`,
        // a menu of the window's by its title, or else in Scripts; in the command palette and
        // the Shortcuts table; and run by `iskra.run(id)`. A script in the Scripts folder adds
        // its commands as it is read, and edits only when one is run.
        command: function (def) {
            if (!(def instanceof Object) || typeof def.id !== "string" || !/^[A-Za-z0-9_-]+\.[A-Za-z0-9_.-]+$/.test(def.id)) {
                throw new Error("a command's `id` is a group and a name, as \"my.renameDrawings\"");
            }
            if (typeof def.run !== "function") throw new Error("the command `" + def.id + "` has no `run` function");
            if (__commands.some((c) => c.id === def.id)) throw new Error("the command `" + def.id + "` is added twice");
            __commands.push({ id: def.id, title: def.title === undefined ? def.id : String(def.title), menu: def.menu, run: def.run });
        },
        // Make what `fn` does an undo step of its own, named `name`, and give back what it gives.
        undoGroup: function (name, fn) {
            __undo("begin", String(name));
            try {
                return fn();
            } finally {
                __undo("end", String(name));
            }
        },
        // Ask for values (plan §9.5): { title, ok, fields: [{ name, label, kind, value, … }] },
        // each field's `kind` "text", "number" (with `min`, `max`, `step`), "check", "choice"
        // (with `options`, strings or { value, label }), "layer" (of the shot in hand, or
        // `shot`), "drawing" (of the layer in hand, or `layer`) or "swatch". Gives the values
        // by name, a layer, drawing or swatch as its object; null if it is cancelled. The
        // window runs the script again from its start once it is answered, so ask before
        // writing files.
        dialog: function (def) {
            var made = __describe(def);
            var answer = JSON.parse(__dialog(JSON.stringify(made.described)));
            if (answer === null || answer === false) return null;
            var values = {};
            made.described.fields.forEach((field, i) => {
                values[field.name] = __answered(field, made.objects[i], answer instanceof Object ? answer[field.name] : undefined);
            });
            return values;
        },
        // Files: read in the project's folder; written, or read elsewhere, only with Settings ▸
        // Scripting ▸ Let scripts write files on.
        file: {
            read: (path) => JSON.parse(__file("read", String(path))),
            write: (path, text) => {
                __file("write", String(path), String(text));
            },
            append: (path, text) => {
                __file("append", String(path), String(text));
            },
            exists: (path) => JSON.parse(__file("exists", String(path))),
            isFolder: (path) => JSON.parse(__file("isFolder", String(path))),
            list: (path) => JSON.parse(__file("list", String(path))),
            // The folder of the project's file; null for a project not saved yet.
            get projectFolder() {
                return JSON.parse(__file("projectFolder", ""));
            },
        },
        print: function () {
            __print(Array.prototype.map.call(arguments, __text).join(" "));
        },
    };
})();

var print = iskra.print;
var console = { log: iskra.print, info: iskra.print, warn: iskra.print, error: iskra.print };
