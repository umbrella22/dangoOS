#!/usr/bin/env python3
"""Replace generated connection glyphs with editable canonical PhotoCraft paths."""
import argparse
import json
import math
import subprocess
from pathlib import Path

import cv2
import numpy as np
from PIL import Image

from refresh_companion_expressions import IDS, fingerprint
from refine_image26_art import CLI, foreground, contact_sheet
from refine_articulated_faces import execute, stroke


def erase_attached_badge(identity):
    """Remove model-painted status badges that touch the ear contour."""
    cx, cy, radius = (227, 96, 16) if identity == 'eclipse' else (225, 92, 16)
    circle = [(cx + radius * math.cos(t), cy + radius * math.sin(t))
              for t in np.linspace(0, math.tau, 81)]
    commands = [
        ('path.set', {'name': 'old-badge-removal', 'path': {'subpaths': [{'closed': True, 'knots': circle}]}}),
        ('path.toSelection', {'name': 'old-badge-removal', 'antiAlias': False}),
        ('paint.stroke', {'points': [[cx, cy]], 'size': 36, 'hardness': 1,
                          'opacity': 1, 'flow': 1, 'erase': True}),
    ]
    if identity == 'eclipse':
        # Only the red sliver hidden by the badge is reconstructed. The local
        # selection protects all existing hair, fur and the rest of the ear.
        commands += [
            ('path.set', {'name': 'repaired-ear-edge', 'path': {'subpaths': [{
                'closed': True, 'knots': [[246, 79], [247, 116], [209, 116]]}]}}),
            ('path.fill', {'name': 'repaired-ear-edge', 'color': '#cb4a65'}),
        ]
        commands += stroke('ear-outline-repair', [(246, 79), (209, 116)], '#773042', 1.1)
    commands.append(('select.deselect', {}))
    return commands


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--work', type=Path, required=True)
    args = parser.parse_args(); work = args.work.resolve()
    folder = work / 'connection-icons'; folder.mkdir(exist_ok=True)
    blank = folder / 'transparent.png'; Image.new('RGBA', (352, 336)).save(blank)
    for identity in IDS:
        art = work / 'art' / identity
        for protocol in ['network', 'bluetooth']:
            for state in ['disconnected', 'connected']:
                name = protocol + '-' + state
                # Always start at the registered model cell. Re-running icon
                # refinement must not compound masks or include our own glyphs.
                source = Image.open(art / (name + '-registered.png')).convert('RGBA')
                mask = foreground(source)
                contours, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
                main = max(contours, key=cv2.contourArea)
                keep = np.zeros(mask.shape, np.uint8)
                cv2.drawContours(keep, [main], -1, 255, -1)
                for c in contours:
                    x, y, w, h = cv2.boundingRect(c)
                    if y > 235 and cv2.contourArea(c) > 250: cv2.drawContours(keep, [c], -1, 255, -1)
                keep = cv2.dilate(keep, np.ones((3, 3), np.uint8))
                alpha = np.minimum(np.asarray(source.getchannel('A')), keep)
                source.putalpha(Image.fromarray(alpha))
                base = folder / (identity + '-' + name + '-body.png'); source.save(base)
                connected = state == 'connected'
                color = '#86baff' if connected else '#c2bad9'
                commands = [('layer.renameLayer', {'name': 'character'})]
                if protocol == 'bluetooth' and state == 'disconnected' and identity in ('eclipse', 'normal'):
                    commands += erase_attached_badge(identity)
                commands += [
                    ('file.placeEmbedded', {'path': str(blank), 'fit': False, 'smartObject': False, 'center': [176, 168]}),
                    ('layer.renameLayer', {'name': protocol + '-symbol'})]
                if protocol == 'bluetooth':
                    # Standard Bluetooth rune: central stem and two right-side
                    # triangular lobes with the two left diagonal terminals.
                    for n, points in enumerate([
                        [(176, 26), (176, 74)],
                        [(176, 26), (192, 38), (160, 62)],
                        [(160, 38), (192, 62), (176, 74)],
                    ]): commands += stroke('bluetooth-' + str(n), points, color, 3.0)
                else:
                    for radius in [16, 29, 42]:
                        points = [(176 + radius * math.cos(t), 69 + radius * math.sin(t)) for t in np.linspace(-2.30, -.84, 48)]
                        commands += stroke('wifi-' + str(radius), points, color, 3.0)
                    commands += stroke('wifi-dot', [(176, 69), (176.1, 69)], color, 5.5)
                commands += [('file.placeEmbedded', {'path': str(blank), 'fit': False, 'smartObject': False, 'center': [176, 168]}),
                    ('layer.renameLayer', {'name': 'connection-' + state})]
                points = [(218 + 10 * math.cos(t), 62 + 10 * math.sin(t)) for t in np.linspace(0, math.tau, 65)]
                commands += [('path.set', {'name': 'status-badge', 'path': {'subpaths': [{'closed': True, 'knots': points}]}}),
                    ('path.fill', {'name': 'status-badge', 'color': '#fff5fa'})]
                paths = [[(213, 62), (217, 66), (224, 57)]] if connected else [[(214, 58), (222, 66)], [(222, 58), (214, 66)]]
                for n, points in enumerate(paths): commands += stroke('status-mark-' + str(n), points, '#487fca' if connected else '#c64d69', 2.3)
                records = execute(CLI, base, commands, art / (name + '.pcraft'))
                subprocess.run([str(CLI), 'convert', str(art / (name + '.pcraft')), str(art / (name + '.png'))], check=True, capture_output=True)
                subprocess.run([str(CLI), 'convert', str(art / (name + '.pcraft')), str(folder / (identity + '-' + name + '.psd'))], check=True, capture_output=True)
                final = Image.open(art / (name + '.png')).convert('RGBA')
                flat = Image.new('RGB', final.size); flat.paste(final, mask=final.getchannel('A'))
                flat.save(art / (name + '-device.png'))
                record = json.loads((art / (name + '-receipt.json')).read_text())
                record.update(canonical_icon=protocol, connection_state=state, icon_refinement=records,
                              png_sha256=fingerprint(art / (name + '.png')))
                (art / (name + '-receipt.json')).write_text(json.dumps(record, ensure_ascii=False, indent=2) + '\n')
                print(identity, name, 'canonical icon and independent status layer', flush=True)
        contact_sheet(work, identity)


if __name__ == '__main__': main()
