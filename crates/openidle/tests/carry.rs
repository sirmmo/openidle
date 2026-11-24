//! A prestige can let some generators follow into the next run, each up to a cap.
use std::collections::BTreeMap;
use std::sync::Arc;

use openidle::{Command, Game, Num, World};

fn world() -> World {
    let files = BTreeMap::from([
        (
            "game.toml".to_string(),
            "name = \"Carry\"\nscripts = [\"carry.idle\"]\ntick = 1\n".to_string(),
        ),
        (
            "carry.idle".to_string(),
            "
resource Coin
end
resource Fame
end
generator Hand
    cost Coin 1
end
generator Cart
    cost Coin 1
    start 1
end
generator Mule
    cost Coin 1
end
prestige sell
    gain Fame 2
    carry Hand Cart max 1 + Fame
end
"
            .to_string(),
        ),
    ]);
    let mut w = World::new(Arc::new(Game::from_files(&files, &[]).unwrap()), 1);
    w.join("p", "P");
    w
}

fn owned(w: &World, g: &str) -> f64 {
    w.eval(Some("p"), g).unwrap().to_f64()
}

fn buy(w: &mut World, id: &str, n: u32) {
    let cmd = Command::Buy {
        id: id.into(),
        n: Some(n),
        max: false,
    };
    w.apply("p", &cmd).unwrap();
}

#[test]
fn carried_generators_follow_up_to_their_cap() {
    let mut w = world();
    w.credit(Some("p"), "Coin", Num::from_f64(100.0)).unwrap();
    buy(&mut w, "Hand", 5);
    buy(&mut w, "Mule", 4);
    w.apply("p", &Command::Prestige { id: "sell".into() }).unwrap();
    // The cap is worked out after the gains: 1 + 2 fame.
    assert_eq!(owned(&w, "Hand"), 3.0);
    // Never below where a generator starts; what is not carried starts over.
    assert_eq!(owned(&w, "Cart"), 1.0);
    assert_eq!(owned(&w, "Mule"), 0.0);
    assert_eq!(owned(&w, "Coin"), 0.0);
    // Fewer than the cap: all of them follow.
    w.credit(Some("p"), "Coin", Num::from_f64(100.0)).unwrap();
    buy(&mut w, "Hand", 1);
    w.apply("p", &Command::Prestige { id: "sell".into() }).unwrap();
    assert_eq!(owned(&w, "Fame"), 4.0);
    assert_eq!(owned(&w, "Hand"), 4.0);
}

#[test]
fn carry_takes_generators_and_a_cap() {
    let bad = |line: &str| {
        let files = BTreeMap::from([
            ("game.toml".to_string(), "name = \"C\"\nscripts = [\"c.idle\"]\n".to_string()),
            (
                "c.idle".to_string(),
                format!("resource Coin\nend\ngenerator Hand\n    cost Coin 1\nend\nprestige sell\n    gain Coin 1\n    {line}\nend\n"),
            ),
        ]);
        Game::from_files(&files, &[]).unwrap_err().to_string()
    };
    assert!(bad("carry Hand").contains("max"));
    assert!(bad("carry Coin max 3").contains("not a generator"));
    assert!(bad("carry Nobody max 3").contains("no generator"));
}
