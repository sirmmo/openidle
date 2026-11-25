// The browser player: draws any OpenIdle game from the engine's view, alone or in a shared
// world. It is plain on purpose; a game can bring its own page and use openidle.js the same way.
//
//   play/?game=sparks                          alone, saved in this browser
//   play/?game=sparks&world=main               in a world on the server this page came from
//   play/?game=sparks&world=main&server=wss://example.org/ws
//
// The engine (../openidle.js, ../openidle.wasm) and the games (../games/) sit one level up,
// where any other page can use them too.
import { engine, Local, Online, format, duration, log10 } from '../openidle.js';

const q = new URLSearchParams(location.search);
const gameName = q.get('game') || 'sparks';
const worldName = q.get('world');
const online = !!(worldName || q.get('server'));
const $ = (id) => document.getElementById(id);

let session;
let view = null;
let autoplay = null;
let showBought = false;
const onlinePlayers = new Set();

function esc(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
}

function toast(text) {
  const t = document.createElement('div');
  t.className = 'toast';
  t.textContent = text;
  $('toasts').appendChild(t);
  setTimeout(() => t.remove(), 4000);
  while ($('toasts').children.length > 5) $('toasts').firstChild.remove();
}

function dialog(html, ok = 'OK', cancel = null) {
  return new Promise((resolve) => {
    $('dialogBody').innerHTML = html;
    const d = $('dialog');
    const row = $('dialogOk').parentNode;
    row.innerHTML = '';
    if (cancel) {
      const c = document.createElement('button');
      c.className = 'btn';
      c.textContent = cancel;
      c.onclick = () => { d.close(); resolve(false); };
      row.appendChild(c);
    }
    const b = document.createElement('button');
    b.className = 'btn primary';
    b.id = 'dialogOk';
    b.textContent = ok;
    b.onclick = () => { d.close(); resolve(true); };
    row.appendChild(b);
    d.showModal();
  });
}

function label(id) {
  const r = view && view.resources.find((x) => x.id === id);
  return r ? r.label : id;
}

function amountOf(id) {
  const r = view.resources.find((x) => x.id === id);
  return r ? r.amount : '0';
}

function costHtml(list) {
  return list
    .map((c) => {
      const short = log10(amountOf(c.res)) < log10(c.amount) - 1e-12;
      return `<span class="${short ? 'short' : ''}">${format(c.amount)} ${esc(label(c.res))}</span>`;
    })
    .join(', ');
}

/** Keeps one element per item in `box`, made by `make` and refreshed by `update`. */
function keyed(box, items, make, update) {
  box._els ??= new Map();
  const keep = new Set();
  let prev = null;
  for (const it of items) {
    let el = box._els.get(it.id);
    if (!el) {
      el = make(it);
      box._els.set(it.id, el);
    }
    keep.add(it.id);
    update(el, it);
    const next = prev ? prev.nextSibling : box.firstChild;
    if (next !== el) box.insertBefore(el, next);
    prev = el;
  }
  for (const [id, el] of box._els) if (!keep.has(id)) { el.remove(); box._els.delete(id); }
}

function act(cmd) {
  try {
    session.do(cmd);
  } catch (e) {
    toast(e.message);
  }
  render();
}

function render() {
  view = session.view();
  if (!view) return;
  document.title = view.game;
  $('title').textContent = view.game;
  $('who').textContent = online ? `${view.player.name} in “${worldName || 'main'}”` : `${duration(view.player.time)} played`;

  keyed($('resources'), view.resources.filter((r) => r.visible), () => {
    const d = document.createElement('div');
    d.className = 'res';
    d.innerHTML = '<div class="l"></div><div class="n"></div><div class="r"></div>';
    return d;
  }, (d, r) => {
    if (r.view.color) d.style.setProperty('--c', r.view.color);
    d.title = r.text;
    d.querySelector('.l').innerHTML = esc(r.label) + (r.scope === 'world' ? '<span class="world">shared</span>' : '');
    d.querySelector('.n').textContent = format(r.amount) + (r.cap != null ? ' / ' + format(r.cap) : '');
    const rate = d.querySelector('.r');
    const neg = String(r.rate).startsWith('-');
    rate.className = 'r' + (neg ? ' neg' : '');
    rate.textContent = log10(r.rate) > -6 ? (neg ? '' : '+') + format(r.rate) + '/s' : '';
  });

  keyed($('actions'), view.actions.filter((a) => a.visible), (a) => {
    const b = document.createElement('button');
    b.className = 'btn act';
    b.innerHTML = '<span class="label"></span><small></small><div class="bar"></div>';
    b.onclick = () => act({ cmd: 'act', id: a.id });
    return b;
  }, (b, a) => {
    b.disabled = !a.can;
    b.title = a.text;
    b.querySelector('.label').textContent = a.label;
    const parts = [];
    if (a.gain.length) parts.push('+' + a.gain.map((g) => format(g.amount) + ' ' + label(g.res)).join(', '));
    if (a.cost.length) parts.push('costs ' + a.cost.map((g) => format(g.amount) + ' ' + label(g.res)).join(', '));
    if (a.active_for > 0) parts.push('active ' + duration(a.active_for));
    else if (a.ready_in > 0) parts.push('ready in ' + duration(a.ready_in));
    else if (!a.gain.length && a.text) parts.push(a.text);
    b.querySelector('small').textContent = parts.join(' · ');
    b.classList.toggle('active', a.active_for > 0);
    b.querySelector('.bar').style.width = a.cooldown > 0 ? (100 * (1 - a.ready_in / a.cooldown)) + '%' : '0';
  });

  const prest = view.prestiges.filter((p) => p.visible);
  $('prestigeBox').hidden = !prest.length;
  keyed($('prestiges'), prest, (p) => {
    const d = document.createElement('div');
    d.className = 'card prest';
    d.innerHTML = '<div class="t"><span class="name"></span><span class="badge"></span></div><div class="d"></div><div class="cost"></div><div class="row"><button class="btn primary"></button></div>';
    d.querySelector('button').onclick = async () => {
      const ok = await dialog(`<h3>${esc(p.label)}?</h3><p>${esc(p.text)}</p>`, p.label, 'Not yet');
      if (ok) act({ cmd: 'prestige', id: p.id });
    };
    return d;
  }, (d, p) => {
    d.querySelector('.name').textContent = p.label;
    d.querySelector('.badge').textContent = p.count ? `${p.count}×` : '';
    d.querySelector('.d').textContent = p.text;
    d.querySelector('.cost').innerHTML = 'Gives ' + p.gain.map((g) => `${format(g.amount)} ${esc(label(g.res))}`).join(', ');
    const b = d.querySelector('button');
    b.textContent = p.label;
    b.disabled = !p.can;
  });

  keyed($('generators'), view.generators.filter((g) => g.visible), (g) => {
    const d = document.createElement('div');
    d.className = 'card';
    d.innerHTML = '<div class="t"><span class="name"></span><span class="owned"></span></div><div class="d"></div><div class="cost"></div>' +
      '<div class="row"><button class="btn primary one">Buy</button><button class="btn max">Max</button></div>';
    d.querySelector('.one').onclick = () => act({ cmd: 'buy', id: g.id });
    d.querySelector('.max').onclick = () => act({ cmd: 'buy', id: g.id, max: true });
    return d;
  }, (d, g) => {
    d.querySelector('.name').textContent = g.label;
    d.querySelector('.owned').textContent = g.owned + (g.max != null ? ' / ' + g.max : '');
    const makes = Object.entries(g.makes).map(([r, x]) => `+${format(x)} ${label(r)}/s each`).join(', ');
    d.querySelector('.d').textContent = [g.text, makes].filter(Boolean).join(' — ');
    d.querySelector('.cost').innerHTML = costHtml(g.cost);
    d.querySelector('.one').disabled = !g.can;
    const max = d.querySelector('.max');
    max.disabled = !g.can;
    max.textContent = g.afford > 1 ? `Max (${g.afford >= 1000 ? '1000+' : g.afford})` : 'Max';
  });

  // Levels: milestones with `view kind level`. `view tier N` groups them: a tier with several
  // levels is a branch, any one of which takes the player on.
  const tiers = [];
  view.milestones.filter((m) => m.view.kind === 'level').forEach((m, i) => {
    const n = Number(m.view.tier) || i + 1;
    let t = tiers.find((x) => x.n === n);
    if (!t) tiers.push((t = { n, levels: [] }));
    t.levels.push(m);
  });
  tiers.sort((a, b) => a.n - b.n);
  $('levels').hidden = !tiers.length;
  if (tiers.length) {
    const reached = tiers.filter((t) => t.levels.some((m) => m.done));
    const top = reached[reached.length - 1];
    $('levelNow').textContent = top ? top.levels.filter((m) => m.done).map((m) => m.label).join(' + ') : 'Level 0';
    $('levelPips').innerHTML = tiers.map((t) => `<span class="tier${t.levels.length > 1 ? ' branch' : ''}">` +
      t.levels.map((m) => `<span class="pip${m.done ? ' done' : ''}" title="${esc(m.label)}${m.done ? '' : ': ' + esc(m.text)}"></span>`).join('') +
      '</span>').join('');
    const next = tiers.find((t) => !t.levels.some((m) => m.done));
    const one = (m) => `<b title="${esc(m.text)}">${esc(m.label)}</b>`;
    $('levelNext').innerHTML = !next ? 'Every level reached.'
      : next.levels.length === 1 ? `Next: ${one(next.levels[0])}${next.levels[0].text ? ' — ' + esc(next.levels[0].text) : ''}`
      : `Next, one of: ${next.levels.map(one).join(' or ')}`;
  }

  const shown = view.upgrades.filter((u) => u.visible);
  const bought = shown.filter((u) => u.bought).length;
  $('upgCount').textContent = shown.length ? `${bought} / ${shown.length}` : '';
  $('showBought').hidden = !bought;
  $('showBought').textContent = showBought ? 'Hide bought' : `Show bought`;
  const ups = shown.filter((u) => showBought || !u.bought || u.view.kind === 'path').sort((a, b) => a.bought - b.bought);
  $('upgrades').querySelectorAll('.empty').forEach((e) => e.remove());
  keyed($('upgrades'), ups, (u) => {
    const d = document.createElement('div');
    d.className = 'card upg';
    d.innerHTML = '<div class="t"><span class="name"></span><span class="badge"></span></div><div class="d"></div><div class="cost"></div><div class="row"><button class="btn primary">Buy</button></div>';
    d.querySelector('button').onclick = () => act({ cmd: 'buy', id: u.id });
    return d;
  }, (d, u) => {
    d.classList.toggle('bought', u.bought);
    d.classList.toggle('path', u.view.kind === 'path');
    d.querySelector('.name').textContent = u.label;
    d.querySelector('.badge').textContent = u.bought ? (u.view.kind === 'path' ? 'chosen' : 'bought') : u.scope === 'world' ? 'for everyone' : u.view.kind === 'path' ? 'choose one' : '';
    d.querySelector('.d').textContent = u.text;
    d.querySelector('.cost').innerHTML = u.bought ? '' : costHtml(u.cost);
    d.querySelector('button').disabled = !u.can;
  });
  if (!ups.length) $('upgrades').insertAdjacentHTML('beforeend', `<div class="empty">${bought ? 'All bought.' : 'Nothing yet.'}</div>`);

  const goals = view.milestones.filter((m) => m.view.kind !== 'level' && (m.visible || m.done));
  $('goalCount').textContent = goals.length ? `${goals.filter((m) => m.done).length} / ${goals.length}` : '';
  const miles = [...goals.filter((m) => !m.done), ...goals.filter((m) => m.done)];
  keyed($('milestones'), miles, () => {
    const d = document.createElement('div');
    d.className = 'card mile';
    d.innerHTML = '<span class="dot"></span><span><span class="txt"></span><small></small></span>';
    return d;
  }, (d, m) => {
    d.classList.toggle('done', m.done);
    d.title = m.text;
    d.querySelector('.txt').textContent = m.label + (m.scope === 'world' ? ' (world)' : '');
    d.querySelector('small').textContent = m.text;
  });

  if (online) {
    const ps = [...view.players].sort((a, b) => log10(b.score ?? 0) - log10(a.score ?? 0));
    keyed($('players'), ps, () => {
      const d = document.createElement('div');
      d.className = 'p';
      d.innerHTML = '<span><span class="dot"></span><span class="nm"></span></span><span class="sc"></span>';
      return d;
    }, (d, p) => {
      d.classList.toggle('me', p.id === view.player.id);
      d.classList.toggle('on', onlinePlayers.has(p.id));
      d.querySelector('.nm').textContent = p.name;
      d.querySelector('.sc').textContent = p.score != null ? format(p.score) : '';
    });
  }
}

let pending = false;
function soon() {
  if (!pending) {
    pending = true;
    requestAnimationFrame(() => { pending = false; render(); });
  }
}

function onEvents(events) {
  const me = session.player;
  for (const e of events) {
    const mine = e.player === me;
    if (e.kind === 'unlock' && mine) toast(`New: ${e.label}`);
    else if (e.kind === 'milestone' && (mine || !e.player)) toast(`Milestone: ${e.label}`);
    else if (e.kind === 'prestige' && mine) toast(`${e.label}!`);
    else if (e.kind === 'upgrade' && e.scope === 'world') toast(`${nameOf(e.player)} built ${e.label} for everyone`);
    else if (e.kind === 'gift' && e.to === me) toast(`${nameOf(e.player)} gave you ${format(e.amount)} ${label(e.id)}`);
    else if (e.kind === 'join' && online && !mine) toast(`${e.name} joined`);
  }
  soon();
}

function nameOf(id) {
  const p = view && view.players.find((x) => x.id === id);
  return p ? p.name : 'Someone';
}

async function main() {
  $('showBought').onclick = () => { showBought = !showBought; render(); };
  const call = await engine('../openidle.wasm');
  if (online) {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = q.get('server') || `${proto}//${location.host}/ws`;
    let name = localStorage.getItem('openidle:name');
    if (!name) {
      name = (prompt('Your name in this world?') || 'Player').slice(0, 40);
      localStorage.setItem('openidle:name', name);
    }
    session = new Online(call, url, gameName, worldName || 'main', name);
    session.onStatus = (s) => { $('status').textContent = s; $('status').className = s; };
    session.onOnline = (ids) => { onlinePlayers.clear(); ids.forEach((i) => onlinePlayers.add(i)); soon(); };
    session.onError = (m) => toast(m);
    session.onChat = (m) => {
      const line = document.createElement('div');
      line.innerHTML = `<b>${esc(m.name)}</b> ${esc(m.text)}`;
      $('chatlog').appendChild(line);
      $('chatlog').scrollTop = 1e9;
    };
    $('chatform').onsubmit = (e) => {
      e.preventDefault();
      const t = $('chatinput').value.trim();
      if (t) session.chat(t);
      $('chatinput').value = '';
    };
    $('social').hidden = false;
    $('main').classList.add('online');
    $('tools').hidden = true;
    session.on(onEvents);
    session.start();
  } else {
    const bundle = await (await fetch(`../games/${gameName}.json`)).json();
    session = new Local(call, bundle, gameName);
    session.on(onEvents);
    $('status').textContent = 'solo';
    const away = session.start();
    render();
    if (away && away.ran > 10) {
      const gains = away.gains.map((g) => `<li>${esc(label(g.id))}: ${format(g.from)} → <b>${format(g.to)}</b></li>`).join('');
      const capped = away.ran + 1 < away.seconds ? ` (progress counts up to ${duration(away.ran)})` : '';
      dialog(`<h3>Welcome back</h3><p>You were away ${duration(away.seconds)}${capped}.</p><ul>${gains}</ul>`);
    }
    $('autoplay').onclick = () => {
      if (autoplay) { clearInterval(autoplay); autoplay = null; }
      else autoplay = setInterval(() => session.bot(3), 1000);
      $('autoplay').classList.toggle('on', !!autoplay);
    };
    $('export').onclick = () => dialog(`<h3>Your save</h3><p>Keep this text to load your game elsewhere.</p><textarea readonly>${esc(session.exportSave())}</textarea>`);
    $('import').onclick = async () => {
      const ok = await dialog('<h3>Load a save</h3><p>Paste a save; it replaces this game.</p><textarea id="importText"></textarea>', 'Load', 'Cancel');
      if (!ok) return;
      try { session.importSave(document.getElementById('importText').value); toast('Loaded'); render(); }
      catch (e) { toast('That is not a save'); }
    };
    $('reset').onclick = async () => {
      if (await dialog('<h3>Start over?</h3><p>Everything in this game is lost.</p>', 'Start over', 'Cancel')) {
        session.reset();
        location.reload();
      }
    };
  }
  setInterval(soon, 250);
  window.WavedashHook?.ready(); // on Wavedash: the game is drawn, show it
}

main().catch((e) => {
  document.body.innerHTML = `<div id="fatal">Could not start: ${esc(e.message)}</div>`;
  console.error(e);
  window.WavedashHook?.ready();
});
