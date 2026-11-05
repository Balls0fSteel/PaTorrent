// Builds the Rookparcel app icon from its three drawings (D59):
//   16 px         src/mark-16.svg   (drawn pixel by pixel: src/mark-16.txt, pixelmap.py)
//   20-32 px      src/mark-32.svg   (the head close-up)
//   48-512 px     src/stamp-256.svg (the full stamp)
// into exports/<size>.png and exports/rookparcel.ico (16-256 px PNG frames).
//   SHARP=<path to sharp> node build.cjs
const fs = require('fs');
const path = require('path');
const sharp = require(process.env.SHARP || 'sharp');

const root = __dirname;
const SOURCE = (size) => (size <= 16 ? 'mark-16.svg' : size <= 32 ? 'mark-32.svg' : 'stamp-256.svg');
const PNG_SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256, 512];
const ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256];

async function render(size) {
  const file = path.join(root, 'src', SOURCE(size));
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
  const out = path.join(root, 'exports');
  fs.mkdirSync(out, { recursive: true });
  const frames = [];
  for (const size of PNG_SIZES) {
    const png = await render(size);
    const meta = await sharp(png).metadata();
    if (meta.width !== size || meta.height !== size || !meta.hasAlpha) throw Error(`${size}: bad render`);
    fs.writeFileSync(path.join(out, `${size}.png`), png);
    if (ICO_SIZES.includes(size)) frames.push({ size, png });
  }
  const data = ico(frames);
  fs.writeFileSync(path.join(out, 'rookparcel.ico'), data);
  console.log(`exports: ${PNG_SIZES.length} PNGs, rookparcel.ico ${ICO_SIZES.length} frames, ${data.length} bytes`);
})();
