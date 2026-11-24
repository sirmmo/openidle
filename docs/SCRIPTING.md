# Writing a game

A game is a folder:

```text
games/sparks/
├── game.toml       its name, its scripts, the length of a tick
├── params.toml     its numbers, used as $name everywhere
├── hearth.idle     blocks: resources, generators, rules, upgrades, ...
├── ...             more blocks (any number of files, read in the order game.toml lists them)
└── world.idle
```

`openidle check games/sparks` reads a game and lists what it declares; errors give the file and
the line. `openidle play games/sparks --hours 2` plays it with a bot, faster than real time, and
prints when each thing unlocked, each milestone, upgrade and prestige: the quickest way to tune a
game's pace. `openidle bundle games/sparks` writes it as one JSON file for the browser.

## game.toml

```toml
name = "Sparks"
description = "Strike sparks, build kindlers ..."
scripts = ["hearth.idle", "iron.idle", "light.idle", "stars.idle", "gems.idle", "goals.idle", "world.idle"]
tick = 0.2            # seconds a tick lasts (default 0.2)
offline_max = 28800   # most seconds of progress for time away (default 86400)
score = "ever(Sparks)"  # an expression that ranks players (optional)
```

## Parameters

`params.toml` is a flat list of numbers, strings and booleans:

```toml
growth = 1.15
ascend_at = 1e5
```

`$growth` is replaced by its value in `game.toml` and in the scripts before they are read, so a
parameter can stand anywhere. `$$` is a dollar sign; `$name` in a `#` comment is left alone.
`openidle play --set growth=1.2` (and `Game::from_dir(dir, &[("growth", "1.2")])`) overrides one;
a name the game does not declare is an error.

## Blocks

Scripts hold blocks, GlassBox-style: a kind and an id, lines of a key and the rest, `end`.

```text
generator Kindler          # kind and id (letters, digits, _)
    text Makes sparks.     # a line: a key and the rest
    cost Sparks 10 * 1.15 ^ owned
end
```

`#` starts a comment (at the start of a line or after a space). Ids are shared by every kind,
so a name means one thing everywhere. The kinds are `resource`, `generator`, `rule` (also
`unitRule`), `upgrade`, `action`, `milestone` and `prestige`.

Every kind but `rule` takes:

| Line | |
|---|---|
| `label <text>` | the name players see (default: the id, `_` as spaces) |
| `text <text>` | a description (several `text` lines are joined) |
| `show if <condition>` | hidden until the condition is true for the player, then shown for good; without it, shown from the start |
| `view <key> <value>` | anything for the page that draws the game (a colour, an icon); the engine passes it on. The player in `web/play/` reads `view color #hex`; on milestones `view kind level` and `view tier N`, the game's levels (several in one tier are branches, shown side by side, the next ones' `text` as goals); on upgrades `view kind path`, a choice of one among several |
| `boost <target> <factor>` | multiplies production (below) |
| `discount <target> <factor>` | multiplies costs (below) |

Things a player cannot see cannot be bought, used or taken.

### resource

```text
resource Embers
    scope player           # player (default): each player has their own; world: one for all
    start 0                # amount at the start (and after a prestige that resets it)
    cap 50 * 1.6 ^ Brazier # most it can hold (an expression: upgrades can raise it)
    giftable               # players can give it to each other
    hidden                 # never shown (a counter for the rules)
end
```

### generator

What players buy many of. Its count is its value in expressions.

```text
generator Kindler
    cost Sparks 10 * 1.15 ^ owned   # several cost lines: all are paid
    max 100                         # most a player can have
    start 0
end
```

`owned` is how many the player has, so `10 * 1.15 ^ owned` is the usual growing price. Buying
many at once prices each one in turn.

### rule

GlassBox's unit rule: what generators (or the player, or the world) take in and put out, every
tick, in amounts per second.

```text
rule forge
    appliesTo Forge Smelter        # once for each of these the player has (none: once per player)
    if Embers < cap(Embers)        # tests: all must be true for the rule to run (also `test`)
    in Sparks 10                   # per second, per generator
    out Embers 1
end

rule fade
    scope world                    # once per tick for the whole world (world resources only)
    in Beacon 0.002 * Beacon
end
```

* **Inputs scale the rule**: with half of what it needs, a rule takes that half and gives half.
* A rule with inputs also slows down when a capped output is full, so nothing is wasted. Rules
  without inputs just stop at the cap.
* Outputs into a `world` resource count as the player's contribution (`contrib(R)`).
* Rules run in the order they are declared, world rules first; players take turns being first
  each tick, so nobody always comes first to a shared resource.

### upgrade

Bought once.

```text
upgrade Leather
    label Leather bellows
    cost Sparks 2000
    if Bellows >= 5                # needed to buy it (also `need`), on top of `show if`
    boost Bellows 2
    do add Gems 1                  # effects when bought
    auto                           # bought by itself as soon as it can be
    scope world                    # one for the whole world: anyone buys it, everyone has it
end
```

A `world` upgrade's costs in world resources come from the shared pool; costs in player
resources are paid by the player who buys it. An `auto` world upgrade costs only world resources.

### action

What players click.

```text
action strike
    in Energy 1                    # costs, paid in full (also `cost`)
    out Sparks 1 + Flint           # gains (also `gain`), multiplied by boosts
    if Sparks < 1e6                # needed to use it
    cooldown 60                    # seconds before it can be used again
    lasts 10                       # its boosts last this long after each use
    boost all 3
    do add Gems 1
end
```

### milestone

Reached once, when its conditions are true; its boosts hold from then on.

```text
milestone bonfire
    label A million sparks
    if ever(Sparks) >= 1e6         # all must be true
    boost all 1.25
    do add Gems 5
    scope world                    # reached once for the whole world (`do` on world resources only)
end
```

### prestige

Starts the player's run again for something that stays.

```text
prestige ascend
    gain Stars floor(sqrt(total(Sparks) / 1e5))   # several gain lines allowed
    if gain >= 1                   # needed (default: the first gain is more than 0)
    keep Gems Flint                # kept through the reset
    reset kindlers                 # also reset (milestones are kept unless listed)
    do add Gems gain               # effects after the reset; `gain` is the first gain
end
```

A prestige resets every player resource to its `start` (and its `total` to 0), every
generator to its `start` and every player upgrade, except what `keep` lists and what it gains.
Milestones, `ever` amounts, contributions and what the player has seen are kept. Its count is its
value in expressions (how many times the player took it).

## Levels and branches

Levels are milestones, so they are reached once and kept. A branch is a choice of upgrades that
rule each other out; what a prestige resets can be chosen again, so each run can take another
path. The demo's level 5:

```text
upgrade Clockwork
    cost Light 2000
    if not Alchemy and not Glassworks   # one path a run: ascending resets upgrades
    view kind path
end

milestone L5a
    label Level 5 · Clockwork
    text Take the path of clockwork and make 1000 gears.
    if ever(Gears) >= 1000
    view kind level
    view tier 5                         # L5b and L5c are tier 5 too: three branches
end
```

A milestone such as `if L5a and L5b and L5c` rewards walking them all. To keep a choice longer,
list it in a prestige's `keep` (the demo's skies last until the next supernova).

## Boosts and discounts

`boost <target> <factor>` multiplies production while its source is in force:

| Source | In force |
|---|---|
| resource, generator | always (`boost all 1 + 0.1 * Stars`: each star +10%) |
| upgrade | once bought |
| milestone | once reached |
| action | for `lasts` seconds after each use |
| prestige | once taken at least once |

| Target | Multiplies |
|---|---|
| a resource | every rule's and action's output of it |
| a generator | the outputs of rules that apply to it |
| a rule or an action | its outputs |
| `actions` | every action's gains |
| `all` | everything |

Boosts multiply each other. The factor is an expression, worked out again every tick, so
`boost Kindler 1 + 0.01 * Kindler` grows with the count. `discount <generator|upgrade|all>
<factor>` multiplies costs the same way (`discount all 0.9`: 10% off).

## Expressions

Every amount, cost, cap, condition and factor is an expression:

```text
10 * 1.15 ^ owned
floor(sqrt(total(Sparks) / 1e5))
Kindler >= 10 and not Forge > 0
if(Stars > 10, 2, 1)
```

* Numbers: `12`, `1.5`, `1e6`, `1_000`, `1e500` (numbers go far beyond `1e308`).
* Operators, loosest first: `or`, `and`, `not`, comparisons (`<`, `<=`, `>`, `>=`, `==`, `!=`, and
  GlassBox's `less`, `atMost`, `greater`, `atLeast`, `is`), `+ -`, `* /`, unary `-`, `^` (right to
  left; `-2 ^ 2` is -4). Comparisons and logic give 1 or 0; any non-zero value is true.
* Dividing by zero gives zero.

| Name | Value |
|---|---|
| a resource | the player's amount (the world's, for a world resource) |
| a generator | how many the player has |
| an upgrade, a milestone | 1 when bought or reached (by the player, or by the world for world ones) |
| an action | 1 while its boosts are in force |
| a prestige | how many times the player took it |
| `total(R)` | made this run (since the last prestige that reset it) |
| `ever(R)` | made since the player joined |
| `rate(R)` | net change per second on the last tick |
| `cap(R)` | the cap (0 without one) |
| `contrib(R)` | what the player put into a world resource |
| `sum(X)`, `best(X)` | the sum and the largest of X over every player in the world |
| `players` | how many players the world has |
| `time`, `runtime` | seconds since the player joined, since their last prestige |
| `owned` | in a generator's costs: how many the player has |
| `gain` | in a prestige's `if` and `do`: its first gain |

Functions: `floor`, `ceil`, `round`, `sqrt`, `abs`, `log10`, `ln`, `log(x, base)`,
`min(a, b, ...)`, `max(a, b, ...)`, `clamp(x, lo, hi)`, `if(c, a, b)`, and `chance(p)`: 1 with
probability p, from the world's own dice (so it is the same everywhere).

## Commands

What players do, as JSON (`openidle::Command`):

```json
{ "cmd": "buy", "id": "Kindler" }               // one
{ "cmd": "buy", "id": "Kindler", "n": 10 }
{ "cmd": "buy", "id": "Kindler", "max": true }  // as many as can be paid for
{ "cmd": "buy", "id": "Leather" }               // an upgrade
{ "cmd": "act", "id": "strike" }
{ "cmd": "prestige", "id": "ascend" }
{ "cmd": "gift", "to": "p2", "res": "Gems", "amount": "1" }
{ "cmd": "join", "name": "Ada" }
```

## Views and events

`World::view(player)` is everything a page needs to draw the game for one player: every
resource (amount, rate, cap, totals), generator (count, next price, how many can be afforded,
what each makes), upgrade, action (gains, cooldown), milestone, prestige (what it would give) and
player (name, score), each with its `label`, `text`, `view` and `visible`. Numbers are strings in
the engine's exact form: the shortest exact decimal within the range of a double (`"1.5e300"`),
beyond it `"<m>p<e>"`, m × 2^e (`"1.25p2000"`). `web/openidle.js` formats both.

`World::take_events()` says what happened: `join`, `unlock` (something became visible),
`milestone`, `upgrade`, `prestige` (with what it gave), `gift`.
