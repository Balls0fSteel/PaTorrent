// node preview.cjs <out.png> <svg>:<size>[,<size>...] ...
// Renders each SVG at native sizes, shows them 4x enlarged (nearest) on a light and a dark row.
const sharp = require(process.env.SHARP || 'sharp');
const [out, ...specs] = process.argv.slice(2);
const K = 4, GAP = 8, CELL = 48 * K;
(async () => {
  const items = [];
  for (const spec of specs) {
    const [svg, sizes] = spec.split(':');
    for (const s of sizes.split(',').map(Number)) {
      const png = await sharp(svg, { density: 72 * s / 32 * 32 / Number(require('fs').readFileSync(svg, 'utf8').match(/viewBox="0 0 (\d+)/)[1]) })
        .resize(s, s, { kernel: 'lanczos3' }).png().toBuffer();
      items.push({ s, png });
    }
  }
  const W = GAP + items.length * (CELL + GAP), H = GAP + 2 * (CELL + GAP);
  const comps = [];
  for (const [row, bg] of [[0, '#FAFAFA'], [1, '#202020']].entries()) {
    for (const [i, it] of items.entries()) {
      const big = await sharp(it.png).resize(it.s * K, it.s * K, { kernel: 'nearest' }).toBuffer();
      const cell = await sharp({ create: { width: CELL, height: CELL, channels: 4, background: bg[1] } })
        .composite([{ input: big, gravity: 'center' }]).png().toBuffer();
      comps.push({ input: cell, left: GAP + i * (CELL + GAP), top: GAP + row * (CELL + GAP) });
    }
  }
  await sharp({ create: { width: W, height: H, channels: 4, background: '#808080' } }).composite(comps).png().toFile(out);
  console.log(out);
})();
