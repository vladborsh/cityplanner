// Renders the self-made UI icons: assets/ui/icons/src/*.svg -> assets/ui/icons/*.png, 128x128.
// Not part of the build; the PNGs are committed.
//
//   cd tools/assets && npm install
//   node icons.mjs
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { Resvg } from '@resvg/resvg-js';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../assets/ui/icons');
const src = path.join(root, 'src');
const SIZE = 128;

for (const file of fs.readdirSync(src).filter((f) => f.endsWith('.svg')).sort()) {
  const svg = fs.readFileSync(path.join(src, file));
  const png = new Resvg(svg, { fitTo: { mode: 'width', value: SIZE } }).render().asPng();
  fs.writeFileSync(path.join(root, file.replace(/\.svg$/, '.png')), png);
  console.log(file);
}
