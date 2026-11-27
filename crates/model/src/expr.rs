//! The expressions of a Driver node (plan §8.8): a few lines that work out, on every frame,
//! amounts that are added to a peg's values, from other pegs, the driver's parameters and the
//! frame.
//!
//! It reads as JavaScript arithmetic but is a small language of Iskra's own: parsed once, when
//! the expressions are set, into a tree, and worked out in plain Rust wherever a peg's values
//! are read, on any thread. What it reads of the shot comes through a [`Scope`], so the
//! language knows nothing of pegs.
//!
//! ```text
//! // A wheel under its car.
//! let turns = parent.travelled / (2 * PI * radius)
//! rotation = turns * 360
//! ```

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A value of a peg that expressions give an amount for, which is added to the peg's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Driven {
    X,
    Y,
    Rotation,
    ScaleX,
    ScaleY,
    Skew,
    Depth,
}

impl Driven {
    pub const ALL: [Driven; 7] = [Driven::X, Driven::Y, Driven::Rotation, Driven::ScaleX, Driven::ScaleY, Driven::Skew, Driven::Depth];

    /// Its name in the expressions.
    pub fn name(self) -> &'static str {
        match self {
            Driven::X => "x",
            Driven::Y => "y",
            Driven::Rotation => "rotation",
            Driven::ScaleX => "scaleX",
            Driven::ScaleY => "scaleY",
            Driven::Skew => "skew",
            Driven::Depth => "depth",
        }
    }

    fn named(name: &str) -> Option<Driven> {
        Driven::ALL.into_iter().find(|driven| driven.name() == name)
    }
}

/// What expressions read of a peg.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Read {
    /// One of its own values, in its parent's space, as Properties shows it.
    Own(Driven),
    /// Where it is in the scene, through every peg above it.
    WorldX,
    WorldY,
    /// In degrees, clockwise on screen.
    WorldRotation,
    /// How much larger the scene sees it than it is drawn, across.
    WorldScale,
}

/// What the expressions read the shot through. Frames are the model's, from 0.
pub trait Scope {
    /// What `input` reads of its peg on `frame`: `own` is the driven peg's own values, before
    /// the driver; `parent` its parent; any other name one of the driver's inputs. `None` for
    /// an input the driver has not got, or a peg that is gone.
    fn read(&self, input: &str, read: Read, frame: u32) -> Option<f64>;
    /// The driver's parameter `name` on `frame`.
    fn parameter(&self, name: &str, frame: u32) -> Option<f64>;
    /// A number that is another whenever anything the expressions can read is: what goes
    /// over every frame up to the one asked for (`travelled`, `spring`) is kept by it, and
    /// worked out again only once it changes. `None` keeps nothing.
    fn fingerprint(&self) -> Option<u64> {
        None
    }
}

/// Where in the text something is, both counted from 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct At {
    pub line: u32,
    pub column: u32,
}

/// Why expressions can't be set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExprError {
    pub at: At,
    pub message: String,
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, column {}: {}", self.at.line, self.at.column, self.message)
    }
}

impl std::error::Error for ExprError {}

fn error<T>(at: At, message: impl Into<String>) -> Result<T, ExprError> {
    Err(ExprError { at, message: message.into() })
}

/// What can be read of an input after its name.
const READS: &str = "x, y, rotation, scaleX, scaleY, skew, depth, travelled or world";

/// Names a parameter can't have: what the language means by them.
pub fn reserved(name: &str) -> bool {
    const WORDS: [&str; 12] = ["frame", "time", "fps", "PI", "E", "Math", "own", "parent", "let", "at", "spring", "true"];
    WORDS.contains(&name) || name == "false" || Driven::named(name).is_some() || Func::named(name).is_some()
}

// ----- Tokens -----

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Name(String),
    /// An operator or a mark, by its text.
    Mark(&'static str),
    /// The end of a line: a new line, or a `;`.
    End,
}

const MARKS: [&str; 24] = ["**", "<=", ">=", "==", "!=", "&&", "||", "+", "-", "*", "/", "%", "<", ">", "!", "?", ":", "(", ")", ",", ".", "=", "===", "!=="];

fn tokens(text: &str) -> Result<Vec<(Token, At)>, ExprError> {
    let chars: Vec<char> = text.chars().collect();
    let (mut out, mut i, mut line, mut column, mut brackets) = (Vec::new(), 0, 1u32, 1u32, 0i32);
    while i < chars.len() {
        let c = chars[i];
        let at = At { line, column };
        let mut step = 1;
        if c == '\n' {
            // A line goes on inside brackets.
            if brackets == 0 {
                out.push((Token::End, at));
            }
            line += 1;
            column = 0;
        } else if c == ';' {
            out.push((Token::End, at));
        } else if c.is_whitespace() {
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i + step < chars.len() && chars[i + step] != '\n' {
                step += 1;
            }
        } else if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) {
            while i + step < chars.len() && (chars[i + step].is_ascii_digit() || chars[i + step] == '.') {
                step += 1;
            }
            // An exponent: 1e3, 2.5e-2.
            if matches!(chars.get(i + step), Some('e' | 'E')) {
                let sign = usize::from(matches!(chars.get(i + step + 1), Some('+' | '-')));
                if chars.get(i + step + 1 + sign).is_some_and(char::is_ascii_digit) {
                    step += 1 + sign;
                    while i + step < chars.len() && chars[i + step].is_ascii_digit() {
                        step += 1;
                    }
                }
            }
            let digits: String = chars[i..i + step].iter().collect();
            let Ok(number) = digits.parse::<f64>() else { return error(at, format!("`{digits}` is not a number")) };
            out.push((Token::Number(number), at));
        } else if c.is_alphabetic() || c == '_' || c == '$' {
            while i + step < chars.len() && (chars[i + step].is_alphanumeric() || chars[i + step] == '_' || chars[i + step] == '$') {
                step += 1;
            }
            out.push((Token::Name(chars[i..i + step].iter().collect()), at));
        } else {
            let rest: String = chars[i..chars.len().min(i + 3)].iter().collect();
            let Some(mark) = MARKS.iter().filter(|mark| rest.starts_with(**mark)).max_by_key(|mark| mark.len()) else {
                return error(at, format!("`{c}` is not understood here"));
            };
            step = mark.chars().count();
            // JavaScript's strict comparisons are the same here, as there are only numbers.
            let mark = match *mark {
                "===" => "==",
                "!==" => "!=",
                mark => mark,
            };
            match mark {
                "(" => brackets += 1,
                ")" => brackets -= 1,
                _ => {}
            }
            out.push((Token::Mark(mark), at));
        }
        i += step;
        column += step as u32;
    }
    out.push((Token::End, At { line, column }));
    Ok(out)
}

// ----- The tree -----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Func {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Sqrt,
    Abs,
    Min,
    Max,
    Floor,
    Ceil,
    Round,
    Sign,
    Pow,
    Exp,
    Log,
    Clamp,
    Lerp,
    Remap,
    Smoothstep,
    Deg,
    Rad,
    Noise,
}

impl Func {
    const ALL: [(&'static str, Func); 25] = [
        ("sin", Func::Sin),
        ("cos", Func::Cos),
        ("tan", Func::Tan),
        ("asin", Func::Asin),
        ("acos", Func::Acos),
        ("atan", Func::Atan),
        ("atan2", Func::Atan2),
        ("sqrt", Func::Sqrt),
        ("abs", Func::Abs),
        ("min", Func::Min),
        ("max", Func::Max),
        ("floor", Func::Floor),
        ("ceil", Func::Ceil),
        ("round", Func::Round),
        ("sign", Func::Sign),
        ("pow", Func::Pow),
        ("exp", Func::Exp),
        ("log", Func::Log),
        ("clamp", Func::Clamp),
        ("lerp", Func::Lerp),
        ("remap", Func::Remap),
        ("smoothstep", Func::Smoothstep),
        ("deg", Func::Deg),
        ("rad", Func::Rad),
        ("noise", Func::Noise),
    ];

    fn named(name: &str) -> Option<Func> {
        Func::ALL.iter().find(|(text, _)| *text == name).map(|(_, func)| *func)
    }

    /// How many arguments it takes, at least and at most.
    fn arity(self) -> (usize, usize) {
        match self {
            Func::Min | Func::Max => (1, usize::MAX),
            Func::Atan2 | Func::Pow => (2, 2),
            Func::Noise => (1, 2),
            Func::Clamp | Func::Lerp | Func::Smoothstep => (3, 3),
            Func::Remap => (5, 5),
            _ => (1, 1),
        }
    }

    fn apply(self, a: &[f64]) -> f64 {
        let arg = |i: usize| a.get(i).copied().unwrap_or(0.0);
        let (x, y, z) = (arg(0), arg(1), arg(2));
        match self {
            Func::Sin => x.sin(),
            Func::Cos => x.cos(),
            Func::Tan => x.tan(),
            Func::Asin => x.asin(),
            Func::Acos => x.acos(),
            Func::Atan => x.atan(),
            Func::Atan2 => x.atan2(y),
            Func::Sqrt => x.sqrt(),
            Func::Abs => x.abs(),
            Func::Min => a.iter().copied().fold(f64::INFINITY, f64::min),
            Func::Max => a.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            Func::Floor => x.floor(),
            Func::Ceil => x.ceil(),
            // As JavaScript's: a half goes up.
            Func::Round => (x + 0.5).floor(),
            Func::Sign => {
                if x == 0.0 {
                    0.0
                } else {
                    x.signum()
                }
            }
            Func::Pow => x.powf(y),
            Func::Exp => x.exp(),
            Func::Log => x.ln(),
            Func::Clamp => x.max(y).min(z),
            Func::Lerp => x + (y - x) * z,
            Func::Remap => {
                let (from, to) = ((arg(1), arg(2)), (arg(3), arg(4)));
                if from.1 == from.0 {
                    return if x < from.0 { to.0 } else { to.1 };
                }
                let t = ((x - from.0) / (from.1 - from.0)).clamp(0.0, 1.0);
                to.0 + (to.1 - to.0) * t
            }
            Func::Smoothstep => {
                if y == x {
                    return if z < x { 0.0 } else { 1.0 };
                }
                let t = ((z - x) / (y - x)).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            }
            Func::Deg => x.to_degrees(),
            Func::Rad => x.to_radians(),
            Func::Noise => noise(x, y),
        }
    }
}

/// Smooth noise along `t`, from -1 to 1, a new value at each whole number eased into the next;
/// another `seed` is another line. The same on every play and every machine.
fn noise(t: f64, seed: f64) -> f64 {
    let at = |i: f64| {
        const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut h = (i as i64 as u64).wrapping_add(seed.to_bits().rotate_left(29)).wrapping_mul(GOLDEN).wrapping_add(GOLDEN);
        // splitmix64's finish.
        h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        h ^= h >> 31;
        (h >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    let (floor, t) = (t.floor(), t - t.floor());
    let ease = t * t * (3.0 - 2.0 * t);
    at(floor) + (at(floor + 1.0) - at(floor)) * ease
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unary {
    Minus,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Binary {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Less,
    LessOrEqual,
    More,
    MoreOrEqual,
    Equal,
    NotEqual,
    And,
    Or,
}

#[derive(Debug)]
enum Expr {
    Number(f64),
    /// The frame as the timeline numbers it, from 1.
    Frame,
    Parameter(String),
    Input(String, InputRead),
    /// A `let`'s expression, worked out where it is used, so that `at` sees it on its frame.
    Named(Arc<Expr>),
    Unary(Unary, Box<Expr>),
    Binary(Binary, Box<Expr>, Box<Expr>),
    Choose(Box<Expr>, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>),
    /// The first on the frame the second gives, as the timeline numbers frames.
    At(Box<Expr>, Box<Expr>),
    /// The first followed by a spring from the first frame, its stiffness and damping.
    Spring(Box<Expr>, Box<Expr>, Box<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputRead {
    Read(Read),
    /// How far it has gone in the scene since the first frame, back where it goes left.
    Travelled,
}

// ----- Parsing -----

struct Parser {
    tokens: Vec<(Token, At)>,
    next: usize,
    /// The `let`s so far, by name.
    named: Vec<(String, Arc<Expr>)>,
    inputs: BTreeSet<String>,
    parameters: BTreeSet<String>,
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.next.min(self.tokens.len() - 1)].0
    }

    fn at(&self) -> At {
        self.tokens[self.next.min(self.tokens.len() - 1)].1
    }

    fn take(&mut self) -> (Token, At) {
        let taken = self.tokens[self.next.min(self.tokens.len() - 1)].clone();
        self.next = (self.next + 1).min(self.tokens.len());
        taken
    }

    fn is(&self, mark: &str) -> bool {
        matches!(self.peek(), Token::Mark(m) if *m == mark)
    }

    fn eat(&mut self, mark: &str) -> bool {
        let is = self.is(mark);
        if is {
            self.next += 1;
        }
        is
    }

    fn expect(&mut self, mark: &str) -> Result<(), ExprError> {
        if self.eat(mark) {
            return Ok(());
        }
        let at = self.at();
        error(at, format!("`{mark}` is wanted here, not {}", self.describe()))
    }

    fn describe(&self) -> String {
        match self.peek() {
            Token::Number(n) => format!("the number {n}"),
            Token::Name(name) => format!("`{name}`"),
            Token::Mark(mark) => format!("`{mark}`"),
            Token::End if self.next + 1 >= self.tokens.len() => "the end".to_string(),
            Token::End => "the end of the line".to_string(),
        }
    }

    fn name(&mut self) -> Result<(String, At), ExprError> {
        match self.take() {
            (Token::Name(name), at) => Ok((name, at)),
            (_, at) => {
                self.next -= 1;
                error(at, format!("a name is wanted here, not {}", self.describe()))
            }
        }
    }

    fn lines(&mut self) -> Result<Vec<(Driven, Expr)>, ExprError> {
        let mut gives: Vec<(Driven, Expr)> = Vec::new();
        loop {
            while matches!(self.peek(), Token::End) && self.next + 1 < self.tokens.len() {
                self.next += 1;
            }
            if self.next + 1 >= self.tokens.len() {
                return Ok(gives);
            }
            let (word, at) = self.name()?;
            if word == "let" || word == "const" || word == "var" {
                let (name, at) = self.name()?;
                if reserved(&name) {
                    return error(at, format!("`{name}` is a name the language has; take another"));
                }
                self.expect("=")?;
                let expr = self.expr()?;
                self.named.retain(|(other, _)| *other != name);
                self.named.push((name, Arc::new(expr)));
            } else if let Some(driven) = Driven::named(&word) {
                if !self.eat("=") {
                    let at = self.at();
                    return error(at, format!("`=` is wanted after `{word}`, to give it an amount"));
                }
                let expr = self.expr()?;
                // Given again, the last is what counts, as in JavaScript.
                gives.retain(|(other, _)| *other != driven);
                gives.push((driven, expr));
            } else {
                let wanted = Driven::ALL.map(Driven::name).join(", ");
                return error(at, format!("a line gives one of {wanted}, or begins with `let`; `{word}` is neither"));
            }
            if !matches!(self.peek(), Token::End) {
                let at = self.at();
                return error(at, format!("the line should end here, not at {}", self.describe()));
            }
        }
    }

    fn expr(&mut self) -> Result<Expr, ExprError> {
        let test = self.binary(0)?;
        if !self.eat("?") {
            return Ok(test);
        }
        let yes = self.expr()?;
        self.expect(":")?;
        let no = self.expr()?;
        Ok(Expr::Choose(Box::new(test), Box::new(yes), Box::new(no)))
    }

    /// Operators bound at least as tightly as `level`, left to right.
    fn binary(&mut self, level: usize) -> Result<Expr, ExprError> {
        const LEVELS: [&[(&str, Binary)]; 6] = [
            &[("||", Binary::Or)],
            &[("&&", Binary::And)],
            &[("==", Binary::Equal), ("!=", Binary::NotEqual)],
            &[("<=", Binary::LessOrEqual), (">=", Binary::MoreOrEqual), ("<", Binary::Less), (">", Binary::More)],
            &[("+", Binary::Add), ("-", Binary::Sub)],
            &[("*", Binary::Mul), ("/", Binary::Div), ("%", Binary::Rem)],
        ];
        let Some(operators) = LEVELS.get(level) else { return self.unary() };
        let mut left = self.binary(level + 1)?;
        while let Some((_, op)) = operators.iter().find(|(mark, _)| self.is(mark)) {
            self.next += 1;
            let right = self.binary(level + 1)?;
            left = Expr::Binary(*op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, ExprError> {
        if self.eat("-") {
            return Ok(Expr::Unary(Unary::Minus, Box::new(self.unary()?)));
        }
        if self.eat("+") {
            return self.unary();
        }
        if self.eat("!") {
            return Ok(Expr::Unary(Unary::Not, Box::new(self.unary()?)));
        }
        self.power()
    }

    /// `**` binds tighter than a minus in front, and from the right: `-2 ** 2` is -4.
    fn power(&mut self) -> Result<Expr, ExprError> {
        let base = self.atom()?;
        if !self.eat("**") {
            return Ok(base);
        }
        let exponent = self.unary()?;
        Ok(Expr::Binary(Binary::Pow, Box::new(base), Box::new(exponent)))
    }

    fn atom(&mut self) -> Result<Expr, ExprError> {
        let (token, at) = self.take();
        match token {
            Token::Number(n) => Ok(Expr::Number(n)),
            Token::Mark("(") => {
                let expr = self.expr()?;
                self.expect(")")?;
                Ok(expr)
            }
            Token::Name(name) => self.named_atom(name, at),
            _ => {
                self.next -= 1;
                error(at, format!("a number, a name or `(` is wanted here, not {}", self.describe()))
            }
        }
    }

    fn named_atom(&mut self, name: String, at: At) -> Result<Expr, ExprError> {
        // `Math.sin`, `Math.PI`: JavaScript's way to the same.
        let (name, at) = if name == "Math" {
            self.expect(".")?;
            self.name()?
        } else {
            (name, at)
        };
        if self.is("(") {
            return self.call(&name, at);
        }
        match name.as_str() {
            "PI" => return Ok(Expr::Number(std::f64::consts::PI)),
            "E" => return Ok(Expr::Number(std::f64::consts::E)),
            "true" => return Ok(Expr::Number(1.0)),
            "false" => return Ok(Expr::Number(0.0)),
            "frame" => return Ok(Expr::Frame),
            // A shot does not know the project's frame rate: expressions count in frames.
            "time" | "fps" => return error(at, format!("expressions count in frames: `{name}` is not known; use `frame`")),
            _ => {}
        }
        if let Some((_, expr)) = self.named.iter().rev().find(|(named, _)| *named == name) {
            return Ok(Expr::Named(expr.clone()));
        }
        if self.is(".") {
            return self.input(name, at);
        }
        if let Some(driven) = Driven::named(&name) {
            return error(at, format!("`{name}` is what a line gives, not what it reads; the peg's own is `own.{}`", driven.name()));
        }
        if name == "own" || name == "parent" {
            return error(at, format!("`{name}` is a peg: read one of its values, as `{name}.x`"));
        }
        if Func::named(&name).is_some() || name == "at" || name == "spring" {
            return error(at, format!("`{name}` is a function: it is wanted with `(` after it"));
        }
        self.parameters.insert(name.clone());
        Ok(Expr::Parameter(name))
    }

    fn input(&mut self, input: String, at: At) -> Result<Expr, ExprError> {
        self.expect(".")?;
        let (what, what_at) = self.name()?;
        let read = match what.as_str() {
            "travelled" => InputRead::Travelled,
            "world" => {
                self.expect(".")?;
                let (what, at) = self.name()?;
                InputRead::Read(match what.as_str() {
                    "x" => Read::WorldX,
                    "y" => Read::WorldY,
                    "rotation" => Read::WorldRotation,
                    "scale" => Read::WorldScale,
                    _ => return error(at, format!("what is read of where `{input}` is in the scene is x, y, rotation or scale, not `{what}`")),
                })
            }
            _ => match Driven::named(&what) {
                Some(driven) => InputRead::Read(Read::Own(driven)),
                None => return error(what_at, format!("what is read of `{input}` is {READS}, not `{what}`")),
            },
        };
        if reserved(&input) && input != "own" && input != "parent" {
            return error(at, format!("`{input}` is a name the language has, not an input"));
        }
        if input != "own" && input != "parent" {
            self.inputs.insert(input.clone());
        }
        Ok(Expr::Input(input, read))
    }

    fn arguments(&mut self) -> Result<Vec<Expr>, ExprError> {
        self.expect("(")?;
        let mut arguments = Vec::new();
        if !self.eat(")") {
            loop {
                arguments.push(self.expr()?);
                if self.eat(")") {
                    break;
                }
                self.expect(",")?;
            }
        }
        Ok(arguments)
    }

    fn call(&mut self, name: &str, at: At) -> Result<Expr, ExprError> {
        let mut arguments = self.arguments()?;
        let count = |wanted: usize, arguments: &Vec<Expr>| {
            if arguments.len() == wanted { Ok(()) } else { error(at, format!("`{name}` takes {wanted} numbers, not {}", arguments.len())) }
        };
        match name {
            "at" => {
                count(2, &arguments)?;
                let (frame, expr) = (arguments.pop(), arguments.pop());
                Ok(Expr::At(Box::new(expr.unwrap_or(Expr::Number(0.0))), Box::new(frame.unwrap_or(Expr::Number(0.0)))))
            }
            "spring" => {
                count(3, &arguments)?;
                let mut taken = arguments.into_iter();
                let mut next = || Box::new(taken.next().unwrap_or(Expr::Number(0.0)));
                Ok(Expr::Spring(next(), next(), next()))
            }
            _ => {
                let Some(func) = Func::named(name) else { return error(at, format!("there is no function `{name}`")) };
                let (least, most) = func.arity();
                if arguments.len() < least || arguments.len() > most {
                    let wanted = if least == most {
                        format!("{least}")
                    } else if most == usize::MAX {
                        format!("{least} or more")
                    } else {
                        format!("{least} to {most}")
                    };
                    return error(at, format!("`{name}` takes {wanted} numbers, not {}", arguments.len()));
                }
                Ok(Expr::Call(func, arguments))
            }
        }
    }
}

// ----- Expressions, and working them out -----

/// A Driver's expressions: their text, and what it was parsed into.
#[derive(Clone, Debug)]
pub struct Expressions {
    text: String,
    parsed: Arc<Parsed>,
}

#[derive(Debug, Default)]
struct Parsed {
    gives: Vec<(Driven, Expr)>,
    inputs: BTreeSet<String>,
    parameters: BTreeSet<String>,
    /// Why the text does not parse, for text read from a file: it then gives nothing.
    error: Option<ExprError>,
}

/// What the expressions give on a frame: an amount for each value they give one for, which is
/// added to the peg's own.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Amounts {
    pub amounts: [Option<f64>; 7],
    /// A value they give came to no number on the frame (a division by zero, an input that is
    /// gone), and gives nothing there.
    pub failed: bool,
}

impl Amounts {
    pub fn of(&self, driven: Driven) -> Option<f64> {
        self.amounts[driven as usize]
    }
}

/// How many frames `at` and `spring` may be nested through, each of which may go over all the
/// frames before.
const NESTING: u32 = 4;

struct Work<'a> {
    scope: &'a dyn Scope,
    nesting: u32,
    /// The scope's fingerprint, asked for once.
    fingerprint: Option<u64>,
}

thread_local! {
    /// What `travelled` and `spring` worked out on every frame from the first, by the
    /// fingerprint of what they read and by which of them it is: on each frame, how far it
    /// had gone and where it was, or where the spring was and how fast.
    static KEPT: std::cell::RefCell<std::collections::HashMap<(u64, u64), Vec<[f64; 3]>>> = Default::default();
}

/// How many of them a thread keeps before it lets them all go.
const KEEPS: usize = 64;

/// Which of them it is, as a number: `travelled` of an input, or a spring of an expression in
/// a tree that is shared while the expressions are the same.
fn which(parts: impl std::hash::Hash) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault};
    BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default().hash_one(parts)
}

impl Expressions {
    /// Parse `text`, or say why it can't be.
    pub fn parse(text: &str) -> Result<Expressions, ExprError> {
        let mut parser = Parser { tokens: tokens(text)?, next: 0, named: Vec::new(), inputs: BTreeSet::new(), parameters: BTreeSet::new() };
        let gives = parser.lines()?;
        let parsed = Parsed { gives, inputs: parser.inputs, parameters: parser.parameters, error: None };
        Ok(Expressions { text: text.to_string(), parsed: Arc::new(parsed) })
    }

    /// `text` parsed, or where it does not parse, kept as it is, giving nothing: what is read
    /// from a file, which is never dropped.
    pub fn read(text: &str) -> Expressions {
        Expressions::parse(text)
            .unwrap_or_else(|error| Expressions { text: text.to_string(), parsed: Arc::new(Parsed { error: Some(error), ..Parsed::default() }) })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Why the text did not parse, where it was read as it was.
    pub fn error(&self) -> Option<&ExprError> {
        self.parsed.error.as_ref()
    }

    /// The values they give an amount for.
    pub fn gives(&self) -> Vec<Driven> {
        self.parsed.gives.iter().map(|(driven, _)| *driven).collect()
    }

    /// The inputs they read, by name, besides `own` and `parent`.
    pub fn inputs(&self) -> &BTreeSet<String> {
        &self.parsed.inputs
    }

    /// The parameters they read, by name.
    pub fn parameters(&self) -> &BTreeSet<String> {
        &self.parsed.parameters
    }

    /// The same with the input (`input`) or the parameter `old` called `new` wherever they
    /// read it, the rest of the text as it was; or why the text that makes can't be set.
    pub fn renamed(&self, old: &str, new: &str, input: bool) -> Result<Expressions, ExprError> {
        Expressions::parse(&renamed_in(&self.text, old, new, input))
    }

    /// What they give on `frame`, the model's, from 0.
    pub fn amounts(&self, scope: &dyn Scope, frame: u32) -> Amounts {
        let work = Work { scope, nesting: 0, fingerprint: scope.fingerprint() };
        let mut out = Amounts::default();
        for (driven, expr) in &self.parsed.gives {
            let value = work.eval(expr, frame);
            if value.is_finite() {
                out.amounts[*driven as usize] = Some(value);
            } else {
                out.failed = true;
            }
        }
        out
    }
}

impl PartialEq for Expressions {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Default for Expressions {
    fn default() -> Self {
        Expressions::read("")
    }
}

impl Serialize for Expressions {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.text.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Expressions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Expressions::read(&String::deserialize(deserializer)?))
    }
}

/// `text` with each name `old` that reads an input (with a `.` after it) or a parameter (with
/// neither a `.` nor a `(` after it, and none before) written `new`; comments are left alone.
fn renamed_in(text: &str, old: &str, new: &str, input: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let (mut out, mut i) = (String::with_capacity(text.len()), 0);
    let next_mark = |mut at: usize| {
        while at < chars.len() && chars[at].is_whitespace() && chars[at] != '\n' {
            at += 1;
        }
        chars.get(at).copied()
    };
    while i < chars.len() {
        if chars[i] == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }
        let starts = word(chars[i]) && !chars[i].is_ascii_digit() && (i == 0 || !word(chars[i - 1]));
        if !starts {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let mut end = i;
        while end < chars.len() && word(chars[end]) {
            end += 1;
        }
        let name: String = chars[i..end].iter().collect();
        let before = out.trim_end().chars().last();
        let after = next_mark(end);
        // What is given (`x = …`) or named (`let k = …`) is no input and no parameter.
        let mut at = end;
        while at < chars.len() && chars[at].is_whitespace() && chars[at] != '\n' {
            at += 1;
        }
        let given = after == Some('=') && chars.get(at + 1) != Some(&'=');
        let member = before == Some('.') && !out.trim_end().ends_with("..");
        let reads = match input {
            true => after == Some('.'),
            false => after != Some('.') && after != Some('('),
        };
        out.push_str(if name == old && !member && !given && reads { new } else { &name });
        i = end;
    }
    out
}

/// A frame as the timeline numbers it, from 1, as the model's, from 0.
fn model_frame(shown: f64) -> u32 {
    if shown.is_finite() { (shown.round() - 1.0).clamp(0.0, f64::from(u32::MAX)) as u32 } else { 0 }
}

impl Work<'_> {
    fn eval(&self, expr: &Expr, frame: u32) -> f64 {
        let truth = |b: bool| if b { 1.0 } else { 0.0 };
        match expr {
            Expr::Number(n) => *n,
            Expr::Frame => f64::from(frame) + 1.0,
            Expr::Parameter(name) => self.scope.parameter(name, frame).unwrap_or(f64::NAN),
            Expr::Input(input, InputRead::Read(read)) => self.scope.read(input, *read, frame).unwrap_or(f64::NAN),
            Expr::Input(input, InputRead::Travelled) => self.travelled(input, frame),
            Expr::Named(expr) => self.eval(expr, frame),
            Expr::Unary(Unary::Minus, a) => -self.eval(a, frame),
            Expr::Unary(Unary::Not, a) => truth(self.eval(a, frame) == 0.0),
            Expr::Binary(op, a, b) => {
                let a = self.eval(a, frame);
                // `&&` and `||` look at the second only where it counts, as JavaScript does,
                // and give what they looked at last.
                match op {
                    Binary::And if a == 0.0 || a.is_nan() => return a,
                    Binary::Or if a != 0.0 && !a.is_nan() => return a,
                    Binary::And | Binary::Or => return self.eval(b, frame),
                    _ => {}
                }
                let b = self.eval(b, frame);
                match op {
                    Binary::Add => a + b,
                    Binary::Sub => a - b,
                    Binary::Mul => a * b,
                    Binary::Div => a / b,
                    Binary::Rem => a % b,
                    Binary::Pow => a.powf(b),
                    Binary::Less => truth(a < b),
                    Binary::LessOrEqual => truth(a <= b),
                    Binary::More => truth(a > b),
                    Binary::MoreOrEqual => truth(a >= b),
                    Binary::Equal => truth(a == b),
                    Binary::NotEqual => truth(a != b),
                    Binary::And | Binary::Or => unreachable!("worked out above"),
                }
            }
            Expr::Choose(test, yes, no) => {
                let test = self.eval(test, frame);
                if test.is_nan() {
                    f64::NAN
                } else if test != 0.0 {
                    self.eval(yes, frame)
                } else {
                    self.eval(no, frame)
                }
            }
            Expr::Call(func, arguments) => {
                let values: Vec<f64> = arguments.iter().map(|argument| self.eval(argument, frame)).collect();
                func.apply(&values)
            }
            Expr::At(expr, at) => {
                let at = model_frame(self.eval(at, frame));
                self.deeper(|work| work.eval(expr, at))
            }
            Expr::Spring(expr, stiffness, damping) => {
                let (stiffness, damping) = (self.eval(stiffness, frame), self.eval(damping, frame).clamp(0.0, 1.0));
                let spring = which(("spring", std::ptr::from_ref::<Expr>(expr) as usize, stiffness.to_bits(), damping.to_bits(), self.nesting));
                self.deeper(|work| {
                    // Stepped a frame at a time from the first, at rest where it begins.
                    let [place, ..] = work.kept(
                        spring,
                        frame,
                        |work| [work.eval(expr, 0), 0.0, 0.0],
                        |work, at, [place, speed, _]| {
                            let target = work.eval(expr, at);
                            let speed = (speed + (target - place) * stiffness) * (1.0 - damping);
                            [place + speed, speed, 0.0]
                        },
                    );
                    place
                })
            }
        }
    }

    /// `f` with one more level of `at` and `spring` inside, or no number past the limit.
    fn deeper(&self, f: impl FnOnce(&Work) -> f64) -> f64 {
        if self.nesting >= NESTING {
            return f64::NAN;
        }
        f(&Work { scope: self.scope, nesting: self.nesting + 1, fingerprint: self.fingerprint })
    }

    /// How far `input` has gone in the scene from the first frame to `frame`, frame by frame,
    /// each step counted back where it goes left: a wheel rolls back as its car backs.
    fn travelled(&self, input: &str, frame: u32) -> f64 {
        let place = |work: &Work, at: u32| {
            let read = |read| work.scope.read(input, read, at).unwrap_or(f64::NAN);
            (read(Read::WorldX), read(Read::WorldY))
        };
        let [gone, ..] = self.kept(
            which(("travelled", input)),
            frame,
            |work| {
                let (x, y) = place(work, 0);
                [0.0, x, y]
            },
            |work, at, [gone, x, y]| {
                let (here_x, here_y) = place(work, at);
                let (across, down) = (here_x - x, here_y - y);
                let step = across.hypot(down);
                [gone + if across < 0.0 { -step } else { step }, here_x, here_y]
            },
        );
        // Where it could not be read on a frame, it has gone no way that can be told.
        gone
    }

    /// What `next` makes of each frame from what it made of the one before, from what `first`
    /// makes of the first, on `frame`: kept by the fingerprint, so that a frame after the
    /// last asked for goes on from there, and one before is looked up.
    fn kept(&self, what: u64, frame: u32, first: impl Fn(&Work) -> [f64; 3], next: impl Fn(&Work, u32, [f64; 3]) -> [f64; 3]) -> [f64; 3] {
        let key = self.fingerprint.map(|fingerprint| (fingerprint, what));
        // Taken out while it is worked on: what it reads may keep tables of its own.
        let mut table = key.and_then(|key| KEPT.with(|kept| kept.borrow_mut().remove(&key))).unwrap_or_default();
        if table.is_empty() {
            table.push(first(self));
        }
        while table.len() <= frame as usize {
            let at = table.len() as u32;
            let made = next(self, at, table[table.len() - 1]);
            table.push(made);
        }
        let made = table[frame as usize];
        if let Some(key) = key {
            KEPT.with(|kept| {
                let mut kept = kept.borrow_mut();
                if kept.len() >= KEEPS {
                    kept.clear();
                }
                kept.insert(key, table);
            });
        }
        made
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// A scope of pegs whose values are worked out from the frame, and parameters held.
    #[derive(Default)]
    struct Pegs {
        pegs: HashMap<&'static str, Box<dyn Fn(Read, u32) -> f64>>,
        parameters: HashMap<&'static str, f64>,
    }

    impl Scope for Pegs {
        fn read(&self, input: &str, read: Read, frame: u32) -> Option<f64> {
            self.pegs.get(input).map(|peg| peg(read, frame))
        }
        fn parameter(&self, name: &str, _: u32) -> Option<f64> {
            self.parameters.get(name).copied()
        }
    }

    fn value(text: &str) -> f64 {
        value_in(&Pegs::default(), text, 0)
    }

    fn value_in(scope: &Pegs, text: &str, frame: u32) -> f64 {
        let expressions = Expressions::parse(&format!("x = {text}")).unwrap_or_else(|e| panic!("{text}: {e}"));
        let amounts = expressions.amounts(scope, frame);
        amounts.of(Driven::X).unwrap_or(f64::NAN)
    }

    fn fails(text: &str) -> ExprError {
        Expressions::parse(text).expect_err(text)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// A car keyed along `x(frame)` and `y(frame)`, in the scene as in its parent.
    fn car(x: fn(u32) -> f64, y: fn(u32) -> f64) -> Box<dyn Fn(Read, u32) -> f64> {
        Box::new(move |read, frame| match read {
            Read::Own(Driven::X) | Read::WorldX => x(frame),
            Read::Own(Driven::Y) | Read::WorldY => y(frame),
            Read::Own(Driven::ScaleX | Driven::ScaleY) | Read::WorldScale => 1.0,
            _ => 0.0,
        })
    }

    #[test]
    fn a_wheel_rolls_with_its_car_forwards_and_back() {
        let mut scope = Pegs::default();
        // Ten pixels a frame to the right for ten frames, then back.
        scope.pegs.insert("parent", car(|f| if f <= 10 { 10.0 * f64::from(f) } else { 100.0 - 10.0 * f64::from(f - 10) }, |_| 0.0));
        scope.parameters.insert("radius", 50.0);
        let wheel = Expressions::parse("rotation = parent.travelled / radius * 180 / PI").unwrap();
        let turn = |frame| wheel.amounts(&scope, frame).of(Driven::Rotation).unwrap();
        assert!(close(turn(0), 0.0));
        // A hundred pixels rolled on a wheel of radius 50 is two radians.
        assert!(close(turn(10), 2f64.to_degrees()));
        assert!(close(turn(15), 1f64.to_degrees()));
        assert!(close(turn(20), 0.0));
        assert_eq!(wheel.parameters().iter().collect::<Vec<_>>(), ["radius"]);
        assert!(wheel.inputs().is_empty());
    }

    #[test]
    fn up_a_slope_a_wheel_rolls_the_way_gone_not_the_way_across() {
        let mut scope = Pegs::default();
        scope.pegs.insert("parent", car(|f| 3.0 * f64::from(f), |f| -4.0 * f64::from(f)));
        assert!(close(value_in(&scope, "parent.travelled", 10), 50.0));
        assert!(close(value_in(&scope, "parent.x", 10), 30.0));
    }

    #[test]
    fn a_shadow_shrinks_as_the_character_jumps() {
        let mut scope = Pegs::default();
        scope.pegs.insert("char", car(|_| 40.0, |f| -30.0 * f64::from(f)));
        scope.parameters.insert("ground", 0.0);
        let shadow = Expressions::parse(
            "// The shadow under the character.\n\
             let height = abs(char.y - ground)\n\
             x = char.world.x; scaleX = remap(height, 0, 300, 0, -0.6)\n\
             scaleY = remap(height, 0, 300, 0, -0.6)",
        )
        .unwrap();
        let at = |frame| shadow.amounts(&scope, frame);
        assert_eq!(at(0).of(Driven::ScaleX), Some(0.0));
        assert!(close(at(5).of(Driven::ScaleY).unwrap(), -0.3));
        // Past the height it stays at its smallest.
        assert!(close(at(20).of(Driven::ScaleX).unwrap(), -0.6));
        assert_eq!(at(3).of(Driven::X), Some(40.0));
        assert_eq!(at(3).of(Driven::Rotation), None);
        assert_eq!(shadow.gives(), [Driven::X, Driven::ScaleX, Driven::ScaleY]);
        assert_eq!(shadow.inputs().iter().collect::<Vec<_>>(), ["char"]);
        assert_eq!(shadow.parameters().iter().collect::<Vec<_>>(), ["ground"]);
    }

    #[test]
    fn operators_bind_as_in_javascript() {
        assert_eq!(value("1 + 2 * 3"), 7.0);
        assert_eq!(value("(1 + 2) * 3"), 9.0);
        assert_eq!(value("10 - 4 - 3"), 3.0);
        assert_eq!(value("2 ** 3 ** 2"), 512.0);
        assert_eq!(value("-2 ** 2"), -4.0);
        assert_eq!(value("2 ** -1"), 0.5);
        assert_eq!(value("-7 % 3"), -1.0);
        assert_eq!(value("1 < 2 == 1"), 1.0);
        assert_eq!(value("1 + 1 > 1 && 0 || 5"), 5.0);
        assert_eq!(value("0 && 1 / 0"), 0.0);
        assert_eq!(value("!0 + !3"), 1.0);
        assert_eq!(value("2 > 1 ? 10 : 20"), 10.0);
        assert_eq!(value("0 ? 1 : 0 ? 2 : 3"), 3.0);
        assert_eq!(value("3 === 3"), 1.0);
        assert_eq!(value("1.5e2 + .5"), 150.5);
        assert_eq!(value("true + false"), 1.0);
    }

    #[test]
    fn every_function_works_out() {
        let pairs = [
            ("sin(PI / 2)", 1.0),
            ("Math.cos(0)", 1.0),
            ("tan(0)", 0.0),
            ("asin(1)", std::f64::consts::FRAC_PI_2),
            ("acos(1)", 0.0),
            ("atan(1)", std::f64::consts::FRAC_PI_4),
            ("atan2(1, 1)", std::f64::consts::FRAC_PI_4),
            ("sqrt(16)", 4.0),
            ("abs(-3)", 3.0),
            ("min(4, 2, 8)", 2.0),
            ("max(4, 2, 8)", 8.0),
            ("floor(-1.5)", -2.0),
            ("ceil(1.2)", 2.0),
            ("round(2.5)", 3.0),
            ("round(-2.5)", -2.0),
            ("sign(-4) + sign(0)", -1.0),
            ("pow(2, 10)", 1024.0),
            ("exp(0)", 1.0),
            ("log(E)", 1.0),
            ("clamp(5, 0, 3)", 3.0),
            ("lerp(10, 20, 0.25)", 12.5),
            ("remap(5, 0, 10, 100, 200)", 150.0),
            ("remap(-5, 0, 10, 100, 200)", 100.0),
            ("smoothstep(0, 1, 0.5)", 0.5),
            ("deg(PI)", 180.0),
            ("rad(180)", std::f64::consts::PI),
            ("Math.PI", std::f64::consts::PI),
        ];
        for (text, wanted) in pairs {
            assert!(close(value(text), wanted), "{text} is {}, not {wanted}", value(text));
        }
    }

    #[test]
    fn noise_is_smooth_bounded_and_the_same_each_time() {
        let samples: Vec<f64> = (0..400).map(|i| value(&format!("noise({}, 7)", f64::from(i) * 0.05))).collect();
        assert!(samples.iter().all(|v| (-1.0..=1.0).contains(v)));
        assert!(samples.windows(2).all(|pair| (pair[1] - pair[0]).abs() < 0.2), "steps are small");
        assert!(samples.iter().any(|v| *v > 0.3) && samples.iter().any(|v| *v < -0.3), "it moves about");
        assert_eq!(value("noise(3.3, 7)"), value("noise(3.3, 7)"));
        assert_ne!(value("noise(3.3, 7)"), value("noise(3.3, 8)"));
        assert_eq!(value("noise(2.5)"), value("noise(2.5, 0)"));
    }

    #[test]
    fn the_frame_is_numbered_as_the_timeline_numbers_it() {
        let scope = Pegs::default();
        assert_eq!(value_in(&scope, "frame", 0), 1.0);
        assert!(fails("x = time").message.contains("count in frames"));
    }

    #[test]
    fn at_works_out_on_another_frame_with_what_lets_name() {
        let mut scope = Pegs::default();
        scope.pegs.insert("car", car(|f| 10.0 * f64::from(f), |_| 0.0));
        assert_eq!(value_in(&scope, "at(car.x, frame - 3)", 10), 70.0);
        // Before the first frame is the first frame.
        assert_eq!(value_in(&scope, "at(car.x, frame - 30)", 10), 0.0);
        let speed = Expressions::parse("let place = car.x\nx = place - at(place, frame - 1)").unwrap();
        assert_eq!(speed.amounts(&scope, 6).of(Driven::X), Some(10.0));
    }

    #[test]
    fn a_spring_overshoots_and_settles() {
        let mut scope = Pegs::default();
        // A step from 0 to 100 at frame 5.
        scope.pegs.insert("car", car(|f| if f < 5 { 0.0 } else { 100.0 }, |_| 0.0));
        let text = "spring(car.x, 0.3, 0.2)";
        let path: Vec<f64> = (0..80).map(|frame| value_in(&scope, text, frame)).collect();
        assert_eq!(path[4], 0.0);
        assert!(path.iter().any(|v| *v > 105.0), "it overshoots");
        assert!((path[79] - 100.0).abs() < 0.5, "it settles: {}", path[79]);
    }

    #[test]
    fn a_line_given_twice_gives_its_last() {
        let expressions = Expressions::parse("x = 1\nx = 2").unwrap();
        assert_eq!(expressions.amounts(&Pegs::default(), 0).of(Driven::X), Some(2.0));
        assert_eq!(expressions.gives(), [Driven::X]);
    }

    #[test]
    fn a_long_line_goes_on_inside_brackets() {
        assert_eq!(value("(1 +\n 2)"), 3.0);
    }

    #[test]
    fn what_gives_no_number_gives_nothing_and_says_so() {
        let expressions = Expressions::parse("x = 1 / 0\ny = 2\nrotation = missing.x").unwrap();
        let amounts = expressions.amounts(&Pegs::default(), 0);
        assert_eq!(amounts.of(Driven::X), None);
        assert_eq!(amounts.of(Driven::Y), Some(2.0));
        assert_eq!(amounts.of(Driven::Rotation), None);
        assert!(amounts.failed);
        assert!(!Expressions::parse("y = 2").unwrap().amounts(&Pegs::default(), 0).failed);
    }

    #[test]
    fn errors_say_where_and_why() {
        let e = fails("x = 1 +");
        assert_eq!((e.at.line, e.at.column), (1, 8));
        let e = fails("x = 2\ny = (3 * 4");
        assert_eq!(e.at.line, 2);
        assert!(e.message.contains("`)`"), "{}", e.message);
        let e = fails("x = 1\n  height = 3");
        assert_eq!((e.at.line, e.at.column), (2, 3));
        assert!(e.message.contains("`height`"), "{}", e.message);
        assert!(fails("x = sinn(1)").message.contains("no function `sinn`"));
        assert!(fails("x = clamp(1, 2)").message.contains("takes 3"));
        assert!(fails("x = car.colour").message.contains("not `colour`"));
        assert!(fails("x = car.world.z").message.contains("not `z`"));
        assert!(fails("x = rotation").message.contains("own.rotation"));
        assert!(fails("x = parent").message.contains("parent.x"));
        assert!(fails("x = sin").message.contains("function"));
        assert!(fails("let frame = 3").message.contains("take another"));
        assert!(fails("x = 1 2").message.contains("should end"));
        assert!(fails("x = 3 # 4").message.contains("`#`"));
        assert_eq!(fails("x = 1\ny").to_string(), "line 2, column 2: `=` is wanted after `y`, to give it an amount");
    }

    #[test]
    fn text_from_a_file_that_does_not_parse_is_kept_and_gives_nothing() {
        let kept = Expressions::read("x = (");
        assert_eq!(kept.text(), "x = (");
        assert!(kept.error().is_some());
        assert!(kept.gives().is_empty());
        let json = serde_json::to_string(&kept).unwrap();
        assert_eq!(json, "\"x = (\"");
        let back: Expressions = serde_json::from_str("\"rotation = frame * 2\"").unwrap();
        assert_eq!(back.amounts(&Pegs::default(), 4).of(Driven::Rotation), Some(10.0));
        assert!(Expressions::default().gives().is_empty());
    }

    #[test]
    fn nesting_too_deep_gives_nothing_rather_than_taking_forever() {
        let text = "at(at(at(at(at(1, 1), 1), 1), 1), 1)";
        assert!(value(text).is_nan());
        assert_eq!(value("at(at(at(at(1, 1), 1), 1), 1)"), 1.0);
    }

    #[test]
    fn reserved_names_are_the_languages() {
        for name in ["frame", "rotation", "sin", "parent", "own", "spring", "PI"] {
            assert!(reserved(name), "{name}");
        }
        assert!(!reserved("radius"));
    }

    #[test]
    fn an_input_or_a_parameter_renamed_is_renamed_where_it_is_read() {
        let text = "// car is read here\nlet car2 = car.x + carry\nx = car.world.x * carry + Math.max(carry, 1)\nrotation = cos(car . y)";
        let expressions = Expressions::parse(text).unwrap();
        let renamed = expressions.renamed("car", "truck", true).unwrap();
        assert_eq!(
            renamed.text(),
            "// car is read here\nlet car2 = truck.x + carry\nx = truck.world.x * carry + Math.max(carry, 1)\nrotation = cos(truck . y)"
        );
        assert_eq!(renamed.inputs().iter().collect::<Vec<_>>(), ["truck"]);
        let renamed = expressions.renamed("carry", "load", false).unwrap();
        assert!(renamed.text().contains("car.x + load") && renamed.text().contains("Math.max(load, 1)"), "{}", renamed.text());
        // A parameter named as a value of an input is not one.
        let expressions = Expressions::parse("x = car.x + x2").unwrap();
        assert_eq!(expressions.renamed("x", "y2", false).unwrap().text(), "x = car.x + x2");
        // A new name the language has is refused, as the text it makes does not parse.
        assert!(Expressions::parse("x = k").unwrap().renamed("k", "frame", false).is_ok_and(|e| e.parameters().is_empty()));
        assert!(Expressions::parse("x = k").unwrap().renamed("k", "sin", false).is_err());
    }
}
