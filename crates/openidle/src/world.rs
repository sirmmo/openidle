//! A world: the players of one game and everything they have, on one clock.
//!
//! A single-player game is a world with one player. A server keeps a world with many, where
//! `world` resources, upgrades and milestones are shared by all of them.
//!
//! The world changes in two ways only: [`World::step`] runs one tick, [`World::apply`] carries
//! out one player's command. Both are deterministic: the same world, ticks and commands give the
//! same world, bit for bit, natively and in WebAssembly. That is what lets a server and every
//! browser connected to it run the same world in step.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::expr::{Env, Expr, Ref, Thing};
use crate::game::{Amount, Effect, EffectOp, Game, Scope};
use crate::num::Num;

/// What a player asks for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Command {
    /// Buy a generator (`n` of them, or as many as can be paid for with `max`) or an upgrade.
    Buy {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        n: Option<u32>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        max: bool,
    },
    /// Use an action.
    Act { id: String },
    /// Take a prestige.
    Prestige { id: String },
    /// Give some of a `giftable` resource to another player.
    Gift { to: String, res: String, amount: Num },
    /// Enter the world (or change name).
    Join { name: String },
}

type MakeThing = fn(usize) -> Thing;

/// Most generators one command buys.
pub const MAX_BULK: u64 = 10_000;

#[derive(Clone, Debug)]
struct Player {
    id: String,
    name: String,
    joined: u64,
    run_start: u64,
    amount: Vec<Num>,
    total: Vec<Num>,
    ever: Vec<Num>,
    contrib: Vec<Num>,
    rate: Vec<Num>,
    owned: Vec<u64>,
    upg: Vec<bool>,
    mile: Vec<bool>,
    seen: Vec<bool>,
    prestiged: Vec<u64>,
    ready: Vec<u64>,
    until: Vec<u64>,
}

#[derive(Clone, Debug)]
struct State {
    tick: u64,
    rng: Cell<u64>,
    amount: Vec<Num>,
    total: Vec<Num>,
    ever: Vec<Num>,
    rate: Vec<Num>,
    upg: Vec<bool>,
    mile: Vec<bool>,
    players: Vec<Player>,
}

pub struct World {
    pub game: Arc<Game>,
    s: State,
    events: Vec<Value>,
}

/// What expressions read, for one player (or for the world).
struct Ctx<'a> {
    g: &'a Game,
    s: &'a State,
    p: Option<usize>,
    owned: u64,
    gain: Num,
}

impl<'a> Ctx<'a> {
    fn new(g: &'a Game, s: &'a State, p: Option<usize>) -> Ctx<'a> {
        Ctx {
            g,
            s,
            p,
            owned: 0,
            gain: Num::ZERO,
        }
    }

    fn player(&self) -> Option<&Player> {
        self.p.map(|i| &self.s.players[i])
    }

    fn of(&self, p: Option<&Player>, t: Thing) -> Num {
        let flag = |b: bool| if b { Num::ONE } else { Num::ZERO };
        match t {
            Thing::Res(r) => {
                if self.g.resources[r].scope == Scope::World {
                    self.s.amount[r]
                } else {
                    p.map_or(Num::ZERO, |p| p.amount[r])
                }
            }
            Thing::Gen(i) => p.map_or(Num::ZERO, |p| Num::from_u64(p.owned[i])),
            Thing::Upg(i) => {
                if self.g.upgrades[i].scope == Scope::World {
                    flag(self.s.upg[i])
                } else {
                    flag(p.is_some_and(|p| p.upg[i]))
                }
            }
            Thing::Mile(i) => {
                if self.g.milestones[i].scope == Scope::World {
                    flag(self.s.mile[i])
                } else {
                    flag(p.is_some_and(|p| p.mile[i]))
                }
            }
            Thing::Act(i) => flag(p.is_some_and(|p| p.until[i] > self.s.tick)),
            Thing::Pres(i) => p.map_or(Num::ZERO, |p| Num::from_u64(p.prestiged[i])),
            Thing::Rule(_) => Num::ZERO,
        }
    }

    fn per(&self, r: usize, pick: impl Fn(&Player) -> Num, world: impl Fn(&State) -> Num) -> Num {
        if self.g.resources[r].scope == Scope::World {
            world(self.s)
        } else {
            self.player().map_or(Num::ZERO, pick)
        }
    }
}

impl Env for Ctx<'_> {
    fn get(&self, r: Ref) -> Num {
        match r {
            Ref::Thing(t) => self.of(self.player(), t),
            Ref::Total(r) => self.per(r, |p| p.total[r], |s| s.total[r]),
            Ref::Ever(r) => self.per(r, |p| p.ever[r], |s| s.ever[r]),
            Ref::Rate(r) => self.per(r, |p| p.rate[r], |s| s.rate[r]),
            Ref::Contrib(r) => self.player().map_or(Num::ZERO, |p| p.contrib[r]),
            Ref::Cap(r) => self.g.resources[r].cap.as_ref().map_or(Num::ZERO, |c| c.eval(self)),
            Ref::Sum(t) => self.s.players.iter().fold(Num::ZERO, |a, p| a + self.of(Some(p), t)),
            Ref::Best(t) => self
                .s
                .players
                .iter()
                .map(|p| self.of(Some(p), t))
                .reduce(Num::max)
                .unwrap_or(Num::ZERO),
            Ref::Owned => Num::from_u64(self.owned),
            Ref::Players => Num::from_u64(self.s.players.len() as u64),
            Ref::Time => {
                let since = self.player().map_or(0, |p| p.joined);
                Num::from_u64(self.s.tick - since) * self.g.dt
            }
            Ref::RunTime => {
                let since = self.player().map_or(0, |p| p.run_start);
                Num::from_u64(self.s.tick - since) * self.g.dt
            }
            Ref::Gain => self.gain,
        }
    }

    fn random(&self) -> f64 {
        // splitmix64
        let x = self.s.rng.get().wrapping_add(0x9E37_79B9_7F4A_7C15);
        self.s.rng.set(x);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn all(conds: &[Expr], env: &dyn Env) -> bool {
    conds.iter().all(|c| c.test(env))
}

impl World {
    /// A new world, with no players yet.
    pub fn new(game: Arc<Game>, seed: u64) -> World {
        let g = &game;
        let n = g.resources.len();
        let mut s = State {
            tick: 0,
            rng: Cell::new(seed),
            amount: vec![Num::ZERO; n],
            total: vec![Num::ZERO; n],
            ever: vec![Num::ZERO; n],
            rate: vec![Num::ZERO; n],
            upg: vec![false; g.upgrades.len()],
            mile: vec![false; g.milestones.len()],
            players: Vec::new(),
        };
        let starts: Vec<Num> = {
            let ctx = Ctx::new(g, &s, None);
            g.resources.iter().map(|r| r.start.eval(&ctx)).collect()
        };
        for (r, v) in starts.into_iter().enumerate() {
            if g.resources[r].scope == Scope::World {
                s.amount[r] = v;
            }
        }
        World {
            game,
            s,
            events: Vec::new(),
        }
    }

    pub fn tick(&self) -> u64 {
        self.s.tick
    }

    /// Seconds since the world began.
    pub fn seconds(&self) -> f64 {
        self.s.tick as f64 * self.game.tick
    }

    pub fn players(&self) -> impl Iterator<Item = (&str, &str)> {
        self.s.players.iter().map(|p| (p.id.as_str(), p.name.as_str()))
    }

    pub fn has_player(&self, id: &str) -> bool {
        self.index(id).is_some()
    }

    fn index(&self, id: &str) -> Option<usize> {
        self.s.players.iter().position(|p| p.id == id)
    }

    /// What happened since the last call: joins, unlocks, milestones, upgrades, prestiges, gifts.
    pub fn take_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.events)
    }

    fn event(&mut self, p: Option<usize>, kind: &str, id: &str, extra: Value) {
        let mut e = json!({ "tick": self.s.tick, "kind": kind, "id": id });
        if let Some(p) = p {
            e["player"] = json!(self.s.players[p].id);
        }
        if let Value::Object(m) = extra {
            for (k, v) in m {
                e[k] = v;
            }
        }
        self.events.push(e);
    }

    /// A resource's amount: the player's, or the world's for a world resource.
    pub fn amount(&self, player: Option<&str>, res: &str) -> Option<Num> {
        let r = match self.game.thing(res)? {
            Thing::Res(r) => r,
            _ => return None,
        };
        if self.game.resources[r].scope == Scope::World {
            return Some(self.s.amount[r]);
        }
        Some(self.s.players[self.index(player?)?].amount[r])
    }

    /// Credits a resource from outside the game, as if a rule had made it: income from another
    /// engine that hosts this one (an inn's takings, the guests it served). It counts in `total`
    /// and `ever` and stops at the cap, like production; a negative amount takes away, down to
    /// zero. `player` is `None` for a world resource.
    pub fn credit(&mut self, player: Option<&str>, res: &str, x: Num) -> Result<(), String> {
        let r = match self.game.thing(res) {
            Some(Thing::Res(r)) => r,
            _ => return Err(format!("there is no resource {res:?}")),
        };
        let p = match player {
            Some(id) => Some(self.index(id).ok_or("no such player")?),
            None if self.game.resources[r].scope == Scope::World => None,
            None => return Err(format!("{res} belongs to a player")),
        };
        let x = if x.is_negative() { x.max(-self.have(p, r)) } else { x };
        let cap = self.cap(p, r);
        self.give(p, r, x, cap, false);
        Ok(())
    }

    /// The value of any expression for a player (or the world): `"Miner"`, `"total(Gold)"`.
    pub fn eval(&self, player: Option<&str>, text: &str) -> Result<Num, String> {
        let p = match player {
            Some(id) => Some(self.index(id).ok_or("no such player")?),
            None => None,
        };
        let mut e = Expr::parse(text)?;
        let names = NamesOf(&self.game);
        e.resolve(&names, &|w| match w {
            "players" => Some(Ref::Players),
            "time" => Some(Ref::Time),
            "runtime" => Some(Ref::RunTime),
            _ => None,
        })?;
        Ok(e.eval(&Ctx::new(&self.game, &self.s, p)))
    }

    // ---------------------------------------------------------------- the clock

    /// Runs `n` ticks.
    pub fn run(&mut self, n: u64) {
        for _ in 0..n {
            self.step();
        }
    }

    /// Runs one tick: world rules, then every player's rules (starting with a different player
    /// each tick, so that nobody always comes first to a shared resource), then milestones,
    /// `auto` upgrades and what players see.
    pub fn step(&mut self) {
        let game = self.game.clone();
        let g = &*game;
        for r in self.s.rate.iter_mut() {
            *r = Num::ZERO;
        }
        for p in self.s.players.iter_mut() {
            for r in p.rate.iter_mut() {
                *r = Num::ZERO;
            }
        }
        let world_boosts = self.boost_values(None);
        for ri in 0..g.rules.len() {
            if g.rules[ri].scope == Scope::World {
                self.run_rule(None, ri, 1, &world_boosts);
            }
        }
        let n = self.s.players.len();
        for k in 0..n {
            let p = (self.s.tick as usize + k) % n;
            let boosts = self.boost_values(Some(p));
            for ri in 0..g.rules.len() {
                let rule = &g.rules[ri];
                if rule.scope != Scope::Player {
                    continue;
                }
                let count = if rule.applies.is_empty() {
                    1
                } else {
                    rule.applies.iter().map(|&i| self.s.players[p].owned[i]).sum()
                };
                if count > 0 {
                    self.run_rule(Some(p), ri, count, &boosts);
                }
            }
        }
        for r in self.s.rate.iter_mut() {
            *r = *r / g.dt;
        }
        for p in self.s.players.iter_mut() {
            for r in p.rate.iter_mut() {
                *r = *r / g.dt;
            }
        }
        self.s.tick += 1;
        self.settle_world();
        for p in 0..n {
            self.settle(p);
        }
    }

    /// GlassBox's unit rule: with part of its inputs, a rule takes that part and gives that part.
    fn run_rule(&mut self, p: Option<usize>, ri: usize, count: u64, boosts: &[Num]) {
        let g = self.game.clone();
        let rule = &g.rules[ri];
        let dt = g.dt * Num::from_u64(count);
        let (ins, outs, caps) = {
            let ctx = Ctx::new(&g, &self.s, p);
            if !all(&rule.tests, &ctx) {
                return;
            }
            let ins: Vec<Num> = rule.ins.iter().map(|a| a.expr.eval(&ctx) * dt).collect();
            let outs: Vec<Num> = rule
                .outs
                .iter()
                .zip(&rule.out_boosts)
                .map(|(a, bs)| bs.iter().fold(a.expr.eval(&ctx) * dt, |x, &b| x * boosts[b]))
                .collect();
            let caps: Vec<Option<Num>> = rule
                .outs
                .iter()
                .map(|a| g.resources[a.res].cap.as_ref().map(|c| c.eval(&ctx)))
                .collect();
            (ins, outs, caps)
        };
        let mut f = Num::ONE;
        for (a, need) in rule.ins.iter().zip(&ins) {
            if need.is_positive() {
                let have = self.have(p, a.res);
                if have < *need {
                    f = f.min(have.max(Num::ZERO) / *need);
                }
            }
        }
        // A converter slows down when what it makes has no room left, so nothing is wasted.
        if !rule.ins.is_empty() {
            for ((a, out), cap) in rule.outs.iter().zip(&outs).zip(&caps) {
                if let (Some(cap), true) = (cap, out.is_positive()) {
                    let room = (*cap - self.have(p, a.res)).max(Num::ZERO);
                    f = f.min(room / *out);
                }
            }
        }
        if !f.is_positive() {
            return;
        }
        for (a, need) in rule.ins.iter().zip(&ins) {
            if need.is_positive() {
                self.take(p, a.res, *need * f, true);
            }
        }
        for ((a, out), cap) in rule.outs.iter().zip(&outs).zip(caps) {
            self.give(p, a.res, *out * f, cap, true);
        }
    }

    fn have(&self, p: Option<usize>, r: usize) -> Num {
        if self.game.resources[r].scope == Scope::World {
            self.s.amount[r]
        } else {
            p.map_or(Num::ZERO, |p| self.s.players[p].amount[r])
        }
    }

    fn take(&mut self, p: Option<usize>, r: usize, x: Num, flow: bool) {
        if self.game.resources[r].scope == Scope::World {
            self.s.amount[r] = self.s.amount[r] - x;
            if flow {
                self.s.rate[r] = self.s.rate[r] - x;
            }
        } else if let Some(p) = p {
            let pl = &mut self.s.players[p];
            pl.amount[r] = pl.amount[r] - x;
            if flow {
                pl.rate[r] = pl.rate[r] - x;
            }
        }
    }

    /// Adds `x` (made, so it counts in totals), up to the cap.
    fn give(&mut self, p: Option<usize>, r: usize, x: Num, cap: Option<Num>, flow: bool) {
        if !x.is_positive() {
            if x.is_negative() {
                self.take(p, r, -x, flow);
            }
            return;
        }
        let have = self.have(p, r);
        let mut added = x;
        if let Some(cap) = cap {
            added = added.min((cap - have).max(Num::ZERO));
        }
        if self.game.resources[r].scope == Scope::World {
            self.s.amount[r] = have + added;
            self.s.total[r] = self.s.total[r] + added;
            self.s.ever[r] = self.s.ever[r] + added;
            if flow {
                self.s.rate[r] = self.s.rate[r] + added;
            }
            if let Some(p) = p {
                let pl = &mut self.s.players[p];
                pl.contrib[r] = pl.contrib[r] + added;
            }
        } else if let Some(p) = p {
            let pl = &mut self.s.players[p];
            pl.amount[r] = have + added;
            pl.total[r] = pl.total[r] + added;
            pl.ever[r] = pl.ever[r] + added;
            if flow {
                pl.rate[r] = pl.rate[r] + added;
            }
        }
    }

    fn cap(&self, p: Option<usize>, r: usize) -> Option<Num> {
        let c = self.game.resources[r].cap.as_ref()?;
        Some(c.eval(&Ctx::new(&self.game, &self.s, p)))
    }

    /// The value of every boost for a player (1 for those not in force).
    fn boost_values(&self, p: Option<usize>) -> Vec<Num> {
        let g = &*self.game;
        let ctx = Ctx::new(g, &self.s, p);
        g.boosts
            .iter()
            .map(|b| {
                if p.is_none() && g.scope_of(b.source) == Scope::Player {
                    return Num::ONE;
                }
                let on = match b.source {
                    Thing::Res(_) | Thing::Gen(_) => true,
                    t @ (Thing::Upg(_) | Thing::Mile(_) | Thing::Act(_)) => !ctx.of(ctx.player(), t).is_zero(),
                    Thing::Pres(i) => ctx.player().is_some_and(|pl| pl.prestiged[i] > 0),
                    Thing::Rule(_) => false,
                };
                if on {
                    b.value.eval(&ctx)
                } else {
                    Num::ONE
                }
            })
            .collect()
    }

    fn apply_effects(&mut self, p: Option<usize>, effects: &[Effect], gain: Num) {
        for e in effects {
            let (v, cap) = {
                let mut ctx = Ctx::new(&self.game, &self.s, p);
                ctx.gain = gain;
                let v = e.value.eval(&ctx);
                let cap = match e.target {
                    Thing::Res(r) => self.game.resources[r].cap.as_ref().map(|c| c.eval(&ctx)),
                    _ => None,
                };
                (v, cap)
            };
            match e.target {
                Thing::Res(r) => {
                    let have = self.have(p, r);
                    let delta = match e.op {
                        EffectOp::Add => v,
                        EffectOp::Set => v - have,
                        EffectOp::Mul => have * v - have,
                    };
                    self.give(p, r, delta, cap, false);
                }
                Thing::Gen(i) => {
                    if let Some(p) = p {
                        let o = &mut self.s.players[p].owned[i];
                        let x = Num::from_u64(*o);
                        *o = match e.op {
                            EffectOp::Add => x + v,
                            EffectOp::Set => v,
                            EffectOp::Mul => x * v,
                        }
                        .to_u64();
                    }
                }
                _ => {}
            }
        }
    }

    /// Milestones and `auto` upgrades of the world.
    fn settle_world(&mut self) {
        let g = self.game.clone();
        for i in 0..g.milestones.len() {
            let m = &g.milestones[i];
            if m.scope == Scope::World && !self.s.mile[i] && all(&m.conds, &Ctx::new(&g, &self.s, None)) {
                self.s.mile[i] = true;
                self.apply_effects(None, &m.effects, Num::ZERO);
                self.event(None, "milestone", &m.info.id, json!({ "label": m.info.label }));
            }
        }
        for i in 0..g.upgrades.len() {
            let u = &g.upgrades[i];
            if u.scope == Scope::World && u.auto && !self.s.upg[i] && self.upgrade_ready(None, i).is_ok() {
                self.pay_upgrade(None, i);
            }
        }
    }

    /// A player's milestones, `auto` upgrades and what they see.
    fn settle(&mut self, p: usize) {
        let g = self.game.clone();
        for i in 0..g.milestones.len() {
            let m = &g.milestones[i];
            if m.scope == Scope::Player && !self.s.players[p].mile[i] && all(&m.conds, &Ctx::new(&g, &self.s, Some(p)))
            {
                self.s.players[p].mile[i] = true;
                self.apply_effects(Some(p), &m.effects, Num::ZERO);
                self.event(Some(p), "milestone", &m.info.id, json!({ "label": m.info.label }));
            }
        }
        for i in 0..g.upgrades.len() {
            let u = &g.upgrades[i];
            if u.scope == Scope::Player && u.auto && !self.s.players[p].upg[i] && self.upgrade_ready(Some(p), i).is_ok()
            {
                self.pay_upgrade(Some(p), i);
            }
        }
        let newly: Vec<usize> = {
            let ctx = Ctx::new(&g, &self.s, Some(p));
            let seen = &self.s.players[p].seen;
            (0..g.items())
                .filter(|&i| !seen[i] && g.shows[i].as_ref().is_some_and(|e| e.test(&ctx)))
                .collect()
        };
        for i in newly {
            self.s.players[p].seen[i] = true;
            if let Some(t) = self.item_thing(i) {
                let id = g.id_of(t).to_string();
                let label = g.label_of(t).to_string();
                self.event(Some(p), "unlock", &id, json!({ "label": label }));
            }
        }
    }

    fn item_thing(&self, mut i: usize) -> Option<Thing> {
        let g = &self.game;
        let groups: [(usize, MakeThing); 6] = [
            (g.resources.len(), Thing::Res),
            (g.generators.len(), Thing::Gen),
            (g.upgrades.len(), Thing::Upg),
            (g.actions.len(), Thing::Act),
            (g.milestones.len(), Thing::Mile),
            (g.prestiges.len(), Thing::Pres),
        ];
        for (n, f) in groups {
            if i < n {
                return Some(f(i));
            }
            i -= n;
        }
        None
    }

    fn visible(&self, p: usize, t: Thing) -> bool {
        if let Thing::Res(r) = t {
            if self.game.resources[r].hidden {
                return false;
            }
        }
        self.game.item(t).is_some_and(|i| self.s.players[p].seen[i])
    }

    // ---------------------------------------------------------------- commands

    /// Carries out a player's command, or says why not (and changes nothing).
    pub fn apply(&mut self, player: &str, cmd: &Command) -> Result<(), String> {
        if let Command::Join { name } = cmd {
            self.join(player, name);
            return Ok(());
        }
        let p = self.index(player).ok_or("you are not in this world")?;
        let g = self.game.clone();
        let thing = |id: &str| g.thing(id).ok_or_else(|| format!("no `{id}` in this game"));
        match cmd {
            Command::Buy { id, n, max } => match thing(id)? {
                Thing::Gen(i) => {
                    let limit = if *max {
                        MAX_BULK
                    } else {
                        u64::from(n.unwrap_or(1)).clamp(1, MAX_BULK)
                    };
                    self.buy_generator(p, i, limit)
                }
                Thing::Upg(i) => {
                    if !self.visible(p, Thing::Upg(i)) {
                        return Err(format!("{} is not available yet", g.upgrades[i].info.label));
                    }
                    let scope = if g.upgrades[i].scope == Scope::World {
                        None
                    } else {
                        Some(p)
                    };
                    self.upgrade_ready_by(scope, Some(p), i)?;
                    self.pay_upgrade_by(scope, Some(p), i);
                    Ok(())
                }
                _ => Err(format!("`{id}` cannot be bought")),
            },
            Command::Act { id } => match thing(id)? {
                Thing::Act(i) => self.act(p, i),
                _ => Err(format!("`{id}` is not an action")),
            },
            Command::Prestige { id } => match thing(id)? {
                Thing::Pres(i) => self.prestige(p, i),
                _ => Err(format!("`{id}` is not a prestige")),
            },
            Command::Gift { to, res, amount } => {
                let r = match thing(res)? {
                    Thing::Res(r) if g.resources[r].giftable => r,
                    _ => return Err(format!("{res} cannot be given")),
                };
                let q = self.index(to).ok_or("no such player")?;
                if q == p {
                    return Err("that is you".into());
                }
                if !amount.is_positive() || *amount > self.s.players[p].amount[r] {
                    return Err(format!("you do not have that much {}", g.resources[r].info.label));
                }
                self.s.players[p].amount[r] = self.s.players[p].amount[r] - *amount;
                self.s.players[q].amount[r] = self.s.players[q].amount[r] + *amount;
                self.event(Some(p), "gift", res, json!({ "to": to, "amount": amount }));
                Ok(())
            }
            Command::Join { .. } => unreachable!(),
        }
    }

    /// Adds a player (or renames one already in the world).
    pub fn join(&mut self, id: &str, name: &str) {
        let name: String = name.chars().take(40).collect();
        if let Some(p) = self.index(id) {
            self.s.players[p].name = name;
            return;
        }
        let pl = self.new_player(id, &name);
        self.s.players.push(pl);
        let p = self.s.players.len() - 1;
        self.event(Some(p), "join", id, json!({ "name": name }));
        self.settle(p);
    }

    fn new_player(&self, id: &str, name: &str) -> Player {
        let g = &*self.game;
        let n = g.resources.len();
        let mut pl = Player {
            id: id.to_string(),
            name: name.to_string(),
            joined: self.s.tick,
            run_start: self.s.tick,
            amount: vec![Num::ZERO; n],
            total: vec![Num::ZERO; n],
            ever: vec![Num::ZERO; n],
            contrib: vec![Num::ZERO; n],
            rate: vec![Num::ZERO; n],
            owned: g.generators.iter().map(|x| x.start).collect(),
            upg: vec![false; g.upgrades.len()],
            mile: vec![false; g.milestones.len()],
            seen: g.shows.iter().map(|s| s.is_none()).collect(),
            prestiged: vec![0; g.prestiges.len()],
            ready: vec![0; g.actions.len()],
            until: vec![0; g.actions.len()],
        };
        let ctx = Ctx::new(g, &self.s, None);
        for (r, res) in g.resources.iter().enumerate() {
            if res.scope == Scope::Player {
                pl.amount[r] = res.start.eval(&ctx);
            }
        }
        pl
    }

    /// Prices of the next `limit` generators (as many as can be paid for): (how many, total).
    fn plan(&self, p: usize, i: usize, limit: u64) -> (u64, Vec<(usize, Num)>, Option<String>) {
        let g = &*self.game;
        let gen = &g.generators[i];
        let pl = &self.s.players[p];
        let mut ctx = Ctx::new(g, &self.s, Some(p));
        let max = gen.max.as_ref().map(|m| m.eval(&ctx).to_u64());
        let boosts = if gen.discounts.is_empty() {
            Vec::new()
        } else {
            self.boost_values(Some(p))
        };
        let discount = gen.discounts.iter().fold(Num::ONE, |x, &b| x * boosts[b]);
        let mut spent: Vec<(usize, Num)> = gen.costs.iter().map(|c| (c.res, Num::ZERO)).collect();
        let mut n = 0;
        let mut why = None;
        while n < limit {
            let owned = pl.owned[i] + n;
            if max.is_some_and(|m| owned >= m) {
                why = Some(format!("no more {} can be had", gen.info.label));
                break;
            }
            ctx.owned = owned;
            let price: Vec<Num> = gen.costs.iter().map(|c| c.expr.eval(&ctx) * discount).collect();
            let short = gen
                .costs
                .iter()
                .zip(&price)
                .zip(&spent)
                .find(|((c, x), (_, s))| self.have(Some(p), c.res) < *s + **x);
            if let Some(((c, _), _)) = short {
                why = Some(format!("not enough {}", g.resources[c.res].info.label));
                break;
            }
            for (s, x) in spent.iter_mut().zip(price) {
                s.1 = s.1 + x;
            }
            n += 1;
        }
        (n, spent, why)
    }

    fn buy_generator(&mut self, p: usize, i: usize, limit: u64) -> Result<(), String> {
        if !self.visible(p, Thing::Gen(i)) {
            return Err(format!("{} is not available yet", self.game.generators[i].info.label));
        }
        let (n, spent, why) = self.plan(p, i, limit);
        if n == 0 {
            return Err(why.unwrap_or_else(|| "nothing to buy".into()));
        }
        for (r, x) in spent {
            self.take(Some(p), r, x, false);
        }
        self.s.players[p].owned[i] += n;
        self.settle(p);
        Ok(())
    }

    fn upgrade_ready(&self, p: Option<usize>, i: usize) -> Result<(), String> {
        self.upgrade_ready_by(p, p, i)
    }

    /// Whether upgrade `i` can be bought, for `owner` (none: the world), paid by `payer`.
    fn upgrade_ready_by(&self, owner: Option<usize>, payer: Option<usize>, i: usize) -> Result<(), String> {
        let g = &*self.game;
        let u = &g.upgrades[i];
        let bought = match owner {
            None => self.s.upg[i],
            Some(p) => self.s.players[p].upg[i],
        };
        if bought {
            return Err(format!("{} is already bought", u.info.label));
        }
        let ctx = Ctx::new(g, &self.s, payer);
        if !all(&u.need, &ctx) {
            return Err(format!("{} is not available yet", u.info.label));
        }
        for (c, x) in self.upgrade_costs(payer, i) {
            if self.have(payer, c) < x {
                return Err(format!("not enough {}", g.resources[c].info.label));
            }
        }
        Ok(())
    }

    fn upgrade_costs(&self, payer: Option<usize>, i: usize) -> Vec<(usize, Num)> {
        let g = &*self.game;
        let u = &g.upgrades[i];
        let ctx = Ctx::new(g, &self.s, payer);
        let boosts = if u.discounts.is_empty() {
            Vec::new()
        } else {
            self.boost_values(payer)
        };
        let discount = u.discounts.iter().fold(Num::ONE, |x, &b| x * boosts[b]);
        u.costs
            .iter()
            .map(|c: &Amount| (c.res, c.expr.eval(&ctx) * discount))
            .collect()
    }

    fn pay_upgrade(&mut self, p: Option<usize>, i: usize) {
        self.pay_upgrade_by(p, p, i)
    }

    fn pay_upgrade_by(&mut self, owner: Option<usize>, payer: Option<usize>, i: usize) {
        let g = self.game.clone();
        for (c, x) in self.upgrade_costs(payer, i) {
            self.take(payer, c, x, false);
        }
        match owner {
            None => self.s.upg[i] = true,
            Some(p) => self.s.players[p].upg[i] = true,
        }
        let u = &g.upgrades[i];
        self.apply_effects(owner, &u.effects, Num::ZERO);
        self.event(
            payer,
            "upgrade",
            &u.info.id,
            json!({ "label": u.info.label, "scope": u.scope.name() }),
        );
        if let Some(p) = payer {
            self.settle(p);
        }
    }

    fn act(&mut self, p: usize, i: usize) -> Result<(), String> {
        let g = self.game.clone();
        let a = &g.actions[i];
        if !self.visible(p, Thing::Act(i)) {
            return Err(format!("{} is not available yet", a.info.label));
        }
        if self.s.players[p].ready[i] > self.s.tick {
            return Err(format!("{} is not ready", a.info.label));
        }
        let boosts = self.boost_values(Some(p));
        let (costs, gains, caps) = {
            let ctx = Ctx::new(&g, &self.s, Some(p));
            if !all(&a.need, &ctx) {
                return Err(format!("{} cannot be used now", a.info.label));
            }
            let costs: Vec<Num> = a.costs.iter().map(|c| c.expr.eval(&ctx)).collect();
            let gains: Vec<Num> = a
                .gains
                .iter()
                .zip(&a.gain_boosts)
                .map(|(c, bs)| bs.iter().fold(c.expr.eval(&ctx), |x, &b| x * boosts[b]))
                .collect();
            let caps: Vec<Option<Num>> = a
                .gains
                .iter()
                .map(|c| g.resources[c.res].cap.as_ref().map(|x| x.eval(&ctx)))
                .collect();
            (costs, gains, caps)
        };
        for (c, x) in a.costs.iter().zip(&costs) {
            if self.have(Some(p), c.res) < *x {
                return Err(format!("not enough {}", g.resources[c.res].info.label));
            }
        }
        for (c, x) in a.costs.iter().zip(costs) {
            self.take(Some(p), c.res, x, false);
        }
        for ((c, x), cap) in a.gains.iter().zip(gains).zip(caps) {
            self.give(Some(p), c.res, x, cap, false);
        }
        let t = self.s.tick;
        let pl = &mut self.s.players[p];
        pl.ready[i] = t + a.cooldown;
        if a.lasts > 0 {
            pl.until[i] = t + a.lasts;
        }
        self.apply_effects(Some(p), &a.effects, Num::ZERO);
        self.settle(p);
        Ok(())
    }

    fn prestige_gains(&self, p: usize, i: usize) -> Vec<(usize, Num)> {
        let g = &*self.game;
        let ctx = Ctx::new(g, &self.s, Some(p));
        g.prestiges[i]
            .gains
            .iter()
            .map(|a| (a.res, a.expr.eval(&ctx).max(Num::ZERO)))
            .collect()
    }

    fn prestige_ready(&self, p: usize, i: usize, gains: &[(usize, Num)]) -> bool {
        let g = &*self.game;
        let pr = &g.prestiges[i];
        let mut ctx = Ctx::new(g, &self.s, Some(p));
        ctx.gain = gains[0].1;
        if pr.need.is_empty() {
            gains[0].1.is_positive()
        } else {
            all(&pr.need, &ctx)
        }
    }

    fn prestige(&mut self, p: usize, i: usize) -> Result<(), String> {
        let g = self.game.clone();
        let pr = &g.prestiges[i];
        if !self.visible(p, Thing::Pres(i)) {
            return Err(format!("{} is not available yet", pr.info.label));
        }
        let gains = self.prestige_gains(p, i);
        if !self.prestige_ready(p, i, &gains) {
            return Err(format!("{} would give nothing yet", pr.info.label));
        }
        let gained: Vec<usize> = gains.iter().map(|x| x.0).collect();
        let starts: Vec<Num> = {
            let ctx = Ctx::new(&g, &self.s, None);
            g.resources.iter().map(|r| r.start.eval(&ctx)).collect()
        };
        let keep = |t: Thing| pr.keep.contains(&t);
        let reset = |t: Thing| pr.reset.contains(&t);
        let before: Vec<u64> = self.s.players[p].owned.clone();
        let tick = self.s.tick;
        let pl = &mut self.s.players[p];
        for (r, res) in g.resources.iter().enumerate() {
            let t = Thing::Res(r);
            if res.scope == Scope::Player && ((!keep(t) && !gained.contains(&r)) || reset(t)) {
                pl.amount[r] = starts[r];
                pl.total[r] = Num::ZERO;
            }
        }
        for (k, gen) in g.generators.iter().enumerate() {
            if !keep(Thing::Gen(k)) || reset(Thing::Gen(k)) {
                pl.owned[k] = gen.start;
            }
        }
        for (k, u) in g.upgrades.iter().enumerate() {
            if u.scope == Scope::Player && (!keep(Thing::Upg(k)) || reset(Thing::Upg(k))) {
                pl.upg[k] = false;
            }
        }
        for k in 0..g.milestones.len() {
            if reset(Thing::Mile(k)) {
                pl.mile[k] = false;
            }
        }
        for k in 0..g.actions.len() {
            pl.ready[k] = 0;
            pl.until[k] = 0;
        }
        pl.run_start = tick;
        pl.prestiged[i] += 1;
        let caps: Vec<Option<Num>> = gains.iter().map(|(r, _)| self.cap(Some(p), *r)).collect();
        for ((r, x), cap) in gains.iter().zip(caps) {
            self.give(Some(p), *r, *x, cap, false);
        }
        // The generators that follow, each up to its cap.
        let mut carried = Vec::new();
        for (gens, cap) in &pr.carry {
            for &k in gens {
                let mut ctx = Ctx::new(&g, &self.s, Some(p));
                ctx.owned = before[k];
                ctx.gain = gains[0].1;
                let cap = cap.eval(&ctx).to_f64().floor().max(0.0) as u64;
                carried.push((k, before[k].min(cap).max(g.generators[k].start)));
            }
        }
        for (k, n) in carried {
            self.s.players[p].owned[k] = n;
        }
        self.apply_effects(Some(p), &pr.effects, gains[0].1);
        let got: BTreeMap<&str, Num> = gains
            .iter()
            .map(|(r, x)| (g.resources[*r].info.id.as_str(), *x))
            .collect();
        self.event(
            Some(p),
            "prestige",
            &pr.info.id,
            json!({ "label": pr.info.label, "gain": got }),
        );
        self.settle(p);
        Ok(())
    }

    // ---------------------------------------------------------------- what a page shows

    /// Everything a page needs to draw the game for one player. Amounts are strings in the
    /// exact form (`"1.5e300"`); `visible` says what the player has unlocked.
    pub fn view(&self, player: &str) -> Result<Value, String> {
        let p = self.index(player).ok_or("you are not in this world")?;
        let g = &*self.game;
        let ctx = Ctx::new(g, &self.s, Some(p));
        let pl = &self.s.players[p];
        let boosts = self.boost_values(Some(p));
        let costs = |list: &[(usize, Num)]| -> Vec<Value> {
            list.iter()
                .map(|(r, x)| json!({ "res": g.resources[*r].info.id, "amount": x }))
                .collect()
        };
        let info = |i: &crate::game::Info, t: Thing| json!({ "id": i.id, "label": i.label, "text": i.text, "view": i.view, "visible": self.visible(p, t) });
        let merge = |mut a: Value, b: Value| {
            if let (Value::Object(a), Value::Object(b)) = (&mut a, b) {
                a.extend(b);
            }
            a
        };
        let resources: Vec<Value> = g
            .resources
            .iter()
            .enumerate()
            .map(|(r, res)| {
                let world = res.scope == Scope::World;
                merge(
                    info(&res.info, Thing::Res(r)),
                    json!({
                        "scope": res.scope.name(),
                        "amount": self.have(Some(p), r),
                        "rate": if world { self.s.rate[r] } else { pl.rate[r] },
                        "cap": res.cap.as_ref().map(|c| c.eval(&ctx)),
                        "total": if world { self.s.total[r] } else { pl.total[r] },
                        "ever": if world { self.s.ever[r] } else { pl.ever[r] },
                        "contrib": pl.contrib[r],
                        "giftable": res.giftable,
                    }),
                )
            })
            .collect();
        let generators: Vec<Value> = g
            .generators
            .iter()
            .enumerate()
            .map(|(i, gen)| {
                let (one, next, _) = self.plan(p, i, 1);
                let price = if one == 1 { next } else { self.price(p, i) };
                let (afford, _, _) = if one == 1 {
                    self.plan(p, i, 1000)
                } else {
                    (0, Vec::new(), None)
                };
                let makes: BTreeMap<&str, Num> = g
                    .rules
                    .iter()
                    .filter(|r| r.applies.contains(&i))
                    .flat_map(|r| {
                        r.outs.iter().zip(&r.out_boosts).map(|(o, bs)| {
                            (
                                g.resources[o.res].info.id.as_str(),
                                bs.iter().fold(o.expr.eval(&ctx), |x, &b| x * boosts[b]),
                            )
                        })
                    })
                    .fold(BTreeMap::new(), |mut m, (k, v)| {
                        let e = m.entry(k).or_insert(Num::ZERO);
                        *e = *e + v;
                        m
                    });
                merge(
                    info(&gen.info, Thing::Gen(i)),
                    json!({
                        "owned": pl.owned[i],
                        "max": gen.max.as_ref().map(|m| m.eval(&ctx).to_u64()),
                        "cost": costs(&price),
                        "can": one == 1 && self.visible(p, Thing::Gen(i)),
                        "afford": afford,
                        "makes": makes,
                    }),
                )
            })
            .collect();
        let upgrades: Vec<Value> = g
            .upgrades
            .iter()
            .enumerate()
            .map(|(i, u)| {
                let world = u.scope == Scope::World;
                let owner = if world { None } else { Some(p) };
                let bought = if world { self.s.upg[i] } else { pl.upg[i] };
                merge(
                    info(&u.info, Thing::Upg(i)),
                    json!({
                        "scope": u.scope.name(),
                        "bought": bought,
                        "cost": costs(&self.upgrade_costs(Some(p), i)),
                        "can": self.visible(p, Thing::Upg(i)) && self.upgrade_ready_by(owner, Some(p), i).is_ok(),
                        "auto": u.auto,
                    }),
                )
            })
            .collect();
        let actions: Vec<Value> = g
            .actions
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let c: Vec<(usize, Num)> = a.costs.iter().map(|c| (c.res, c.expr.eval(&ctx))).collect();
                let gain: Vec<(usize, Num)> = a
                    .gains
                    .iter()
                    .zip(&a.gain_boosts)
                    .map(|(c, bs)| (c.res, bs.iter().fold(c.expr.eval(&ctx), |x, &b| x * boosts[b])))
                    .collect();
                let wait = pl.ready[i].saturating_sub(self.s.tick);
                let on = pl.until[i].saturating_sub(self.s.tick);
                let affordable = c.iter().all(|(r, x)| self.have(Some(p), *r) >= *x);
                merge(
                    info(&a.info, Thing::Act(i)),
                    json!({
                        "cost": costs(&c),
                        "gain": costs(&gain),
                        "ready_in": wait as f64 * g.tick,
                        "cooldown": a.cooldown as f64 * g.tick,
                        "active_for": on as f64 * g.tick,
                        "can": self.visible(p, Thing::Act(i)) && wait == 0 && affordable && all(&a.need, &ctx),
                    }),
                )
            })
            .collect();
        let milestones: Vec<Value> = g
            .milestones
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let done = if m.scope == Scope::World {
                    self.s.mile[i]
                } else {
                    pl.mile[i]
                };
                merge(
                    info(&m.info, Thing::Mile(i)),
                    json!({ "scope": m.scope.name(), "done": done }),
                )
            })
            .collect();
        let prestiges: Vec<Value> = g
            .prestiges
            .iter()
            .enumerate()
            .map(|(i, pr)| {
                let gains = self.prestige_gains(p, i);
                merge(
                    info(&pr.info, Thing::Pres(i)),
                    json!({
                        "gain": costs(&gains),
                        "can": self.visible(p, Thing::Pres(i)) && self.prestige_ready(p, i, &gains),
                        "count": pl.prestiged[i],
                    }),
                )
            })
            .collect();
        let players: Vec<Value> = self
            .s
            .players
            .iter()
            .enumerate()
            .map(|(k, q)| {
                let score = g.score.as_ref().map(|s| s.eval(&Ctx::new(g, &self.s, Some(k))));
                json!({ "id": q.id, "name": q.name, "score": score })
            })
            .collect();
        Ok(json!({
            "game": g.name,
            "tick": self.s.tick,
            "seconds": self.seconds(),
            "tick_seconds": g.tick,
            "player": { "id": pl.id, "name": pl.name, "time": ctx.get(Ref::Time), "runtime": ctx.get(Ref::RunTime) },
            "resources": resources,
            "generators": generators,
            "upgrades": upgrades,
            "actions": actions,
            "milestones": milestones,
            "prestiges": prestiges,
            "players": players,
        }))
    }

    /// The price of the next generator, affordable or not.
    fn price(&self, p: usize, i: usize) -> Vec<(usize, Num)> {
        let g = &*self.game;
        let gen = &g.generators[i];
        let mut ctx = Ctx::new(g, &self.s, Some(p));
        ctx.owned = self.s.players[p].owned[i];
        let boosts = if gen.discounts.is_empty() {
            Vec::new()
        } else {
            self.boost_values(Some(p))
        };
        let discount = gen.discounts.iter().fold(Num::ONE, |x, &b| x * boosts[b]);
        gen.costs
            .iter()
            .map(|c| (c.res, c.expr.eval(&ctx) * discount))
            .collect()
    }

    // ---------------------------------------------------------------- saving

    /// The whole world, by ids (so that saves survive changes to the game's files).
    pub fn save(&self) -> Save {
        let g = &*self.game;
        let res = |v: &[Num], scope: Scope| -> BTreeMap<String, Num> {
            g.resources
                .iter()
                .zip(v)
                .filter(|(r, x)| r.scope == scope && !x.is_zero())
                .map(|(r, x)| (r.info.id.clone(), *x))
                .collect()
        };
        let flags = |v: &[bool], ids: &dyn Fn(usize) -> String| -> Vec<String> {
            v.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| ids(i)).collect()
        };
        let counts = |v: &[u64], ids: &dyn Fn(usize) -> String| -> BTreeMap<String, u64> {
            v.iter()
                .enumerate()
                .filter(|(_, n)| **n > 0)
                .map(|(i, n)| (ids(i), *n))
                .collect()
        };
        let upg = |i: usize| g.upgrades[i].info.id.clone();
        let mile = |i: usize| g.milestones[i].info.id.clone();
        let gen = |i: usize| g.generators[i].info.id.clone();
        let pres = |i: usize| g.prestiges[i].info.id.clone();
        let act = |i: usize| g.actions[i].info.id.clone();
        let item = |i: usize| self.item_thing(i).map(|t| g.id_of(t).to_string()).unwrap_or_default();
        Save {
            game: g.name.clone(),
            tick: self.s.tick,
            rng: self.s.rng.get(),
            amount: res(&self.s.amount, Scope::World),
            total: res(&self.s.total, Scope::World),
            ever: res(&self.s.ever, Scope::World),
            upgrades: flags(&self.s.upg, &upg),
            milestones: flags(&self.s.mile, &mile),
            players: self
                .s
                .players
                .iter()
                .map(|p| SavedPlayer {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    joined: p.joined,
                    run_start: p.run_start,
                    amount: res(&p.amount, Scope::Player),
                    total: res(&p.total, Scope::Player),
                    ever: res(&p.ever, Scope::Player),
                    contrib: res(&p.contrib, Scope::World),
                    owned: counts(&p.owned, &gen),
                    upgrades: flags(&p.upg, &upg),
                    milestones: flags(&p.mile, &mile),
                    seen: flags(&p.seen, &item),
                    prestiged: counts(&p.prestiged, &pres),
                    ready: counts(&p.ready, &act),
                    until: counts(&p.until, &act),
                })
                .collect(),
        }
    }

    /// A world from a save, for this game; things the game no longer has are dropped.
    pub fn load(game: Arc<Game>, save: &Save) -> World {
        let mut w = World::new(game.clone(), save.rng);
        let g = &*game;
        w.s.tick = save.tick;
        let fill = |v: &mut [Num], m: &BTreeMap<String, Num>| {
            for (id, x) in m {
                if let Some(Thing::Res(r)) = g.thing(id) {
                    v[r] = *x;
                }
            }
        };
        for (r, res) in g.resources.iter().enumerate() {
            if res.scope == Scope::World {
                w.s.amount[r] = Num::ZERO;
            }
        }
        fill(&mut w.s.amount, &save.amount);
        fill(&mut w.s.total, &save.total);
        fill(&mut w.s.ever, &save.ever);
        let set = |v: &mut [bool], ids: &[String], f: &dyn Fn(Thing) -> Option<usize>| {
            for id in ids {
                if let Some(i) = g.thing(id).and_then(f) {
                    v[i] = true;
                }
            }
        };
        let upg = |t: Thing| if let Thing::Upg(i) = t { Some(i) } else { None };
        let mile = |t: Thing| if let Thing::Mile(i) = t { Some(i) } else { None };
        set(&mut w.s.upg, &save.upgrades, &upg);
        set(&mut w.s.mile, &save.milestones, &mile);
        for sp in &save.players {
            let mut pl = w.new_player(&sp.id, &sp.name);
            pl.joined = sp.joined;
            pl.run_start = sp.run_start;
            for (r, res) in g.resources.iter().enumerate() {
                if res.scope == Scope::Player {
                    pl.amount[r] = Num::ZERO;
                }
            }
            fill(&mut pl.amount, &sp.amount);
            fill(&mut pl.total, &sp.total);
            fill(&mut pl.ever, &sp.ever);
            fill(&mut pl.contrib, &sp.contrib);
            for (k, gen) in g.generators.iter().enumerate() {
                pl.owned[k] = sp.owned.get(&gen.info.id).copied().unwrap_or(0);
            }
            set(&mut pl.upg, &sp.upgrades, &upg);
            set(&mut pl.mile, &sp.milestones, &mile);
            for id in &sp.seen {
                if let Some(i) = g.thing(id).and_then(|t| g.item(t)) {
                    pl.seen[i] = true;
                }
            }
            for (k, pr) in g.prestiges.iter().enumerate() {
                pl.prestiged[k] = sp.prestiged.get(&pr.info.id).copied().unwrap_or(0);
            }
            for (k, a) in g.actions.iter().enumerate() {
                pl.ready[k] = sp.ready.get(&a.info.id).copied().unwrap_or(0);
                pl.until[k] = sp.until.get(&a.info.id).copied().unwrap_or(0);
            }
            w.s.players.push(pl);
        }
        w.s.rng.set(save.rng);
        w.events.clear();
        w
    }

    /// A fingerprint of the whole world: two copies in step have the same one.
    pub fn fingerprint(&self) -> String {
        let text = serde_json::to_string(&self.save()).unwrap_or_default();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in text.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        format!("{h:016x}")
    }
}

struct NamesOf<'a>(&'a Game);

impl crate::expr::Names for NamesOf<'_> {
    fn thing(&self, name: &str) -> Option<Thing> {
        self.0.thing(name)
    }
}

/// A saved world.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Save {
    pub game: String,
    pub tick: u64,
    pub rng: u64,
    #[serde(default)]
    pub amount: BTreeMap<String, Num>,
    #[serde(default)]
    pub total: BTreeMap<String, Num>,
    #[serde(default)]
    pub ever: BTreeMap<String, Num>,
    #[serde(default)]
    pub upgrades: Vec<String>,
    #[serde(default)]
    pub milestones: Vec<String>,
    #[serde(default)]
    pub players: Vec<SavedPlayer>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedPlayer {
    pub id: String,
    pub name: String,
    pub joined: u64,
    pub run_start: u64,
    #[serde(default)]
    pub amount: BTreeMap<String, Num>,
    #[serde(default)]
    pub total: BTreeMap<String, Num>,
    #[serde(default)]
    pub ever: BTreeMap<String, Num>,
    #[serde(default)]
    pub contrib: BTreeMap<String, Num>,
    #[serde(default)]
    pub owned: BTreeMap<String, u64>,
    #[serde(default)]
    pub upgrades: Vec<String>,
    #[serde(default)]
    pub milestones: Vec<String>,
    #[serde(default)]
    pub seen: Vec<String>,
    #[serde(default)]
    pub prestiged: BTreeMap<String, u64>,
    #[serde(default)]
    pub ready: BTreeMap<String, u64>,
    #[serde(default)]
    pub until: BTreeMap<String, u64>,
}
