#!/usr/bin/env python3
"""Match spiral shapes to the approved fox and isolate their animation layers."""
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


def reference_eyes(work, rig=False):
    path = (work / 'components/default/fall.png' if rig else
            work / 'art/default/confused-device.png')
    rgb = np.asarray(Image.open(path).convert('RGB')).astype('float32')
    ink = ((rgb[:, :, 0] > 40) & (rgb[:, :, 0] > rgb[:, :, 1] * 1.7)
           & (rgb[:, :, 0] > rgb[:, :, 2] * 1.6)).astype('uint8')
    ink[:245 if rig else 240] = 0
    ink[292 if rig else 300:] = 0
    ink[:, :95 if rig else 75] = 0; ink[:, 260 if rig else 280:] = 0
    count, labels, stats, _ = cv2.connectedComponentsWithStats(ink, 8)
    eyes = sorted([i for i in range(1, count) if 300 < stats[i, 4] < 700],
                  key=lambda i: stats[i, 0])
    if len(eyes) != 2:
        raise ValueError('Expected two approved fox spiral shapes')
    result = []
    for i in eyes:
        mask = (labels == i).astype('uint8') * 255
        support = cv2.dilate(mask, np.ones((3, 3), np.uint8))
        skin = cv2.inpaint(rgb.astype('uint8'), support, 3, cv2.INPAINT_TELEA).astype('float32')
        dark = float(np.percentile(rgb[:, :, 1][mask > 0], 15))
        alpha = np.clip((skin[:, :, 1] - rgb[:, :, 1]) /
                        np.maximum(skin[:, :, 1] - dark, 1), 0, 1)
        alpha[support == 0] = 0
        x, y, w, h = stats[i, :4]
        result.append(Image.fromarray((alpha * 255).astype('uint8')).crop(
            (int(x - 2), int(y - 2), int(x + w + 2), int(y + h + 2))))
    return result


def place_reference_eyes(work, identity, plate, centers, project, rig=False):
    out = work / 'components' / identity
    color = (135, 39, 53) if identity == 'eclipse' else (53, 48, 71)
    commands = [('layer.renameLayer', {'name': 'clean-face-without-old-eyes'})]
    for n, (alpha, (cx, cy)) in enumerate(zip(reference_eyes(work, rig), centers)):
        tile = Image.new('RGBA', alpha.size, (*color, 255)); tile.putalpha(alpha)
        full = Image.new('RGBA', SIZE)
        full.alpha_composite(tile, (round(cx - alpha.width / 2), round(cy - alpha.height / 2)))
        path = out / f'{project}-eye-{n}.png'; full.save(path)
        commands += [
            ('file.placeEmbedded', {'path': str(path), 'fit': False, 'smartObject': False, 'center': [176, 168]}),
            ('layer.renameLayer', {'name': f'fox-style-spiral-{n}'}),
            ('layer.layerMask.fromTransparency', {}),
        ]
    records = execute(CLI, plate, commands, out / (project + '.pcraft'))
    result = out / (project + '.png')
    subprocess.run([str(CLI), 'convert', str(out / (project + '.pcraft')), str(result)],
                   check=True, capture_output=True)
    (out / (project + '-receipt.json')).write_text(json.dumps({
        'reference': 'default/fall' if rig else 'default/confused',
        'eye_centers': centers, 'photocraft': records,
    }, indent=2) + '\n')
    return result


def normalize_spirals(work, identity):
    out = work / 'components' / identity
    source = work / 'art' / identity / 'confused-device.png'
    original = out / 'confused-style-source.png'
    if not original.exists():
        original.write_bytes(source.read_bytes())
    base = Image.open(original).convert('RGB')
    eyes = spiral_masks(base, identity)
    plate = out / 'confused-style-clean.png'
    repair = out / 'confused-eye-cleanup.png'
    if not repair.exists():
        raise ValueError('Prepare the image 2.6 clean eye plate with --clean-sheet first')
    # Diffusion from nearby red/blue/black hair creates triangular stains.
    # Use the reviewed generated skin plate, with a local PhotoCraft mask.
    plate.write_bytes(repair.read_bytes())
    # Preserve the face's tilted eye line, with a little more space from the
    # bangs and slightly closer-set eyes to match the approved proportions.
    centers = [(x + (w - 1) / 2 + (2 if n == 0 else -2), y + (h - 1) / 2 + 2)
               for n, (_, (x, y, w, h)) in enumerate(eyes)]
    result = place_reference_eyes(work, identity, plate, centers, 'confused-style')
    Image.open(result).convert('RGB').save(source)
    Image.open(result).save(source.with_name('confused.png'))


def rig_spiral_plate(work, identity):
    out = work / 'components' / identity
    return place_reference_eyes(work, identity, out / 'rig-clean-face-registered.png',
                               [(136, 273), (216, 273)], 'rig-dizzy-corrected', rig=True)


def prepare_clean_plates(work, sheet_path):
    from build_image26_components import align
    sheet = Image.open(sheet_path).convert('RGB')
    if sheet.size != (1536, 1024):
        raise ValueError('Expected the three-sprite cleanup sheet at 1536x1024')
    for i, identity in enumerate(IDS[1:]):
        out = work / 'components' / identity
        original = out / 'confused-style-source.png'
        base = Image.open(original).convert('RGB')
        source = sheet.crop((i * 512 + 36, 64, i * 512 + 476, 484)).resize(SIZE, Image.Resampling.LANCZOS)
        aligned, registration = align(source, base, identity)
        eyes = spiral_masks(base, identity)
        # Replace the whole old eye disk, including its pale outer stroke.
        # Painting only the dark spiral leaves a white spiral-shaped halo.
        erase = np.zeros((SIZE[1], SIZE[0]), np.uint8)
        for _, (x, y, width, height) in eyes:
            cv2.ellipse(erase, (round(x + (width - 1) / 2), round(y + (height - 1) / 2)),
                        (round(width / 2 + 4), round(height / 2 + 4)), 0, 0, 360, 255, -1)
        rgb = np.asarray(base).astype('float32'); replacement = np.asarray(aligned).astype('float32')
        yy, xx = np.mgrid[:SIZE[1], :SIZE[0]]
        # Match the plate to existing pale peach skin, excluding hair, eye ink,
        # outlines and the lower cheek shadow from the color samples.
        samples = ((yy > 238) & (yy < 292) & (xx > 108) & (xx < 253) & (erase == 0)
                   & (rgb[:, :, 0] - rgb[:, :, 1] > 12)
                   & (rgb[:, :, 1] - rgb[:, :, 2] > 2) & (rgb[:, :, 2] > 140))
        if samples.sum() < 100:
            raise ValueError('Not enough clean skin samples for ' + identity)
        correction = np.median((rgb - replacement)[samples], axis=0)
        replacement = np.clip(replacement + correction, 0, 255).astype('uint8')
        alpha = cv2.GaussianBlur(erase, (5, 5), .8)
        tile = Image.fromarray(replacement).convert('RGBA'); tile.putalpha(Image.fromarray(alpha))
        path = out / 'confused-eye-repair-layer.png'; tile.save(path)
        commands = [
            ('layer.renameLayer', {'name': 'original-expression'}),
            ('file.placeEmbedded', {'path': str(path), 'fit': False, 'smartObject': False, 'center': [176, 168]}),
            ('layer.renameLayer', {'name': 'image26-clean-skin-only'}),
            ('layer.layerMask.fromTransparency', {}),
        ]
        project = out / 'confused-eye-cleanup.pcraft'
        records = execute(CLI, original, commands, project)
        subprocess.run([str(CLI), 'convert', str(project), str(out / 'confused-eye-cleanup.png')], check=True, capture_output=True)
        (out / 'confused-eye-cleanup-receipt.json').write_text(json.dumps({
            'model': 'gpt-image-2.6-sunburst', 'sheet_sha256': hashlib.sha256(sheet_path.read_bytes()).hexdigest(),
            'registration': registration, 'skin_color_correction': correction.tolist(), 'photocraft': records,
        }, indent=2) + '\n')


def spiral_masks(image, identity):
    rgb = np.asarray(image.convert('RGB')).astype('float32')
    if identity == 'eclipse':
        ink = (rgb[:, :, 0] > 60) & (rgb[:, :, 0] > rgb[:, :, 1] * 1.5) & (rgb[:, :, 0] > rgb[:, :, 2] * 1.35)
    elif identity == 'normal':
        ink = ((rgb[:, :, 2] > 80) & (rgb[:, :, 2] > rgb[:, :, 0] * 1.25) & (rgb[:, :, 1] > rgb[:, :, 0] * 1.1)) | (rgb.max(2) < 125)
    elif identity == 'black-blue':
        ink = rgb.max(2) < 125
    else:
        raise ValueError('The approved fox animation is retained unchanged')
    yy, xx = np.mgrid[:SIZE[1], :SIZE[0]]
    ink &= (yy > 230) & (yy < 299) & (xx > 90) & (xx < 273)
    count, labels, stats, centers = cv2.connectedComponentsWithStats(ink.astype('uint8'), 8)
    eyes = [i for i in range(1, count)
            if 28 <= stats[i, 2] <= 50 and 23 <= stats[i, 3] <= 48 and 280 <= stats[i, 4] <= 1000]
    eyes.sort(key=lambda i: centers[i, 0])
    if len(eyes) != 2:
        raise ValueError(f'{identity}: expected exactly two isolated spiral strokes, found {len(eyes)}')
    return [((labels == i).astype('uint8') * 255, stats[i, :4].tolist()) for i in eyes]


def build_spiral_groups(work, identity):
    normalize_spirals(work, identity)
    out = work / 'components' / identity
    source = work / 'art' / identity / 'confused-device.png'
    plate = out / 'confused-clean-eyes.png'
    plate.write_bytes((out / 'confused-style-clean.png').read_bytes())
    parts = []
    records = []
    for n in range(2):
        tile = Image.open(out / f'confused-style-eye-{n}.png').convert('RGBA')
        x, y, right, bottom = tile.getbbox()
        width, height = right - x, bottom - y
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
    parser.add_argument('--clean-sheet', type=Path, help='Register a reviewed image 2.6 blank eye plate sheet')
    args = parser.parse_args(); work = args.work.resolve()
    if args.clean_sheet:
        prepare_clean_plates(work, args.clean_sheet.resolve())
    for identity in [args.id] if args.id else IDS[1:]:
        groups = build_spiral_groups(work, identity)
        path = work / 'components' / identity / 'manifest.json'
        spec = json.loads(path.read_text())
        spec['poses'].setdefault('confused', {}).update(groups)
        spec['skin_version'] = '2026-11-11-image26-v4-clean'
        path.write_text(json.dumps(spec, ensure_ascii=False, indent=2) + '\n')
        # Rebuild the fall plates and editable rig with the same approved ink.
        from build_image26_components import make_rig
        make_rig(work, identity)
        print(identity, 'PhotoCraft spiral animation complete', flush=True)


if __name__ == '__main__': main()
