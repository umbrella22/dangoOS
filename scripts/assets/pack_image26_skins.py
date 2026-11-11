#!/usr/bin/env python3
"""Pack refreshed art and animation patches in the existing v4 skin format.

The palette uses an explicit per-character budget, with 96 colors selected for
the delivered revision. Quantization is lossy; compression and reuse are lossless.
Every device package must remain below the existing 1 MiB transfer limit.
"""
import argparse
import hashlib
import json
import struct
import zlib
from pathlib import Path

import numpy as np
from PIL import Image
import zopfli.zlib

from refresh_companion_expressions import IDS, POSES, MODEL, SIZE

HEADER = struct.Struct('<8sIHHHHIIII')
DESC = struct.Struct('<hhHHIII')
GROUPS = ['blink', 'mouth-closed', 'mouth-wide', 'ear', 'gaze-left', 'gaze-right',
          'symbols', 'tremble', 'spiral-1', 'spiral-2', 'spiral-3']
RIG_PARTS = ['body', 'left', 'right', 'strain', 'fall', 'land', 'relief', 'ear-left', 'ear-right', 'blink']


def read_skin(path):
    data = path.read_bytes()
    magic, version, w, h, colors, poses, ml, bl, checksum, reserved = HEADER.unpack_from(data)
    if magic != b'MSKIN01\0' or version != 4 or reserved or (w, h) != SIZE:
        raise ValueError('Expected current v4 source skin')
    if len(data) != HEADER.size + ml + bl or zlib.crc32(data[HEADER.size:]) != checksum:
        raise ValueError('Source skin length/CRC mismatch')
    meta = data[HEADER.size:HEADER.size + ml]; blob = data[HEADER.size + ml:]
    pal = struct.unpack_from('<' + 'H' * colors, meta)
    rgb = [(((c >> 11) & 31) * 255 // 31, ((c >> 5) & 63) * 255 // 63, (c & 31) * 255 // 31) for c in pal]
    lut = np.array([(*c, 255) for c in rgb] + [(0, 0, 0, 0)] * (256 - colors), np.uint8)
    at = colors * 2

    def image():
        nonlocal at
        x, y, iw, ih, offset, length, flags = DESC.unpack_from(meta, at); at += DESC.size
        raw = zlib.decompress(blob[offset:offset + length])
        if len(raw) != iw * ih: raise ValueError('Source sprite decoded length')
        im = Image.fromarray(lut[np.frombuffer(raw, np.uint8).reshape((ih, iw))], 'RGBA')
        return {'image': im, 'x': x, 'y': y, 'flags': flags}

    items = {}
    for i in range(poses):
        pose = struct.unpack_from('<H', meta, at)[0]; at += 2
        if pose != i: raise ValueError('Source pose order')
        item = {'base': image(), 'groups': {}}
        for group in GROUPS:
            count = struct.unpack_from('<H', meta, at)[0]; at += 2
            item['groups'][group] = [image() for _ in range(count)]
        items[POSES[i]] = item
    return items


def train_palette(images, colors=128):
    # Include patches at their actual device resolution. Repeated full frames
    # never enter palette training, so animation duration cannot bias colors.
    atlas = Image.new('RGB', (352 * 5, 336 * ((len(images) + 4) // 5)))
    for i, image in enumerate(images):
        flat = Image.new('RGB', image.size)
        flat.paste(image.convert('RGB'), mask=image.getchannel('A') if image.mode == 'RGBA' else None)
        atlas.paste(flat, (i % 5 * 352, i // 5 * 336))
    # Connection colors carry meaning and must survive even a predominantly
    # red character palette. Reserve the exact path colors before training.
    fixed = ['000000', '86baff', 'c2bad9', '487fca', 'c64d69', 'fff5fa']
    rgb565 = []
    for color in fixed:
        r, g, b = (int(color[i:i + 2], 16) for i in (0, 2, 4))
        rgb565.append(((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3))
    palette = atlas.quantize(colors=colors - len(fixed), method=Image.Quantize.MEDIANCUT,
                             dither=Image.Dither.NONE).getpalette()
    for i in range(colors - len(fixed)):
        r, g, b = palette[i * 3:i * 3 + 3]
        value = ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
        if value not in rgb565: rgb565.append(value)
    rgb = []
    for c in rgb565:
        rgb.extend((((c >> 11) & 31) * 255 // 31, ((c >> 5) & 63) * 255 // 63, (c & 31) * 255 // 31))
    # Pillow's P palette has 256 entries; duplicate real black in unused slots.
    ref = Image.new('P', (1, 1)); ref.putpalette(rgb + [0] * (768 - len(rgb)))
    return rgb565, ref


def build(work, identity, colors=128, palette_from=None, version=None):
    art = work / 'art' / identity
    components = work / 'components' / identity
    spec = json.loads((components / 'manifest.json').read_text())
    old = read_skin(work / 'before/skins' / identity / 'fox.bin')
    output = work / 'skins' / identity; output.mkdir(parents=True, exist_ok=True)
    all_images = []
    for pose in POSES:
        item = old[pose]
        item['base'] = {'image': Image.open(art / (pose + '-device.png')).convert('RGB'), 'x': 0, 'y': 0, 'flags': 0}
        all_images.append(item['base']['image'])
        for group, replacements in spec['poses'].get(pose, {}).items():
            item['groups'][group] = [{'image': Image.open(components / p['file']).convert('RGBA'),
                'x': p['x'], 'y': p['y'], 'flags': p.get('flags', 0)} for p in replacements]
        for group in item['groups'].values(): all_images.extend(i['image'] for i in group)
    rig_spec = json.loads((components / 'rig.json').read_text())
    rig_images = {name: Image.open(components / (name + '.png')).convert('RGBA') for name in RIG_PARTS}
    all_images.extend(rig_images.values())
    if palette_from is None and 'palette_rgb565' not in spec:
        palette, ref = train_palette(all_images, colors)
    else:
        # Local animation repairs reuse the accepted RGB565 palette so every
        # unrelated pose decodes to exactly the same pixels after repacking.
        if palette_from is not None:
            read_skin(palette_from)
            source_data = palette_from.read_bytes()
            count = HEADER.unpack_from(source_data)[4]
            palette = list(struct.unpack_from('<' + 'H' * count, source_data, HEADER.size))
        else:
            palette = spec['palette_rgb565']
        if not 1 <= len(palette) <= colors or any(type(c) is not int or not 0 <= c <= 65535 for c in palette):
            raise ValueError('Stored palette must fit the selected RGB565 color budget')
        rgb = []
        for c in palette:
            rgb.extend((((c >> 11) & 31) * 255 // 31, ((c >> 5) & 63) * 255 // 63, (c & 31) * 255 // 31))
        ref = Image.new('P', (1, 1)); ref.putpalette(rgb + [0] * (768 - len(rgb)))
    meta = bytearray(struct.pack('<' + 'H' * len(palette), *palette))
    blob = bytearray(); cache = {}; records = []; color_error = []
    compression_dir = work / 'compression-cache'; compression_dir.mkdir(exist_ok=True)

    def encode(item, label, base=False):
        image = item['image']; box = (0, 0, *image.size)
        indexed = np.asarray(image.convert('RGB').quantize(palette=ref, dither=Image.Dither.NONE)).copy()
        indexed[indexed >= len(palette)] = 0
        if image.mode == 'RGBA' and not base: indexed[np.asarray(image.getchannel('A')) < 128] = 255
        if base and (indexed == 255).any(): raise ValueError('Transparent base image')
        raw = indexed.tobytes(); digest = hashlib.sha256(raw).hexdigest()
        if digest not in cache:
            cached = compression_dir / (digest + '.zlib')
            if cached.exists(): compressed = cached.read_bytes()
            else:
                compressed = zopfli.zlib.compress(raw, numiterations=5)
                cached.write_bytes(compressed)
            if zlib.decompress(compressed) != raw: raise ValueError('Compression round trip')
            cache[digest] = (len(blob), len(compressed)); blob.extend(compressed)
        offset, length = cache[digest]
        meta.extend(DESC.pack(item['x'], item['y'], image.width, image.height, offset, length, item.get('flags', 0)))
        records.append({'label': label, 'decoded_sha256': digest, 'width': image.width, 'height': image.height,
                        'offset': offset, 'compressed_bytes': length})
        if base:
            decoded = Image.fromarray(indexed, 'P'); decoded.putpalette(ref.getpalette())
            decoded.save(output / (label + '.png'))
            a = np.asarray(image.convert('RGB')).astype('float32')
            b = np.asarray(decoded.convert('RGB')).astype('float32')
            visible = a.max(2) > 20
            color_error.append(float(np.abs(a[visible] - b[visible]).mean()))

    for i, pose in enumerate(POSES):
        meta.extend(struct.pack('<H', i)); encode(old[pose]['base'], pose, True)
        for group in GROUPS:
            items = old[pose]['groups'][group]; meta.extend(struct.pack('<H', len(items)))
            for n, item in enumerate(items): encode(item, f'{pose}/{group}/{n}')
    rig_start = len(meta)
    for name, image in rig_images.items():
        box = (0, 0, *SIZE) if name == 'body' else image.getbbox()
        if box is None: raise ValueError('Empty rig part ' + name)
        encode({'image': image.crop(box), 'x': box[0], 'y': box[1], 'flags': 0}, 'rig/' + name)
        if name in ('left', 'right', 'ear-left', 'ear-right'):
            px, py = rig_spec['parts'][name]['pivot']; px -= box[0]; py -= box[1]
            if not (0 <= px < box[2] - box[0] and 0 <= py < box[3] - box[1]):
                raise ValueError('Pivot outside part ' + name)
            meta.extend(struct.pack('<hh', px, py))
    if len(meta) - rig_start != 216: raise ValueError('v4 rig descriptor size')
    payload = meta + blob
    data = HEADER.pack(b'MSKIN01\0', 4, *SIZE, len(palette), 25, len(meta), len(blob), zlib.crc32(payload), 0) + payload
    if len(data) > 1024 * 1024: raise ValueError('Device skin transfer limit exceeded')
    for rec in records:
        raw = zlib.decompress(blob[rec['offset']:rec['offset'] + rec['compressed_bytes']])
        if hashlib.sha256(raw).hexdigest() != rec['decoded_sha256']: raise ValueError('Final stream checksum')
    (output / 'fox.bin').write_bytes(data)
    info = json.loads((work / 'before/skins' / identity / 'skin.json').read_text())
    info.update(version=version or spec.get('skin_version', '2026-11-11-image26-v1'),
                binary_bytes=len(data), binary_crc32=zlib.crc32(data))
    (output / 'skin.json').write_text(json.dumps(info, ensure_ascii=False, indent=2) + '\n')
    before = (work / 'before/skins' / identity / 'fox.bin').stat().st_size
    report = {'id': identity, 'model': MODEL, 'format': 4, 'before_bytes': before, 'after_bytes': len(data),
        'saved_bytes': before - len(data), 'saved_percent': round((before - len(data)) * 100 / before, 2),
        'palette_colors': len(palette), 'palette_budget': colors, 'expressions': 25,
        'connection_colors_reserved': True,
        'images': len(records), 'unique_streams': len(cache), 'mean_rgb_absolute_error': round(sum(color_error) / 25, 3),
        'sha256': hashlib.sha256(data).hexdigest(), 'images_verified': records}
    (output / 'build-report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    return {k: v for k, v in report.items() if k != 'images_verified'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--colors', type=int, default=96, choices=[96, 128])
    parser.add_argument('--id', choices=IDS)
    parser.add_argument('--palette-from', type=Path, help='Reuse a validated v4 skin palette; requires --id')
    parser.add_argument('--version', help='Override the component manifest skin version')
    args = parser.parse_args(); work = args.work.resolve()
    if args.palette_from and not args.id:
        parser.error('--palette-from requires --id')
    report = []
    for identity in [args.id] if args.id else IDS:
        record = build(work, identity, args.colors, args.palette_from, args.version); report.append(record)
        print(json.dumps(record, ensure_ascii=False), flush=True)
    (work / ('storage-' + args.id + '.json' if args.id else 'storage.json')).write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')


if __name__ == '__main__': main()
