//! Expressions: every amount, cost, condition and boost in a game is one.
//!
//! ```text
//! 10 * 1.15 ^ owned                    # a generator's cost
//! 1 + 0.1 * Souls                      # a boost
//! total(Gold) atLeast 1e6 and Miner > 5
//! floor(sqrt(total(Gold) / 1e6))       # a prestige's gain
//! ```
//!
//! Names are the game's things (a resource's amount, a generator's count, 1 for a bought upgrade
//! or a reached milestone, a prestige's count) and a few words of the engine (`owned`, `players`,
//! `time`, `runtime`, `gain`). Comparisons and `and`/`or`/`not` give 1 or 0.

use crate::num::Num;

/// A thing a game declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Thing {
    Res(usize),
    Gen(usize),
    Rule(usize),
    Upg(usize),
    Act(usize),
    Mile(usize),
    Pres(usize),
}

/// What a name in an expression reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ref {
    /// A resource's amount, a generator's count, 1/0 for an upgrade, a milestone or an action
    /// whose boost is on, how many times a prestige was taken.
    Thing(Thing),
    /// Produced this run (since the last prestige that reset it).
    Total(usize),
    /// Produced since the player joined.
    Ever(usize),
    /// Net change per second, on the last tick.
    Rate(usize),
    /// What the player put into a shared resource, ever.
    Contrib(usize),
    /// A resource's cap (0 when it has none).
    Cap(usize),
    /// The thing summed over every player in the world.
    Sum(Thing),
    /// The largest value of the thing among the players.
    Best(Thing),
    /// How many of this generator the player has (in its costs).
    Owned,
    /// How many players the world has.
    Players,
    /// Seconds since the player joined.
    Time,
    /// Seconds since the player's last prestige.
    RunTime,
    /// In a prestige, what it would give now (of its first resource).
    Gain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fun {
    Floor,
    Ceil,
    Round,
    Sqrt,
    Log10,
    Ln,
    Log,
    Min,
    Max,
    Abs,
    Chance,
    If,
    Clamp,
}

impl Fun {
    fn named(s: &str) -> Option<(Fun, usize, usize)> {
        Some(match s {
            "floor" => (Fun::Floor, 1, 1),
            "ceil" => (Fun::Ceil, 1, 1),
            "round" => (Fun::Round, 1, 1),
            "sqrt" => (Fun::Sqrt, 1, 1),
            "log10" => (Fun::Log10, 1, 1),
            "ln" => (Fun::Ln, 1, 1),
            "log" => (Fun::Log, 2, 2),
            "min" => (Fun::Min, 1, 64),
            "max" => (Fun::Max, 1, 64),
            "abs" => (Fun::Abs, 1, 1),
            "chance" => (Fun::Chance, 1, 1),
            "if" => (Fun::If, 3, 3),
            "clamp" => (Fun::Clamp, 3, 3),
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Lit(Num),
    /// A name not resolved yet.
    Name(String),
    /// A call not resolved yet: `total(Gold)`, `sqrt(x)`.
    CallName(String, Vec<Expr>),
    Ref(Ref),
    Call(Fun, Vec<Expr>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
}

/// What expressions read when they run.
pub trait Env {
    fn get(&self, r: Ref) -> Num;
    /// A number in [0, 1) from the game's own dice.
    fn random(&self) -> f64;
}

/// What names mean, when expressions are resolved.
pub trait Names {
    fn thing(&self, name: &str) -> Option<Thing>;
}

fn truth(b: bool) -> Num {
    if b {
        Num::ONE
    } else {
        Num::ZERO
    }
}

impl Expr {
    pub fn num(x: f64) -> Expr {
        Expr::Lit(Num::from_f64(x))
    }

    pub fn parse(text: &str) -> Result<Expr, String> {
        let toks = lex(text)?;
        if toks.is_empty() {
            return Err("an expression is missing".into());
        }
        let mut p = Parser { t: toks, i: 0 };
        let e = p.or()?;
        if p.i < p.t.len() {
            return Err(format!("unexpected `{}`", p.t[p.i].text()));
        }
        Ok(e)
    }

    /// Replaces names by what they read; `extra` resolves the engine's words first.
    pub fn resolve(&mut self, names: &dyn Names, extra: &dyn Fn(&str) -> Option<Ref>) -> Result<(), String> {
        match self {
            Expr::Name(n) => {
                let r = if let Some(r) = extra(n) {
                    r
                } else if let Some(t) = names.thing(n) {
                    if matches!(t, Thing::Rule(_)) {
                        return Err(format!("`{n}` is a rule, which has no value"));
                    }
                    Ref::Thing(t)
                } else {
                    return Err(format!("`{n}` is not declared"));
                };
                *self = Expr::Ref(r);
            }
            Expr::CallName(f, args) => {
                for a in args.iter_mut() {
                    a.resolve(names, extra)?;
                }
                let thing_arg = |args: &[Expr]| -> Result<Thing, String> {
                    match args {
                        [Expr::Ref(Ref::Thing(t))] => Ok(*t),
                        _ => Err(format!("`{f}` takes the name of one thing")),
                    }
                };
                let res_arg = |args: &[Expr]| -> Result<usize, String> {
                    match thing_arg(args)? {
                        Thing::Res(r) => Ok(r),
                        _ => Err(format!("`{f}` takes the name of a resource")),
                    }
                };
                let new = match f.as_str() {
                    "total" => Expr::Ref(Ref::Total(res_arg(args)?)),
                    "ever" => Expr::Ref(Ref::Ever(res_arg(args)?)),
                    "rate" => Expr::Ref(Ref::Rate(res_arg(args)?)),
                    "contrib" => Expr::Ref(Ref::Contrib(res_arg(args)?)),
                    "cap" => Expr::Ref(Ref::Cap(res_arg(args)?)),
                    "sum" => Expr::Ref(Ref::Sum(thing_arg(args)?)),
                    "best" => Expr::Ref(Ref::Best(thing_arg(args)?)),
                    name => {
                        let (fun, lo, hi) = Fun::named(name).ok_or_else(|| format!("no function `{name}`"))?;
                        if args.len() < lo || args.len() > hi {
                            return Err(format!(
                                "`{name}` takes {lo}{} arguments",
                                if hi > lo { "+" } else { "" }
                            ));
                        }
                        Expr::Call(fun, std::mem::take(args))
                    }
                };
                *self = new;
            }
            Expr::Call(_, args) => {
                for a in args {
                    a.resolve(names, extra)?;
                }
            }
            Expr::Neg(a) | Expr::Not(a) => a.resolve(names, extra)?,
            Expr::Bin(_, a, b) => {
                a.resolve(names, extra)?;
                b.resolve(names, extra)?;
            }
            Expr::Lit(_) | Expr::Ref(_) => {}
        }
        Ok(())
    }

    /// Whether the expression reads this kind of reference anywhere.
    pub fn uses(&self, f: &dyn Fn(&Ref) -> bool) -> bool {
        match self {
            Expr::Ref(r) => f(r),
            Expr::Call(_, a) | Expr::CallName(_, a) => a.iter().any(|x| x.uses(f)),
            Expr::Neg(a) | Expr::Not(a) => a.uses(f),
            Expr::Bin(_, a, b) => a.uses(f) || b.uses(f),
            Expr::Lit(_) | Expr::Name(_) => false,
        }
    }

    /// The value as a literal, if the expression is one.
    pub fn constant(&self) -> Option<Num> {
        match self {
            Expr::Lit(n) => Some(*n),
            Expr::Neg(a) => a.constant().map(|n| -n),
            _ => None,
        }
    }

    pub fn eval(&self, env: &dyn Env) -> Num {
        match self {
            Expr::Lit(n) => *n,
            Expr::Ref(r) => env.get(*r),
            Expr::Name(_) | Expr::CallName(..) => Num::ZERO,
            Expr::Neg(a) => -a.eval(env),
            Expr::Not(a) => truth(a.eval(env).is_zero()),
            Expr::Bin(op, a, b) => {
                match op {
                    Op::And => return truth(!a.eval(env).is_zero() && !b.eval(env).is_zero()),
                    Op::Or => return truth(!a.eval(env).is_zero() || !b.eval(env).is_zero()),
                    _ => {}
                }
                let (x, y) = (a.eval(env), b.eval(env));
                match op {
                    Op::Add => x + y,
                    Op::Sub => x - y,
                    Op::Mul => x * y,
                    Op::Div => x / y,
                    Op::Pow => x.pow(y),
                    Op::Lt => truth(x < y),
                    Op::Le => truth(x <= y),
                    Op::Gt => truth(x > y),
                    Op::Ge => truth(x >= y),
                    Op::Eq => truth(x == y),
                    Op::Ne => truth(x != y),
                    Op::And | Op::Or => unreachable!(),
                }
            }
            Expr::Call(f, args) => {
                let v = |i: usize| args[i].eval(env);
                match f {
                    Fun::Floor => v(0).floor(),
                    Fun::Ceil => v(0).ceil(),
                    Fun::Round => v(0).round(),
                    Fun::Sqrt => v(0).sqrt(),
                    Fun::Log10 => {
                        let x = v(0);
                        if x.is_positive() {
                            Num::from_f64(x.log10())
                        } else {
                            Num::ZERO
                        }
                    }
                    Fun::Ln => {
                        let x = v(0);
                        if x.is_positive() {
                            Num::from_f64(x.ln())
                        } else {
                            Num::ZERO
                        }
                    }
                    Fun::Log => {
                        let (x, b) = (v(0), v(1));
                        if x.is_positive() && b.is_positive() && b != Num::ONE {
                            Num::from_f64(x.ln() / b.ln())
                        } else {
                            Num::ZERO
                        }
                    }
                    Fun::Min => args.iter().map(|a| a.eval(env)).reduce(Num::min).unwrap_or(Num::ZERO),
                    Fun::Max => args.iter().map(|a| a.eval(env)).reduce(Num::max).unwrap_or(Num::ZERO),
                    Fun::Abs => v(0).abs(),
                    Fun::Chance => truth(env.random() < v(0).to_f64()),
                    Fun::If => {
                        if v(0).is_zero() {
                            v(2)
                        } else {
                            v(1)
                        }
                    }
                    Fun::Clamp => v(0).max(v(1)).min(v(2)),
                }
            }
        }
    }

    pub fn test(&self, env: &dyn Env) -> bool {
        !self.eval(env).is_zero()
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(Num),
    Ident(String),
    Sym(&'static str),
}

impl Tok {
    fn text(&self) -> String {
        match self {
            Tok::Num(n) => n.to_string(),
            Tok::Ident(s) => s.clone(),
            Tok::Sym(s) => s.to_string(),
        }
    }
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch.is_ascii_digit() || (ch == '.' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.' || c[i] == '_') {
                i += 1;
            }
            if i < c.len() && (c[i] == 'e' || c[i] == 'E') {
                let mut j = i + 1;
                if j < c.len() && (c[j] == '+' || c[j] == '-') {
                    j += 1;
                }
                if j < c.len() && c[j].is_ascii_digit() {
                    i = j;
                    while i < c.len() && c[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text: String = c[start..i].iter().collect();
            out.push(Tok::Num(text.parse()?));
        } else if ch.is_alphabetic() || ch == '_' {
            let start = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            let w: String = c[start..i].iter().collect();
            out.push(match w.as_str() {
                "atLeast" => Tok::Sym(">="),
                "atMost" => Tok::Sym("<="),
                "greater" => Tok::Sym(">"),
                "less" => Tok::Sym("<"),
                "is" => Tok::Sym("=="),
                "and" => Tok::Sym("and"),
                "or" => Tok::Sym("or"),
                "not" => Tok::Sym("not"),
                _ => Tok::Ident(w),
            });
        } else {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            let sym = match two.as_str() {
                ">=" => Some(">="),
                "<=" => Some("<="),
                "==" => Some("=="),
                "!=" => Some("!="),
                "&&" => Some("and"),
                "||" => Some("or"),
                "**" => Some("^"),
                _ => None,
            };
            if let Some(s) = sym {
                out.push(Tok::Sym(s));
                i += 2;
                continue;
            }
            let s = match ch {
                '+' => "+",
                '-' => "-",
                '*' => "*",
                '/' => "/",
                '^' => "^",
                '(' => "(",
                ')' => ")",
                ',' => ",",
                '<' => "<",
                '>' => ">",
                '=' => "==",
                '!' => "not",
                _ => return Err(format!("unexpected `{ch}`")),
            };
            out.push(Tok::Sym(s));
            i += 1;
        }
    }
    Ok(out)
}

struct Parser {
    t: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn peek_sym(&self) -> Option<&'static str> {
        match self.t.get(self.i) {
            Some(Tok::Sym(s)) => Some(s),
            _ => None,
        }
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.peek_sym() == Some(s) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut a = self.and()?;
        while self.eat("or") {
            a = Expr::Bin(Op::Or, Box::new(a), Box::new(self.and()?));
        }
        Ok(a)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut a = self.not()?;
        while self.eat("and") {
            a = Expr::Bin(Op::And, Box::new(a), Box::new(self.not()?));
        }
        Ok(a)
    }

    fn not(&mut self) -> Result<Expr, String> {
        if self.eat("not") {
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Result<Expr, String> {
        let a = self.add()?;
        let op = match self.peek_sym() {
            Some("<") => Op::Lt,
            Some("<=") => Op::Le,
            Some(">") => Op::Gt,
            Some(">=") => Op::Ge,
            Some("==") => Op::Eq,
            Some("!=") => Op::Ne,
            _ => return Ok(a),
        };
        self.i += 1;
        Ok(Expr::Bin(op, Box::new(a), Box::new(self.add()?)))
    }

    fn add(&mut self) -> Result<Expr, String> {
        let mut a = self.mul()?;
        loop {
            let op = match self.peek_sym() {
                Some("+") => Op::Add,
                Some("-") => Op::Sub,
                _ => return Ok(a),
            };
            self.i += 1;
            a = Expr::Bin(op, Box::new(a), Box::new(self.mul()?));
        }
    }

    fn mul(&mut self) -> Result<Expr, String> {
        let mut a = self.unary()?;
        loop {
            let op = match self.peek_sym() {
                Some("*") => Op::Mul,
                Some("/") => Op::Div,
                _ => return Ok(a),
            };
            self.i += 1;
            a = Expr::Bin(op, Box::new(a), Box::new(self.unary()?));
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat("-") {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        if self.eat("+") {
            return self.unary();
        }
        self.pow()
    }

    fn pow(&mut self) -> Result<Expr, String> {
        let a = self.atom()?;
        if self.eat("^") {
            return Ok(Expr::Bin(Op::Pow, Box::new(a), Box::new(self.unary()?)));
        }
        Ok(a)
    }

    fn atom(&mut self) -> Result<Expr, String> {
        let t = self.t.get(self.i).cloned().ok_or("the expression ends too soon")?;
        self.i += 1;
        match t {
            Tok::Num(n) => Ok(Expr::Lit(n)),
            Tok::Ident(name) => {
                if self.eat("(") {
                    let mut args = Vec::new();
                    if !self.eat(")") {
                        loop {
                            args.push(self.or()?);
                            if self.eat(")") {
                                break;
                            }
                            if !self.eat(",") {
                                return Err(format!("`,` or `)` expected in `{name}(...)`"));
                            }
                        }
                    }
                    Ok(Expr::CallName(name, args))
                } else {
                    Ok(Expr::Name(name))
                }
            }
            Tok::Sym("(") => {
                let e = self.or()?;
                if !self.eat(")") {
                    return Err("`)` expected".into());
                }
                Ok(e)
            }
            Tok::Sym(s) => Err(format!("unexpected `{s}`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct E(Cell<u32>);
    impl Env for E {
        fn get(&self, r: Ref) -> Num {
            match r {
                Ref::Thing(Thing::Res(0)) => Num::from_f64(50.0),
                Ref::Owned => Num::from_f64(3.0),
                _ => Num::ZERO,
            }
        }
        fn random(&self) -> f64 {
            self.0.set(self.0.get() + 1);
            0.5
        }
    }
    struct N;
    impl Names for N {
        fn thing(&self, n: &str) -> Option<Thing> {
            (n == "Gold").then_some(Thing::Res(0))
        }
    }

    fn ev(s: &str) -> f64 {
        let mut e = Expr::parse(s).unwrap();
        e.resolve(&N, &|w| (w == "owned").then_some(Ref::Owned)).unwrap();
        e.eval(&E(Cell::new(0))).to_f64()
    }

    #[test]
    fn evaluates() {
        assert_eq!(ev("1 + 2 * 3"), 7.0);
        assert_eq!(ev("-2 ^ 2"), -4.0);
        assert_eq!(ev("2 ^ 3 ^ 2"), 512.0);
        assert_eq!(ev("10 * 2 ^ owned"), 80.0);
        assert_eq!(ev("Gold atLeast 50 and not Gold > 60"), 1.0);
        assert_eq!(ev("if(Gold < 10, 1, 2)"), 2.0);
        assert_eq!(ev("max(1, Gold, 3)"), 50.0);
        assert_eq!(ev("floor(sqrt(1e6 / 100))"), 100.0);
        assert_eq!(ev("1_000 + 1e3"), 2000.0);
        assert_eq!(ev("chance(0.7)"), 1.0);
    }

    #[test]
    fn errors() {
        let mut e = Expr::parse("Silver + 1").unwrap();
        assert!(e.resolve(&N, &|_| None).unwrap_err().contains("Silver"));
        assert!(Expr::parse("1 +").is_err());
        assert!(Expr::parse("(1").is_err());
        let mut e = Expr::parse("sqrt(1, 2)").unwrap();
        assert!(e.resolve(&N, &|_| None).is_err());
    }
}
