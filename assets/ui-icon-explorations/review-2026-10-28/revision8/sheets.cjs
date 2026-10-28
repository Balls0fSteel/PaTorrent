// Revision 8 review evidence: neighbour sheets, actual-size menu strip, masks, IoU.
// Usage: node sheets.cjs <ui-icon-explorations dir> <out dir> <sharp path>
'use strict';
const fs = require('fs');
const path = require('path');
const [root, out, sharpPath] = process.argv.slice(2);
const sharp = require(sharpPath);
fs.mkdirSync(out, {recursive: true});

const NEW = ['website', 'check-updates', 'notices', 'import', 'window', 'edit', 'feed-disabled', 'no-label'];
const GROUPS = {
  website: ['web-ui', 'set-location', 'open-folder', 'logout', 'desktop-integration'],
  'check-updates': ['download', 'tray', 'refresh', 'reload-ip-filter', 'save'],
  notices: ['help', 'about', 'general', 'skip', 'search'],
  import: ['logout', 'tray', 'add-torrent', 'set-location', 'save'],
  window: ['web-ui', 'desktop-integration', 'reset-columns', 'clear-selection', 'select-all'],
  edit: ['create-torrent', 'properties', 'rss-rule', 'search'],
  'feed-disabled': ['rss', 'add-feed', 'rss-filters', 'rss-rule', 'tracker-error'],
  'no-label': ['labels', 'new-label', 'remove-label', 'offline', 'tracker-error'],
};
const SIZES = [16, 20, 24, 32];
const ZOOM = {16: 8, 20: 6, 24: 5, 32: 4};
const MODES = {light: ['png', '#ffffff'], dark: ['png-dark', '#2b2b2b'], mono: ['png-mono', '#ffffff']};
const file = (mode, size, id) => path.join(root, MODES[mode][0], 'bold', String(size), id + '.png');
const esc = t => t.replace(/&/g, '&amp;').replace(/</g, '&lt;');
const text = (x, y, t, size = 13, fill = '#222', weight = 'normal') =>
  `<text x="${x}" y="${y}" font-family="Segoe UI, Arial, sans-serif" font-size="${size}" font-weight="${weight}" fill="${fill}">${esc(t)}</text>`;

async function zoomed(mode, size, id, z) {
  return sharp(file(mode, size, id)).resize(size * z, size * z, {kernel: 'nearest'}).png().toBuffer();
}
function gridSvg(size, z, colour = 'rgba(0,0,0,0.12)') {
  let l = '';
  for (let i = 0; i <= size; i++) {
    l += `<line x1="${i * z}" y1="0" x2="${i * z}" y2="${size * z}" stroke="${colour}" stroke-width="1"/>`;
    l += `<line x1="0" y1="${i * z}" x2="${size * z}" y2="${i * z}" stroke="${colour}" stroke-width="1"/>`;
  }
  return Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" width="${size * z}" height="${size * z}">${l}</svg>`);
}

// One sheet per new symbol: the symbol and its neighbours, light/dark at 16/20/24/32
// (nearest x8/x6/x5/x4), mono 16 x8 with a pixel grid, and native 1x light/dark.
async function groupSheet(id, list, outName) {
  const ids = list || [id, ...GROUPS[id]];
  const cell = 140, label = 150, head = 44, nat = 70;
  const cols = [];
  for (const m of ['light', 'dark']) for (const s of SIZES) cols.push([m, s]);
  cols.push(['mono', 16]);
  const W = label + cols.length * cell + nat + 10, Hh = head + ids.length * cell;
  const comps = [];
  let svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${Hh}"><rect width="100%" height="100%" fill="#e8e8e8"/>`;
  cols.forEach(([m, s], k) => {
    const x = label + k * cell;
    svg += `<rect x="${x}" y="${head}" width="${cell}" height="${ids.length * cell}" fill="${MODES[m][1]}"/>`;
    svg += text(x + 6, 28, `${m} ${s}px x${ZOOM[s]}${m === 'mono' ? ' grid' : ''}`, 13);
  });
  const nx = label + cols.length * cell;
  svg += `<rect x="${nx}" y="${head}" width="${nat / 2}" height="${ids.length * cell}" fill="#ffffff"/>`;
  svg += `<rect x="${nx + nat / 2}" y="${head}" width="${nat / 2}" height="${ids.length * cell}" fill="#2b2b2b"/>`;
  svg += text(nx + 4, 28, '1x', 13);
  ids.forEach((i, r) => {
    svg += text(8, head + r * cell + 74, i, 14, '#111', r === 0 ? 'bold' : 'normal');
  });
  svg += '</svg>';
  comps.push({input: Buffer.from(svg), left: 0, top: 0});
  for (let r = 0; r < ids.length; r++) {
    for (let k = 0; k < cols.length; k++) {
      const [m, s] = cols[k], z = ZOOM[s], d = s * z;
      const left = label + k * cell + Math.floor((cell - d) / 2), top = head + r * cell + Math.floor((cell - d) / 2);
      comps.push({input: await zoomed(m, s, ids[r], z), left, top});
      if (m === 'mono') comps.push({input: gridSvg(s, z), left, top});
    }
    comps.push({input: file('light', 16, ids[r]), left: nx + 9, top: head + r * cell + 62});
    comps.push({input: file('dark', 16, ids[r]), left: nx + nat / 2 + 9, top: head + r * cell + 62});
  }
  const name = outName || `group-${id}.png`;
  await sharp({create: {width: W, height: Hh, channels: 4, background: '#e8e8e8'}}).composite(comps).png().toFile(path.join(out, name));
  return name;
}

// Actual-size menu mock-up: icon + label at 100/125/150/200 % on light and dark menus,
// plus the GTK case (32 px export shown at 16 logical px, i.e. downscaled with a linear kernel).
const LABELS = {
  website: 'PaTorrent Website', 'check-updates': 'Check for Updates', notices: 'Third-Party Notices',
  import: 'Import from qBittorrent...', window: 'Show PaTorrent', edit: 'Edit Feed...',
  'feed-disabled': 'Disable Feed', 'no-label': 'No Label',
};
async function menuStrip() {
  const variants = [[16, 'Win32 100%'], [20, 'Win32 125%'], [24, 'Win32 150%'], [32, 'Win32 200%'], ['gtk', 'GTK 32->16']];
  const rowH = s => (s === 'gtk' ? 22 : Math.round(s * 1.4) + 4);
  const colW = 300;
  const comps = [];
  let total = 30;
  for (const [s] of variants) total += NEW.length * rowH(s) + 30;
  const W = 20 + colW * 2;
  let svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${total}"><rect width="100%" height="100%" fill="#e8e8e8"/>`;
  let y = 30;
  const places = [];
  for (const [s, name] of variants) {
    svg += text(10, y - 8, name, 12, '#333', 'bold');
    const h = rowH(s), px = s === 'gtk' ? 16 : s, fs_ = s === 'gtk' ? 12 : Math.round(12 * s / 16);
    svg += `<rect x="10" y="${y}" width="${colW}" height="${NEW.length * h}" fill="#f9f9f9"/>`;
    svg += `<rect x="${10 + colW}" y="${y}" width="${colW}" height="${NEW.length * h}" fill="#2b2b2b"/>`;
    NEW.forEach((id, k) => {
      const ty = y + k * h;
      svg += text(20 + px + 10, ty + h / 2 + fs_ / 3, LABELS[id], fs_, '#1a1a1a');
      svg += text(10 + colW + 10 + px + 10, ty + h / 2 + fs_ / 3, LABELS[id], fs_, '#f0f0f0');
      places.push({s, id, x: 20, x2: 10 + colW + 10, y: ty + Math.floor((h - px) / 2)});
    });
    y += NEW.length * h + 30;
  }
  svg += '</svg>';
  comps.push({input: Buffer.from(svg), left: 0, top: 0});
  for (const pl of places) {
    if (pl.s === 'gtk') {
      comps.push({input: await sharp(file('light', 32, pl.id)).resize(16, 16, {kernel: 'linear'}).png().toBuffer(), left: pl.x, top: pl.y});
      comps.push({input: await sharp(file('dark', 32, pl.id)).resize(16, 16, {kernel: 'linear'}).png().toBuffer(), left: pl.x2, top: pl.y});
    } else {
      comps.push({input: file('light', pl.s, pl.id), left: pl.x, top: pl.y});
      comps.push({input: file('dark', pl.s, pl.id), left: pl.x2, top: pl.y});
    }
  }
  const buf = await sharp({create: {width: W, height: total, channels: 4, background: '#e8e8e8'}}).composite(comps).png().toBuffer();
  await sharp(buf).toFile(path.join(out, 'menu-1x.png'));
  const meta = await sharp(buf).metadata();
  await sharp(buf).resize(meta.width * 2, meta.height * 2, {kernel: 'nearest'}).toFile(path.join(out, 'menu-2x.png'));
}

// Overview of the eight at every shipped size, light/dark/mono, nearest x4 of native.
async function overview() {
  const cell = 32 * 4 + 12, label = 130;
  const cols = [];
  for (const m of ['light', 'dark', 'mono']) for (const s of SIZES) cols.push([m, s]);
  const W = label + cols.length * cell, Hh = 30 + NEW.length * cell;
  let svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${Hh}"><rect width="100%" height="100%" fill="#e8e8e8"/>`;
  cols.forEach(([m, s], k) => {
    svg += `<rect x="${label + k * cell}" y="30" width="${cell}" height="${NEW.length * cell}" fill="${MODES[m][1]}"/>`;
    svg += text(label + k * cell + 4, 20, `${m} ${s} x4`, 12);
  });
  NEW.forEach((id, r) => { svg += text(6, 30 + r * cell + cell / 2 + 4, id, 13, '#111', 'bold'); });
  svg += '</svg>';
  const comps = [{input: Buffer.from(svg), left: 0, top: 0}];
  for (let r = 0; r < NEW.length; r++) for (let k = 0; k < cols.length; k++) {
    const [m, s] = cols[k], d = s * 4;
    comps.push({input: await zoomed(m, s, NEW[r], 4), left: label + k * cell + Math.floor((cell - d) / 2), top: 30 + r * cell + Math.floor((cell - d) / 2)});
  }
  await sharp({create: {width: W, height: Hh, channels: 4, background: '#e8e8e8'}}).composite(comps).png().toFile(path.join(out, 'overview.png'));
}

async function alpha(size, id) {
  const {data, info} = await sharp(file('mono', size, id)).ensureAlpha().raw().toBuffer({resolveWithObject: true});
  const a = [];
  for (let i = 0; i < info.width * info.height; i++) a.push(data[i * 4 + 3]);
  return a;
}
async function asciiAndNeighbours() {
  const ids = fs.readdirSync(path.join(root, 'png-mono', 'bold', '16')).map(f => f.replace(/\.png$/, '')).sort();
  for (const size of [16, 20, 24, 32]) {
    let t = `# ${size} px mono masks of the revision 8 symbols. '#' alpha >= 128, '+' 32-127, '.' < 32. Columns 0-${size - 1}.\n`;
    for (const id of NEW) {
      const a = await alpha(size, id);
      t += `\n${id}\n    ` + [...Array(size).keys()].map(x => x % 10).join('') + '\n';
      for (let y = 0; y < size; y++) t += String(y).padStart(3) + ' ' + a.slice(y * size, y * size + size).map(v => v >= 128 ? '#' : v >= 32 ? '+' : '.').join('') + '\n';
      // Stroke-width audit: lengths of horizontal and vertical solid runs.
    }
    fs.writeFileSync(path.join(out, `ascii-${size}.txt`), t);
  }
  let rep = '';
  for (const size of [16, 32]) {
    const M = {};
    for (const id of ids) M[id] = (await alpha(size, id)).map(v => v >= 128);
    const iou = (a, b) => { let i = 0, u = 0; for (let k = 0; k < a.length; k++) { if (a[k] && b[k]) i++; if (a[k] || b[k]) u++; } return u ? i / u : 1; };
    rep += `# ${size} px: top 6 mask neighbours (IoU, alpha >= 128) of each revision 8 symbol\n`;
    for (const id of NEW) {
      const sc = ids.filter(o => o !== id).map(o => [iou(M[id], M[o]), o]).sort((p, q) => q[0] - p[0]).slice(0, 6);
      rep += id.padEnd(15) + sc.map(([v, o]) => `${o} ${v.toFixed(2)}`).join('  ') + '\n';
    }
    rep += '\n';
  }
  // Outer pixel and badge clearance at every shipped size (any alpha > 0 counts as ink).
  rep += '# edge ink (alpha > 0 on the outer row/column) and badge gap (Chebyshev distance, 16 px, alpha >= 128; 2 = one clear pixel)\n';
  for (const id of NEW) {
    const edges = [];
    for (const size of [16, 20, 24, 32, 48, 64]) {
      const a = await alpha(size, id);
      let e = 0;
      for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) if (a[y * size + x] > 0 && (x === 0 || y === 0 || x === size - 1 || y === size - 1)) e++;
      edges.push(`${size}:${e}`);
    }
    const a = (await alpha(16, id)).map(v => v >= 128);
    // Badge = ink inside x 10-14, y 9-14 that is a separate 8-connected component from the rest.
    const comp = new Array(256).fill(-1); let n = 0;
    for (let k = 0; k < 256; k++) if (a[k] && comp[k] < 0) {
      const st = [k]; comp[k] = n;
      while (st.length) { const q = st.pop(), x = q % 16, y = (q / 16) | 0;
        for (let dy = -1; dy <= 1; dy++) for (let dx = -1; dx <= 1; dx++) { const X = x + dx, Y = y + dy, j = Y * 16 + X;
          if (X >= 0 && Y >= 0 && X < 16 && Y < 16 && a[j] && comp[j] < 0) { comp[j] = n; st.push(j); } } }
      n++;
    }
    const inBox = q => { const x = q % 16, y = (q / 16) | 0; return x >= 10 && x <= 14 && y >= 9 && y <= 14; };
    const badge = new Set();
    for (let c2 = 0; c2 < n; c2++) { const px = [...Array(256).keys()].filter(k => comp[k] === c2); if (px.every(inBox)) badge.add(c2); }
    let gap = '-';
    if (badge.size) {
      let best = 99;
      for (let p1 = 0; p1 < 256; p1++) if (a[p1] && badge.has(comp[p1])) for (let p2 = 0; p2 < 256; p2++) if (a[p2] && !badge.has(comp[p2])) {
        best = Math.min(best, Math.max(Math.abs(p1 % 16 - p2 % 16), Math.abs(((p1 / 16) | 0) - ((p2 / 16) | 0))));
      }
      gap = String(best);
    }
    rep += `${id.padEnd(15)} edge ${edges.join(' ')}  components ${n}  badge gap ${gap}\n`;
  }
  fs.writeFileSync(path.join(out, 'neighbours.txt'), rep);
}

(async () => {
  for (const id of NEW) console.log(await groupSheet(id));
  // The highest-IoU neighbours (neighbours.txt) that the hand-picked groups do not cover.
  console.log(await groupSheet(null, ['notices', 'error', 'appearance', 'copy', 'about', 'window', 'file-video', 'disk', 'logger'], 'iou-1.png'));
  console.log(await groupSheet(null, ['window', 'random-port', 'folder', 'scheduler', 'website', 'select-all', 'file-image', 'feed-disabled', 'rss-history'], 'iou-2.png'));
  console.log(await groupSheet(null, ['no-label', 'wanted-complete', 'edit', 'no-peers', 'start', 'import', 'unlock', 'privacy'], 'iou-3.png'));
  await menuStrip();
  await overview();
  await asciiAndNeighbours();
  console.log('done');
})().catch(e => { console.error(e); process.exit(1); });
