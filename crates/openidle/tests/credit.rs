//! A host engine credits resources from outside the game.
use std::collections::BTreeMap;
use std::sync::Arc;

use openidle::{Game, Num, World};

fn world() -> World {
    let files = BTreeMap::from([
        (
            "game.toml".to_string(),
            "name = \"Host\"\nscripts = [\"host.idle\"]\ntick = 1\n".to_string(),
        ),
        (
            "host.idle".to_string(),
            "
resource Coin
end
resource Served
    cap 5
end
resource Pool
    scope world
end
milestone busy
    if ever(Served) >= 5
end
"
            .to_string(),
        ),
    ]);
    let mut w = World::new(Arc::new(Game::from_files(&files, &[]).unwrap()), 1);
    w.join("inn", "Inn");
    w
}

fn amount(w: &World, p: Option<&str>, r: &str) -> f64 {
    w.amount(p, r).unwrap().to_f64()
}

#[test]
fn credits_count_like_production_and_debits_stop_at_zero() {
    let mut w = world();
    w.credit(Some("inn"), "Coin", Num::from_f64(30.0)).unwrap();
    w.credit(Some("inn"), "Coin", Num::from_f64(-12.0)).unwrap();
    assert_eq!(amount(&w, Some("inn"), "Coin"), 18.0);
    assert_eq!(w.eval(Some("inn"), "total(Coin)").unwrap().to_f64(), 30.0);
    w.credit(Some("inn"), "Coin", Num::from_f64(-100.0)).unwrap();
    assert_eq!(amount(&w, Some("inn"), "Coin"), 0.0);
    // Caps hold, and milestones see what was credited on the next tick.
    w.credit(Some("inn"), "Served", Num::from_f64(9.0)).unwrap();
    assert_eq!(amount(&w, Some("inn"), "Served"), 5.0);
    w.run(1);
    assert_eq!(w.eval(Some("inn"), "busy").unwrap().to_f64(), 1.0);
    // World resources take no player; player resources need one.
    w.credit(None, "Pool", Num::from_f64(2.0)).unwrap();
    assert_eq!(amount(&w, None, "Pool"), 2.0);
    assert!(w.credit(None, "Coin", Num::ONE).is_err());
    assert!(w.credit(Some("inn"), "Nothing", Num::ONE).is_err());
}
