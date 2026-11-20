//! OpenIdle: a deterministic engine for idle and incremental games, with every rule in data
//! files, in the spirit of GlassBox.
//!
//! A game is a folder: `game.toml` (its name, its scripts, the length of a tick), `params.toml`
//! (numbers used as `$name` everywhere) and scripts of blocks: resources, generators, rules,
//! upgrades, actions, milestones and prestiges. [`Game`] reads and checks them; a [`World`] runs
//! them for one player or many. The same world, ticks and commands give the same result to the
//! bit, natively and in WebAssembly, so a browser can play alone or follow a server's world in
//! lockstep.
//!
//! ```
//! use std::collections::BTreeMap;
//! use std::sync::Arc;
//! use openidle::{Command, Game, World};
//!
//! let files = BTreeMap::from([
//!     ("game.toml".to_string(), "name = \"Tiny\"\nscripts = [\"tiny.idle\"]\ntick = 1\n".to_string()),
//!     ("tiny.idle".to_string(), "
//! resource Gold
//!     start 10
//! end
//! generator Miner
//!     cost Gold 10 * 1.15 ^ owned
//! end
//! rule mine
//!     appliesTo Miner
//!     out Gold 1
//! end
//! ".to_string()),
//! ]);
//! let game = Arc::new(Game::from_files(&files, &[]).unwrap());
//! let mut world = World::new(game, 1);
//! world.join("p1", "Ada");
//! world.apply("p1", &Command::Buy { id: "Miner".into(), n: None, max: false }).unwrap();
//! world.run(5);
//! assert_eq!(world.amount(Some("p1"), "Gold").unwrap().to_f64(), 5.0);
//! ```

pub mod bot;
pub mod expr;
pub mod game;
pub mod num;
pub mod script;
pub mod world;

pub use game::{Game, Scope};
pub use num::Num;
pub use script::Error;
pub use world::{Command, Save, World};
