//! The worlds a server keeps, and the messages their players send.
//!
//! Every world runs on the server's clock, whoever is connected: it is an idle game. A browser
//! that joins gets the game's files and the whole world, then every tick number and every
//! player's command as the server carries them out, and runs the same engine on them, so it
//! shows the world as it is without the server sending it again. Now and then the server sends a
//! fingerprint of the world; a browser whose copy differs asks for the world again.
//!
//! Messages are JSON objects with a type `t`.
//!
//! From a browser:
//!
//! * `hello` `{game, world, token, name}`: enter a world (made if it does not exist). `token` is
//!   the browser's secret (8 to 64 letters and digits): the same token is the same player.
//! * `cmd` `{cmd}`: a command (`{"cmd": "buy", "id": "Kindler"}`, see `openidle::Command`).
//! * `name` `{name}`, `chat` `{text}`, `resync`.
//!
//! From the server:
//!
//! * `welcome` `{player, bundle, save, tick, chat}`: who you are, the game, the world.
//! * `tick` `{tick}`: run your copy up to this tick.
//! * `cmd` `{tick, player, cmd}`: run up to `tick`, then carry out the command.
//! * `check` `{tick, fp}`: the world's fingerprint at that tick (compare once you are there).
//! * `state` `{save, tick}` (after `resync`), `online` `{players}`, `chat` `{player, name, text}`,
//!   `err` `{msg}`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use openidle::{Command, Game, Save, World};
use serde_json::{json, Value};

pub type Out = Vec<(u64, Value)>;

/// Seconds between fingerprints sent to the browsers.
const CHECK_EVERY: f64 = 10.0;
const CHAT_KEPT: usize = 30;

pub struct Room {
    pub game: String,
    pub name: String,
    pub world: World,
    epoch: Instant,
    epoch_tick: u64,
    /// Token hash → player id.
    tokens: BTreeMap<String, String>,
    /// Connection → player id.
    pub peers: BTreeMap<u64, String>,
    chat: Vec<Value>,
    last_check: u64,
    pub dirty: bool,
}

impl Room {
    fn new(game: &str, name: &str, world: World, now: Instant) -> Room {
        let tick = world.tick();
        Room {
            game: game.into(),
            name: name.into(),
            world,
            epoch: now,
            epoch_tick: tick,
            tokens: BTreeMap::new(),
            peers: BTreeMap::new(),
            chat: Vec::new(),
            last_check: tick,
            dirty: true,
        }
    }

    pub fn key(&self) -> String {
        format!("{}/{}", self.game, self.name)
    }

    fn target(&self, now: Instant) -> u64 {
        self.epoch_tick + (now.duration_since(self.epoch).as_secs_f64() / self.world.game.tick) as u64
    }

    fn send_all(&self, out: &mut Out, msg: &Value) {
        for id in self.peers.keys() {
            out.push((*id, msg.clone()));
        }
    }

    fn online(&self) -> Value {
        let mut ids: Vec<&String> = self.peers.values().collect();
        ids.sort();
        ids.dedup();
        json!({ "t": "online", "players": ids })
    }

    /// Brings the world up to the clock; tells the browsers.
    pub fn advance(&mut self, now: Instant, out: &mut Out) {
        let target = self.target(now);
        if target <= self.world.tick() {
            return;
        }
        // Never more than a minute of ticks in one go, so one world cannot stall the others.
        let n = (target - self.world.tick()).min((60.0 / self.world.game.tick) as u64 + 1);
        self.world.run(n);
        self.world.take_events();
        self.dirty = true;
        if self.peers.is_empty() {
            return;
        }
        let t = self.world.tick();
        self.send_all(out, &json!({ "t": "tick", "tick": t }));
        if (t - self.last_check) as f64 * self.world.game.tick >= CHECK_EVERY {
            self.last_check = t;
            let fp = self.world.fingerprint();
            self.send_all(out, &json!({ "t": "check", "tick": t, "fp": fp }));
        }
    }

    /// Carries out a command for a player and passes it on to everyone.
    fn command(&mut self, player: &str, cmd: Command, out: &mut Out) -> Result<(), String> {
        self.world.apply(player, &cmd)?;
        self.world.take_events();
        self.dirty = true;
        let msg = json!({ "t": "cmd", "tick": self.world.tick(), "player": player, "cmd": cmd });
        self.send_all(out, &msg);
        Ok(())
    }

    pub fn to_file(&self) -> Value {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        json!({
            "game": self.game, "world": self.name, "saved_at": now,
            "tokens": self.tokens, "save": self.world.save(),
        })
    }
}

pub struct Conn {
    pub room: Option<String>,
    pub player: Option<String>,
}

pub struct Hub {
    pub games: BTreeMap<String, Arc<Game>>,
    pub rooms: BTreeMap<String, Room>,
    pub conns: HashMap<u64, Conn>,
    pub data: Option<PathBuf>,
    pub max_worlds: usize,
    pub max_players: usize,
}

fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

pub fn word(v: &Value, max: usize) -> Option<String> {
    v.as_str()
        .filter(|s| {
            !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .map(str::to_string)
}

fn short(v: &Value, max: usize) -> String {
    v.as_str()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

impl Hub {
    pub fn new(games: BTreeMap<String, Arc<Game>>, data: Option<PathBuf>) -> Hub {
        Hub {
            games,
            rooms: BTreeMap::new(),
            conns: HashMap::new(),
            data,
            max_worlds: 100,
            max_players: 200,
        }
    }

    /// Reads every saved world, and runs each for the time the server was away (up to its
    /// game's `offline_max`).
    pub fn load_all(&mut self, now: Instant) {
        let Some(dir) = self.data.clone() else { return };
        let Ok(games) = std::fs::read_dir(&dir) else { return };
        let unix = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        for g in games.flatten() {
            let Ok(files) = std::fs::read_dir(g.path()) else {
                continue;
            };
            for f in files.flatten() {
                let path = f.path();
                if path.extension().is_none_or(|e| e != "json") {
                    continue;
                }
                match self.load_room(&path, now, unix) {
                    Ok(key) => eprintln!("world {key} loaded"),
                    Err(e) => eprintln!("{}: {e}", path.display()),
                }
            }
        }
    }

    fn load_room(&mut self, path: &Path, now: Instant, unix: u64) -> Result<String, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let game = v["game"].as_str().ok_or("no game")?;
        let name = v["world"].as_str().ok_or("no world")?;
        let g = self
            .games
            .get(game)
            .ok_or(format!("no game `{game}` on this server"))?
            .clone();
        let save: Save = serde_json::from_value(v["save"].clone()).map_err(|e| e.to_string())?;
        let mut world = World::load(g.clone(), &save);
        let away = unix.saturating_sub(v["saved_at"].as_u64().unwrap_or(unix)) as f64;
        world.run((away.min(g.offline_max) / g.tick) as u64);
        world.take_events();
        let mut room = Room::new(game, name, world, now);
        room.tokens = serde_json::from_value(v["tokens"].clone()).unwrap_or_default();
        let key = room.key();
        self.rooms.insert(key.clone(), room);
        Ok(key)
    }

    pub fn save(&mut self, key: &str) {
        let (Some(dir), Some(room)) = (&self.data, self.rooms.get_mut(key)) else {
            return;
        };
        if !room.dirty {
            return;
        }
        let folder = dir.join(&room.game);
        let file = folder.join(format!("{}.json", room.name));
        let tmp = folder.join(format!("{}.json.tmp", room.name));
        let r = std::fs::create_dir_all(&folder)
            .and_then(|_| std::fs::write(&tmp, room.to_file().to_string()))
            .and_then(|_| std::fs::rename(&tmp, &file));
        match r {
            Ok(()) => room.dirty = false,
            Err(e) => eprintln!("world {key}: could not save: {e}"),
        }
    }

    pub fn save_all(&mut self) {
        let keys: Vec<String> = self.rooms.keys().cloned().collect();
        for k in keys {
            self.save(&k);
        }
    }

    /// Runs every world up to the clock.
    pub fn clock(&mut self, now: Instant) -> Out {
        let mut out = Out::new();
        for r in self.rooms.values_mut() {
            r.advance(now, &mut out);
        }
        out
    }

    pub fn connect(&mut self, id: u64) {
        self.conns.insert(
            id,
            Conn {
                room: None,
                player: None,
            },
        );
    }

    /// Takes a connection out of its world.
    pub fn leave(&mut self, id: u64) -> Out {
        let mut out = Out::new();
        let Some(key) = self.conns.get_mut(&id).and_then(|c| c.room.take()) else {
            return out;
        };
        if let Some(r) = self.rooms.get_mut(&key) {
            r.peers.remove(&id);
            let online = r.online();
            r.send_all(&mut out, &online);
        }
        out
    }

    pub fn disconnect(&mut self, id: u64) -> Out {
        let out = self.leave(id);
        self.conns.remove(&id);
        out
    }

    pub fn handle(&mut self, id: u64, msg: &Value, now: Instant) -> Out {
        match self.handle_inner(id, msg, now) {
            Ok(out) => out,
            Err(e) => vec![(id, json!({ "t": "err", "msg": e }))],
        }
    }

    fn handle_inner(&mut self, id: u64, msg: &Value, now: Instant) -> Result<Out, String> {
        let t = msg["t"].as_str().unwrap_or("");
        if t == "hello" {
            return self.hello(id, msg, now);
        }
        let conn = self.conns.get(&id).ok_or("who are you?")?;
        let (Some(key), Some(player)) = (conn.room.clone(), conn.player.clone()) else {
            return Err("say hello first".into());
        };
        let room = self.rooms.get_mut(&key).ok_or("that world is gone")?;
        let mut out = Out::new();
        room.advance(now, &mut out);
        match t {
            "cmd" => {
                let cmd: Command =
                    serde_json::from_value(msg["cmd"].clone()).map_err(|e| format!("not a command: {e}"))?;
                if matches!(cmd, Command::Join { .. }) {
                    return Err("use `name` to change your name".into());
                }
                if let Err(e) = room.command(&player, cmd, &mut out) {
                    out.push((id, json!({ "t": "err", "msg": e })));
                }
            }
            "name" => {
                let name = short(&msg["name"], 40);
                if name.is_empty() {
                    return Err("a name, please".into());
                }
                room.command(&player, Command::Join { name }, &mut out)?;
            }
            "chat" => {
                let text = short(&msg["text"], 300);
                if !text.is_empty() {
                    let name = room
                        .world
                        .players()
                        .find(|(p, _)| *p == player)
                        .map(|(_, n)| n.to_string())
                        .unwrap_or_default();
                    let m = json!({ "t": "chat", "player": player, "name": name, "text": text });
                    room.chat.push(m.clone());
                    if room.chat.len() > CHAT_KEPT {
                        room.chat.remove(0);
                    }
                    room.send_all(&mut out, &m);
                }
            }
            "resync" => {
                out.push((
                    id,
                    json!({ "t": "state", "tick": room.world.tick(), "save": room.world.save() }),
                ));
            }
            _ => return Err(format!("unknown message `{t}`")),
        }
        Ok(out)
    }

    fn hello(&mut self, id: u64, msg: &Value, now: Instant) -> Result<Out, String> {
        let game = word(&msg["game"], 32).ok_or("which game?")?;
        let world = word(&msg["world"], 32).ok_or("which world? (letters, digits, - and _)")?;
        let token = msg["token"]
            .as_str()
            .filter(|t| (8..=64).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_alphanumeric()))
            .ok_or("a token of 8 to 64 letters and digits, please")?;
        let mut name = short(&msg["name"], 40);
        if name.is_empty() {
            name = "Player".into();
        }
        let g = self
            .games
            .get(&game)
            .ok_or_else(|| format!("no game `{game}` here"))?
            .clone();
        let key = format!("{game}/{world}");
        if !self.rooms.contains_key(&key) {
            if self.rooms.len() >= self.max_worlds {
                return Err("this server has all the worlds it can hold".into());
            }
            let seed = now.elapsed().as_nanos() as u64
                ^ hash(&key).len() as u64
                ^ (SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos() as u64));
            self.rooms
                .insert(key.clone(), Room::new(&game, &world, World::new(g.clone(), seed), now));
        }
        let mut out = self.leave(id);
        let max_players = self.max_players;
        let room = self.rooms.get_mut(&key).unwrap();
        room.advance(now, &mut out);
        let th = hash(&format!("{key}:{token}"));
        let player = match room.tokens.get(&th) {
            Some(p) => p.clone(),
            None => {
                if room.tokens.len() >= max_players {
                    return Err("this world is full".into());
                }
                let p = format!("p{}", room.tokens.len() + 1);
                room.tokens.insert(th, p.clone());
                p
            }
        };
        if !room.world.has_player(&player) {
            room.command(&player, Command::Join { name }, &mut out)?;
        }
        room.peers.insert(id, player.clone());
        out.push((
            id,
            json!({
                "t": "welcome", "player": player, "game": game, "world": world,
                "bundle": g.bundle(), "save": room.world.save(), "tick": room.world.tick(),
                "chat": room.chat,
            }),
        ));
        let online = room.online();
        room.send_all(&mut out, &online);
        if let Some(c) = self.conns.get_mut(&id) {
            c.room = Some(key);
            c.player = Some(player);
        }
        Ok(out)
    }

    pub fn report(&self) -> Value {
        let worlds: Vec<Value> = self
            .rooms
            .values()
            .map(|r| {
                json!({
                    "game": r.game, "world": r.name, "players": r.world.players().count(),
                    "online": r.peers.len(), "tick": r.world.tick(), "seconds": r.world.seconds(),
                })
            })
            .collect();
        json!({
            "games": self.games.keys().collect::<Vec<_>>(),
            "connections": self.conns.len(),
            "worlds": worlds,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub() -> Hub {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/sparks");
        let g = Arc::new(Game::from_dir(&dir, &[]).unwrap());
        Hub::new(BTreeMap::from([("sparks".to_string(), g)]), None)
    }

    fn to(out: &Out, id: u64, t: &str) -> Vec<Value> {
        out.iter()
            .filter(|(c, m)| *c == id && m["t"] == t)
            .map(|(_, m)| m.clone())
            .collect()
    }

    /// A browser: follows the server's messages with its own copy of the world.
    struct Browser {
        world: Option<World>,
        checks: usize,
    }

    impl Browser {
        fn take(&mut self, out: &Out, me: u64) {
            for (c, m) in out {
                if *c != me {
                    continue;
                }
                match m["t"].as_str().unwrap() {
                    "welcome" | "state" => {
                        let g = Arc::new(
                            Game::from_bundle(&m["bundle"])
                                .unwrap_or_else(|_| (*self.world.as_ref().unwrap().game).clone()),
                        );
                        let save: Save = serde_json::from_value(m["save"].clone()).unwrap();
                        self.world = Some(World::load(g, &save));
                    }
                    "tick" => {
                        let w = self.world.as_mut().unwrap();
                        let t = m["tick"].as_u64().unwrap();
                        w.run(t - w.tick());
                    }
                    "cmd" => {
                        let w = self.world.as_mut().unwrap();
                        let t = m["tick"].as_u64().unwrap();
                        w.run(t - w.tick());
                        let cmd: Command = serde_json::from_value(m["cmd"].clone()).unwrap();
                        w.apply(m["player"].as_str().unwrap(), &cmd).unwrap();
                    }
                    "check" => {
                        self.checks += 1;
                        let w = self.world.as_ref().unwrap();
                        assert_eq!(w.tick(), m["tick"].as_u64().unwrap());
                        assert_eq!(w.fingerprint(), m["fp"].as_str().unwrap(), "the browser's copy drifted");
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn browsers_follow_the_world() {
        let mut h = hub();
        let t0 = Instant::now();
        h.connect(1);
        h.connect(2);
        let hello = |token: &str, name: &str| json!({ "t": "hello", "game": "sparks", "world": "main", "token": token, "name": name });
        let mut a = Browser { world: None, checks: 0 };
        let mut b = Browser { world: None, checks: 0 };
        let out = h.handle(1, &hello("aaaaaaaaaa", "Ada"), t0);
        assert_eq!(to(&out, 1, "welcome")[0]["player"], "p1");
        a.take(&out, 1);
        for s in 1..=120u64 {
            let now = t0 + std::time::Duration::from_secs(s);
            if s == 5 {
                let out = h.handle(2, &hello("bbbbbbbbbb", "Bo"), now);
                a.take(&out, 1);
                b.take(&out, 2);
            }
            for id in [1u64, 2] {
                let joined = if id == 1 { a.world.is_some() } else { b.world.is_some() };
                if !joined {
                    continue;
                }
                for _ in 0..3 {
                    let out = h.handle(id, &json!({ "t": "cmd", "cmd": { "cmd": "act", "id": "strike" } }), now);
                    a.take(&out, 1);
                    b.take(&out, 2);
                }
                let out = h.handle(
                    id,
                    &json!({ "t": "cmd", "cmd": { "cmd": "buy", "id": "Kindler", "max": true } }),
                    now,
                );
                a.take(&out, 1);
                b.take(&out, 2);
            }
            let out = h.clock(now);
            a.take(&out, 1);
            b.take(&out, 2);
        }
        assert!(a.checks >= 10 && b.checks >= 10, "{} {}", a.checks, b.checks);
        let key = "sparks/main";
        let fp = h.rooms[key].world.fingerprint();
        assert_eq!(a.world.as_ref().unwrap().fingerprint(), fp);
        assert_eq!(b.world.as_ref().unwrap().fingerprint(), fp);
        // The same token is the same player.
        h.connect(3);
        let out = h.handle(
            3,
            &hello("bbbbbbbbbb", "Bo again"),
            t0 + std::time::Duration::from_secs(121),
        );
        assert_eq!(to(&out, 3, "welcome")[0]["player"], "p2");
    }

    #[test]
    fn refuses_what_it_should() {
        let mut h = hub();
        let t0 = Instant::now();
        h.connect(1);
        let out = h.handle(1, &json!({ "t": "cmd", "cmd": { "cmd": "act", "id": "strike" } }), t0);
        assert_eq!(to(&out, 1, "err").len(), 1);
        let out = h.handle(
            1,
            &json!({ "t": "hello", "game": "nope", "world": "w", "token": "aaaaaaaaaa" }),
            t0,
        );
        assert!(to(&out, 1, "err")[0]["msg"].as_str().unwrap().contains("nope"));
        let out = h.handle(
            1,
            &json!({ "t": "hello", "game": "sparks", "world": "w", "token": "short" }),
            t0,
        );
        assert_eq!(to(&out, 1, "err").len(), 1);
        h.handle(
            1,
            &json!({ "t": "hello", "game": "sparks", "world": "w", "token": "aaaaaaaaaa" }),
            t0,
        );
        let out = h.handle(1, &json!({ "t": "cmd", "cmd": { "cmd": "buy", "id": "Kindler" } }), t0);
        assert!(to(&out, 1, "err")[0]["msg"].as_str().unwrap().contains("not enough"));
    }
}
