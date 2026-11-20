//! The engine on the demo game: determinism, saves, several players, prestige, errors.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use openidle::bot::Bot;
use openidle::{Command, Game, Num, Save, World};

fn game() -> Arc<Game> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/sparks");
    Arc::new(Game::from_dir(&dir, &[]).expect("the demo game loads"))
}

fn amount(w: &World, p: &str, r: &str) -> f64 {
    w.amount(Some(p), r).unwrap().to_f64()
}

/// Plays `seconds` with bots for each player.
fn play(w: &mut World, players: &[&str], seconds: u64) {
    let bot = Bot::default();
    for _ in 0..seconds {
        for p in players {
            bot.second(w, p);
        }
        w.run(5);
    }
}

#[test]
fn the_same_commands_give_the_same_world() {
    let g = game();
    let mut a = World::new(g.clone(), 7);
    let mut b = World::new(g, 7);
    for w in [&mut a, &mut b] {
        w.join("p1", "Ada");
        w.join("p2", "Bo");
        play(w, &["p1", "p2"], 600);
    }
    assert_eq!(a.fingerprint(), b.fingerprint());
    assert!(amount(&a, "p1", "Stars") >= 1.0, "the bot ascended in ten minutes");
}

#[test]
fn a_saved_world_goes_on_as_before() {
    let g = game();
    let mut a = World::new(g.clone(), 3);
    a.join("p1", "Ada");
    play(&mut a, &["p1"], 200);
    let text = serde_json::to_string(&a.save()).unwrap();
    let save: Save = serde_json::from_str(&text).unwrap();
    let mut b = World::load(g, &save);
    assert_eq!(a.fingerprint(), b.fingerprint());
    play(&mut a, &["p1"], 100);
    play(&mut b, &["p1"], 100);
    assert_eq!(a.fingerprint(), b.fingerprint());
}

#[test]
fn generators_produce_and_cost_more() {
    let mut w = World::new(game(), 1);
    w.join("p1", "Ada");
    for _ in 0..10 {
        w.apply("p1", &Command::Act { id: "strike".into() }).unwrap();
    }
    assert_eq!(amount(&w, "p1", "Sparks"), 10.0);
    w.apply(
        "p1",
        &Command::Buy {
            id: "Kindler".into(),
            n: None,
            max: false,
        },
    )
    .unwrap();
    assert_eq!(amount(&w, "p1", "Sparks"), 0.0);
    let err = w
        .apply(
            "p1",
            &Command::Buy {
                id: "Kindler".into(),
                n: None,
                max: false,
            },
        )
        .unwrap_err();
    assert!(err.contains("not enough Sparks"), "{err}");
    w.run(50); // ten seconds: 10 sparks, and the beacon gets its share
    assert!((amount(&w, "p1", "Sparks") - 10.0).abs() < 1e-9);
    assert!(w.amount(None, "Beacon").unwrap().is_positive());
    let v = w.view("p1").unwrap();
    let cost = &v["generators"][0]["cost"][0]["amount"];
    assert_eq!(cost.as_str().unwrap().parse::<Num>().unwrap().to_f64(), 11.5);
    // Not unlocked yet.
    assert!(w
        .apply(
            "p1",
            &Command::Buy {
                id: "Forge".into(),
                n: None,
                max: false
            }
        )
        .is_err());
}

#[test]
fn prestige_resets_and_keeps() {
    let mut w = World::new(game(), 1);
    w.join("p1", "Ada");
    play(&mut w, &["p1"], 200);
    let v = w.view("p1").unwrap();
    let asc = &v["prestiges"][0];
    assert_eq!(asc["count"], 1, "{asc}");
    assert!(amount(&w, "p1", "Gems") >= 2.0);
    // A fresh run has no Forge yet.
    let mut fresh = World::new(game(), 1);
    fresh.join("p1", "Ada");
    let err = fresh
        .apply("p1", &Command::Prestige { id: "ascend".into() })
        .unwrap_err();
    assert!(err.contains("not available") || err.contains("nothing"), "{err}");
}

#[test]
fn players_share_the_world() {
    let mut w = World::new(game(), 1);
    for p in ["a", "b", "c"] {
        w.join(p, p);
    }
    play(&mut w, &["a", "b", "c"], 300);
    let v = w.view("a").unwrap();
    let beacon = v["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "Beacon")
        .unwrap()
        .clone();
    assert_eq!(beacon["scope"], "world");
    let contrib = beacon["contrib"].as_str().unwrap().parse::<Num>().unwrap();
    assert!(contrib.is_positive());
    let lit = v["milestones"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "lit")
        .unwrap()
        .clone();
    assert_eq!(lit["done"], true);
    // The world's upgrade is bought once, for everyone.
    let house = |p: &str| {
        let v = w.view(p).unwrap();
        v["upgrades"]
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["id"] == "Lighthouse")
            .unwrap()["bought"]
            .clone()
    };
    assert_eq!(house("a"), house("c"));
    // Gifts.
    let gems = amount(&w, "a", "Gems");
    w.apply(
        "a",
        &Command::Gift {
            to: "b".into(),
            res: "Gems".into(),
            amount: Num::ONE,
        },
    )
    .unwrap();
    assert_eq!(amount(&w, "a", "Gems"), gems - 1.0);
    assert!(w
        .apply(
            "a",
            &Command::Gift {
                to: "b".into(),
                res: "Sparks".into(),
                amount: Num::ONE
            }
        )
        .is_err());
}

#[test]
fn script_errors_say_where() {
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    files.insert("game.toml".into(), "name = \"x\"\nscripts = [\"a.idle\"]\n".into());
    files.insert(
        "a.idle".into(),
        "resource Gold\nend\n\ngenerator Mine\n    cost Silver 10\nend\n".into(),
    );
    let e = Game::from_files(&files, &[]).unwrap_err();
    assert_eq!(e.to_string(), "a.idle:5: no resource `Silver`");
    files.insert("a.idle".into(), "resource Gold\n    boost Gold 1 +\nend\n".into());
    let e = Game::from_files(&files, &[]).unwrap_err();
    assert_eq!(e.line, 2);
    files.insert("a.idle".into(), "resource Gold\n    max $nope\nend\n".into());
    let e = Game::from_files(&files, &[]).unwrap_err();
    assert!(e.msg.contains("nope"));
}

#[test]
fn parameters_change_the_game() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/sparks");
    let g = Arc::new(Game::from_dir(&dir, &[("strike".into(), "5".into())]).unwrap());
    let mut w = World::new(g, 1);
    w.join("p1", "Ada");
    w.apply("p1", &Command::Act { id: "strike".into() }).unwrap();
    assert_eq!(amount(&w, "p1", "Sparks"), 5.0);
    assert!(Game::from_dir(&dir, &[("nope".into(), "5".into())]).is_err());
}

#[test]
fn bundles_round_trip() {
    let g = game();
    let again = Game::from_bundle(&g.bundle()).unwrap();
    assert_eq!(again.name, g.name);
    assert_eq!(again.boosts.len(), g.boosts.len());
}
