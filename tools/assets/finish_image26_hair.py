#!/usr/bin/env python3
"""Apply the requested family face shape and coherent matte hair shading."""
import argparse
import json
import shutil
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

from refresh_companion_expressions import IDS, POSES, EXTRAS, MODEL, BRIDGE, LABELS, fingerprint


def prepare(work):
    before = work / 'before-hair-art'
    if not before.exists(): shutil.copytree(work / 'art', before)
    (work / 'style.json').write_text(json.dumps({'default_geometry': 'normal', 'hair': 'coherent matte cel shading'}, indent=2))
    style = Image.open(json.loads((work / 'family-style/result.json').read_text())['file']).convert('RGB')
    font = ImageFont.truetype('/System/Library/Fonts/Supplemental/Arial.ttf', 18)
    for identity in IDS:
        folder = work / 'finishing' / identity; folder.mkdir(parents=True, exist_ok=True)
        for batch in range(5):
            names = (POSES + EXTRAS)[batch * 6:(batch + 1) * 6]
            anchor = 512 if identity == 'default' else 0
            board = Image.new('RGB', (1536, 1024 + anchor)); draw = ImageDraw.Draw(board)
            if anchor:
                im = style.copy(); im.thumbnail((610, 480))
                board.paste(im, ((1536 - im.width) // 2, 20))
                draw.text((24, 24), 'APPROVED CHARACTER DESIGN + MATTE HAIR ANCHOR. OUTPUT ONLY THE SIX EXPRESSIONS BELOW.', font=font, fill=(170, 180, 196))
            for i, name in enumerate(names):
                source = Image.open(before / identity / (name + '-device.png')).convert('RGB').resize((440, 420), Image.Resampling.LANCZOS)
                x, y = i % 3 * 512, i // 3 * 512 + anchor
                board.paste(source, (x + 36, y + 64))
                draw.text((x + 18, y + 14), f'{i + 1}. {name}', font=font, fill=(150, 160, 176))
            board.save(folder / f'{batch:02}-reference.png')
            prompt = '''Clean up the HAIR SHADING of this finished chibi anime sprite sheet. Return exactly six matching sprites, three columns by two rows on a 1536x1024 canvas, one character per 512x512 cell. Pure mathematically flat RGB(0,0,0) background. NO halo, gradient, spot light, bloom, glow, vignette, cast shadow, floor or colored background of any kind. Preserve all isolated expression symbols. No letters, numbers, captions, frame or cell boundaries. Every silhouette and symbol must stay inside its own cell with black padding.

Use clean Japanese animation cel painting. Repair incorrect highlights and smeared hair: each curved lock is a clear contiguous volume with ONE consistent soft light direction from upper left, simple broad light and shadow planes, crisp ink separation between locks. MATTE, controlled two-or-three-tone hair shading. Remove all floating white gloss spots, disconnected white scratch streaks, repeated zigzag highlight bands, cloudy smudges, soft brush swirls, blended watercolor splotches, grain and noise. Do not blur the edges. Do not erase locks or recolor the character. Highlights on eyes and the tiny metal accessory are allowed; decorative sparkles are allowed ONLY where already present. Hair itself must look clean, softly grouped and solid rather than shiny, smeary or plastic.

Keep every expression's eyes, brows, mouth, blush, ear pose, hand pose, head tilt, hair silhouette, hair accessories, symbol arrangement, colors and character identity faithful to its source cell. No new arms or body: only the round mochi head, ears, and two tiny floating round mitten paws. Keep exhausted/error/dying poses flat and wide. No finger anatomy. Angry fang is a small UPPER tooth pointing downward. Do not change sleepy eyes into X eyes. No remnant old eyelashes behind new lids. Wi-Fi and Bluetooth glyphs must stay distinct and accurate.
'''
            if identity == 'default':
                prompt += '''
SPECIAL DEFAULT FOX UPDATE: The input's TOP ROW is a DESIGN ANCHOR, not an output cell. Draw the six LOWER reference expressions using exactly that anchor's revised character design, softly grouped curved hair and matte finish. Change the six old tall/pointy fox faces to the anchor's wider, lower, round mochi face so they belong to the same family as the other chibi girls. Short wide soft cheeks and a small curved jaw; eyes/face proportions like the family characters. Keep the white-silver/lavender hair, red flame forehead lock, gold-red diamond ornament with gold teardrop, red-to-gold irises and diamond pupils. Never add a maid headband or another character's ribbons. The LOWER references give ONLY the expression, tilt, ear/hand pose and symbols; the TOP anchor gives the final face/hair design. Output exactly THREE columns TWO rows, not the input's three-row layout. More round and horizontally generous, less tall and spiky.
'''
            else:
                prompt += '\nDo not redesign the face or hairstyle. This is a precise paint correction of the six existing sprites; change only the messy or inconsistent hair shading, and preserve everything else.\n'
            prompt += '\nOutput order, left to right, top to bottom:\n' + '\n'.join(f'{i + 1}. {name} / {LABELS[name]}' for i, name in enumerate(names))
            prompt += '\nFor clean-face keep skin completely blank (no eyes/brows/mouth). For blink retain only closed lids. For effort only image-left eye closed. For dizzy remove normal eyes completely and keep clean spirals. For weary mildly resigned, not angry. No glowing background anywhere.\n'
            (folder / f'{batch:02}-prompt.txt').write_text(prompt)


def generate(work, identity, batch):
    folder = work / 'finishing' / identity
    result = folder / f'{batch:02}-result.json'
    if result.exists(): return
    started = folder / f'{batch:02}-started.json'
    if started.exists(): raise RuntimeError('Unconfirmed finishing request; inspect before retrying')
    reference = folder / f'{batch:02}-reference.png'
    started.write_text(json.dumps({'model': MODEL, 'reference_sha256': fingerprint(reference)}))
    proc = subprocess.run([sys.executable, str(BRIDGE), '--mode', 'edit', '--model', MODEL,
        '--size', '4:3', '--image', str(reference), '--output-dir', str(folder / f'{batch:02}-output'),
        (folder / f'{batch:02}-prompt.txt').read_text()], capture_output=True, text=True)
    (folder / f'{batch:02}-response.log').write_text(proc.stdout + proc.stderr)
    if proc.returncode: raise RuntimeError(f'{identity} finishing {batch} failed; inspect response log')
    receipt = json.loads(proc.stdout)
    if not receipt.get('file'): raise RuntimeError('No local finishing image')
    receipt['sha256'] = fingerprint(Path(receipt['file']))
    receipt['expressions'] = (POSES + EXTRAS)[batch * 6:(batch + 1) * 6]
    result.write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + '\n')
    print(identity, batch, receipt['file'], flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['prepare', 'generate'])
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--id', choices=IDS)
    parser.add_argument('--batch', type=int, choices=range(5))
    args = parser.parse_args(); work = args.work.resolve()
    if args.command == 'prepare': prepare(work)
    else:
        if args.id is None or args.batch is None: parser.error('generate requires --id and --batch')
        generate(work, args.id, args.batch)


if __name__ == '__main__': main()
