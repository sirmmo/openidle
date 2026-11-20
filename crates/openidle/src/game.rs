//! A game: what `game.toml`, `params.toml` and the scripts declare, checked and compiled.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::Deserialize;

use crate::expr::{Expr, Names, Ref, Thing};
use crate::num::Num;
use crate::script::{self, Block, Error, Line};

/// Whose a thing is: each player's own, or one for the whole world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Player,
    World,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::Player => "player",
            Scope::World => "world",
        }
    }
}

/// What every visible thing has.
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub id: String,
    pub label: String,
    pub text: String,
    /// Free `view key value` lines, for the page that draws the game.
    pub view: BTreeMap<String, String>,
}

/// An amount of a resource.
#[derive(Clone, Debug)]
pub struct Amount {
    pub res: usize,
    pub expr: Expr,
}

#[derive(Clone, Debug)]
pub struct Resource {
    pub info: Info,
    pub scope: Scope,
    pub start: Expr,
    pub cap: Option<Expr>,
    pub giftable: bool,
    pub hidden: bool,
}

#[derive(Clone, Debug)]
pub struct Generator {
    pub info: Info,
    pub costs: Vec<Amount>,
    pub max: Option<Expr>,
    pub start: u64,
    /// Boosts (discounts) on its costs.
    pub discounts: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub id: String,
    pub scope: Scope,
    pub applies: Vec<usize>,
    pub tests: Vec<Expr>,
    pub ins: Vec<Amount>,
    pub outs: Vec<Amount>,
    /// For each output, the boosts that multiply it.
    pub out_boosts: Vec<Vec<usize>>,
}

#[derive(Clone, Debug)]
pub enum EffectOp {
    Add,
    Set,
    Mul,
}

/// A one-off change: `do add Gold 100`.
#[derive(Clone, Debug)]
pub struct Effect {
    pub op: EffectOp,
    pub target: Thing,
    pub value: Expr,
}

#[derive(Clone, Debug)]
pub struct Upgrade {
    pub info: Info,
    pub scope: Scope,
    pub costs: Vec<Amount>,
    pub need: Vec<Expr>,
    pub effects: Vec<Effect>,
    pub auto: bool,
    pub discounts: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Action {
    pub info: Info,
    pub costs: Vec<Amount>,
    pub gains: Vec<Amount>,
    pub gain_boosts: Vec<Vec<usize>>,
    pub need: Vec<Expr>,
    /// In ticks.
    pub cooldown: u64,
    /// In ticks: how long its boosts last once used.
    pub lasts: u64,
    pub effects: Vec<Effect>,
}

#[derive(Clone, Debug)]
pub struct Milestone {
    pub info: Info,
    pub scope: Scope,
    pub conds: Vec<Expr>,
    pub effects: Vec<Effect>,
}

#[derive(Clone, Debug)]
pub struct Prestige {
    pub info: Info,
    pub gains: Vec<Amount>,
    pub need: Vec<Expr>,
    pub keep: Vec<Thing>,
    pub reset: Vec<Thing>,
    pub effects: Vec<Effect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    All,
    Actions,
    Thing(Thing),
}

/// A multiplier from a thing: on production (`boost`) or on costs (`discount`).
#[derive(Clone, Debug)]
pub struct Boost {
    pub source: Thing,
    pub target: Target,
    pub discount: bool,
    pub value: Expr,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    name: String,
    #[serde(default)]
    description: String,
    scripts: Vec<String>,
    #[serde(default = "default_tick")]
    tick: f64,
    #[serde(default = "default_offline")]
    offline_max: f64,
    #[serde(default)]
    score: Option<String>,
}

fn default_tick() -> f64 {
    0.2
}

fn default_offline() -> f64 {
    86_400.0
}

#[derive(Clone, Debug)]
pub struct Game {
    pub name: String,
    pub description: String,
    /// Seconds a tick lasts.
    pub tick: f64,
    pub dt: Num,
    /// Most seconds of progress given for time away (single player).
    pub offline_max: f64,
    /// What ranks players (`players` in a view).
    pub score: Option<Expr>,
    pub params: BTreeMap<String, String>,
    pub resources: Vec<Resource>,
    pub generators: Vec<Generator>,
    pub rules: Vec<Rule>,
    pub upgrades: Vec<Upgrade>,
    pub actions: Vec<Action>,
    pub milestones: Vec<Milestone>,
    pub prestiges: Vec<Prestige>,
    pub boosts: Vec<Boost>,
    /// `show if` of every item (see [`Game::item`]); none: always shown.
    pub shows: Vec<Option<Expr>>,
    pub ids: HashMap<String, Thing>,
    /// The files it was made from, to send to browsers (`Game::from_files` reads them back).
    pub files: BTreeMap<String, String>,
    pub sets: Vec<(String, String)>,
}

struct Ids<'a>(&'a HashMap<String, Thing>);

impl Names for Ids<'_> {
    fn thing(&self, name: &str) -> Option<Thing> {
        self.0.get(name).copied()
    }
}

const RESERVED: &[&str] = &[
    "owned", "players", "time", "runtime", "gain", "all", "actions", "and", "or", "not", "is", "if",
];

fn general(w: &str) -> Option<Ref> {
    match w {
        "players" => Some(Ref::Players),
        "time" => Some(Ref::Time),
        "runtime" => Some(Ref::RunTime),
        _ => None,
    }
}

fn with_owned(w: &str) -> Option<Ref> {
    if w == "owned" {
        Some(Ref::Owned)
    } else {
        general(w)
    }
}

fn with_gain(w: &str) -> Option<Ref> {
    if w == "gain" {
        Some(Ref::Gain)
    } else {
        general(w)
    }
}

impl Game {
    /// Reads a game folder: `game.toml`, `params.toml` (if any) and the scripts it lists.
    pub fn from_dir(dir: &Path, sets: &[(String, String)]) -> Result<Game, Error> {
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name)).map_err(|e| Error::new(name, 0, format!("cannot read: {e}")))
        };
        let mut files = BTreeMap::new();
        let manifest = read("game.toml")?;
        if dir.join("params.toml").exists() {
            files.insert("params.toml".to_string(), read("params.toml")?);
        }
        let raw: toml::Table = toml::from_str(&manifest).map_err(|e| Error::new("game.toml", 0, e.to_string()))?;
        if let Some(list) = raw.get("scripts").and_then(|s| s.as_array()) {
            for s in list.iter().filter_map(|s| s.as_str()) {
                files.insert(s.to_string(), read(s)?);
            }
        }
        files.insert("game.toml".to_string(), manifest);
        Game::from_files(&files, sets)
    }

    /// Builds a game from its files in memory (names as `game.toml` lists them), with parameter
    /// values in `sets` replacing those of `params.toml`.
    pub fn from_files(files: &BTreeMap<String, String>, sets: &[(String, String)]) -> Result<Game, Error> {
        let mut params = BTreeMap::new();
        if let Some(text) = files.get("params.toml") {
            let t: toml::Table = toml::from_str(text).map_err(|e| Error::new("params.toml", 0, e.to_string()))?;
            for (k, v) in t {
                let s = match v {
                    toml::Value::String(s) => s,
                    toml::Value::Integer(i) => i.to_string(),
                    toml::Value::Float(f) => f.to_string(),
                    toml::Value::Boolean(b) => (if b { "1" } else { "0" }).to_string(),
                    _ => {
                        return Err(Error::new(
                            "params.toml",
                            0,
                            format!("`{k}` must be a number, a string or a boolean"),
                        ))
                    }
                };
                params.insert(k, s);
            }
        }
        for (k, v) in sets {
            if !params.contains_key(k) {
                return Err(Error::new("params.toml", 0, format!("no parameter `{k}` to set")));
            }
            params.insert(k.clone(), v.clone());
        }
        let manifest_text = files
            .get("game.toml")
            .ok_or_else(|| Error::new("game.toml", 0, "missing"))?;
        let manifest_text = script::substitute_file("game.toml", manifest_text, &params)?;
        let m: Manifest = toml::from_str(&manifest_text).map_err(|e| Error::new("game.toml", 0, e.to_string()))?;
        if !(m.tick > 0.0 && m.tick <= 60.0) {
            return Err(Error::new("game.toml", 0, "`tick` must be between 0 and 60 seconds"));
        }
        let mut blocks = Vec::new();
        for s in &m.scripts {
            let text = files.get(s).ok_or_else(|| Error::new(s, 0, "missing"))?;
            let text = script::substitute_file(s, text, &params)?;
            blocks.extend(script::blocks(s, &text)?);
        }
        let mut g = Game {
            name: m.name,
            description: m.description,
            tick: m.tick,
            dt: Num::from_f64(m.tick),
            offline_max: m.offline_max,
            score: None,
            params,
            resources: Vec::new(),
            generators: Vec::new(),
            rules: Vec::new(),
            upgrades: Vec::new(),
            actions: Vec::new(),
            milestones: Vec::new(),
            prestiges: Vec::new(),
            boosts: Vec::new(),
            shows: Vec::new(),
            ids: HashMap::new(),
            files: files.clone(),
            sets: sets.to_vec(),
        };
        g.compile(&blocks)?;
        if let Some(s) = &m.score {
            let mut e = Expr::parse(s).map_err(|m| Error::new("game.toml", 0, format!("score: {m}")))?;
            e.resolve(&Ids(&g.ids), &general)
                .map_err(|m| Error::new("game.toml", 0, format!("score: {m}")))?;
            g.score = Some(e);
        }
        Ok(g)
    }

    /// The files as one JSON object, which `from_bundle` reads back (what browsers load).
    pub fn bundle(&self) -> serde_json::Value {
        serde_json::json!({ "files": self.files, "sets": self.sets.iter().map(|(k, v)| [k, v]).collect::<Vec<_>>() })
    }

    pub fn from_bundle(v: &serde_json::Value) -> Result<Game, Error> {
        let files: BTreeMap<String, String> = serde_json::from_value(v["files"].clone())
            .map_err(|e| Error::new("", 0, format!("not a game bundle: {e}")))?;
        let sets: Vec<(String, String)> = v["sets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|p| Some((p[0].as_str()?.to_string(), p[1].as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        Game::from_files(&files, &sets)
    }

    /// The index of a thing among the items that can be shown (all but rules).
    pub fn item(&self, t: Thing) -> Option<usize> {
        let (r, g, u, a, m) = (
            self.resources.len(),
            self.generators.len(),
            self.upgrades.len(),
            self.actions.len(),
            self.milestones.len(),
        );
        Some(match t {
            Thing::Res(i) => i,
            Thing::Gen(i) => r + i,
            Thing::Upg(i) => r + g + i,
            Thing::Act(i) => r + g + u + i,
            Thing::Mile(i) => r + g + u + a + i,
            Thing::Pres(i) => r + g + u + a + m + i,
            Thing::Rule(_) => return None,
        })
    }

    pub fn items(&self) -> usize {
        self.resources.len()
            + self.generators.len()
            + self.upgrades.len()
            + self.actions.len()
            + self.milestones.len()
            + self.prestiges.len()
    }

    pub fn thing(&self, id: &str) -> Option<Thing> {
        self.ids.get(id).copied()
    }

    pub fn id_of(&self, t: Thing) -> &str {
        match t {
            Thing::Res(i) => &self.resources[i].info.id,
            Thing::Gen(i) => &self.generators[i].info.id,
            Thing::Rule(i) => &self.rules[i].id,
            Thing::Upg(i) => &self.upgrades[i].info.id,
            Thing::Act(i) => &self.actions[i].info.id,
            Thing::Mile(i) => &self.milestones[i].info.id,
            Thing::Pres(i) => &self.prestiges[i].info.id,
        }
    }

    pub fn label_of(&self, t: Thing) -> &str {
        match t {
            Thing::Res(i) => &self.resources[i].info.label,
            Thing::Gen(i) => &self.generators[i].info.label,
            Thing::Rule(i) => &self.rules[i].id,
            Thing::Upg(i) => &self.upgrades[i].info.label,
            Thing::Act(i) => &self.actions[i].info.label,
            Thing::Mile(i) => &self.milestones[i].info.label,
            Thing::Pres(i) => &self.prestiges[i].info.label,
        }
    }

    /// Whose the thing is.
    pub fn scope_of(&self, t: Thing) -> Scope {
        match t {
            Thing::Res(i) => self.resources[i].scope,
            Thing::Upg(i) => self.upgrades[i].scope,
            Thing::Mile(i) => self.milestones[i].scope,
            Thing::Rule(i) => self.rules[i].scope,
            Thing::Gen(_) | Thing::Act(_) | Thing::Pres(_) => Scope::Player,
        }
    }

    fn seconds_to_ticks(&self, s: f64) -> u64 {
        if s <= 0.0 {
            0
        } else {
            (s / self.tick - 1e-9).ceil() as u64
        }
    }

    fn compile(&mut self, blocks: &[Block]) -> Result<(), Error> {
        // First every id, so that blocks can name things declared after them.
        let mut counts = [0usize; 7];
        for b in blocks {
            let k = script::KINDS.iter().position(|k| *k == b.kind).unwrap();
            let t = match k {
                0 => Thing::Res(counts[0]),
                1 => Thing::Gen(counts[1]),
                2 => Thing::Rule(counts[2]),
                3 => Thing::Upg(counts[3]),
                4 => Thing::Act(counts[4]),
                5 => Thing::Mile(counts[5]),
                _ => Thing::Pres(counts[6]),
            };
            counts[k] += 1;
            if RESERVED.contains(&b.id.as_str()) {
                return Err(Error::new(&b.file, b.no, format!("`{}` is a reserved word", b.id)));
            }
            if self.ids.insert(b.id.clone(), t).is_some() {
                return Err(Error::new(&b.file, b.no, format!("`{}` is declared twice", b.id)));
            }
        }
        let mut shows: HashMap<Thing, Expr> = HashMap::new();
        for b in blocks {
            let me = self.ids[&b.id];
            let mut c = Compiler {
                g: self,
                b,
                me,
                info: Info {
                    id: b.id.clone(),
                    label: b.id.replace('_', " "),
                    ..Default::default()
                },
            };
            let show = c.compile()?;
            if let Some(s) = show {
                shows.insert(me, s);
            }
        }
        // Which boosts touch which outputs.
        for (ri, rule) in self.rules.iter_mut().enumerate() {
            rule.out_boosts = rule
                .outs
                .iter()
                .map(|o| {
                    self.boosts
                        .iter()
                        .enumerate()
                        .filter(|(_, b)| {
                            !b.discount
                                && match b.target {
                                    Target::All => true,
                                    Target::Actions => false,
                                    Target::Thing(Thing::Res(r)) => r == o.res,
                                    Target::Thing(Thing::Rule(r)) => r == ri,
                                    Target::Thing(Thing::Gen(g)) => rule.applies.contains(&g),
                                    Target::Thing(_) => false,
                                }
                        })
                        .map(|(i, _)| i)
                        .collect()
                })
                .collect();
        }
        for (ai, act) in self.actions.iter_mut().enumerate() {
            act.gain_boosts = act
                .gains
                .iter()
                .map(|o| {
                    self.boosts
                        .iter()
                        .enumerate()
                        .filter(|(_, b)| {
                            !b.discount
                                && match b.target {
                                    Target::All | Target::Actions => true,
                                    Target::Thing(Thing::Res(r)) => r == o.res,
                                    Target::Thing(Thing::Act(a)) => a == ai,
                                    Target::Thing(_) => false,
                                }
                        })
                        .map(|(i, _)| i)
                        .collect()
                })
                .collect();
        }
        let discounts = |t: Thing, boosts: &[Boost]| -> Vec<usize> {
            boosts
                .iter()
                .enumerate()
                .filter(|(_, b)| b.discount && (b.target == Target::All || b.target == Target::Thing(t)))
                .map(|(i, _)| i)
                .collect()
        };
        for i in 0..self.generators.len() {
            self.generators[i].discounts = discounts(Thing::Gen(i), &self.boosts);
        }
        for i in 0..self.upgrades.len() {
            self.upgrades[i].discounts = discounts(Thing::Upg(i), &self.boosts);
        }
        let mut list = vec![None; self.items()];
        for (t, e) in shows {
            list[self.item(t).unwrap()] = Some(e);
        }
        self.shows = list;
        Ok(())
    }
}

struct Compiler<'a> {
    g: &'a mut Game,
    b: &'a Block,
    me: Thing,
    info: Info,
}

impl Compiler<'_> {
    fn expr(&self, l: &Line, text: &str, extra: &dyn Fn(&str) -> Option<Ref>) -> Result<Expr, Error> {
        let mut e = Expr::parse(text).map_err(|m| self.b.err(l, m))?;
        e.resolve(&Ids(&self.g.ids), extra).map_err(|m| self.b.err(l, m))?;
        Ok(e)
    }

    fn resource(&self, l: &Line, name: &str) -> Result<usize, Error> {
        match self.g.ids.get(name) {
            Some(Thing::Res(r)) => Ok(*r),
            Some(_) => Err(self.b.err(l, format!("`{name}` is not a resource"))),
            None => Err(self.b.err(l, format!("no resource `{name}`"))),
        }
    }

    /// `Gold 10 * 2 ^ owned`: a resource and an expression.
    fn amount(&self, l: &Line, extra: &dyn Fn(&str) -> Option<Ref>) -> Result<Amount, Error> {
        let (name, rest) = l
            .rest
            .split_once(char::is_whitespace)
            .ok_or_else(|| self.b.err(l, format!("`{}` needs a resource and an amount", l.key)))?;
        Ok(Amount {
            res: self.resource(l, name)?,
            expr: self.expr(l, rest, extra)?,
        })
    }

    fn scope(&self, l: &Line) -> Result<Scope, Error> {
        match l.rest.as_str() {
            "player" => Ok(Scope::Player),
            "world" | "shared" => Ok(Scope::World),
            s => Err(self.b.err(l, format!("scope is `player` or `world`, not `{s}`"))),
        }
    }

    fn things(&self, l: &Line) -> Result<Vec<Thing>, Error> {
        l.rest
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .map(|s| {
                self.g
                    .ids
                    .get(s)
                    .copied()
                    .ok_or_else(|| self.b.err(l, format!("`{s}` is not declared")))
            })
            .collect()
    }

    fn seconds(&self, l: &Line) -> Result<u64, Error> {
        let e = self.expr(l, &l.rest, &|_| None)?;
        let s = e
            .constant()
            .ok_or_else(|| self.b.err(l, "a number of seconds is expected"))?;
        Ok(self.g.seconds_to_ticks(s.to_f64()))
    }

    fn effect(&self, l: &Line, extra: &dyn Fn(&str) -> Option<Ref>) -> Result<Effect, Error> {
        let mut w = l.rest.splitn(3, char::is_whitespace);
        let op = match w.next() {
            Some("add") => EffectOp::Add,
            Some("set") => EffectOp::Set,
            Some("mul") => EffectOp::Mul,
            _ => {
                return Err(self
                    .b
                    .err(l, "`do` is `do add|set|mul <resource or generator> <amount>`"))
            }
        };
        let name = w.next().unwrap_or("");
        let target = match self.g.ids.get(name) {
            Some(t @ (Thing::Res(_) | Thing::Gen(_))) => *t,
            _ => {
                return Err(self
                    .b
                    .err(l, format!("`do` changes a resource or a generator, not `{name}`")))
            }
        };
        let value = self.expr(l, w.next().unwrap_or(""), extra)?;
        Ok(Effect { op, target, value })
    }

    /// `boost <target> <expr>` and `discount <target> <expr>`.
    fn boost(&mut self, l: &Line, discount: bool, extra: &dyn Fn(&str) -> Option<Ref>) -> Result<(), Error> {
        let (name, rest) = l
            .rest
            .split_once(char::is_whitespace)
            .ok_or_else(|| self.b.err(l, format!("`{}` needs a target and a factor", l.key)))?;
        let target = match name {
            "all" => Target::All,
            "actions" if !discount => Target::Actions,
            n => {
                let t = self
                    .g
                    .ids
                    .get(n)
                    .copied()
                    .ok_or_else(|| self.b.err(l, format!("`{n}` is not declared")))?;
                let ok = if discount {
                    matches!(t, Thing::Gen(_) | Thing::Upg(_))
                } else {
                    matches!(t, Thing::Res(_) | Thing::Gen(_) | Thing::Rule(_) | Thing::Act(_))
                };
                if !ok {
                    return Err(self.b.err(
                        l,
                        if discount {
                            format!("a discount is on a generator, an upgrade or `all`, not `{n}`")
                        } else {
                            format!("a boost is on a resource, a generator, a rule, an action, `actions` or `all`, not `{n}`")
                        },
                    ));
                }
                Target::Thing(t)
            }
        };
        let value = self.expr(l, rest, extra)?;
        self.g.boosts.push(Boost {
            source: self.me,
            target,
            discount,
            value,
        });
        Ok(())
    }

    /// Lines every visible thing may have; true if the line was one of them.
    fn common(&mut self, l: &Line, show: &mut Option<Expr>) -> Result<bool, Error> {
        match l.key.as_str() {
            "label" => self.info.label = unquote(&l.rest),
            "text" => {
                if !self.info.text.is_empty() {
                    self.info.text.push(' ');
                }
                self.info.text.push_str(&unquote(&l.rest));
            }
            "view" => {
                let (k, v) = l.rest.split_once(char::is_whitespace).unwrap_or((&l.rest, ""));
                self.info.view.insert(k.to_string(), unquote(v.trim()));
            }
            "show" => {
                let t = l.rest.strip_prefix("if ").unwrap_or(&l.rest);
                *show = Some(self.expr(l, t, &general)?);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn unknown(&self, l: &Line) -> Error {
        self.b.err(l, format!("`{}` is not a line of a {}", l.key, self.b.kind))
    }

    fn compile(&mut self) -> Result<Option<Expr>, Error> {
        let mut show = None;
        let b = self.b;
        match self.me {
            Thing::Res(_) => {
                let mut r = Resource {
                    info: Info::default(),
                    scope: Scope::Player,
                    start: Expr::num(0.0),
                    cap: None,
                    giftable: false,
                    hidden: false,
                };
                for l in &b.lines {
                    if self.common(l, &mut show)? {
                        continue;
                    }
                    match l.key.as_str() {
                        "scope" => r.scope = self.scope(l)?,
                        "start" => r.start = self.expr(l, &l.rest, &general)?,
                        "max" | "cap" => {
                            let e = self.expr(l, &l.rest, &general)?;
                            if e.uses(&|r| matches!(r, Ref::Cap(_))) {
                                return Err(self.b.err(l, "a cap cannot read caps"));
                            }
                            r.cap = Some(e)
                        }
                        "giftable" => r.giftable = true,
                        "hidden" => r.hidden = true,
                        "boost" => self.boost(l, false, &general)?,
                        "discount" => self.boost(l, true, &general)?,
                        _ => return Err(self.unknown(l)),
                    }
                }
                if r.giftable && r.scope == Scope::World {
                    return Err(Error::new(&b.file, b.no, "a world resource cannot be giftable"));
                }
                r.info = std::mem::take(&mut self.info);
                self.g.resources.push(r);
            }
            Thing::Gen(_) => {
                let mut g = Generator {
                    info: Info::default(),
                    costs: Vec::new(),
                    max: None,
                    start: 0,
                    discounts: Vec::new(),
                };
                for l in &b.lines {
                    if self.common(l, &mut show)? {
                        continue;
                    }
                    match l.key.as_str() {
                        "cost" => g.costs.push(self.amount(l, &with_owned)?),
                        "max" => g.max = Some(self.expr(l, &l.rest, &general)?),
                        "start" => {
                            g.start = self
                                .expr(l, &l.rest, &|_| None)?
                                .constant()
                                .ok_or_else(|| self.b.err(l, "`start` is a number"))?
                                .to_u64()
                        }
                        "boost" => self.boost(l, false, &general)?,
                        "discount" => self.boost(l, true, &general)?,
                        _ => return Err(self.unknown(l)),
                    }
                }
                g.info = std::mem::take(&mut self.info);
                self.g.generators.push(g);
            }
            Thing::Rule(_) => {
                let mut r = Rule {
                    id: b.id.clone(),
                    scope: Scope::Player,
                    applies: Vec::new(),
                    tests: Vec::new(),
                    ins: Vec::new(),
                    outs: Vec::new(),
                    out_boosts: Vec::new(),
                };
                for l in &b.lines {
                    match l.key.as_str() {
                        "scope" => r.scope = self.scope(l)?,
                        "appliesTo" | "applies" => {
                            for t in self.things(l)? {
                                match t {
                                    Thing::Gen(g) => r.applies.push(g),
                                    _ => return Err(self.b.err(l, "a rule applies to generators")),
                                }
                            }
                        }
                        "if" | "test" => {
                            let t = l.rest.strip_prefix("if ").unwrap_or(&l.rest);
                            r.tests.push(self.expr(l, t, &general)?)
                        }
                        "in" => r.ins.push(self.amount(l, &general)?),
                        "out" => r.outs.push(self.amount(l, &general)?),
                        _ => return Err(self.b.err(l, format!("`{}` is not a line of a rule", l.key))),
                    }
                }
                if r.scope == Scope::World {
                    if !r.applies.is_empty() {
                        return Err(Error::new(&b.file, b.no, "a world rule applies to no generators"));
                    }
                    if r.ins
                        .iter()
                        .chain(&r.outs)
                        .any(|a| self.g.resources[a.res].scope != Scope::World)
                    {
                        return Err(Error::new(&b.file, b.no, "a world rule moves only world resources"));
                    }
                }
                self.g.rules.push(r);
            }
            Thing::Upg(_) => {
                let mut u = Upgrade {
                    info: Info::default(),
                    scope: Scope::Player,
                    costs: Vec::new(),
                    need: Vec::new(),
                    effects: Vec::new(),
                    auto: false,
                    discounts: Vec::new(),
                };
                for l in &b.lines {
                    if self.common(l, &mut show)? {
                        continue;
                    }
                    match l.key.as_str() {
                        "scope" => u.scope = self.scope(l)?,
                        "cost" => u.costs.push(self.amount(l, &general)?),
                        "if" | "need" => u.need.push(self.expr(l, &l.rest, &general)?),
                        "do" => u.effects.push(self.effect(l, &general)?),
                        "auto" => u.auto = true,
                        "boost" => self.boost(l, false, &general)?,
                        "discount" => self.boost(l, true, &general)?,
                        _ => return Err(self.unknown(l)),
                    }
                }
                if u.scope == Scope::World {
                    if u.auto && u.costs.iter().any(|a| self.g.resources[a.res].scope != Scope::World) {
                        return Err(Error::new(
                            &b.file,
                            b.no,
                            "an `auto` world upgrade costs only world resources",
                        ));
                    }
                    self.world_effects(&u.effects)?;
                }
                u.info = std::mem::take(&mut self.info);
                self.g.upgrades.push(u);
            }
            Thing::Act(_) => {
                let mut a = Action {
                    info: Info::default(),
                    costs: Vec::new(),
                    gains: Vec::new(),
                    gain_boosts: Vec::new(),
                    need: Vec::new(),
                    cooldown: 0,
                    lasts: 0,
                    effects: Vec::new(),
                };
                for l in &b.lines {
                    if self.common(l, &mut show)? {
                        continue;
                    }
                    match l.key.as_str() {
                        "in" | "cost" => a.costs.push(self.amount(l, &general)?),
                        "out" | "gain" => a.gains.push(self.amount(l, &general)?),
                        "if" | "need" => a.need.push(self.expr(l, &l.rest, &general)?),
                        "cooldown" => a.cooldown = self.seconds(l)?,
                        "lasts" => a.lasts = self.seconds(l)?,
                        "do" => a.effects.push(self.effect(l, &general)?),
                        "boost" => self.boost(l, false, &general)?,
                        "discount" => self.boost(l, true, &general)?,
                        _ => return Err(self.unknown(l)),
                    }
                }
                let has_boost = self.g.boosts.iter().any(|x| x.source == self.me);
                if has_boost && a.lasts == 0 {
                    return Err(Error::new(&b.file, b.no, "an action's boosts need `lasts` (seconds)"));
                }
                a.info = std::mem::take(&mut self.info);
                self.g.actions.push(a);
            }
            Thing::Mile(_) => {
                let mut m = Milestone {
                    info: Info::default(),
                    scope: Scope::Player,
                    conds: Vec::new(),
                    effects: Vec::new(),
                };
                for l in &b.lines {
                    if self.common(l, &mut show)? {
                        continue;
                    }
                    match l.key.as_str() {
                        "scope" => m.scope = self.scope(l)?,
                        "if" => m.conds.push(self.expr(l, &l.rest, &general)?),
                        "do" => m.effects.push(self.effect(l, &general)?),
                        "boost" => self.boost(l, false, &general)?,
                        "discount" => self.boost(l, true, &general)?,
                        _ => return Err(self.unknown(l)),
                    }
                }
                if m.conds.is_empty() {
                    return Err(Error::new(&b.file, b.no, "a milestone needs an `if`"));
                }
                if m.scope == Scope::World {
                    self.world_effects(&m.effects)?;
                }
                m.info = std::mem::take(&mut self.info);
                self.g.milestones.push(m);
            }
            Thing::Pres(_) => {
                let mut p = Prestige {
                    info: Info::default(),
                    gains: Vec::new(),
                    need: Vec::new(),
                    keep: Vec::new(),
                    reset: Vec::new(),
                    effects: Vec::new(),
                };
                for l in &b.lines {
                    if self.common(l, &mut show)? {
                        continue;
                    }
                    match l.key.as_str() {
                        "gain" | "out" => p.gains.push(self.amount(l, &general)?),
                        "if" | "need" => p.need.push(self.expr(l, &l.rest, &with_gain)?),
                        "keep" => p.keep.extend(self.things(l)?),
                        "reset" => p.reset.extend(self.things(l)?),
                        "do" => p.effects.push(self.effect(l, &with_gain)?),
                        "boost" => self.boost(l, false, &general)?,
                        "discount" => self.boost(l, true, &general)?,
                        _ => return Err(self.unknown(l)),
                    }
                }
                if p.gains.is_empty() {
                    return Err(Error::new(&b.file, b.no, "a prestige needs a `gain`"));
                }
                if p.gains.iter().any(|a| self.g.resources[a.res].scope == Scope::World) {
                    return Err(Error::new(&b.file, b.no, "a prestige gives player resources"));
                }
                p.info = std::mem::take(&mut self.info);
                self.g.prestiges.push(p);
            }
        }
        Ok(show)
    }

    fn world_effects(&self, effects: &[Effect]) -> Result<(), Error> {
        for e in effects {
            let ok = match e.target {
                Thing::Res(r) => self.g.resources[r].scope == Scope::World,
                _ => false,
            };
            if !ok {
                return Err(Error::new(
                    &self.b.file,
                    self.b.no,
                    "a world thing's `do` changes only world resources",
                ));
            }
        }
        Ok(())
    }
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}
