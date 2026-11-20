// OpenIdle in a page: the engine (WebAssembly), numbers for people, and two ways to play:
// alone (`Local`, saved in the browser, with progress for the time away) or in a shared world
// on a server (`Online`, the browser's copy kept in step with the server's).
//
//   import { engine, Local, Online, format } from './openidle.js';
//   const call = await engine('openidle.wasm');
//   const game = await (await fetch('games/sparks.json')).json();
//   const s = new Local(call, game, 'sparks');
//   s.start(); s.on(events => ...); s.view(); s.do({ cmd: 'buy', id: 'Kindler' });

/** Loads the engine; returns `call(request) -> answer` (throws on `error`). */
export async function engine(source = 'openidle.wasm') {
  const bytes = typeof source === 'string' ? await (await fetch(source)).arrayBuffer() : source;
  const { instance } = await WebAssembly.instantiate(bytes, {});
  const x = instance.exports;
  const enc = new TextEncoder();
  const dec = new TextDecoder();
  return function call(req) {
    const b = enc.encode(JSON.stringify(req));
    const p = x.oi_alloc(b.length);
    new Uint8Array(x.memory.buffer, p, b.length).set(b);
    const out = x.oi_call(p, b.length);
    const a = JSON.parse(dec.decode(new Uint8Array(x.memory.buffer, out, x.oi_out_len())));
    if (a && a.error) throw new Error(a.error);
    return a;
  };
}

/** log10 of an engine number (`"1.5e300"`, `"1.25p2000"`, `42`); -Infinity for 0. */
export function log10(n) {
  if (typeof n === 'number') return Math.log10(Math.abs(n));
  const s = String(n);
  const i = s.indexOf('p');
  if (i >= 0) return Math.log10(Math.abs(parseFloat(s.slice(0, i)))) + parseInt(s.slice(i + 1), 10) * Math.LOG10_2;
  return Math.log10(Math.abs(parseFloat(s)));
}

/** The number as a double (Infinity beyond 1.8e308). */
export function toNumber(n) {
  if (typeof n === 'number') return n;
  const s = String(n);
  const i = s.indexOf('p');
  if (i >= 0) return parseFloat(s.slice(0, i)) * Math.pow(2, parseInt(s.slice(i + 1), 10));
  return parseFloat(s);
}

const SUFFIX = ['', 'K', 'M', 'B', 'T', 'Qa', 'Qi'];

/** An engine number for people: 12.5, 1.23K, 4.56M, 7.89e42. */
export function format(n, digits = 2) {
  const s = String(n);
  const neg = s.startsWith('-');
  const l = log10(n);
  if (!isFinite(l)) return '0';
  let out;
  if (l < 3) {
    const x = Math.abs(toNumber(n));
    out = x >= 100 || Number.isInteger(x) ? String(Math.floor(x)) : x.toFixed(x < 10 ? digits : 1).replace(/\.?0+$/, '');
  } else if (l < 21) {
    const k = Math.floor(l / 3);
    out = Math.pow(10, l - 3 * k).toFixed(digits) + SUFFIX[k];
  } else {
    const e = Math.floor(l);
    let m = Math.pow(10, l - e);
    if (m >= 9.995) return (neg ? '-' : '') + '1.00e' + (e + 1);
    out = m.toFixed(digits) + 'e' + e;
  }
  return (neg ? '-' : '') + out;
}

/** Seconds for people: 42s, 3m 05s, 2h 10m. */
export function duration(s) {
  s = Math.max(0, Math.round(s));
  if (s < 60) return s + 's';
  if (s < 3600) return Math.floor(s / 60) + 'm ' + String(s % 60).padStart(2, '0') + 's';
  return Math.floor(s / 3600) + 'h ' + String(Math.floor(s / 60) % 60).padStart(2, '0') + 'm';
}

function randomToken() {
  const a = new Uint8Array(24);
  crypto.getRandomValues(a);
  return Array.from(a, (b) => 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789'[b % 62]).join('');
}

function store(key, value) {
  try {
    if (value === undefined) return JSON.parse(localStorage.getItem(key) || 'null');
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, JSON.stringify(value));
  } catch (e) {
    return null;
  }
}

class Session {
  constructor(call) {
    this.call = call;
    this.listeners = [];
    this.player = null;
  }
  on(f) {
    this.listeners.push(f);
  }
  emit(events, extra) {
    if ((events && events.length) || extra) for (const f of this.listeners) f(events || [], extra);
  }
  view() {
    return this.player ? this.call({ op: 'view', player: this.player }) : null;
  }
}

/** One player, in this browser. Saves itself every ten seconds and when the page is hidden. */
export class Local extends Session {
  constructor(call, bundle, key) {
    super(call);
    this.bundle = bundle;
    this.key = 'openidle:' + key;
    this.player = 'me';
  }

  /** Starts (or resumes, running the time away); returns `{away, gains}` when there was any. */
  start(name = 'You') {
    const saved = store(this.key);
    let away = null;
    if (saved && saved.save) {
      const info = this.call({ op: 'load', bundle: this.bundle, save: saved.save });
      this.tickSeconds = info.tick_seconds;
      const before = this.amounts();
      const seconds = (Date.now() - saved.at) / 1000;
      if (seconds > 1) {
        const r = this.call({ op: 'offline', seconds });
        const after = this.amounts();
        const gains = Object.keys(after)
          .filter((k) => log10(after[k]) > log10(before[k] ?? 0) && toNumber(after[k]) > toNumber(before[k] ?? 0))
          .map((k) => ({ id: k, from: before[k] ?? '0', to: after[k] }));
        away = { seconds, ran: r.seconds, gains };
        this.emit(r.events);
      }
    } else {
      const info = this.call({ op: 'load', bundle: this.bundle, seed: Math.floor(Math.random() * 2 ** 31) });
      this.tickSeconds = info.tick_seconds;
      this.emit(this.call({ op: 'join', player: this.player, name }).events);
    }
    this.last = performance.now();
    this.carry = 0;
    this.timer = setInterval(() => this.frame(), 100);
    this.saver = setInterval(() => this.save(), 10000);
    addEventListener('pagehide', () => this.save());
    document.addEventListener('visibilitychange', () => document.hidden && this.save());
    return away;
  }

  amounts() {
    const v = this.view();
    const out = {};
    for (const r of v.resources) if (r.visible) out[r.id] = r.amount;
    return out;
  }

  frame() {
    const now = performance.now();
    this.carry += (now - this.last) / 1000;
    this.last = now;
    const ticks = Math.floor(this.carry / this.tickSeconds);
    if (ticks > 0) {
      this.carry -= ticks * this.tickSeconds;
      const r = this.call({ op: 'step', ticks: Math.min(ticks, 100000) });
      this.emit(r.events, { tick: r.tick });
    }
  }

  do(cmd) {
    const r = this.call({ op: 'apply', player: this.player, cmd });
    this.emit(r.events, { tick: r.tick });
    return r;
  }

  bot(clicks = 3) {
    const r = this.call({ op: 'bot', player: this.player, clicks });
    this.emit(r.events, { tick: r.tick });
  }

  save() {
    store(this.key, { at: Date.now(), save: this.call({ op: 'save' }) });
  }

  exportSave() {
    return btoa(unescape(encodeURIComponent(JSON.stringify({ at: Date.now(), save: this.call({ op: 'save' }) }))));
  }

  importSave(text) {
    const s = JSON.parse(decodeURIComponent(escape(atob(text.trim()))));
    this.call({ op: 'load', bundle: this.bundle, save: s.save });
    store(this.key, s);
  }

  reset() {
    clearInterval(this.saver);
    store(this.key, null);
  }
}

/**
 * A shared world on a server. The browser keeps its own copy of the world and runs it to the
 * ticks and commands the server sends; commands go to the server, which carries them out and
 * sends them back to everyone. `onChat`, `onOnline`, `onStatus` are callbacks the page sets.
 */
export class Online extends Session {
  constructor(call, url, game, world, name) {
    super(call);
    this.url = url;
    this.game = game;
    this.world = world;
    this.name = name;
    this.tokenKey = 'openidle:token:' + url + ':' + game + ':' + world;
    this.token = store(this.tokenKey) || randomToken();
    store(this.tokenKey, this.token);
    this.loaded = false;
    this.retry = 1000;
    this.onChat = () => {};
    this.onOnline = () => {};
    this.onStatus = () => {};
    this.onError = () => {};
  }

  start() {
    this.connect();
  }

  connect() {
    this.onStatus('connecting');
    const ws = (this.ws = new WebSocket(this.url));
    ws.onopen = () => {
      this.retry = 1000;
      ws.send(JSON.stringify({ t: 'hello', game: this.game, world: this.world, token: this.token, name: this.name }));
    };
    ws.onmessage = (e) => this.message(JSON.parse(e.data));
    ws.onclose = () => {
      this.onStatus('offline');
      setTimeout(() => this.connect(), this.retry);
      this.retry = Math.min(this.retry * 2, 30000);
    };
  }

  message(m) {
    switch (m.t) {
      case 'welcome':
        this.bundle = m.bundle;
        this.player = m.player;
        this.call({ op: 'load', bundle: m.bundle, save: m.save });
        this.loaded = true;
        this.onStatus('online');
        for (const c of m.chat || []) this.onChat(c);
        this.emit([], { tick: m.tick, welcome: true });
        break;
      case 'state':
        this.call({ op: 'load', bundle: this.bundle, save: m.save });
        this.emit([], { tick: m.tick });
        break;
      case 'tick':
        if (this.loaded) this.emit(this.call({ op: 'to', tick: m.tick }).events, { tick: m.tick });
        break;
      case 'cmd':
        if (!this.loaded) break;
        try {
          const ev = this.call({ op: 'to', tick: m.tick }).events;
          const r = this.call({ op: 'apply', player: m.player, cmd: m.cmd });
          this.emit(ev.concat(r.events), { tick: m.tick, cmd: m });
        } catch (e) {
          this.ws.send(JSON.stringify({ t: 'resync' }));
        }
        break;
      case 'check':
        if (this.loaded) {
          this.call({ op: 'to', tick: m.tick });
          const f = this.call({ op: 'fingerprint' });
          if (f.tick === m.tick && f.fingerprint !== m.fp) this.ws.send(JSON.stringify({ t: 'resync' }));
        }
        break;
      case 'online':
        this.onOnline(m.players);
        break;
      case 'chat':
        this.onChat(m);
        break;
      case 'err':
        this.onError(m.msg);
        break;
    }
  }

  send(m) {
    if (this.ws && this.ws.readyState === 1) this.ws.send(JSON.stringify(m));
  }

  do(cmd) {
    this.send({ t: 'cmd', cmd });
  }

  chat(text) {
    this.send({ t: 'chat', text });
  }

  rename(name) {
    this.send({ t: 'name', name });
  }
}
