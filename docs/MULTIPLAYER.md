# Shared worlds

An idle game alone is a world with one player. On a server, a world has many: each has their
own resources, generators and upgrades, and they share what the game declares with
`scope world`: resources (a commons everyone feeds or draws on), upgrades (bought once, by
anyone, for everyone), milestones (reached once by the whole world), and rules that run on the
world itself. Expressions read the others too: `players`, `sum(Kindler)`, `best(Stars)`,
`contrib(Beacon)`. Players can give each other `giftable` resources.

The demo's beacon is the smallest example: every kindler in the world feeds it, it fades, and
once it has held 100 everyone's sparks are faster; anyone can spend it on the Lighthouse, which
doubles everything for everyone.

## How it stays in step

The server runs every world on its clock, whether anyone is connected or not: it is an idle
game. Browsers do not receive the world over and over. They run it themselves:

1. A browser says `hello` with a game, a world and a secret token, and gets `welcome`: its
   player id, the game's files and the whole world.
2. The server sends `tick` as its clock moves (a few times a second); the browser runs its copy
   up to that tick.
3. A command goes to the server, which carries it out at its current tick and sends it to
   everyone, the sender included, as `cmd` with that tick. Each browser runs up to the tick, then
   carries out the command. A command the server refuses comes back as `err` to its sender only.
4. Every ten seconds the server sends `check`, the world's fingerprint; a browser whose copy
   differs asks for the world again (`resync`).

This works because the engine is deterministic: it uses only IEEE-754 operations that are exact
to the bit everywhere (its logarithms and powers are its own, not the platform's), its dice are
in the world, and players take turns in a fixed order. `tools/crosscheck.mjs` plays the same game
natively and in WebAssembly and compares the fingerprints; CI runs it.

So every browser shows the whole world as it is, live, for the price of a few small messages a
second; the server does the same work as one browser per world. Cheating is not possible:
the server's copy is the world, and it refuses what its rules refuse.

## Messages

JSON objects with a type `t`, over a WebSocket at `/ws`.

From a browser:

| `t` | Fields | |
|---|---|---|
| `hello` | `game`, `world`, `token`, `name` | enter a world; made if it does not exist. `token`: 8 to 64 letters and digits, the browser's secret; the same token is the same player |
| `cmd` | `cmd` | a command (see SCRIPTING.md) |
| `name` | `name` | change name |
| `chat` | `text` | to everyone in the world |
| `resync` | | send me the world again |

From the server:

| `t` | Fields | |
|---|---|---|
| `welcome` | `player`, `game`, `world`, `bundle`, `save`, `tick`, `chat` | who you are, the game's files, the world, the last chat |
| `tick` | `tick` | run your copy up to here |
| `cmd` | `tick`, `player`, `cmd` | run up to `tick`, then carry out the command |
| `check` | `tick`, `fp` | the world's fingerprint at that tick |
| `state` | `tick`, `save` | the world, after `resync` |
| `online` | `players` | who is connected |
| `chat` | `player`, `name`, `text` | |
| `err` | `msg` | |

`web/openidle.js` (`Online`) speaks this for a page.

## Running a server

```sh
cargo build --release -p openidle-server
OI_SITE=web ./target/release/openidle-server     # then http://localhost:8080/?world=main
```

| Variable | Default | |
|---|---|---|
| `OI_ADDR` | `0.0.0.0:8080` | where to listen |
| `OI_GAMES` | `games` | a folder with one folder per game |
| `OI_DATA` | `data` | where worlds are saved (empty: memory only) |
| `OI_SITE` | | a folder to serve as the site, such as `web/` |
| `OI_ORIGINS` | `*` | pages allowed to connect, comma-separated |
| `OI_MAX_WORLDS`, `OI_MAX_PLAYERS` | 100, 200 | limits |

Plain HTTP on the same port: `/health`, `/stats.json` (games, connections, worlds),
`/games/<game>.json` (a game's bundle).

Worlds are saved every thirty seconds and when the server stops (SIGTERM, Ctrl-C). On start the
server loads them and runs each for the time it was away, up to its game's `offline_max`. Every
loaded world runs all the time, so a server's work grows with its worlds and their players;
`OI_MAX_WORLDS` keeps it bounded.

The server in `crates/openidle-server` is one way to host worlds. The engine (`openidle`)
does not know about networks: another host (a Cloudflare Durable Object, a game backend) can
run `World::step` on its clock, `World::apply` on commands and pass them on the same way.
