#!/usr/bin/env python3
"""Register animation edits, refine masks and assemble editable v4 rig parts."""
import argparse
import json
import math
import shutil
import subprocess
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFilter

from refresh_companion_expressions import ROOT, IDS, POSES, MODEL, SIZE
from refine_image26_art import CLI, foreground, bbox
from refine_articulated_faces import execute, stroke
from prepare_articulated_art import paw_path, PAWS
from pack_image26_skins import read_skin
from refine_dizzy_animation import build_spiral_groups


def align(source, reference, identity):
    source = source.convert('RGB'); reference = reference.convert('RGB')
    moving = cv2.cvtColor(np.asarray(source), cv2.COLOR_RGB2GRAY).astype('float32') / 255
    template = cv2.cvtColor(np.asarray(reference), cv2.COLOR_RGB2GRAY).astype('float32') / 255
    mask = np.zeros((336, 352), np.uint8)
    mask[:235] = 255
    matrix = np.eye(2, 3, dtype='float32')
    try:
        score, matrix = cv2.findTransformECC(template, moving, matrix, cv2.MOTION_AFFINE,
            (cv2.TERM_CRITERIA_EPS | cv2.TERM_CRITERIA_COUNT, 250, 1e-6), mask, 5)
    except cv2.error:
        a = bbox(foreground(source)); b = bbox(foreground(reference))
        sx = (a[2] - a[0]) / (b[2] - b[0]); sy = (a[3] - a[1]) / (b[3] - b[1])
        matrix = np.array([[sx, 0, a[0] - b[0] * sx], [0, sy, a[1] - b[1] * sy]], np.float32)
        score, matrix = cv2.findTransformECC(template, moving, matrix, cv2.MOTION_AFFINE,
            (cv2.TERM_CRITERIA_EPS | cv2.TERM_CRITERIA_COUNT, 250, 1e-6), mask, 5)
    if score < 0.65 or not np.isfinite(matrix).all():
        raise ValueError(f'Unreliable {identity} animation registration: {score}')
    result = cv2.warpAffine(np.asarray(source), matrix, SIZE, flags=cv2.INTER_LINEAR | cv2.WARP_INVERSE_MAP)
    return Image.fromarray(result), {'hair_correlation': float(score), 'matrix': matrix.tolist()}


def register_completions(work):
    records = []
    for batch in range(5):
        receipt = json.loads((work / 'animation-completion' / f'{batch:02}-result.json').read_text())
        sheet = Image.open(receipt['file']).convert('RGBA')
        for i, item in enumerate(receipt['cells']):
            identity, pose, edit = item['id'], item['pose'], item['edit']
            x, y = i % 3 * 512, i // 3 * 512
            cell = sheet.crop((x + 36, y + 64, x + 476, y + 484)).resize(SIZE, Image.Resampling.LANCZOS)
            reference = Image.open(work / 'art' / identity / (pose + '-device.png')).convert('RGB')
            aligned, registration = align(cell, reference, identity)
            out = work / 'components' / identity; out.mkdir(parents=True, exist_ok=True)
            aligned.save(out / f'{pose}-{edit}-registered.png')
            records.append({**item, **registration, 'generation': receipt['file']})
    (work / 'animation-registration.json').write_text(json.dumps(records, ensure_ascii=False, indent=2) + '\n')
    for identity in IDS:
        receipt = json.loads((work / 'rig-completion' / identity / '00-result.json').read_text())
        sheet = Image.open(receipt['file']).convert('RGBA')
        reference = Image.open(work / 'art' / identity / 'idle-device.png')
        out = work / 'components' / identity
        for i, name in enumerate(['neutral', 'effort', 'dizzy', 'weary', 'blink', 'clean-face']):
            x, y = i % 3 * 512, i // 3 * 512
            raw_cell = sheet.crop((x, y, x + 512, y + 512))
            raw_path = out / ('rig-' + name + '-source.png'); raw_cell.save(raw_path)
            # A model may paint a halo despite the black-background request.
            # Keep an editable PhotoCraft subject mask before registration, so
            # no background light can be copied into an occlusion repair.
            execute(CLI, raw_path, [('select.subject', {}), ('select.refineEdge', {
                'radius': 1, 'smooth': 1, 'feather': .3, 'shiftEdge': -5,
                'decontaminate': True, 'output': 'layerMask'})], out / ('rig-' + name + '-subject.pcraft'))
            subject_path = out / ('rig-' + name + '-subject.png')
            subprocess.run([str(CLI), 'convert', str(out / ('rig-' + name + '-subject.pcraft')), str(subject_path)], check=True, capture_output=True)
            subject = Image.open(subject_path).convert('RGBA')
            flat = Image.new('RGB', subject.size); flat.paste(subject, mask=subject.getchannel('A'))
            cell = flat.crop((36, 64, 476, 484)).resize(SIZE, Image.Resampling.LANCZOS)
            aligned, registration = align(cell, reference, identity)
            aligned.save(out / ('rig-' + name + '-registered.png'))
            records.append({'id': identity, 'rig_face': name, **registration, 'generation': receipt['file']})
    (work / 'animation-registration.json').write_text(json.dumps(records, ensure_ascii=False, indent=2) + '\n')


def masked_patch(out, name, source, base, regions, coverage=None):
    mask = Image.new('L', SIZE)
    draw = ImageDraw.Draw(mask)
    for region in regions: draw.rounded_rectangle(region, radius=5, fill=255)
    if coverage is not None:
        mask = Image.fromarray(np.minimum(np.asarray(mask), np.asarray(coverage)))
    # Match the surrounding complexion at the seam without carrying old eye
    # pixels into the filled region. Only the source gradients fill the mask.
    ys, xs = np.where(np.asarray(mask) > 0)
    center = ((int(xs.min()) + int(xs.max())) // 2, (int(ys.min()) + int(ys.max())) // 2)
    merged = Image.fromarray(cv2.seamlessClone(np.asarray(source.convert('RGB')),
        np.asarray(base.convert('RGB')), np.asarray(mask), center, cv2.NORMAL_CLONE))
    mask = mask.filter(ImageFilter.GaussianBlur(0.55))
    merged = merged.convert('RGBA'); merged.putalpha(mask.point(lambda a: 255 if a else 0))
    full = out / (name + '-masked.png'); merged.save(full)
    # Convert the baked coverage to an editable PhotoCraft mask. The edge RGB
    # is blended only once, keeping the old iris from shining through a feather.
    commands = [('layer.renameLayer', {'name': name}), ('layer.layerMask.fromTransparency', {})]
    records = execute(CLI, full, commands, out / (name + '.pcraft'))
    subprocess.run([str(CLI), 'convert', str(out / (name + '.pcraft')), str(out / (name + '.png'))], check=True, capture_output=True)
    image = Image.open(out / (name + '.png')).convert('RGBA')
    box = image.getbbox()
    image.crop(box).save(out / (name + '-crop.png'))
    return {'file': name + '-crop.png', 'x': box[0], 'y': box[1], 'flags': 0}, records


def make_pose_parts(work, identity):
    out = work / 'components' / identity; out.mkdir(parents=True, exist_ok=True)
    old = read_skin(work / 'before/skins' / identity / 'fox.bin')
    manifest = {'model': MODEL, 'poses': {}}
    if (out / 'manifest.json').exists():
        existing = json.loads((out / 'manifest.json').read_text())
        for key in ('palette_rgb565', 'skin_version'):
            if key in existing: manifest[key] = existing[key]
    receipts = []
    for pose, item in old.items():
        groups = item['groups']; changes = {}
        base = Image.open(work / 'art' / identity / (pose + '-device.png')).convert('RGB')
        if groups['blink']:
            source = Image.open(out / f'{pose}-blink-registered.png').convert('RGB')
            regions = [(78, 207, 278, 304)] if identity == 'default' else [(92, 216, 175, 294), (182, 216, 263, 294)]
            # Tilted poses need a continuous band across the bridge of the
            # nose, or the inner corner of the previous eye can survive.
            changes['blink'] = []
            for n, region in enumerate(regions):
                patch, records = masked_patch(out, f'{pose}-blink-{n}', source, base, [region])
                changes['blink'].append(patch); receipts.extend(records)
        for group in ['mouth-closed', 'mouth-wide', 'gaze-left', 'gaze-right']:
            if not groups[group]: continue
            source = Image.open(out / f'{pose}-{group}-registered.png').convert('RGB')
            changes[group] = []
            regions = [(78, 207, 278, 296)] if group.startswith('gaze') else [(157, 277, 201, 313)]
            for n, region in enumerate(regions):
                patch, records = masked_patch(out, f'{pose}-{group}-{n}', source, base, [region])
                changes[group].append(patch); receipts.extend(records)
        if groups['ear']:
            changes['ear'] = []
            for n, old_part in enumerate(groups['ear']):
                x, y = old_part['x'], old_part['y']; w, h = old_part['image'].size
                before = np.asarray(item['base']['image'].convert('RGB').crop((x, y, x + w, y + h)))
                after = np.asarray(old_part['image'].convert('RGB'))
                flow = cv2.calcOpticalFlowFarneback(cv2.cvtColor(after, cv2.COLOR_RGB2GRAY),
                    cv2.cvtColor(before, cv2.COLOR_RGB2GRAY), None, 0.5, 4, 21, 4, 5, 1.1, 0)
                yy, xx = np.mgrid[:h, :w].astype('float32')
                current = np.asarray(base.crop((x, y, x + w, y + h)))
                warped = cv2.remap(current, xx + flow[:, :, 0], yy + flow[:, :, 1], cv2.INTER_LINEAR,
                                   borderMode=cv2.BORDER_REPLICATE)
                filename = f'{pose}-ear-{n}.png'; Image.fromarray(warped).save(out / filename)
                changes['ear'].append({'file': filename, 'x': x, 'y': y, 'flags': 0})
        if identity == 'default' and pose == 'confused':
            # Isolate the two continuous red spirals before rotating. A disk
            # crop would also rotate the neighboring brow and eyelash pixels.
            pixels = np.asarray(base).astype('int16')
            red = ((pixels[:, :, 0] > 80) & (pixels[:, :, 0] > pixels[:, :, 1] * 1.7)
                   & (pixels[:, :, 0] > pixels[:, :, 2] * 1.6)).astype('uint8')
            red[:240] = 0; red[:, :75] = 0; red[:, 280:] = 0
            count, labels, stats, centers = cv2.connectedComponentsWithStats(red, 8)
            eyes = [i for i in range(1, count) if 300 < stats[i, 4] < 700]
            eyes.sort(key=lambda i: centers[i, 0])
            if len(eyes) != 2: raise ValueError('Expected two separate fox spiral strokes')
            for frame in range(1, 4):
                parts = []
                for n, label in enumerate(eyes):
                    x, y, w, h, _ = [int(v) for v in stats[label]]; cx, cy = x + w // 2, y + h // 2
                    radius = max(w, h) // 2 + 4; box = (cx - radius, cy - radius, cx + radius + 1, cy + radius + 1)
                    mask = ((labels == label) * 255).astype('uint8')
                    mask = cv2.dilate(mask, np.ones((3, 3), np.uint8))
                    clean = cv2.inpaint(np.asarray(base), mask, 3, cv2.INPAINT_TELEA)
                    tile = base.crop(box).convert('RGBA'); tile.putalpha(Image.fromarray(mask).crop(box))
                    rotated = tile.rotate(frame * 90, Image.Resampling.BICUBIC)
                    part = Image.fromarray(clean).crop(box).convert('RGBA'); part.alpha_composite(rotated)
                    filename = f'confused-spiral-{frame}-{n}.png'; part.save(out / filename)
                    parts.append({'file': filename, 'x': box[0], 'y': box[1], 'flags': 0})
                changes[f'spiral-{frame}'] = parts
        if identity != 'default' and pose == 'confused':
            changes.update(build_spiral_groups(work, identity))
        if changes: manifest['poses'][pose] = changes
    (out / 'manifest.json').write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + '\n')
    (out / 'pose-refinement-receipt.json').write_text(json.dumps(receipts, ensure_ascii=False, indent=2) + '\n')


def detect_paws(base, identity):
    edges = cv2.Canny(np.asarray(base.convert('RGB')), 50, 110)
    edges[:280] = 0
    contours, _ = cv2.findContours(edges, cv2.RETR_LIST, cv2.CHAIN_APPROX_SIMPLE)
    ellipses = [cv2.fitEllipse(c) for c in contours if len(c) >= 10]
    found = []
    for expected in PAWS[identity]:
        cx, cy, rx, ry = expected
        candidates = [e for e in ellipses if abs(e[0][0] - cx) < 18 and abs(e[0][1] - cy) < 15
                      and e[0][1] > 294 and all(33 < v < 70 for v in e[1])]
        if candidates:
            (x, y), (w, h), angle = min(candidates, key=lambda e: (e[0][0] - cx) ** 2 + (e[0][1] - cy) ** 2)
            found.append((float(x), float(y), max(w, h) / 2 + .8, min(w, h) / 2 + .8))
        else: raise ValueError('No reliable lower-paw contour for ' + identity)
    return found


def make_rig(work, identity):
    out = work / 'components' / identity; art = work / 'art' / identity
    base = Image.open(art / 'idle-device.png').convert('RGB')
    completion = Image.open(out / 'rig-neutral-registered.png').convert('RGB')
    paws = detect_paws(base, 'normal' if identity == 'default' else identity)
    body = completion.convert('RGBA')
    mask = foreground(body, 12, 20)
    contours, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    mask[:] = 0; cv2.drawContours(mask, [max(contours, key=cv2.contourArea)], -1, 255, -1)
    body.putalpha(Image.fromarray(mask)); body.save(out / 'neutral.png')
    spec = {'model': MODEL, 'canvas': SIZE, 'parts': {}, 'faces': {'strain': 'effort', 'fall': 'dizzy',
        'land': 'weary', 'relief': 'weary', 'blink': 'blink'}, 'completion': 'image26 occluded head and local animation edits'}
    commands = [('layer.renameLayer', {'name': 'idle-reference'})]
    for name, shape in zip(['left', 'right'], paws):
        points = paw_path(base, shape)
        commands += [('layer.select', {'name': 'idle-reference'}),
            ('path.set', {'name': name + '-contour', 'path': {'subpaths': [{'closed': True, 'knots': points}]}}),
            ('path.toSelection', {'name': name + '-contour', 'antiAlias': True}),
            ('layer.new.layerViaCopy', {}), ('layer.renameLayer', {'name': name}), ('select.deselect', {})]
        spec['parts'][name] = {'pivot': [round(shape[0]), round(shape[1])], 'contour': points}
    commands += [('layer.select', {'name': 'idle-reference'}), ('layer.hideLayers', {}),
        ('file.export.layersToFiles', {'dir': str(out / 'paws'), 'format': 'png', 'trim': False})]
    receipts = execute(CLI, art / 'idle-device.png', commands, out / 'paws.pcraft')
    for name in ['left', 'right']:
        shutil.copyfile(next((out / 'paws').glob('*_' + name + '.png')), out / (name + '.png'))
    x0, y0, x1, y1 = body.getbbox(); w, h = x1 - x0, y1 - y0
    ear_polygons = [
        [(x0 - 2, y0 - 2), (x0 + w * .32, y0 + h * .16), (x0 + w * .22, y0 + h * .30), (x0 + w * .11, y0 + h * .49), (x0 - 2, y0 + h * .46)],
        [(x1 + 2, y0 - 2), (x1 - w * .32, y0 + h * .16), (x1 - w * .22, y0 + h * .30), (x1 - w * .11, y0 + h * .49), (x1 + 2, y0 + h * .46)]]
    neutral = body.copy()
    for name, polygon in zip(['ear-left', 'ear-right'], ear_polygons):
        mask = Image.new('L', SIZE); ImageDraw.Draw(mask).polygon(polygon, fill=255)
        ear = neutral.copy(); ear.putalpha(Image.fromarray(np.minimum(np.asarray(mask.filter(ImageFilter.MaxFilter(11))), np.asarray(neutral.getchannel('A')))))
        ear.save(out / (name + '.png'))
        alpha = np.asarray(body.getchannel('A')).copy(); alpha[np.asarray(mask) > 0] = 0
        body.putalpha(Image.fromarray(alpha))
        b = ear.getbbox(); spec['parts'][name] = {'pivot': [round((b[0] + b[2]) / 2), b[3] - 4]}
    body.save(out / 'body.png')
    regions = [(90, 223, 175, 293), (179, 223, 267, 293), (160, 290, 197, 312)]
    spec['face_regions'] = regions
    if identity != 'default':
        # Match the approved fox's clean spiral-eye language. Drawing on the
        # completely blank image26 skin plate cannot retain an old lash/iris.
        ink = '#762334' if identity == 'eclipse' else '#353047'
        commands = [('layer.renameLayer', {'name': 'clean-face-and-hair'})]
        for n, (cx, cy) in enumerate([(136, 270), (216, 270)]):
            spiral = []
            for t in np.linspace(0, 1, 161):
                angle = t * math.tau * 2.1
                radius = 1 + t * 15
                spiral.append((cx + radius * math.cos(angle), cy + radius * math.sin(angle)))
            commands += stroke('clean-dizzy-spiral-' + str(n), spiral, ink, 2.5)
        receipts += execute(CLI, out / 'rig-clean-face-registered.png', commands, out / 'rig-dizzy-corrected.pcraft')
        subprocess.run([str(CLI), 'convert', str(out / 'rig-dizzy-corrected.pcraft'),
                        str(out / 'rig-dizzy-corrected.png')], check=True, capture_output=True)
    face_records = []
    for name, pose in [('strain', 'effort'), ('fall', 'dizzy'), ('land', 'weary'), ('blink', 'blink')]:
        source_path = out / ('rig-' + pose + '-registered.png')
        source = Image.open(source_path).convert('RGB')
        registration = {'registered_file': str(source_path)}
        face_records.append({'face': name, **registration})
        assembled = Image.new('RGBA', SIZE)
        for feature, region in zip(['eye-left', 'eye-right', 'mouth'], regions):
            # A blink changes only the eyes; it does not switch the mouth.
            image = base if pose == 'blink' and feature == 'mouth' else source
            if pose == 'dizzy' and identity != 'default' and feature != 'mouth':
                image = Image.open(out / 'rig-dizzy-corrected.png').convert('RGB')
            _, records = masked_patch(out, name + '-' + feature, image, neutral, [region]); receipts.extend(records)
            assembled.alpha_composite(Image.open(out / (name + '-' + feature + '.png')).convert('RGBA'))
        assembled.save(out / (name + '.png'))
    for suffix in ['', '-eye-left', '-eye-right', '-mouth']:
        shutil.copyfile(out / ('land' + suffix + '.png'), out / ('relief' + suffix + '.png'))
    (out / 'rig.json').write_text(json.dumps(spec, ensure_ascii=False, indent=2) + '\n')
    layers = ['ear-left', 'ear-right'] + [state + '-' + part for state in ['strain', 'fall', 'land', 'relief', 'blink'] for part in ['eye-left', 'eye-right', 'mouth']] + ['left', 'right']
    commands = [('layer.renameLayer', {'name': 'body'})]
    for name in layers:
        commands += [('file.placeEmbedded', {'path': str(out / (name + '.png')), 'fit': False,
            'smartObject': False, 'center': [176, 168]}), ('layer.renameLayer', {'name': name})]
        if name.startswith(('strain', 'fall', 'land', 'relief', 'blink')): commands.append(('layer.hideLayers', {}))
    receipts += execute(CLI, out / 'body.png', commands, out / (identity + '-articulated.pcraft'))
    subprocess.run([str(CLI), 'convert', str(out / (identity + '-articulated.pcraft')), str(out / (identity + '-articulated.psd'))], check=True, capture_output=True)
    (out / 'rig-refinement-receipt.json').write_text(json.dumps({'commands': receipts, 'faces': face_records, 'paws': paws}, ensure_ascii=False, indent=2) + '\n')
    # A separate expression document keeps all 25 full poses individually editable.
    commands = [('layer.renameLayer', {'name': 'idle'})]
    for pose in POSES[1:]:
        commands += [('file.placeEmbedded', {'path': str(art / (pose + '.png')), 'fit': False,
            'smartObject': False, 'center': [176, 168]}), ('layer.renameLayer', {'name': pose}), ('layer.hideLayers', {})]
    execute(CLI, art / 'idle.png', commands, out / (identity + '-expressions.pcraft'))
    subprocess.run([str(CLI), 'convert', str(out / (identity + '-expressions.pcraft')), str(out / (identity + '-expressions.psd'))], check=True, capture_output=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--skip-registration', action='store_true')
    args = parser.parse_args(); work = args.work.resolve()
    if not args.skip_registration:
        register_completions(work)
    for identity in IDS:
        make_pose_parts(work, identity); make_rig(work, identity)
        print(identity, 'animation components and PhotoCraft projects complete', flush=True)


if __name__ == '__main__': main()
