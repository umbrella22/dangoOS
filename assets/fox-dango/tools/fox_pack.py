"""Pack device/ (poses, parts, manifest) into firmware tables.

Usage: python3 assets/fox-dango/tools/fox_pack.py
Writes crates/os/src/assets/fox.bin (row RLE of palette indices) and
crates/os/src/fox/data.rs (palette and image tables). Run after
fox_parts.py; both outputs are committed, so the firmware build needs no Python.

RLE: every image row is a sequence of (count 1..=255, index) byte pairs that
sums to the image width. Index 255 is transparent (sprites only).
"""
import argparse
import struct
import zlib
import re
import json
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
DEV = ROOT / 'device'
CRATE_SRC = ROOT.parent.parent / 'crates' / 'os' / 'src'
OUT = CRATE_SRC / 'fox'          # data.rs (Rust tables)
BLOB_OUT = CRATE_SRC / 'assets'  # fox.bin (binary resources)
TRANSPARENT = 255

# Part name -> table field. Symbols of every pose go to `symbols`.
PATCH_FIELDS = {'blink': 'blink', 'mouth_closed': 'mouth_closed', 'mouth_wide': 'mouth_wide',
                'ear_flick': 'ear', 'ear_turn': 'ear', 'gaze_left': 'gaze_left', 'gaze_right': 'gaze_right'}


def indices(path, sprite, lut):
    a = np.asarray(Image.open(DEV / path).convert('RGBA')).astype(int)
    v = ((a[..., 0] >> 3) << 11) | ((a[..., 1] >> 2) << 5) | (a[..., 2] >> 3)
    flat = np.array([lut.get(int(c), -1) for c in v.ravel()]).reshape(v.shape)
    if sprite:
        flat[a[..., 3] < 128] = TRANSPARENT
    assert (flat >= 0).all(), f'{path}: colour outside the palette'
    return flat.astype(np.uint8)


def rle(a):
    out = bytearray()
    for row in a:
        i = 0
        while i < len(row):
            j = i
            while j < len(row) and row[j] == row[i] and j - i < 255:
                j += 1
            out += bytes((j - i, int(row[i])))
            i = j
    return bytes(out)


def main():
    global DEV
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--device', type=Path, default=DEV, help='device art directory')
    parser.add_argument('--skin-dir', type=Path, help='write standalone skin package instead of firmware tables')
    parser.add_argument('--id', help='skin identity, required with --skin-dir')
    parser.add_argument('--name', default='Fox skin')
    parser.add_argument('--version', default='1')
    parser.add_argument('--palette-xor', type=lambda v: int(v, 0), default=0,
                        help='RGB565 palette XOR for a deterministic recolor package')
    args = parser.parse_args()
    if args.skin_dir and (not args.id or not re.fullmatch(r'[A-Za-z0-9_-]{1,63}', args.id) or args.id == 'default'):
        parser.error('--skin-dir needs an exact valid non-default --id')
    if not 0 <= args.palette_xor <= 65535 or (args.palette_xor and not args.skin_dir):
        parser.error('--palette-xor must be uint16 and requires --skin-dir')
    DEV = args.device
    m = json.load(open(DEV / 'manifest.json'))
    W, H = m['canvas']
    pal = m['palette_rgb565']
    lut = {c: i for i, c in enumerate(pal)}
    blob = bytearray()
    packed_poses = []
    images = {}

    def image(item, sprite):
        data = rle(indices(item['file'], sprite, lut))
        off = len(blob)
        blob.extend(data)
        a = Image.open(DEV / item['file'])
        x, y = item.get('x', 0), item.get('y', 0)
        bg = 'true' if item.get('over') == 'bg' else 'false'
        text = f'Img {{ x: {x}, y: {y}, w: {a.width}, h: {a.height}, off: {off}, len: {len(data)}, over_bg: {bg} }}'
        images[text] = struct.pack('<hhHHIII', x, y, a.width, a.height, off, len(data), int(bg == 'true'))
        return text

    poses = []
    for key, p in m['poses'].items():
        fields = {f: [] for f in ('blink', 'mouth_closed', 'mouth_wide', 'ear', 'gaze_left', 'gaze_right',
                                  'symbols', 'tremble')}
        spiral = {1: [], 2: [], 3: []}
        for name, items in p['parts'].items():
            for it in items:
                if name == 'spiral':
                    spiral[it['frame']].append(image(it, False))
                elif name in PATCH_FIELDS:
                    fields[PATCH_FIELDS[name]].append(image(it, False))
                elif name == 'tremble':
                    fields['tremble'].append(image(it, True))
                else:
                    fields['symbols'].append(image(it, True))
        base = image({'file': p['file']}, False)
        lines = [f'    // {key}', '    Pose {', f'        base: {base},']
        for f, imgs in fields.items():
            lines.append(f'        {f}: &[{", ".join(imgs)}],')
        lines.append('        spiral: [' + ', '.join(f'&[{", ".join(spiral[n])}]' for n in (1, 2, 3)) + '],')
        lines.append('    },')
        poses.append('\n'.join(lines))
        packed_poses.append((base, list(fields.values()) + [spiral[n] for n in (1, 2, 3)]))

    if args.skin_dir:
        expected = ['idle', 'sleep', 'curious', 'happy', 'puzzled', 'confused', 'surprised',
                    'panic', 'angry', 'satisfied', 'listening', 'thinking', 'speaking', 'error',
                    'tired', 'dying', 'annoyed', 'charging', 'peering', 'dazzled']
        # Manifest pose keys are checked rather than silently mapped by dictionary order.
        actual = list(m['poses'])
        if [key.lower() for key in actual] != expected:
            parser.error('skin requires all 20 poses in canonical order: ' + ','.join(expected))
        meta = bytearray(struct.pack('<' + 'H' * len(pal), *[c ^ args.palette_xor for c in pal]))
        for index, (base, groups) in enumerate(packed_poses):
            meta += struct.pack('<H', index) + images[base]
            for group in groups:
                meta += struct.pack('<H', len(group))
                for img in group:
                    meta += images[img]
        body = bytes(meta + blob)
        binary = struct.pack('<8sIHHHHIIII', b'MSKIN01\0', 1, W, H, len(pal), len(poses),
                             len(meta), len(blob), zlib.crc32(body), 0) + body
        if len(binary) > 1024 * 1024:
            parser.error('skin binary exceeds protocol 1 MiB file limit')
        args.skin_dir.mkdir(parents=True, exist_ok=True)
        (args.skin_dir / 'fox.bin').write_bytes(binary)
        (args.skin_dir / 'skin.json').write_text(json.dumps({
            'schema_version': 1, 'id': args.id, 'name': args.name, 'version': args.version,
            'canvas': [W, H], 'binary_bytes': len(binary), 'binary_crc32': zlib.crc32(binary),
        }, ensure_ascii=False, indent=2) + '\n')
        print(f'skin {args.id}: {len(poses)} poses, {len(binary)} bytes -> {args.skin_dir}')
        return

    OUT.mkdir(parents=True, exist_ok=True)
    BLOB_OUT.mkdir(parents=True, exist_ok=True)
    (BLOB_OUT / 'fox.bin').write_bytes(bytes(blob))
    rs = f'''// @generated by assets/fox-dango/tools/fox_pack.py from assets/fox-dango/device/.
// Do not edit by hand; rerun the packer after changing the art.

use super::{{Img, Pose}};

/// Canvas width in pixels.
pub const W: usize = {W};
/// Canvas height in pixels.
pub const H: usize = {H};

/// Shared RGB565 palette; index 255 in the RLE stream is transparent.
pub static PALETTE: [u16; {len(pal)}] = [{", ".join(f"0x{c:04x}" for c in pal)}];

/// Row RLE of every image, concatenated ({len(blob)} bytes).
pub static BLOB: &[u8] = include_bytes!("../assets/fox.bin");

/// One entry per [`super::FoxPose`], in declaration order.
pub static POSES: [Pose; {len(poses)}] = [
{chr(10).join(poses)}
];
'''
    (OUT / 'data.rs').write_text(rs)
    print(f'{len(poses)} poses, {len(blob) / 1024:.1f} KiB: {BLOB_OUT}/fox.bin + {OUT}/data.rs')


if __name__ == '__main__':
    main()
