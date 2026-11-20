# openidle

A deterministic engine for idle and incremental games, with every rule in data files: resources,
generators, GlassBox-style rules (inputs scale outputs), upgrades, actions, milestones, prestiges,
boosts and discounts, all with expressions; big numbers far past `1e308`; worlds of one player or
many, sharing world resources, upgrades and milestones.

Every operation is exact to the bit natively and in WebAssembly, so a browser can follow a
server's world in lockstep. Dependencies: serde, serde_json, toml.

The language is described in
[SCRIPTING.md](https://github.com/sirmmo/openidle/blob/main/docs/SCRIPTING.md), shared worlds in
[MULTIPLAYER.md](https://github.com/sirmmo/openidle/blob/main/docs/MULTIPLAYER.md).

Licence: GPL-3.0-or-later.
