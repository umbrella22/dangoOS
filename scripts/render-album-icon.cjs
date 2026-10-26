#!/usr/bin/env node
// SVG is the editable master. The launcher consumes an RGB JPEG at 96 px.
const fs = require('node:fs');
const path = require('node:path');
const { createRequire } = require('node:module');
const localRequire = process.env.ALBUM_ICON_NODE_MODULES
  ? createRequire(path.join(path.resolve(process.env.ALBUM_ICON_NODE_MODULES), '__album_icon__.cjs'))
  : require;
const sharp = localRequire('sharp');
const root = path.resolve(__dirname, '..');

async function main() {
  const svg = fs.readFileSync(path.join(root, 'apps/album_app/assets/icon.svg'));
  const image = sharp(svg, { density: 576 });
  await image.clone().resize(96, 96).flatten({ background: '#000000' })
    .jpeg({ quality: 96, chromaSubsampling: '4:4:4', progressive: false })
    .toFile(path.join(root, 'apps/album_app/assets/icon.jpg'));
  if (process.argv[2]) {
    await image.clone().resize(512, 512).png().toFile(path.resolve(process.argv[2]));
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
