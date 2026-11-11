#!/usr/bin/env python3
"""Normalize generated expression cells and retain PhotoCraft contour masks."""
import argparse
import json
import subprocess
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFont

from refresh_companion_expressions import IDS, POSES, EXTRAS, SIZE, LABELS, NAMES, original, fingerprint
from refine_articulated_faces import execute

CLI = Path('/Users/ikaros/Downloads/photocraft-cli-0.21.0-macos-universal/photocraft-cli')


def foreground(image, threshold=14, minimum=4):
    rgb = np.asarray(image.convert('RGB'))
    visible = (rgb.max(2) > threshold).astype('uint8')
    count, labels, stats, _ = cv2.connectedComponentsWithStats(visible, 8)
    visible[:] = 0
    for i in range(1, count):
        if stats[i, cv2.CC_STAT_AREA] >= minimum:
            visible[labels == i] = 255
    return visible


def bbox(mask):
    ys, xs = np.where(mask > 0)
    if not len(xs):
        raise ValueError('Empty generated cell')
    return (int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1)


def refine_cell(work, identity, name, cell, receipt):
    out = work / 'art' / identity
    out.mkdir(parents=True, exist_ok=True)
    done = out / (name + '-receipt.json')
    if done.exists() and json.loads(done.read_text())['generation_sha256'] == receipt['sha256'] and json.loads(done.read_text()).get('normalization_revision') == 2:
        return
    ref = original(identity, name, work)
    source_box = bbox(foreground(cell))
    old = bbox(foreground(ref))
    target = (max(16, old[0]), max(12, old[1]), min(336, old[2]), min(330, old[3]))
    # All semantics, including detached symbols, fit within the original pose's
    # footprint. Side margins accommodate the animator's squash expansion;
    # bottom clearance also keeps a stretched mitten inside the frame.
    normalized = Image.new('RGB', SIZE)
    normalized.paste(cell.crop(source_box).resize((target[2] - target[0], target[3] - target[1]),
                                                 Image.Resampling.LANCZOS), target[:2])
    normalized.save(out / (name + '-registered.png'))
    mask = foreground(normalized, 10, 2)
    mask = cv2.dilate(mask, np.ones((3, 3), np.uint8))
    contours, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    paths = [{'closed': True, 'knots': cv2.approxPolyDP(c, 0.25, True)[:, 0].astype(float).tolist()}
             for c in contours if cv2.contourArea(c) > 2]
    source = out / (name + '-registered.png')
    commands = [
        ('layer.renameLayer', {'name': 'image26-original'}),
        ('layer.hideLayers', {}),
        ('file.placeEmbedded', {'path': str(source.resolve()), 'fit': False,
                                'smartObject': False, 'center': [176, 168]}),
        ('layer.renameLayer', {'name': name + '-clean-contour'}),
        ('path.set', {'name': 'foreground-and-symbols', 'path': {'subpaths': paths}}),
        ('path.toSelection', {'name': 'foreground-and-symbols', 'antiAlias': True, 'feather': 0.3}),
        ('layer.layerMask.revealSelection', {}),
        ('select.deselect', {}),
    ]
    records = execute(CLI, source, commands, out / (name + '.pcraft'))
    subprocess.run([str(CLI), 'convert', str(out / (name + '.pcraft')), str(out / (name + '.png'))],
                   check=True, capture_output=True)
    result = Image.open(out / (name + '.png')).convert('RGBA')
    flat = Image.new('RGB', SIZE)
    flat.paste(result, mask=result.getchannel('A'))
    flat.save(out / (name + '-device.png'))
    done.write_text(json.dumps({'model': receipt['model'], 'generation': receipt['file'],
        'generation_sha256': receipt['sha256'], 'normalization_revision': 2, 'expression': name, 'cell_bounds': source_box,
        'device_bounds': target, 'photocraft': records, 'png_sha256': fingerprint(out / (name + '.png'))},
        ensure_ascii=False, indent=2) + '\n')
    print(identity, name, 'contour refined', flush=True)


def contact_sheet(work, identity):
    out = work / 'art' / identity
    font = ImageFont.truetype('/System/Library/Fonts/STHeiti Light.ttc', 18)
    sheet = Image.new('RGB', (1200, 6 * 253), (18, 21, 28))
    draw = ImageDraw.Draw(sheet)
    for i, name in enumerate(POSES + EXTRAS):
        p = out / (name + '-device.png')
        if not p.exists():
            continue
        x, y = i % 5 * 240, i // 5 * 253
        im = Image.open(p); im.thumbnail((234, 223))
        sheet.paste(im, (x + (240 - im.width) // 2, y + 27))
        draw.text((x + 8, y + 5), LABELS[name], font=font, fill=(230, 231, 235))
    sheet.save(work / (identity + '-image26-all.png'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--generation', choices=['generation', 'finishing'], default='generation')
    parser.add_argument('--id', choices=IDS)
    args = parser.parse_args(); work = args.work.resolve()
    for identity in [args.id] if args.id else IDS:
        for batch in range(5):
            path = work / args.generation / identity / f'{batch:02}-result.json'
            if not path.exists():
                continue
            receipt = json.loads(path.read_text())
            sheet = Image.open(receipt['file']).convert('RGB')
            if sheet.size != (1536, 1024):
                raise ValueError(f'Unexpected sheet dimensions: {sheet.size}')
            for i, name in enumerate(receipt['expressions']):
                cell = sheet.crop((i % 3 * 512, i // 3 * 512, (i % 3 + 1) * 512, (i // 3 + 1) * 512))
                refine_cell(work, identity, name, cell, receipt)
        correction = work / 'dizzy-correction' / 'result.json'
        if args.generation == 'finishing' and identity != 'default' and correction.exists():
            receipt = json.loads(correction.read_text())
            sheet = Image.open(receipt['file']).convert('RGB')
            i = IDS.index(identity) - 1
            refine_cell(work, identity, 'confused', sheet.crop((i * 512, 0, (i + 1) * 512, 512)), receipt)
        contact_sheet(work, identity)


if __name__ == '__main__':
    main()
