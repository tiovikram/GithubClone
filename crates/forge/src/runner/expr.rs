//! GitHub Actions expressions (`${{ ... }}` and `if:` conditions).
//!
//! Supported: literals (`null`, booleans, numbers, `'strings'`), context access (`a.b`, `a['b']`,
//! `a.*.b` object filters), `!`, `<`, `<=`, `>`, `>=`, `==`, `!=`, `&&`, `||`, parentheses and the
//! functions `contains`, `startsWith`, `endsWith`, `format`, `join`, `toJSON`, `fromJSON`,
//! `hashFiles` (when the evaluator has a workspace) and the status checks `success()`,
//! `failure()`, `always()`, `cancelled()`. Comparison follows GitHub's loose rules: values of
//! different types are compared as numbers and strings compare case-insensitively.
use serde_json::{json, Map, Value};

/// Job/step status the status functions look at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Success,
    Failure,
    Cancelled,
}

/// Evaluation context: a JSON object of named contexts (`github`, `env`, `matrix`, ...).
pub struct Ctx<'a> {
    pub contexts: &'a Value,
    pub status: Status,
    /// Workspace directory for `hashFiles`.
    pub workspace: Option<&'a std::path::Path>,
}

impl<'a> Ctx<'a> {
    pub fn new(contexts: &'a Value) -> Self {
        Ctx { contexts, status: Status::Success, workspace: None }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Op(&'static str),
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if ch == '\'' {
            let mut s = String::new();
            i += 1;
            loop {
                if i >= c.len() {
                    return Err("unterminated string".into());
                }
                if c[i] == '\'' {
                    if i + 1 < c.len() && c[i + 1] == '\'' {
                        s.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                s.push(c[i]);
                i += 1;
            }
            out.push(Tok::Str(s));
            continue;
        }
        let two: String = c[i..(i + 2).min(c.len())].iter().collect();
        let op2 = match two.as_str() {
            "<=" => Some("<="),
            ">=" => Some(">="),
            "==" => Some("=="),
            "!=" => Some("!="),
            "&&" => Some("&&"),
            "||" => Some("||"),
            _ => None,
        };
        if let Some(op) = op2 {
            out.push(Tok::Op(op));
            i += 2;
            continue;
        }
        let op1 = match ch {
            '(' => Some("("),
            ')' => Some(")"),
            '[' => Some("["),
            ']' => Some("]"),
            '.' if !(i + 1 < c.len() && c[i + 1].is_ascii_digit() && matches!(out.last(), None | Some(Tok::Op(_)))) => Some("."),
            '!' => Some("!"),
            '<' => Some("<"),
            '>' => Some(">"),
            ',' => Some(","),
            '*' => Some("*"),
            _ => None,
        };
        if let Some(op) = op1 {
            out.push(Tok::Op(op));
            i += 1;
            continue;
        }
        if ch.is_ascii_digit() || ch == '.' || (ch == '-' && i + 1 < c.len() && (c[i + 1].is_ascii_digit() || c[i + 1] == '.')) {
            let start = i;
            i += 1;
            while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == '.' || ((c[i] == '-' || c[i] == '+') && matches!(c[i - 1], 'e' | 'E'))) {
                i += 1;
            }
            let text: String = c[start..i].iter().collect();
            let n = parse_number(&text).ok_or_else(|| format!("invalid number {text}"))?;
            out.push(Tok::Num(n));
            continue;
        }
        if ch.is_alphabetic() || ch == '_' {
            let start = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '-') {
                i += 1;
            }
            out.push(Tok::Ident(c[start..i].iter().collect()));
            continue;
        }
        return Err(format!("unexpected character '{ch}'"));
    }
    Ok(out)
}

fn parse_number(t: &str) -> Option<f64> {
    if let Some(h) = t.strip_prefix("0x") {
        return i64::from_str_radix(h, 16).ok().map(|v| v as f64);
    }
    if let Some(o) = t.strip_prefix("0o") {
        return i64::from_str_radix(o, 8).ok().map(|v| v as f64);
    }
    t.parse::<f64>().ok()
}

#[derive(Debug, Clone)]
enum Node {
    Lit(Value),
    Ctx(String),
    Index(Box<Node>, Box<Node>),
    Star(Box<Node>),
    Not(Box<Node>),
    Bin(&'static str, Box<Node>, Box<Node>),
    Call(String, Vec<Node>),
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn eat_op(&mut self, op: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Op(o)) if *o == op) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect_op(&mut self, op: &str) -> Result<(), String> {
        if self.eat_op(op) {
            Ok(())
        } else {
            Err(format!("expected '{op}'"))
        }
    }
    fn or(&mut self) -> Result<Node, String> {
        let mut l = self.and()?;
        while self.eat_op("||") {
            let r = self.and()?;
            l = Node::Bin("||", Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn and(&mut self) -> Result<Node, String> {
        let mut l = self.eq()?;
        while self.eat_op("&&") {
            let r = self.eq()?;
            l = Node::Bin("&&", Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn eq(&mut self) -> Result<Node, String> {
        let mut l = self.cmp()?;
        loop {
            let op = if self.eat_op("==") {
                "=="
            } else if self.eat_op("!=") {
                "!="
            } else {
                break;
            };
            let r = self.cmp()?;
            l = Node::Bin(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn cmp(&mut self) -> Result<Node, String> {
        let mut l = self.unary()?;
        loop {
            let op = ["<=", ">=", "<", ">"].into_iter().find(|o| self.eat_op(o));
            let Some(op) = op else { break };
            let r = self.unary()?;
            l = Node::Bin(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn unary(&mut self) -> Result<Node, String> {
        if self.eat_op("!") {
            return Ok(Node::Not(Box::new(self.unary()?)));
        }
        self.postfix()
    }
    fn postfix(&mut self) -> Result<Node, String> {
        let mut n = self.primary()?;
        loop {
            if self.eat_op(".") {
                if self.eat_op("*") {
                    n = Node::Star(Box::new(n));
                    continue;
                }
                match self.toks.get(self.pos).cloned() {
                    Some(Tok::Ident(name)) => {
                        self.pos += 1;
                        n = Node::Index(Box::new(n), Box::new(Node::Lit(Value::String(name))));
                    }
                    Some(Tok::Num(x)) => {
                        // `a.1` is unusual but tokenizes as a number
                        self.pos += 1;
                        n = Node::Index(Box::new(n), Box::new(Node::Lit(Value::String(fmt_num(x)))));
                    }
                    _ => return Err("expected property name after '.'".into()),
                }
            } else if self.eat_op("[") {
                if self.eat_op("*") {
                    self.expect_op("]")?;
                    n = Node::Star(Box::new(n));
                    continue;
                }
                let idx = self.or()?;
                self.expect_op("]")?;
                n = Node::Index(Box::new(n), Box::new(idx));
            } else {
                break;
            }
        }
        Ok(n)
    }
    fn primary(&mut self) -> Result<Node, String> {
        let t = self.toks.get(self.pos).cloned().ok_or("unexpected end of expression")?;
        self.pos += 1;
        match t {
            Tok::Num(n) => Ok(Node::Lit(json!(n))),
            Tok::Str(s) => Ok(Node::Lit(Value::String(s))),
            Tok::Op("(") => {
                let e = self.or()?;
                self.expect_op(")")?;
                Ok(e)
            }
            Tok::Ident(id) => {
                match id.as_str() {
                    "true" => return Ok(Node::Lit(Value::Bool(true))),
                    "false" => return Ok(Node::Lit(Value::Bool(false))),
                    "null" => return Ok(Node::Lit(Value::Null)),
                    "NaN" => return Ok(Node::Lit(json!(f64::NAN))),
                    "Infinity" => return Ok(Node::Lit(json!(f64::INFINITY))),
                    _ => {}
                }
                if self.eat_op("(") {
                    let mut args = Vec::new();
                    if !self.eat_op(")") {
                        loop {
                            args.push(self.or()?);
                            if self.eat_op(")") {
                                break;
                            }
                            self.expect_op(",")?;
                        }
                    }
                    Ok(Node::Call(id.to_ascii_lowercase(), args))
                } else {
                    Ok(Node::Ctx(id))
                }
            }
            Tok::Op(o) => Err(format!("unexpected '{o}'")),
        }
    }
}

fn parse(src: &str) -> Result<Node, String> {
    let toks = tokenize(src)?;
    if toks.is_empty() {
        return Err("empty expression".into());
    }
    let mut p = Parser { toks, pos: 0 };
    let n = p.or()?;
    if p.pos != p.toks.len() {
        return Err(format!("unexpected token {:?}", p.toks[p.pos]));
    }
    Ok(n)
}

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn to_num(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                0.0
            } else {
                parse_number(t).unwrap_or(f64::NAN)
            }
        }
        _ => f64::NAN,
    }
}

fn fmt_num(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        format!("{f}")
    }
}

/// String form of a value as used in interpolation.
pub fn to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.as_f64().map(fmt_num).unwrap_or_else(|| n.to_string()),
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

fn loose_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => x.to_lowercase() == y.to_lowercase(),
        (Value::Null, Value::Null) => true,
        (Value::Array(_) | Value::Object(_), _) | (_, Value::Array(_) | Value::Object(_)) => std::ptr::eq(a, b) || a == b,
        _ => {
            let (x, y) = (to_num(a), to_num(b));
            x == y
        }
    }
}

fn compare(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::String(x), Value::String(y)) => Some(x.to_lowercase().cmp(&y.to_lowercase())),
        _ => to_num(a).partial_cmp(&to_num(b)),
    }
}

fn get_prop(v: &Value, key: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let k = to_string(key);
            if let Some(x) = m.get(&k) {
                return x.clone();
            }
            let lk = k.to_lowercase();
            m.iter().find(|(kk, _)| kk.to_lowercase() == lk).map(|(_, x)| x.clone()).unwrap_or(Value::Null)
        }
        Value::Array(a) => {
            let f = to_num(key);
            if f.is_nan() || f < 0.0 || f.fract() != 0.0 {
                return Value::Null;
            }
            a.get(f as usize).cloned().unwrap_or(Value::Null)
        }
        _ => Value::Null,
    }
}

impl Ctx<'_> {
    /// Evaluates a property chain; the flag tells whether the value is a filtered array
    /// (`a.*`), whose further property accesses apply to every element.
    fn eval_chain(&self, n: &Node) -> Result<(Value, bool), String> {
        Ok(match n {
            Node::Index(base, idx) => {
                let (b, filtered) = self.eval_chain(base)?;
                let i = self.eval(idx)?;
                match b {
                    Value::Array(items) if filtered => {
                        (Value::Array(items.iter().map(|x| get_prop(x, &i)).filter(|x| !x.is_null()).collect()), true)
                    }
                    other => (get_prop(&other, &i), false),
                }
            }
            Node::Star(base) => {
                let (b, filtered) = self.eval_chain(base)?;
                let elems = |v: Value| -> Vec<Value> {
                    match v {
                        Value::Array(a) => a,
                        Value::Object(m) => m.into_iter().map(|(_, v)| v).collect(),
                        _ => vec![],
                    }
                };
                let out = if filtered { b.as_array().cloned().unwrap_or_default().into_iter().flat_map(elems).collect() } else { elems(b) };
                (Value::Array(out), true)
            }
            other => (self.eval(other)?, false),
        })
    }

    fn eval(&self, n: &Node) -> Result<Value, String> {
        Ok(match n {
            Node::Lit(v) => v.clone(),
            Node::Ctx(name) => get_prop(self.contexts, &Value::String(name.clone())),
            Node::Index(..) | Node::Star(_) => self.eval_chain(n)?.0,
            Node::Not(x) => Value::Bool(!truthy(&self.eval(x)?)),
            Node::Bin(op, l, r) => {
                let a = self.eval(l)?;
                match *op {
                    "&&" => {
                        if !truthy(&a) {
                            a
                        } else {
                            self.eval(r)?
                        }
                    }
                    "||" => {
                        if truthy(&a) {
                            a
                        } else {
                            self.eval(r)?
                        }
                    }
                    _ => {
                        let b = self.eval(r)?;
                        Value::Bool(match *op {
                            "==" => loose_eq(&a, &b),
                            "!=" => !loose_eq(&a, &b),
                            "<" => compare(&a, &b) == Some(std::cmp::Ordering::Less),
                            "<=" => matches!(compare(&a, &b), Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)),
                            ">" => compare(&a, &b) == Some(std::cmp::Ordering::Greater),
                            ">=" => matches!(compare(&a, &b), Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)),
                            _ => return Err(format!("unknown operator {op}")),
                        })
                    }
                }
            }
            Node::Call(f, args) => self.call(f, args)?,
        })
    }

    fn call(&self, f: &str, args: &[Node]) -> Result<Value, String> {
        let argv = |i: usize| -> Result<Value, String> { args.get(i).map(|a| self.eval(a)).unwrap_or(Ok(Value::Null)) };
        let need = |n: usize| -> Result<(), String> {
            if args.len() < n {
                Err(format!("{f}() expects at least {n} argument(s)"))
            } else {
                Ok(())
            }
        };
        Ok(match f {
            "success" => Value::Bool(self.status == Status::Success),
            "failure" => Value::Bool(self.status == Status::Failure),
            "cancelled" => Value::Bool(self.status == Status::Cancelled),
            "always" => Value::Bool(true),
            "contains" => {
                need(2)?;
                let (hay, item) = (argv(0)?, argv(1)?);
                Value::Bool(match &hay {
                    Value::Array(a) => a.iter().any(|x| loose_eq(x, &item)),
                    other => to_string(other).to_lowercase().contains(&to_string(&item).to_lowercase()),
                })
            }
            "startswith" => {
                need(2)?;
                Value::Bool(to_string(&argv(0)?).to_lowercase().starts_with(&to_string(&argv(1)?).to_lowercase()))
            }
            "endswith" => {
                need(2)?;
                Value::Bool(to_string(&argv(0)?).to_lowercase().ends_with(&to_string(&argv(1)?).to_lowercase()))
            }
            "format" => {
                need(1)?;
                let fmt = to_string(&argv(0)?);
                let vals: Vec<String> = (1..args.len()).map(|i| argv(i).map(|v| to_string(&v))).collect::<Result<_, _>>()?;
                Value::String(format_str(&fmt, &vals)?)
            }
            "join" => {
                need(1)?;
                let sep = if args.len() > 1 { to_string(&argv(1)?) } else { ",".into() };
                match argv(0)? {
                    Value::Array(a) => Value::String(a.iter().map(to_string).collect::<Vec<_>>().join(&sep)),
                    other => Value::String(to_string(&other)),
                }
            }
            "tojson" => {
                need(1)?;
                Value::String(serde_json::to_string_pretty(&argv(0)?).unwrap_or_default())
            }
            "fromjson" => {
                need(1)?;
                let s = to_string(&argv(0)?);
                serde_json::from_str(&s).map_err(|e| format!("fromJSON: invalid JSON: {e}"))?
            }
            "hashfiles" => {
                let pats: Vec<String> = (0..args.len()).map(|i| argv(i).map(|v| to_string(&v))).collect::<Result<_, _>>()?;
                Value::String(match self.workspace {
                    Some(ws) => hash_files(ws, &pats),
                    None => String::new(),
                })
            }
            other => return Err(format!("unknown function {other}()")),
        })
    }
}

fn format_str(fmt: &str, vals: &[String]) -> Result<String, String> {
    let c: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '{' {
            if i + 1 < c.len() && c[i + 1] == '{' {
                out.push('{');
                i += 2;
                continue;
            }
            let end = c[i..].iter().position(|&x| x == '}').map(|p| p + i).ok_or("format: unclosed '{'")?;
            let idx: String = c[i + 1..end].iter().collect();
            let n: usize = idx.trim().parse().map_err(|_| format!("format: invalid placeholder {{{idx}}}"))?;
            out.push_str(vals.get(n).ok_or_else(|| format!("format: missing argument {n}"))?);
            i = end + 1;
        } else if c[i] == '}' && i + 1 < c.len() && c[i + 1] == '}' {
            out.push('}');
            i += 2;
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    Ok(out)
}

/// sha256 over the sha256 of each matching file (sorted), like GitHub's `hashFiles`.
fn hash_files(ws: &std::path::Path, patterns: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let mut files = Vec::new();
    for p in patterns {
        let (neg, p) = match p.strip_prefix('!') {
            Some(x) => (true, x),
            None => (false, p.as_str()),
        };
        let Some(re) = crate::runner::glob::to_regex(p) else { continue };
        let mut found = Vec::new();
        walk(ws, ws, &mut found);
        for f in found {
            if re.is_match(&f) {
                if neg {
                    files.retain(|x: &String| x != &f);
                } else if !files.contains(&f) {
                    files.push(f);
                }
            }
        }
    }
    if files.is_empty() {
        return String::new();
    }
    files.sort();
    let mut outer = Sha256::new();
    for f in files {
        if let Ok(bytes) = std::fs::read(ws.join(&f)) {
            outer.update(Sha256::digest(&bytes));
        }
    }
    hex::encode(outer.finalize())
}

fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if e.file_name() == ".git" {
            continue;
        }
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(root, &p, out),
            Ok(t) if t.is_file() => {
                if let Ok(rel) = p.strip_prefix(root) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
            _ => {}
        }
    }
}

/// Evaluates one expression (without `${{ }}`).
pub fn eval(ctx: &Ctx, src: &str) -> Result<Value, String> {
    let n = parse(src)?;
    ctx.eval(&n)
}

/// Whether an expression calls one of the status functions (then no implicit `success() &&`).
fn has_status_fn(src: &str) -> bool {
    let Ok(toks) = tokenize(src) else { return false };
    toks.windows(2).any(|w| {
        matches!((&w[0], &w[1]), (Tok::Ident(f), Tok::Op("(")) if matches!(f.to_ascii_lowercase().as_str(), "success" | "failure" | "always" | "cancelled"))
    })
}

/// Evaluates an `if:` condition (bare or wrapped in `${{ }}`); empty means `success()`.
pub fn eval_if(ctx: &Ctx, cond: Option<&Value>) -> Result<bool, String> {
    let src = match cond {
        None | Some(Value::Null) => String::new(),
        Some(Value::Bool(b)) => return Ok(*b && ctx.status == Status::Success),
        Some(Value::Number(n)) => return Ok(n.as_f64().unwrap_or(0.0) != 0.0 && ctx.status == Status::Success),
        Some(Value::String(s)) => s.trim().to_string(),
        Some(other) => to_string(other),
    };
    let mut src = src.trim().to_string();
    if src.starts_with("${{") && src.ends_with("}}") && src.matches("${{").count() == 1 {
        src = src[3..src.len() - 2].trim().to_string();
    } else if src.contains("${{") {
        // Mixed text: interpolate, then the result is truthy if non-empty.
        let s = interpolate(ctx, &src)?;
        return Ok(ctx.status == Status::Success && truthy(&Value::String(s)));
    }
    if src.is_empty() {
        return Ok(ctx.status == Status::Success);
    }
    let full = if has_status_fn(&src) { src } else { format!("success() && ({src})") };
    Ok(truthy(&eval(ctx, &full)?))
}

/// Finds the end of a `${{ ... }}` block starting after `${{` at byte `start`; returns the
/// byte index of the closing `}}`.
fn find_close(s: &str, start: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = start;
    let mut in_str = false;
    while i < b.len() {
        if in_str {
            if b[i] == b'\'' {
                if i + 1 < b.len() && b[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                in_str = false;
            }
        } else if b[i] == b'\'' {
            in_str = true;
        } else if b[i] == b'}' && i + 1 < b.len() && b[i + 1] == b'}' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Replaces every `${{ expr }}` in `s` with the expression's string value.
pub fn interpolate(ctx: &Ctx, s: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = 0;
    while let Some(p) = s[rest..].find("${{") {
        let open = rest + p;
        out.push_str(&s[rest..open]);
        let close = find_close(s, open + 3).ok_or_else(|| format!("unclosed expression in {s:?}"))?;
        let v = eval(ctx, s[open + 3..close].trim())?;
        out.push_str(&to_string(&v));
        rest = close + 2;
    }
    out.push_str(&s[rest..]);
    Ok(out)
}

/// Like `interpolate`, but a string that is exactly one `${{ expr }}` keeps the value's type
/// (used for `strategy.matrix`, `with:` and other structured values).
pub fn interpolate_value(ctx: &Ctx, v: &Value) -> Result<Value, String> {
    Ok(match v {
        Value::String(s) => {
            let t = s.trim();
            if t.starts_with("${{") && t.ends_with("}}") && find_close(t, 3) == Some(t.len() - 2) {
                eval(ctx, t[3..t.len() - 2].trim())?
            } else if s.contains("${{") {
                Value::String(interpolate(ctx, s)?)
            } else {
                v.clone()
            }
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| interpolate_value(ctx, x)).collect::<Result<_, _>>()?),
        Value::Object(m) => {
            let mut o = Map::new();
            for (k, x) in m {
                o.insert(k.clone(), interpolate_value(ctx, x)?);
            }
            Value::Object(o)
        }
        other => other.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(c: &Value, s: &str) -> Value {
        eval(&Ctx::new(c), s).unwrap()
    }

    #[test]
    fn basics() {
        let c = json!({"github": {"event_name": "push", "ref": "refs/heads/main"}, "matrix": {"os": "linux", "n": 2},
            "needs": {"a": {"outputs": {"x": "1"}, "result": "success"}, "b": {"outputs": {"x": "2"}}}});
        assert_eq!(ev(&c, "github.event_name == 'PUSH'"), json!(true));
        assert_eq!(ev(&c, "github['event_name'] != 'pull_request'"), json!(true));
        assert_eq!(ev(&c, "matrix.n > 1 && matrix.os"), json!("linux"));
        assert_eq!(ev(&c, "!matrix.missing"), json!(true));
        assert_eq!(ev(&c, "needs.*.outputs.x"), json!(["1", "2"]));
        assert_eq!(ev(&c, "contains(needs.*.result, 'success')"), json!(true));
        assert_eq!(ev(&c, "startsWith(github.ref, 'refs/heads/')"), json!(true));
        assert_eq!(ev(&c, "format('{0}-{1}{{x}}', 'a', 3)"), json!("a-3{x}"));
        assert_eq!(ev(&c, "fromJSON('[1,2]')[1]"), json!(2));
        assert_eq!(ev(&c, "join(fromJSON('[\"a\",\"b\"]'), '+')"), json!("a+b"));
        assert_eq!(ev(&c, "'1' == 1"), json!(true));
        assert_eq!(ev(&c, "null == 0"), json!(true));
        let ctx = Ctx::new(&c);
        assert_eq!(interpolate(&ctx, "os=${{ matrix.os }} n=${{ matrix.n }}").unwrap(), "os=linux n=2");
        assert!(eval_if(&ctx, Some(&json!("github.event_name == 'push'"))).unwrap());
        assert!(!eval_if(&ctx, Some(&json!("${{ github.event_name == 'pull_request' }}"))).unwrap());
        let failed = Ctx { contexts: &c, status: Status::Failure, workspace: None };
        assert!(!eval_if(&failed, None).unwrap());
        assert!(eval_if(&failed, Some(&json!("failure()"))).unwrap());
        assert!(eval_if(&failed, Some(&json!("always()"))).unwrap());
        assert_eq!(interpolate_value(&ctx, &json!("${{ fromJSON('[1]') }}")).unwrap(), json!([1]));
    }
}
