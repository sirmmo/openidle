//! A simple player, to test a game's pace: `openidle play` runs it for hours of game time in
//! seconds, and a page can use it for "autoplay".
//!
//! Every second it strikes the clickable actions `clicks` times, uses actions with a cooldown
//! when they are ready, buys every upgrade it can, then generators, cheapest first, and takes a
//! prestige when it would at least double what the prestige gives (and the run is a minute old).

use serde_json::Value;

use crate::num::Num;
use crate::world::{Command, World};

#[derive(Clone, Debug)]
pub struct Bot {
    /// Clicks a second on actions without a cooldown.
    pub clicks: u32,
    /// Take a prestige when it gives at least this many times what the player has of it.
    pub prestige_factor: f64,
}

impl Default for Bot {
    fn default() -> Bot {
        Bot {
            clicks: 3,
            prestige_factor: 1.0,
        }
    }
}

fn num(v: &Value) -> Num {
    match v {
        Value::String(s) => s.parse().unwrap_or(Num::ZERO),
        Value::Number(n) => Num::from_f64(n.as_f64().unwrap_or(0.0)),
        _ => Num::ZERO,
    }
}

fn amount_of(view: &Value, res: &str) -> Num {
    view["resources"]
        .as_array()
        .and_then(|a| a.iter().find(|r| r["id"] == res))
        .map(|r| num(&r["amount"]))
        .unwrap_or(Num::ZERO)
}

impl Bot {
    /// One second of decisions for `player`; returns the commands it carried out.
    pub fn second(&self, w: &mut World, player: &str) -> Vec<Command> {
        let mut done = Vec::new();
        let mut try_cmd = |w: &mut World, c: Command| {
            if w.apply(player, &c).is_ok() {
                done.push(c);
                true
            } else {
                false
            }
        };
        let Ok(view) = w.view(player) else {
            return done;
        };
        for a in view["actions"].as_array().into_iter().flatten() {
            if a["visible"] != true {
                continue;
            }
            let id = a["id"].as_str().unwrap_or_default().to_string();
            let times = if a["cooldown"].as_f64().unwrap_or(0.0) > 0.0 {
                1
            } else {
                self.clicks
            };
            for _ in 0..times {
                if !try_cmd(w, Command::Act { id: id.clone() }) {
                    break;
                }
            }
        }
        if let Ok(view) = w.view(player) {
            for p in view["prestiges"].as_array().into_iter().flatten() {
                if p["can"] != true || num(&view["player"]["runtime"]) < Num::from_f64(60.0) {
                    continue;
                }
                let first = &p["gain"][0];
                let gain = num(&first["amount"]);
                let have = amount_of(&view, first["res"].as_str().unwrap_or_default());
                if gain >= Num::ONE.max(have * Num::from_f64(self.prestige_factor)) {
                    try_cmd(
                        w,
                        Command::Prestige {
                            id: p["id"].as_str().unwrap_or_default().into(),
                        },
                    );
                }
            }
        }
        for _ in 0..200 {
            let Ok(view) = w.view(player) else { break };
            let mut bought = false;
            for u in view["upgrades"].as_array().into_iter().flatten() {
                if u["can"] == true && u["auto"] != true {
                    bought |= try_cmd(
                        w,
                        Command::Buy {
                            id: u["id"].as_str().unwrap_or_default().into(),
                            n: None,
                            max: false,
                        },
                    );
                }
            }
            // The generator whose price is the smallest share of what the player has.
            let best = view["generators"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|g| g["can"] == true)
                .filter_map(|g| {
                    let c = &g["cost"][0];
                    let share = num(&c["amount"]) / amount_of(&view, c["res"].as_str()?).max(Num::from_f64(1e-300));
                    Some((share, g["id"].as_str()?.to_string()))
                })
                .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            if let Some((_, id)) = best {
                bought |= try_cmd(
                    w,
                    Command::Buy {
                        id,
                        n: None,
                        max: false,
                    },
                );
            }
            if !bought {
                break;
            }
        }
        done
    }
}
