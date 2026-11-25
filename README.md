# OpenIdle

An open-source engine for idle and incremental games, in Rust. Every rule of a game is in data
files, in a small block language in the spirit of GlassBox (the rule language of SimCity 2013,
as in [Lilienstern](https://github.com/sirmmo/lilienstern)): resources, generators that make
them, rules that turn some into others, upgrades, clickable actions, milestones and prestiges,
with every number an expression and every expression a parameter away from a change.

The same engine runs **in the browser** (WebAssembly, about 600 KB, no generated glue) for a
game played alone and saved in the browser, with progress for the time away, and **on a
server**, where a **world** holds many players: each has their own economy, and the game decides
what they share (a commons everyone feeds, upgrades anyone buys for everyone, milestones the
whole world reaches, gifts). The server runs every world on its clock, day and night; browsers
follow it in lockstep, so everyone sees the whole world live.

- **Rules are data**: `game.toml`, `params.toml` and `.idle` scripts. See
  [docs/SCRIPTING.md](docs/SCRIPTING.md).
- **Deterministic to the bit**, natively and in WebAssembly: its own big numbers (far past
  `1e308`) and its own logarithms, only IEEE operations that are exact everywhere, the dice in the
  world. The same world, ticks and commands give the same fingerprint anywhere.
- **Shared worlds**: an authoritative server that keeps worlds running and saved, and browsers
  that run the same engine on its ticks and commands. See [docs/MULTIPLAYER.md](docs/MULTIPLAYER.md).
- **A bot** that plays a game faster than real time and says when everything happened: tune a
  game's pace in seconds (`openidle play`), or let it autoplay in the browser.

**Play the demo** at <https://ingmmo.com/openidle/play/>, alone in your browser. These pages, as a
site: <https://ingmmo.com/openidle/>.

## Quick start

```sh
./build.sh                                   # tests, WebAssembly, bundles, command, server
./target/release/openidle check games/sparks
./target/release/openidle play games/sparks --hours 2 --every 600
./target/release/openidle play games/sparks --minutes 30 --players 4 --set growth=1.2

python3 -m http.server -d web 8000                     # alone: http://localhost:8000/play/
OI_SITE=web ./target/release/openidle-server           # together: http://localhost:8080/play/?world=main
```

`play` prints the bot's game: when each thing unlocked, each milestone, upgrade and prestige,
and every `--every` seconds what it has and makes:

```text
0:01:33 milestone Level 2 · The forge
0:02:16 milestone The beacon is lit
0:04:52 milestone Level 3 · The Iron Age
0:06:54 milestone Level 4 · The Age of Light
0:11:23 milestone Level 5 · Clockwork
0:22:39 prestige  Ascend {"Stars":"1"}
0:30:00 == Sparks 1.03B (836.94M/s), Embers 186.08K (71.44K/s), Iron 15.38K (6.59K/s), ... | 127 Kindler, 98 Bellows, ...
2:07:53 prestige  Supernova {"Novae":"1"}
6:02:12 prestige  Big Bang {"Universes":"1"}
```

## A game

```text
resource Sparks
end

generator Kindler
    text Makes sparks.
    cost Sparks 10 * $growth ^ owned
end

rule kindle                  # GlassBox's unit rule: per second, per kindler
    appliesTo Kindler
    out Sparks 1
end

upgrade DryWood
    label Dry wood
    cost Sparks 200
    boost Kindler 2
    show if Kindler >= 5
end

prestige ascend
    gain Stars floor(sqrt(total(Sparks) / 1e5))
    keep Gems
end

resource Beacon              # one for the whole world
    scope world
end

rule feed                    # every player's kindlers feed it
    appliesTo Kindler
    out Beacon 0.01
end
```

`games/sparks` is the demo: every feature of the engine in one game. It has eleven levels,
from a hearth to a multiverse, over about a day of play. Two of them branch: at level 5 you
choose Clockwork, Alchemy or Glassworks, and at level 7 a comet or a nebula. Ascending lets you
choose again. Three prestiges are stacked: Ascend, Supernova and the Big Bang.

## Layout

| | |
|---|---|
| `crates/openidle` | The engine: numbers, expressions, scripts, games, worlds, the bot. Depends only on serde and toml. |
| `crates/openidle-web` | The engine as WebAssembly, driven by JSON messages (`oi_alloc`, `oi_call`, `oi_out_len`). |
| `crates/openidle-server` | Shared worlds: WebSocket at `/ws`, saved worlds, a static site. |
| `crates/openidle-cli` | The `openidle` command: `check`, `play`, `bundle`. |
| `web/` | The site: these docs (`index.html`), `openidle.js` (the engine for a page: `Local` and `Online` sessions, number formatting) and, in `play/`, a plain player for any game. |
| `games/` | Games. |
| `tools/crosscheck.mjs` | Plays a game in WebAssembly under Node; its fingerprint must equal the native one. |

Using the engine from Rust:

```rust
let game = Arc::new(Game::from_dir(Path::new("games/sparks"), &[])?);
let mut world = World::new(game, seed);
world.join("p1", "Ada");
world.apply("p1", &Command::Buy { id: "Kindler".into(), n: None, max: false })?;
world.run(50);                      // ten seconds at 0.2 s a tick
let view = world.view("p1")?;       // JSON for a page
let save = world.save();            // serde: by ids, so saves survive changes to the game
```

## Licence

GPL-3.0-or-later.

## Publishing on Wavedash

Each game can be its own game on [Wavedash](https://wavedash.com)
([docs](https://docs.wavedash.com)), configured in `wavedash/<game>/wavedash.toml`; today
`wavedash/sparks/` publishes the demo. Another game gets there with a copy of that folder renamed
after it, its id, and its name added to the workflow's `options` and `env`.

1. **Create the game** in the Wavedash Developer Portal, or with the CLI (`curl -fsSL
   https://wavedash.com/cli/install.sh | sh`, `wavedash auth login`, then `wavedash project create
   --title "Sparks" --team-id <TEAM_ID>`).
2. **Set its id**: `game_id` in its `wavedash.toml`, or, to keep it out of the repository,
   `WAVEDASH_GAME_ID_<GAME>` in the environment and as a repository variable (Settings › Secrets and
   variables › Actions › Variables), e.g. `WAVEDASH_GAME_ID_SPARKS`.
3. **Upload a build**: `sh tools/wavedash.sh [game]` builds the engine and the bundles as the Pages
   workflow does, copies the player into `wavedash/<game>/build/` (at its root, opening that game
   without `?game=`; alone, not online) and, with the CLI on `PATH` and `WAVEDASH_TOKEN` set, runs
   `wavedash build push`. Without the CLI (or with `WAVEDASH_DRY_RUN=1`) it only says what it would
   push. On GitHub: Actions › Wavedash › Run workflow, with the secret `WAVEDASH_TOKEN`.
4. **Publish it**: `wavedash publish <BUILD_ID>` (the id `build push` printed), or in the Developer
   Portal.

On Wavedash the game stays behind Wavedash's loading screen until it calls `Wavedash.init()`:
`web/play/play.js` does it through `web/play/wavedash.js` once the game is drawn, or when it could
not start, to show why. Elsewhere (GitHub Pages, a local server) there is no `Wavedash` object and
the calls do nothing.
