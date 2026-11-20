//! OpenIdle in the browser. The page sends JSON requests and gets JSON answers through three
//! functions, with no generated glue (`web/openidle.js` wraps them):
//!
//! ```js
//! const bytes = new TextEncoder().encode(JSON.stringify(request));
//! const ptr = wasm.oi_alloc(bytes.length);
//! new Uint8Array(wasm.memory.buffer, ptr, bytes.length).set(bytes);
//! const out = wasm.oi_call(ptr, bytes.length);       // frees the request
//! const answer = JSON.parse(new TextDecoder().decode(
//!     new Uint8Array(wasm.memory.buffer, out, wasm.oi_out_len())));
//! ```
//!
//! Requests (`op`):
//!
//! * `load` `{bundle, seed?, save?}`: the game (a bundle from `openidle bundle`) and a new world,
//!   or the saved one. `check` `{bundle}` only reads the game and says what is wrong.
//! * `join` `{player, name}`; `apply` `{player, cmd}` (a command: `{"cmd": "buy", "id": ...}`).
//! * `step` `{ticks}`; `to` `{tick}` runs up to that tick (to follow a server);
//!   `offline` `{seconds}` runs the time a player was away (up to the game's `offline_max`).
//! * `view` `{player}`, `save`, `fingerprint`, `eval` `{player?, expr}`, `bot` `{player, clicks?}`
//!   (one second of the bot's decisions).
//!
//! Answers carry `error` when something went wrong, and `events` (what happened) when the world
//! changed.

use std::cell::RefCell;
use std::sync::Arc;

use openidle::bot::Bot;
use openidle::{Command, Game, Save, World};
use serde_json::{json, Value};

thread_local! {
    static WORLD: RefCell<Option<World>> = const { RefCell::new(None) };
    static OUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Room for a request of `len` bytes.
#[no_mangle]
pub extern "C" fn oi_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Answers the request at `ptr` (and frees it); the answer is at the returned pointer, for
/// [`oi_out_len`] bytes, until the next call.
///
/// # Safety
/// `ptr` must come from [`oi_alloc`] with `len` bytes written.
#[no_mangle]
pub unsafe extern "C" fn oi_call(ptr: *mut u8, len: usize) -> *const u8 {
    let bytes = Vec::from_raw_parts(ptr, len, len.max(1));
    let answer = match std::str::from_utf8(&bytes)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(t).ok())
    {
        Some(req) => handle(&req).unwrap_or_else(|e| json!({ "error": e })),
        None => json!({ "error": "the request is not JSON" }),
    };
    OUT.with(|o| {
        let mut o = o.borrow_mut();
        *o = answer.to_string().into_bytes();
        o.as_ptr()
    })
}

#[no_mangle]
pub extern "C" fn oi_out_len() -> usize {
    OUT.with(|o| o.borrow().len())
}

fn s<'a>(req: &'a Value, k: &str) -> Result<&'a str, String> {
    req[k].as_str().ok_or_else(|| format!("`{k}` is missing"))
}

/// Answers one request (also usable natively, for tests).
pub fn handle(req: &Value) -> Result<Value, String> {
    let op = s(req, "op")?;
    match op {
        "check" => {
            let g = Game::from_bundle(&req["bundle"]).map_err(|e| e.to_string())?;
            return Ok(json!({ "name": g.name, "description": g.description, "tick": g.tick }));
        }
        "load" => {
            let g = Arc::new(Game::from_bundle(&req["bundle"]).map_err(|e| e.to_string())?);
            let w = if req["save"].is_object() {
                let save: Save = serde_json::from_value(req["save"].clone()).map_err(|e| format!("not a save: {e}"))?;
                World::load(g.clone(), &save)
            } else {
                World::new(g.clone(), req["seed"].as_u64().unwrap_or(1))
            };
            let answer = json!({
                "name": g.name, "description": g.description, "tick": w.tick(),
                "tick_seconds": g.tick, "offline_max": g.offline_max,
            });
            WORLD.with(|c| *c.borrow_mut() = Some(w));
            return Ok(answer);
        }
        _ => {}
    }
    WORLD.with(|c| {
        let mut c = c.borrow_mut();
        let w = c.as_mut().ok_or("no game: send `load` first")?;
        let changed = |w: &mut World, mut v: Value| {
            v["tick"] = json!(w.tick());
            v["events"] = Value::Array(w.take_events());
            v
        };
        Ok(match op {
            "join" => {
                w.join(s(req, "player")?, req["name"].as_str().unwrap_or("Player"));
                changed(w, json!({}))
            }
            "apply" => {
                let cmd: Command =
                    serde_json::from_value(req["cmd"].clone()).map_err(|e| format!("not a command: {e}"))?;
                w.apply(s(req, "player")?, &cmd)?;
                changed(w, json!({}))
            }
            "step" => {
                let n = req["ticks"].as_u64().unwrap_or(1).min(10_000_000);
                w.run(n);
                changed(w, json!({}))
            }
            "to" => {
                let t = req["tick"].as_u64().ok_or("`tick` is missing")?;
                let n = t.saturating_sub(w.tick()).min(10_000_000);
                w.run(n);
                changed(w, json!({}))
            }
            "offline" => {
                let secs = req["seconds"].as_f64().unwrap_or(0.0).clamp(0.0, w.game.offline_max);
                let n = (secs / w.game.tick).floor() as u64;
                w.run(n);
                changed(w, json!({ "ticks": n, "seconds": n as f64 * w.game.tick }))
            }
            "view" => w.view(s(req, "player")?)?,
            "save" => serde_json::to_value(w.save()).map_err(|e| e.to_string())?,
            "fingerprint" => json!({ "tick": w.tick(), "fingerprint": w.fingerprint() }),
            "eval" => json!({ "value": w.eval(req["player"].as_str(), s(req, "expr")?)? }),
            "bot" => {
                let bot = Bot {
                    clicks: req["clicks"].as_u64().unwrap_or(3) as u32,
                    ..Bot::default()
                };
                let done = bot.second(w, s(req, "player")?);
                changed(w, json!({ "commands": done }))
            }
            _ => return Err(format!("no op `{op}`")),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_through_json() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/sparks");
        let bundle = Game::from_dir(&dir, &[]).unwrap().bundle();
        handle(&json!({ "op": "load", "bundle": bundle, "seed": 4 })).unwrap();
        handle(&json!({ "op": "join", "player": "me", "name": "Me" })).unwrap();
        let r = handle(&json!({ "op": "apply", "player": "me", "cmd": { "cmd": "act", "id": "strike" } })).unwrap();
        assert_eq!(r["tick"], 0);
        let e = handle(&json!({ "op": "apply", "player": "me", "cmd": { "cmd": "buy", "id": "Kindler" } }));
        assert!(e.unwrap_err().contains("not enough"));
        handle(&json!({ "op": "offline", "seconds": 1e9 })).unwrap();
        let v = handle(&json!({ "op": "view", "player": "me" })).unwrap();
        assert_eq!(v["tick"], 144_000);
        let save = handle(&json!({ "op": "save" })).unwrap();
        let fp = handle(&json!({ "op": "fingerprint" })).unwrap();
        handle(&json!({ "op": "load", "bundle": bundle, "save": save })).unwrap();
        assert_eq!(handle(&json!({ "op": "fingerprint" })).unwrap(), fp);
        assert_eq!(
            handle(&json!({ "op": "eval", "player": "me", "expr": "Sparks" })).unwrap()["value"],
            "1"
        );
    }
}
