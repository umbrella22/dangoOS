#!/usr/bin/env python3
"""Isolate the existing spiral ink in PhotoCraft and add the missing eye frames."""
import argparse
import hashlib
import json
import math
import subprocess
from pathlib import Path

import cv2
import numpy as np
from PIL import Image

from refine_articulated_faces import execute
from refine_image26_art import CLI
from refresh_companion_expressions import IDS, SIZE


def spiral_masks(image, identity):
    rgb = np.asarray(image.convert('RGB')).astype('float32')
    if identity == 'eclipse':
        ink = (rgb[:, :, 0] > 60) & (rgb[:, :, 0] > rgb[:, :, 1] * 1.5) & (rgb[:, :, 0] > rgb[:, :, 2] * 1.35)
    elif identity == 'normal':
        ink = (rgb[:, :, 2] > 80) & (rgb[:, :, 2] > rgb[:, :, 0] * 1.25) & (rgb[:, :, 1] > rgb[:, :, 0] * 1.1)
    elif identity == 'black-blue':
        ink = rgb.max(2) < 125
    else:
        raise ValueError('The approved fox animation is retained unchanged')
    yy, xx = np.mgrid[:SIZE[1], :SIZE[0]]
    ink &= (yy > 230) & (yy < 299) & (xx > 90) & (xx < 273)
    count, labels, stats, centers = cv2.connectedComponentsWithStats(ink.astype('uint8'), 8)
    eyes = [i for i in range(1, count)
            if 30 <= stats[i, 2] <= 50 and 25 <= stats[i, 3] <= 48 and 400 <= stats[i, 4] <= 1000]
    eyes.sort(key=lambda i: centers[i, 0])
    if len(eyes) != 2:
        raise ValueError(f'{identity}: expected exactly two isolated spiral strokes, found {len(eyes)}')
    return [((labels == i).astype('uint8') * 255, stats[i, :4].tolist()) for i in eyes]


def build_spiral_groups(work, identity):
    out = work / 'components' / identity
    source = work / 'art' / identity / 'confused-device.png'
    base = Image.open(source).convert('RGB')
    eyes = spiral_masks(base, identity)
    masks = [cv2.dilate(mask, np.ones((3, 3), np.uint8)) for mask, _ in eyes]
    # Remove just the isolated ink and its antialias fringe. Brows, hair and
    # mitten paws are outside these masks and must not rotate with the eyes.
    erase = cv2.dilate(np.maximum.reduce(masks), np.ones((3, 3), np.uint8))
    clean = cv2.inpaint(np.asarray(base), erase, 3, cv2.INPAINT_TELEA)
    plate = out / 'confused-clean-eyes.png'
    Image.fromarray(clean).save(plate)
    parts = []
    records = []
    for n, ((_, (x, y, width, height)), mask) in enumerate(zip(eyes, masks)):
        tile = base.convert('RGBA'); tile.putalpha(Image.fromarray(mask))
        path = out / f'confused-eye-{n}.png'; tile.save(path)
        parts.append({'file': path, 'center': [x + (width - 1) / 2, y + (height - 1) / 2],
                      'box': [x - 4, y - 4, x + width + 4, y + height + 4],
                      'ratio': (width - 1) / (height - 1)})

    groups = {}
    for frame in range(1, 4):
        commands = [('layer.renameLayer', {'name': 'skin-without-old-spirals'})]
        for n, part in enumerate(parts):
            cx, cy = part['center']; ratio = part['ratio']
            angle = -frame * math.pi / 2
            a, d = math.cos(angle), math.cos(angle)
            b, c = math.sin(angle) / ratio, -math.sin(angle) * ratio
            # Rotate inside the original eye ellipse. A circular rotation of
            # a wide eye would reach the bangs or paw on a tilted face.
            commands += [
                ('file.placeEmbedded', {'path': str(part['file']), 'fit': False, 'smartObject': False, 'center': [176, 168]}),
                ('layer.renameLayer', {'name': f'spiral-{frame}-eye-{n}'}),
                ('edit.transform', {'matrix': [a, b, c, d, cx - a * cx - c * cy, cy - b * cx - d * cy],
                                    'interpolation': 'bicubic'}),
                ('layer.layerMask.fromTransparency', {}),
            ]
        project = out / f'confused-spiral-{frame}.pcraft'
        records += execute(CLI, plate, commands, project)
        rendered = out / f'confused-spiral-{frame}-full.png'
        subprocess.run([str(CLI), 'convert', str(project), str(rendered)], check=True, capture_output=True)
        rendered_image = Image.open(rendered).convert('RGB')
        groups[f'spiral-{frame}'] = []
        for n, part in enumerate(parts):
            box = part['box']; filename = f'confused-spiral-{frame}-{n}.png'
            rendered_image.crop(box).save(out / filename)
            groups[f'spiral-{frame}'].append({'file': filename, 'x': box[0], 'y': box[1], 'flags': 0})

    # One reviewable document keeps each animation phase as separate eye
    # layers over the clean plate; only the original phase is initially visible.
    commands = [('layer.renameLayer', {'name': 'skin-without-old-spirals'})]
    for frame in range(4):
        for n, part in enumerate(parts):
            if frame == 0:
                path = part['file']
            else:
                patch = Image.open(out / f'confused-spiral-{frame}-{n}.png').convert('RGBA')
                full = Image.new('RGBA', SIZE); full.paste(patch, part['box'][:2])
                path = out / f'confused-spiral-{frame}-{n}-layer.png'; full.save(path)
            commands += [
                ('file.placeEmbedded', {'path': str(path), 'fit': False, 'smartObject': False, 'center': [176, 168]}),
                ('layer.renameLayer', {'name': f'phase-{frame}-eye-{n}'}),
                ('layer.layerMask.fromTransparency', {}),
            ]
            if frame: commands.append(('layer.hideLayers', {}))
    project = out / 'confused-animation.pcraft'
    records += execute(CLI, plate, commands, project)
    subprocess.run([str(CLI), 'convert', str(project), str(out / 'confused-animation.psd')], check=True, capture_output=True)
    (out / 'confused-animation-receipt.json').write_text(json.dumps({
        'source_sha256': hashlib.sha256(source.read_bytes()).hexdigest(),
        'frame_duration_ms': 100, 'frames': 4,
        'eyes': [{k: v for k, v in p.items() if k != 'file'} for p in parts],
        'photocraft': records,
    }, indent=2) + '\n')
    return groups


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--id', choices=IDS[1:])
    args = parser.parse_args(); work = args.work.resolve()
    for identity in [args.id] if args.id else IDS[1:]:
        groups = build_spiral_groups(work, identity)
        path = work / 'components' / identity / 'manifest.json'
        spec = json.loads(path.read_text())
        spec['poses'].setdefault('confused', {}).update(groups)
        path.write_text(json.dumps(spec, ensure_ascii=False, indent=2) + '\n')
        print(identity, 'PhotoCraft spiral animation complete', flush=True)


if __name__ == '__main__': main()
