#!/usr/bin/env python3
"""Reuse the accepted surprised artwork for the v4 rig's startled face.

PhotoCraft keeps the face mask editable. Only the formerly duplicate relief
descriptor and its new compressed stream change; every accepted image, palette
entry and existing compressed byte is preserved.
"""
import argparse
import hashlib
import json
import struct
import subprocess
import zlib
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFilter

from pack_image26_skins import HEADER, DESC, GROUPS, RIG_PARTS, read_skin
from refine_articulated_faces import execute

IDS = ('default', 'eclipse', 'normal', 'black-blue')
SIZE = (352, 336)


def refine(skins, work, identity, cli, clean_faces):
    source = skins / identity
    out = work / identity
    out.mkdir(parents=True, exist_ok=True)
    data = (source / 'fox.bin').read_bytes()
    fields = list(HEADER.unpack_from(data))
    count, meta_len = fields[4], fields[6]
    meta = bytearray(data[HEADER.size:HEADER.size + meta_len])
    blob = data[HEADER.size + meta_len:]
    palette = list(struct.unpack_from('<' + 'H' * count, meta))
    rgb = [(((c >> 11) & 31) * 255 // 31, ((c >> 5) & 63) * 255 // 63,
            (c & 31) * 255 // 31) for c in palette]
    lut = np.array([(*c, 255) for c in rgb] + [(0, 0, 0, 0)] * (256 - count), np.uint8)
    at = count * 2
    for _ in range(fields[5]):
        at += 2 + DESC.size
        for _ in GROUPS:
            n = struct.unpack_from('<H', meta, at)[0]
            at += 2 + n * DESC.size
    rig = {}
    offsets = {}
    for name in RIG_PARTS:
        offsets[name] = at
        x, y, w, h, off, length, _ = DESC.unpack_from(meta, at)
        raw = np.frombuffer(zlib.decompress(blob[off:off + length]), np.uint8).reshape(h, w)
        rig[name] = (Image.fromarray(lut[raw], 'RGBA'), (x, y))
        at += DESC.size + (4 if name in ('left', 'right', 'ear-left', 'ear-right') else 0)
    assert at == len(meta)
    neutral = Image.new('RGBA', SIZE)
    for name in ('body', 'ear-left', 'ear-right'):
        image, xy = rig[name]
        neutral.alpha_composite(image, xy)
    neutral.save(out / 'neutral-reference.png')
    surprise = read_skin(source / 'fox.bin')['surprised']['base']['image'].convert('RGB')
    surprise.save(out / 'surprised-reference.png')
    reference = Image.new('RGB', SIZE)
    reference.paste(neutral, mask=neutral.getchannel('A'))
    # The surprised poses deliberately raise the ears and elongate the face.
    # Register eye/mouth landmarks rather than letting hair alignment drag the
    # expression below the accepted, rounder neutral face.
    landmarks = {
        'default': ([(133,248),(220,248),(176,290)], [(132,273),(221,273),(177,300)]),
        'eclipse': ([(137,265),(215,265),(176,294)], [(138,273),(218,273),(178,299)]),
        'normal': ([(137,263),(215,263),(176,293)], [(138,273),(218,273),(178,299)]),
        'black-blue': ([(138,269),(215,269),(177,300)], [(137,274),(219,274),(178,300)]),
    }
    before, after = landmarks[identity]
    matrix = cv2.getAffineTransform(np.float32(before), np.float32(after))
    registered = cv2.warpAffine(np.asarray(surprise), matrix, SIZE, flags=cv2.INTER_LINEAR)
    Image.fromarray(registered).save(out / 'startled-registered.png')
    regions = [(97, 242, 174, 296), (180, 242, 259, 296), (161, 282, 196, 312)]
    coverage = Image.new('L', SIZE)
    draw = ImageDraw.Draw(coverage)
    for region in regions:
        draw.rounded_rectangle(region, radius=8, fill=255)
    # Clear the complete previous eye with the existing image-2.6 clean face
    # completion. Copy only isolated new eyes and mouth; Poisson cloning eye
    # ink would wash out its colors and full rectangles would include paws.
    clean = Image.open(clean_faces / identity / 'rig-clean-face-registered.png').convert('RGB')
    edge = coverage.filter(ImageFilter.GaussianBlur(.8))
    backing = Image.composite(clean, reference, edge)
    features = Image.new('L', SIZE)
    draw = ImageDraw.Draw(features)
    cy = 275 if identity == 'black-blue' else 272
    for cx in (134, 219):
        draw.ellipse((cx - 29, cy - 24, cx + 29, cy + 18), fill=255)
    draw.ellipse((166, 287, 190, 312), fill=255)
    merged = Image.composite(Image.fromarray(registered), backing,
                             features.filter(ImageFilter.GaussianBlur(.55)))
    plate = Image.composite(merged, reference, edge).convert('RGBA')
    plate.putalpha(edge.point(lambda v: 255 if v else 0))
    plate.save(out / 'startled-mask.png')
    records = execute(cli, out / 'startled-mask.png', [
        ('layer.renameLayer', {'name': 'startled-clean-face'}),
        ('layer.layerMask.fromTransparency', {}),
    ], out / 'startled.pcraft')
    subprocess.run([str(cli), 'convert', str(out / 'startled.pcraft'), str(out / 'startled.png')],
                   check=True, capture_output=True)
    patch = Image.open(out / 'startled.png').convert('RGBA')
    preview = neutral.copy()
    preview.alpha_composite(patch)
    preview.save(out / 'startled-preview.png')
    box = patch.getbbox()
    patch = patch.crop(box)
    ref = Image.new('P', (1, 1))
    ref.putpalette([c for color in rgb for c in color] + [0] * (768 - count * 3))
    indexed = np.asarray(patch.convert('RGB').quantize(palette=ref, dither=Image.Dither.NONE)).copy()
    indexed[indexed >= count] = 0
    indexed[np.asarray(patch.getchannel('A')) < 128] = 255
    compressed = zlib.compress(indexed.tobytes(), 9)
    old_face = DESC.unpack_from(meta, offsets['relief'])
    reuse = blob[old_face[4]:old_face[4] + old_face[5]] == compressed
    stream_offset = old_face[4] if reuse else len(blob)
    added = b'' if reuse else compressed
    DESC.pack_into(meta, offsets['relief'], box[0], box[1], patch.width, patch.height,
                   stream_offset, len(compressed), 0)
    payload = meta + blob + added
    fields[7] = len(blob) + len(added)
    fields[8] = zlib.crc32(payload)
    result = HEADER.pack(*fields) + payload
    assert len(result) < 1024 * 1024
    assert result[HEADER.size + meta_len:HEADER.size + meta_len + len(blob)] == blob
    original_meta = data[HEADER.size:HEADER.size + meta_len]
    off = offsets['relief']
    assert meta[:off] == original_meta[:off]
    assert meta[off + DESC.size:] == original_meta[off + DESC.size:]
    (out / 'fox.bin').write_bytes(result)
    info = json.loads((source / 'skin.json').read_text())
    info.update(version='2026-11-11-image26-crawl', binary_bytes=len(result),
                binary_crc32=zlib.crc32(result))
    (out / 'skin.json').write_text(json.dumps(info, ensure_ascii=False, indent=2) + '\n')
    # Review the actual RGB565 plate, as it will appear on the device.
    packed = neutral.copy()
    decoded = Image.fromarray(lut[indexed], 'RGBA')
    packed.alpha_composite(decoded, box[:2])
    packed.save(out / 'startled-rgb565.png')
    record = {'id': identity, 'source_sha256': hashlib.sha256(data).hexdigest(),
              'sha256': hashlib.sha256(result).hexdigest(), 'before_bytes': len(data),
              'after_bytes': len(result), 'added_bytes': len(result) - len(data),
              'all_other_streams_and_palette_unchanged': True,
              'landmarks': {'source': before, 'destination': after}, 'matrix': matrix.tolist(),
              'photocraft_commands': records}
    (out / 'receipt.json').write_text(json.dumps(record, ensure_ascii=False, indent=2) + '\n')
    return record


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--skins', type=Path, required=True)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--photocraft', type=Path, required=True)
    parser.add_argument('--clean-faces', type=Path, required=True, help='Existing image-2.6 component completion root')
    args = parser.parse_args()
    args.work.mkdir(parents=True, exist_ok=True)
    records = [refine(args.skins.resolve(), args.work.resolve(), i, args.photocraft.resolve(), args.clean_faces.resolve()) for i in IDS]
    (args.work / 'receipt.json').write_text(json.dumps(records, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps([{k: v for k, v in r.items() if k != 'photocraft_commands'} for r in records], indent=2))
