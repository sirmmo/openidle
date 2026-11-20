// Plays a game with the bot in WebAssembly (under Node) and prints the world's fingerprint,
// which must equal the native one (`openidle play GAME --minutes M --quiet`):
//
//   node tools/crosscheck.mjs target/wasm32-unknown-unknown/wasm/openidle_web.wasm games/sparks.json 10
import { readFileSync } from 'fs';
import { engine } from '../web/openidle.js';

const [wasm, bundle, minutes = '10', players = '1'] = process.argv.slice(2);
const call = await engine(readFileSync(wasm));
call({ op: 'load', bundle: JSON.parse(readFileSync(bundle, 'utf8')), seed: 1 });
const ids = Array.from({ length: +players }, (_, i) => `p${i + 1}`);
for (const id of ids) call({ op: 'join', player: id, name: `Bot ${id.slice(1)}` });
for (let s = 0; s < minutes * 60; s++) {
  for (const id of ids) call({ op: 'bot', player: id, clicks: 3 });
  call({ op: 'step', ticks: 5 });
}
console.log(call({ op: 'fingerprint' }).fingerprint);
