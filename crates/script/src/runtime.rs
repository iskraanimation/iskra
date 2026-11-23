//! The boa runtime: the natives the prelude is written over (`__exec`, `__print`, `__inHand`,
//! `__undo`, `__file`, `__dialog`), the session handed to them while a script runs, its undo steps, and its
//! errors placed at their lines.

use std::cell::RefCell;

use boa_engine::{Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction, Source, js_string};
use iskra_engine::{HistoryMark, ScriptRequest, Session};
use serde_json::Value;

use crate::{Outcome, ScriptError};

const PRELUDE: &str = include_str!("prelude.js");

/// What a script can't run: another script, and what is no edit of the project but of the
/// session itself, which would leave the run's undo step with nothing to stand on.
fn forbidden(id: &str) -> Option<&'static str> {
    match id {
        "script.run" => Some("scripts can't run scripts"),
        "edit.undo" | "edit.redo" | "edit.gotoHistory" => Some("a script can't move through the history: its run is one step, undone after it"),
        _ if id.starts_with("file.") => Some("a script can't open, save or replace the project"),
        _ => None,
    }
}

/// The session and what is printed, while a script runs: the natives reach the session here,
/// so nothing borrowed is ever in the script's heap.
struct Active {
    session: Session,
    output: Vec<String>,
    /// What the script was given as in hand, as JSON.
    in_hand: String,
    write_files: bool,
    /// The script's name, which its own undo steps have.
    name: String,
    /// Where the history was when the run began, to take it all back to if the script fails.
    first: HistoryMark,
    /// Where the step being made began: the script's own, or an `undoGroup`'s.
    open: HistoryMark,
    /// How many `undoGroup`s the script is inside; only the outermost makes a step.
    groups: u32,
    /// The script's file is being read for the commands it adds, which edits nothing.
    adding: bool,
    /// How many script commands are run inside one another.
    depth: u32,
    /// The answers to the run's dialogs, and how many of them were taken.
    answers: Vec<Value>,
    answered: usize,
    /// The dialog asked that had no answer: the run stops at it.
    asks: Option<Value>,
    /// A file was written: a dialog asked after it would have it written again.
    wrote: bool,
}

impl Active {
    fn new(session: Session, name: &str, adding: bool) -> Self {
        let first = session.mark();
        Active {
            session,
            output: Vec::new(),
            in_hand: "{}".to_string(),
            write_files: false,
            name: name.to_string(),
            first,
            open: first,
            groups: 0,
            adding,
            depth: 0,
            answers: Vec::new(),
            answered: 0,
            asks: None,
            wrote: false,
        }
    }
}

/// What stops a script that asked a dialog and goes on, as one catching the stop would.
const ASKED: &str = "the script waits for its dialog";

/// What a script may run while its file is read for its commands: what only reads.
const READS: [&str; 4] = ["project.get", "app.commands", "edit.history", "palette.usage"];

/// How deep script commands may run one another.
const DEEPEST: u32 = 16;

thread_local! {
    static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
}

fn with_active<T>(f: impl FnOnce(&mut Active) -> Result<T, String>) -> Result<T, String> {
    ACTIVE.with(|a| match a.try_borrow_mut() {
        Ok(mut active) => match active.as_mut() {
            Some(active) => f(active),
            None => Err("no script is running".into()),
        },
        Err(_) => Err("scripts can't run scripts".into()),
    })
}

fn throw(message: impl Into<String>) -> boa_engine::JsError {
    JsNativeError::error().with_message(message.into()).into()
}

fn arg_string(args: &[JsValue], i: usize, ctx: &mut Context) -> JsResult<String> {
    Ok(args.get(i).cloned().unwrap_or_default().to_string(ctx)?.to_std_string_escaped())
}

fn js_str(s: &str) -> JsValue {
    JsValue::from(JsString::from(s))
}

/// `__exec(id, paramsJson)` → the result as JSON: runs a command of the session.
fn native_exec(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = arg_string(args, 0, ctx)?;
    let params = arg_string(args, 1, ctx)?;
    if let Some(why) = forbidden(&id) {
        return Err(throw(format!("`{id}`: {why}")));
    }
    let params: Value = serde_json::from_str(&params).map_err(|e| throw(format!("`{id}`: the parameters are not JSON: {e}")))?;
    let (adding, asked) = with_active(|a| Ok((a.adding, a.asks.is_some()))).map_err(throw)?;
    if asked {
        return Err(throw(ASKED));
    }
    if adding && !READS.contains(&id.as_str()) {
        return Err(throw(format!("`{id}`: a script that adds commands edits only in their `run`, not as its file is read")));
    }
    // Another script's command runs here, inside this run and its step.
    let script = with_active(|a| Ok(a.session.script_commands().iter().find(|c| c.id == id).cloned())).map_err(throw)?;
    if let Some(command) = script.filter(|_| !iskra_engine::commands().iter().any(|c| c.id == id)) {
        return nested(&command, ctx);
    }
    let result = with_active(|a| a.session.execute(&id, params).map_err(|e| e.to_string()));
    match result {
        Ok(v) => Ok(js_str(&v.to_string())),
        Err(e) => Err(throw(e)),
    }
}

/// Run a script's command from inside a script: its file in a context of its own, on the same
/// session, in the same step.
fn nested(command: &iskra_engine::ScriptCommand, _: &mut Context) -> JsResult<JsValue> {
    let depth = with_active(|a| {
        a.depth += 1;
        Ok(a.depth)
    })
    .map_err(throw)?;
    let result = if depth > DEEPEST {
        Err(ScriptError { message: format!("script commands run one another more than {DEEPEST} deep"), file: command.file.clone(), ..Default::default() })
    } else {
        let request = command.request();
        new_context()
            .map_err(|message| ScriptError { message, file: command.file.clone(), ..Default::default() })
            .and_then(|mut ctx| eval_request(&mut ctx, &request.code, &command.file, request.command.as_deref()))
    };
    let _ = with_active(|a| {
        a.depth -= 1;
        Ok(())
    });
    match result {
        Ok(v) => Ok(js_str(&v.to_string())),
        Err(e) => Err(throw(format!("`{}`: {e}", command.id))),
    }
}

/// `__inHand()` → what the script was given as in hand, as JSON.
fn native_in_hand(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    with_active(|a| Ok(js_str(&a.in_hand))).map_err(throw)
}

/// `__undo("begin", name)` and `__undo("end", name)`, round an `iskra.undoGroup`: what the
/// script did before it is a step of the script's, what is done within it a step of its own.
fn native_undo(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let op = arg_string(args, 0, ctx)?;
    let name = arg_string(args, 1, ctx)?;
    with_active(|a| {
        match op.as_str() {
            "begin" => {
                a.groups += 1;
                if a.groups == 1 {
                    a.session.fold(a.open, &a.name, true);
                    a.open = a.session.mark();
                }
            }
            _ => {
                a.groups = a.groups.saturating_sub(1);
                if a.groups == 0 {
                    a.session.fold(a.open, &name, true);
                    a.open = a.session.mark();
                }
            }
        }
        Ok(())
    })
    .map_err(throw)?;
    Ok(JsValue::undefined())
}

/// The folder of the project's file: what a script reads in without the preference.
fn project_dir(s: &Session) -> Option<std::path::PathBuf> {
    s.path()?.parent().map(std::path::Path::to_path_buf)
}

/// `a/./b/../c` as `a/c`, made absolute; where the file or its folder is there, its links are
/// followed, so that a link can't lead out of the project's folder.
fn normalize(p: &str) -> std::path::PathBuf {
    use std::path::{Component, Path, PathBuf};
    let path = Path::new(p);
    let abs = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    std::fs::canonicalize(&out)
        .or_else(|_| out.parent().map(std::fs::canonicalize).unwrap_or(Ok(out.clone())).map(|d| d.join(out.file_name().unwrap_or_default())))
        .unwrap_or(out)
}

/// The preference that lets scripts write, as the Settings window names it.
pub(crate) const GATE: &str = "Settings ▸ Scripting ▸ Let scripts write files";

/// Where a script may go: it reads in the project's folder, and writes, or reads elsewhere,
/// only with the preference.
fn check_access(a: &Active, path: &str, write: bool) -> Result<std::path::PathBuf, String> {
    let p = normalize(path);
    if a.write_files {
        return Ok(p);
    }
    if write {
        return Err(format!("{path} can't be written: scripts write files only with {GATE} on"));
    }
    if let Some(dir) = project_dir(&a.session)
        && p.starts_with(normalize(&dir.to_string_lossy()))
    {
        return Ok(p);
    }
    Err(format!("{path} can't be read: scripts read only in the project's folder, unless {GATE} is on"))
}

/// `__file(op, path, text)`: `read` → the text, `write` and `append` → nothing, `exists` and
/// `isFolder` → whether, `list` → the paths in a folder, `projectFolder` → the project's.
fn native_file(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let op = arg_string(args, 0, ctx)?;
    let path = arg_string(args, 1, ctx)?;
    let text = if args.len() > 2 { arg_string(args, 2, ctx)? } else { String::new() };
    let done: Result<Value, String> = with_active(|a| match op.as_str() {
        "projectFolder" => Ok(project_dir(&a.session).map_or(Value::Null, |dir| Value::String(dir.to_string_lossy().into_owned()))),
        "exists" | "isFolder" => {
            // Where it may not look, it finds nothing.
            let Ok(p) = check_access(a, &path, false) else { return Ok(Value::Bool(false)) };
            Ok(Value::Bool(if op == "exists" { p.exists() } else { p.is_dir() }))
        }
        "read" => {
            let p = check_access(a, &path, false)?;
            std::fs::read_to_string(&p).map(Value::String).map_err(|e| format!("{path} can't be read: {e}"))
        }
        "write" | "append" => {
            let p = check_access(a, &path, true)?;
            let written = if op == "append" {
                use std::io::Write;
                std::fs::OpenOptions::new().create(true).append(true).open(&p).and_then(|mut f| f.write_all(text.as_bytes()))
            } else {
                std::fs::write(&p, text.as_bytes())
            };
            a.wrote = true;
            written.map(|()| Value::Null).map_err(|e| format!("{path} can't be written: {e}"))
        }
        "list" => {
            let p = check_access(a, &path, false)?;
            let mut paths: Vec<String> = std::fs::read_dir(&p)
                .map_err(|e| format!("{path} can't be listed: {e}"))?
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path().to_string_lossy().into_owned())
                .collect();
            paths.sort();
            Ok(Value::from(paths))
        }
        _ => Err(format!("no file operation `{op}`")),
    });
    match done {
        Ok(v) => Ok(js_str(&v.to_string())),
        Err(e) => Err(throw(e)),
    }
}

/// `__dialog(descriptionJson)` → the answer as JSON: the next of the run's answers, or, where
/// it has none left, the dialog is kept for the outcome and the script stopped.
fn native_dialog(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let described = arg_string(args, 0, ctx)?;
    let described: Value = serde_json::from_str(&described).map_err(|e| throw(format!("the dialog is not JSON: {e}")))?;
    let answer = with_active(|a| {
        if a.asks.is_some() {
            return Err(ASKED.to_string());
        }
        if a.adding {
            return Err("a script that adds commands asks its dialogs in their `run`, not as its file is read".to_string());
        }
        if let Some(answer) = a.answers.get(a.answered) {
            a.answered += 1;
            return Ok(answer.clone());
        }
        if a.wrote {
            return Err("a dialog is asked after a file was written: the script is run again from its start once it is answered, \
                        so it asks its dialogs first"
                .to_string());
        }
        a.asks = Some(described);
        Err(ASKED.to_string())
    })
    .map_err(throw)?;
    Ok(js_str(&answer.to_string()))
}

/// `__print(text)`: a line, or lines, of the run's output.
fn native_print(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let text = arg_string(args, 0, ctx)?;
    let _ = with_active(|a| {
        a.output.extend(text.split('\n').map(str::to_string));
        Ok(())
    });
    Ok(JsValue::undefined())
}

fn new_context() -> Result<Context, String> {
    let mut ctx = Context::default();
    let limits = ctx.runtime_limits_mut();
    // A loop that never ends stops, rather than the window with it: after some seconds, where
    // the turns of any loop a shot needs are counted in thousands.
    limits.set_loop_iteration_limit(10_000_000);
    limits.set_recursion_limit(2000);
    let natives: [(JsString, usize, fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>); 6] = [
        (js_string!("__exec"), 2, native_exec),
        (js_string!("__print"), 1, native_print),
        (js_string!("__inHand"), 0, native_in_hand),
        (js_string!("__undo"), 2, native_undo),
        (js_string!("__file"), 3, native_file),
        (js_string!("__dialog"), 1, native_dialog),
    ];
    for (name, length, f) in natives {
        ctx.register_global_callable(name, length, NativeFunction::from_fn_ptr(f)).map_err(|e| e.to_string())?;
    }
    ctx.eval(Source::from_bytes(PRELUDE)).map_err(|e| format!("the script prelude: {e}"))?;
    Ok(ctx)
}

/// Gives the session back to the run's caller when the run ends, however it ends.
struct Restore<'a> {
    target: &'a mut Session,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        if let Some(active) = ACTIVE.with(|a| a.borrow_mut().take()) {
            *self.target = active.session;
        }
    }
}

/// Run a script against `session`. All it does is one undo step named after it, but for what
/// it does in an `iskra.undoGroup`, a step of the group's; if it throws, none of it stays. Its
/// errors are in the outcome, not returned.
pub fn run(session: &mut Session, request: &ScriptRequest) -> Outcome {
    let name = request.name.as_str();
    let failed = |message: String| Outcome { error: Some(ScriptError { message, file: name.to_string(), ..Default::default() }), ..Default::default() };
    if let Err(message) = check_nesting(&request.code) {
        return failed(message);
    }
    if ACTIVE.with(|a| a.try_borrow().map(|active| active.is_some()).unwrap_or(true)) {
        return failed("scripts can't run scripts".into());
    }
    let mut active = Active::new(std::mem::take(session), name, false);
    if request.in_hand.is_object() {
        active.in_hand = request.in_hand.to_string();
    }
    (active.write_files, active.answers) = (request.write_files, request.answers.clone());
    ACTIVE.with(|a| *a.borrow_mut() = Some(active));
    let guard = Restore { target: session };
    let result = match new_context() {
        Ok(mut ctx) => eval_request(&mut ctx, &request.code, name, request.command.as_deref()),
        Err(message) => Err(ScriptError { message, file: name.to_string(), ..Default::default() }),
    };
    // A run that asked a dialog is taken back however it ended, as one that failed: it is run
    // again once the dialog is answered, and prints then what it prints.
    let (output, asks) = with_active(|a| {
        match (&result, &a.asks) {
            (Ok(_), None) => a.session.fold(a.open, &a.name, true),
            _ => a.session.fold(a.first, "", false),
        }
        Ok(match a.asks.take() {
            Some(asks) => (Vec::new(), Some(asks)),
            None => (std::mem::take(&mut a.output), None),
        })
    })
    .unwrap_or_default();
    drop(guard);
    if asks.is_some() {
        return Outcome { asks, ..Default::default() };
    }
    let mut outcome = Outcome { output, ..Default::default() };
    match result {
        Ok(value) => outcome.result = value,
        Err(error) => outcome.error = Some(error),
    }
    outcome
}

/// Evaluate a script, or, with `command`, read the file for its commands and run that one's
/// `run`, giving what it gives.
fn eval_request(ctx: &mut Context, code: &str, file: &str, command: Option<&str>) -> Result<Value, ScriptError> {
    let Some(id) = command else { return eval(ctx, code, file) };
    let was = with_active(|a| Ok(std::mem::replace(&mut a.adding, true))).unwrap_or_default();
    let read = eval(ctx, code, file);
    let _ = with_active(|a| {
        a.adding = was;
        Ok(())
    });
    read?;
    eval(ctx, &format!("__run({})", Value::from(id)), &format!("{file} (run)"))
}

/// The commands the script `code` adds with `iskra.command`, read from it without running any.
/// The file is read against `session`, which it does not change.
pub fn added_commands(session: &mut Session, code: &str, file: &str) -> Result<Vec<crate::Added>, ScriptError> {
    let request = ScriptRequest { code: code.to_string(), name: file.to_string(), ..Default::default() };
    let failed = |message: String| ScriptError { message, file: file.to_string(), ..Default::default() };
    check_nesting(code).map_err(failed)?;
    if ACTIVE.with(|a| a.try_borrow().map(|active| active.is_some()).unwrap_or(true)) {
        return Err(failed("scripts can't run scripts".into()));
    }
    let active = Active::new(std::mem::take(session), file, true);
    ACTIVE.with(|a| *a.borrow_mut() = Some(active));
    let guard = Restore { target: session };
    let added = new_context().map_err(failed).and_then(|mut ctx| {
        eval(&mut ctx, &request.code, file)?;
        eval(&mut ctx, "__added()", &format!("{file} (commands)"))
    });
    let _ = with_active(|a| {
        a.session.fold(a.first, "", false);
        Ok(())
    });
    drop(guard);
    let added = added?;
    serde_json::from_value(added).map_err(|e| failed(format!("its commands: {e}")))
}

/// Evaluate `code` and give its last value as JSON.
fn eval(ctx: &mut Context, code: &str, file: &str) -> Result<Value, ScriptError> {
    let source = Source::from_bytes(code.as_bytes()).with_path(std::path::Path::new(file));
    let script = boa_engine::Script::parse(source, None, ctx).map_err(|e| script_error(&e, ctx, file, code))?;
    let value = script.evaluate(ctx).map_err(|e| script_error(&e, ctx, file, code))?;
    Ok(result_json(&value, ctx))
}

/// The script's completion value as JSON (`__result` in the prelude makes the text of it).
fn result_json(v: &JsValue, ctx: &mut Context) -> Value {
    if v.is_undefined() {
        return Value::Null;
    }
    let global = ctx.global_object();
    let Some(f) = global.get(js_string!("__result"), ctx).ok().and_then(|f| f.as_object()) else { return Value::Null };
    match f.call(&JsValue::undefined(), std::slice::from_ref(v), ctx) {
        Ok(s) if s.is_string() => {
            let s = s.to_string(ctx).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            serde_json::from_str(&s).unwrap_or(Value::String(s))
        }
        _ => Value::Null,
    }
}

/// A boa error as a message with its line and column (`code` is the script).
fn script_error(e: &boa_engine::JsError, ctx: &mut Context, file: &str, code: &str) -> ScriptError {
    // Thrown values: `throw new Error("x")`, `throw "x"`, native errors.
    let display = e.to_string();
    let message = match e.as_opaque() {
        Some(v) => {
            let o = v.as_object();
            let msg = o.as_ref().and_then(|o| o.get(js_string!("message"), ctx).ok()).filter(|m| !m.is_undefined());
            let name = o.as_ref().and_then(|o| o.get(js_string!("name"), ctx).ok()).filter(|m| !m.is_undefined());
            match (name, msg) {
                (Some(n), Some(m)) => {
                    let n = n.to_string(ctx).map(|s| s.to_std_string_escaped()).unwrap_or_default();
                    let m = m.to_string(ctx).map(|s| s.to_std_string_escaped()).unwrap_or_default();
                    if n == "Error" { m } else { format!("{n}: {m}") }
                }
                _ => v.to_string(ctx).map(|s| s.to_std_string_escaped()).unwrap_or_else(|_| display.clone()),
            }
        }
        None => {
            let first = display.lines().next().unwrap_or("").trim();
            let first = first.split(" at line ").next().unwrap_or(first).trim();
            let first = match first.rfind(" (") {
                Some(i) if first.ends_with(')') && first[i..].contains(':') => first[..i].trim(),
                _ => first,
            };
            first.strip_prefix("Error: ").unwrap_or(first).to_string()
        }
    };
    let (line, column) = position(&display, file, code);
    ScriptError { message, line, column, file: file.to_string() }
}

/// Line and column of an error: `at line N, col M` (syntax errors), else the innermost frame of
/// the backtrace in the script (`file:line:col`), else, for a name boa can't place, the first
/// use of that undefined name in `code`.
fn position(msg: &str, file: &str, code: &str) -> (Option<u32>, Option<u32>) {
    if let Some(i) = msg.find("at line ") {
        let rest = &msg[i + 8..];
        let line = rest.split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse().ok());
        let col = rest.find("col ").and_then(|j| rest[j + 4..].split(|c: char| !c.is_ascii_digit()).next()).and_then(|n| n.parse().ok());
        return (line, col);
    }
    // `(file:12:5)`: the digits after the file's name.
    let pick = |l: &str| -> Option<(u32, u32)> {
        let i = l.find(&format!("({file}:"))? + file.len() + 2;
        let mut it = l[i..].split([':', ')']);
        Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
    };
    let frames: Vec<&str> = msg.lines().skip(1).collect();
    if let Some((a, b)) = frames.iter().find_map(|l| pick(l)).or_else(|| msg.lines().next().and_then(pick)) {
        return (Some(a), Some(b));
    }
    // `ReferenceError: name is not defined` without a place.
    let first = msg.lines().next().unwrap_or("");
    if let Some(name) = first.split("ReferenceError: ").nth(1).and_then(|r| r.split(" is not defined").next()) {
        for (n, line) in code.lines().enumerate() {
            let b = line.as_bytes();
            let mut from = 0;
            while let Some(i) = line[from..].find(name).map(|i| i + from) {
                let before = i.checked_sub(1).map(|j| b[j]);
                let after = b.get(i + name.len()).copied();
                let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
                if !before.is_some_and(|c| ident(c) || c == b'.') && !after.is_some_and(ident) {
                    return (Some(n as u32 + 1), Some(i as u32 + 1));
                }
                from = i + name.len();
            }
        }
    }
    (None, None)
}

/// The deepest nesting of brackets a script may have. The parser recurses a level at a time and
/// overflows the stack, which ends the app and can't be caught, at about 200 levels.
pub const MAX_NESTING: usize = 96;

/// An error when `code` nests brackets deeper than [`MAX_NESTING`]; strings, template literals
/// and comments are skipped.
pub fn check_nesting(code: &str) -> Result<(), String> {
    let b = code.as_bytes();
    let (mut i, mut depth, mut max) = (0usize, 0usize, 0usize);
    while let Some(&c) = b.get(i) {
        match c {
            b'(' | b'[' | b'{' => {
                depth += 1;
                max = max.max(depth);
            }
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'"' | b'\'' | b'`' => {
                // To the closing quote; an escape skips a byte.
                i += 1;
                while let Some(&d) = b.get(i) {
                    if d == b'\\' {
                        i += 1;
                    } else if d == c || (d == b'\n' && c != b'`') {
                        break;
                    }
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while b.get(i).is_some_and(|d| *d != b'\n') {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < b.len() && !(b.get(i) == Some(&b'*') && b.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    if max > MAX_NESTING {
        return Err(format!("the script is nested too deeply ({max} levels of brackets; at most {MAX_NESTING})"));
    }
    Ok(())
}
