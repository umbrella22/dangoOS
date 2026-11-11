#!/usr/bin/env python3
"""Add an articulated balance rig to an existing indexed-zlib skin.

The art directory contains body.png, left.png, right.png and rig.json. PNGs use
the existing 352x336 canvas; hand alpha is retained. The original pose table,
palette and compressed pose bytes stay unchanged. v3 appends a body descriptor
and two (image descriptor, local pivot) records, followed by four expressive
face descriptors, to the v2 metadata.
"""
import argparse
import hashlib
import json
import struct
import zlib
from pathlib import Path

from PIL import Image

HEADER = struct.Struct('<8sIHHHHIIII')
DESCRIPTOR = struct.Struct('<hhHHIII')


def build(skin_dir, art_dir):
    binary = (skin_dir / 'fox.bin').read_bytes()
    magic, version, w, h, colors, poses, meta_len, blob_len, _, reserved = HEADER.unpack_from(binary)
    if magic != b'MSKIN01\0' or version != 2 or reserved or (w, h) != (352, 336):
        raise ValueError('input must be an unchanged v2 indexed-zlib skin')
    if len(binary) != HEADER.size + meta_len + blob_len:
        raise ValueError('input skin length')
    if zlib.crc32(binary[HEADER.size:]) != HEADER.unpack_from(binary)[-2]:
        raise ValueError('input skin checksum')
    metadata = bytearray(binary[HEADER.size:HEADER.size + meta_len])
    blob = bytearray(binary[HEADER.size + meta_len:])
    rgb565 = struct.unpack_from('<' + 'H' * colors, metadata)
    palette = []
    for color in rgb565:
        palette.extend((((color >> 11) & 31) * 255 // 31,
                        ((color >> 5) & 63) * 255 // 63, (color & 31) * 255 // 31))
    palette.extend([0] * (768 - len(palette)))
    reference = Image.new('P', (1, 1))
    reference.putpalette(palette)
    spec = json.loads((art_dir / 'rig.json').read_text())
    hashes = {}
    for name in ('body', 'left', 'right', 'strain', 'fall', 'land', 'relief'):
        path = art_dir / (name + '.png')
        source = Image.open(path).convert('RGBA')
        if source.size != (w, h):
            raise ValueError(f'{path}: art canvas')
        hashes[name] = hashlib.sha256(path.read_bytes()).hexdigest()
        box = (0, 0, w, h) if name == 'body' else source.getbbox()
        if box is None:
            raise ValueError(f'{path}: empty hand')
        image = source.crop(box)
        indices = bytearray(image.convert('RGB').quantize(palette=reference, dither=Image.Dither.NONE).tobytes())
        if name != 'body':
            for n, alpha in enumerate(image.getchannel('A').tobytes()):
                if alpha < 128:
                    indices[n] = 255
        # Pillow's fixed palette has unused black slots; map each to a real
        # colour index, including skins whose black entry is not index zero.
        for n, value in enumerate(indices):
            if value != 255 and value >= colors:
                indices[n] = min(range(colors), key=lambda i: sum(v * v for v in palette[i*3:i*3+3]))
        compressed = zlib.compress(bytes(indices), 9)
        metadata.extend(DESCRIPTOR.pack(box[0], box[1], image.width, image.height, len(blob), len(compressed), 0))
        blob.extend(compressed)
        if name in ('left', 'right'):
            pivot = spec[name]['pivot']
            local = (pivot[0] - box[0], pivot[1] - box[1])
            if not (0 <= local[0] < image.width and 0 <= local[1] < image.height):
                raise ValueError(f'{name}: pivot outside hand')
            metadata.extend(struct.pack('<hh', *local))
    payload = metadata + blob
    result = HEADER.pack(magic, 3, w, h, colors, poses, len(metadata), len(blob), zlib.crc32(payload), 0) + payload
    if len(result) > 1024 * 1024:
        raise ValueError('rig exceeds skin transfer limit')
    (skin_dir / 'fox.bin').write_bytes(result)
    info_path = skin_dir / 'skin.json'
    info = json.loads(info_path.read_text())
    info.update(version='2026-11-11-balance-v1', binary_bytes=len(result), binary_crc32=zlib.crc32(result))
    info_path.write_text(json.dumps(info, ensure_ascii=False, indent=2) + '\n')
    return {'id': info['id'], 'bytes_before': len(binary), 'bytes_after': len(result), 'art_sha256': hashes}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--skin-dir', type=Path, required=True)
    parser.add_argument('--art-dir', type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(build(args.skin_dir, args.art_dir), ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
