// Builds the Rookparcel app icon, family 04-postal-rook (D59), from three sources:
//   16 px         src/courier-16.svg (drawn pixel by pixel: src/courier-16.txt, pixelmap.py)
//   20-32 px      src/courier-32.svg (the courier rook's head, after the generated 09 Compact Courier)
//   48-512 px     src/stamp-512.png (the generated Postal Rook stamp the owner chose for the
//                 large sizes, docs/art/rookparcel-candidates-2026-11-05)
// into ../patina-icons/exports/app/04-postal-rook/<size>.png and 04-postal-rook.ico, where every
// program and package reads the family ../patina-icons/app-icon.txt names
// (../patina-icons/INTEGRATION.md). src/stamp-256.svg (a vector stamp) and src/mark-16/-32 (the first
// small marks, a stamp close-up) are kept as alternatives.
//   SHARP=<path to sharp> node build.cjs
const fs = require('fs');
const path = require('path');
const sharp = require(process.env.SHARP || 'sharp');

const root = __dirname;
const FAMILY = '04-postal-rook';
const SOURCE = (size) => (size <= 16 ? 'courier-16.svg' : size <= 32 ? 'courier-32.svg' : 'stamp-512.png');
// as ../patina-icons/manifest.json's appPngSizes and the ICO frames crates/win32/build.rs embeds
const PNG_SIZES = [16, 20, 24, 32, 48, 64, 128, 256, 512];
const ICO_SIZES = [16, 20, 24, 32, 48, 64, 128, 256];

async function render(size) {
  const file = path.join(root, 'src', SOURCE(size));
  if (file.endsWith('.png')) {
    return sharp(file).resize(size, size, { kernel: 'lanczos3' }).png().toBuffer();
  }
  const box = Number(fs.readFileSync(file, 'utf8').match(/viewBox="0 0 (\d+)/)[1]);
  // drawn at its own size: exact pixels; otherwise rendered large and reduced
  const density = box === size ? 72 : Math.max(72, (72 * size * 2) / box);
  return sharp(file, { density }).resize(size, size, { kernel: 'lanczos3' }).png().toBuffer();
}

function ico(frames) {
  const head = Buffer.alloc(6 + 16 * frames.length);
  head.writeUInt16LE(0, 0);
  head.writeUInt16LE(1, 2);
  head.writeUInt16LE(frames.length, 4);
  let offset = head.length;
  frames.forEach(({ size, png }, i) => {
    const e = 6 + 16 * i;
    head.writeUInt8(size >= 256 ? 0 : size, e);
    head.writeUInt8(size >= 256 ? 0 : size, e + 1);
    head.writeUInt16LE(1, e + 4);
    head.writeUInt16LE(32, e + 6);
    head.writeUInt32LE(png.length, e + 8);
    head.writeUInt32LE(offset, e + 12);
    offset += png.length;
  });
  return Buffer.concat([head, ...frames.map((f) => f.png)]);
}

(async () => {
  const app = path.join(root, '..', 'patina-icons', 'exports', 'app');
  const out = path.join(app, FAMILY);
  fs.mkdirSync(out, { recursive: true });
  const frames = [];
  for (const size of PNG_SIZES) {
    // 8-bit RGBA, as icons::app_png promises
    const png = await sharp(await render(size)).ensureAlpha().png({ palette: false }).toBuffer();
    const meta = await sharp(png).metadata();
    if (meta.width !== size || meta.height !== size || meta.channels !== 4 || meta.depth !== 'uchar') {
      throw Error(`${size}: not ${size}x${size} 8-bit RGBA`);
    }
    fs.writeFileSync(path.join(out, `${size}.png`), png);
    if (ICO_SIZES.includes(size)) frames.push({ size, png });
  }
  const data = ico(frames);
  fs.writeFileSync(path.join(app, `${FAMILY}.ico`), data);
  console.log(`${FAMILY}: ${PNG_SIZES.length} PNGs, ${FAMILY}.ico with ${ICO_SIZES.length} frames, ${data.length} bytes`);
})();
