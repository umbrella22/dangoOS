#!/usr/bin/env python3
"""Prepare local balance art with PhotoCraft and reviewed image completions.

Completions live in image-completion-{id}/*.png. The default completion uses
the 1254px fox master; family completions use the device image letterboxed to
a square. Only the paw occlusion masks are imported from generated images.
Original source masters and other expression images are never overwritten.
"""
import argparse
import importlib.util
import json
import math
import shutil
import subprocess
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

PROJECT = Path(__file__).resolve().parents[2]
HANDS = {
    'default': [(82, 257, 137, 315), (212, 257, 268, 315)],
    'eclipse': [(91, 286, 140, 333), (211, 286, 262, 333)],
    'normal': [(93, 289, 141, 334), (211, 289, 260, 334)],
    'black-blue': [(89, 287, 140, 334), (214, 287, 262, 334)],
}


def execute(binary, source, commands, destination):
    args = [str(binary), 'run', str(source)]
    for command, params in commands:
        args.extend(['--cmd', command, '--params', json.dumps(params)])
    args.extend(['--out', str(destination)])
    result = subprocess.run(args, check=True, capture_output=True, text=True)
    return [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--photocraft', type=Path, required=True)
    parser.add_argument('--completions', type=Path, required=True)
    parser.add_argument('--expressions', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    spec = importlib.util.spec_from_file_location('fox_build', PROJECT / 'design/fox-dango/tools/fox_build.py')
    fox = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fox)
    geometry = fox.geometry()
    receipts = []
    for identity, boxes in HANDS.items():
        directory = (args.output / identity).resolve()
        directory.mkdir(parents=True, exist_ok=True)
        source = PROJECT / ('design/fox-dango/device/poses/idle.png' if identity == 'default' else f'design/dango-family/device/{identity}/idle.png')
        original = Image.open(source).convert('RGB')
        completion_path = next((args.completions / ('image-completion-' + identity)).glob('*.png'))
        completion = Image.open(completion_path).convert('RGB')
        completion = fox.place(completion.resize((1254, 1254), Image.Resampling.LANCZOS), geometry) if identity == 'default' else completion.resize((352, 352), Image.Resampling.LANCZOS).crop((0, 0, 352, 336))
        repair = Image.new('L', original.size)
        draw = ImageDraw.Draw(repair)
        for x0, y0, x1, y1 in boxes:
            draw.ellipse((x0-3, y0-3, x1+3, y1+3), fill=255)
        if identity == 'default':
            # Join the reconstructed lower jaw between the occlusions. A pair
            # of isolated circular repairs otherwise leaves a stepped chin.
            draw.rectangle((91, 283, 260, 321), fill=255)
        repair = repair.filter(ImageFilter.GaussianBlur(1.2))
        body = Image.composite(completion, original, repair)
        body.save(directory / 'body.png')
        repair.save(directory / 'repair-mask.png')
        commands = [('layer.new.layerFromBackground', {}), ('layer.renameLayer', {'name': 'original-reference'})]
        rig = {'source': str(source.relative_to(PROJECT)), 'model': 'gpt-image-2.5-sunburst'}
        for name, box in zip(('left', 'right'), boxes):
            x0, y0, x1, y1 = box
            cx, cy, rx, ry = (x0+x1)/2, (y0+y1)/2, (x1-x0)/2, (y1-y0)/2
            points = [[round(cx+rx*math.cos(n*math.tau/64), 3), round(cy+ry*math.sin(n*math.tau/64), 3)] for n in range(64)]
            commands.extend([
                ('layer.select', {'name': 'original-reference'}),
                ('select.lasso', {'points': points, 'polygonal': True, 'antiAlias': True}),
                ('layer.new.layerViaCopy', {}),
                ('layer.renameLayer', {'name': name}),
                ('select.deselect', {}),
            ])
            rig[name] = {'selection': points, 'pivot': [round(cx), y0+8]}
        commands.extend([
            ('layer.select', {'name': 'original-reference'}),
            ('layer.hideLayers', {}),
            ('file.export.layersToFiles', {'dir': str(directory / 'exports'), 'format': 'png', 'visibleOnly': True, 'trim': False}),
        ])
        receipt = execute(args.photocraft, source, commands, directory / 'selections.pcraft')
        for name in ('left', 'right'):
            export = next((directory / 'exports').glob('*_' + name + '.png'))
            shutil.copyfile(export, directory / (name + '.png'))
        expression_path = next((args.expressions / ('expressions-' + identity)).glob('*.png'))
        board = Image.open(expression_path).convert('RGB')
        cell = board.width // 2, board.height // 2
        face_mask = Image.new('L', (352, 336))
        fd = ImageDraw.Draw(face_mask)
        face_box = (88, 181, 264, 308) if identity == 'default' else (107, 230, 246, 324)
        fd.rounded_rectangle(face_box, radius=18, fill=255)
        face_mask = face_mask.filter(ImageFilter.GaussianBlur(2.0))
        for index, name in enumerate(('strain', 'fall', 'land', 'relief')):
            x, y = index % 2 * cell[0], index // 2 * cell[1]
            face = board.crop((x, y, x+cell[0], y+cell[1])).resize((352,352),Image.Resampling.LANCZOS).crop((0,0,352,336)).convert('RGBA')
            face = Image.composite(face.convert('RGB'), body, face_mask).convert('RGBA')
            face.putalpha(face_mask.point(lambda value: 255 if value else 0))
            face.save(directory / (name+'.png'))
        rig['expression_source'] = str(expression_path)
        (directory / 'rig.json').write_text(json.dumps(rig, ensure_ascii=False, indent=2) + '\n')
        commands = [('layer.new.layerFromBackground', {}), ('layer.renameLayer', {'name': 'body-clean'})]
        for name in ('strain', 'fall', 'land', 'relief'):
            commands.extend([('file.placeEmbedded', {'path': str(directory / (name+'.png')), 'fit': False, 'smartObject': False, 'center': [176,168]}), ('layer.renameLayer', {'name': name}), ('layer.hideLayers', {})])
        for name in ('left', 'right'):
            commands.extend([('file.placeEmbedded', {'path': str(directory / (name+'.png')), 'fit': False, 'smartObject': False, 'center': [176, 168]}), ('layer.renameLayer', {'name': name})])
        execute(args.photocraft, directory / 'body.png', commands, directory / (identity + '-balance.pcraft'))
        subprocess.run([str(args.photocraft), 'convert', str(directory / (identity + '-balance.pcraft')), str(directory / (identity + '-balance.psd'))], check=True, capture_output=True)
        # This check concerns the deterministic splice, not a claim about the
        # provider preserving pixels outside an unmasked edit request.
        unchanged = ImageChops.difference(original, body)
        outside = ImageChops.multiply(unchanged.convert('L'), repair.point(lambda v: 255 if v == 0 else 0))
        assert outside.getbbox() is None
        receipts.append({'id': identity, 'photocraft_commands': receipt, 'completion': str(completion_path), 'outside_repair_mask_unchanged': True})
    (args.output / 'receipt.json').write_text(json.dumps(receipts, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps([{'id': r['id'], 'outside_repair_mask_unchanged': True} for r in receipts]))


if __name__ == '__main__':
    main()
