// App and menu-bar icons from one source of geometry: the Cantor dial (rings cut like a Cantor set, each ring
// keeping the outer part of every arc of the one before; the path walks one kept arc per ring down to the
// answer). Slate tile for dark mode, Paper for light; grey rings, a white (or ink) path, and a dot at each stop
// in the latency series colors: TLD green, Uncached rose, then the answer in Cached blue. A Mono set (no hue)
// is written alongside as an alternate. Small sizes are drawn for their size, with fewer rings and heavier
// strokes, rather than scaled down. Renders exact transparent PNGs with headless Chrome (no other SVG
// rasterizer is assumed), then builds icon.icns with iconutil and icon.ico by hand.
// Usage: node tools/icons.mjs            (writes crates/desktop/icons/, its src/*.svg and alt-mono/)
import { spawn, execFileSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, readFileSync, rmSync, mkdirSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const out = join(root, 'crates/desktop/icons');
const f = (v) => (Math.round(v * 10) / 10).toString();

// ---- Geometry -------------------------------------------------------------------------
// Ring 0 is whole; ring 1 keeps two arcs; each later ring keeps the outer 40% of every arc above it.
// The path takes arc `walk` at each split, and every ring turns a little further (tw) so it reads as a dial.
// c: { ink, path, stop: [dot color per ring], ans }.
const SIZES = {
  lg: { radii: [300, 222, 150, 84], w: 40 }, // 64 px and up
  sm: { radii: [292, 188, 96], w: 64 }, // 32 px, and 16 pt at 2x
  xs: { radii: [272, 136], w: 100 }, // under 32 px
};
function dial(size, c) {
  const { radii, w } = SIZES[size];
  const keep = 0.4, walk = [1, 0, 1], tw = 0.075;
  const levels = [[[0, 1]], [[0.05, 0.45], [0.55, 0.95]]], ci = [0, walk[0]];
  for (let r = 2; r < radii.length; r++) {
    levels.push(levels[r - 1].flatMap((s) => { const q = (s[1] - s[0]) * keep; return [[s[0], s[0] + q], [s[1] - q, s[1]]]; }));
    ci.push(ci[r - 1] * 2 + walk[r - 1]);
  }
  // Turn the whole dial so the answer sits at the lower right (45°).
  const L = radii.length - 1, fin = levels[L][ci[L]], off = 0.125 - (fin[0] + fin[1]) / 2 - L * tw;
  const at = (R, t) => [f(512 + R * Math.cos(t)), f(512 + R * Math.sin(t))];
  const dot = (R, t, r, fill) => { const [x, y] = at(R, t); return `<circle cx="${x}" cy="${y}" r="${f(r)}" fill="${fill}"/>`; };
  const arcs = [`<circle cx="512" cy="512" r="${radii[0]}" fill="none" stroke="${c.ink}" stroke-width="${w}"/>`], dots = [];
  for (let r = 1; r <= L; r++) {
    const R = radii[r];
    levels[r].forEach((s, i) => {
      const on = i === ci[r], answer = on && r === L;
      const a = 2 * Math.PI * (s[0] + off + r * tw), b = 2 * Math.PI * (s[1] + off + r * tw), cap = w / 2 / R, mid = (a + b) / 2;
      if (b - a <= 2 * cap + 0.02) {
        arcs.push(dot(R, mid, (w / 2) * (answer ? 1.3 : 1), answer ? c.ans : on ? c.path : c.ink));
      } else {
        const [a0, a1] = [at(R, a + cap), at(R, b - cap)];
        arcs.push(`<path d="M${a0}A${R},${R} 0 ${b - a - 2 * cap > Math.PI ? 1 : 0} 1 ${a1}" fill="none" stroke="${on ? c.path : c.ink}" stroke-width="${w}" stroke-linecap="round"/>`);
        if (answer) dots.push(dot(R, mid, w * 0.7, c.ans));
        else if (on && c.stop[r]) dots.push(dot(R, mid, w * 0.62, c.stop[r]));
      }
    });
  }
  return arcs.concat(dots).join('');
}

// Tiles, and the series colors for each from app.css (the light-surface values on Paper, dark on Slate).
const SLATE = { t0: '#3b4452', t1: '#171c25', ink: '#7d8796', path: '#e6eaef', top: '#ffffff', sheen: 0.15, shadow: 0.28, edge: '#ffffff', edgeOpacity: 0.12, tld: '#1baf7a', uncached: '#eb6834', cached: '#3987e5' };
const PAPER = { t0: '#fdf9f4', t1: '#ece3d8', ink: '#b5ada4', path: '#4a403c', top: '#1c1614', sheen: 0.5, shadow: 0.14, edge: '#000000', edgeOpacity: 0.08, tld: '#199e70', uncached: '#d95926', cached: '#2a78d6' };
const TRI = (t) => ({ ink: t.ink, path: t.path, stop: [null, t.tld, t.uncached], ans: t.cached });
const MONO = (t) => ({ ink: t.ink, path: t.path, stop: [], ans: t.top });

const tile = (t) => `
  <defs>
    <linearGradient id="tile" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${t.t0}"/><stop offset="1" stop-color="${t.t1}"/></linearGradient>
    <linearGradient id="sheen" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#ffffff" stop-opacity="${t.sheen}"/><stop offset="0.5" stop-color="#ffffff" stop-opacity="0"/></linearGradient>
    <filter id="shadow" x="-10%" y="-10%" width="120%" height="125%"><feDropShadow dx="0" dy="10" stdDeviation="12" flood-color="#000" flood-opacity="${t.shadow}"/></filter>
  </defs>
  <g filter="url(#shadow)"><rect x="100" y="100" width="824" height="824" rx="185" fill="url(#tile)"/></g>
  <rect x="100" y="100" width="824" height="824" rx="185" fill="url(#sheen)"/>`;
const edge = (t) => `<rect x="101" y="101" width="822" height="822" rx="184" fill="none" stroke="${t.edge}" stroke-opacity="${t.edgeOpacity}" stroke-width="2"/>`;

function appSvg(size, t, look, note) {
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">
  <!-- ${note} -->${tile(t)}
  ${dial(size, look(t))}
  ${edge(t)}
</svg>
`;
}

// Menu-bar template: black on transparent, the structure at 45% alpha; macOS tints it for the bar.
const TRAY = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="170 170 684 684" width="22" height="22">
  <!-- Menu-bar template image: black on transparent; macOS tints it for light and dark menu bars. -->
  ${dial('xs', { ink: 'rgb(0 0 0 / 0.45)', path: '#000', stop: [], ans: '#000' })}
</svg>
`;

// The bundle icon is Slate; the app swaps the Dock icon to app-light (Paper) in light mode (shell.rs).
const sources = (look, name) => ({
  'app.svg': appSvg('lg', SLATE, look, `Recursa app icon (${name}, Slate, dark mode), 64 px and up: the Cantor dial (macOS grid: 824 px body, 100 px margin).`),
  'app-32.svg': appSvg('sm', SLATE, look, `App icon (${name}) drawn for 32 px (and 16 pt at 2x): three rings, heavier strokes.`),
  'app-16.svg': appSvg('xs', SLATE, look, `App icon (${name}) drawn for 16 px: two rings, so the gaps stay open.`),
  'app-light.svg': appSvg('lg', PAPER, look, `App icon (${name}, Paper): the Dock icon in light mode.`),
});
const SOURCES = { ...sources(TRI, 'Tri'), 'tray.svg': TRAY };
const ALT = sources(MONO, 'Mono');

// Which drawing each output size uses.
const pick = (px) => (px < 32 ? 'app-16.svg' : px < 64 ? 'app-32.svg' : 'app.svg');

// ---- Rendering --------------------------------------------------------------------------
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function chrome() {
  const profile = mkdtempSync(join(process.env.TMPDIR ?? tmpdir(), 'icons-'));
  const port = 9800 + Math.floor(Math.random() * 400);
  const proc = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, '--no-first-run', '--hide-scrollbars', 'about:blank',
  ], { stdio: 'ignore' });
  let target;
  for (let i = 0; i < 60 && !target; i++) {
    await sleep(150);
    try {
      target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === 'page');
    } catch {}
  }
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r));
  let id = 0;
  const pending = new Map();
  ws.addEventListener('message', (m) => {
    const msg = JSON.parse(m.data);
    if (msg.id && pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); }
  });
  const send = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
  await send('Page.enable');
  await send('Emulation.setDefaultBackgroundColorOverride', { color: { r: 0, g: 0, b: 0, a: 0 } });
  return {
    async render(svg, px) {
      await send('Emulation.setDeviceMetricsOverride', { width: px, height: px, deviceScaleFactor: 1, mobile: false });
      const html = `<!doctype html><style>html,body{margin:0;background:transparent}svg{display:block;width:${px}px;height:${px}px}</style>${svg}`;
      await send('Page.navigate', { url: 'data:text/html;base64,' + Buffer.from(html).toString('base64') });
      await sleep(120);
      const r = await send('Page.captureScreenshot', { format: 'png', clip: { x: 0, y: 0, width: px, height: px, scale: 1 } });
      return Buffer.from(r.result.data, 'base64');
    },
    // Chrome keeps writing to its profile until it has exited.
    async close() {
      ws.close();
      const exited = new Promise((r) => proc.once('exit', r));
      proc.kill();
      await exited;
      rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    },
  };
}

// ICO with PNG entries (Windows Vista and later read these directly).
function ico(images) {
  const head = Buffer.alloc(6 + 16 * images.length);
  head.writeUInt16LE(0, 0); head.writeUInt16LE(1, 2); head.writeUInt16LE(images.length, 4);
  let offset = head.length;
  images.forEach(([px, png], i) => {
    const e = 6 + 16 * i;
    head.writeUInt8(px >= 256 ? 0 : px, e); head.writeUInt8(px >= 256 ? 0 : px, e + 1);
    head.writeUInt16LE(1, e + 4); head.writeUInt16LE(32, e + 6);
    head.writeUInt32LE(png.length, e + 8); head.writeUInt32LE(offset, e + 12);
    offset += png.length;
  });
  return Buffer.concat([head, ...images.map(([, png]) => png)]);
}

const c = await chrome();

// macOS: the .iconset names each size and its 2x; 16 pt at 2x keeps the 32 px drawing, as it is still shown small.
async function icns(src, dest, at) {
  const set = mkdtempSync(join(process.env.TMPDIR ?? tmpdir(), 'iconset-')) + '/Recursa.iconset';
  mkdirSync(set);
  for (const pt of [16, 32, 128, 256, 512]) {
    writeFileSync(join(set, `icon_${pt}x${pt}.png`), await at(pt));
    writeFileSync(join(set, `icon_${pt}x${pt}@2x.png`), pt === 16 ? await at(32) : await at(pt * 2));
  }
  execFileSync('/usr/bin/iconutil', ['-c', 'icns', set, '-o', dest]);
  rmSync(dirname(set), { recursive: true, force: true });
}
const renderer = (src) => { const png = {}; return async (px) => (png[px] ??= await c.render(src[pick(px)], px)); };

mkdirSync(join(out, 'src'), { recursive: true });
for (const [name, svg] of Object.entries(SOURCES)) writeFileSync(join(out, 'src', name), svg);
const at = renderer(SOURCES);
await icns(SOURCES, join(out, 'icon.icns'), at);

// Tauri's PNGs, the Windows icon and the menu-bar template (18 pt at 2x).
writeFileSync(join(out, '32x32.png'), await at(32));
writeFileSync(join(out, '128x128.png'), await at(128));
writeFileSync(join(out, '128x128@2x.png'), await at(256));
writeFileSync(join(out, 'icon.png'), await at(512));
writeFileSync(join(out, 'icon.ico'), ico(await Promise.all([16, 24, 32, 48, 64, 256].map(async (px) => [px, await at(px)]))));
writeFileSync(join(out, 'tray.png'), await c.render(TRAY, 36));
// The light-mode Dock icon the app switches to (icon.png is the dark one).
writeFileSync(join(out, 'icon-light.png'), await c.render(SOURCES['app-light.svg'], 512));

// The Mono alternate: sources, icns and the two 512 px Dock PNGs. Not wired into the bundle.
const alt = join(out, 'alt-mono');
mkdirSync(join(alt, 'src'), { recursive: true });
for (const [name, svg] of Object.entries(ALT)) writeFileSync(join(alt, 'src', name), svg);
const altAt = renderer(ALT);
await icns(ALT, join(alt, 'icon.icns'), altAt);
writeFileSync(join(alt, 'icon.png'), await altAt(512));
writeFileSync(join(alt, 'icon-light.png'), await c.render(ALT['app-light.svg'], 512));
await c.close();
console.log('icons written to', out);
