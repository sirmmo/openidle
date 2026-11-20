//! The `openidle` command.

use std::path::PathBuf;
use std::sync::Arc;

use openidle::bot::Bot;
use openidle::{Game, Num, World};

const USAGE: &str = "\
openidle check  GAME_DIR [--set name=value]...
openidle play   GAME_DIR [--minutes M | --hours H] [--players N] [--clicks C] [--seed S]
                         [--every SECONDS] [--quiet] [--set name=value]...
openidle bundle GAME_DIR [-o FILE]

check   reads the game and lists what it declares
play    plays it with a simple bot (or several, in one world), faster than real time, and
        prints what happened when: unlocks, milestones, upgrades, prestiges
bundle  writes the game as one JSON file for the browser (default: GAME_DIR.json)";

struct Args {
    cmd: String,
    dir: PathBuf,
    sets: Vec<(String, String)>,
    seconds: f64,
    players: usize,
    clicks: u32,
    seed: u64,
    every: Option<f64>,
    quiet: bool,
    out: Option<PathBuf>,
}

fn args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().ok_or(USAGE)?;
    if cmd == "-h" || cmd == "--help" {
        return Err(USAGE.into());
    }
    let dir = PathBuf::from(it.next().ok_or(USAGE)?);
    let mut a = Args {
        cmd,
        dir,
        sets: Vec::new(),
        seconds: 3600.0,
        players: 1,
        clicks: 3,
        seed: 1,
        every: None,
        quiet: false,
        out: None,
    };
    while let Some(f) = it.next() {
        let mut val = || it.next().ok_or(format!("{f} needs a value"));
        let num = |s: String| s.parse::<f64>().map_err(|_| format!("{f}: `{s}` is not a number"));
        match f.as_str() {
            "--set" => {
                let s = val()?;
                let (k, v) = s.split_once('=').ok_or("--set name=value")?;
                a.sets.push((k.to_string(), v.to_string()));
            }
            "--minutes" => a.seconds = num(val()?)? * 60.0,
            "--hours" => a.seconds = num(val()?)? * 3600.0,
            "--players" => a.players = num(val()?)? as usize,
            "--clicks" => a.clicks = num(val()?)? as u32,
            "--seed" => a.seed = num(val()?)? as u64,
            "--every" => a.every = Some(num(val()?)?),
            "--quiet" => a.quiet = true,
            "-o" | "--out" => a.out = Some(PathBuf::from(val()?)),
            _ => return Err(format!("unknown option {f}\n\n{USAGE}")),
        }
    }
    Ok(a)
}

fn clock(s: f64) -> String {
    let s = s as u64;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// A number for people: 1234 → 1.23K, 1e15 → 1.00e15.
fn short(n: Num) -> String {
    let x = n.to_f64();
    if n.is_zero() {
        return "0".into();
    }
    let l = n.log10();
    if l < 3.0 {
        return format!("{}", (x * 100.0).round() / 100.0);
    }
    if l < 15.0 {
        let units = ["K", "M", "B", "T"];
        let k = ((l / 3.0).floor() as usize).min(4);
        return format!("{:.2}{}", x / 10f64.powi(3 * k as i32), units[k - 1]);
    }
    let e = l.floor();
    format!("{:.2}e{}", 10f64.powf(l - e), e)
}

fn run() -> Result<(), String> {
    let a = args()?;
    let game = Game::from_dir(&a.dir, &a.sets).map_err(|e| format!("{}: {e}", a.dir.display()))?;
    match a.cmd.as_str() {
        "check" => {
            println!("{}: {}", game.name, game.description);
            println!("  tick {} s, offline up to {} s", game.tick, game.offline_max);
            let list = |label: &str, ids: Vec<&str>| {
                if !ids.is_empty() {
                    println!("  {} {}: {}", ids.len(), label, ids.join(", "));
                }
            };
            list("resources", game.resources.iter().map(|x| x.info.id.as_str()).collect());
            list(
                "generators",
                game.generators.iter().map(|x| x.info.id.as_str()).collect(),
            );
            list("rules", game.rules.iter().map(|x| x.id.as_str()).collect());
            list("upgrades", game.upgrades.iter().map(|x| x.info.id.as_str()).collect());
            list("actions", game.actions.iter().map(|x| x.info.id.as_str()).collect());
            list(
                "milestones",
                game.milestones.iter().map(|x| x.info.id.as_str()).collect(),
            );
            list("prestiges", game.prestiges.iter().map(|x| x.info.id.as_str()).collect());
            println!(
                "  {} boosts and discounts, {} parameters",
                game.boosts.len(),
                game.params.len()
            );
        }
        "bundle" => {
            let out = a.out.unwrap_or_else(|| a.dir.with_extension("json"));
            std::fs::write(&out, game.bundle().to_string()).map_err(|e| format!("{}: {e}", out.display()))?;
            println!("{} written", out.display());
        }
        "play" => play(game, &a),
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

fn play(game: Game, a: &Args) {
    let tick = game.tick;
    let mut w = World::new(Arc::new(game), a.seed);
    let ids: Vec<String> = (1..=a.players.max(1)).map(|i| format!("p{i}")).collect();
    for id in &ids {
        w.join(id, &format!("Bot {}", &id[1..]));
    }
    w.take_events();
    let bot = Bot {
        clicks: a.clicks,
        ..Bot::default()
    };
    let per_second = (1.0 / tick).round().max(1.0) as u64;
    let seconds = a.seconds as u64;
    let every = a.every.map_or((seconds / 10).max(1), |e| e as u64).max(1);
    let one = ids.len() == 1;
    let started = std::time::Instant::now();
    for s in 0..seconds {
        for id in &ids {
            bot.second(&mut w, id);
        }
        w.run(per_second);
        if !a.quiet {
            for e in w.take_events() {
                let kind = e["kind"].as_str().unwrap_or_default();
                if kind == "join" {
                    continue;
                }
                let who = if one {
                    String::new()
                } else {
                    format!("{:>4} ", e["player"].as_str().unwrap_or("all"))
                };
                let label = e["label"].as_str().or(e["id"].as_str()).unwrap_or_default();
                let extra = match kind {
                    "prestige" => format!(" {}", e["gain"]),
                    "upgrade" if e["scope"] == "world" => " (for everyone)".into(),
                    _ => String::new(),
                };
                println!("{} {who}{kind:<9} {label}{extra}", clock(w.seconds()));
            }
        } else {
            w.take_events();
        }
        if (s + 1) % every == 0 || s + 1 == seconds {
            let v = w.view(&ids[0]).unwrap();
            let res: Vec<String> = v["resources"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["visible"] == true)
                .map(|r| {
                    let n = |k: &str| r[k].as_str().unwrap_or("0").parse::<Num>().unwrap_or(Num::ZERO);
                    format!(
                        "{} {} ({}/s)",
                        r["id"].as_str().unwrap(),
                        short(n("amount")),
                        short(n("rate"))
                    )
                })
                .collect();
            let gens: Vec<String> = v["generators"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|g| g["owned"].as_u64().unwrap_or(0) > 0)
                .map(|g| format!("{} {}", g["owned"], g["id"].as_str().unwrap()))
                .collect();
            println!("{} == {} | {}", clock(w.seconds()), res.join(", "), gens.join(", "));
        }
    }
    eprintln!(
        "{} of game time for {} player(s) in {:.2} s; fingerprint {}",
        clock(w.seconds()),
        ids.len(),
        started.elapsed().as_secs_f64(),
        w.fingerprint()
    );
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
