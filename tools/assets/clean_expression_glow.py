#!/usr/bin/env python3
"""Remove generated background lighting with PhotoCraft subject/edge masks."""
import argparse
import json
import math
import subprocess
from pathlib import Path

import cv2
import numpy as np
from PIL import Image

from refine_articulated_faces import execute, stroke
from refine_image26_art import CLI, foreground, bbox, contact_sheet
from refresh_companion_expressions import original, fingerprint


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--work', type=Path, required=True)
    args = parser.parse_args(); work = args.work.resolve()
    folder = work / 'glow-cleanup'; folder.mkdir(exist_ok=True)
    receipt = json.loads((work / 'generation/eclipse/03-result.json').read_text())
    sheet = Image.open(receipt['file']).convert('RGBA')
    for i, name in enumerate(receipt['expressions']):
        source = folder / (name + '-source.png')
        sheet.crop((i % 3 * 512, i // 3 * 512, (i % 3 + 1) * 512, (i // 3 + 1) * 512)).save(source)
        commands = [('select.subject', {}), ('select.refineEdge', {'radius': 1, 'smooth': 1,
            'feather': .3, 'shiftEdge': -5, 'decontaminate': True, 'output': 'layerMask'})]
        records = execute(CLI, source, commands, folder / (name + '-subject.pcraft'))
        subprocess.run([str(CLI), 'convert', str(folder / (name + '-subject.pcraft')), str(folder / (name + '-subject.png'))], check=True, capture_output=True)
        image = Image.open(folder / (name + '-subject.png')).convert('RGBA')
        alpha = np.asarray(image.getchannel('A')).copy()
        count, labels, stats, _ = cv2.connectedComponentsWithStats((alpha > 100).astype('uint8'), 8)
        biggest = 1 + int(np.argmax(stats[1:, cv2.CC_STAT_AREA]))
        keep = cv2.dilate((labels == biggest).astype('uint8'), np.ones((3, 3), np.uint8))
        alpha[keep == 0] = 0; image.putalpha(Image.fromarray(alpha))
        ref = original('eclipse', name, work)
        mask = foreground(ref)
        contours, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
        mask[:] = 0; cv2.drawContours(mask, [max(contours, key=cv2.contourArea)], -1, 255, -1)
        target = bbox(mask)
        target = (max(3, target[0]), max(3, target[1]), min(349, target[2]), min(333, target[3]))
        placed = Image.new('RGBA', (352, 336))
        placed.paste(image.crop(image.getbbox()).resize((target[2] - target[0], target[3] - target[1]), Image.Resampling.LANCZOS), target[:2])
        body = folder / (name + '-body.png'); placed.save(body)
        commands = [('layer.renameLayer', {'name': 'matte-character'})]
        if name == 'dazzled':
            for n, points in enumerate([[(278, 201), (284, 188)], [(286, 207), (298, 198)], [(290, 217), (304, 215)]]):
                commands += stroke(f'light-reaction-{n}', points, '#ffc09b', 5)
        elif name.startswith(('network', 'bluetooth')):
            connected = name.endswith('-connected')
            color = '#85b9ff' if connected else '#c9b8da'
            cx, cy = 176, 69
            if name.startswith('network'):
                for radius in [16, 29, 42]:
                    points = [(cx + radius * math.cos(t), cy + radius * math.sin(t)) for t in np.linspace(-2.30, -.84, 40)]
                    commands += stroke('wifi-' + str(radius), points, color, 3.2)
                commands += stroke('wifi-dot', [(cx, cy), (cx + .1, cy)], color, 6)
            else:
                commands += stroke('bluetooth', [(176, 26), (192, 40), (161, 63), (176, 74), (176, 26), (192, 60), (161, 39)], color, 3)
            points = [(215 + 10 * math.cos(t), 66 + 10 * math.sin(t)) for t in np.linspace(0, math.tau, 61)]
            commands += [('path.set', {'name': 'status-badge', 'path': {'subpaths': [{'closed': True, 'knots': points}]}}),
                         ('path.fill', {'name': 'status-badge', 'color': '#fff5f5'})]
            paths = [[(210, 66), (214, 70), (220, 62)]] if connected else [[(211, 62), (219, 70)], [(219, 62), (211, 70)]]
            for n, points in enumerate(paths): commands += stroke('status-' + str(n), points, '#4d7cbb' if connected else '#c74e66', 2.3)
        art = work / 'art/eclipse'
        records += execute(CLI, body, commands, art / (name + '.pcraft'))
        subprocess.run([str(CLI), 'convert', str(art / (name + '.pcraft')), str(art / (name + '.png'))], check=True, capture_output=True)
        final = Image.open(art / (name + '.png')).convert('RGBA')
        flat = Image.new('RGB', final.size); flat.paste(final, mask=final.getchannel('A'))
        flat.save(art / (name + '-device.png'))
        info = json.loads((art / (name + '-receipt.json')).read_text())
        info.update(glow_removed=True, glow_refinement=records, png_sha256=fingerprint(art / (name + '.png')))
        (art / (name + '-receipt.json')).write_text(json.dumps(info, ensure_ascii=False, indent=2) + '\n')
        print(name, 'PhotoCraft glow removed; silhouette and symbols isolated', flush=True)
    contact_sheet(work, 'eclipse')


if __name__ == '__main__': main()
