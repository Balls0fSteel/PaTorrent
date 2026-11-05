// node render-one.cjs <svg> <out.png> <size>: renders one SVG at one size (for drafting).
const sharp = require(process.env.SHARP || 'sharp');
const [svg, out, size] = process.argv.slice(2);
const n = Number(size || 512);
sharp(svg, { density: Math.max(72, (72 * n) / 64) })
  .resize(n, n, { kernel: 'lanczos3' })
  .png()
  .toFile(out)
  .then(() => console.log(out));
