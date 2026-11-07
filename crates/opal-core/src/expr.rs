//! Expressions: the small language a number or a length may be given in, typed into a
//! field or passed to the API (`design.md`, *Expressions*).
//!
//! An expression is parsed into a tree ([`Expr`]) and worked out against a [`Scope`]: the
//! image it may name the canvas and layers of, the resolution lengths are converted at, the
//! unit a bare number in a length is in, the point positions are measured from, and the box
//! being placed, if there is one. Lengths come out in pixels, angles in degrees.
//!
//! A value has a kind: a length (to any whole power: an area is a length squared), an
//! angle, a plain number, a point, or the bounds of the canvas or a layer. A *bare* number,
//! one typed without a unit, takes the kind of what it is added to (`width + 10` adds ten
//! of the field's unit), and is a plain factor when it multiplies or divides; a bare
//! number left as the whole result is in the field's unit.

use crate::document::{Document, Layer, LayerId};
use crate::units::Unit;
use std::fmt::Write as _;
use std::ops::Range;
use std::sync::Arc;

/// Why an expression could not be read or worked out: what is wrong, and where, as the
/// byte range of the text at fault.
#[derive(Clone, Debug, PartialEq)]
pub struct Error {
    pub at: Range<usize>,
    pub message: String,
}

impl Error {
    fn new(at: Range<usize>, message: impl Into<String>) -> Error {
        Error { at, message: message.into() }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// What a field or a parameter takes, and so what an expression must come to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// A length, worked out in pixels.
    Length,
    /// An angle, worked out in degrees.
    Angle,
    /// A plain number.
    Number,
    /// A number of pixels: a plain number, or a length (a bare number is in pixels then,
    /// whatever the scope's unit).
    Pixels,
}

/// The box being placed, as its fields show it: its position that of its reference point,
/// from the scope's origin, and its size, in pixels; its angle in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Own {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub angle: f64,
}

/// What an expression is worked out against.
#[derive(Clone, Copy)]
pub struct Scope<'a> {
    /// The image whose canvas and layers it may name, if there is one.
    pub doc: Option<&'a Document>,
    /// Pixels per inch, for lengths in other units.
    pub ppi: f64,
    /// The unit a bare number in a length is in.
    pub unit: Unit,
    /// The point positions are measured from, in document pixels.
    pub origin: [f64; 2],
    /// The box being placed, if there is one.
    pub own: Option<Own>,
}

impl Scope<'_> {
    /// A scope of no image and no box: lengths in `unit` at `ppi`, measured from (0, 0).
    pub fn bare(unit: Unit, ppi: f64) -> Scope<'static> {
        Scope { doc: None, ppi, unit, origin: [0.0, 0.0], own: None }
    }
}

/// A parsed expression.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    root: Node,
}

/// A layer an expression names: by its name as typed, or by its id (`layer(12)`, and every
/// name once [bound](Expr::bind)).
#[derive(Clone, Debug, PartialEq)]
pub enum LayerRef {
    Name(String),
    Id(LayerId),
}

#[derive(Clone, Debug, PartialEq)]
struct Node {
    kind: NodeKind,
    /// The text it was read from (empty for one made, not read).
    at: Range<usize>,
    /// How many nodes deep it goes, itself included: at most [`MAX_DEPTH`].
    depth: usize,
}

/// The longest text read as an expression, in bytes.
pub const MAX_LEN: usize = 4096;
/// The deepest an expression may nest: what working it out recurses through.
const MAX_DEPTH: usize = 64;

impl Node {
    fn leaf(kind: NodeKind, at: Range<usize>) -> Node {
        Node { kind, at, depth: 1 }
    }

    /// A node of `kind` read from `at`, unless it nests deeper than [`MAX_DEPTH`].
    fn made(kind: NodeKind, at: Range<usize>) -> Result<Node, Error> {
        let below = match &kind {
            NodeKind::Member(a, _) | NodeKind::Neg(a) => a.depth,
            NodeKind::Bin(_, a, b) => a.depth.max(b.depth),
            NodeKind::Call(_, args) => args.iter().map(|a| a.depth).max().unwrap_or(0),
            _ => 0,
        };
        if below >= MAX_DEPTH {
            return Err(Error::new(at, format!("This nests more than {MAX_DEPTH} deep: write it more simply")));
        }
        Ok(Node { kind, at, depth: below + 1 })
    }
}

#[derive(Clone, Debug, PartialEq)]
enum NodeKind {
    Number {
        value: f64,
        suffix: Option<Suffix>,
    },
    /// A word of the language (`width`, `canvas`) or a layer's name.
    Word(String),
    Layer(LayerRef),
    Member(Box<Node>, String),
    Call(Func, Vec<Node>),
    Neg(Box<Node>),
    Bin(Op, Box<Node>, Box<Node>),
}

/// What a number is typed in.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Suffix {
    Length(Unit),
    Degrees,
    Radians,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
}

impl Op {
    /// How tightly it binds: a higher one first.
    fn precedence(self) -> u8 {
        match self {
            Op::Add | Op::Sub => 1,
            Op::Mul | Op::Div => 2,
            Op::Pow => 4,
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Op::Add => "+",
            Op::Sub => "-",
            Op::Mul => "*",
            Op::Div => "/",
            Op::Pow => "^",
        }
    }
}

/// The functions, with how many arguments each takes (`None`: any number from the least).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Func {
    Min,
    Max,
    Abs,
    Round,
    Floor,
    Ceil,
    Sqrt,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Mid,
    Lerp,
    Dist,
    Point,
}

const FUNCS: [(Func, &str, usize, Option<usize>); 18] = [
    (Func::Min, "min", 1, None),
    (Func::Max, "max", 1, None),
    (Func::Abs, "abs", 1, Some(1)),
    (Func::Round, "round", 1, Some(2)),
    (Func::Floor, "floor", 1, Some(2)),
    (Func::Ceil, "ceil", 1, Some(2)),
    (Func::Sqrt, "sqrt", 1, Some(1)),
    (Func::Sin, "sin", 1, Some(1)),
    (Func::Cos, "cos", 1, Some(1)),
    (Func::Tan, "tan", 1, Some(1)),
    (Func::Asin, "asin", 1, Some(1)),
    (Func::Acos, "acos", 1, Some(1)),
    (Func::Atan, "atan", 1, Some(1)),
    (Func::Atan2, "atan2", 2, Some(2)),
    (Func::Mid, "mid", 2, Some(2)),
    (Func::Lerp, "lerp", 3, Some(3)),
    (Func::Dist, "dist", 2, Some(2)),
    (Func::Point, "point", 2, Some(2)),
];

impl Func {
    fn named(name: &str) -> Option<Func> {
        FUNCS.iter().find(|f| f.1 == name).map(|f| f.0)
    }

    fn name(self) -> &'static str {
        FUNCS.iter().find(|f| f.0 == self).expect("every function is listed").1
    }

    fn arity(self) -> (usize, Option<usize>) {
        let f = FUNCS.iter().find(|f| f.0 == self).expect("every function is listed");
        (f.2, f.3)
    }
}

/// The words of the language: names a layer called so must be quoted to be named by.
const WORDS: [&str; 9] = ["x", "y", "width", "height", "angle", "ppi", "pi", "canvas", "layer"];

/// What the canvas's and layers' bounds have, besides their nine points' coordinates.
const EDGES: [&str; 8] = ["left", "right", "top", "bottom", "width", "height", "x", "y"];
/// The points the bounds have.
const POINTS: [(&str, [f64; 2]); 5] = [
    ("top_left", [0.0, 0.0]),
    ("top_right", [1.0, 0.0]),
    ("center", [0.5, 0.5]),
    ("bottom_left", [0.0, 1.0]),
    ("bottom_right", [1.0, 1.0]),
];

// ---- Reading --------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Str(String),
    Op(Op),
    Open,
    Close,
    Comma,
    Dot,
    End,
}

/// Splits `text` into tokens, each with the text it was read from.
fn tokens(text: &str) -> Result<Vec<(Tok, Range<usize>)>, Error> {
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some(&(i, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let one = |t: Tok| (t, i..i + c.len_utf8());
        if c.is_ascii_digit() || (c == '.' && text[i + 1..].starts_with(|d: char| d.is_ascii_digit())) {
            let mut end = i;
            let mut seen_dot = false;
            while let Some(&(j, d)) = chars.peek() {
                if d.is_ascii_digit() || (d == '.' && !seen_dot) {
                    seen_dot |= d == '.';
                    end = j + 1;
                    chars.next();
                } else {
                    break;
                }
            }
            // An exponent: "e", a sign perhaps, and a digit (else the "e" starts a word).
            let rest = &text[end..];
            if let Some(after) = rest.strip_prefix(['e', 'E']) {
                let digits = after.trim_start_matches(['+', '-']);
                if digits.starts_with(|d: char| d.is_ascii_digit()) {
                    let sign = after.len() - digits.len();
                    let n = digits.chars().take_while(char::is_ascii_digit).count();
                    let stop = end + 1 + sign + n;
                    while chars.peek().is_some_and(|&(j, _)| j < stop) {
                        chars.next();
                    }
                    end = stop;
                }
            }
            let value: f64 =
                text[i..end].parse().map_err(|_| Error::new(i..end, format!("“{}” is not a number", &text[i..end])))?;
            out.push((Tok::Num(value), i..end));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let mut end = i;
            while let Some(&(j, d)) = chars.peek() {
                if d.is_alphanumeric() || d == '_' {
                    end = j + d.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            out.push((Tok::Ident(text[i..end].to_owned()), i..end));
            continue;
        }
        if c == '"' {
            chars.next();
            let mut s = String::new();
            let mut closed = None;
            while let Some((j, d)) = chars.next() {
                match d {
                    '"' => {
                        closed = Some(j + 1);
                        break;
                    }
                    '\\' => match chars.next() {
                        Some((_, e @ ('"' | '\\'))) => s.push(e),
                        Some((k, e)) => {
                            return Err(Error::new(j..k + e.len_utf8(), "Only \\\" and \\\\ may follow a backslash"));
                        }
                        None => break,
                    },
                    d => s.push(d),
                }
            }
            let Some(end) = closed else {
                return Err(Error::new(i..text.len(), "This text has no closing quote"));
            };
            out.push((Tok::Str(s), i..end));
            continue;
        }
        chars.next();
        let tok = match c {
            '+' => one(Tok::Op(Op::Add)),
            '-' | '−' => one(Tok::Op(Op::Sub)),
            '*' if chars.peek().is_some_and(|&(_, d)| d == '*') => {
                chars.next();
                (Tok::Op(Op::Pow), i..i + 2)
            }
            '*' | '×' => one(Tok::Op(Op::Mul)),
            '/' | '÷' => one(Tok::Op(Op::Div)),
            '^' => one(Tok::Op(Op::Pow)),
            '(' => one(Tok::Open),
            ')' => one(Tok::Close),
            ',' => one(Tok::Comma),
            '.' => one(Tok::Dot),
            '°' => one(Tok::Ident("deg".to_owned())),
            _ => return Err(Error::new(i..i + c.len_utf8(), format!("“{c}” has no meaning here"))),
        };
        out.push(tok);
    }
    out.push((Tok::End, text.len()..text.len()));
    Ok(out)
}

/// A unit a number may be followed by.
fn suffix(word: &str) -> Option<Suffix> {
    match word.to_ascii_lowercase().as_str() {
        "deg" | "degree" | "degrees" => Some(Suffix::Degrees),
        "rad" | "radian" | "radians" => Some(Suffix::Radians),
        _ => Unit::from_name(word).map(Suffix::Length),
    }
}

struct Parser {
    toks: Vec<(Tok, Range<usize>)>,
    at: usize,
    /// How deep reading has recursed, through signs, parentheses and arguments.
    nesting: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.at].0
    }

    fn span(&self) -> Range<usize> {
        self.toks[self.at].1.clone()
    }

    fn next(&mut self) -> (Tok, Range<usize>) {
        let t = self.toks[self.at].clone();
        if self.at + 1 < self.toks.len() {
            self.at += 1;
        }
        t
    }

    fn sum(&mut self) -> Result<Node, Error> {
        let mut left = self.product()?;
        while let Tok::Op(op @ (Op::Add | Op::Sub)) = *self.peek() {
            self.next();
            let right = self.product()?;
            left = bin(op, left, right)?;
        }
        Ok(left)
    }

    fn product(&mut self) -> Result<Node, Error> {
        let mut left = self.unary()?;
        while let Tok::Op(op @ (Op::Mul | Op::Div)) = *self.peek() {
            self.next();
            let right = self.unary()?;
            left = bin(op, left, right)?;
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Node, Error> {
        // Every way reading recurses comes through here.
        self.nesting += 1;
        if self.nesting > MAX_DEPTH {
            return Err(Error::new(
                self.span(),
                format!("This nests more than {MAX_DEPTH} deep: write it more simply"),
            ));
        }
        let node = match *self.peek() {
            Tok::Op(Op::Sub) => {
                let start = self.next().1.start;
                let inner = self.unary()?;
                let at = start..inner.at.end;
                Node::made(NodeKind::Neg(Box::new(inner)), at)
            }
            Tok::Op(Op::Add) => {
                self.next();
                self.unary()
            }
            _ => self.power(),
        };
        self.nesting -= 1;
        node
    }

    fn power(&mut self) -> Result<Node, Error> {
        let base = self.postfix()?;
        if *self.peek() == Tok::Op(Op::Pow) {
            self.next();
            // Right to left, and taking a sign: 2^-1, 2^3^2 = 2^9.
            let exponent = self.unary()?;
            return bin(Op::Pow, base, exponent);
        }
        Ok(base)
    }

    fn postfix(&mut self) -> Result<Node, Error> {
        let mut node = self.primary()?;
        while *self.peek() == Tok::Dot {
            self.next();
            let (tok, at) = self.next();
            let Tok::Ident(name) = tok else {
                return Err(Error::new(at, "A name must follow the “.”, such as .left or .center"));
            };
            let span = node.at.start..at.end;
            node = Node::made(NodeKind::Member(Box::new(node), name), span)?;
        }
        Ok(node)
    }

    fn primary(&mut self) -> Result<Node, Error> {
        let (tok, at) = self.next();
        match tok {
            Tok::Num(value) => {
                // A unit after it, unless that word is something else ("2 m" is two meters).
                if let Tok::Ident(word) = self.peek()
                    && let Some(s) = suffix(word)
                {
                    let end = self.next().1.end;
                    return Ok(Node::leaf(NodeKind::Number { value, suffix: Some(s) }, at.start..end));
                }
                Ok(Node::leaf(NodeKind::Number { value, suffix: None }, at))
            }
            Tok::Ident(name) if *self.peek() == Tok::Open => {
                self.next();
                let mut args = Vec::new();
                if *self.peek() != Tok::Close {
                    loop {
                        args.push(self.argument(&name)?);
                        if *self.peek() == Tok::Comma {
                            self.next();
                        } else {
                            break;
                        }
                    }
                }
                let close = self.span();
                if self.next().0 != Tok::Close {
                    return Err(Error::new(close, format!("“)” is missing, to close {name}(")));
                }
                let span = at.start..close.end;
                call(&name, args, span, at)
            }
            Tok::Ident(name) => Ok(Node::leaf(NodeKind::Word(name), at)),
            Tok::Open => {
                let inner = self.sum()?;
                let close = self.span();
                if self.next().0 != Tok::Close {
                    return Err(Error::new(close, "“)” is missing"));
                }
                Ok(Node { at: at.start..close.end, ..inner })
            }
            Tok::Str(_) => Err(Error::new(at, "Text in quotes names a layer: layer(\"Its name\")")),
            Tok::End => Err(Error::new(at, "Something is missing at the end")),
            _ => Err(Error::new(at, "A number, a name or “(” should be here")),
        }
    }

    /// One argument of function `name`: an expression, or for `layer`, a name in quotes or
    /// an id.
    fn argument(&mut self, name: &str) -> Result<Node, Error> {
        if name == "layer"
            && let Tok::Str(s) = self.peek().clone()
        {
            let at = self.next().1;
            return Ok(Node::leaf(NodeKind::Layer(LayerRef::Name(s)), at));
        }
        self.sum()
    }
}

fn bin(op: Op, a: Node, b: Node) -> Result<Node, Error> {
    let at = a.at.start..b.at.end;
    Node::made(NodeKind::Bin(op, Box::new(a), Box::new(b)), at)
}

/// The call `name(args)` read from `span` (the name itself at `name_at`).
fn call(name: &str, mut args: Vec<Node>, span: Range<usize>, name_at: Range<usize>) -> Result<Node, Error> {
    if name == "layer" {
        let arg = match (args.pop(), args.is_empty()) {
            (Some(a), true) => a,
            _ => return Err(Error::new(span, "layer( ) takes one thing: a name in quotes, or an id")),
        };
        let r = match arg.kind {
            NodeKind::Layer(r) => r,
            NodeKind::Number { value, suffix: None }
                if value >= 0.0 && value.fract() == 0.0 && value < 2f64.powi(53) =>
            {
                LayerRef::Id(LayerId(value as u64))
            }
            _ => {
                return Err(Error::new(arg.at, "A layer is named in quotes, layer(\"Logo\"), or by its id, layer(12)"));
            }
        };
        return Ok(Node::leaf(NodeKind::Layer(r), span));
    }
    let Some(f) = Func::named(name) else {
        let known: Vec<&str> = FUNCS.iter().map(|f| f.1).collect();
        return Err(Error::new(name_at, format!("There is no function “{name}” (there are {})", known.join(", "))));
    };
    let (least, most) = f.arity();
    let n = args.len();
    if n < least || most.is_some_and(|m| n > m) {
        let takes = match most {
            Some(m) if m == least => format!("{least}"),
            Some(m) => format!("{least} or {m}"),
            None => format!("at least {least}"),
        };
        let s = if takes == "1" { "" } else { "s" };
        return Err(Error::new(span, format!("{name}( ) takes {takes} argument{s}, not {n}")));
    }
    Node::made(NodeKind::Call(f, args), span)
}

impl Expr {
    /// Reads `text`.
    pub fn parse(text: &str) -> Result<Expr, Error> {
        if text.len() > MAX_LEN {
            return Err(Error::new(
                0..text.len(),
                format!("This is longer than an expression may be ({MAX_LEN} bytes)"),
            ));
        }
        let toks = tokens(text)?;
        let mut p = Parser { toks, at: 0, nesting: 0 };
        if *p.peek() == Tok::End {
            return Err(Error::new(0..text.len(), "There is nothing here"));
        }
        let root = p.sum()?;
        if *p.peek() != Tok::End {
            let at = p.span();
            return Err(Error::new(at, "An operator (such as + or *) should be here"));
        }
        Ok(Expr { root })
    }

    /// Whether it is no more than a number, with a unit or not.
    pub fn is_number(&self) -> bool {
        matches!(self.root.kind, NodeKind::Number { .. })
    }

    /// What it comes to in `scope`, as `want` says: a length in pixels, an angle in
    /// degrees, or a number.
    pub fn eval(&self, scope: &Scope, want: Want) -> Result<f64, Error> {
        let v = eval(&self.root, scope)?;
        finish(v, want, scope, &self.root.at)
    }

    /// The same, its layers named by their ids, so that it names them still after a layer
    /// is renamed, or another is given the name. A layer it names that `doc` has not is an
    /// error.
    pub fn bind(&self, doc: &Document) -> Result<Expr, Error> {
        fn walk(n: &Node, doc: &Document) -> Result<Node, Error> {
            let kind = match &n.kind {
                NodeKind::Word(w) if !WORDS.contains(&w.as_str()) => {
                    NodeKind::Layer(LayerRef::Id(layer_named(doc, w, &n.at)?.id))
                }
                NodeKind::Layer(LayerRef::Name(name)) => {
                    NodeKind::Layer(LayerRef::Id(layer_named(doc, name, &n.at)?.id))
                }
                NodeKind::Layer(LayerRef::Id(id)) => {
                    if doc.layer(*id).is_none() {
                        return Err(Error::new(n.at.clone(), format!("There is no layer {}", id.0)));
                    }
                    n.kind.clone()
                }
                NodeKind::Member(a, m) => NodeKind::Member(Box::new(walk(a, doc)?), m.clone()),
                NodeKind::Call(f, args) => {
                    NodeKind::Call(*f, args.iter().map(|a| walk(a, doc)).collect::<Result<_, _>>()?)
                }
                NodeKind::Neg(a) => NodeKind::Neg(Box::new(walk(a, doc)?)),
                NodeKind::Bin(op, a, b) => NodeKind::Bin(*op, Box::new(walk(a, doc)?), Box::new(walk(b, doc)?)),
                k => k.clone(),
            };
            Ok(Node { kind, at: n.at.clone(), depth: n.depth })
        }
        Ok(Expr { root: walk(&self.root, doc)? })
    }

    /// The layers it names by id.
    pub fn layers(&self) -> Vec<LayerId> {
        fn walk(n: &Node, out: &mut Vec<LayerId>) {
            match &n.kind {
                NodeKind::Layer(LayerRef::Id(id)) if !out.contains(id) => out.push(*id),
                NodeKind::Member(a, _) | NodeKind::Neg(a) => walk(a, out),
                NodeKind::Call(_, args) => args.iter().for_each(|a| walk(a, out)),
                NodeKind::Bin(_, a, b) => {
                    walk(a, out);
                    walk(b, out);
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    /// Its text, a layer named by id written by the name `name_of` gives it (as a word if
    /// it can be one, else in quotes), or by its id if that gives none. With names it reads
    /// as the user would type it; with ids (`|_| None`) it is the form to keep, which reads
    /// back to the same expression whatever the layers are called.
    pub fn text(&self, name_of: &dyn Fn(LayerId) -> Option<String>) -> String {
        let mut out = String::new();
        write(&self.root, name_of, 0, &mut out);
        out
    }
}

/// Writes `n` into `out`, in parentheses if it binds less tightly than `outer`.
fn write(n: &Node, name_of: &dyn Fn(LayerId) -> Option<String>, outer: u8, out: &mut String) {
    match &n.kind {
        NodeKind::Number { value, suffix } => {
            let _ = write!(out, "{value}");
            match suffix {
                Some(Suffix::Length(u)) => out.push_str(u.id()),
                Some(Suffix::Degrees) => out.push_str("deg"),
                Some(Suffix::Radians) => out.push_str("rad"),
                None => {}
            }
        }
        NodeKind::Word(w) => out.push_str(w),
        NodeKind::Layer(LayerRef::Name(name)) => out.push_str(&layer_text(name)),
        NodeKind::Layer(LayerRef::Id(id)) => match name_of(*id) {
            Some(name) => out.push_str(&layer_text(&name)),
            None => {
                let _ = write!(out, "layer({})", id.0);
            }
        },
        NodeKind::Member(a, m) => {
            write(a, name_of, 5, out);
            out.push('.');
            out.push_str(m);
        }
        NodeKind::Call(f, args) => {
            out.push_str(f.name());
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write(a, name_of, 0, out);
            }
            out.push(')');
        }
        NodeKind::Neg(a) => {
            let wrap = outer > 3;
            if wrap {
                out.push('(');
            }
            out.push('-');
            write(a, name_of, 3, out);
            if wrap {
                out.push(')');
            }
        }
        NodeKind::Bin(op, a, b) => {
            let p = op.precedence();
            let wrap = p < outer;
            if wrap {
                out.push('(');
            }
            // Left to right, but for powers, which group from the right.
            let (pa, pb) = if *op == Op::Pow { (p + 1, p) } else { (p, p + 1) };
            write(a, name_of, pa, out);
            if *op == Op::Pow {
                out.push('^');
            } else {
                let _ = write!(out, " {} ", op.symbol());
            }
            write(b, name_of, pb, out);
            if wrap {
                out.push(')');
            }
        }
    }
}

/// How a layer called `name` is written: as a word if it can be one, else `layer("…")`.
fn layer_text(name: &str) -> String {
    let word = name.starts_with(|c: char| c.is_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !WORDS.contains(&name)
        && suffix(name).is_none();
    if word { name.to_owned() } else { format!("layer(\"{}\")", name.replace('\\', "\\\\").replace('"', "\\\"")) }
}

/// Every layer of `layers` and of the groups in them.
fn all_layers(layers: &[Arc<Layer>], out: &mut Vec<Arc<Layer>>) {
    for l in layers {
        out.push(l.clone());
        if let Some(c) = l.children() {
            all_layers(c, out);
        }
    }
}

/// The one layer of `doc` called `name`: exactly so, or else, if just one is, so in
/// another case.
fn layer_named(doc: &Document, name: &str, at: &Range<usize>) -> Result<Arc<Layer>, Error> {
    let mut all = Vec::new();
    all_layers(&doc.layers, &mut all);
    let exact: Vec<_> = all.iter().filter(|l| l.name == name).collect();
    let found: Vec<_> = if exact.is_empty() {
        all.iter().filter(|l| l.name.to_lowercase() == name.to_lowercase()).collect()
    } else {
        exact
    };
    match found.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(Error::new(
            at.clone(),
            format!("There is no layer called “{name}”, and “{name}” is not a word the language has"),
        )),
        _ => Err(Error::new(at.clone(), format!("{} layers are called “{name}”: rename all but one", found.len()))),
    }
}

// ---- Working out ----------------------------------------------------------------------

/// A value: a quantity, a point, or bounds (left, top, right, bottom), positions measured
/// from the scope's origin, in pixels.
#[derive(Clone, Copy, Debug)]
enum Val {
    Q(Q),
    Point([f64; 2]),
    Bounds([f64; 4]),
}

/// A quantity: `v` in pixels to the power `len`, and degrees to the power `ang`; `bare` if
/// it was typed without a unit and has not yet taken one.
#[derive(Clone, Copy, Debug)]
struct Q {
    v: f64,
    len: i32,
    ang: i32,
    bare: bool,
}

impl Q {
    fn plain(v: f64) -> Q {
        Q { v, len: 0, ang: 0, bare: false }
    }

    fn bare(v: f64) -> Q {
        Q { v, len: 0, ang: 0, bare: true }
    }

    fn length(v: f64) -> Q {
        Q { v, len: 1, ang: 0, bare: false }
    }

    fn angle(v: f64) -> Q {
        Q { v, len: 0, ang: 1, bare: false }
    }

    fn same_kind(&self, o: &Q) -> bool {
        self.len == o.len && self.ang == o.ang
    }

    /// What kind it is, in words: "a length", "an angle", "an area".
    fn kind(&self) -> String {
        match (self.len, self.ang) {
            (0, 0) => "a number".to_owned(),
            (1, 0) => "a length".to_owned(),
            (2, 0) => "an area".to_owned(),
            (0, 1) => "an angle".to_owned(),
            (l, 0) => format!("a length to the power {l}"),
            (0, a) => format!("an angle to the power {a}"),
            (l, a) => format!("a length to the power {l} times an angle to the power {a}"),
        }
    }
}

fn kind_of(v: &Val) -> String {
    match v {
        Val::Q(q) => q.kind(),
        Val::Point(_) => "a point".to_owned(),
        Val::Bounds(_) => "a layer's bounds".to_owned(),
    }
}

/// `q`, a bare number, given the kind of `like` (a length in the scope's unit, an angle in
/// degrees, or a plain number), to be combined with it.
fn adapt(q: Q, like: &Q, scope: &Scope, at: &Range<usize>) -> Result<Q, Error> {
    match (like.len, like.ang) {
        (1, 0) => Ok(Q::length(scope.unit.to_px(q.v, scope.ppi))),
        (0, 1) => Ok(Q::angle(q.v)),
        (0, 0) => Ok(Q::plain(q.v)),
        _ => Err(Error::new(
            at.clone(),
            format!("A number without a unit cannot be taken with {}: give it a unit", like.kind()),
        )),
    }
}

/// `a` and `b` of one kind, a bare one taking the other's: for adding, comparing, and the
/// like (`what` says which, for an error).
fn unify(a: Q, b: Q, scope: &Scope, at: &Range<usize>, what: &str) -> Result<(Q, Q), Error> {
    let (a, b) = match (a.bare, b.bare) {
        (true, true) | (false, false) => (a, b),
        (true, false) => (adapt(a, &b, scope, at)?, b),
        (false, true) => (a, adapt(b, &a, scope, at)?),
    };
    if a.same_kind(&b) {
        Ok((a, b))
    } else {
        Err(Error::new(at.clone(), format!("{} cannot be {what} {}", capital(&a.kind()), b.kind())))
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

fn quantity(v: Val, at: &Range<usize>) -> Result<Q, Error> {
    match v {
        Val::Q(q) => Ok(q),
        Val::Point(_) => Err(Error::new(at.clone(), "This is a point: give its .x or .y")),
        Val::Bounds(_) => Err(Error::new(
            at.clone(),
            "This is a layer, or the canvas: give one of its edges or points, such as .left or .center.x",
        )),
    }
}

fn point(v: Val, at: &Range<usize>) -> Result<[f64; 2], Error> {
    match v {
        Val::Point(p) => Ok(p),
        Val::Bounds(_) => {
            Err(Error::new(at.clone(), "This is a layer, or the canvas: give one of its points, such as .center"))
        }
        v => Err(Error::new(at.clone(), format!("This is {}, not a point", kind_of(&v)))),
    }
}

/// `q` as a length in pixels: a bare number in the scope's unit.
fn as_length(q: Q, scope: &Scope, at: &Range<usize>) -> Result<f64, Error> {
    let q = if q.bare { adapt(q, &Q::length(0.0), scope, at)? } else { q };
    if (q.len, q.ang) == (1, 0) {
        Ok(q.v)
    } else {
        Err(Error::new(at.clone(), format!("This is {}, not a length", q.kind())))
    }
}

/// `q` as a plain number.
fn as_number(q: Q, at: &Range<usize>) -> Result<f64, Error> {
    if (q.len, q.ang) == (0, 0) {
        Ok(q.v)
    } else {
        Err(Error::new(at.clone(), format!("This is {}, not a number", q.kind())))
    }
}

/// `q` as an angle in degrees: a bare number in degrees.
fn as_angle(q: Q, at: &Range<usize>) -> Result<f64, Error> {
    if q.bare || (q.len, q.ang) == (0, 1) {
        Ok(q.v)
    } else {
        Err(Error::new(at.clone(), format!("This is {}, not an angle", q.kind())))
    }
}

/// What `v` comes to, as `want` says.
fn finish(v: Val, want: Want, scope: &Scope, at: &Range<usize>) -> Result<f64, Error> {
    let q = quantity(v, at)?;
    let out = match want {
        Want::Length => as_length(q, scope, at),
        Want::Angle => as_angle(q, at),
        Want::Number => as_number(q, at),
        Want::Pixels if (q.len, q.ang) == (1, 0) => Ok(q.v),
        Want::Pixels => as_number(q, at).map_err(|e| Error { message: format!("{}, nor a length", e.message), ..e }),
    }
    .map_err(|e| Error { message: format!("{}, so it cannot go here", e.message), ..e })?;
    if out.is_finite() { Ok(out) } else { Err(Error::new(at.clone(), "This comes to no finite number")) }
}

fn eval(n: &Node, scope: &Scope) -> Result<Val, Error> {
    let at = &n.at;
    let v = match &n.kind {
        NodeKind::Number { value, suffix } => Val::Q(match suffix {
            None => Q::bare(*value),
            Some(Suffix::Length(u)) => Q::length(u.to_px(*value, scope.ppi)),
            Some(Suffix::Degrees) => Q::angle(*value),
            Some(Suffix::Radians) => Q::angle(value.to_degrees()),
        }),
        NodeKind::Word(w) => word(w, scope, at)?,
        NodeKind::Layer(r) => {
            let doc =
                scope.doc.ok_or_else(|| Error::new(at.clone(), "No image is open, so there are no layers to name"))?;
            match r {
                LayerRef::Name(name) => {
                    let layer = layer_named(doc, name, at)?;
                    layer_bounds(&layer, scope, at)?
                }
                LayerRef::Id(id) => {
                    let l =
                        doc.layer(*id).ok_or_else(|| Error::new(at.clone(), format!("There is no layer {}", id.0)))?;
                    layer_bounds(l, scope, at)?
                }
            }
        }
        NodeKind::Member(a, m) => member(eval(a, scope)?, m, at)?,
        NodeKind::Neg(a) => match eval(a, scope)? {
            Val::Q(q) => Val::Q(Q { v: -q.v, ..q }),
            v => return Err(Error::new(at.clone(), format!("{} cannot be negated", capital(&kind_of(&v))))),
        },
        NodeKind::Bin(op, a, b) => {
            let (va, vb) = (eval(a, scope)?, eval(b, scope)?);
            let (Val::Q(x), Val::Q(y)) = (va, vb) else {
                let which = if matches!(va, Val::Q(_)) { vb } else { va };
                let help = match which {
                    Val::Point(_) => "a point: use its .x and .y, or mid( ), lerp( ) and dist( )",
                    _ => "a layer, or the canvas: give one of its edges, such as .left",
                };
                return Err(Error::new(at.clone(), format!("Arithmetic is done on numbers, and this has {help}")));
            };
            Val::Q(arith(*op, x, y, scope, at)?)
        }
        NodeKind::Call(f, args) => {
            let vals = args.iter().map(|a| eval(a, scope)).collect::<Result<Vec<_>, _>>()?;
            let spans: Vec<Range<usize>> = args.iter().map(|a| a.at.clone()).collect();
            function(*f, &vals, &spans, scope, at)?
        }
    };
    if let Val::Q(q) = v
        && !q.v.is_finite()
    {
        return Err(Error::new(at.clone(), "This comes to no finite number"));
    }
    Ok(v)
}

/// The value of the word `w`: a word of the language, or a layer's name.
fn word(w: &str, scope: &Scope, at: &Range<usize>) -> Result<Val, Error> {
    let own = |f: fn(&Own) -> f64, what: &str| match &scope.own {
        Some(o) => Ok(f(o)),
        None => Err(Error::new(at.clone(), format!("“{w}” is the box's own {what}, and there is no box here"))),
    };
    Ok(match w {
        "x" => Val::Q(Q::length(own(|o| o.x, "position across")?)),
        "y" => Val::Q(Q::length(own(|o| o.y, "position down")?)),
        "width" => Val::Q(Q::length(own(|o| o.width, "width")?)),
        "height" => Val::Q(Q::length(own(|o| o.height, "height")?)),
        "angle" => Val::Q(Q::angle(own(|o| o.angle, "angle")?)),
        "ppi" => Val::Q(Q::plain(scope.ppi)),
        "pi" => Val::Q(Q::plain(std::f64::consts::PI)),
        "canvas" => {
            let doc = scope.doc.ok_or_else(|| Error::new(at.clone(), "No image is open, so there is no canvas"))?;
            let c = doc.canvas.rect().to_f();
            Val::Bounds([
                c.x0 - scope.origin[0],
                c.y0 - scope.origin[1],
                c.x1 - scope.origin[0],
                c.y1 - scope.origin[1],
            ])
        }
        "layer" => return Err(Error::new(at.clone(), "layer( ) names a layer: layer(\"Its name\")")),
        name => {
            let doc = scope.doc.ok_or_else(|| {
                Error::new(
                    at.clone(),
                    format!("“{name}” is not a word the language has, and no image is open to have a layer so called"),
                )
            })?;
            let layer = layer_named(doc, name, at)?;
            layer_bounds(&layer, scope, at)?
        }
    })
}

fn layer_bounds(layer: &Layer, scope: &Scope, at: &Range<usize>) -> Result<Val, Error> {
    let b =
        layer.bounds().ok_or_else(|| Error::new(at.clone(), format!("“{}” holds nothing to measure", layer.name)))?;
    let [ox, oy] = scope.origin;
    Ok(Val::Bounds([b.x0 - ox, b.y0 - oy, b.x1 - ox, b.y1 - oy]))
}

/// Member `m` of `v`.
fn member(v: Val, m: &str, at: &Range<usize>) -> Result<Val, Error> {
    match v {
        Val::Bounds([l, t, r, b]) => {
            let q = |v| Ok(Val::Q(Q::length(v)));
            match m {
                "left" | "x" => q(l),
                "right" => q(r),
                "top" | "y" => q(t),
                "bottom" => q(b),
                "width" => q(r - l),
                "height" => q(b - t),
                _ => match POINTS.iter().find(|p| p.0 == m) {
                    Some((_, [u, v])) => Ok(Val::Point([l + u * (r - l), t + v * (b - t)])),
                    None => {
                        let names: Vec<&str> = EDGES.iter().copied().chain(POINTS.iter().map(|p| p.0)).collect();
                        Err(Error::new(at.clone(), format!("Bounds have no “{m}” (they have {})", names.join(", "))))
                    }
                },
            }
        }
        Val::Point([x, y]) => match m {
            "x" => Ok(Val::Q(Q::length(x))),
            "y" => Ok(Val::Q(Q::length(y))),
            _ => Err(Error::new(at.clone(), format!("A point has no “{m}”, only .x and .y"))),
        },
        Val::Q(q) => Err(Error::new(at.clone(), format!("{} has no “{m}”", capital(&q.kind())))),
    }
}

fn arith(op: Op, a: Q, b: Q, scope: &Scope, at: &Range<usize>) -> Result<Q, Error> {
    Ok(match op {
        Op::Add | Op::Sub => {
            let (a, b) = unify(a, b, scope, at, if op == Op::Add { "added to" } else { "taken from" })?;
            let v = if op == Op::Add { a.v + b.v } else { a.v - b.v };
            Q { v, ..a }
        }
        Op::Mul => {
            let (len, ang) = powers(a.len + b.len, a.ang + b.ang, at)?;
            Q { v: a.v * b.v, len, ang, bare: a.bare && b.bare }
        }
        Op::Div => {
            if b.v == 0.0 {
                return Err(Error::new(at.clone(), "This divides by zero"));
            }
            let (len, ang) = powers(a.len - b.len, a.ang - b.ang, at)?;
            Q { v: a.v / b.v, len, ang, bare: a.bare && b.bare }
        }
        Op::Pow => {
            if (b.len, b.ang) != (0, 0) {
                return Err(Error::new(at.clone(), format!("A power must be a number, not {}", b.kind())));
            }
            let whole = |d: i32| {
                let p = f64::from(d) * b.v;
                ((p - p.round()).abs() < 1e-9).then_some(p.round() as i32)
            };
            let (Some(len), Some(ang)) = (whole(a.len), whole(a.ang)) else {
                return Err(Error::new(
                    at.clone(),
                    format!("{} to this power is no kind of thing", capital(&a.kind())),
                ));
            };
            let (len, ang) = powers(len, ang, at)?;
            Q { v: a.v.powf(b.v), len, ang, bare: a.bare && b.bare }
        }
    })
}

/// The powers a quantity's lengths and angles are raised to, unless beyond what any
/// sensible expression needs (and what keeps the counting of them from overflowing).
fn powers(len: i32, ang: i32, at: &Range<usize>) -> Result<(i32, i32), Error> {
    if len.abs() > 64 || ang.abs() > 64 {
        return Err(Error::new(at.clone(), "This raises lengths or angles to powers beyond any use"));
    }
    Ok((len, ang))
}

fn function(f: Func, vals: &[Val], spans: &[Range<usize>], scope: &Scope, at: &Range<usize>) -> Result<Val, Error> {
    let q = |i: usize| quantity(vals[i], &spans[i]);
    let p = |i: usize| point(vals[i], &spans[i]);
    let num = |v: f64| Ok(Val::Q(Q::plain(v)));
    let angle = |v: f64| Ok(Val::Q(Q::angle(v)));
    match f {
        Func::Min | Func::Max => {
            let mut best = q(0)?;
            for i in 1..vals.len() {
                let (a, b) = unify(best, q(i)?, scope, at, "compared with")?;
                let take = if f == Func::Min { b.v < a.v } else { b.v > a.v };
                best = if take { b } else { a };
            }
            Ok(Val::Q(best))
        }
        Func::Abs => {
            let a = q(0)?;
            Ok(Val::Q(Q { v: a.v.abs(), ..a }))
        }
        Func::Round | Func::Floor | Func::Ceil => {
            let way = |v: f64| match f {
                Func::Round => v.round(),
                Func::Floor => v.floor(),
                _ => v.ceil(),
            };
            let a = q(0)?;
            if vals.len() == 1 {
                // A length to whole pixels, an angle to whole degrees.
                return Ok(Val::Q(Q { v: way(a.v), ..a }));
            }
            let (a, step) = unify(a, q(1)?, scope, at, "rounded to")?;
            if step.v <= 0.0 {
                return Err(Error::new(spans[1].clone(), "What it rounds to must be more than nothing"));
            }
            Ok(Val::Q(Q { v: way(a.v / step.v) * step.v, ..a }))
        }
        Func::Sqrt => {
            let a = q(0)?;
            if a.len % 2 != 0 || a.ang % 2 != 0 {
                return Err(Error::new(at.clone(), format!("The square root of {} is no kind of thing", a.kind())));
            }
            if a.v < 0.0 {
                return Err(Error::new(at.clone(), "A number below zero has no square root"));
            }
            Ok(Val::Q(Q { v: a.v.sqrt(), len: a.len / 2, ang: a.ang / 2, bare: a.bare }))
        }
        Func::Sin | Func::Cos | Func::Tan => {
            let r = as_angle(q(0)?, &spans[0])?.to_radians();
            num(match f {
                Func::Sin => r.sin(),
                Func::Cos => r.cos(),
                _ => r.tan(),
            })
        }
        Func::Asin | Func::Acos => {
            let v = as_number(q(0)?, &spans[0])?;
            if !(-1.0..=1.0).contains(&v) {
                return Err(Error::new(spans[0].clone(), format!("{}( ) takes a number from -1 to 1", f.name())));
            }
            angle(if f == Func::Asin { v.asin() } else { v.acos() }.to_degrees())
        }
        Func::Atan => angle(as_number(q(0)?, &spans[0])?.atan().to_degrees()),
        Func::Atan2 => {
            let (y, x) = unify(q(0)?, q(1)?, scope, at, "taken with")?;
            angle(y.v.atan2(x.v).to_degrees())
        }
        Func::Mid => {
            let (a, b) = (p(0)?, p(1)?);
            Ok(Val::Point([(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]))
        }
        Func::Lerp => {
            let t = as_number(q(2)?, &spans[2])?;
            match (vals[0], vals[1]) {
                (Val::Q(a), Val::Q(b)) => {
                    let (a, b) = unify(a, b, scope, at, "taken with")?;
                    Ok(Val::Q(Q { v: a.v + (b.v - a.v) * t, ..a }))
                }
                _ => {
                    let (a, b) = (p(0)?, p(1)?);
                    Ok(Val::Point([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]))
                }
            }
        }
        Func::Dist => {
            let (a, b) = (p(0)?, p(1)?);
            Ok(Val::Q(Q::length((b[0] - a[0]).hypot(b[1] - a[1]))))
        }
        Func::Point => Ok(Val::Point([as_length(q(0)?, scope, &spans[0])?, as_length(q(1)?, scope, &spans[1])?])),
    }
}

/// What `text` comes to in `scope`, as `want` says (see [`Expr::eval`]).
pub fn eval_text(text: &str, scope: &Scope, want: Want) -> Result<f64, Error> {
    Expr::parse(text)?.eval(scope, want)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{IdGen, Position};
    use crate::geom::IRect;
    use crate::raster::RasterImage;
    use crate::tile::pixel_from_f32;

    /// A 200 × 100 image at 100 ppi with a layer "Logo" holding (20, 10)–(60, 30), and one
    /// "Back ground" holding the canvas.
    fn image() -> (Document, LayerId) {
        let mut ids = IdGen::default();
        let mut doc = Document::new(200, 100);
        doc.resolution = 100.0;
        let filled = |r: IRect| RasterImage::filled(&r, pixel_from_f32([1.0; 4]));
        let top = |index| Position { parent: None, index };
        let back = Layer::new_raster(ids.layer(), ids.object(), "Back ground", filled(IRect::new(0, 0, 200, 100)));
        doc.insert(top(0), back).unwrap();
        let logo = ids.layer();
        doc.insert(top(1), Layer::new_raster(logo, ids.object(), "Logo", filled(IRect::new(20, 10, 60, 30)))).unwrap();
        (doc, logo)
    }

    fn scope(doc: &Document) -> Scope<'_> {
        let own = Own { x: 5.0, y: 6.0, width: 50.0, height: 25.0, angle: 30.0 };
        Scope { doc: Some(doc), ppi: 100.0, unit: Unit::In, origin: [0.0, 0.0], own: Some(own) }
    }

    fn length(text: &str) -> f64 {
        let (doc, _) = image();
        eval_text(text, &scope(&doc), Want::Length).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    fn fails(text: &str, want: Want) -> Error {
        let (doc, _) = image();
        eval_text(text, &scope(&doc), want).expect_err(text)
    }

    #[test]
    fn numbers_take_their_units_and_bare_ones_the_field_s() {
        // The field is in inches, at 100 ppi.
        assert_eq!(length("2"), 200.0);
        assert_eq!(length("2in + 3mm"), 200.0 + 300.0 / 25.4);
        assert_eq!(length("1in + 50px"), 150.0);
        assert_eq!(length("1.5e1px"), 15.0);
        assert_eq!(length("2 inches"), 200.0);
        // A bare number added to a length is in the field's unit; multiplying, a factor.
        assert_eq!(length("50px + 1"), 150.0);
        assert_eq!(length("50px * 2"), 100.0);
        assert_eq!(length("(1 + 1) * 50px"), 100.0);
        assert_eq!(length("100px / 4"), 25.0);
        assert_eq!(length("-1in"), -100.0);
        assert_eq!(length("2^3 * 1px"), 8.0);
        assert_eq!(length("sqrt(30px * 30px + 40px * 40px)"), 50.0);
        // An exponent is not a unit, and a unit is not an exponent.
        assert_eq!(length("1e2px"), 100.0);
        assert!(fails("3em", Want::Length).message.contains("operator"));
    }

    #[test]
    fn kinds_must_agree() {
        assert!(fails("1in + 5deg", Want::Length).message.contains("cannot be added to an angle"));
        assert!(fails("1in * 1in", Want::Length).message.contains("an area"));
        assert!(fails("30deg", Want::Length).message.contains("not a length"));
        assert!(fails("1in", Want::Number).message.contains("not a number"));
        assert_eq!(eval_text("width / height", &scope(&image().0), Want::Number).unwrap(), 2.0);
        // An angle in degrees, or radians; and trigonometry in degrees.
        let (doc, _) = image();
        let angle = |t: &str| eval_text(t, &scope(&doc), Want::Angle).unwrap();
        assert_eq!(angle("45"), 45.0);
        assert!((angle("1rad") - 57.29577951308232).abs() < 1e-9);
        assert_eq!(angle("90° - angle"), 60.0);
        assert!((angle("atan2(1in, 1in)") - 45.0).abs() < 1e-9);
        let number = |t: &str| eval_text(t, &scope(&doc), Want::Number).unwrap();
        assert!((number("sin(30)") - 0.5).abs() < 1e-12);
        assert!((number("cos(60deg)") - 0.5).abs() < 1e-12);
    }

    #[test]
    fn names_reach_the_canvas_the_layers_and_the_box() {
        assert_eq!(length("canvas.width"), 200.0);
        assert_eq!(length("canvas.center.x"), 100.0);
        assert_eq!(length("Logo.right + 5mm"), 60.0 + 500.0 / 25.4);
        assert_eq!(length("Logo.bottom_right.y"), 30.0);
        assert_eq!(length("layer(\"Back ground\").height"), 100.0);
        assert_eq!(length("logo.left"), 20.0, "in another case, if just one is");
        assert_eq!(length("mid(Logo.top_left, canvas.bottom_right).x"), 110.0);
        assert_eq!(length("lerp(Logo.left, Logo.right, 0.25)"), 30.0);
        assert_eq!(length("dist(Logo.top_left, Logo.bottom_right)"), (40f64 * 40.0 + 20.0 * 20.0).sqrt());
        assert_eq!(length("point(1, 2).y"), 200.0);
        // The box's own numbers.
        assert_eq!(length("x + width / 2"), 30.0);
        // Positions from the scope's origin; lengths the same from anywhere.
        let (doc, logo) = image();
        let from = Scope { origin: [20.0, 10.0], ..scope(&doc) };
        assert_eq!(eval_text("Logo.right", &from, Want::Length).unwrap(), 40.0);
        assert_eq!(eval_text("Logo.width", &from, Want::Length).unwrap(), 40.0);
        assert_eq!(eval_text(&format!("layer({}).top", logo.0), &from, Want::Length).unwrap(), 0.0);
    }

    #[test]
    fn rounding_is_to_whole_pixels_or_to_a_step() {
        assert_eq!(length("round(10.6px)"), 11.0);
        assert_eq!(length("floor(10.6px)"), 10.0);
        assert_eq!(length("round(37px, 10px)"), 40.0);
        // The step in the field's unit when bare: to the nearest inch.
        assert_eq!(length("round(170px, 1)"), 200.0);
        assert_eq!(length("min(1in, 50px, 2)"), 50.0);
        assert_eq!(length("max(Logo.width, Logo.height)"), 40.0);
    }

    #[test]
    fn errors_say_what_and_where() {
        let (doc, _) = image();
        let s = scope(&doc);
        let at = |text: &str| {
            let e = eval_text(text, &s, Want::Length).expect_err(text);
            (text[e.at.clone()].to_owned(), e.message)
        };
        assert_eq!(at("Logo.right + Lgo.left").0, "Lgo");
        assert!(at("Lgo.left").1.contains("no layer called “Lgo”"));
        assert_eq!(at("2 + * 3").0, "*");
        assert_eq!(at("(1 + 2").0, "");
        assert_eq!(at("foo(1)").0, "foo");
        assert!(at("min()").1.contains("at least 1"));
        assert!(at("round(1, 2, 3)").1.contains("1 or 2 arguments"));
        assert!(at("Logo.middle").1.contains("no “middle”"));
        assert!(at("Logo").1.contains("give one of its edges"));
        assert!(at("Logo.center").1.contains("point"));
        assert!(at("1in / 0").1.contains("zero"));
        assert!(at("2 3").1.contains("operator"));
        assert!(at("\"Logo\".left").1.contains("layer("));
        assert!(at("").1.contains("nothing"));
        assert!(at("1in $ 2").1.contains("“$”"));
        // Without a box, or an image.
        let none = Scope::bare(Unit::Px, 72.0);
        assert!(eval_text("width", &none, Want::Length).unwrap_err().message.contains("no box here"));
        assert!(eval_text("canvas.width", &none, Want::Length).unwrap_err().message.contains("No image"));
        // Two layers of one name: which is meant cannot be told.
        let (mut doc, _) = image();
        let mut ids = IdGen::default();
        for _ in 0..10 {
            ids.layer();
        }
        let twin = Layer::new_empty(ids.layer(), "Logo");
        doc.insert(Position { parent: None, index: 2 }, twin).unwrap();
        let e = eval_text("Logo.left", &scope(&doc), Want::Length).unwrap_err();
        assert!(e.message.contains("2 layers are called “Logo”"), "{e}");
    }

    #[test]
    fn text_from_anyone_is_read_within_bounds() {
        // A script's or a file's text: too deep, too long or too high a power is refused,
        // never taken down the stack or past what a count holds.
        let deep = format!("{}1{}", "(".repeat(10_000), ")".repeat(10_000));
        assert!(Expr::parse(&deep).unwrap_err().message.contains("longer than"));
        let deep = format!("{}1{}", "(".repeat(100), ")".repeat(100));
        assert!(Expr::parse(&deep).unwrap_err().message.contains("nests more than"));
        let signs = format!("{}1", "-".repeat(1000));
        assert!(Expr::parse(&signs).unwrap_err().message.contains("nests more than"));
        let chain = format!("1{}", " + 1".repeat(200));
        assert!(Expr::parse(&chain).unwrap_err().message.contains("nests more than"));
        let calls = format!("{}1{}", "abs(".repeat(100), ")".repeat(100));
        assert!(Expr::parse(&calls).unwrap_err().message.contains("nests more than"));
        assert!(fails("1px^40 * 1px^40", Want::Length).message.contains("powers beyond"));
        assert!(fails("1px^1e12", Want::Length).message.contains("powers beyond"));
        assert!(fails("10^400", Want::Number).message.contains("no finite number"));
        // Within them, a long expression is fine.
        let fine = format!("1px{}", " + 1px".repeat(60));
        assert_eq!(eval_text(&fine, &Scope::bare(Unit::Px, 72.0), Want::Length).unwrap(), 61.0);
    }

    #[test]
    fn bound_it_follows_its_layers_through_renaming() {
        let (mut doc, logo) = image();
        let e = Expr::parse("(Logo.right + 5mm) * 2 - layer(\"Back ground\").top").unwrap();
        let bound = e.bind(&doc).unwrap();
        // Kept by ids, shown by names: as typed, or quoted where a name cannot be a word.
        let kept = bound.text(&|_| None);
        assert!(kept.contains(&format!("layer({})", logo.0)), "{kept}");
        doc.layer_mut(logo).unwrap().name = "Mark 2".to_owned();
        let names = |id: LayerId| doc.layer(id).map(|l| l.name.clone());
        let shown = bound.text(&names);
        assert_eq!(shown, "(layer(\"Mark 2\").right + 5mm) * 2 - layer(\"Back ground\").top");
        // Read back, either text is the same expression.
        let again = Expr::parse(&kept).unwrap();
        assert_eq!(again.eval(&scope(&doc), Want::Length).unwrap(), bound.eval(&scope(&doc), Want::Length).unwrap());
        assert_eq!(again.layers(), vec![logo, doc.layers[0].id]);
        // A layer gone is an error that says so, never another layer.
        let gone = Expr::parse("layer(999).left").unwrap();
        assert!(gone.eval(&scope(&doc), Want::Length).unwrap_err().message.contains("no layer 999"));
    }

    #[test]
    fn its_text_reads_back_as_the_same_expression() {
        for text in ["-(1 + 2) * 3", "2^3^2", "(2^3)^2", "1 - (2 - 3)", "-2^2", "1in + 2mm * 3", "min(1, 2, 3) / 4"] {
            let e = Expr::parse(text).unwrap();
            let written = e.text(&|_| None);
            let s = Scope::bare(Unit::Px, 72.0);
            let (a, b) =
                (e.eval(&s, Want::Length).unwrap(), Expr::parse(&written).unwrap().eval(&s, Want::Length).unwrap());
            assert_eq!(a, b, "{text} was written {written}");
        }
        assert_eq!(Expr::parse("2^3^2").unwrap().eval(&Scope::bare(Unit::Px, 72.0), Want::Number).unwrap(), 512.0);
        assert_eq!(Expr::parse("-2^2").unwrap().eval(&Scope::bare(Unit::Px, 72.0), Want::Number).unwrap(), -4.0);
        assert!(Expr::parse("2.5cm").unwrap().is_number());
        assert!(!Expr::parse("2.5cm + 1").unwrap().is_number());
    }
}
